//! The fake brain as a conversation (S2 §9): `claude -p --input-format stream-json …` and `codex app-server`'s
//! threads and turns. Each user message runs a scripted turn: call gilvt's tools through MCP (the real
//! `gilvt mcp`, started from the CLI's MCP configuration), then stream an answer in that CLI's own format.
//! Control words: any line of `$HOME/.gilvt-fake-brain` (see [`Control`]). `missing-*` models fail in chat too.
//!
//! Which control words apply where: `chat-slow` and `chat-auth` apply to Claude and Codex; `chat-exit` and
//! `chat-hang` only to Claude ([`CodexChat`] ignores them: its turns run to the end in order, no interrupt
//! mid-turn), so GUI cases use exit / hang with Claude only. The chat writes only [`CHAT_LOG`].

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::brain::{self, MISSING_MODEL_PREFIX};
use crate::Agent;
use crate::mcp_client::{self, McpClient};

pub const CHAT_LOG: &str = ".gilvt-fake-brain-chat.log";
/// What the fake `codex features list` prints (gilvt's required and optional switches).
pub const FAKE_FEATURES: [&str; 15] = [
    "shell_tool", "unified_exec", "hooks", "view_image", "browser_use", "computer_use", "multi_agent", "image_generation", "apps", "plugins", "tool_suggest",
    "skill_search", "sleep_tool", "goals", "in_app_browser",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Control {
    /// `chat-slow`: 1.5 s between the streamed pieces (else 80 ms).
    pub slow: bool,
    /// `chat-exit`: exit 1 after the first piece.
    pub exit: bool,
    /// `chat-hang`: after the first piece, wait for an interrupt (Claude only).
    pub hang: bool,
    /// `chat-auth`: the turn ends with an authentication error.
    pub auth: bool,
}

fn words(home: Option<&Path>) -> Vec<String> {
    home.and_then(|h| std::fs::read_to_string(h.join(crate::brain::CONTROL_FILE)).ok())
        .map(|t| t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}

impl Control {
    pub fn read(home: Option<&Path>) -> Control {
        let w = words(home);
        let has = |x: &str| w.iter().any(|y| y == x);
        Control { slow: has("chat-slow"), exit: has("chat-exit"), hang: has("chat-hang"), auth: has("chat-auth") }
    }

    fn pause(&self) -> Duration {
        Duration::from_millis(if self.slow { 1500 } else { 80 })
    }
}

pub fn is_claude_chat(args: &[String]) -> bool {
    args.iter().any(|a| a == "-p" || a == "--print") && args.windows(2).any(|w| w[0] == "--input-format" && w[1] == "stream-json")
}

pub fn features_list(home: Option<&Path>) -> String {
    let no_shell = words(home).iter().any(|w| w == "features-no-shell");
    FAKE_FEATURES.iter().filter(|f| !(no_shell && **f == "shell_tool")).map(|f| format!("{f:<40} stable             true\n")).collect()
}

fn log(home: Option<&Path>, line: &str) {
    let Some(home) = home else { return };
    let mut text = std::fs::read_to_string(home.join(CHAT_LOG)).unwrap_or_default();
    text.push_str(line);
    text.push('\n');
    let _ = std::fs::write(home.join(CHAT_LOG), text);
}

pub fn scope_keys(message: &str) -> Vec<String> {
    let first = message.lines().next().unwrap_or("");
    first.strip_prefix("<scope keys=\"").and_then(|r| r.split_once('"')).map(|(keys, _)| keys.split_whitespace().map(String::from).collect()).unwrap_or_default()
}

pub fn plan(message: &str) -> Vec<(String, Value)> {
    if message.contains("站会简报") {
        return vec![("list_sessions".into(), json!({}))];
    }
    if let Some(k) = scope_keys(message).first() {
        return vec![("get_session".into(), json!({"key": k})), ("get_timeline".into(), json!({"key": k, "turns": "last:2"}))];
    }
    vec![("list_sessions".into(), json!({}))]
}

pub fn answer(message: &str, results: &[(String, bool, String)]) -> String {
    let parsed = |tool: &str| results.iter().find(|(t, ok, _)| t == tool && *ok).and_then(|(_, _, text)| serde_json::from_str::<Value>(text).ok());
    let sessions: Vec<Value> = parsed("list_sessions").and_then(|v| v.get("sessions").and_then(Value::as_array).cloned()).unwrap_or_default();
    let link = |s: &Value| format!("[{}](gilvt://session/{})", s["name"].as_str().unwrap_or("?"), s["key"].as_str().unwrap_or(""));
    if message.contains("站会简报") {
        let need: Vec<String> = sessions
            .iter()
            .filter(|s| matches!(s["group"].as_str(), Some("needs_you" | "error")))
            .map(|s| format!("- {}：{}", link(s), s["status"].as_str().unwrap_or("")))
            .collect();
        let body = if need.is_empty() { "- 暂时没有".to_string() } else { need.join("\n") };
        let running = sessions.iter().filter(|s| s["group"] == "running").count();
        return format!("### 要你处理\n{body}\n\n### 整体\n共 {} 个会话，{running} 个在跑（fake 简报）。", sessions.len());
    }
    if message.contains("list_sessions") {
        return "ok".into();
    }
    if let Some(key) = scope_keys(message).first() {
        let name = parsed("get_session").and_then(|v| v.pointer("/session/name").and_then(Value::as_str).map(String::from)).unwrap_or_else(|| key.clone());
        return format!("**{name}** 的情况（第 1 段）：fake 读过它的概况和最近两轮。\n\n[{name}](gilvt://session/{key}) 现在没有卡住（第 2 段）。\n\n以上（完）");
    }
    format!("现在有 {} 个会话（完）", sessions.len())
}

/// The pieces the answer is streamed in: its paragraphs (with their separators), or two halves.
pub fn chunks(answer: &str) -> Vec<String> {
    let parts: Vec<&str> = answer.split("\n\n").collect();
    if parts.len() >= 2 {
        let last = parts.len() - 1;
        return parts.iter().enumerate().map(|(i, p)| if i < last { format!("{p}\n\n") } else { p.to_string() }).collect();
    }
    let chars: Vec<char> = answer.chars().collect();
    if chars.len() < 2 {
        return vec![answer.to_string()];
    }
    let mid = chars.len() / 2;
    vec![chars[..mid].iter().collect(), chars[mid..].iter().collect()]
}

/// The model the one-shot helpers would refuse (`missing-*`), with the real CLI's message for it.
fn missing_model_message(agent: Agent, model: Option<&str>) -> Option<String> {
    let model = model.filter(|m| m.starts_with(MISSING_MODEL_PREFIX))?;
    let line = brain::missing_model(agent, model).stdout;
    let v: Value = line.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|v| v["is_error"] == true || v["type"] == "error")?;
    v["result"].as_str().or(v["message"].as_str()).map(String::from)
}

fn emit(out: &mut dyn Write, v: Value) {
    let _ = writeln!(out, "{v}").and_then(|()| out.flush());
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

/// Waits `wait`, answering control requests; true when an interrupt came. Other messages are kept for later.
fn wait_for_interrupt(rx: &mpsc::Receiver<Value>, wait: Duration, pending: &mut VecDeque<Value>, out: &mut dyn Write) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        match rx.recv_timeout(left) {
            Ok(v) if v["type"] == "control_request" => {
                emit(out, json!({"type": "control_response", "response": {"subtype": "success", "request_id": v["request_id"], "response": {"still_queued": []}}}));
                if v.pointer("/request/subtype") == Some(&json!("interrupt")) {
                    return true;
                }
            }
            Ok(v) => pending.push_back(v),
            Err(mpsc::RecvTimeoutError::Timeout) => return false,
            // stdin is closed: nothing can interrupt any more; the turn still finishes (as the real CLI does).
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(left);
                return false;
            }
        }
    }
}

/// `claude -p --input-format stream-json --output-format stream-json …`. Returns the exit code.
pub fn claude_chat(args: &[String], home: Option<&Path>, input: impl BufRead + Send + 'static, out: &mut dyn Write) -> i32 {
    let partial = args.iter().any(|a| a == "--include-partial-messages");
    let missing = missing_model_message(Agent::Claude, brain::requested_model(Agent::Claude, args));
    let mut mcp = mcp_client::from_claude(args).and_then(|s| McpClient::start(&s).ok());
    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in input.lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            // Like the real CLI: one line that is not JSON ends the session (sent as Null).
            let v = serde_json::from_str::<Value>(&line).unwrap_or(Value::Null);
            let stop = v.is_null();
            if tx.send(v).is_err() || stop {
                break;
            }
        }
    });
    let mut pending: VecDeque<Value> = VecDeque::new();
    let mut turn = 0u32;
    loop {
        let msg = match pending.pop_front() {
            Some(m) => m,
            None => match rx.recv() {
                Ok(m) => m,
                Err(_) => return 0,
            },
        };
        if msg.is_null() {
            eprintln!("Error parsing streaming input line");
            return 1;
        }
        match msg["type"].as_str() {
            Some("control_request") => emit(out, json!({"type": "control_response", "response": {"subtype": "success", "request_id": msg["request_id"], "response": {}}})),
            Some("user") => {
                let text: String = msg.pointer("/message/content").map(|c| match c {
                    Value::String(s) => s.clone(),
                    Value::Array(a) => a.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
                    _ => String::new(),
                }).unwrap_or_default();
                log(home, &format!("user: {text}"));
                let control = Control::read(home);
                if turn == 0 {
                    let status = if mcp.is_some() { "connected" } else { "failed" };
                    let tools: Vec<String> = mcp.as_ref().map(|c| c.tools.iter().map(|t| format!("mcp__gilvt__{t}")).collect()).unwrap_or_default();
                    emit(out, json!({"type": "system", "subtype": "init", "model": "fake-claude", "tools": tools, "mcp_servers": [{"name": "gilvt", "status": status}]}));
                }
                turn += 1;
                if let Some(message) = &missing {
                    emit(out, json!({"type": "assistant", "message": {"id": format!("msg_{turn}"), "model": "<synthetic>", "role": "assistant", "content": [{"type": "text", "text": message}]}, "parent_tool_use_id": null}));
                    emit(out, json!({"type": "result", "subtype": "success", "is_error": true, "num_turns": 1, "result": message}));
                    continue;
                }
                if control.auth {
                    emit(out, json!({"type": "result", "subtype": "success", "is_error": true, "result": "Not logged in · Please run /login"}));
                    continue;
                }
                let mut results = Vec::new();
                for (i, (tool, args)) in plan(&text).into_iter().enumerate() {
                    let id = format!("toolu_{turn}_{i}");
                    emit(out, json!({"type": "assistant", "message": {"id": format!("msg_{turn}_t{i}"), "role": "assistant", "content": [{"type": "tool_use", "id": id, "name": format!("mcp__gilvt__{tool}"), "input": args}]}, "parent_tool_use_id": null}));
                    let (ok, result) = match &mut mcp {
                        Some(c) => c.call(&tool, args),
                        None => (false, "gilvt MCP server is not running".to_string()),
                    };
                    log(home, &format!("tool {tool}: {}", first_line(&result)));
                    emit(out, json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": [{"type": "text", "text": result}], "is_error": !ok}]}, "parent_tool_use_id": null}));
                    results.push((tool, ok, result));
                }
                let text_out = answer(&text, &results);
                let msg_id = format!("msg_{turn}");
                if partial {
                    emit(out, json!({"type": "stream_event", "event": {"type": "message_start", "message": {"id": msg_id, "role": "assistant", "content": []}}, "parent_tool_use_id": null}));
                }
                let mut interrupted = false;
                for (i, piece) in chunks(&text_out).iter().enumerate() {
                    if partial {
                        emit(out, json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": piece}}, "parent_tool_use_id": null}));
                    }
                    if i == 0 && control.exit {
                        eprintln!("fake brain: chat-exit");
                        return 1;
                    }
                    let wait = if i == 0 && control.hang { Duration::from_secs(3600) } else { control.pause() };
                    if wait_for_interrupt(&rx, wait, &mut pending, out) {
                        interrupted = true;
                        break;
                    }
                }
                if interrupted {
                    emit(out, json!({"type": "result", "subtype": "error_during_execution", "is_error": true, "result": ""}));
                    continue;
                }
                emit(out, json!({"type": "assistant", "message": {"id": msg_id, "role": "assistant", "content": [{"type": "text", "text": text_out}]}, "parent_tool_use_id": null}));
                emit(out, json!({"type": "result", "subtype": "success", "is_error": false, "num_turns": 1, "result": text_out}));
            }
            _ => {}
        }
    }
}

/// `codex app-server`'s conversation side (`hooks::app_server` hands it `thread/start` / `turn/start`).
pub struct CodexChat {
    config: Vec<String>,
    home: Option<PathBuf>,
    mcp: Option<McpClient>,
    turns: u32,
    /// `thread/start`'s `model`.
    model: Option<String>,
}

impl CodexChat {
    pub fn new(config: &[String], home: Option<&Path>) -> CodexChat {
        CodexChat { config: config.to_vec(), home: home.map(Path::to_path_buf), mcp: None, turns: 0, model: None }
    }

    fn ensure_mcp(&mut self) -> bool {
        if self.mcp.is_none() {
            let own = |k: &str| std::env::var(k).ok();
            self.mcp = mcp_client::from_codex(&self.config, &own).and_then(|s| McpClient::start(&s).ok());
        }
        self.mcp.is_some()
    }

    /// Remembers the `model` of a `thread/start` (a `missing-*` one fails every turn).
    pub fn set_model(&mut self, params: &Value) {
        self.model = params["model"].as_str().map(String::from);
    }

    pub fn thread_start(&mut self, id: &Value) -> Vec<Value> {
        let status = if self.ensure_mcp() { "ready" } else { "failed" };
        vec![
            json!({"id": id, "result": {"thread": {"id": "fake-thread-1", "ephemeral": true, "status": {"type": "idle"}, "turns": []}, "model": "fake-codex-large", "approvalPolicy": "never", "sandbox": {"type": "readOnly"}}}),
            json!({"method": "mcpServer/startupStatus/updated", "params": {"threadId": "fake-thread-1", "name": "gilvt", "status": status, "error": null}}),
        ]
    }

    /// The answer to `turn/start` and the notifications of the whole turn, each with the pause before it.
    pub fn turn_start(&mut self, id: &Value, params: &Value) -> Vec<(Value, Duration)> {
        self.ensure_mcp();
        self.turns += 1;
        let turn_id = format!("turn-{}", self.turns);
        let home = self.home.clone();
        let text: String = params["input"].as_array().map(|a| a.iter().filter_map(|i| i["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
        log(home.as_deref(), &format!("user: {text}"));
        let control = Control::read(home.as_deref());
        let no = Duration::ZERO;
        let p = |method: &str, extra: Value| {
            let mut params = json!({"threadId": "fake-thread-1", "turnId": turn_id});
            if let (Some(o), Some(e)) = (params.as_object_mut(), extra.as_object()) {
                o.extend(e.clone());
            }
            json!({"method": method, "params": params})
        };
        let mut out = vec![
            (json!({"id": id, "result": {"turn": {"id": turn_id, "items": [], "status": "inProgress", "error": null}}}), no),
            (p("turn/started", json!({"turn": {"id": turn_id, "items": [], "status": "inProgress", "error": null}})), no),
        ];
        if let Some(message) = missing_model_message(Agent::Codex, self.model.as_deref()) {
            out.push((p("turn/completed", json!({"turn": {"id": turn_id, "items": [], "status": "failed", "error": {"message": message}}})), no));
            return out;
        }
        if control.auth {
            out.push((p("turn/completed", json!({"turn": {"id": turn_id, "items": [], "status": "failed", "error": {"message": "401 Unauthorized: not logged in"}}})), no));
            return out;
        }
        let mut results = Vec::new();
        for (i, (tool, args)) in plan(&text).into_iter().enumerate() {
            let call_id = format!("call-{}-{i}", self.turns);
            let item = |status: &str, result: Value| json!({"type": "mcpToolCall", "id": call_id, "server": "gilvt", "tool": tool, "arguments": args, "status": status, "result": result, "error": null});
            out.push((p("item/started", json!({"item": item("inProgress", Value::Null), "startedAtMs": 0})), no));
            let (ok, result) = match &mut self.mcp {
                Some(c) => c.call(&tool, args.clone()),
                None => (false, "gilvt MCP server is not running".to_string()),
            };
            log(home.as_deref(), &format!("tool {tool}: {}", first_line(&result)));
            let done = if ok { item("completed", json!({"content": [{"type": "text", "text": result}]})) } else {
                let mut i = item("failed", Value::Null);
                i["error"] = json!({"message": result});
                i
            };
            out.push((p("item/completed", json!({"item": done, "completedAtMs": 0})), no));
            results.push((tool, ok, result));
        }
        let text_out = answer(&text, &results);
        let msg_id = format!("msg-{}", self.turns);
        for piece in chunks(&text_out) {
            out.push((p("item/agentMessage/delta", json!({"itemId": msg_id, "delta": piece})), control.pause()));
        }
        out.push((p("item/completed", json!({"item": {"type": "agentMessage", "id": msg_id, "text": text_out}, "completedAtMs": 0})), no));
        out.push((p("turn/completed", json!({"turn": {"id": turn_id, "items": [], "status": "completed", "error": null}})), no));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_the_stream_json_chat() {
        let a = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(is_claude_chat(&a(&["-p", "--input-format", "stream-json", "--output-format", "stream-json"])));
        assert!(!is_claude_chat(&a(&["-p", "--output-format", "json"])), "the one-shot summary");
        assert!(!is_claude_chat(&a(&["@scenario:default"])));
    }

    #[test]
    fn plans_and_answers() {
        assert_eq!(plan("生成站会简报"), vec![("list_sessions".to_string(), json!({}))]);
        let scoped = "<scope keys=\"agent:claude:a1 pane:3\"/>\n为什么失败？";
        assert_eq!(scope_keys(scoped), ["agent:claude:a1", "pane:3"]);
        assert_eq!(plan(scoped)[1], ("get_timeline".to_string(), json!({"key": "agent:claude:a1", "turns": "last:2"})));
        let list = r#"{"gilvt_label":"已列出 3 个会话","sessions":[
            {"key":"agent:claude:a1","name":"api-refactor","group":"needs_you","status":"⏳ 等待审批"},
            {"key":"pane:3","name":"zsh","group":"terminals","status":"空闲"},
            {"key":"agent:codex:c2","name":"web-login","group":"running","status":"● Bash"}]}"#;
        let b = answer("生成站会简报", &[("list_sessions".into(), true, list.into())]);
        assert!(b.starts_with("### 要你处理\n- [api-refactor](gilvt://session/agent:claude:a1)：⏳ 等待审批\n"), "{b}");
        assert!(b.contains("\n\n### 整体\n共 3 个会话，1 个在跑"), "{b}");
        let s = answer(scoped, &[("get_session".into(), true, r#"{"session":{"name":"web-login"}}"#.into())]);
        assert!(s.contains("（第 1 段）") && s.contains("[web-login](gilvt://session/agent:claude:a1)") && s.ends_with("（完）"), "{s}");
        assert_eq!(answer("请调用一次 list_sessions 工具", &[]), "ok");
    }

    #[test]
    fn chunks_add_up_to_the_answer() {
        let a = "第一段\n\n第二段\n\n第三段";
        let c = chunks(a);
        assert_eq!(c.len(), 3);
        assert_eq!(c.concat(), a);
        assert_eq!(chunks("ok").concat(), "ok");
        assert!(chunks("一句话没有空行").len() >= 2);
    }

    #[test]
    fn control_words_anywhere_in_the_file() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(crate::brain::CONTROL_FILE), "auth\nchat-slow\nfeatures-no-shell\n").unwrap();
        let c = Control::read(Some(home.path()));
        assert!(c.slow && !c.exit && !c.auth, "the one-shot `auth` is not the chat's");
        assert!(!features_list(Some(home.path())).contains("shell_tool"));
        assert!(features_list(None).contains("shell_tool"));
    }

    #[test]
    fn mcp_server_specs() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("mcp.json");
        std::fs::write(&cfg, r#"{"mcpServers":{"gilvt":{"type":"stdio","command":"/g/gilvt","args":["mcp"],"env":{"GILVT_SOCKET":"/s","GILVT_MONITOR_TOKEN":"t"}}}}"#).unwrap();
        let a = vec!["-p".to_string(), "--mcp-config".into(), cfg.display().to_string()];
        let s = crate::mcp_client::from_claude(&a).unwrap();
        assert_eq!((s.command.as_str(), s.args.clone()), ("/g/gilvt", vec!["mcp".to_string()]));
        assert!(s.env.contains(&("GILVT_MONITOR_TOKEN".to_string(), "t".to_string())));
        let c = vec![
            "mcp_servers.gilvt.command=\"/g/gilvt\"".to_string(),
            "mcp_servers.gilvt.args=[\"mcp\"]".into(),
            "mcp_servers.gilvt.env_vars=[\"GILVT_SOCKET\",\"GILVT_MONITOR_TOKEN\"]".into(),
        ];
        let own = |k: &str| (k == "GILVT_MONITOR_TOKEN").then(|| "from-env".to_string());
        let s = crate::mcp_client::from_codex(&c, &own).unwrap();
        assert_eq!(s.command, "/g/gilvt");
        assert_eq!(s.env, vec![("GILVT_MONITOR_TOKEN".to_string(), "from-env".to_string())]);
    }

    fn run_claude(args: &[&str], home: &Path, input: &str) -> (i32, Vec<Value>) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::new();
        let code = claude_chat(&args, Some(home), std::io::BufReader::new(std::io::Cursor::new(input.to_string().into_bytes())), &mut out);
        (code, String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect())
    }

    fn user(text: &str) -> String {
        format!("{}\n", json!({"type": "user", "message": {"role": "user", "content": text}}))
    }

    #[test]
    fn claude_chat_missing_model_and_auth_end_every_turn_with_an_error() {
        let home = tempfile::tempdir().unwrap();
        let input = user("a") + &user("b");
        let (code, out) = run_claude(&["-p", "--input-format", "stream-json", "--model", "missing-x"], home.path(), &input);
        assert_eq!(code, 0);
        let results: Vec<&Value> = out.iter().filter(|v| v["type"] == "result").collect();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r["is_error"] == true && r["result"].as_str().unwrap().contains("issue with the selected model")), "{results:?}");
        assert_eq!(out.iter().filter(|v| v["subtype"] == "init").count(), 1, "init only on the first turn");
        std::fs::write(home.path().join(crate::brain::CONTROL_FILE), "chat-auth\n").unwrap();
        let (_, out) = run_claude(&["-p", "--input-format", "stream-json"], home.path(), &user("a"));
        assert_eq!(out.last().unwrap()["result"], "Not logged in · Please run /login");
        assert!(out[0]["mcp_servers"][0]["status"] == "failed", "no --mcp-config: the tools are not connected");
    }

    #[test]
    fn claude_chat_interrupt_ends_the_turn_with_an_error_result() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(crate::brain::CONTROL_FILE), "chat-hang\n").unwrap();
        let interrupt = format!("{}\n", json!({"type": "control_request", "request_id": "r1", "request": {"subtype": "interrupt"}}));
        let (code, out) = run_claude(&["-p", "--input-format", "stream-json"], home.path(), &(user("请调用一次 list_sessions 工具") + &interrupt));
        assert_eq!(code, 0);
        assert!(out.iter().any(|v| v["type"] == "control_response"));
        let last = out.last().unwrap();
        assert_eq!((last["subtype"].as_str(), last["is_error"].clone()), (Some("error_during_execution"), json!(true)));
    }

    #[test]
    fn claude_chat_exit_control_word_exits_1_after_the_first_piece() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(crate::brain::CONTROL_FILE), "chat-exit\n").unwrap();
        let (code, out) = run_claude(&["-p", "--input-format", "stream-json", "--include-partial-messages"], home.path(), &user("生成站会简报"));
        assert_eq!(code, 1);
        assert!(out.iter().any(|v| v["event"]["type"] == "content_block_delta"));
        assert!(out.iter().all(|v| v["type"] != "result"));
    }

    #[test]
    fn codex_chat_missing_model_and_auth_fail_the_turn_and_ignore_exit_and_hang() {
        let home = tempfile::tempdir().unwrap();
        let mut chat = CodexChat::new(&[], Some(home.path()));
        chat.set_model(&json!({"model": "missing-x"}));
        let turn = chat.turn_start(&json!(5), &json!({"input": [{"type": "text", "text": "a"}]}));
        let done = &turn.last().unwrap().0;
        assert_eq!(done["method"], "turn/completed");
        assert_eq!(done["params"]["turn"]["status"], "failed");
        assert!(done["params"]["turn"]["error"]["message"].as_str().unwrap().contains("missing-x"));
        std::fs::write(home.path().join(crate::brain::CONTROL_FILE), "chat-exit\nchat-hang\n").unwrap();
        let mut chat = CodexChat::new(&[], Some(home.path()));
        let turn = chat.turn_start(&json!(5), &json!({"input": [{"type": "text", "text": "a"}]}));
        assert_eq!(turn.last().unwrap().0["params"]["turn"]["status"], "completed", "chat-exit / chat-hang are Claude-only");
        std::fs::write(home.path().join(crate::brain::CONTROL_FILE), "chat-auth\n").unwrap();
        let turn = chat.turn_start(&json!(6), &json!({"input": [{"type": "text", "text": "a"}]}));
        assert_eq!(turn.last().unwrap().0["params"]["turn"]["status"], "failed");
    }
}
