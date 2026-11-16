use serde::{Deserialize, Serialize};
use std::path::Path;

pub const ACTIONS: [&str; 5] = ["push_to_talk", "push_to_talk_with_screenshot", "screenshot", "stop", "new_session"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Bindings {
    pub push_to_talk: String,
    pub push_to_talk_with_screenshot: String,
    pub screenshot: String,
    pub stop: String,
    pub new_session: String,
}

impl Default for Bindings {
    fn default() -> Self {
        Self {
            push_to_talk: "mouse:x2".into(),
            push_to_talk_with_screenshot: String::new(),
            screenshot: String::new(),
            stop: "key:pause".into(),
            new_session: String::new(),
        }
    }
}

impl Bindings {
    pub fn get(&self, action: &str) -> &str {
        match action {
            "push_to_talk" => &self.push_to_talk,
            "push_to_talk_with_screenshot" => &self.push_to_talk_with_screenshot,
            "screenshot" => &self.screenshot,
            "stop" => &self.stop,
            "new_session" => &self.new_session,
            _ => "",
        }
    }

    pub fn actions_for(&self, input: &str) -> Vec<&'static str> {
        ACTIONS.iter().copied().filter(|action| !input.is_empty() && self.get(action) == input).collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub onboarded: bool,
    pub language: String,
    pub game: String,
    pub preferred_sources: Vec<String>,
    pub bindings: Bindings,
    pub interrupt_on_push_to_talk: bool,
    pub release_tail_ms: u64,
    pub min_recording_seconds: f32,
    pub input_device: String,
    pub output_device: String,
    pub volume: f32,
    pub beeps: bool,
    pub beep_volume: f32,
    pub speak_answers: bool,
    pub whisper_model: String,
    pub use_gpu: bool,
    pub initial_prompt: String,
    pub voice: String,
    pub voice_speed: f32,
    pub claude_executable: String,
    pub claude_model: String,
    pub claude_effort: String,
    /// "subscription" (Claude Pro/Max login) or "api_key" (pay as you go).
    pub auth_mode: String,
    /// API key encrypted with Windows DPAPI.
    pub api_key_protected: String,
    pub screenshot_auto: bool,
    pub screenshot_keywords: Vec<String>,
    pub screenshot_max_width: u32,
    pub screenshot_armed_seconds: u64,
    pub wow_folder: String,
    pub share_character: bool,
    pub always_on_top: bool,
    /// Small click-through indicator over the game (listening, thinking, speaking).
    pub overlay_enabled: bool,
    /// "top", "top-left", "top-right" or "bottom".
    pub overlay_position: String,
    /// Keep a local copy of the official patch notes and developer posts for the coach.
    pub knowledge_enabled: bool,
}

/// Words that help the speech recognition with the names of the game.
const DEFAULT_VOCABULARY: &str = "Questions sur World of Warcraft Forever : quêtes, donjons, raids, talents, métiers, macros, équipement, Horde, Alliance, Wowhead, Éolides, île de Zéphras, correctifs, ensemble de règles.";
const LEGACY_VOCABULARY: &str = "Questions sur World of Warcraft Forever : quêtes, donjons, raids, talents, métiers, macros, équipement, Horde, Alliance, Wowhead.";

/// Sources suggested by the first versions: replaced by `default_sources` when untouched.
const LEGACY_SOURCES: [&str; 5] = ["wowhead.com", "warcrafttavern.com", "icy-veins.com", "news.blizzard.com", "the official Blizzard forums"];

fn default_sources() -> Vec<String> {
    [
        "wowhead.com/forever (the WoW Forever section of Wowhead: database, guides, news)",
        "news.blizzard.com",
        "the official forums us.forums.blizzard.com/en/wow/c/wow-forever",
        "icy-veins.com/wow-forever",
        "method.gg/wow-forever",
    ]
    .iter()
    .map(|source| source.to_string())
    .collect()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            onboarded: false,
            language: "fr".into(),
            game: "World of Warcraft: Forever".into(),
            preferred_sources: default_sources(),
            bindings: Bindings::default(),
            interrupt_on_push_to_talk: true,
            release_tail_ms: 200,
            min_recording_seconds: 0.35,
            input_device: String::new(),
            output_device: String::new(),
            volume: 1.0,
            beeps: true,
            beep_volume: 1.0,
            speak_answers: true,
            whisper_model: "large-v3-turbo".into(),
            use_gpu: true,
            initial_prompt: DEFAULT_VOCABULARY.into(),
            voice: "fr-FR-VivienneMultilingualNeural".into(),
            voice_speed: 1.05,
            claude_executable: String::new(),
            claude_model: "sonnet".into(),
            claude_effort: "low".into(),
            auth_mode: "subscription".into(),
            api_key_protected: String::new(),
            screenshot_auto: true,
            screenshot_keywords: crate::text::DEFAULT_SCREEN_KEYWORDS.iter().map(|keyword| keyword.to_string()).collect(),
            screenshot_max_width: 1600,
            screenshot_armed_seconds: 90,
            wow_folder: String::new(),
            share_character: true,
            always_on_top: false,
            overlay_enabled: true,
            overlay_position: "top".into(),
            knowledge_enabled: true,
        }
    }
}

impl Settings {
    pub fn uses_api_key(&self) -> bool {
        self.auth_mode == "api_key"
    }

    /// The decrypted API key when the API key mode is selected.
    pub fn api_key(&self) -> Option<String> {
        if !self.uses_api_key() {
            return None;
        }
        crate::secret::unprotect(&self.api_key_protected).ok()
    }

    pub fn load(path: &Path) -> Self {
        let mut settings: Self = match std::fs::read_to_string(path) {
            // Editors such as Windows PowerShell add a UTF-8 byte order mark: ignore it.
            Ok(text) => serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_else(|error| {
                log::warn!("Unreadable settings file, using defaults: {error}");
                Self::default()
            }),
            Err(_) => Self::default(),
        };
        if settings.preferred_sources.iter().map(String::as_str).eq(LEGACY_SOURCES) {
            settings.preferred_sources = default_sources();
        }
        if settings.initial_prompt == LEGACY_VOCABULARY {
            settings.initial_prompt = DEFAULT_VOCABULARY.into();
        }
        settings
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_settings_with_byte_order_mark() {
        let path = std::env::temp_dir().join("mentor_settings_bom.json");
        std::fs::write(&path, "\u{feff}{\"beeps\": false, \"bindings\": {\"push_to_talk\": \"mouse:x1\"}}").unwrap();
        let settings = Settings::load(&path);
        assert!(!settings.beeps);
        assert_eq!(settings.bindings.push_to_talk, "mouse:x1");
    }

    #[test]
    fn upgrades_untouched_legacy_sources_only() {
        let path = std::env::temp_dir().join("mentor_settings_sources.json");
        std::fs::write(&path, serde_json::json!({ "preferred_sources": LEGACY_SOURCES }).to_string()).unwrap();
        assert!(Settings::load(&path).preferred_sources[0].starts_with("wowhead.com/forever"));
        std::fs::write(&path, r#"{"preferred_sources": ["monsite.fr"]}"#).unwrap();
        assert_eq!(Settings::load(&path).preferred_sources, vec!["monsite.fr".to_string()]);
    }
}
