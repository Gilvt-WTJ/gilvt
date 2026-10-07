//! `claude -p` as a conversation: stream-json in and out (S2 §6.2). Shapes: Claude Code 2.1.291 —
//! `tests/fixtures/claude-chat-model-error.jsonl` (recorded) and `claude-chat-turn.jsonl` (built to the format).

use std::path::Path;

use serde_json::{json, Value};

use super::{ChatEvent, Decoded, McpLaunch, Protocol, TurnEnd};
use crate::provider::classify;
use crate::tools::CLAUDE_PREFIX;

pub fn args(model: Option<&str>, instructions: &str, mcp_config: &Path) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--tools",
        "",
        "--allowedTools",
        "mcp__gilvt",
        "--mcp-config",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    a.push(mcp_config.display().to_string());
    a.extend(["--strict-mcp-config", "--no-session-persistence", "--settings", r#"{"disableAllHooks":true}"#, "--system-prompt", instructions].map(String::from));
    // Never the user's `permissions.defaultMode` (bypassPermissions / auto would approve on their own): `dontAsk`
    // denies whatever `--allowedTools` does not pre-approve (Claude Code 2.1.292 `--help`: `manual` / `dontAsk`
    // are the modes that never approve by themselves; `default` is no longer listed). Without
    // `--permission-prompt-tool stdio` no `can_use_tool` request comes: `Claude::line`'s deny is defensive only.
    a.extend(["--permission-mode", "dontAsk"].map(String::from));
    if let Some(m) = model.filter(|m| !m.is_empty()) {
        a.extend(["--model".to_string(), m.to_string()]);
    }
    a
}

/// The `--mcp-config` file: `gilvt mcp` as the only server, with the socket and the token in its environment.
pub fn mcp_config(mcp: &McpLaunch) -> String {
    json!({"mcpServers": {"gilvt": {
        "type": "stdio",
        "command": mcp.gilvt.display().to_string(),
        "args": ["mcp"],
        "env": {"GILVT_SOCKET": mcp.socket.display().to_string(), "GILVT_MONITOR_TOKEN": mcp.token},
    }}})
    .to_string()
}

pub fn user_line(text: &str) -> String {
    json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text}]}, "parent_tool_use_id": null, "session_id": ""}).to_string()
}

pub fn interrupt_line(request_id: &str) -> String {
    json!({"type": "control_request", "request_id": request_id, "request": {"subtype": "interrupt"}}).to_string()
}

#[derive(Debug, Default)]
pub struct Claude {
    requests: u64,
    in_turn: bool,
    interrupting: bool,
    /// The assistant message the stream events belong to (`message_start`).
    message: Option<String>,
    unnamed: u64,
}

impl Claude {
    fn current_message(&mut self) -> String {
        if self.message.is_none() {
            self.unnamed += 1;
            self.message = Some(format!("m{}", self.unnamed));
        }
        self.message.clone().unwrap_or_default()
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

impl Protocol for Claude {
    fn start(&mut self) -> Vec<String> {
        Vec::new()
    }

    fn user(&mut self, text: &str) -> Vec<String> {
        self.in_turn = true;
        vec![user_line(text)]
    }

    fn interrupt(&mut self) -> Vec<String> {
        if !self.in_turn || self.interrupting {
            return Vec::new();
        }
        self.interrupting = true;
        self.requests += 1;
        vec![interrupt_line(&format!("gilvt-{}", self.requests))]
    }

    fn line(&mut self, line: &str) -> Decoded {
        let mut d = Decoded::default();
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            d.events.push(ChatEvent::BadLine(line.to_string()));
            return d;
        };
        let s = |p: &str| v.pointer(p).and_then(Value::as_str);
        match s("/type") {
            Some("system") if s("/subtype") == Some("init") => {
                let status = v
                    .get("mcp_servers")
                    .and_then(Value::as_array)
                    .and_then(|a| a.iter().find(|m| m.get("name").and_then(Value::as_str) == Some("gilvt")))
                    .and_then(|m| m.get("status").and_then(Value::as_str))
                    .unwrap_or("missing")
                    .to_string();
                let note = (status != "connected").then(|| format!("gilvt 工具没有连上（{status}）"));
                d.events.push(ChatEvent::Ready { model: s("/model").map(str::to_string), note });
            }
            Some("stream_event") => match s("/event/type") {
                Some("message_start") => self.message = s("/event/message/id").map(str::to_string),
                Some("content_block_delta") if s("/event/delta/type") == Some("text_delta") => {
                    let delta = s("/event/delta/text").unwrap_or("").to_string();
                    let message = self.current_message();
                    d.events.push(ChatEvent::Text { message, delta });
                }
                _ => {}
            },
            Some("assistant") if v.get("parent_tool_use_id").is_none_or(Value::is_null) => {
                let id = match s("/message/id") {
                    Some(id) => id.to_string(),
                    None => self.current_message(),
                };
                let blocks = v.pointer("/message/content").and_then(Value::as_array).cloned().unwrap_or_default();
                let kind = |b: &Value, k: &str| b.get("type").and_then(Value::as_str) == Some(k);
                let text: String = blocks.iter().filter(|b| kind(b, "text")).filter_map(|b| b.get("text").and_then(Value::as_str)).collect();
                if !text.is_empty() {
                    d.events.push(ChatEvent::TextDone { message: id, text });
                }
                for b in blocks.iter().filter(|b| kind(b, "tool_use")) {
                    let name = b.get("name").and_then(Value::as_str).unwrap_or("");
                    d.events.push(ChatEvent::ToolStarted {
                        id: b.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                        tool: name.strip_prefix(CLAUDE_PREFIX).unwrap_or(name).to_string(),
                        args: b.get("input").cloned().unwrap_or(Value::Null),
                    });
                }
            }
            Some("user") if v.get("parent_tool_use_id").is_none_or(Value::is_null) => {
                let blocks = v.pointer("/message/content").and_then(Value::as_array).cloned().unwrap_or_default();
                for b in blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result")) {
                    d.events.push(ChatEvent::ToolDone {
                        id: b.get("tool_use_id").and_then(Value::as_str).unwrap_or("").to_string(),
                        ok: !b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                        text: b.get("content").map(text_of).unwrap_or_default(),
                    });
                }
            }
            Some("result") => {
                let interrupting = std::mem::take(&mut self.interrupting);
                let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                // A turn that finished before the interrupt got there is an answer, not 「（这一轮已中断）」.
                let end = if s("/subtype") == Some("success") && !is_error {
                    TurnEnd::Done
                } else if interrupting {
                    TurnEnd::Interrupted
                } else if is_error {
                    TurnEnd::Failed(classify(s("/result").unwrap_or("这一轮出错了"), None))
                } else {
                    TurnEnd::Done
                };
                self.in_turn = false;
                self.message = None;
                d.events.push(ChatEvent::TurnEnded(end));
            }
            Some("control_request") => {
                let id = s("/request_id").unwrap_or("").to_string();
                let reply = if s("/request/subtype") == Some("can_use_tool") {
                    json!({"type": "control_response", "response": {"subtype": "success", "request_id": id,
                           "response": {"behavior": "deny", "message": "gilvt 的监控官是只读的，不能使用这个工具"}}})
                } else {
                    json!({"type": "control_response", "response": {"subtype": "error", "request_id": id, "error": "gilvt 不支持这个请求"}})
                };
                d.replies.push(reply.to_string());
            }
            // control_response (interrupt acks), system/status, keep-alives, subagent traffic …
            Some(_) => {}
            None => d.events.push(ChatEvent::BadLine(line.to_string())),
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderError;

    fn feed(c: &mut Claude, text: &str) -> Decoded {
        let mut all = Decoded::default();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let d = c.line(line);
            all.replies.extend(d.replies);
            all.events.extend(d.events);
        }
        all
    }

    #[test]
    fn args_are_stream_json_with_only_gilvt_tools() {
        let a = args(Some("sonnet"), "INSTR", Path::new("/run/mcp-1-0.json"));
        let joined = a.join(" ");
        for flag in [
            "-p",
            "--input-format stream-json",
            "--output-format stream-json",
            "--verbose",
            "--include-partial-messages",
            "--allowedTools mcp__gilvt",
            "--mcp-config /run/mcp-1-0.json",
            "--strict-mcp-config",
            "--no-session-persistence",
            "--system-prompt INSTR",
            "--model sonnet",
            // Never the user's `permissions.defaultMode` (it could be bypassPermissions / auto).
            "--permission-mode dontAsk",
        ] {
            assert!(joined.contains(flag), "{flag} in {joined}");
        }
        assert_eq!(a[a.iter().position(|x| x == "--tools").unwrap() + 1], "");
        assert_eq!(a[a.iter().position(|x| x == "--settings").unwrap() + 1], r#"{"disableAllHooks":true}"#);
        assert!(!args(None, "I", Path::new("/x")).contains(&"--model".to_string()));
    }

    #[test]
    fn mcp_config_runs_gilvt_mcp_with_socket_and_token() {
        let mcp = McpLaunch { gilvt: "/Apps/Gilvt.app/Contents/MacOS/gilvt".into(), socket: "/tmp/gilvt-501/9.sock".into(), token: "abc".into() };
        let v: Value = serde_json::from_str(&mcp_config(&mcp)).unwrap();
        let s = &v["mcpServers"]["gilvt"];
        assert_eq!(s["type"], "stdio");
        assert_eq!(s["command"], "/Apps/Gilvt.app/Contents/MacOS/gilvt");
        assert_eq!(s["args"], json!(["mcp"]));
        assert_eq!(s["env"]["GILVT_SOCKET"], "/tmp/gilvt-501/9.sock");
        assert_eq!(s["env"]["GILVT_MONITOR_TOKEN"], "abc");
        assert_eq!(v["mcpServers"].as_object().unwrap().len(), 1, "the only server");
    }

    #[test]
    fn user_and_interrupt_lines_are_json() {
        let u: Value = serde_json::from_str(&user_line("第一行\n\"引号\"")).unwrap();
        assert_eq!(u["type"], "user");
        assert_eq!(u["message"]["role"], "user");
        assert_eq!(u["message"]["content"][0]["text"], "第一行\n\"引号\"");
        let i: Value = serde_json::from_str(&interrupt_line("gilvt-1")).unwrap();
        assert_eq!(i["request"]["subtype"], "interrupt");
        assert_eq!(i["request_id"], "gilvt-1");
    }

    #[test]
    fn a_turn_with_a_tool_call() {
        let mut c = Claude::default();
        assert_eq!(c.user("hi").len(), 1);
        let d = feed(&mut c, include_str!("../../tests/fixtures/claude-chat-turn.jsonl"));
        assert!(d.replies.is_empty());
        assert_eq!(d.events[0], ChatEvent::Ready { model: Some("claude-sonnet-4-6".into()), note: None });
        assert_eq!(d.events[1], ChatEvent::Text { message: "msg_1".into(), delta: "我先看一下".into() });
        assert_eq!(d.events[3], ChatEvent::TextDone { message: "msg_1".into(), text: "我先看一下会话列表。".into() });
        assert_eq!(d.events[4], ChatEvent::ToolStarted { id: "toolu_1".into(), tool: "list_sessions".into(), args: json!({}) });
        assert!(matches!(&d.events[5], ChatEvent::ToolDone { id, ok: true, text } if id == "toolu_1" && text.contains("已列出 1 个会话")));
        assert_eq!(d.events[6], ChatEvent::Text { message: "msg_2".into(), delta: "只有 [zsh](gilvt://session/pane:3)。".into() });
        assert_eq!(d.events.last(), Some(&ChatEvent::TurnEnded(TurnEnd::Done)));
    }

    #[test]
    fn recorded_model_error() {
        let mut c = Claude::default();
        c.user("hi");
        let d = feed(&mut c, include_str!("../../tests/fixtures/claude-chat-model-error.jsonl"));
        assert_eq!(d.events[0], ChatEvent::Ready { model: Some("gilvt-no-such-model".into()), note: None });
        assert!(matches!(d.events.last(), Some(ChatEvent::TurnEnded(TurnEnd::Failed(ProviderError::Exited { .. })))), "{:?}", d.events);
        assert!(!d.events.iter().any(|e| matches!(e, ChatEvent::BadLine(_))), "status and control_response lines are not bad");
    }

    #[test]
    fn interrupt_ends_the_turn_as_interrupted() {
        let mut c = Claude::default();
        assert!(c.interrupt().is_empty(), "nothing to interrupt before a turn");
        c.user("hi");
        let lines = c.interrupt();
        assert_eq!(lines.len(), 1);
        assert!(c.interrupt().is_empty(), "once per turn");
        let d = c.line(r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":""}"#);
        assert_eq!(d.events, vec![ChatEvent::TurnEnded(TurnEnd::Interrupted)]);
        c.user("next");
        let d = c.line(r#"{"type":"result","subtype":"success","is_error":false,"result":"ok"}"#);
        assert_eq!(d.events, vec![ChatEvent::TurnEnded(TurnEnd::Done)], "the flag does not leak into the next turn");
    }

    #[test]
    fn a_turn_that_finished_before_the_interrupt_is_done() {
        let mut c = Claude::default();
        c.user("hi");
        assert_eq!(c.interrupt().len(), 1);
        // The answer was complete when the interrupt arrived: it is an answer, not 「（这一轮已中断）」.
        let d = c.line(r#"{"type":"result","subtype":"success","is_error":false,"result":"ok"}"#);
        assert_eq!(d.events, vec![ChatEvent::TurnEnded(TurnEnd::Done)]);
        c.user("next");
        assert_eq!(c.interrupt().len(), 1, "the flag was cleared: the next turn can be interrupted");
        let d = c.line(r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":""}"#);
        assert_eq!(d.events, vec![ChatEvent::TurnEnded(TurnEnd::Interrupted)]);
    }

    #[test]
    fn auth_failure_and_a_disconnected_mcp_server() {
        let mut c = Claude::default();
        c.user("hi");
        let d = c.line(r#"{"type":"system","subtype":"init","model":"m","mcp_servers":[{"name":"gilvt","status":"failed"}]}"#);
        assert_eq!(d.events, vec![ChatEvent::Ready { model: Some("m".into()), note: Some("gilvt 工具没有连上（failed）".into()) }]);
        let d = c.line(r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#);
        assert!(matches!(&d.events[0], ChatEvent::TurnEnded(TurnEnd::Failed(ProviderError::Auth(_)))));
    }

    #[test]
    fn permission_requests_are_denied_and_bad_lines_reported() {
        let mut c = Claude::default();
        let d = c.line(r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{}}}"#);
        let reply: Value = serde_json::from_str(&d.replies[0]).unwrap();
        assert_eq!(reply["response"]["request_id"], "r1");
        assert_eq!(reply["response"]["response"]["behavior"], "deny");
        let d = c.line(r#"{"type":"control_request","request_id":"r2","request":{"subtype":"hook_callback"}}"#);
        let reply: Value = serde_json::from_str(&d.replies[0]).unwrap();
        assert_eq!(reply["response"]["subtype"], "error");
        assert_eq!(c.line("Error: something").events, vec![ChatEvent::BadLine("Error: something".into())]);
        assert_eq!(c.line(r#"{"no":"type"}"#).events.len(), 1, "JSON without a type is bad too");
    }

    #[test]
    fn subagent_messages_are_not_the_answer() {
        let mut c = Claude::default();
        c.user("hi");
        let d = c.line(r#"{"type":"assistant","message":{"id":"x","content":[{"type":"text","text":"inner"}]},"parent_tool_use_id":"toolu_9"}"#);
        assert!(d.events.is_empty());
    }
}
