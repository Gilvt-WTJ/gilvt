//! The built `gilvt mcp` against a test socket that plays the app.

use std::io::Write;
use std::process::{Command, Stdio};

use gilvt_ipc::{DebugQueries, Request, Response, Server, DEBUG_STATE_DISABLED, MONITOR_BUSY, QUERY_QUEUE};
use serde_json::{json, Value};

fn gilvt() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_gilvt"));
    c.env_remove("GILVT_SOCKET").env_remove("GILVT_PANE_ID").env_remove("GILVT_MONITOR_TOKEN");
    c
}

/// A socket whose "app" accepts only `good` and answers list_sessions with no sessions.
fn app(dir: &std::path::Path) -> (Server, std::path::PathBuf) {
    let path = dir.join("app.sock");
    let (tx, _rx) = async_channel::unbounded();
    let (mtx, mrx) = async_channel::bounded(QUERY_QUEUE);
    let server = Server::start_with_monitor(&path, tx, DebugQueries::Refused(DEBUG_STATE_DISABLED), mtx).unwrap();
    std::thread::spawn(move || {
        while let Ok(q) = mrx.recv_blocking() {
            let Request::Monitor { token, tool, .. } = &q.request else { continue };
            let resp = if token == "good" {
                Response::Tool { text: format!("{{\"gilvt_label\":\"已列出 0 个会话\",\"tool\":\"{tool}\",\"sessions\":[]}}"), is_error: false }
            } else {
                Response::Tool { text: "token 无效".into(), is_error: true }
            };
            q.respond(resp);
        }
    });
    (server, path)
}

fn session(sock: &std::path::Path, token: &str, lines: &[Value]) -> (Vec<Value>, std::process::ExitStatus) {
    let mut child = gilvt()
        .arg("mcp")
        .env("GILVT_SOCKET", sock)
        .env("GILVT_MONITOR_TOKEN", token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for l in lines {
        writeln!(stdin, "{l}").unwrap();
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let replies = String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    (replies, out.status)
}

fn handshake_and_call() -> Vec<Value> {
    vec![
        json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "codex-mcp-client"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "list_sessions", "arguments": {}}}),
    ]
}

#[test]
fn a_tool_call_reaches_the_app_with_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let (_server, sock) = app(dir.path());
    let (replies, status) = session(&sock, "good", &handshake_and_call());
    assert!(status.success());
    assert_eq!(replies.iter().map(|r| r["id"].clone()).collect::<Vec<_>>(), [json!(0), json!(1), json!(2)]);
    assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 5);
    assert_eq!(replies[2]["result"]["isError"], false);
    let text = replies[2]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("\"tool\":\"list_sessions\""), "{text}");
}

#[test]
fn wrong_token_is_a_tool_error_not_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let (_server, sock) = app(dir.path());
    let (replies, status) = session(&sock, "stolen", &handshake_and_call());
    assert!(status.success(), "the server keeps running");
    assert_eq!(replies[2]["result"]["isError"], true);
    assert_eq!(replies[2]["result"]["content"][0]["text"], "token 无效");
}

#[test]
fn an_app_that_went_away_is_busy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gone.sock");
    let (tx, _rx) = async_channel::unbounded();
    let (mtx, mrx) = async_channel::bounded::<gilvt_ipc::Query>(QUERY_QUEUE);
    drop(mrx); // the app side is gone: every query is answered "shutting down"
    let _server = Server::start_with_monitor(&path, tx, DebugQueries::Refused(DEBUG_STATE_DISABLED), mtx).unwrap();
    let (replies, _) = session(&path, "good", &handshake_and_call());
    assert_eq!(replies[2]["result"]["isError"], true);
    assert!(replies[2]["result"]["content"][0]["text"].as_str().unwrap().starts_with(MONITOR_BUSY));
}

#[test]
fn missing_environment_exits_2() {
    let out = gilvt().arg("mcp").stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("GILVT_MONITOR_TOKEN"));
    let out = gilvt().args(["mcp", "--extra"]).env("GILVT_SOCKET", "/x").env("GILVT_MONITOR_TOKEN", "t").stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn a_non_utf8_line_is_a_parse_error_and_the_server_keeps_going() {
    let dir = tempfile::tempdir().unwrap();
    let (_server, sock) = app(dir.path());
    let mut child = gilvt().arg("mcp").env("GILVT_SOCKET", &sock).env("GILVT_MONITOR_TOKEN", "good")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"\xff\xfe not utf-8\n").unwrap();
    writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})).unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let replies: Vec<Value> = String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["error"]["code"], -32700);
    assert_eq!(replies[1]["id"], 1);
    assert_eq!(replies[1]["result"], json!({}));
}
