//! Agent status on the panes and tabs (mockup m3a-part3.html ①②): a 2 px ring on the pane's edge
//! (yellow needs you, red error, green 完成未看) and a dot on the tab for its most urgent session.
//! The ring is an overlay: it never changes the terminal's rows / columns.

use gilvt_agent::{Session, Status};

/// A session's mark, in rising priority (`Ord`): the tab dot is the maximum over its panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mark {
    /// 完成未看.
    Done,
    /// 思考中 / 执行中 (dot only; running panes get no ring).
    Running,
    Error,
    /// 等待审批 / 在问你.
    NeedsYou,
}

impl Mark {
    /// The ring / dot color: the theme's vivid status color.
    pub fn color(self, ui: &gilvt_theme::UiColors) -> gilvt_theme::color::Rgb {
        match self {
            Mark::NeedsYou => ui.attention.ring,
            Mark::Error => ui.error.ring,
            Mark::Running => ui.running.ring,
            Mark::Done => ui.done.ring,
        }
    }
}

/// The mark of a live session; idle (and ended) sessions have none.
pub fn session_mark(s: &Session) -> Option<Mark> {
    match &s.status {
        Status::Ended => None,
        _ if s.needs_you() => Some(Mark::NeedsYou),
        Status::Error { .. } => Some(Mark::Error),
        st if st.is_running() || s.background_tasks > 0 => Some(Mark::Running),
        _ if s.unseen_done => Some(Mark::Done),
        _ => None,
    }
}

/// Ring color of a pane showing `s`: every mark but running.
pub fn ring_mark(s: Option<&Session>) -> Option<Mark> {
    s.and_then(session_mark).filter(|m| *m != Mark::Running)
}

/// The tab dot: the highest-priority mark among the tab's sessions; none when all are idle.
pub fn tab_dot<'a>(sessions: impl IntoIterator<Item = &'a Session>) -> Option<Mark> {
    sessions.into_iter().filter_map(session_mark).max()
}

/// Layout border (1 px, only in a split tab, as before M3a) plus the ring overlay drawn inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    /// The 1 px border's color; `None` = the palette background (no visible border).
    pub border: Option<EdgeColor>,
    /// The overlay's mark and width in px, so border + overlay add up to 2 px.
    pub ring: Option<(Mark, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeColor {
    Focus,
    Status(Mark),
}

pub const RING: f32 = 2.;

/// A status color wins over the focus blue; the terminal grid is the same with or without it.
pub fn edge(ring: Option<Mark>, focused: bool, split: bool) -> Edge {
    match (ring, split) {
        (Some(m), true) => Edge { border: Some(EdgeColor::Status(m)), ring: Some((m, RING - 1.)) },
        (Some(m), false) => Edge { border: None, ring: Some((m, RING)) },
        (None, true) if focused => Edge { border: Some(EdgeColor::Focus), ring: None },
        (None, _) => Edge { border: None, ring: None },
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    #[test]
    fn default_themes_keep_the_ring_colors() {
        use gilvt_theme::color::rgb;
        for ui in [gilvt_theme::ui::gilvt_light(), gilvt_theme::ui::gilvt_dark()] {
            assert_eq!(Mark::NeedsYou.color(&ui), rgb(0xe5a100));
            assert_eq!(Mark::Error.color(&ui), rgb(0xd93a3a));
            assert_eq!(Mark::Running.color(&ui), rgb(0x3b7ddd));
            assert_eq!(Mark::Done.color(&ui), rgb(0x2e9e4f));
        }
    }

    use gilvt_agent::AgentKind;

    use super::*;

    fn s(status: Status) -> Session {
        let mut s = Session::new((AgentKind::Claude, "a".into()), Instant::now());
        s.status = status;
        s
    }

    #[test]
    fn marks_follow_the_status() {
        assert_eq!(session_mark(&s(Status::NeedsApproval { action: String::new() })), Some(Mark::NeedsYou));
        assert_eq!(session_mark(&s(Status::Asking { question: "?".into() })), Some(Mark::NeedsYou));
        assert_eq!(session_mark(&s(Status::Error { message: "x".into() })), Some(Mark::Error));
        assert_eq!(session_mark(&s(Status::Thinking)), Some(Mark::Running));
        assert_eq!(session_mark(&s(Status::Tool { label: "Bash".into() })), Some(Mark::Running));
        assert_eq!(session_mark(&s(Status::Idle)), None);
        let mut done = s(Status::Idle);
        done.unseen_done = true;
        assert_eq!(session_mark(&done), Some(Mark::Done));
        let mut bg = s(Status::Idle);
        bg.background_tasks = 1;
        assert_eq!(session_mark(&bg), Some(Mark::Running), "后台任务运行中");
        let mut ended = s(Status::Ended);
        ended.unseen_done = true;
        assert_eq!(session_mark(&ended), None);
    }

    #[test]
    fn rings_skip_running_panes() {
        assert_eq!(ring_mark(None), None);
        assert_eq!(ring_mark(Some(&s(Status::Thinking))), None);
        assert_eq!(ring_mark(Some(&s(Status::Error { message: String::new() }))), Some(Mark::Error));
        let mut waiting_done = s(Status::Asking { question: String::new() });
        waiting_done.unseen_done = true;
        assert_eq!(ring_mark(Some(&waiting_done)), Some(Mark::NeedsYou), "needs you wins over 完成未看");
    }

    #[test]
    fn tab_dot_takes_the_most_urgent() {
        let mut done = s(Status::Idle);
        done.unseen_done = true;
        let run = s(Status::Thinking);
        let err = s(Status::Error { message: String::new() });
        let need = s(Status::NeedsApproval { action: String::new() });
        let idle = s(Status::Idle);
        assert_eq!(tab_dot([&idle]), None);
        assert_eq!(tab_dot([]), None, "a shell-only tab has no dot");
        assert_eq!(tab_dot([&idle, &done]), Some(Mark::Done));
        assert_eq!(tab_dot([&done, &run]), Some(Mark::Running));
        assert_eq!(tab_dot([&run, &err, &done]), Some(Mark::Error));
        assert_eq!(tab_dot([&err, &need, &run]), Some(Mark::NeedsYou));
        assert!(Mark::NeedsYou > Mark::Error && Mark::Error > Mark::Running && Mark::Running > Mark::Done);
    }

    #[test]
    fn status_color_wins_over_focus() {
        assert_eq!(edge(None, false, false), Edge { border: None, ring: None });
        assert_eq!(edge(None, true, false), Edge { border: None, ring: None }, "alone in the tab: no focus border");
        assert_eq!(edge(None, true, true), Edge { border: Some(EdgeColor::Focus), ring: None });
        assert_eq!(edge(None, false, true), Edge { border: None, ring: None });
        let y = Mark::NeedsYou;
        assert_eq!(edge(Some(y), true, true), Edge { border: Some(EdgeColor::Status(y)), ring: Some((y, 1.)) });
        assert_eq!(edge(Some(y), false, true), Edge { border: Some(EdgeColor::Status(y)), ring: Some((y, 1.)) });
        assert_eq!(edge(Some(y), true, false), Edge { border: None, ring: Some((y, 2.)) });
    }
}
