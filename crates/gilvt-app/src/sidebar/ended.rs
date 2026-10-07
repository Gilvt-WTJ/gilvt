//! The sidebar's 已结束 rows (M3c spec §6) as pure decisions: resuming one (in the session's original cwd as
//! the history indexed it, like the 会话 palette; the registry's latest hook cwd only when the index does not
//! list the session), the entry its 移到废纸篓… moves
//! (the history's, else one made from the registry's transcript), and the keys of the confirm bar, which
//! shows the 会话 palette's copy (`launcher::sessions_view::Confirm`).

use std::path::Path;
use std::time::SystemTime;

use gpui::Modifiers;
use gilvt_agent::{disk_size, session_files, HistoryEntry, ReviewState, Session};

use crate::launcher::sessions_model::Resume;

/// Double click / 恢复 / 在右侧恢复 on `s`. `indexed_cwd`: the history's cwd of the session, preferred so both
/// entry points cd to the same directory (spec §10.1); `exists` checks the disk. A session that runs again meanwhile goes to its pane instead.
pub fn resume(s: &Session, indexed_cwd: Option<&Path>, launch: &str, exists: impl Fn(&Path) -> bool) -> Resume {
    if s.is_live() {
        return s.pane.map_or(Resume::Elsewhere, Resume::Focus);
    }
    if s.transcript.as_deref().is_some_and(|t| !exists(t)) {
        return Resume::Gone;
    }
    let Some(cwd) = indexed_cwd.filter(|c| !c.as_os_str().is_empty()).or(s.cwd.as_deref()) else { return Resume::NoCwd };
    if !exists(cwd) {
        return Resume::NoDir(cwd.to_path_buf());
    }
    Resume::Run {
        dir: cwd.to_path_buf(),
        command: gilvt_agent::resume_command(launch, s.agent(), s.session_id(), cwd),
    }
}

/// What 移到废纸篓… on `s` moves: the history's entry when it lists the session, else one made from the
/// registry (a session the index leaves out, e.g. one without a real prompt), sized on disk. None when the
/// registry never saw its transcript.
pub fn trash_entry(s: &Session, indexed: Option<&HistoryEntry>, now: SystemTime) -> Option<HistoryEntry> {
    if let Some(e) = indexed {
        return Some(e.clone());
    }
    let mut e = HistoryEntry {
        agent: s.agent(),
        session_id: s.session_id().to_string(),
        cwd: s.cwd.clone().unwrap_or_default(),
        transcript: s.transcript.clone()?,
        first_prompt: s.name.clone(),
        topic_prompt: String::new(),
        custom_title: None,
        ai_title: None,
        started: None,
        last_active: now,
        turns: 0,
        model: s.model.clone(),
        size: 0,
    };
    e.size = session_files(&e).iter().map(|p| disk_size(p)).sum();
    Some(e)
}

/// Whether an ended session is left out of 已结束: archived as of the turn count the history lists. A session
/// the history does not list counts as not archived (it has no turn count to compare).
pub fn is_archived(state: &ReviewState, entry: Option<&HistoryEntry>) -> bool {
    entry.is_some_and(|e| state.archived_for(e.turns))
}

/// A key on the window root while the confirm bar shows: Some(true) = move to the Trash (a plain ↩, not while
/// the key repeats), Some(false) = cancel (a plain Esc, once no row menu is open), None = not the bar's.
/// Any modifier (⌘ ⌥ ⌃ ⇧ fn) makes the key someone else's.
pub fn confirm_key(key: &str, mods: &Modifiers, held: bool, menu_open: bool) -> Option<bool> {
    if mods.modified() {
        return None;
    }
    match key {
        "escape" if !menu_open => Some(false),
        "enter" if !held && !menu_open => Some(true),
        _ => None,
    }
}

/// Where the root's keyboard stands for the confirm bar: asked for when the bar opened, asked for a second time
/// because the first frame found it elsewhere, or seen on the root (from then on, losing it closes the bar).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConfirmFocus {
    #[default]
    Requested,
    Retried,
    Held,
}

/// What a frame does with the confirm bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmFrame {
    Keep(ConfirmFocus),
    /// The root's keyboard has not landed yet: ask for it once more (the state becomes `Retried`), keep the bar.
    Refocus,
    /// The root had the keyboard and lost it (a click into a pane, a field, …): close the bar.
    Dismiss,
}

/// The confirm bar on a frame where the root has the keyboard (`root_focused`) or not. The bar is opened from the
/// row menu, and the frame it first shows in may not see the root focused yet: only a root that was seen focused
/// and then lost the keyboard closes it.
pub fn confirm_frame(state: ConfirmFocus, root_focused: bool) -> ConfirmFrame {
    match (state, root_focused) {
        (_, true) => ConfirmFrame::Keep(ConfirmFocus::Held),
        (ConfirmFocus::Held, false) => ConfirmFrame::Dismiss,
        (ConfirmFocus::Requested, false) => ConfirmFrame::Refocus,
        (ConfirmFocus::Retried, false) => ConfirmFrame::Keep(ConfirmFocus::Retried),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_agent::{AgentKind, Status};
    use std::fs;
    use std::path::PathBuf;
    use std::time::Instant;

    #[test]
    fn an_archived_session_is_not_listed_until_a_new_turn_arrives() {
        let state = ReviewState { archived_at: Some(SystemTime::UNIX_EPOCH), archived_turns: 3, ..ReviewState::default() };
        let mut entry = trash_entry(&ended(AgentKind::Claude, "a"), None, SystemTime::UNIX_EPOCH).unwrap();
        entry.turns = 3;
        assert!(is_archived(&state, Some(&entry)));
        entry.turns = 4;
        assert!(!is_archived(&state, Some(&entry)), "a new turn brings it back");
        assert!(!is_archived(&state, None), "no history entry: not archived");
        assert!(!is_archived(&ReviewState::default(), Some(&entry)));
    }

    fn ended(agent: AgentKind, id: &str) -> Session {
        let mut s = Session::new((agent, id.to_string()), Instant::now());
        s.status = Status::Ended;
        s.name = "修复登录".into();
        s.cwd = Some("/Users/u/my proj".into());
        s.transcript = Some(format!("/Users/u/.claude/projects/-Users-u-my-proj/{id}.jsonl").into());
        s
    }

    #[test]
    fn without_an_index_entry_the_registry_cwd_is_used() {
        let s = ended(AgentKind::Claude, "c1");
        let r = resume(&s, None, "claude", |_| true);
        let command = "cd '/Users/u/my proj' && claude --resume c1".to_string();
        assert_eq!(r, Resume::Run { dir: "/Users/u/my proj".into(), command });
    }

    #[test]
    fn the_history_cwd_wins_like_in_the_palette() {
        // The registry's cwd is the latest hook's (the agent may have cd'd); the palette resumes in the
        // session's original cwd from the index, and so does the sidebar.
        let x = ended(AgentKind::Codex, "x1");
        let r = resume(&x, Some(Path::new("/Users/u/proj")), "codex-w", |_| true);
        let command = "cd /Users/u/proj && codex-w resume x1".to_string();
        assert_eq!(r, Resume::Run { dir: "/Users/u/proj".into(), command }, "the history's cwd wins");
        let mut s = ended(AgentKind::Claude, "c1");
        s.cwd = None;
        let r = resume(&s, Some(Path::new("/Users/u/proj")), "claude", |_| true);
        assert_eq!(r, Resume::Run { dir: "/Users/u/proj".into(), command: "cd /Users/u/proj && claude --resume c1".into() });
        assert_eq!(resume(&s, None, "claude", |_| true), Resume::NoCwd);
        assert_eq!(Resume::NoCwd.toast().as_deref(), Some("不知道该会话的目录，无法恢复"));
    }

    #[test]
    fn missing_files_open_nothing() {
        let s = ended(AgentKind::Claude, "c1");
        let r = resume(&s, None, "claude", |p| !p.ends_with("c1.jsonl"));
        assert_eq!(r, Resume::Gone);
        let r = resume(&s, None, "claude", |p| p != Path::new("/Users/u/my proj"));
        assert_eq!(r, Resume::NoDir("/Users/u/my proj".into()));
        assert_eq!(r.toast().as_deref(), Some("会话目录已不存在：/Users/u/my proj"));
        // Without a known transcript the resume is attempted anyway (the CLI reports a wrong id itself).
        let mut s = s;
        s.transcript = None;
        assert!(matches!(resume(&s, None, "claude", |p| p == Path::new("/Users/u/my proj")), Resume::Run { .. }));
    }

    #[test]
    fn a_session_running_again_is_not_resumed_twice() {
        let mut s = ended(AgentKind::Claude, "c1");
        s.status = Status::Idle;
        assert_eq!(resume(&s, None, "claude", |_| true), Resume::Elsewhere);
        s.pane = Some(7);
        assert_eq!(resume(&s, None, "claude", |_| true), Resume::Focus(7));
    }

    #[test]
    fn trash_entry_prefers_the_history() {
        let s = ended(AgentKind::Claude, "c1");
        let now = SystemTime::now();
        let mut indexed = trash_entry(&s, None, now).unwrap();
        indexed.size = 1234;
        indexed.first_prompt = "首条提示词".into();
        assert_eq!(trash_entry(&s, Some(&indexed), now), Some(indexed));
        let mut unknown = s;
        unknown.transcript = None;
        assert_eq!(trash_entry(&unknown, None, now), None, "nothing to move");
    }

    #[test]
    fn trash_entry_from_the_registry_is_sized_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".claude/projects/-Users-u-proj");
        fs::create_dir_all(dir.join("c1/subagents")).unwrap();
        fs::write(dir.join("c1.jsonl"), "0123456789").unwrap();
        fs::write(dir.join("c1/subagents/agent-1.jsonl"), "01234").unwrap();
        fs::write(dir.join("other.jsonl"), "not counted").unwrap();
        let mut s = ended(AgentKind::Claude, "c1");
        s.transcript = Some(dir.join("c1.jsonl"));
        s.model = Some("claude-opus".into());
        let now = SystemTime::now();
        let e = trash_entry(&s, None, now).unwrap();
        assert_eq!((e.size, e.first_prompt.as_str(), e.last_active), (15, "修复登录", now));
        assert_eq!((&e.cwd, e.model.as_deref()), (&PathBuf::from("/Users/u/my proj"), Some("claude-opus")));
        assert_eq!(session_files(&e), [dir.join("c1.jsonl"), dir.join("c1")]);
    }

    #[test]
    fn confirm_bar_keys() {
        let plain = Modifiers::default();
        assert_eq!(confirm_key("enter", &plain, false, false), Some(true));
        assert_eq!(confirm_key("enter", &plain, true, false), None, "a held ↩ never confirms");
        assert_eq!(confirm_key("escape", &plain, false, false), Some(false));
        assert_eq!(confirm_key("escape", &plain, false, true), None, "Esc closes the row menu first");
        assert_eq!(confirm_key("enter", &plain, false, true), None);
        assert_eq!(confirm_key("a", &plain, false, false), None);
    }

    #[test]
    fn modified_keys_never_answer_the_bar() {
        for m in [Modifiers::command(), Modifiers::alt(), Modifiers::control(), Modifiers::shift(), Modifiers { function: true, ..Default::default() }] {
            assert_eq!(confirm_key("enter", &m, false, false), None, "{m:?}");
            assert_eq!(confirm_key("escape", &m, false, false), None, "{m:?}");
        }
    }

    #[test]
    fn confirm_bar_survives_the_frame_it_opens_in() {
        use ConfirmFocus::*;
        // Opened: the root is focused at once, or on the retry.
        assert_eq!(confirm_frame(Requested, true), ConfirmFrame::Keep(Held));
        assert_eq!(confirm_frame(Requested, false), ConfirmFrame::Refocus, "the first frame never closes the bar");
        assert_eq!(confirm_frame(Retried, false), ConfirmFrame::Keep(Retried), "nor does the retry's");
        assert_eq!(confirm_frame(Retried, true), ConfirmFrame::Keep(Held));
        // Seen on the root, then the keyboard moves elsewhere: the bar goes.
        assert_eq!(confirm_frame(Held, true), ConfirmFrame::Keep(Held));
        assert_eq!(confirm_frame(Held, false), ConfirmFrame::Dismiss);
    }
}
