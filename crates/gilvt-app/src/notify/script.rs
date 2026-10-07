//! Fallback when gilvt does not run from its app bundle (e.g. `cargo run`): `osascript`'s
//! `display notification`. Clicking one opens Script Editor, not gilvt.

use std::time::{Duration, Instant};

use super::decide::Post;

/// Minimum spacing between desktop notifications from one pane (protects against OSC 9/777 floods).
pub const MIN_INTERVAL: Duration = Duration::from_secs(1);

/// Whether a notification may be shown now, given when this pane last showed one.
pub fn allowed(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|t| now.saturating_duration_since(t) >= MIN_INTERVAL)
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn applescript_for(p: &Post) -> String {
    let mut script = format!("display notification {} with title {}", quote(&p.body), quote(&p.title));
    if !p.subtitle.is_empty() {
        script.push_str(&format!(" subtitle {}", quote(&p.subtitle)));
    }
    script
}

pub fn show(p: &Post) {
    let spawned = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(applescript_for(p))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => eprintln!("gilvt: failed to show notification via osascript: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::decide::plain_post;
    use super::*;

    #[test]
    fn escapes_quotes_and_newlines() {
        let p = plain_post(1, Some("Claude \"code\""), "gilvt", "line1\nline2 \\ end", 0);
        assert_eq!(applescript_for(&p), r#"display notification "line1 line2 \\ end" with title "Claude \"code\"""#);
    }

    #[test]
    fn falls_back_to_pane_title() {
        let p = plain_post(1, None, "zsh", "done", 0);
        assert_eq!(applescript_for(&p), r#"display notification "done" with title "zsh""#);
    }

    #[test]
    fn subtitle_when_present() {
        let mut p = plain_post(1, None, "t", "b", 0);
        p.subtitle = "修 \"登录\"".into();
        assert_eq!(applescript_for(&p), r#"display notification "b" with title "t" subtitle "修 \"登录\"""#);
    }

    #[test]
    fn allowed_with_no_prior_notification() {
        assert!(allowed(None, Instant::now()));
    }

    #[test]
    fn not_allowed_200ms_after() {
        let last = Instant::now();
        assert!(!allowed(Some(last), last + Duration::from_millis(200)));
    }

    #[test]
    fn allowed_exactly_1s_after() {
        let last = Instant::now();
        assert!(allowed(Some(last), last + Duration::from_secs(1)));
    }
}
