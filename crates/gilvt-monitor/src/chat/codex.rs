//! `codex app-server` as a conversation (S2 §6.2): JSON-RPC 2.0 without the `jsonrpc` field, one object per line.
//! `initialize` → `initialized` → `thread/start { read-only, never, ephemeral }` → `turn/start` per message;
//! `turn/interrupt { threadId, turnId }`. Before starting, `codex features list` must show the switches that take
//! Codex's shell away (总体设计 §4.1). Shapes: codex-cli 0.160.0 — `tests/fixtures/codex-app-server-*.jsonl`.

use std::path::PathBuf;

use serde_json::{json, Value};

use super::{ChatEvent, Decoded, McpLaunch, Protocol, TurnEnd};
use crate::provider::codex::mcp_off_args;
use crate::provider::{classify, ProviderError};

/// Without these turned off the 监控官 could run commands: refuse to chat when any cannot be.
pub const REQUIRED_FEATURES: [&str; 3] = ["shell_tool", "unified_exec", "hooks"];
/// Turned off when this Codex has them (总体设计 §4.1); missing ones are only logged.
pub const OPTIONAL_FEATURES: [&str; 12] =
    ["view_image", "browser_use", "computer_use", "multi_agent", "image_generation", "apps", "plugins", "tool_suggest", "skill_search", "sleep_tool", "goals", "in_app_browser"];

const INIT_ID: u64 = 1;
const THREAD_ID: u64 = 2;

pub fn features_args() -> Vec<String> {
    vec!["features".into(), "list".into()]
}

/// `codex features list`: `<name> <stage, maybe two words> <true|false>` per line → (name, stage).
pub fn parse_features(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let words: Vec<&str> = l.split_whitespace().collect();
            if words.len() < 3 || !matches!(words[words.len() - 1], "true" | "false") {
                return None;
            }
            Some((words[0].to_string(), words[1..words.len() - 1].join(" ")))
        })
        .collect()
}

#[derive(Debug, PartialEq)]
pub struct Features {
    /// Optional switches this Codex has: passed as `--disable`.
    pub disable: Vec<String>,
    /// Optional switches it does not have (logged).
    pub missing: Vec<String>,
}

/// Whether this Codex can be made read-only. A switch is usable when it is listed and not `removed` (its value
/// after `--disable` is not reliable: 0.160 still lists `unified_exec … true`).
pub fn check_features(list: &str) -> Result<Features, ProviderError> {
    let features = parse_features(list);
    let usable = |name: &str| features.iter().any(|(n, stage)| n == name && stage != "removed");
    let lacking: Vec<&str> = REQUIRED_FEATURES.iter().copied().filter(|f| !usable(f)).collect();
    if !lacking.is_empty() {
        return Err(ProviderError::Unsupported(if gilvt_i18n::english() {
            format!(
                "This Codex has no {} switch. To make sure the Monitor can only read data through gilvt's read-only tools, chat is turned off (✦ summaries are not affected). Switch to Claude in Settings, or update gilvt.",
                lacking.join(", ")
            )
        } else {
            format!(
                "当前 Codex 里找不到 {} 这个开关。为了保证监控官只能通过 gilvt 的只读工具查数据，对话已停用（✦ 总结不受影响）。可以在设置里改用 Claude，或升级 gilvt。",
                lacking.join("、")
            )
        }));
    }
    let (disable, missing): (Vec<&str>, Vec<&str>) = OPTIONAL_FEATURES.iter().partition(|f| usable(f));
    Ok(Features { disable: disable.into_iter().map(String::from).collect(), missing: missing.into_iter().map(String::from).collect() })
}

/// `user_servers`: the MCP servers in the user's own Codex config, switched off so the 监控官 has only gilvt's tools.
/// That off-table is one `-c mcp_servers={…}` inline value and must come BEFORE the dotted `mcp_servers.gilvt.*`
/// keys (a table value after them would replace gilvt's entry). A user server named `gilvt` is not switched off:
/// it merges into ours.
pub fn args(optional: &[String], mcp: &McpLaunch, user_servers: &[String]) -> Vec<String> {
    let mut a = Vec::new();
    for f in REQUIRED_FEATURES.iter().map(|s| s.to_string()).chain(optional.iter().cloned()) {
        a.push("--disable".to_string());
        a.push(f);
    }
    a.push("-c".to_string());
    a.push(r#"web_search="disabled""#.to_string());
    // The user's `notify` program would get every answer and inherit GILVT_MONITOR_TOKEN from our environment.
    a.push("-c".to_string());
    a.push("notify=[]".to_string());
    let others: Vec<String> = user_servers.iter().filter(|s| *s != "gilvt").cloned().collect();
    a.extend(mcp_off_args(&others));
    // A JSON string is a TOML basic string.
    let command = serde_json::to_string(&mcp.gilvt.display().to_string()).unwrap_or_default();
    for c in [
        format!("mcp_servers.gilvt.command={command}"),
        r#"mcp_servers.gilvt.args=["mcp"]"#.to_string(),
        r#"mcp_servers.gilvt.env_vars=["GILVT_SOCKET","GILVT_MONITOR_TOKEN"]"#.to_string(),
        r#"mcp_servers.gilvt.default_tools_approval_mode="approve""#.to_string(),
    ] {
        a.push("-c".to_string());
        a.push(c);
    }
    a.push("app-server".to_string());
    a
}

/// The app-server's own environment additions: Codex hands exactly these to `gilvt mcp` (`env_vars`), so the token
/// never appears on a command line.
pub fn env(mcp: &McpLaunch) -> Vec<(String, String)> {
    vec![("GILVT_SOCKET".into(), mcp.socket.display().to_string()), ("GILVT_MONITOR_TOKEN".into(), mcp.token.clone())]
}

#[derive(Debug)]
enum Phase {
    Initializing,
    StartingThread,
    Ready { thread: String },
    Failed,
}

#[derive(Debug)]
pub struct Codex {
    model: Option<String>,
    instructions: String,
    cwd: PathBuf,
    next_id: u64,
    phase: Phase,
    /// Messages sent before the thread existed (sent as one turn when it does).
    queued: Vec<String>,
    turn: Option<String>,
    turn_requested: bool,
    interrupt_pending: bool,
    interrupting: bool,
    last_error: Option<String>,
}

fn rpc_failure(what: &str, err: &Value) -> ProviderError {
    let message = err.get("message").and_then(Value::as_str).unwrap_or("").to_string();
    match classify(&message, None) {
        auth @ ProviderError::Auth(_) => auth,
        _ => ProviderError::Protocol(format!("{what}{}{message}", gilvt_i18n::text("：", ": "))),
    }
}

fn content_text(v: Option<&Value>) -> String {
    v.and_then(Value::as_array)
        .map(|blocks| blocks.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

impl Codex {
    pub fn new(model: Option<String>, instructions: String, cwd: PathBuf) -> Codex {
        Codex {
            model: model.filter(|m| !m.is_empty()),
            instructions,
            cwd,
            next_id: 3,
            phase: Phase::Initializing,
            queued: Vec::new(),
            turn: None,
            turn_requested: false,
            interrupt_pending: false,
            interrupting: false,
            last_error: None,
        }
    }

    fn request(&mut self, method: &str, params: Value) -> String {
        let id = self.next_id;
        self.next_id += 1;
        json!({"id": id, "method": method, "params": params}).to_string()
    }

    fn turn_start(&mut self, thread: &str, text: &str) -> String {
        self.turn = None;
        self.turn_requested = true;
        self.last_error = None;
        self.request("turn/start", json!({"threadId": thread, "input": [{"type": "text", "text": text}]}))
    }

    fn thread(&self) -> Option<String> {
        match &self.phase {
            Phase::Ready { thread } => Some(thread.clone()),
            _ => None,
        }
    }

    /// The current turn's id is known: send an interrupt asked for before it was.
    fn got_turn(&mut self, turn: &str, d: &mut Decoded) {
        if self.turn.is_some() || !self.turn_requested {
            return;
        }
        self.turn = Some(turn.to_string());
        if std::mem::take(&mut self.interrupt_pending) {
            if let Some(thread) = self.thread() {
                self.interrupting = true;
                d.replies.push(self.request("turn/interrupt", json!({"threadId": thread, "turnId": turn})));
            }
        }
    }

    fn end_turn(&mut self, end: TurnEnd, d: &mut Decoded) {
        self.turn = None;
        self.turn_requested = false;
        self.interrupt_pending = false;
        self.interrupting = false;
        d.events.push(ChatEvent::TurnEnded(end));
    }

    fn notification(&mut self, method: &str, p: &Value, d: &mut Decoded) {
        let s = |path: &str| p.pointer(path).and_then(Value::as_str);
        match method {
            "turn/started" => {
                if let Some(t) = s("/turn/id") {
                    self.got_turn(t, d);
                }
            }
            "item/agentMessage/delta" => d.events.push(ChatEvent::Text { message: s("/itemId").unwrap_or("").into(), delta: s("/delta").unwrap_or("").into() }),
            "item/started" if s("/item/type") == Some("mcpToolCall") && s("/item/server") == Some("gilvt") => d.events.push(ChatEvent::ToolStarted {
                id: s("/item/id").unwrap_or("").into(),
                tool: s("/item/tool").unwrap_or("").into(),
                args: p.pointer("/item/arguments").cloned().unwrap_or(Value::Null),
            }),
            "item/completed" => match s("/item/type") {
                Some("agentMessage") => d.events.push(ChatEvent::TextDone { message: s("/item/id").unwrap_or("").into(), text: s("/item/text").unwrap_or("").into() }),
                Some("mcpToolCall") if s("/item/server") == Some("gilvt") => {
                    let ok = s("/item/status") == Some("completed");
                    let text = if ok { content_text(p.pointer("/item/result/content")) } else { s("/item/error/message").unwrap_or(gilvt_i18n::text("工具调用失败", "The tool call failed")).to_string() };
                    d.events.push(ChatEvent::ToolDone { id: s("/item/id").unwrap_or("").into(), ok, text });
                }
                _ => {}
            },
            "turn/completed" => {
                // By its status only: a turn that completed before an interrupt got there is Done (the interrupt
                // flags are cleared by `end_turn` either way).
                let end = match s("/turn/status") {
                    Some("interrupted") => TurnEnd::Interrupted,
                    Some("failed") => {
                        let message = s("/turn/error/message").map(str::to_string).or_else(|| self.last_error.take()).unwrap_or_else(|| gilvt_i18n::text("这一轮失败了", "This turn failed").into());
                        TurnEnd::Failed(classify(&message, None))
                    }
                    _ => TurnEnd::Done,
                };
                self.end_turn(end, d);
            }
            "error" if p.get("willRetry").and_then(Value::as_bool) != Some(true) => self.last_error = s("/error/message").map(str::to_string),
            "mcpServer/startupStatus/updated" if s("/name") == Some("gilvt") && s("/status") == Some("failed") => {
                let why = s("/error").or(s("/failureReason")).unwrap_or(gilvt_i18n::text("未知原因", "unknown reason"));
                d.events.push(ChatEvent::Notice(if gilvt_i18n::english() {
                    format!("gilvt's tools did not connect: {why}")
                } else {
                    format!("gilvt 工具没有连上：{why}")
                }));
            }
            _ => {}
        }
    }
}

impl Protocol for Codex {
    fn start(&mut self) -> Vec<String> {
        vec![json!({"id": INIT_ID, "method": "initialize", "params": {"clientInfo": {"name": "gilvt", "version": env!("CARGO_PKG_VERSION")}}}).to_string()]
    }

    fn user(&mut self, text: &str) -> Vec<String> {
        match self.thread() {
            Some(thread) => vec![self.turn_start(&thread, text)],
            None => {
                self.queued.push(text.to_string());
                Vec::new()
            }
        }
    }

    fn interrupt(&mut self) -> Vec<String> {
        let Some(thread) = self.thread() else {
            self.queued.clear();
            return Vec::new();
        };
        if self.interrupting {
            return Vec::new();
        }
        match self.turn.clone() {
            Some(turn) => {
                self.interrupting = true;
                vec![self.request("turn/interrupt", json!({"threadId": thread, "turnId": turn}))]
            }
            None => {
                self.interrupt_pending = self.turn_requested;
                Vec::new()
            }
        }
    }

    fn line(&mut self, line: &str) -> Decoded {
        let mut d = Decoded::default();
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            d.events.push(ChatEvent::BadLine(line.to_string()));
            return d;
        };
        if let Some(method) = v.get("method").and_then(Value::as_str) {
            if let Some(id) = v.get("id") {
                // A request from the server (approvals, elicitations, user input): the 监控官 is read-only.
                d.replies.push(json!({"id": id, "error": {"code": -32601, "message": "gilvt 的监控官是只读的，不处理这个请求"}}).to_string());
            } else {
                let params = v.get("params").cloned().unwrap_or(Value::Null);
                self.notification(method, &params, &mut d);
            }
            return d;
        }
        match v.get("id").and_then(Value::as_u64) {
            Some(INIT_ID) => match v.get("error") {
                Some(e) => {
                    self.phase = Phase::Failed;
                    d.events.push(ChatEvent::Failed(rpc_failure("initialize", e)));
                }
                None => {
                    self.phase = Phase::StartingThread;
                    d.replies.push(json!({"method": "initialized"}).to_string());
                    let mut params = json!({"sandbox": "read-only", "approvalPolicy": "never", "ephemeral": true, "developerInstructions": self.instructions, "cwd": self.cwd.display().to_string()});
                    if let Some(m) = &self.model {
                        params["model"] = json!(m);
                    }
                    d.replies.push(json!({"id": THREAD_ID, "method": "thread/start", "params": params}).to_string());
                }
            },
            Some(THREAD_ID) => match v.pointer("/result/thread/id").and_then(Value::as_str) {
                Some(thread) => {
                    self.phase = Phase::Ready { thread: thread.to_string() };
                    let model = v.pointer("/result/model").and_then(Value::as_str).map(str::to_string);
                    d.events.push(ChatEvent::Ready { model, note: None });
                    if !self.queued.is_empty() {
                        let text = std::mem::take(&mut self.queued).join("\n\n");
                        let thread = thread.to_string();
                        d.replies.push(self.turn_start(&thread, &text));
                    }
                }
                None => {
                    self.phase = Phase::Failed;
                    d.events.push(ChatEvent::Failed(rpc_failure("thread/start", v.get("error").unwrap_or(&Value::Null))));
                }
            },
            Some(_) => {
                if let Some(turn) = v.pointer("/result/turn/id").and_then(Value::as_str) {
                    self.got_turn(turn, &mut d);
                } else if let (Some(e), true, None) = (v.get("error"), self.turn_requested, &self.turn) {
                    // turn/start refused: the turn never began.
                    let failure = rpc_failure("turn/start", e);
                    self.end_turn(TurnEnd::Failed(failure), &mut d);
                }
            }
            None => d.events.push(ChatEvent::BadLine(line.to_string())),
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderError;

    fn mcp() -> McpLaunch {
        McpLaunch { gilvt: "/Apps/Gilvt.app/Contents/MacOS/gilvt".into(), socket: "/tmp/gilvt-501/9.sock".into(), token: "secret-token".into() }
    }

    fn feed(c: &mut Codex, text: &str) -> Decoded {
        let mut all = Decoded::default();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let d = c.line(line);
            all.replies.extend(d.replies);
            all.events.extend(d.events);
        }
        all
    }

    fn ready() -> Codex {
        let mut c = Codex::new(Some("gpt-5.5".into()), "INSTR".into(), "/tmp/run".into());
        c.start();
        c.line(r#"{"id":1,"result":{"userAgent":"x"}}"#);
        c.line(r#"{"id":2,"result":{"thread":{"id":"th-1"},"model":"gpt-5.5"}}"#);
        c
    }

    #[test]
    fn features_are_read_like_the_cli_prints_them() {
        let f = parse_features(include_str!("../../tests/fixtures/codex-features-list.txt"));
        assert!(f.contains(&("code_mode".to_string(), "under development".to_string())), "two-word stages");
        let ok = check_features(include_str!("../../tests/fixtures/codex-features-list.txt")).unwrap();
        assert_eq!(ok.disable.len(), OPTIONAL_FEATURES.len(), "{ok:?}");
        assert!(ok.missing.is_empty());
    }

    #[test]
    fn missing_shell_tool_is_unsupported() {
        let list: String = include_str!("../../tests/fixtures/codex-features-list.txt").lines().filter(|l| !l.starts_with("shell_tool")).map(|l| format!("{l}\n")).collect();
        match check_features(&list) {
            Err(ProviderError::Unsupported(m)) => assert!(m.contains("shell_tool") && m.contains("总结不受影响"), "{m}"),
            other => panic!("{other:?}"),
        }
        let removed = "shell_tool removed false\nunified_exec stable true\nhooks stable true\n";
        assert!(matches!(check_features(removed), Err(ProviderError::Unsupported(_))), "a removed switch cannot be turned off");
    }

    #[test]
    fn optional_switches_only_when_listed() {
        let list = "shell_tool stable true\nunified_exec stable true\nhooks stable true\nview_image stable true\nplugins removed false\n";
        let f = check_features(list).unwrap();
        assert_eq!(f.disable, vec!["view_image".to_string()]);
        assert!(f.missing.contains(&"plugins".to_string()) && f.missing.contains(&"apps".to_string()));
    }

    #[test]
    fn args_turn_everything_off_and_start_gilvt_mcp() {
        let a = args(&["view_image".to_string()], &mcp(), &[]);
        let joined = a.join(" ");
        for flag in [
            "--disable shell_tool",
            "--disable unified_exec",
            "--disable hooks",
            "--disable view_image",
            "-c web_search=\"disabled\"",
            "-c notify=[]",
            "-c mcp_servers.gilvt.command=\"/Apps/Gilvt.app/Contents/MacOS/gilvt\"",
            "-c mcp_servers.gilvt.args=[\"mcp\"]",
            "-c mcp_servers.gilvt.env_vars=[\"GILVT_SOCKET\",\"GILVT_MONITOR_TOKEN\"]",
            "-c mcp_servers.gilvt.default_tools_approval_mode=\"approve\"",
        ] {
            assert!(joined.contains(flag), "{flag} in {joined}");
        }
        assert_eq!(a.last().map(String::as_str), Some("app-server"));
    }

    #[test]
    fn token_is_not_on_the_command_line() {
        assert!(!args(&[], &mcp(), &[]).iter().any(|a| a.contains("secret-token")));
        assert_eq!(env(&mcp()), vec![("GILVT_SOCKET".to_string(), "/tmp/gilvt-501/9.sock".to_string()), ("GILVT_MONITOR_TOKEN".to_string(), "secret-token".to_string())]);
    }

    #[test]
    fn recorded_handshake() {
        let mut c = Codex::new(None, "INSTR".into(), "/tmp/run".into());
        let first: Value = serde_json::from_str(&c.start()[0]).unwrap();
        assert_eq!((first["id"].clone(), first["method"].clone()), (json!(1), json!("initialize")));
        assert!(c.user("早").is_empty(), "queued until the thread exists");
        let d = feed(&mut c, include_str!("../../tests/fixtures/codex-app-server-handshake.jsonl"));
        let sent: Vec<Value> = d.replies.iter().map(|r| serde_json::from_str(r).unwrap()).collect();
        assert_eq!(sent[0], json!({"method": "initialized"}));
        assert_eq!(sent[1]["method"], "thread/start");
        assert_eq!(sent[1]["params"]["sandbox"], "read-only");
        assert_eq!(sent[1]["params"]["approvalPolicy"], "never");
        assert_eq!(sent[1]["params"]["ephemeral"], true);
        assert_eq!(sent[1]["params"]["developerInstructions"], "INSTR");
        assert_eq!(sent[1]["params"]["cwd"], "/tmp/run");
        assert!(sent[1]["params"].get("model").is_none(), "no model = the CLI's default");
        assert_eq!(sent[2]["method"], "turn/start", "the queued message goes out once the thread exists");
        assert_eq!(sent[2]["params"]["threadId"], "01a1102b-9d08-7d03-9f33-f84edc56c80e");
        assert_eq!(sent[2]["params"]["input"][0], json!({"type": "text", "text": "早"}));
        assert!(d.events.contains(&ChatEvent::Ready { model: Some("gpt-5.6-sol".into()), note: None }));
        assert!(!d.events.iter().any(|e| matches!(e, ChatEvent::BadLine(_) | ChatEvent::Failed(_))), "{:?}", d.events);
    }

    #[test]
    fn a_turn_with_a_tool_call() {
        let mut c = ready();
        assert_eq!(c.user("哪些需要我？").len(), 1);
        let d = feed(&mut c, include_str!("../../tests/fixtures/codex-app-server-turn.jsonl"));
        assert_eq!(
            d.events,
            vec![
                ChatEvent::ToolStarted { id: "call-1".into(), tool: "list_sessions".into(), args: json!({}) },
                ChatEvent::ToolDone { id: "call-1".into(), ok: true, text: "{\"gilvt_label\":\"已列出 1 个会话\",\"sessions\":[]}".into() },
                ChatEvent::Text { message: "msg-1".into(), delta: "api-refactor ".into() },
                ChatEvent::Text { message: "msg-1".into(), delta: "在等你批准。".into() },
                ChatEvent::TextDone { message: "msg-1".into(), text: "api-refactor 在等你批准。".into() },
                ChatEvent::TurnEnded(TurnEnd::Done),
            ]
        );
    }

    #[test]
    fn interrupt_before_the_turn_id_is_sent_when_it_arrives() {
        let mut c = ready();
        c.user("a");
        assert!(c.interrupt().is_empty(), "no turn id yet");
        let d = c.line(r#"{"id":3,"result":{"turn":{"id":"turn-7","items":[],"status":"inProgress"}}}"#);
        let sent: Value = serde_json::from_str(&d.replies[0]).unwrap();
        assert_eq!(sent["method"], "turn/interrupt");
        assert_eq!(sent["params"], json!({"threadId": "th-1", "turnId": "turn-7"}));
        let d = c.line(r#"{"method":"turn/completed","params":{"threadId":"th-1","turn":{"id":"turn-7","items":[],"status":"interrupted"}}}"#);
        assert_eq!(d.events, vec![ChatEvent::TurnEnded(TurnEnd::Interrupted)]);
        c.user("b");
        let d = c.line(r#"{"method":"turn/started","params":{"threadId":"th-1","turn":{"id":"turn-8","items":[],"status":"inProgress"}}}"#);
        assert!(d.replies.is_empty(), "the pending interrupt was used up");
    }

    #[test]
    fn a_turn_that_completed_before_the_interrupt_is_done() {
        let mut c = ready();
        c.user("a");
        c.line(r#"{"id":3,"result":{"turn":{"id":"turn-7","items":[],"status":"inProgress"}}}"#);
        assert_eq!(c.interrupt().len(), 1);
        let d = c.line(r#"{"method":"turn/completed","params":{"threadId":"th-1","turn":{"id":"turn-7","items":[],"status":"completed"}}}"#);
        assert_eq!(d.events, vec![ChatEvent::TurnEnded(TurnEnd::Done)]);
        // The late answer to turn/interrupt (an error: no turn running) ends nothing.
        assert!(c.line(r#"{"id":4,"error":{"code":-32600,"message":"no active turn"}}"#).events.is_empty());
        c.user("b");
        c.line(r#"{"id":5,"result":{"turn":{"id":"turn-8","items":[],"status":"inProgress"}}}"#);
        assert_eq!(c.interrupt().len(), 1, "the next turn can be interrupted");
    }

    #[test]
    fn failures() {
        let mut c = ready();
        c.user("a");
        c.line(r#"{"method":"error","params":{"threadId":"th-1","turnId":"t","willRetry":false,"error":{"message":"401 Unauthorized"}}}"#);
        let d = c.line(r#"{"method":"turn/completed","params":{"threadId":"th-1","turn":{"id":"t","items":[],"status":"failed","error":null}}}"#);
        assert!(matches!(&d.events[0], ChatEvent::TurnEnded(TurnEnd::Failed(ProviderError::Auth(_)))), "{:?}", d.events);
        let mut c = Codex::new(None, "I".into(), "/r".into());
        c.start();
        c.line(r#"{"id":1,"result":{}}"#);
        let d = c.line(r#"{"id":2,"error":{"code":-32603,"message":"model not available"}}"#);
        assert_eq!(d.events, vec![ChatEvent::Failed(ProviderError::Protocol("thread/start：model not available".into()))]);
        let d = c.line(r#"{"method":"mcpServer/startupStatus/updated","params":{"name":"gilvt","status":"failed","error":"spawn failed"}}"#);
        assert_eq!(d.events, vec![ChatEvent::Notice("gilvt 工具没有连上：spawn failed".into())]);
    }

    #[test]
    fn server_requests_are_refused_and_garbage_reported() {
        let mut c = ready();
        let d = c.line(r#"{"id":"r1","method":"item/commandExecution/requestApproval","params":{}}"#);
        let reply: Value = serde_json::from_str(&d.replies[0]).unwrap();
        assert_eq!(reply["id"], "r1");
        assert_eq!(reply["error"]["code"], -32601);
        assert_eq!(c.line("WARN something").events, vec![ChatEvent::BadLine("WARN something".into())]);
    }

    #[test]
    fn chosen_model_goes_into_thread_start() {
        let mut c = Codex::new(Some("gpt-5.5".into()), "I".into(), "/r".into());
        c.start();
        let d = c.line(r#"{"id":1,"result":{}}"#);
        let start: Value = serde_json::from_str(&d.replies[1]).unwrap();
        assert_eq!(start["params"]["model"], "gpt-5.5");
    }

    #[test]
    fn the_users_own_mcp_servers_are_off_before_gilvt_is_defined() {
        let users = vec!["github".to_string(), "gilvt".to_string(), "we\"ird".to_string()];
        let a = args(&[], &mcp(), &users);
        let table = a.iter().position(|x| x.starts_with("mcp_servers={")).expect("an off-table");
        assert_eq!(a[table - 1], "-c");
        assert_eq!(a[table], r#"mcp_servers={"github"={enabled=false},"we\"ird"={enabled=false}}"#, "gilvt is not switched off");
        let command = a.iter().position(|x| x.starts_with("mcp_servers.gilvt.command=")).unwrap();
        let web = a.iter().position(|x| x.starts_with("web_search=")).unwrap();
        assert!(web < table && table < command, "an inline table after the dotted keys would replace them: {a:?}");
        assert!(!args(&[], &mcp(), &["gilvt".to_string()]).iter().any(|x| x.starts_with("mcp_servers={")), "nothing to turn off");
    }
}
