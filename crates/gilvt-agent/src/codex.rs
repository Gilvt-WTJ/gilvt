//! Codex adapter: hook payloads (spec §2.2) and `~/.codex/sessions/…/rollout-*.jsonl` records.

use serde_json::{json, Value};

use crate::event::{str_field, u64_field, Event};
use crate::summary::{tool_label, tool_summary};

pub(crate) fn parse_hook(event: &str, p: &Value) -> Vec<Event> {
    let in_subagent = p.get("agent_id").is_some();
    let tool = str_field(p, "tool_name").unwrap_or("");
    let summary = || tool_summary(tool, p.get("tool_input").unwrap_or(&Value::Null));
    match event {
        "SessionStart" => vec![Event::SessionStart { model: str_field(p, "model").map(str::to_string) }],
        "SessionEnd" => vec![Event::SessionEnd],
        "UserPromptSubmit" => {
            let mut events = model(p);
            events.push(Event::PromptSubmit { text: str_field(p, "prompt").unwrap_or("").to_string() });
            events
        }
        "PreToolUse" if !in_subagent => vec![Event::ToolStart { tool: tool.to_string(), summary: summary() }],
        "PostToolUse" if !in_subagent => vec![Event::ToolEnd { tool: tool.to_string(), ok: true }],
        "PermissionRequest" => vec![Event::PermissionNeeded { action: tool_label(tool, &summary()) }],
        "Stop" => vec![Event::TurnEnd { background_tasks: 0 }],
        "SubagentStart" => vec![Event::SubagentStart],
        "SubagentStop" => vec![Event::SubagentStop],
        _ => Vec::new(),
    }
}

fn model(p: &Value) -> Vec<Event> {
    str_field(p, "model").filter(|m| !m.is_empty()).map(|m| Event::Model { name: m.to_string() }).into_iter().collect()
}

pub(crate) fn parse_record(r: &Value) -> Vec<Event> {
    let payload = r.get("payload").unwrap_or(&Value::Null);
    match str_field(r, "type") {
        Some("event_msg") => event_msg(payload),
        Some("response_item") => response_item(payload),
        Some("turn_context") => model(payload),
        _ => Vec::new(),
    }
}

fn event_msg(p: &Value) -> Vec<Event> {
    let event = match str_field(p, "type") {
        Some("token_count") => token_count(p),
        Some("user_message") => Some(Event::PromptSubmit { text: str_field(p, "message").unwrap_or("").to_string() }),
        Some("task_complete") => Some(Event::TurnEnd { background_tasks: 0 }),
        // The user pressed Esc: like a Claude interruption, not an error.
        Some("turn_aborted") if str_field(p, "reason") == Some("interrupted") => Some(Event::Interrupted),
        Some("turn_aborted") => {
            let reason = str_field(p, "reason").unwrap_or("unknown");
            Some(Event::Error { message: format!("turn aborted: {reason}") })
        }
        Some("error") => Some(Event::Error { message: str_field(p, "message").unwrap_or("error").to_string() }),
        _ => None,
    };
    event.into_iter().collect()
}

/// Context occupancy is the last request's total (what the next request carries), against the model window.
fn token_count(p: &Value) -> Option<Event> {
    let info = p.get("info").filter(|i| i.is_object())?;
    let last = info.get("last_token_usage")?;
    Some(Event::Usage {
        context_tokens: u64_field(last, "total_tokens"),
        context_window: info.get("model_context_window").and_then(Value::as_u64),
        message_id: None,
    })
}

fn response_item(p: &Value) -> Vec<Event> {
    let name = str_field(p, "name").unwrap_or("");
    let event = match str_field(p, "type") {
        Some("function_call") => {
            let args = str_field(p, "arguments").and_then(|a| serde_json::from_str(a).ok()).unwrap_or(Value::Null);
            Some(Event::ToolStart { tool: name.to_string(), summary: tool_summary(name, &args) })
        }
        Some("custom_tool_call") => {
            let input = json!({ "command": str_field(p, "input").unwrap_or("") });
            Some(Event::ToolStart { tool: name.to_string(), summary: tool_summary(name, &input) })
        }
        Some("function_call_output" | "custom_tool_call_output") => {
            Some(Event::ToolEnd { tool: String::new(), ok: true })
        }
        _ => None,
    };
    event.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_request_names_the_action() {
        let p = json!({"session_id": "s", "hook_event_name": "PermissionRequest", "model": "gpt-5.6-sol",
            "tool_name": "Bash", "tool_input": {"command": "rm -rf build"}, "turn_id": "t"});
        assert_eq!(
            parse_hook("PermissionRequest", &p),
            vec![Event::PermissionNeeded { action: "Bash(rm -rf build)".into() }]
        );
        let sub = json!({"agent_id": "a", "tool_name": "Bash", "tool_input": {"command": "ls"}});
        assert_eq!(parse_hook("PermissionRequest", &sub).len(), 1, "a subagent still waits for the user");
        assert!(parse_hook("PreToolUse", &sub).is_empty());
    }

    #[test]
    fn token_count_without_info_is_ignored() {
        let r = json!({"type": "event_msg", "payload": {"type": "token_count", "info": null, "rate_limits": {}}});
        assert!(parse_record(&r).is_empty());
    }

    #[test]
    fn error_events() {
        let r = json!({"type": "event_msg", "payload": {"type": "error", "message": "stream disconnected"}});
        assert_eq!(parse_record(&r), vec![Event::Error { message: "stream disconnected".into() }]);
        let aborted = |reason: Value| json!({"type": "event_msg", "payload": {"type": "turn_aborted", "reason": reason}});
        assert_eq!(parse_record(&aborted(json!("interrupted"))), vec![Event::Interrupted]);
        assert_eq!(parse_record(&aborted(json!("replaced"))), vec![Event::Error { message: "turn aborted: replaced".into() }]);
        assert_eq!(parse_record(&aborted(Value::Null)), vec![Event::Error { message: "turn aborted: unknown".into() }]);
    }

    #[test]
    fn exec_command_arguments_are_json_text() {
        let r = json!({"type": "response_item", "payload": {"type": "function_call", "name": "exec_command",
            "arguments": "{\"cmd\":\"cargo test -p gilvt-agent\",\"workdir\":\"/w\"}", "call_id": "c"}});
        assert_eq!(
            parse_record(&r),
            vec![Event::ToolStart { tool: "exec_command".into(), summary: "cargo test -p gilvt-agent".into() }]
        );
        let bad = json!({"type": "response_item", "payload": {"type": "function_call", "name": "vdl_status", "arguments": "{"}});
        assert_eq!(parse_record(&bad), vec![Event::ToolStart { tool: "vdl_status".into(), summary: String::new() }]);
    }
}
