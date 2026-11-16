//! Small, tolerant HTML to Markdown converter for the official pages kept in the knowledge
//! base (Blizzard news articles and forum posts). It keeps headings, nested lists, quotes,
//! table rows and links, and drops everything else.

/// Tags whose whole content is dropped.
const SKIPPED: [&str; 10] = ["script", "style", "template", "svg", "noscript", "iframe", "head", "button", "select", "video"];

#[derive(Default)]
struct Writer {
    out: String,
    /// Line breaks owed before the next text: 1 = new line, 2 = new paragraph.
    breaks: u8,
    line_has_text: bool,
    space_pending: bool,
    /// One entry per open list: `None` for bullets, the next number for ordered lists.
    lists: Vec<Option<u32>>,
    item_pending: bool,
    heading_pending: Option<u8>,
    quote: usize,
    bold: usize,
    in_paragraph: bool,
    paragraph_start: Option<usize>,
    paragraph_has_plain: bool,
    paragraph_has_bold: bool,
    link: Option<(String, Option<usize>)>,
}

impl Writer {
    fn request_break(&mut self, count: u8) {
        self.breaks = self.breaks.max(count);
        self.space_pending = false;
        self.line_has_text = false;
    }

    fn begin_line(&mut self) {
        if !self.out.is_empty() {
            for _ in 0..self.breaks.max(1) {
                self.out.push('\n');
            }
        }
        self.breaks = 0;
        for _ in 0..self.quote {
            self.out.push_str("> ");
        }
        if self.item_pending {
            self.item_pending = false;
            for _ in 1..self.lists.len() {
                self.out.push_str("  ");
            }
            match self.lists.last_mut() {
                Some(Some(number)) => {
                    self.out.push_str(&format!("{number}. "));
                    *number += 1;
                }
                _ => self.out.push_str("- "),
            }
        } else {
            for _ in 0..self.lists.len() {
                self.out.push_str("  ");
            }
        }
        if let Some(level) = self.heading_pending.take() {
            for _ in 0..level {
                self.out.push('#');
            }
            self.out.push(' ');
        }
    }

    fn text(&mut self, raw: &str) {
        let decoded = decode_entities(raw);
        for character in decoded.chars() {
            if character.is_whitespace() {
                self.space_pending = self.line_has_text;
                continue;
            }
            if !self.line_has_text {
                self.begin_line();
                self.line_has_text = true;
            } else if self.space_pending {
                self.out.push(' ');
            }
            self.space_pending = false;
            if self.in_paragraph {
                self.paragraph_start.get_or_insert(self.out.len());
                if self.bold > 0 {
                    self.paragraph_has_bold = true;
                } else {
                    self.paragraph_has_plain = true;
                }
            }
            if let Some((_, start)) = self.link.as_mut() {
                start.get_or_insert(self.out.len());
            }
            self.out.push(character);
        }
    }

    fn open(&mut self, name: &str, tag: &str) {
        match name {
            "p" => {
                self.request_break(if self.lists.is_empty() { 2 } else { 1 });
                self.in_paragraph = true;
                self.paragraph_start = None;
                self.paragraph_has_plain = false;
                self.paragraph_has_bold = false;
            }
            "div" | "section" | "article" | "tr" | "summary" | "figcaption" | "header" | "footer" | "dt" | "dd" => self.request_break(1),
            "table" | "details" | "pre" | "figure" => self.request_break(2),
            "br" => self.request_break(1),
            "hr" => {
                self.request_break(2);
                self.text("---");
                self.request_break(2);
            }
            "ul" | "ol" => {
                self.request_break(if self.lists.is_empty() { 2 } else { 1 });
                self.lists.push((name == "ol").then_some(1));
            }
            "li" => {
                self.request_break(1);
                self.item_pending = true;
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.request_break(2);
                let level = name.as_bytes()[1] - b'0';
                self.heading_pending = Some(level.clamp(1, 4));
            }
            "blockquote" | "aside" => {
                self.request_break(2);
                self.quote += 1;
            }
            "td" | "th" => {
                if self.line_has_text {
                    self.out.push_str(" | ");
                    self.space_pending = false;
                }
            }
            "strong" | "b" => self.bold += 1,
            "a" => {
                self.link = attribute(tag, "href").filter(|href| href.starts_with("http://") || href.starts_with("https://")).map(|href| (href, None));
            }
            _ => {}
        }
    }

    fn close(&mut self, name: &str) {
        match name {
            "p" => {
                if self.in_paragraph && self.paragraph_has_bold && !self.paragraph_has_plain && self.lists.is_empty() && self.quote == 0 {
                    // A paragraph made only of bold text is a section title in Blizzard's notes.
                    if let Some(start) = self.paragraph_start {
                        self.out.insert_str(start, "### ");
                    }
                }
                self.in_paragraph = false;
                self.request_break(if self.lists.is_empty() { 2 } else { 1 });
            }
            "div" | "section" | "article" | "tr" | "summary" | "figcaption" | "header" | "footer" | "li" | "dt" | "dd" => self.request_break(1),
            "table" | "details" | "pre" | "figure" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.heading_pending = None;
                self.request_break(2);
            }
            "ul" | "ol" => {
                self.lists.pop();
                self.item_pending = false;
                self.request_break(if self.lists.is_empty() { 2 } else { 1 });
            }
            "blockquote" | "aside" => {
                self.quote = self.quote.saturating_sub(1);
                self.request_break(2);
            }
            "strong" | "b" => self.bold = self.bold.saturating_sub(1),
            "a" => {
                if let Some((href, Some(start))) = self.link.take() {
                    if self.out[start..] != href {
                        self.out.insert(start, '[');
                        self.out.push_str(&format!("]({href})"));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Index of the `>` that ends the tag starting at `html[0]`, ignoring `>` inside quotes.
fn tag_end(html: &str) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (index, byte) in html.bytes().enumerate() {
        match (quote, byte) {
            (Some(open), _) if byte == open => quote = None,
            (None, b'"') | (None, b'\'') => quote = Some(byte),
            (None, b'>') => return Some(index),
            _ => {}
        }
    }
    None
}

/// Value of an attribute inside a tag (`a href="…" class="…"`).
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search = 0;
    while let Some(found) = lower[search..].find(name) {
        let start = search + found;
        let after = start + name.len();
        search = after;
        let boundary = start == 0 || lower.as_bytes()[start - 1].is_ascii_whitespace();
        let rest = tag[after..].trim_start();
        if !boundary || !rest.starts_with('=') {
            continue;
        }
        let value = rest[1..].trim_start();
        let value = match value.chars().next() {
            Some(quote @ ('"' | '\'')) => value[1..].split(quote).next().unwrap_or_default(),
            _ => value.split(|character: char| character.is_whitespace() || character == '>').next().unwrap_or_default(),
        };
        return Some(decode_entities(value));
    }
    None
}

pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        output.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest[1..].find(|character: char| !(character.is_ascii_alphanumeric() || character == '#')).map(|index| index + 1);
        let decoded = match end {
            Some(end) if rest[end..].starts_with(';') => entity(&rest[1..end]).map(|character| (character, end + 1)),
            _ => None,
        };
        match decoded {
            Some((character, consumed)) => {
                output.push(character);
                rest = &rest[consumed..];
            }
            None => {
                output.push('&');
                rest = &rest[1..];
            }
        }
    }
    output.push_str(rest);
    output
}

fn entity(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hexadecimal) => u32::from_str_radix(hexadecimal, 16).ok()?,
            None => number.parse().ok()?,
        };
        return char::from_u32(code);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "rsquo" | "lsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "bull" | "middot" => '•',
        "times" => '×',
        "deg" => '°',
        "eacute" => 'é',
        "egrave" => 'è',
        "ecirc" => 'ê',
        "agrave" => 'à',
        "acirc" => 'â',
        "ccedil" => 'ç',
        "ugrave" => 'ù',
        "ucirc" => 'û',
        "icirc" => 'î',
        "ocirc" => 'ô',
        "oelig" => 'œ',
        _ => return None,
    })
}

/// Text of an HTML fragment without any markup, on one line.
pub fn plain_text(html: &str) -> String {
    to_markdown(html).split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn to_markdown(html: &str) -> String {
    let mut writer = Writer::default();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        writer.text(&rest[..start]);
        rest = &rest[start..];
        if rest.starts_with("<!--") {
            rest = rest.find("-->").map(|end| &rest[end + 3..]).unwrap_or("");
            continue;
        }
        let is_tag = rest[1..].chars().next().is_some_and(|character| character.is_ascii_alphabetic() || character == '/' || character == '!' || character == '?');
        let Some(end) = is_tag.then(|| tag_end(rest)).flatten() else {
            writer.text("<");
            rest = &rest[1..];
            continue;
        };
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric() || *character == '-')
            .collect::<String>()
            .to_ascii_lowercase();
        if closing {
            writer.close(&name);
            continue;
        }
        if SKIPPED.contains(&name.as_str()) && !tag.ends_with('/') {
            let lower = rest.to_ascii_lowercase();
            let close_tag = format!("</{name}");
            let open_tag = format!("<{name}");
            let mut depth = 1;
            let mut position = 0;
            while depth > 0 {
                let next_close = lower[position..].find(&close_tag).map(|index| index + position);
                let next_open = lower[position..].find(&open_tag).map(|index| index + position);
                match (next_close, next_open) {
                    (Some(close), Some(open)) if open < close => {
                        depth += 1;
                        position = open + open_tag.len();
                    }
                    (Some(close), _) => {
                        depth -= 1;
                        position = close + close_tag.len();
                    }
                    (None, _) => {
                        position = lower.len();
                        break;
                    }
                }
            }
            rest = &rest[position..];
            rest = rest.find('>').map(|end| &rest[end + 1..]).unwrap_or("");
            continue;
        }
        writer.open(&name, tag);
    }
    writer.text(rest);
    writer.out.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_patch_notes() {
        let html = "<p>Here you will find a list of hotfixes.</p>\n<h2><strong>Hotfixes</strong></h2>\n<hr />\n\
            <p><strong><span>November 11, 2026</span></strong></p>\n<p><strong>Classes</strong></p>\n\
            <ul>\n\t<li><strong>Druid</strong>\n\t<ul>\n\t\t<li><strong>Feral</strong>\n\t\t<ul>\n\t\t\t<li>Rend &amp; Tear now works on Pounce&nbsp;Bleed.</li>\n\t\t</ul>\n\t\t</li>\n\t</ul>\n\t</li>\n\
            \t<li>Fixed an issue where <em>Volley</em> was activating procs.</li>\n</ul>";
        let expected = "Here you will find a list of hotfixes.\n\n## Hotfixes\n\n---\n\n### November 11, 2026\n\n### Classes\n\n\
            - Druid\n  - Feral\n    - Rend & Tear now works on Pounce Bleed.\n- Fixed an issue where Volley was activating procs.";
        assert_eq!(to_markdown(html), expected);
    }

    #[test]
    fn keeps_links_quotes_and_tables_and_drops_scripts() {
        let html = "<blz-image><template shadowroot=\"open\"><style>.a > .b { color: red; }</style><img src=\"x.png\"></template></blz-image>\
            <p>Learn more in <a href=\"https://worldofwarcraft.blizzard.com/news/24307383\">our article</a> or <a href=\"/t/1\">here</a>.</p>\
            <blockquote><p>Quoted line</p></blockquote>\
            <table><tbody><tr><td>Classique</td><td>Am&eacute;lior&#233;</td></tr><tr><td>1 &lt; 2</td><td>ok</td></tr></tbody></table>\
            <ol><li>Un</li><li>Deux<br>suite</li></ol><script>var a = '<p>no</p>';</script>";
        let markdown = to_markdown(html);
        assert_eq!(
            markdown,
            "Learn more in [our article](https://worldofwarcraft.blizzard.com/news/24307383) or here.\n\n> Quoted line\n\n\
             Classique | Amélioré\n1 < 2 | ok\n\n1. Un\n2. Deux\n  suite"
        );
        assert_eq!(plain_text("WoW: Forever&mdash;What&rsquo;s <span>New</span>\n"), "WoW: Forever—What’s New");
    }
}
