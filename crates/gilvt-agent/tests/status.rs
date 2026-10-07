//! Event sequences → status sequences: every row of spec §3.1 and the §7 edge cases.

mod common;

use std::time::Duration;

use common::{hooks, key, lines, payload, Driver};
use gilvt_agent::{AgentKind, Change, Status};
use serde_json::json;

const CLAUDE: AgentKind = AgentKind::Claude;
const CODEX: AgentKind = AgentKind::Codex;
const RUN1: &str = "11111111-2222-4333-8444-555555555503";

fn tool(label: &str) -> Status {
    Status::Tool { label: label.into() }
}

/// The captured Claude run: prompt → tools → Agent launched in the background → Stop with a background
/// task → the subagent's own hooks → SubagentStop → SessionEnd.
#[test]
fn claude_run_with_a_background_agent() {
    let mut d = Driver::new();
    let k = key(CLAUDE, RUN1);
    let mut statuses = Vec::new();
    let mut changes = Vec::new();
    for (_, p) in hooks("claude/hooks-run1.jsonl") {
        changes.push(d.hook(Some(7), CLAUDE, &p));
        let s = d.session(&k);
        statuses.push((s.status.clone(), s.background_tasks));
    }
    let expected = vec![
        (Status::Idle, 0),
        (Status::Thinking, 0),
        (tool("ToolSearch(select:TaskCreate,TaskUpdate)"), 0),
        (Status::Thinking, 0),
        (tool("TaskCreate(write file)"), 0),
        (Status::Thinking, 0),
        (tool("Bash(echo hi > out.txt)"), 0),
        (tool("Agent(Read out.txt file contents)"), 0),
        (tool("Agent(Read out.txt file contents)"), 0),
        (Status::Thinking, 0),
        // Stop with a background task: still running, 「后台任务运行中」.
        (Status::Thinking, 1),
        (Status::Thinking, 1),
        (Status::Thinking, 1),
        (Status::Thinking, 0),
        (Status::Ended, 0),
    ];
    assert_eq!(statuses, expected);
    let updated = || vec![Change::Updated(k.clone())];
    assert_eq!(changes[0], vec![Change::Added(k.clone())]);
    assert_eq!(changes[1], updated());
    assert_eq!(changes[8], vec![], "SubagentStart changes nothing");
    assert_eq!(changes[10], updated());
    assert_eq!(changes[11], vec![], "the subagent's Read is not the session's tool");
    assert_eq!(changes[14], vec![Change::Ended(k.clone())]);

    let s = d.session(&k);
    assert_eq!(s.name, "Do these steps in order: 1) Use TodoWrit…");
    assert_eq!(s.pane, Some(7));
    assert_eq!(s.cwd.as_deref(), Some(std::path::Path::new("/Users/u/proj")));
    assert!(s.transcript.as_ref().unwrap().ends_with(format!("{RUN1}.jsonl")));
    assert!(!s.lite);
    assert!(d.reg.by_pane(7).is_none(), "ended sessions are not the pane's live session");
}

#[test]
fn prompt_starts_the_turn_timer_and_stop_records_it() {
    let mut d = Driver::new();
    let k = key(CLAUDE, "s");
    d.hook(Some(1), CLAUDE, &payload("s", "SessionStart", json!({"source": "startup", "model": "claude-opus-5"})));
    assert_eq!(d.session(&k).status, Status::Idle);
    assert_eq!(d.session(&k).model.as_deref(), Some("claude-opus-5"));
    d.hook(Some(1), CLAUDE, &payload("s", "UserPromptSubmit", json!({"prompt": "\n  fix the login bug\nthen test"})));
    let started = d.now();
    assert_eq!(d.session(&k).turn_started, Some(started));
    assert_eq!(d.session(&k).name, "fix the login bug");
    d.secs += 40;
    d.hook(Some(1), CLAUDE, &payload("s", "Stop", json!({"stop_hook_active": false, "background_tasks": []})));
    let s = d.session(&k);
    assert_eq!(s.status, Status::Idle);
    assert_eq!(s.last_turn, Some(Duration::from_secs(41)));
    assert_eq!(s.turn_started, None);
    assert!(!s.unseen_done, "the pane was visible");
    // A second prompt keeps the first prompt's name.
    d.hook(Some(1), CLAUDE, &payload("s", "UserPromptSubmit", json!({"prompt": "now refactor"})));
    assert_eq!(d.session(&k).name, "fix the login bug");
}

#[test]
fn stop_in_a_hidden_pane_is_unseen_until_focused() {
    let mut d = Driver::new();
    let k = key(CODEX, "c");
    d.visible = false;
    d.hook(Some(3), CODEX, &payload("c", "UserPromptSubmit", json!({"prompt": "go", "model": "gpt-5.5"})));
    d.hook(Some(3), CODEX, &payload("c", "Stop", json!({"stop_hook_active": false})));
    assert!(d.session(&k).unseen_done);
    assert_eq!(d.reg.pane_focused(4), vec![]);
    assert_eq!(d.reg.pane_focused(3), vec![Change::Updated(k.clone())]);
    assert!(!d.session(&k).unseen_done);
    assert_eq!(d.reg.pane_focused(3), vec![], "nothing left to clear");
    // A new prompt also clears it.
    d.hook(Some(3), CODEX, &payload("c", "Stop", json!({})));
    d.hook(Some(3), CODEX, &payload("c", "UserPromptSubmit", json!({"prompt": "again"})));
    assert!(!d.session(&k).unseen_done);
}

#[test]
fn task_notification_prompt_is_not_a_new_turn() {
    let mut d = Driver::new();
    let k = key(CLAUDE, RUN1);
    let all = hooks("claude/hooks-run1.jsonl");
    for (_, p) in &all[..11] {
        d.hook(Some(1), CLAUDE, p);
    }
    let before = d.session(&k).clone();
    assert_eq!((before.status.clone(), before.background_tasks), (Status::Thinking, 1));
    // The prompt Claude Code submits when the background agent finishes (text from the captured transcript).
    let wake = lines("claude/transcript-run1.jsonl")
        .iter()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find_map(|r| {
            r.pointer("/message/content")?.as_str().filter(|c| c.starts_with("<task-notification>")).map(str::to_string)
        })
        .unwrap();
    let changes = d.hook(Some(1), CLAUDE, &payload(RUN1, "UserPromptSubmit", json!({ "prompt": wake })));
    assert_eq!(changes, vec![]);
    let after = d.session(&k);
    assert_eq!(after.turn_started, before.turn_started);
    assert_eq!(after.name, before.name);
    assert_eq!(after.status, Status::Thinking);
}

#[test]
fn claude_permission_prompt_then_approved() {
    let mut d = Driver::new();
    let k = key(CLAUDE, "s");
    d.hook(Some(1), CLAUDE, &payload("s", "UserPromptSubmit", json!({"prompt": "clean up"})));
    d.hook(
        Some(1),
        CLAUDE,
        &payload("s", "PreToolUse", json!({"tool_name": "Bash", "tool_input": {"command": "rm -rf build"}})),
    );
    let asked = d.now() + Duration::from_secs(1);
    let n = json!({"message": "Claude needs your permission to use Bash", "notification_type": "permission_prompt"});
    d.hook(Some(1), CLAUDE, &payload("s", "Notification", n));
    let s = d.session(&k);
    // Without a PermissionRequest (older setups), the notification alone keeps the tool that is running.
    assert_eq!(s.status, Status::NeedsApproval { action: "Bash(rm -rf build)".into() });
    assert!(s.needs_you());
    assert_eq!(s.waiting_since, Some(asked));
    d.hook(Some(1), CLAUDE, &payload("s", "PostToolUse", json!({"tool_name": "Bash", "tool_response": {}})));
    assert_eq!(d.session(&k).status, Status::Thinking);
    assert_eq!(d.session(&k).waiting_since, None);
}

#[test]
fn claude_permission_prompt_then_denied() {
    let mut d = Driver::new();
    let k = key(CLAUDE, "s");
    d.hook(Some(1), CLAUDE, &payload("s", "UserPromptSubmit", json!({"prompt": "x"})));
    d.hook(
        Some(1),
        CLAUDE,
        &payload("s", "PreToolUse", json!({"tool_name": "Write", "tool_input": {"file_path": "/w/a.rs"}})),
    );
    d.hook(
        Some(1),
        CLAUDE,
        &payload("s", "Notification", json!({"message": "m", "notification_type": "permission_prompt"})),
    );
    assert_eq!(d.session(&k).status, Status::NeedsApproval { action: "Write(a.rs)".into() });
    d.hook(Some(1), CLAUDE, &payload("s", "PermissionDenied", json!({"tool_name": "Write", "reason": "user"})));
    assert_eq!(d.session(&k).status, Status::Thinking);
    assert_eq!(d.session(&k).waiting_since, None);
    // PermissionDenied outside an approval changes nothing.
    assert_eq!(d.hook(Some(1), CLAUDE, &payload("s", "PermissionDenied", json!({}))), vec![]);
}

#[test]
fn approval_cleared_by_stop_or_failed_tool() {
    let mut d = Driver::new();
    let k = key(CLAUDE, "s");
    let perm = json!({"message": "Claude needs your permission", "notification_type": "permission_prompt"});
    d.hook(Some(1), CLAUDE, &payload("s", "UserPromptSubmit", json!({"prompt": "x"})));
    d.hook(Some(1), CLAUDE, &payload("s", "Notification", perm.clone()));
    // No tool seen first, and the notification does not name one.
    assert_eq!(d.session(&k).status, Status::NeedsApproval { action: String::new() });
    d.hook(Some(1), CLAUDE, &payload("s", "Stop", json!({"background_tasks": []})));
    assert_eq!(d.session(&k).status, Status::Idle);
    assert_eq!(d.session(&k).waiting_since, None);

    d.hook(Some(1), CLAUDE, &payload("s", "UserPromptSubmit", json!({"prompt": "y"})));
    d.hook(
        Some(1),
        CLAUDE,
        &payload("s", "PreToolUse", json!({"tool_name": "Bash", "tool_input": {"command": "make"}})),
    );
    d.hook(Some(1), CLAUDE, &payload("s", "Notification", perm));
    d.hook(Some(1), CLAUDE, &payload("s", "PostToolUseFailure", json!({"tool_name": "Bash", "error": "exit 2"})));
    assert_eq!(d.session(&k).status, Status::Thinking);
}

/// The interactive capture: PermissionRequest waits at once with its tool; the permission_prompt Notification
/// ~6 s later changes nothing (same status, same start of the wait). AskUserQuestion's is a question.
#[test]
fn claude_permission_request_then_notification() {
    let mut d = Driver::new();
    let k = key(CLAUDE, "22222222-3333-4444-8555-0000000000a4");
    let mut seen = Vec::new();
    let mut waits = Vec::new();
    for (event, p) in hooks("claude/hooks-interactive.jsonl") {
        let changes = d.hook(Some(1), CLAUDE, &p);
        let s = d.session(&k);
        if matches!(event.as_str(), "PermissionRequest" | "Notification") {
            seen.push((event, s.status.clone(), changes.is_empty()));
            waits.push(s.waiting_since);
        }
    }
    let approval = Status::NeedsApproval { action: "Bash(touch y.txt)".into() };
    let question = Status::Asking { question: "red or blue?".into() };
    assert_eq!(
        seen,
        vec![
            ("Notification".into(), Status::Idle, true), // idle_prompt
            ("PermissionRequest".into(), approval.clone(), false),
            ("Notification".into(), approval, true),
            ("PermissionRequest".into(), question.clone(), true),
            ("Notification".into(), question, true),
        ]
    );
    assert!(waits[1].is_some() && waits[1] == waits[2], "{waits:?}");
    assert!(waits[3].is_some() && waits[3] == waits[4], "{waits:?}");
}

#[test]
fn claude_permission_request_outranks_the_running_tool() {
    let mut d = Driver::new();
    let k = key(CLAUDE, "s");
    d.hook(Some(1), CLAUDE, &payload("s", "UserPromptSubmit", json!({"prompt": "x"})));
    let agent = json!({"tool_name": "Agent", "tool_input": {"description": "Explore"}});
    d.hook(Some(1), CLAUDE, &payload("s", "PreToolUse", agent));
    // A subagent's Bash needs approval: the request names it, not the Agent tool that is running.
    let sub = json!({"agent_id": "a1", "tool_name": "Bash", "tool_input": {"command": "rm x"}});
    d.hook(Some(1), CLAUDE, &payload("s", "PermissionRequest", sub));
    assert_eq!(d.session(&k).status, Status::NeedsApproval { action: "Bash(rm x)".into() });
    let since = d.session(&k).waiting_since;
    // A second request while still waiting names the new action but keeps the start of the wait.
    let next = json!({"tool_name": "Write", "tool_input": {"file_path": "/w/b.rs"}});
    d.hook(Some(1), CLAUDE, &payload("s", "PermissionRequest", next));
    assert_eq!(d.session(&k).status, Status::NeedsApproval { action: "Write(b.rs)".into() });
    assert_eq!(d.session(&k).waiting_since, since);
}

#[test]
fn codex_permission_request() {
    let mut d = Driver::new();
    let k = key(CODEX, "c");
    let tool_input = json!({"command": "cargo publish"});
    d.hook(Some(2), CODEX, &payload("c", "UserPromptSubmit", json!({"prompt": "ship it", "model": "gpt-5.6-sol"})));
    d.hook(Some(2), CODEX, &payload("c", "PreToolUse", json!({"tool_name": "Bash", "tool_input": tool_input})));
    d.hook(Some(2), CODEX, &payload("c", "PermissionRequest", json!({"tool_name": "Bash", "tool_input": tool_input})));
    assert_eq!(d.session(&k).status, Status::NeedsApproval { action: "Bash(cargo publish)".into() });
    d.hook(Some(2), CODEX, &payload("c", "PostToolUse", json!({"tool_name": "Bash", "tool_response": "ok"})));
    assert_eq!(d.session(&k).status, Status::Thinking);
    // Rejected in Codex's own UI: no tool result, the turn ends.
    d.hook(Some(2), CODEX, &payload("c", "PermissionRequest", json!({"tool_name": "Bash", "tool_input": tool_input})));
    d.hook(Some(2), CODEX, &payload("c", "Stop", json!({"stop_hook_active": false})));
    assert_eq!(d.session(&k).status, Status::Idle);
}

#[test]
fn questions() {
    let mut d = Driver::new();
    let k = key(CLAUDE, "s");
    let ask = json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "Which database?"}]}});
    d.hook(Some(1), CLAUDE, &payload("s", "UserPromptSubmit", json!({"prompt": "x"})));
    d.hook(Some(1), CLAUDE, &payload("s", "PreToolUse", ask));
    let asked = d.now();
    assert_eq!(d.session(&k).status, Status::Asking { question: "Which database?".into() });
    // A later waiting signal keeps the original start of the wait.
    let n = json!({"message": "Claude is waiting for your input", "notification_type": "elicitation_dialog"});
    d.hook(Some(1), CLAUDE, &payload("s", "Notification", n));
    assert_eq!(d.session(&k).waiting_since, Some(asked));
    d.hook(Some(1), CLAUDE, &payload("s", "PostToolUse", json!({"tool_name": "AskUserQuestion"})));
    assert_eq!(d.session(&k).status, Status::Thinking);
    for kind in ["elicitation_dialog", "agent_needs_input"] {
        d.hook(Some(1), CLAUDE, &payload("s", "Notification", json!({"message": kind, "notification_type": kind})));
        assert_eq!(d.session(&k).status, Status::Asking { question: kind.into() });
        d.hook(Some(1), CLAUDE, &payload("s", "Stop", json!({})));
        assert_eq!(d.session(&k).status, Status::Idle);
    }
}

#[test]
fn stop_failure_is_an_error_until_the_next_prompt() {
    let mut d = Driver::new();
    let k = key(CLAUDE, "s");
    d.hook(Some(1), CLAUDE, &payload("s", "UserPromptSubmit", json!({"prompt": "x"})));
    d.hook(Some(1), CLAUDE, &payload("s", "StopFailure", json!({"error": "rate_limit", "error_details": "429"})));
    assert_eq!(d.session(&k).status, Status::Error { message: "429".into() });
    assert_eq!(d.session(&k).turn_started, None);
    // A Stop after the failure does not hide it.
    d.hook(Some(1), CLAUDE, &payload("s", "Stop", json!({})));
    assert_eq!(d.session(&k).status, Status::Error { message: "429".into() });
    d.hook(Some(1), CLAUDE, &payload("s", "UserPromptSubmit", json!({"prompt": "retry"})));
    assert_eq!(d.session(&k).status, Status::Thinking);
}

/// Esc in Codex fires no hook; the rollout's `turn_aborted` (reason `interrupted`) ends the turn like a
/// Claude interruption: idle, no waiting, no turn duration, not 完成未看.
#[test]
fn codex_turn_interrupted_from_the_rollout_is_idle() {
    let mut d = Driver::new();
    let id = "00000000-0000-7000-8000-00000000000a";
    let k = key(CODEX, id);
    d.visible = false;
    // Up to the Bash tool; then the user presses Esc.
    for (_, p) in &hooks("codex/hooks-tui.jsonl")[..5] {
        d.hook(Some(4), CODEX, p);
    }
    assert_eq!(d.session(&k).status, tool("Bash(echo hi)"));
    assert_eq!(d.session(&k).model.as_deref(), Some("gpt-5.6-sol"));
    // Hooked session: the rollout only supplements usage, model, interruptions and errors.
    d.transcript(&k, &lines("codex/rollout-aborted.jsonl"));
    let s = d.session(&k);
    assert_eq!(s.status, Status::Idle);
    assert_eq!((s.waiting_since, s.turn_started, s.last_turn, s.unseen_done), (None, None, None, false));
    assert_eq!(s.context, Some((1290, Some(258_400))));
    assert_eq!(s.context_ratio(), Some(1290.0 / 258_400.0));
    assert_eq!(s.name, "say hi");
}

#[test]
fn codex_interrupt_clears_an_approval_and_other_aborts_are_errors() {
    let mut d = Driver::new();
    let k = key(CODEX, "c");
    let tool_input = json!({"command": "cargo publish"});
    d.hook(Some(2), CODEX, &payload("c", "UserPromptSubmit", json!({"prompt": "ship it"})));
    d.hook(Some(2), CODEX, &payload("c", "PermissionRequest", json!({"tool_name": "Bash", "tool_input": tool_input})));
    assert!(d.session(&k).needs_you());
    let aborted = |reason: &str| json!({"type": "event_msg", "payload": {"type": "turn_aborted", "reason": reason}}).to_string();
    d.transcript(&k, &[aborted("interrupted")]);
    assert_eq!(d.session(&k).status, Status::Idle);
    assert_eq!(d.session(&k).waiting_since, None);
    d.hook(Some(2), CODEX, &payload("c", "UserPromptSubmit", json!({"prompt": "again"})));
    d.transcript(&k, &[aborted("replaced")]);
    assert_eq!(d.session(&k).status, Status::Error { message: "turn aborted: replaced".into() });
}

#[test]
fn session_end_and_process_exit() {
    let mut d = Driver::new();
    let (a, b) = (key(CLAUDE, "a"), key(CODEX, "b"));
    d.hook(Some(1), CLAUDE, &payload("a", "SessionStart", json!({})));
    d.hook(Some(2), CODEX, &payload("b", "SessionStart", json!({})));
    assert_eq!(
        d.hook(Some(1), CLAUDE, &payload("a", "SessionEnd", json!({"reason": "other"}))),
        vec![Change::Ended(a.clone())]
    );
    // Killed without SessionEnd: the pane's foreground went back to the shell.
    d.hook(Some(2), CODEX, &payload("b", "UserPromptSubmit", json!({"prompt": "x"})));
    assert_eq!(d.reg.pane_exited(2, d.now()), vec![Change::Ended(b.clone())]);
    assert_eq!(d.session(&b).status, Status::Ended);
    assert_eq!(d.reg.pane_exited(2, d.now()), vec![], "already ended");
    // Late events for an ended session are ignored.
    assert_eq!(d.hook(Some(2), CODEX, &payload("b", "Stop", json!({}))), vec![]);
    assert_eq!(d.session(&b).status, Status::Ended);
}

/// Interactive Claude, hooks and transcript interleaved as they happened: answering "No" to an approval and
/// Esc on a question fire no hook; the transcript's rejected result + interruption marker end the wait.
#[test]
fn claude_interactive_rejections_are_seen_in_the_transcript() {
    let mut d = Driver::new();
    d.visible = false;
    let k = key(CLAUDE, "22222222-3333-4444-8555-0000000000a4");
    let hook = hooks("claude/hooks-interactive.jsonl");
    let transcript = lines("claude/transcript-interactive.jsonl");
    let run = |d: &mut Driver, hooks: &[(String, serde_json::Value)], lines: &[String]| {
        for (_, p) in hooks {
            d.hook(Some(1), CLAUDE, p);
        }
        d.transcript(&k, lines);
        d.session(&k).status.clone()
    };

    // "Reply with the single word ok." → Stop; idle_prompt changes nothing.
    run(&mut d, &hook[..3], &[]);
    assert!(d.session(&k).unseen_done);
    assert_eq!(run(&mut d, &hook[3..5], &transcript[..5]), Status::Idle);

    // Bash needs approval; the notification itself only says "Claude needs your permission".
    assert_eq!(
        run(&mut d, &hook[5..9], &transcript[5..8]),
        Status::NeedsApproval { action: "Bash(touch y.txt)".into() }
    );
    let s = d.session(&k).clone();
    assert!(s.needs_you() && s.waiting_since.is_some());
    // "No": only the transcript shows it.
    assert_eq!(run(&mut d, &[], &transcript[8..11]), Status::Idle);
    let s = d.session(&k);
    assert_eq!((s.waiting_since, s.turn_started, s.last_turn, s.unseen_done), (None, None, None, false));

    // AskUserQuestion, then its own permission prompt: still a question.
    let question = Status::Asking { question: "red or blue?".into() };
    assert_eq!(run(&mut d, &hook[9..13], &transcript[11..14]), question);
    // Esc on the question.
    assert_eq!(run(&mut d, &[], &transcript[14..]), Status::Idle);
    assert_eq!(d.session(&k).name, "Reply with the single word ok.");
    assert_eq!(d.session(&k).model.as_deref(), Some("claude-haiku-4-5-20251001"));
}
