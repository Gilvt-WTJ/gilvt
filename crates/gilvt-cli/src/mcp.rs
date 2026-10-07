//! `gilvt mcp`: the 监控官's read-only tools as a stdio MCP server (S2 §6.1). JSON-RPC 2.0, one message per line;
//! `initialize`, `notifications/initialized`, `tools/list`, `tools/call` and `ping`. Every tool call becomes a
//! `Request::Monitor` to the app at `GILVT_SOCKET`, with the token the app put in `GILVT_MONITOR_TOKEN` when it
//! started the chat process; the app answers on its main thread. Nothing here writes to a pane.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gilvt_ipc::{Request, Response, ENV_MONITOR_TOKEN, ENV_SOCKET, MONITOR_BUSY};
use gilvt_monitor::tools;
use serde_json::{json, Value};

/// Answered when the client asks for a version not in [`KNOWN_VERSIONS`].
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Versions in which this server's subset of MCP is the same (Claude Code 2.1.291 asks for 2025-11-25,
/// codex 0.160 for 2025-06-18).
pub const KNOWN_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOutcome {
    pub text: String,
    pub is_error: bool,
}

/// The reply to one line from the client; None for notifications and for the client's own answers.
/// `call(tool, args)` runs a checked tool call (tool without Claude's prefix).
pub fn handle(line: &str, call: &mut dyn FnMut(&str, &Value) -> ToolOutcome) -> Option<String> {
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        return Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}}).to_string());
    };
    let method = msg.get("method").and_then(Value::as_str)?;
    let id = msg.get("id").cloned()?;
    let result = match method {
        "initialize" => {
            let asked = msg.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or("");
            let version = if KNOWN_VERSIONS.contains(&asked) { asked } else { PROTOCOL_VERSION };
            json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "gilvt", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "gilvt 的只读工具：读取 gilvt 里 Agent 会话与终端的状态。不能写入任何 pane，也不能执行命令。"
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools::specs()}),
        "tools/call" => {
            let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or("");
            let args = match msg.pointer("/params/arguments") {
                None | Some(Value::Null) => json!({}),
                Some(a) => a.clone(),
            };
            let out = match tools::parse_call(name, &args) {
                Ok(_) => call(name.strip_prefix(tools::CLAUDE_PREFIX).unwrap_or(name), &args),
                Err(e) => ToolOutcome { text: format!("参数错误：{e}"), is_error: true },
            };
            json!({"content": [{"type": "text", "text": out.text}], "isError": out.is_error})
        }
        _ => {
            return Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("Method not found: {method}")}}).to_string());
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string())
}

/// The app's reply to a tool call as what the model sees.
pub fn outcome_of(reply: std::io::Result<Response>) -> ToolOutcome {
    match reply {
        Ok(Response::Tool { text, is_error }) => ToolOutcome { text, is_error },
        Ok(Response::Error { message }) => ToolOutcome { text: format!("{MONITOR_BUSY}（{message}）"), is_error: true },
        Ok(_) => ToolOutcome { text: "gilvt 的回答不是工具结果".into(), is_error: true },
        Err(e) => ToolOutcome { text: format!("{MONITOR_BUSY}（连不上 gilvt：{e}）"), is_error: true },
    }
}

/// Serves `input` until EOF, asking the app at `socket` with `token`.
pub fn serve(mut input: impl BufRead, mut output: impl Write, socket: &Path, token: &str) -> ExitCode {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match input.read_until(b'\n', &mut buf) {
            Ok(0) => return ExitCode::SUCCESS,
            Ok(_) => {}
            Err(e) => {
                eprintln!("gilvt mcp: reading stdin: {e}");
                return ExitCode::from(1);
            }
        }
        // Lossy: a stray non-UTF-8 byte becomes a parse error for that line, not the end of the server.
        let line = String::from_utf8_lossy(&buf);
        if line.trim().is_empty() {
            continue;
        }
        let mut call = |tool: &str, args: &Value| {
            let req = Request::Monitor { token: token.to_string(), tool: tool.to_string(), args: args.clone() };
            outcome_of(gilvt_ipc::send(socket, &req))
        };
        if let Some(reply) = handle(&line, &mut call) {
            if writeln!(output, "{reply}").and_then(|()| output.flush()).is_err() {
                return ExitCode::SUCCESS;
            }
        }
    }
}

pub fn run(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        eprintln!("gilvt mcp: takes no arguments");
        return ExitCode::from(2);
    }
    let socket = std::env::var_os(ENV_SOCKET).filter(|s| !s.is_empty()).map(PathBuf::from);
    let token = std::env::var(ENV_MONITOR_TOKEN).ok().filter(|t| !t.is_empty());
    let (Some(socket), Some(token)) = (socket, token) else {
        eprintln!("gilvt mcp: {ENV_SOCKET} and {ENV_MONITOR_TOKEN} must be set (gilvt sets them for its 监控官)");
        return ExitCode::from(2);
    };
    serve(std::io::stdin().lock(), std::io::stdout().lock(), &socket, &token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reply(line: &str) -> Value {
        let mut never = |_: &str, _: &Value| -> ToolOutcome { panic!("no tool call expected") };
        serde_json::from_str(&handle(line, &mut never).expect("a reply")).unwrap()
    }

    #[test]
    fn initialize_echoes_a_known_version() {
        let r = reply(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"claude-code"}}}"#);
        assert_eq!(r["jsonrpc"], "2.0");
        assert_eq!(r["id"], 0);
        assert_eq!(r["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(r["result"]["serverInfo"]["name"], "gilvt");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        let r = reply(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2099-01-01"}}"#);
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    #[test]
    fn notifications_and_client_answers_get_no_reply() {
        let mut never = |_: &str, _: &Value| -> ToolOutcome { panic!() };
        assert_eq!(handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, &mut never), None);
        assert_eq!(handle(r#"{"jsonrpc":"2.0","id":5,"result":{}}"#, &mut never), None);
    }

    #[test]
    fn tools_list_ping_and_unknown_methods() {
        let r = reply(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":{"progressToken":0}}}"#);
        assert_eq!(r["result"]["tools"].as_array().unwrap().len(), 5);
        assert_eq!(reply(r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#)["result"], json!({}));
        let r = reply(r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#);
        assert_eq!(r["error"]["code"], -32601);
        let r = reply("this is not json");
        assert_eq!((r["error"]["code"].clone(), r["id"].clone()), (json!(-32700), Value::Null));
    }

    #[test]
    fn tool_calls_go_through_with_their_arguments() {
        let mut seen = Vec::new();
        let mut call = |tool: &str, args: &Value| {
            seen.push((tool.to_string(), args.clone()));
            ToolOutcome { text: "{\"gilvt_label\":\"已列出 0 个会话\"}".into(), is_error: false }
        };
        let line = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"mcp__gilvt__get_session","arguments":{"key":"pane:3"}}}"#;
        let r: Value = serde_json::from_str(&handle(line, &mut call).unwrap()).unwrap();
        assert_eq!(r["result"]["isError"], false);
        assert_eq!(r["result"]["content"][0]["type"], "text");
        assert_eq!(seen, vec![("get_session".to_string(), json!({"key": "pane:3"}))], "prefix stripped");
    }

    #[test]
    fn missing_or_null_arguments_mean_an_empty_object() {
        for line in [
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"list_sessions"}}"#,
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"list_sessions","arguments":null}}"#,
        ] {
            let mut seen = Vec::new();
            let mut call = |tool: &str, args: &Value| {
                seen.push((tool.to_string(), args.clone()));
                ToolOutcome { text: "{}".into(), is_error: false }
            };
            let r: Value = serde_json::from_str(&handle(line, &mut call).unwrap()).unwrap();
            assert_eq!(r["result"]["isError"], false, "{r}");
            assert_eq!(seen, vec![("list_sessions".to_string(), json!({}))]);
        }
    }

    #[test]
    fn bad_arguments_never_reach_the_app() {
        let mut never = |_: &str, _: &Value| -> ToolOutcome { panic!("must not be asked") };
        let line = r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"get_timeline","arguments":{"key":"pane:3","turns":"last:9"}}}"#;
        let r: Value = serde_json::from_str(&handle(line, &mut never).unwrap()).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().starts_with("参数错误："));
    }

    #[test]
    fn app_replies_become_outcomes() {
        let ok = outcome_of(Ok(Response::Tool { text: "t".into(), is_error: false }));
        assert_eq!(ok, ToolOutcome { text: "t".into(), is_error: false });
        let busy = outcome_of(Ok(Response::Error { message: "gilvt did not answer within 4s".into() }));
        assert!(busy.is_error && busy.text.starts_with(MONITOR_BUSY), "{busy:?}");
        let gone = outcome_of(Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no socket")));
        assert!(gone.is_error && gone.text.contains("连不上 gilvt"), "{gone:?}");
    }
}
