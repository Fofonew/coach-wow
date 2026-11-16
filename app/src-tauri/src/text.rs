use regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

pub const DEFAULT_SCREEN_KEYWORDS: [&str; 22] = [
    "écran", "ecran", "screen", "regarde", "tu vois", "vois-tu", "ce que je vois", "capture",
    "cet objet", "cet item", "cette quête", "ce mob", "ce boss", "ce pnj", "ce sort", "cette carte",
    "infobulle", "tooltip", "c'est quoi ça", "c'est quoi ce", "c'est quoi cette", "ça c'est quoi",
];

static SENTENCE_BOUNDARY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?P<p>[.!?…]+)\s+|\n+").unwrap());
static MARKDOWN_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\([^)]*\)").unwrap());
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://\S+|www\.\S+").unwrap());
static LIST_MARKER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*(?:[-*•]|\d+[.)])\s+").unwrap());
static MARKDOWN_SYMBOLS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[*_`#>|~]+").unwrap());
static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static HALLUCINATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)amara\.org|sous-titr|merci d'avoir regard|abonnez-vous|thanks for watching").unwrap()
});

pub fn clean_for_speech(text: &str) -> String {
    let text = MARKDOWN_LINK.replace_all(text, "$1");
    let text = URL.replace_all(&text, "");
    let text = LIST_MARKER.replace_all(&text, "");
    let text = MARKDOWN_SYMBOLS.replace_all(&text, "");
    let text = WHITESPACE.replace_all(&text, " ");
    text.trim().to_string()
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub struct SentenceSplitter {
    buffer: String,
    max_bytes: usize,
}

impl Default for SentenceSplitter {
    fn default() -> Self {
        Self { buffer: String::new(), max_bytes: 260 }
    }
}

impl SentenceSplitter {
    pub fn feed(&mut self, text: &str) -> Vec<String> {
        self.buffer.push_str(text);
        let mut sentences = Vec::new();
        loop {
            let (piece, rest_start) = if let Some(captures) = SENTENCE_BOUNDARY.captures(&self.buffer) {
                let whole = captures.get(0).unwrap();
                let end = captures.name("p").map(|punctuation| punctuation.end()).unwrap_or(whole.start());
                (self.buffer[..end].to_string(), whole.end())
            } else if self.buffer.len() > self.max_bytes {
                let limit = floor_char_boundary(&self.buffer, self.max_bytes);
                let cut = self.buffer[..limit]
                    .rfind(", ")
                    .map(|index| index + 1)
                    .or_else(|| self.buffer[..limit].rfind(' '))
                    .unwrap_or(limit);
                let cut = floor_char_boundary(&self.buffer, cut.max(1));
                (self.buffer[..cut].to_string(), cut)
            } else {
                break;
            };
            self.buffer.drain(..rest_start);
            push_sentence(&mut sentences, &piece);
        }
        sentences
    }

    pub fn flush(&mut self) -> Vec<String> {
        let mut sentences = Vec::new();
        let rest = std::mem::take(&mut self.buffer);
        push_sentence(&mut sentences, &rest);
        sentences
    }
}

fn push_sentence(sentences: &mut Vec<String>, piece: &str) {
    let cleaned = clean_for_speech(piece);
    if cleaned.chars().any(|character| character.is_alphanumeric()) {
        sentences.push(cleaned);
    }
}

/// Removes fenced code blocks (macros, talent strings, tables) from a streamed answer
/// so that only the conversational part is spoken.
#[derive(Default)]
pub struct SpeechFilter {
    full: String,
    spoken_len: usize,
}

impl SpeechFilter {
    pub fn push(&mut self, chunk: &str) -> String {
        self.full.push_str(chunk);
        self.take(false)
    }

    pub fn finish(&mut self) -> String {
        self.take(true)
    }

    fn take(&mut self, finished: bool) -> String {
        let speakable = speakable_prefix(&self.full, finished);
        if speakable.len() <= self.spoken_len {
            return String::new();
        }
        let delta = speakable[self.spoken_len..].to_string();
        self.spoken_len = speakable.len();
        delta
    }
}

fn speakable_prefix(text: &str, finished: bool) -> String {
    let mut output = String::new();
    let mut rest = text;
    loop {
        match rest.find("```") {
            Some(open) => {
                output.push_str(&rest[..open]);
                output.push('\n');
                let after = &rest[open + 3..];
                match after.find("```") {
                    Some(close) => rest = &after[close + 3..],
                    None => return output,
                }
            }
            None => {
                if finished {
                    output.push_str(rest);
                } else {
                    output.push_str(rest.trim_end_matches('`'));
                }
                return output;
            }
        }
    }
}

pub fn normalize(text: &str) -> String {
    text.to_lowercase()
        .replace('’', "'")
        .nfkd()
        .filter(|character| !unicode_normalization::char::is_combining_mark(*character))
        .collect()
}

pub struct KeywordMatcher {
    pattern: Option<Regex>,
}

impl KeywordMatcher {
    pub fn new(keywords: &[String]) -> Self {
        let cleaned: Vec<String> = keywords
            .iter()
            .map(|keyword| normalize(keyword).trim().to_string())
            .filter(|keyword| !keyword.is_empty())
            .map(|keyword| regex::escape(&keyword))
            .collect();
        let pattern = if cleaned.is_empty() {
            None
        } else {
            Regex::new(&format!(r"(?:^|[^\w])(?:{})(?:$|[^\w])", cleaned.join("|"))).ok()
        };
        Self { pattern }
    }

    pub fn matches(&self, question: &str) -> bool {
        self.pattern.as_ref().is_some_and(|pattern| pattern.is_match(&normalize(question)))
    }
}

pub fn is_hallucination(text: &str, initial_prompt: &str) -> bool {
    let trimmed = text.trim().trim_matches('.').trim();
    if trimmed.is_empty() || HALLUCINATION.is_match(trimmed) {
        return true;
    }
    !initial_prompt.is_empty() && initial_prompt.to_lowercase().contains(&trimmed.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_sentences() {
        let mut splitter = SentenceSplitter::default();
        let mut sentences = splitter.feed("Bonjour. Va à Orgrimmar");
        sentences.extend(splitter.feed(" ! Ensuite"));
        sentences.extend(splitter.flush());
        assert_eq!(sentences, vec!["Bonjour.", "Va à Orgrimmar !", "Ensuite"]);
    }

    #[test]
    fn filters_code_blocks() {
        let mut filter = SpeechFilter::default();
        let mut spoken = filter.push("Voici ta macro.\n``");
        spoken += &filter.push("`macro Soin\n/cast Soins\n``");
        spoken += &filter.push("`\nGlisse-la sur ta barre.");
        spoken += &filter.finish();
        assert!(!spoken.contains("/cast"));
        assert!(spoken.contains("Glisse-la"));
    }

    #[test]
    fn matches_keywords() {
        let keywords: Vec<String> = DEFAULT_SCREEN_KEYWORDS.iter().map(|keyword| keyword.to_string()).collect();
        let matcher = KeywordMatcher::new(&keywords);
        assert!(matcher.matches("Regarde mon écran"));
        assert!(matcher.matches("c’est quoi ça ?"));
        assert!(!matcher.matches("Où est le maître de cuisine ?"));
    }
}
