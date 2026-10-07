//! The normalized event model shared by both adapters, and the identity fields of a hook payload.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which agent CLI a session belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Claude,
    Codex,
}

impl AgentKind {
    /// `"claude"` / `"codex"`, as used on the `gilvt hook <agent>` command line.
    pub fn name(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
        }
    }

    /// Inverse of [`AgentKind::name`].
    pub fn from_name(name: &str) -> Option<AgentKind> {
        match name {
            "claude" => Some(AgentKind::Claude),
            "codex" => Some(AgentKind::Codex),
            _ => None,
        }
    }
}

/// One normalized observation about a session (spec §3). Unknown input → no event.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    SessionStart {
        model: Option<String>,
    },
    SessionEnd,
    /// A user prompt that starts a turn. Task-notification prompts are never emitted.
    PromptSubmit {
        text: String,
    },
    /// `summary` is the short argument (`go test ./...`); see [`crate::tool_label`] for `Bash(go test ./...)`.
    ToolStart {
        tool: String,
        summary: String,
    },
    /// `tool` is empty when the source (a transcript tool result) does not name it.
    ToolEnd {
        tool: String,
        ok: bool,
    },
    /// `action` is `Tool(summary)`, or empty when the source does not say (Claude's `permission_prompt`
    /// notification): the status then keeps the tool it already shows.
    PermissionNeeded {
        action: String,
    },
    PermissionDenied,
    Question {
        text: String,
    },
    TurnEnd {
        background_tasks: usize,
    },
    /// The user interrupted the turn (Esc, or rejected an approval). Only the transcript shows it: no hook
    /// fires. Claude `[Request interrupted by user…`; Codex `turn_aborted` with reason `interrupted`.
    Interrupted,
    Error {
        message: String,
    },
    /// Context occupancy after the latest reply; `message_id` lets Claude's split replies be deduplicated.
    Usage {
        context_tokens: u64,
        context_window: Option<u64>,
        message_id: Option<String>,
    },
    Model {
        name: String,
    },
    SubagentStart,
    SubagentStop,
    /// The user answered the approval dialog in the session's pane (a digit, Enter or Esc while it waited).
    /// Neither agent sends a hook for the answer, so this comes from the pane's keyboard; whether it was a
    /// yes or a no shows only later (a rejection in the transcript).
    ApprovalAnswered,
}

/// A hook payload as received from `gilvt hook <agent> <Event>`.
pub struct HookInput<'a> {
    pub agent: AgentKind,
    /// The hook event name from the command line (falls back to the payload's `hook_event_name` when empty).
    pub event: &'a str,
    pub payload: &'a Value,
}

/// Common identity fields every hook payload carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookMeta {
    pub session_id: String,
    pub transcript_path: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    /// Set when the hook fired inside a subagent (payload `agent_id`); such hooks never create a session.
    pub agent_id: Option<String>,
}

/// Identity fields of a hook payload; `None` without a non-empty `session_id`.
pub fn hook_meta(payload: &Value) -> Option<HookMeta> {
    let session_id = str_field(payload, "session_id").filter(|s| !s.is_empty())?;
    let path = |key| str_field(payload, key).filter(|s| !s.is_empty()).map(PathBuf::from);
    Some(HookMeta {
        session_id: session_id.to_string(),
        transcript_path: path("transcript_path"),
        cwd: path("cwd"),
        agent_id: str_field(payload, "agent_id").filter(|s| !s.is_empty()).map(str::to_string),
    })
}

/// Normalizes a hook payload with the agent's adapter.
pub fn parse_hook(input: &HookInput) -> Vec<Event> {
    let event = match input.event {
        "" => str_field(input.payload, "hook_event_name").unwrap_or(""),
        event => event,
    };
    match input.agent {
        AgentKind::Claude => crate::claude::parse_hook(event, input.payload),
        AgentKind::Codex => crate::codex::parse_hook(event, input.payload),
    }
}

/// Normalizes one transcript (Claude) or rollout (Codex) line. Invalid JSON and unknown records → no events.
pub fn parse_transcript_line(agent: AgentKind, line: &str) -> Vec<Event> {
    let Ok(record) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    match agent {
        AgentKind::Claude => crate::claude::parse_record(&record),
        AgentKind::Codex => crate::codex::parse_record(&record),
    }
}

pub(crate) fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

pub(crate) fn u64_field(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn meta_of_a_payload() {
        let p = json!({"session_id": "s1", "transcript_path": "/t.jsonl", "cwd": "/w", "hook_event_name": "Stop"});
        let meta = hook_meta(&p).unwrap();
        assert_eq!(meta.session_id, "s1");
        assert_eq!(meta.transcript_path.as_deref(), Some(std::path::Path::new("/t.jsonl")));
        assert_eq!(meta.cwd.as_deref(), Some(std::path::Path::new("/w")));
        assert_eq!(meta.agent_id, None);
        let sub = json!({"session_id": "s1", "transcript_path": null, "agent_id": "a1"});
        assert_eq!(hook_meta(&sub).unwrap().transcript_path, None);
        assert_eq!(hook_meta(&sub).unwrap().agent_id.as_deref(), Some("a1"));
        assert_eq!(hook_meta(&json!({"session_id": ""})), None);
        assert_eq!(hook_meta(&json!([1, 2])), None);
    }

    #[test]
    fn agent_names_round_trip() {
        for kind in [AgentKind::Claude, AgentKind::Codex] {
            assert_eq!(AgentKind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(AgentKind::from_name("gemini"), None);
        assert_eq!(serde_json::to_string(&AgentKind::Codex).unwrap(), "\"codex\"");
    }

    #[test]
    fn event_name_falls_back_to_the_payload() {
        let p = json!({"session_id": "s", "hook_event_name": "SessionEnd"});
        let input = HookInput { agent: AgentKind::Claude, event: "", payload: &p };
        assert_eq!(parse_hook(&input), vec![Event::SessionEnd]);
    }

    #[test]
    fn garbage_lines_yield_nothing() {
        for agent in [AgentKind::Claude, AgentKind::Codex] {
            assert!(parse_transcript_line(agent, "").is_empty());
            assert!(parse_transcript_line(agent, "{not json").is_empty());
            assert!(parse_transcript_line(agent, "42").is_empty());
            assert!(parse_transcript_line(agent, r#"{"type":"from-the-future","x":1}"#).is_empty());
        }
    }
}
