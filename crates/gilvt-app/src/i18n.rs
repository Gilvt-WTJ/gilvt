//! Application language selection and translation helpers.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::Deserialize;

/// Pretends macOS's first preferred language is this tag (`en-US`, `zh-Hans-CN`): GUI tests and development.
pub const ENV_TEST_SYSTEM_LANGUAGE: &str = "GILVT_TEST_SYSTEM_LANGUAGE";

/// With no `language` in config.toml the chrome follows macOS (`Language::system`).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum Language {
    #[serde(rename = "zh-CN", alias = "zh", alias = "chinese")]
    Chinese,
    #[serde(rename = "en", alias = "en-US", alias = "english")]
    English,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::Chinese, Language::English];

    pub fn id(self) -> &'static str {
        match self {
            Language::Chinese => "zh-CN",
            Language::English => "en",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Language::Chinese => "简体中文",
            Language::English => "English",
        }
    }

    /// The chrome language for a system language tag: Chinese for any `zh…` (Traditional too: closer than
    /// English), English for everything else.
    pub fn from_system_tag(tag: &str) -> Language {
        let primary = tag.split(['-', '_']).next().unwrap_or("");
        if primary.eq_ignore_ascii_case("zh") {
            Language::Chinese
        } else {
            Language::English
        }
    }

    /// The language for macOS's first preferred language (`GILVT_TEST_SYSTEM_LANGUAGE` pretends one); English
    /// when there is none. Unit tests always get Chinese, so they do not depend on the machine.
    pub fn system() -> Language {
        if cfg!(test) {
            return Language::Chinese;
        }
        let tag = std::env::var(ENV_TEST_SYSTEM_LANGUAGE).ok().or_else(preferred_language);
        tag.map_or(Language::English, |t| Language::from_system_tag(&t))
    }

    pub fn text<'a>(self, chinese: &'a str, english: &'a str) -> &'a str {
        match self {
            Language::Chinese => chinese,
            Language::English => english,
        }
    }
}

/// The first entry of System Settings → General → Language & Region → Preferred Languages, e.g. `en-US`.
fn preferred_language() -> Option<String> {
    let languages = objc2_foundation::NSLocale::preferredLanguages();
    languages.firstObject().map(|s| s.to_string())
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_current(language: Language) {
    CURRENT.store(language as u8, Ordering::Relaxed);
}

pub fn current() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::English,
        _ => Language::Chinese,
    }
}

pub fn text(chinese: &'static str, english: &'static str) -> &'static str {
    current().text(chinese, english)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Config {
        language: Language,
    }

    #[test]
    fn config_values_and_aliases() {
        let parse = |value| {
            toml::from_str::<Config>(&format!("language = {value:?}")).map(|config| config.language)
        };

        assert_eq!(parse("zh-CN").unwrap(), Language::Chinese);
        assert_eq!(parse("zh").unwrap(), Language::Chinese);
        assert_eq!(parse("en").unwrap(), Language::English);
        assert_eq!(parse("en-US").unwrap(), Language::English);
        assert_eq!(parse("english").unwrap(), Language::English);
        assert!(parse("fr").is_err());
    }

    #[test]
    fn system_tags_map_chinese_to_chinese_and_the_rest_to_english() {
        for tag in ["zh-Hans-CN", "zh-Hant-TW", "zh-HK", "zh_CN", "zh", "ZH-hans"] {
            assert_eq!(Language::from_system_tag(tag), Language::Chinese, "{tag}");
        }
        for tag in ["en-US", "en", "ja-JP", "fr-FR", "zu", "", "x-zh"] {
            assert_eq!(Language::from_system_tag(tag), Language::English, "{tag}");
        }
    }
}
