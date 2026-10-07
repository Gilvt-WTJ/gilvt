//! Application language selection and translation helpers.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum Language {
    #[default]
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

    pub fn text<'a>(self, chinese: &'a str, english: &'a str) -> &'a str {
        match self {
            Language::Chinese => chinese,
            Language::English => english,
        }
    }
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
}
