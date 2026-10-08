//! The interface language, shared by every crate that puts text on screen.
//!
//! `gilvt-app` decides the language (config.toml, else macOS) and calls [`set_current`]; everything else asks
//! [`current`] / [`english`] or picks a translation with [`text`]. Text is chosen when it is shown, not when it
//! is built, so an error stored before a language switch reads in the new language.

use std::cell::Cell;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::Deserialize;

/// With no `language` in config.toml the chrome follows macOS (`gilvt-app`'s `system_language`).
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

    pub fn text<'a>(self, chinese: &'a str, english: &'a str) -> &'a str {
        match self {
            Language::Chinese => chinese,
            Language::English => english,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

thread_local! {
    /// [`with_language`]'s override for this thread.
    static OVERRIDE: Cell<Option<Language>> = const { Cell::new(None) };
}

pub fn set_current(language: Language) {
    CURRENT.store(language as u8, Ordering::Relaxed);
}

/// The interface language: [`with_language`]'s override on this thread, else the one set with [`set_current`]
/// (Chinese until then).
pub fn current() -> Language {
    if let Some(language) = OVERRIDE.with(Cell::get) {
        return language;
    }
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::English,
        _ => Language::Chinese,
    }
}

/// Is the interface in English?
pub fn english() -> bool {
    current() == Language::English
}

pub fn text(chinese: &'static str, english: &'static str) -> &'static str {
    current().text(chinese, english)
}

/// An English count with its noun: `count(1, "session", "sessions")` is "1 session", 2 gives "2 sessions".
pub fn count(n: usize, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
}

/// Runs `f` with [`current`] returning `language` on this thread only (tests run one per thread, so they can
/// check both languages in parallel without touching the process-wide setting).
pub fn with_language<R>(language: Language, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Language>);
    impl Drop for Restore {
        fn drop(&mut self) {
            OVERRIDE.with(|o| o.set(self.0));
        }
    }
    let _restore = Restore(OVERRIDE.with(|o| o.replace(Some(language))));
    f()
}

/// Does `s` contain Chinese characters or full-width punctuation (what an English interface must not show)?
pub fn has_chinese(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '\u{3000}'..='\u{303F}' | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}' | '\u{FF00}'..='\u{FFEF}'))
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

    #[test]
    fn with_language_overrides_this_thread_and_restores() {
        assert_eq!(current(), Language::Chinese);
        let inner = with_language(Language::English, || {
            assert!(english());
            assert_eq!(text("关闭", "Close"), "Close");
            let nested = with_language(Language::Chinese, current);
            assert_eq!(nested, Language::Chinese);
            assert!(english(), "the outer override comes back after a nested one");
            std::thread::spawn(current).join().unwrap()
        });
        assert_eq!(inner, Language::Chinese, "other threads are not affected");
        assert_eq!(current(), Language::Chinese);
    }

    #[test]
    fn counts_agree_with_their_noun() {
        assert_eq!(count(0, "session", "sessions"), "0 sessions");
        assert_eq!(count(1, "session", "sessions"), "1 session");
        assert_eq!(count(12, "line", "lines"), "12 lines");
    }

    #[test]
    fn chinese_detection() {
        for s in ["已更新", "a，b", "（无）", "ａ", "「x」"] {
            assert!(has_chinese(s), "{s}");
        }
        for s in ["Updated", "⌘⇧T → New Tab", "café · 5m", "—…", ""] {
            assert!(!has_chinese(s), "{s}");
        }
    }
}
