//! Application language selection and translation helpers. The language itself lives in `gilvt-i18n`, so
//! the other crates translate what they show; this module adds how gilvt picks it on macOS.

pub use gilvt_i18n::*;

/// Pretends macOS's first preferred language is this tag (`en-US`, `zh-Hans-CN`): GUI tests and development.
pub const ENV_TEST_SYSTEM_LANGUAGE: &str = "GILVT_TEST_SYSTEM_LANGUAGE";

/// The language for macOS's first preferred language (`GILVT_TEST_SYSTEM_LANGUAGE` pretends one); English
/// when there is none. Unit tests always get Chinese, so they do not depend on the machine.
pub fn system_language() -> Language {
    if cfg!(test) {
        return Language::Chinese;
    }
    let tag = std::env::var(ENV_TEST_SYSTEM_LANGUAGE).ok().or_else(preferred_language);
    tag.map_or(Language::English, |t| Language::from_system_tag(&t))
}

/// The first entry of System Settings → General → Language & Region → Preferred Languages, e.g. `en-US`.
fn preferred_language() -> Option<String> {
    let languages = objc2_foundation::NSLocale::preferredLanguages();
    languages.firstObject().map(|s| s.to_string())
}
