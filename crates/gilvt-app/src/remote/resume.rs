//! Where a remote history entry is resumed (spec R2a §5.6). Pure; `Workspace::resume_remote` does the typing.

use std::time::{Duration, Instant};

/// How long a new tab may take to reach the remote prompt.
pub const RESUME_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneState {
    pub pane: u64,
    /// The HostId of the pane's ssh link; None: local.
    pub host: Option<String>,
    /// gilvt-remote runs there (the remote shell has gilvt's integration).
    pub enhanced: bool,
    /// At the remote shell's prompt with nothing running or queued, and no live session in the pane.
    pub idle: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeTarget {
    Pane(u64),
    NewTab,
}

fn usable(host: &str, p: &PaneState) -> bool {
    p.host.as_deref() == Some(host) && p.enhanced && p.idle
}

/// 1. the focused pane, 2. another linked idle pane of the host, 3. a new tab.
pub fn resume_target(host: &str, focused: Option<u64>, panes: &[PaneState]) -> ResumeTarget {
    if let Some(p) = focused.and_then(|f| panes.iter().find(|p| p.pane == f)).filter(|p| usable(host, p)) {
        return ResumeTarget::Pane(p.pane);
    }
    panes.iter().find(|p| usable(host, p)).map_or(ResumeTarget::NewTab, |p| ResumeTarget::Pane(p.pane))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    Waiting,
    Ready,
    GaveUp,
}

/// One check of the new tab (`pane`: its state now, None when it is gone).
pub fn wait_step(host: &str, pane: Option<&PaneState>, started: Instant, now: Instant) -> Wait {
    match pane {
        None => Wait::GaveUp,
        Some(p) if usable(host, p) => Wait::Ready,
        Some(_) if now.saturating_duration_since(started) >= RESUME_WAIT => Wait::GaveUp,
        Some(_) => Wait::Waiting,
    }
}

/// Whether typing into an ssh pane now reaches the remote shell: its prompt marks say so AND the local
/// foreground process is the login (`ssh` / `gilvt`). Read synchronously at typing time: after `exit` the local
/// shell prints its own prompt marks before any link-end poll, and typing then would run on this Mac.
pub fn safe_to_type_remote(foreground: Option<&str>, at_prompt: bool) -> bool {
    at_prompt && matches!(foreground, Some("ssh" | "gilvt"))
}

pub fn gave_up_text(display: &str) -> String {
    if crate::i18n::current() == crate::i18n::Language::English {
        format!("No prompt from {display}; the session was not resumed")
    } else {
        format!("没有等到 {display} 的提示符，未恢复会话")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pane: u64, host: Option<&str>, idle: bool) -> PaneState {
        PaneState { pane, host: host.map(Into::into), enhanced: host.is_some(), idle }
    }

    #[test]
    fn the_focused_idle_pane_first_then_another_then_a_new_tab() {
        let h = "dev@h:22";
        let panes = [p(1, Some(h), true), p(2, Some(h), true), p(3, None, true), p(4, Some("dev@o:22"), true)];
        assert_eq!(resume_target(h, Some(2), &panes), ResumeTarget::Pane(2));
        assert_eq!(resume_target(h, Some(3), &panes), ResumeTarget::Pane(1), "focused pane is local: another remote pane");
        let busy = [p(1, Some(h), false), p(4, Some("dev@o:22"), true)];
        assert_eq!(resume_target(h, Some(1), &busy), ResumeTarget::NewTab);
        let plain = [PaneState { pane: 1, host: Some(h.into()), enhanced: false, idle: true }];
        assert_eq!(resume_target(h, Some(1), &plain), ResumeTarget::NewTab, "a plain login has no gilvt on the remote");
    }

    #[test]
    fn local_prompt_marks_with_the_local_shell_in_front_are_not_safe() {
        assert!(safe_to_type_remote(Some("ssh"), true));
        assert!(safe_to_type_remote(Some("gilvt"), true));
        assert!(!safe_to_type_remote(Some("zsh"), true), "ssh just exited: the prompt is the Mac's");
        assert!(!safe_to_type_remote(None, true));
        assert!(!safe_to_type_remote(Some("ssh"), false));
    }

    #[test]
    fn the_new_tab_waits_thirty_seconds_for_the_first_prompt() {
        let (h, t0) = ("dev@h:22", Instant::now());
        assert_eq!(wait_step(h, Some(&p(5, None, true)), t0, t0), Wait::Waiting, "still the local shell");
        assert_eq!(wait_step(h, Some(&p(5, Some(h), false)), t0, t0 + Duration::from_secs(5)), Wait::Waiting, "linked, no prompt yet");
        assert_eq!(wait_step(h, Some(&p(5, Some(h), true)), t0, t0 + Duration::from_secs(6)), Wait::Ready);
        assert_eq!(wait_step(h, Some(&p(5, Some(h), false)), t0, t0 + RESUME_WAIT), Wait::GaveUp);
        assert_eq!(wait_step(h, None, t0, t0 + Duration::from_secs(1)), Wait::GaveUp, "the pane was closed");
        gilvt_i18n::with_language(gilvt_i18n::Language::English, || assert!(!gilvt_i18n::has_chinese(&gave_up_text("devbox"))));
    }
}
