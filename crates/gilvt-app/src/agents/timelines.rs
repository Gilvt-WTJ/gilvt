//! What the inspector reads of a session's timeline (M3b spec §4): an immutable snapshot built on the tail
//! thread after each change, plus the anchor decision for hook requests (spec §5), made on the main thread.

use std::sync::Arc;
use std::time::SystemTime;

use gilvt_agent::{AgentKind, Anchor, PaneId, PlanItem, Timeline, Turn};
use serde_json::Value;

/// `unparsed_streak` from which the status card says 「该版本暂未完全适配」 (spec §6).
pub const UNPARSED_WARN: usize = 20;

/// A hook request as the timeline takes it. The payload is moved here from the IPC request (no copy).
#[derive(Clone, Debug, PartialEq)]
pub struct HookFeed {
    pub event: String,
    pub payload: Value,
    /// The pane's absolute cursor line when the request arrived (session `PreToolUse` only).
    pub anchor: Option<Anchor>,
    /// When the request arrived.
    pub at: SystemTime,
}

/// The event name as `Timeline::apply_hook` resolves it ("" → the payload's `hook_event_name`).
fn event_name<'a>(event: &'a str, payload: &'a Value) -> &'a str {
    match event {
        "" => payload.get("hook_event_name").and_then(Value::as_str).unwrap_or(""),
        event => event,
    }
}

/// Whether a hook gets an anchor: a `PreToolUse` of the session itself (Claude and Codex name it alike).
/// Subagent tool calls (`agent_id`) are never anchored, so their pane is not read either.
pub fn wants_anchor(event: &str, payload: &Value) -> bool {
    event_name(event, payload) == "PreToolUse"
        && payload.get("agent_id").and_then(Value::as_str).is_none_or(str::is_empty)
}

/// The anchor of a hook request from `pane`. `line_of` reads the pane's absolute cursor line (None: alternate
/// screen, or not a terminal pane of this app) and is called only when the hook wants an anchor.
pub fn anchor_for(
    pane: Option<PaneId>,
    event: &str,
    payload: &Value,
    line_of: impl FnOnce(PaneId) -> Option<u64>,
) -> Option<Anchor> {
    let pane = pane.filter(|_| wants_anchor(event, payload))?;
    Some(Anchor { pane, line: line_of(pane)? })
}

/// Read-only snapshot of one session's timeline. Turns that did not change since the previous snapshot are
/// the same `Arc` (compare with `Arc::ptr_eq` to skip rebuilding their rows).
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineView {
    pub agent: AgentKind,
    /// +1 per published snapshot of this session (starts at 1).
    pub revision: u64,
    /// Oldest first; turns beyond `MAX_TURNS` have `items == []` (one-line summaries).
    pub turns: Vec<Arc<Turn>>,
    /// The latest TODO list (empty: no TODO block).
    pub plan: Vec<PlanItem>,
    pub session_tokens: u64,
    pub unparsed_streak: usize,
    /// The timeline panicked on some input and is no longer updated: 「无法读取该会话的过程」 (spec §6).
    /// The rows are those of the last good snapshot.
    pub failed: bool,
}

impl TimelineView {
    /// The latest turn.
    pub fn current(&self) -> Option<&Turn> {
        self.turns.last().map(|t| &**t)
    }

    /// 「该版本暂未完全适配」.
    pub fn not_adapted(&self) -> bool {
        self.unparsed_streak >= UNPARSED_WARN
    }
}

/// The snapshot after a change, or None when nothing the view shows changed. Turns equal to the previous
/// snapshot's turn at the same position keep its `Arc` (the comparison is cheap next to cloning them).
pub fn snapshot(t: &Timeline, prev: Option<&TimelineView>, failed: bool) -> Option<TimelineView> {
    let revision = prev.map_or(1, |p| p.revision + 1);
    if failed {
        if prev.is_some_and(|p| p.failed) {
            return None;
        }
        let base = prev.cloned().unwrap_or_else(|| empty(t.agent()));
        return Some(TimelineView { revision, failed: true, ..base });
    }
    let turns: Vec<Arc<Turn>> = t
        .turns()
        .iter()
        .enumerate()
        .map(|(i, turn)| match prev.and_then(|p| p.turns.get(i)) {
            Some(old) if **old == *turn => Arc::clone(old),
            _ => Arc::new(turn.clone()),
        })
        .collect();
    let unchanged = prev.is_some_and(|p| {
        p.turns.len() == turns.len()
            && p.turns.iter().zip(&turns).all(|(a, b)| Arc::ptr_eq(a, b))
            && p.plan == t.plan()
            && p.session_tokens == t.session_tokens()
            && p.unparsed_streak == t.unparsed_streak()
    });
    if unchanged {
        return None;
    }
    Some(TimelineView {
        agent: t.agent(),
        revision,
        turns,
        plan: t.plan().to_vec(),
        session_tokens: t.session_tokens(),
        unparsed_streak: t.unparsed_streak(),
        failed: false,
    })
}

fn empty(agent: AgentKind) -> TimelineView {
    TimelineView {
        agent,
        revision: 0,
        turns: Vec::new(),
        plan: Vec::new(),
        session_tokens: 0,
        unparsed_streak: 0,
        failed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn only_session_pre_tool_use_is_anchored() {
        let pre = json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_use_id": "t1"});
        let line = |p: PaneId| Some(100 + p);
        assert_eq!(anchor_for(Some(3), "PreToolUse", &pre, line), Some(Anchor { pane: 3, line: 103 }));
        // `gilvt hook` without an event argument: the payload names it.
        assert_eq!(anchor_for(Some(3), "", &pre, line), Some(Anchor { pane: 3, line: 103 }));
        // Codex sends the same event name (its `tool_use_id` is the rollout's call_id).
        let codex =
            json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_use_id": "call_sh", "turn_id": "x"});
        assert!(anchor_for(Some(1), "PreToolUse", &codex, line).is_some());
        // Alternate screen / not a terminal: no anchor.
        assert_eq!(anchor_for(Some(3), "PreToolUse", &pre, |_| None), None);
        // No pane (hook from outside gilvt).
        assert_eq!(anchor_for(None, "PreToolUse", &pre, line), None);
    }

    #[test]
    fn other_hooks_do_not_read_the_pane() {
        let never = |_: PaneId| -> Option<u64> { panic!("the pane must not be read") };
        let post = json!({"hook_event_name": "PostToolUse", "tool_use_id": "t1"});
        assert_eq!(anchor_for(Some(1), "PostToolUse", &post, never), None);
        assert_eq!(anchor_for(Some(1), "", &post, never), None);
        let sub = json!({"hook_event_name": "PreToolUse", "agent_id": "ab4d", "tool_use_id": "t2"});
        assert_eq!(anchor_for(Some(1), "PreToolUse", &sub, never), None);
        let empty_agent = json!({"hook_event_name": "PreToolUse", "agent_id": ""});
        assert!(wants_anchor("PreToolUse", &empty_agent));
    }

    fn prompt(t: &mut Timeline, text: &str, secs: u64) {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
        t.apply_hook("UserPromptSubmit", &json!({"hook_event_name": "UserPromptSubmit", "prompt": text}), None, at);
    }

    #[test]
    fn unchanged_turns_share_their_arc() {
        let mut t = Timeline::new(AgentKind::Claude);
        prompt(&mut t, "first", 1);
        let v1 = snapshot(&t, None, false).unwrap();
        assert_eq!((v1.revision, v1.turns.len()), (1, 1));
        assert_eq!(snapshot(&t, Some(&v1), false), None, "nothing changed");
        prompt(&mut t, "second", 2);
        let v2 = snapshot(&t, Some(&v1), false).unwrap();
        assert_eq!(v2.revision, 2);
        assert!(Arc::ptr_eq(&v1.turns[0], &v2.turns[0]), "the first turn did not change");
        assert_eq!(v2.current().map(|t| t.prompt.as_str()), Some("second"));
        let pre = json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_use_id": "t1", "tool_input": {"command": "ls"}});
        t.apply_hook("PreToolUse", &pre, Some(Anchor { pane: 1, line: 7 }), SystemTime::UNIX_EPOCH);
        let v3 = snapshot(&t, Some(&v2), false).unwrap();
        assert!(Arc::ptr_eq(&v2.turns[0], &v3.turns[0]));
        assert!(!Arc::ptr_eq(&v2.turns[1], &v3.turns[1]));
    }

    #[test]
    fn a_failed_timeline_keeps_its_last_rows() {
        let mut t = Timeline::new(AgentKind::Codex);
        prompt(&mut t, "x", 1);
        let v1 = snapshot(&t, None, false).unwrap();
        let v2 = snapshot(&t, Some(&v1), true).unwrap();
        assert!(v2.failed);
        assert_eq!((v2.revision, v2.turns.len()), (2, 1));
        assert_eq!(snapshot(&t, Some(&v2), true), None);
        let fresh = snapshot(&Timeline::new(AgentKind::Codex), None, true).unwrap();
        assert!(fresh.failed && fresh.turns.is_empty() && fresh.revision == 1);
    }

    #[test]
    fn unparsable_lines_count_towards_the_warning() {
        let mut t = Timeline::new(AgentKind::Claude);
        for _ in 0..UNPARSED_WARN {
            t.apply_transcript_line("not json");
        }
        let v = snapshot(&t, None, false).unwrap();
        assert!(v.not_adapted());
    }
}
