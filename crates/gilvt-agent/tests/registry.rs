//! Binding sessions to panes, lite (transcript-only) sessions, usage, and persisted user choices.

mod common;

use std::path::PathBuf;

use common::{key, lines, payload, Driver};
use gilvt_agent::{AgentKind, Change, Registry, Status, Store};
use serde_json::json;

const CLAUDE: AgentKind = AgentKind::Claude;
const CODEX: AgentKind = AgentKind::Codex;

#[test]
fn new_session_in_the_same_pane_ends_the_old_one() {
    let mut d = Driver::new();
    let (a, b) = (key(CLAUDE, "a"), key(CLAUDE, "b"));
    d.hook(Some(5), CLAUDE, &payload("a", "SessionStart", json!({})));
    d.hook(Some(5), CLAUDE, &payload("a", "UserPromptSubmit", json!({"prompt": "first"})));
    let changes = d.hook(Some(5), CLAUDE, &payload("b", "SessionStart", json!({"source": "clear"})));
    assert_eq!(changes, vec![Change::Ended(a.clone()), Change::Added(b.clone())]);
    assert_eq!(d.session(&a).status, Status::Ended);
    assert_eq!(d.reg.by_pane(5).unwrap().key, b);
    // Other panes are unaffected.
    d.hook(Some(6), CODEX, &payload("c", "SessionStart", json!({})));
    assert_eq!(d.reg.by_pane(5).unwrap().key, b);
    assert_eq!(d.reg.sessions().map(|s| s.key.1.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
    // Resuming the old session in the pane brings it back and ends the other.
    let changes = d.hook(Some(5), CLAUDE, &payload("a", "SessionStart", json!({"source": "resume"})));
    assert_eq!(changes, vec![Change::Ended(b.clone()), Change::Updated(a.clone())]);
    assert_eq!(d.session(&a).status, Status::Idle);
}

#[test]
fn subagent_hooks_and_lone_session_ends_create_nothing() {
    let mut d = Driver::new();
    let sub = payload("x", "PermissionRequest", json!({"agent_id": "a1", "tool_name": "Bash", "tool_input": {}}));
    assert_eq!(d.hook(Some(1), CODEX, &sub), vec![]);
    assert_eq!(d.hook(Some(1), CLAUDE, &payload("y", "SessionEnd", json!({"reason": "other"}))), vec![]);
    assert_eq!(d.reg.sessions().count(), 0);
    // A session first seen mid-turn (gilvt restarted) is created from any other hook.
    let k = key(CLAUDE, "z");
    let pre = payload("z", "PreToolUse", json!({"tool_name": "Bash", "tool_input": {"command": "ls"}}));
    assert_eq!(d.hook(Some(1), CLAUDE, &pre), vec![Change::Added(k.clone())]);
    assert_eq!(d.session(&k).status, Status::Tool { label: "Bash(ls)".into() });
}

#[test]
fn lite_claude_session_inferred_from_the_transcript() {
    let mut d = Driver::new();
    let id = "11111111-2222-4333-8444-555555555503";
    let k = key(CLAUDE, id);
    let path = PathBuf::from(format!("/Users/u/.claude/projects/-Users-u-proj/{id}.jsonl"));
    let changes = d.reg.bind_lite(9, CLAUDE, id.into(), path.clone(), "/Users/u/proj".into(), d.now());
    assert_eq!(changes, vec![Change::Added(k.clone())]);
    let s = d.session(&k);
    assert!(s.lite);
    assert_eq!((s.status.clone(), s.pane, s.transcript.clone()), (Status::Idle, Some(9), Some(path)));

    let mut statuses = vec![];
    for line in lines("claude/transcript-run1.jsonl") {
        d.transcript(&k, &[line]);
        let status = d.session(&k).status.clone();
        if statuses.last() != Some(&status) {
            statuses.push(status);
        }
    }
    let t = |l: &str| Status::Tool { label: l.into() };
    let expected = vec![
        Status::Idle,
        Status::Thinking,
        t("ToolSearch(select:TaskCreate,TaskUpdate)"),
        Status::Thinking,
        t("TaskCreate(write file)"),
        t("TaskCreate(check file)"),
        Status::Thinking,
        t("Bash(echo hi > out.txt)"),
        Status::Thinking,
        t("Write(out.txt)"),
        Status::Thinking,
        t("Agent(Read out.txt file contents)"),
        Status::Thinking,
        // Lite mode cannot see background tasks: the reply that waits for the agent ends the turn.
        Status::Idle,
        t("TaskUpdate(completed)"),
        Status::Thinking,
        Status::Idle,
    ];
    assert_eq!(statuses, expected);
    let s = d.session(&k);
    assert_eq!(s.name, "Do these steps in order: 1) Use TodoWrit…");
    assert_eq!(s.context, Some((8 + 277 + 33321, None)));
    assert_eq!(s.model.as_deref(), Some("claude-haiku-4-5-20251001"));
}

#[test]
fn lite_codex_session_and_hooks_taking_over() {
    let mut d = Driver::new();
    let id = "00000000-0000-7000-8000-00000000000c";
    let k = key(CODEX, id);
    d.reg.bind_lite(3, CODEX, id.into(), "/r.jsonl".into(), "/Users/u/proj".into(), d.now());
    d.transcript(&k, &lines("codex/rollout-complete.jsonl"));
    let s = d.session(&k);
    assert_eq!((s.status.clone(), s.name.as_str(), s.lite), (Status::Idle, "reply ok", true));
    assert_eq!(s.context, Some((1290, Some(258_400))));
    // The first hook for the session: no longer lite, and its transcript no longer drives the status.
    let changes = d.hook(Some(3), CODEX, &payload(id, "UserPromptSubmit", json!({"prompt": "more"})));
    assert_eq!(changes, vec![Change::Updated(k.clone())]);
    assert!(!d.session(&k).lite);
    let done = r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"t"}}"#.to_string();
    assert_eq!(d.transcript(&k, &[done]), vec![]);
    assert_eq!(d.session(&k).status, Status::Thinking);
}

#[test]
fn lite_binding_never_replaces_a_hooked_session() {
    let mut d = Driver::new();
    d.hook(Some(1), CLAUDE, &payload("hooked", "SessionStart", json!({})));
    assert_eq!(d.reg.bind_lite(1, CLAUDE, "other".into(), "/o.jsonl".into(), "/w".into(), d.now()), vec![]);
    assert_eq!(d.reg.by_pane(1).unwrap().key.1, "hooked");
    // Binding the hooked session itself is a no-op; after the pane exits a lite session may bind.
    let own = "/Users/u/.claude/projects/-Users-u-proj/hooked.jsonl";
    assert_eq!(d.reg.bind_lite(1, CLAUDE, "hooked".into(), own.into(), "/Users/u/proj".into(), d.now()), vec![]);
    assert!(!d.reg.by_pane(1).unwrap().lite);
    d.reg.pane_exited(1, d.now());
    let changes = d.reg.bind_lite(1, CLAUDE, "other".into(), "/o.jsonl".into(), "/w".into(), d.now());
    assert_eq!(changes, vec![Change::Added(key(CLAUDE, "other"))]);
    // A second lite session in the pane ends the first.
    let changes = d.reg.bind_lite(1, CODEX, "third".into(), "/t.jsonl".into(), "/w".into(), d.now());
    assert_eq!(changes, vec![Change::Ended(key(CLAUDE, "other")), Change::Added(key(CODEX, "third"))]);
}

#[test]
fn claude_usage_is_deduplicated_by_message_id() {
    let mut d = Driver::new();
    let id = "11111111-2222-4333-8444-555555555503";
    let k = key(CLAUDE, id);
    d.hook(Some(1), CLAUDE, &payload(id, "SessionStart", json!({})));
    let changes = d.transcript(&k, &lines("claude/transcript-run1.jsonl"));
    // 8 replies over 24 records: one update per reply (the first also sets the model).
    assert_eq!(changes.len(), 8);
    assert!(changes.iter().all(|c| *c == Change::Updated(k.clone())));
    let s = d.session(&k);
    assert_eq!(s.status, Status::Idle, "a hooked session's status ignores the transcript");
    assert_eq!(s.context, Some((33606, None)));
    assert_eq!(s.context_ratio(), Some(33606.0 / 200_000.0));
}

#[test]
fn context_ratio_windows() {
    let mut d = Driver::new();
    let k = key(CLAUDE, "s");
    d.hook(Some(1), CLAUDE, &payload("s", "SessionStart", json!({})));
    assert_eq!(d.session(&k).context_ratio(), None);
    let usage = |tokens: u64, id: &str| {
        let usage = json!({"input_tokens": tokens, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0});
        json!({"type": "assistant", "message": {"id": id, "model": "claude-opus-5", "usage": usage, "content": []}})
            .to_string()
    };
    d.transcript(&k, &[usage(180_000, "m1")]);
    assert_eq!(d.session(&k).context_ratio(), Some(0.9));
    // Past 200k the model must have the 1M window.
    d.transcript(&k, &[usage(250_000, "m2")]);
    assert_eq!(d.session(&k).context_ratio(), Some(0.25));
}

#[test]
fn renames_and_mutes_persist() {
    let dir = tempfile::tempdir().unwrap();
    let k = key(CLAUDE, "s");
    let mut d = Driver::new();
    d.reg = Registry::new(Store::open(dir.path()));
    d.hook(Some(1), CLAUDE, &payload("s", "UserPromptSubmit", json!({"prompt": "auto name"})));
    d.reg.rename(&k, "  登录修复  ".into());
    d.reg.set_muted(&k, true);
    assert_eq!((d.session(&k).name.as_str(), d.session(&k).renamed, d.session(&k).muted), ("登录修复", true, true));

    // Next gilvt run: the same session id comes back renamed and muted, and its prompt keeps the rename.
    let mut d2 = Driver::new();
    d2.reg = Registry::new(Store::open(dir.path()));
    d2.hook(Some(4), CLAUDE, &payload("s", "SessionStart", json!({"source": "resume"})));
    d2.hook(Some(4), CLAUDE, &payload("s", "UserPromptSubmit", json!({"prompt": "continue"})));
    let s = d2.session(&k);
    assert_eq!((s.name.as_str(), s.renamed, s.muted), ("登录修复", true, true));
    // Clearing the rename restores the automatic name.
    d2.reg.rename(&k, String::new());
    d2.reg.set_muted(&k, false);
    let s = d2.session(&k);
    assert_eq!((s.name.as_str(), s.renamed, s.muted), ("continue", false, false));
    let d3 = Registry::new(Store::open(dir.path()));
    assert!(d3.get(&k).is_none());
    let text = std::fs::read_to_string(dir.path().join("sessions.json")).unwrap();
    assert!(!text.contains("claude:s"), "{text}");
}

#[test]
fn forget_drops_ended_sessions_only() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (key(CLAUDE, "a"), key(CODEX, "b"));
    let mut d = Driver::new();
    d.reg = Registry::new(Store::open(dir.path()));
    d.hook(Some(1), CLAUDE, &payload("a", "UserPromptSubmit", json!({"prompt": "first"})));
    d.hook(Some(2), CODEX, &payload("b", "UserPromptSubmit", json!({"prompt": "second"})));
    d.reg.rename(&a, "登录修复".into());
    assert!(!d.reg.forget(&a), "a live session stays");
    assert!(!d.reg.forget(&key(CLAUDE, "unknown")));
    d.reg.pane_exited(1, d.now());
    assert!(d.reg.forget(&a));
    assert!(d.reg.get(&a).is_none());
    assert_eq!(d.reg.sessions().map(|s| s.key.clone()).collect::<Vec<_>>(), [b.clone()]);
    // The rename survives (a restore from the Trash finds it), for sessions this run has not seen too.
    assert_eq!(d.reg.saved_name(&a).as_deref(), Some("登录修复"));
    assert_eq!(Registry::new(Store::open(dir.path())).saved_name(&a).as_deref(), Some("登录修复"));
    assert_eq!(d.reg.saved_name(&b), None);
}

#[test]
fn an_answer_in_the_pane_ends_the_wait() {
    let mut d = Driver::new();
    let k = key(CLAUDE, "a");
    d.hook(Some(5), CLAUDE, &payload("a", "UserPromptSubmit", json!({"prompt": "run it"})));
    let input = json!({"command": "bash scripts/slow.sh"});
    d.hook(Some(5), CLAUDE, &payload("a", "PreToolUse", json!({"tool_name": "Bash", "tool_input": input})));
    d.hook(Some(5), CLAUDE, &payload("a", "PermissionRequest", json!({"tool_name": "Bash", "tool_input": input})));
    assert!(d.session(&k).needs_you());
    assert_eq!(d.reg.approval_answered(6, true, d.now()), vec![], "another pane");
    assert_eq!(d.reg.approval_answered(5, true, d.now()), vec![Change::Updated(k.clone())]);
    let s = d.session(&k);
    assert_eq!((s.status.clone(), s.waiting_since), (Status::Tool { label: "Bash(bash scripts/slow.sh)".into() }, None));
    assert_eq!(d.reg.approval_answered(5, true, d.now()), vec![], "only a waiting session changes");
    // A question is answered by its own PostToolUse, not by a key.
    d.hook(Some(5), CLAUDE, &payload("a", "PreToolUse", json!({"tool_name": "AskUserQuestion",
        "tool_input": {"questions": [{"question": "red or blue?"}]}})));
    assert_eq!(d.reg.approval_answered(5, true, d.now()), vec![]);
    assert_eq!(d.session(&k).status, Status::Asking { question: "red or blue?".into() });
}

#[test]
fn queued_approvals_keep_the_session_waiting() {
    let mut d = Driver::new();
    let k = key(CLAUDE, "a");
    d.hook(Some(5), CLAUDE, &payload("a", "UserPromptSubmit", json!({"prompt": "two agents"})));
    for (agent, cmd) in [("s1", "ls a"), ("s2", "ls b")] {
        let p = json!({"agent_id": agent, "tool_name": "Bash", "tool_input": {"command": cmd}});
        d.hook(Some(5), CLAUDE, &payload("a", "PermissionRequest", p));
    }
    // Claude's generic notification for the same dialog is not another request.
    d.hook(Some(5), CLAUDE, &payload("a", "Notification", json!({"notification_type": "permission_prompt", "message": "m"})));
    assert_eq!(d.session(&k).pending_approvals, 2);
    d.reg.approval_answered(5, true, d.now());
    assert!(d.session(&k).needs_you(), "the second dialog is still shown");
    d.reg.approval_answered(5, true, d.now());
    let s = d.session(&k);
    assert_eq!((s.status.clone(), s.pending_approvals), (Status::Tool { label: "Bash(ls b)".into() }, 0));
}
