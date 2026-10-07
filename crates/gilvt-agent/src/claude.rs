//! Claude Code adapter: hook payloads (spec §2.1) and `~/.claude/projects/…/<session>.jsonl` records.

use serde_json::Value;

use crate::event::{str_field, u64_field, Event};
use crate::summary::{tool_label, tool_summary};

/// Prompt prefixes that are not typed by the user for the model: subagent completions and hand-backs,
/// slash-command echoes, reminders, and `!` shell commands (the user runs them; no model work).
const NOT_A_PROMPT: [&str; 10] = [
    "<task-notification>",
    "<local-command-",
    "<command-name>",
    "<command-message>",
    "<system-reminder>",
    "<bash-input>",
    "<bash-stdout>",
    "<bash-stderr>",
    "<agent-message",
    "Another Claude session sent a message",
];

/// Text of the user record Claude Code writes when the user interrupts a turn (Esc, or rejecting an approval).
const INTERRUPTED: &str = "[Request interrupted by user";

/// The model name Claude Code writes on locally synthesized messages (API errors, interruptions).
const SYNTHETIC_MODEL: &str = "<synthetic>";

pub(crate) fn parse_hook(event: &str, p: &Value) -> Vec<Event> {
    let in_subagent = p.get("agent_id").is_some();
    match event {
        "SessionStart" => vec![Event::SessionStart { model: str_field(p, "model").map(str::to_string) }],
        "SessionEnd" => vec![Event::SessionEnd],
        "UserPromptSubmit" => prompt(str_field(p, "prompt").unwrap_or("")).into_iter().collect(),
        "PreToolUse" if !in_subagent => tool_start(str_field(p, "tool_name").unwrap_or(""), p.get("tool_input")),
        "PostToolUse" | "PostToolUseFailure" if !in_subagent => {
            let tool = str_field(p, "tool_name").unwrap_or("").to_string();
            vec![Event::ToolEnd { tool, ok: event == "PostToolUse" }]
        }
        // Also from a subagent: the user still has to answer.
        "PermissionRequest" => permission_request(str_field(p, "tool_name").unwrap_or(""), p.get("tool_input")),
        "PermissionDenied" => vec![Event::PermissionDenied],
        "Notification" => notification(p).into_iter().collect(),
        "Stop" => {
            let background_tasks = p.get("background_tasks").and_then(Value::as_array).map_or(0, Vec::len);
            vec![Event::TurnEnd { background_tasks }]
        }
        "StopFailure" => vec![Event::Error { message: stop_failure(p) }],
        "SubagentStart" => vec![Event::SubagentStart],
        "SubagentStop" => vec![Event::SubagentStop],
        _ => Vec::new(),
    }
}

/// `None` for prompts the user did not type (task notifications and friends).
fn prompt(text: &str) -> Option<Event> {
    let start = text.trim_start();
    if NOT_A_PROMPT.iter().any(|p| start.starts_with(p)) {
        return None;
    }
    Some(Event::PromptSubmit { text: text.to_string() })
}

fn tool_start(tool: &str, input: Option<&Value>) -> Vec<Event> {
    let input = input.unwrap_or(&Value::Null);
    if tool == "AskUserQuestion" {
        return vec![Event::Question { text: question(input) }];
    }
    vec![Event::ToolStart { tool: tool.to_string(), summary: tool_summary(tool, input) }]
}

/// Arrives when Claude is about to show its approval dialog, ~6 s before `Notification: permission_prompt`.
/// AskUserQuestion goes through the same dialog: that is a question, not an approval.
fn permission_request(tool: &str, input: Option<&Value>) -> Vec<Event> {
    let input = input.unwrap_or(&Value::Null);
    if tool == "AskUserQuestion" {
        return vec![Event::Question { text: question(input) }];
    }
    vec![Event::PermissionNeeded { action: tool_label(tool, &tool_summary(tool, input)) }]
}

fn question(input: &Value) -> String {
    input.pointer("/questions/0/question").and_then(Value::as_str).unwrap_or("").to_string()
}

fn notification(p: &Value) -> Option<Event> {
    let message = str_field(p, "message").unwrap_or("").to_string();
    match str_field(p, "notification_type")? {
        // The message is generic ("Claude needs your permission"): an unnamed action.
        "permission_prompt" => Some(Event::PermissionNeeded { action: String::new() }),
        "elicitation_dialog" | "agent_needs_input" => Some(Event::Question { text: message }),
        _ => None,
    }
}

fn stop_failure(p: &Value) -> String {
    let details = str_field(p, "error_details").filter(|s| !s.is_empty());
    let error = match p.get("error") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(v @ Value::Object(_)) => str_field(v, "message").map(str::to_string),
        _ => None,
    };
    details.map(str::to_string).or(error).unwrap_or_else(|| "API error".to_string())
}

pub(crate) fn parse_record(r: &Value) -> Vec<Event> {
    if r.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return Vec::new();
    }
    match str_field(r, "type") {
        Some("assistant") => assistant(r),
        Some("user") if r.get("isMeta").and_then(Value::as_bool) != Some(true) => user(r),
        Some("system") if str_field(r, "subtype") == Some("turn_duration") => {
            vec![Event::TurnEnd { background_tasks: 0 }]
        }
        _ => Vec::new(),
    }
}

fn assistant(r: &Value) -> Vec<Event> {
    let Some(message) = r.get("message") else { return Vec::new() };
    let content = message.get("content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    if r.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true) {
        let text = content.iter().find_map(|c| str_field(c, "text")).unwrap_or("API error");
        return vec![Event::Error { message: text.to_string() }];
    }
    let mut events = Vec::new();
    let model = str_field(message, "model").filter(|m| !m.is_empty() && *m != SYNTHETIC_MODEL);
    if let Some(model) = model {
        events.push(Event::Model { name: model.to_string() });
        if let Some(usage) = message.get("usage") {
            let context_tokens = u64_field(usage, "input_tokens")
                + u64_field(usage, "cache_creation_input_tokens")
                + u64_field(usage, "cache_read_input_tokens");
            let message_id = str_field(message, "id").map(str::to_string);
            events.push(Event::Usage { context_tokens, context_window: None, message_id });
        }
    }
    let mut uses_tool = false;
    for block in content.iter().filter(|c| str_field(c, "type") == Some("tool_use")) {
        uses_tool = true;
        events.extend(tool_start(str_field(block, "name").unwrap_or(""), block.get("input")));
    }
    let stop = str_field(message, "stop_reason");
    if !uses_tool && matches!(stop, Some("end_turn" | "stop_sequence" | "max_tokens")) {
        events.push(Event::TurnEnd { background_tasks: 0 });
    }
    events
}

fn user(r: &Value) -> Vec<Event> {
    let origin = r.pointer("/origin/kind").and_then(Value::as_str);
    if origin.is_some_and(|kind| kind != "human") {
        return Vec::new();
    }
    let text = match r.pointer("/message/content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => {
            let results: Vec<&Value> = blocks.iter().filter(|b| str_field(b, "type") == Some("tool_result")).collect();
            if !results.is_empty() {
                return tool_results(r, &results);
            }
            blocks.iter().filter_map(|b| str_field(b, "text")).collect::<Vec<_>>().join("\n")
        }
        _ => return Vec::new(),
    };
    if text.starts_with(INTERRUPTED) {
        return vec![Event::Interrupted];
    }
    if text.is_empty() {
        return Vec::new();
    }
    prompt(&text).into_iter().collect()
}

/// A rejected approval (or a dismissed question) only shows here: `toolDenialKind` on the result record.
fn tool_results(r: &Value, results: &[&Value]) -> Vec<Event> {
    let mut events = Vec::new();
    if str_field(r, "toolDenialKind").is_some() {
        events.push(Event::PermissionDenied);
    }
    for result in results {
        events.push(Event::ToolEnd {
            tool: String::new(),
            ok: result.get("is_error").and_then(Value::as_bool) != Some(true),
        });
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn notifications_by_type() {
        let n = |t: &str| json!({"session_id": "s", "message": "Claude needs your permission to use Bash", "notification_type": t});
        let perm = parse_hook("Notification", &n("permission_prompt"));
        assert_eq!(perm, vec![Event::PermissionNeeded { action: String::new() }]);
        let ask = parse_hook("Notification", &n("elicitation_dialog"));
        assert_eq!(ask, vec![Event::Question { text: "Claude needs your permission to use Bash".into() }]);
        assert_eq!(parse_hook("Notification", &n("agent_needs_input")).len(), 1);
        assert!(parse_hook("Notification", &n("idle_prompt")).is_empty());
        assert!(parse_hook("Notification", &n("auth_success")).is_empty());
        assert!(parse_hook("Notification", &json!({"message": "x"})).is_empty());
    }

    #[test]
    fn prompts_that_are_not_turns() {
        assert_eq!(prompt("fix the bug"), Some(Event::PromptSubmit { text: "fix the bug".into() }));
        assert_eq!(prompt("<task-notification>\n<task-id>a</task-id>"), None);
        assert_eq!(prompt("  <task-notification>"), None);
        assert_eq!(prompt("<command-name>/model</command-name>"), None);
        assert_eq!(prompt("<!-- attach: x --> hi"), Some(Event::PromptSubmit { text: "<!-- attach: x --> hi".into() }));
    }

    #[test]
    fn stop_failure_messages() {
        let f = |p: Value| parse_hook("StopFailure", &p);
        let e = |m: &str| vec![Event::Error { message: m.into() }];
        assert_eq!(
            f(json!({"error": "rate_limit", "error_details": "429 Too Many Requests"})),
            e("429 Too Many Requests")
        );
        assert_eq!(f(json!({"error": "server_error"})), e("server_error"));
        assert_eq!(f(json!({"error": {"message": "overloaded"}})), e("overloaded"));
        assert_eq!(f(json!({})), e("API error"));
    }

    #[test]
    fn ask_user_question_is_a_question() {
        let input = json!({"questions": [{"question": "Which database?", "header": "DB", "options": []}]});
        let events = parse_hook("PreToolUse", &json!({"tool_name": "AskUserQuestion", "tool_input": input}));
        assert_eq!(events, vec![Event::Question { text: "Which database?".into() }]);
    }

    #[test]
    fn permission_requests_name_the_tool() {
        let bash = json!({"tool_name": "Bash", "tool_input": {"command": "touch x.txt", "description": "Create x"}});
        assert_eq!(parse_hook("PermissionRequest", &bash), vec![Event::PermissionNeeded { action: "Bash(touch x.txt)".into() }]);
        let sub = json!({"agent_id": "a1", "tool_name": "Write", "tool_input": {"file_path": "/w/a.rs"}});
        assert_eq!(parse_hook("PermissionRequest", &sub), vec![Event::PermissionNeeded { action: "Write(a.rs)".into() }]);
        let ask = json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "red or blue?"}]}});
        assert_eq!(parse_hook("PermissionRequest", &ask), vec![Event::Question { text: "red or blue?".into() }]);
        assert_eq!(parse_hook("PermissionRequest", &json!({})), vec![Event::PermissionNeeded { action: String::new() }]);
    }

    #[test]
    fn subagent_tools_are_not_the_sessions_tools() {
        let p = json!({"agent_id": "a1", "tool_name": "Read", "tool_input": {"file_path": "/x"}});
        assert!(parse_hook("PreToolUse", &p).is_empty());
        assert!(parse_hook("PostToolUse", &p).is_empty());
        assert_eq!(
            parse_hook("PostToolUseFailure", &json!({"tool_name": "Bash"})),
            vec![Event::ToolEnd { tool: "Bash".into(), ok: false }]
        );
        assert!(parse_hook("PreCompact", &json!({})).is_empty());
    }

    #[test]
    fn meta_and_sidechain_records_are_skipped() {
        let meta = json!({"type": "user", "isMeta": true, "message": {"role": "user", "content": "hello"}});
        assert!(parse_record(&meta).is_empty());
        let side = json!({"type": "user", "isSidechain": true, "message": {"role": "user", "content": "hello"}});
        assert!(parse_record(&side).is_empty());
        let notification =
            json!({"type": "user", "origin": {"kind": "task-notification"}, "message": {"content": "done"}});
        assert!(parse_record(&notification).is_empty());
        let human = json!({"type": "user", "origin": {"kind": "human"}, "message": {"content": [{"type": "text", "text": "hi"}]}});
        assert_eq!(parse_record(&human), vec![Event::PromptSubmit { text: "hi".into() }]);
    }

    #[test]
    fn interruptions() {
        let esc = json!({"type": "user", "message": {"role": "user", "content": "[Request interrupted by user]"}});
        assert_eq!(parse_record(&esc), vec![Event::Interrupted]);
        let duration = json!({"type": "system", "subtype": "turn_duration", "durationMs": 1278, "isMeta": false});
        assert_eq!(parse_record(&duration), vec![Event::TurnEnd { background_tasks: 0 }]);
        let retry = json!({"type": "system", "subtype": "api_error", "level": "error", "error": {"message": "Connection error."}});
        assert!(parse_record(&retry).is_empty(), "retried by Claude Code; not a turn outcome");
    }
}
