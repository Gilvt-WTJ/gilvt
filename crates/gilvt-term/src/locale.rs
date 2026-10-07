//! Gives child shells a usable locale even when gilvt itself was launched (e.g. from Finder/Dock)
//! without one. Without `LANG`/`LC_ALL`/`LC_CTYPE`, child shells run in the C locale and readline
//! rejects multibyte input, so Chinese (and other non-ASCII) input silently fails.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Upper bound on how long we wait for `defaults read -g AppleLocale` before giving up and
/// treating the locale as unknown. Runs on the UI thread at app launch, so this must stay small.
const APPLE_LOCALE_TIMEOUT: Duration = Duration::from_millis(300);

/// How often to poll the child while waiting for it to exit.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Runs `cmd`, waiting up to `timeout` for it to exit. Returns its trimmed stdout on success;
/// `None` if it fails to spawn, exits with a non-zero status, or does not finish in time (in which
/// case the child is killed and reaped so it does not linger).
fn command_output_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<String> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null()).spawn().ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().ok()? {
            Some(status) => {
                if !status.success() {
                    return None;
                }
                let mut out = String::new();
                child.stdout.take()?.read_to_string(&mut out).ok()?;
                return Some(out.trim().to_string());
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(POLL_INTERVAL),
        }
    }
}

/// Converts a macOS `AppleLocale` value ("en_US", "zh_CN", "zh-Hans_CN", "en_US@rg=cnzzzz") into a
/// POSIX UTF-8 locale name ("en_US.UTF-8"), or `None` if it carries no region (e.g. "en") or is
/// otherwise not shaped like `<lang>_<region>`.
pub fn utf8_locale_for(apple_locale: &str) -> Option<String> {
    let (lang_part, region_part) = apple_locale.split_once('_')?;
    let lang = lang_part.split('-').next().unwrap_or("");
    let region = region_part.split('@').next().unwrap_or("");
    let ascii_alpha = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphabetic());
    if ascii_alpha(lang) && ascii_alpha(region) {
        Some(format!("{lang}_{region}.UTF-8"))
    } else {
        None
    }
}

/// Value to set as the child's `LANG`, or `None` when the parent (gilvt itself) already has a
/// locale, in which case the child inherits it unchanged.
///
/// `has_locale` is true when `LANG`, `LC_ALL` or `LC_CTYPE` is set and non-empty in gilvt's own
/// environment. `exists(name)` reports whether `/usr/share/locale/<name>` exists, used to check
/// that the macOS `AppleLocale`-derived locale is actually installed before relying on it.
pub fn child_lang(has_locale: bool, apple_locale: Option<&str>, exists: impl Fn(&str) -> bool) -> Option<String> {
    if has_locale {
        return None;
    }
    const FALLBACK: &str = "en_US.UTF-8";
    let candidate = apple_locale.and_then(utf8_locale_for);
    match candidate {
        Some(locale) if exists(&locale) => Some(locale),
        _ => Some(FALLBACK.to_string()),
    }
}

/// Real detection, cached for the lifetime of the process: reads gilvt's own environment, shells out
/// to `defaults read -g AppleLocale`, and checks `/usr/share/locale` for the result.
pub fn detect_child_lang() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let has_locale = ["LANG", "LC_ALL", "LC_CTYPE"]
                .iter()
                .any(|k| std::env::var(k).is_ok_and(|v| !v.is_empty()));
            let apple_locale = command_output_with_timeout(
                Command::new("defaults").args(["read", "-g", "AppleLocale"]),
                APPLE_LOCALE_TIMEOUT,
            );
            child_lang(has_locale, apple_locale.as_deref(), |name| {
                std::path::Path::new("/usr/share/locale").join(name).exists()
            })
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_language_region() {
        assert_eq!(utf8_locale_for("en_US").as_deref(), Some("en_US.UTF-8"));
        assert_eq!(utf8_locale_for("zh_CN").as_deref(), Some("zh_CN.UTF-8"));
    }

    #[test]
    fn strips_script_subtag() {
        assert_eq!(utf8_locale_for("zh-Hans_CN").as_deref(), Some("zh_CN.UTF-8"));
    }

    #[test]
    fn strips_region_variant_suffix() {
        assert_eq!(utf8_locale_for("en_US@rg=cnzzzz").as_deref(), Some("en_US.UTF-8"));
    }

    #[test]
    fn no_region_is_none() {
        assert_eq!(utf8_locale_for("en"), None);
    }

    #[test]
    fn empty_is_none() {
        assert_eq!(utf8_locale_for(""), None);
    }

    #[test]
    fn has_locale_means_no_override() {
        assert_eq!(child_lang(true, Some("zh_CN"), |_| true), None);
    }

    #[test]
    fn apple_locale_installed() {
        assert_eq!(child_lang(false, Some("zh_CN"), |_| true).as_deref(), Some("zh_CN.UTF-8"));
    }

    #[test]
    fn apple_locale_not_installed_falls_back() {
        assert_eq!(child_lang(false, Some("zh_CN"), |_| false).as_deref(), Some("en_US.UTF-8"));
    }

    #[test]
    fn no_apple_locale_falls_back() {
        assert_eq!(child_lang(false, None, |_| true).as_deref(), Some("en_US.UTF-8"));
    }

    #[test]
    fn command_output_with_timeout_returns_stdout() {
        let mut cmd = Command::new("/bin/echo");
        cmd.arg("hi");
        assert_eq!(command_output_with_timeout(&mut cmd, Duration::from_millis(300)).as_deref(), Some("hi"));
    }

    #[test]
    fn command_output_with_timeout_kills_slow_command() {
        let start = Instant::now();
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("5");
        assert_eq!(command_output_with_timeout(&mut cmd, Duration::from_millis(50)), None);
        assert!(start.elapsed() < Duration::from_secs(2), "took too long to time out: {:?}", start.elapsed());
    }
}
