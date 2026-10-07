//! Captured hook payloads and transcript lines (tests/fixtures, redacted) → normalized events.

mod common;

use common::{hook_events, transcript_events};
use gilvt_agent::{AgentKind, Event};

fn tool_start(tool: &str, summary: &str) -> Event {
    Event::ToolStart { tool: tool.into(), summary: summary.into() }
}

fn tool_end(tool: &str, ok: bool) -> Event {
    Event::ToolEnd { tool: tool.into(), ok }
}

const RUN1_PROMPT_START: &str = "Do these steps in order: 1) Use TodoWrite";

#[test]
fn claude_hooks_of_a_turn_with_a_background_agent() {
    let events = hook_events(AgentKind::Claude, "claude/hooks-run1.jsonl");
    let Event::PromptSubmit { text } = &events[1][0] else { panic!("{:?}", events[1]) };
    assert!(text.starts_with(RUN1_PROMPT_START));
    let expected: Vec<Vec<Event>> = vec![
        vec![Event::SessionStart { model: None }],
        events[1].clone(),
        vec![tool_start("ToolSearch", "select:TaskCreate,TaskUpdate")],
        vec![tool_end("ToolSearch", true)],
        vec![tool_start("TaskCreate", "write file")],
        vec![tool_end("TaskCreate", true)],
        // Blocked by the sandbox: no PostToolUse follows.
        vec![tool_start("Bash", "echo hi > out.txt")],
        vec![tool_start("Agent", "Read out.txt file contents")],
        vec![Event::SubagentStart],
        vec![tool_end("Agent", true)],
        // The agent runs in the background: Stop reports it.
        vec![Event::TurnEnd { background_tasks: 1 }],
        // The subagent's own Read (payload carries agent_id) is not the session's tool.
        vec![],
        vec![],
        vec![Event::SubagentStop],
        vec![Event::SessionEnd],
    ];
    assert_eq!(events, expected);
}

#[test]
fn claude_transcript_status_events() {
    let events: Vec<Event> = transcript_events(AgentKind::Claude, "claude/transcript-run1.jsonl").concat();
    let status: Vec<&Event> =
        events.iter().filter(|e| !matches!(e, Event::Usage { .. } | Event::Model { .. })).collect();
    let Event::PromptSubmit { text } = status[0] else { panic!("{:?}", status[0]) };
    assert!(text.starts_with(RUN1_PROMPT_START));
    let expected = vec![
        status[0].clone(),
        tool_start("ToolSearch", "select:TaskCreate,TaskUpdate"),
        tool_end("", true),
        tool_start("TaskCreate", "write file"),
        tool_start("TaskCreate", "check file"),
        tool_end("", true),
        tool_end("", true),
        tool_start("Bash", "echo hi > out.txt"),
        // Blocked by the sandbox: the result record carries toolDenialKind.
        Event::PermissionDenied,
        tool_end("", false),
        tool_start("Write", "out.txt"),
        tool_end("", true),
        tool_start("Agent", "Read out.txt file contents"),
        tool_end("", true),
        // "Waiting for it to complete…" arrives as thinking + text records of one reply.
        Event::TurnEnd { background_tasks: 0 },
        Event::TurnEnd { background_tasks: 0 },
        // The <task-notification> user record is skipped; the wake-up reply follows.
        tool_start("TaskUpdate", "completed"),
        tool_start("TaskUpdate", "completed"),
        tool_end("", true),
        tool_end("", true),
        Event::TurnEnd { background_tasks: 0 },
        Event::TurnEnd { background_tasks: 0 },
    ];
    assert_eq!(status.into_iter().cloned().collect::<Vec<_>>(), expected);
}

#[test]
fn claude_transcript_usage_is_input_plus_cache_per_reply() {
    let events: Vec<Event> = transcript_events(AgentKind::Claude, "claude/transcript-run1.jsonl").concat();
    let mut usage: Vec<(u64, String)> = Vec::new();
    for e in &events {
        match e {
            Event::Usage { context_tokens, context_window: None, message_id: Some(id) } => {
                usage.push((*context_tokens, id.clone()))
            }
            Event::Model { name } => assert_eq!(name, "claude-haiku-4-5-20251001"),
            Event::Usage { .. } => panic!("{e:?}"),
            _ => {}
        }
    }
    // Each reply is split into several records (thinking / text / tool_use), all carrying the same usage.
    assert_eq!(usage.len(), 24);
    usage.dedup();
    let expected = [
        (10 + 11449 + 16631, "msg_011CfPCph9nEpQMRwxRrkJeU"),
        (10 + 2321 + 28080, "msg_011CfPCqLcndmLg3Eh5fJkNK"),
        (8 + 523 + 30401, "msg_011CfPCr7L7A8gvTvQDQqJFt"),
        (8 + 407 + 30924, "msg_011CfPCrPGJXXrttNbhsUgy5"),
        (8 + 737 + 31331, "msg_011CfPCs3Nkk3zUskDK9X9P4"),
        (8 + 535 + 32068, "msg_011CfPCsVGFMGpDPasmx1mhu"),
        (10 + 718 + 32603, "msg_011CfPCtJQfq8Ds4Ku4F9C9n"),
        (8 + 277 + 33321, "msg_011CfPCthhPP1ZAaFw8s5mXP"),
    ];
    assert_eq!(usage, expected.map(|(t, id)| (t, id.to_string())));
}

#[test]
fn claude_api_error_is_an_error_without_usage() {
    let events = transcript_events(AgentKind::Claude, "claude/transcript-api-error.jsonl");
    let message = "There's an issue with the selected model (claude-nonexistent-model-xyz). It may not exist or you may not have access to it.";
    assert_eq!(
        events,
        vec![
            vec![Event::PromptSubmit { text: "reply ok".into() }],
            vec![],
            vec![Event::Error { message: message.into() }],
            vec![]
        ]
    );
}

/// Interactive Claude (captured through a pty): PermissionRequest names the tool (AskUserQuestion's is a
/// question), then a generic permission_prompt Notification follows ~6 s later.
#[test]
fn claude_hooks_of_an_interactive_session() {
    let events = hook_events(AgentKind::Claude, "claude/hooks-interactive.jsonl");
    let prompt = |t: &str| vec![Event::PromptSubmit { text: t.into() }];
    let permission = || vec![Event::PermissionNeeded { action: String::new() }];
    let expected: Vec<Vec<Event>> = vec![
        vec![Event::SessionStart { model: Some("claude-haiku-4-5-20251001".into()) }],
        prompt("Reply with the single word ok."),
        vec![Event::TurnEnd { background_tasks: 0 }],
        vec![Event::SubagentStop],
        // idle_prompt
        vec![],
        prompt("Use the Bash tool to run exactly: touch y.txt"),
        vec![tool_start("Bash", "touch y.txt")],
        vec![Event::PermissionNeeded { action: "Bash(touch y.txt)".into() }],
        permission(),
        // The user answered "No": no hook fires for that (see the transcript test).
        prompt("Use AskUserQuestion to ask me: red or blue?"),
        vec![Event::Question { text: "red or blue?".into() }],
        vec![Event::Question { text: "red or blue?".into() }],
        permission(),
    ];
    assert_eq!(events, expected);
}

#[test]
fn claude_transcript_of_rejections_and_interrupts() {
    let events: Vec<Event> = transcript_events(AgentKind::Claude, "claude/transcript-interactive.jsonl").concat();
    let status: Vec<Event> =
        events.into_iter().filter(|e| !matches!(e, Event::Usage { .. } | Event::Model { .. })).collect();
    let turn_end = || Event::TurnEnd { background_tasks: 0 };
    let expected = vec![
        Event::PromptSubmit { text: "Reply with the single word ok.".into() },
        turn_end(),
        turn_end(),
        // system/turn_duration
        turn_end(),
        Event::PromptSubmit { text: "Use the Bash tool to run exactly: touch y.txt".into() },
        tool_start("Bash", "touch y.txt"),
        // "No" on the approval: a rejected result, then the interruption marker.
        Event::PermissionDenied,
        tool_end("", false),
        Event::Interrupted,
        turn_end(),
        Event::PromptSubmit { text: "Use AskUserQuestion to ask me: red or blue?".into() },
        Event::Question { text: "red or blue?".into() },
        // Esc on the question.
        Event::PermissionDenied,
        tool_end("", false),
        Event::Interrupted,
        turn_end(),
    ];
    assert_eq!(status, expected);
}

#[test]
fn codex_hooks_of_an_interactive_turn() {
    let events = hook_events(AgentKind::Codex, "codex/hooks-tui.jsonl");
    let model = || Some("gpt-5.6-sol".to_string());
    let expected: Vec<Vec<Event>> = vec![
        vec![Event::SessionStart { model: model() }],
        vec![Event::Model { name: "gpt-5.6-sol".into() }, Event::PromptSubmit { text: "say hi".into() }],
        vec![tool_start("update_plan", "")],
        vec![tool_end("update_plan", true)],
        vec![tool_start("Bash", "echo hi")],
        vec![tool_end("Bash", true)],
        vec![Event::TurnEnd { background_tasks: 0 }],
        vec![Event::SessionEnd],
    ];
    assert_eq!(events, expected);
}

#[test]
fn codex_hooks_of_a_patch() {
    let events = hook_events(AgentKind::Codex, "codex/hooks-exec-patch.jsonl").concat();
    assert!(events.contains(&tool_start("apply_patch", "hello.txt")));
    assert!(events.contains(&tool_end("apply_patch", true)));
    assert_eq!(events.len(), 9);
}

#[test]
fn codex_rollout_of_an_aborted_turn() {
    let events: Vec<Event> = transcript_events(AgentKind::Codex, "codex/rollout-aborted.jsonl").concat();
    let usage = Event::Usage { context_tokens: 1290, context_window: Some(258400), message_id: None };
    let expected = vec![
        Event::Model { name: "gpt-5.6-sol".into() },
        Event::PromptSubmit { text: "say hi".into() },
        tool_start("update_plan", ""),
        tool_start("exec_command", "echo hi"),
        tool_end("", true),
        tool_end("", true),
        usage.clone(),
        usage,
        // Esc: an interruption, not an error.
        Event::Interrupted,
    ];
    assert_eq!(events, expected);
}

#[test]
fn codex_rollout_of_a_completed_turn() {
    let events: Vec<Event> = transcript_events(AgentKind::Codex, "codex/rollout-complete.jsonl").concat();
    assert_eq!(events.last(), Some(&Event::TurnEnd { background_tasks: 0 }));
    let usage: Vec<&Event> = events.iter().filter(|e| matches!(e, Event::Usage { .. })).collect();
    // last_token_usage (this request), not total_token_usage (the session so far: 2580).
    assert_eq!(usage, [&Event::Usage { context_tokens: 1290, context_window: Some(258400), message_id: None }; 2]);
}
