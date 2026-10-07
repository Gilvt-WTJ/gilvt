//! The fake brain's conversations must decode with gilvt's own adapters (S2 §9): if the adapters change, these
//! fail first. The MCP server here is a stub script; the real `gilvt mcp` is exercised by the Z cases.

use std::io::{BufReader, Cursor};
use std::os::unix::fs::PermissionsExt;

use gilvt_fake_agent::{brain_chat, hooks};
use gilvt_monitor::chat::claude::Claude;
use gilvt_monitor::chat::codex::{check_features, Codex};
use gilvt_monitor::chat::{ChatEvent, Protocol, TurnEnd};
use serde_json::json;

const STUB: &str = r#"#!/bin/sh
while read l; do
  id=$(printf '%s' "$l" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$l" in
    *'"initialize"'*) echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{\"tools\":{}},\"serverInfo\":{\"name\":\"stub\"}}}";;
    *'"tools/list"'*) echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"tools\":[{\"name\":\"list_sessions\"}]}}";;
    *'"tools/call"'*) echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"{\\\"gilvt_label\\\":\\\"已列出 1 个会话\\\",\\\"sessions\\\":[{\\\"key\\\":\\\"pane:3\\\",\\\"name\\\":\\\"zsh\\\",\\\"group\\\":\\\"error\\\",\\\"status\\\":\\\"✗ make\\\"}]}\"}],\"isError\":false}}";;
  esac
done
"#;

fn stub(dir: &std::path::Path) -> String {
    let p = dir.join("stub-mcp");
    std::fs::write(&p, STUB).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p.display().to_string()
}

fn decode(p: &mut dyn Protocol, output: &str) -> Vec<ChatEvent> {
    output.lines().filter(|l| !l.trim().is_empty()).flat_map(|l| p.line(l).events).collect()
}

#[test]
fn gilvt_decodes_the_fake_claude_chat() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("mcp.json");
    std::fs::write(&cfg, json!({"mcpServers": {"gilvt": {"type": "stdio", "command": stub(dir.path()), "args": ["mcp"], "env": {}}}}).to_string()).unwrap();
    // Exactly the arguments gilvt starts `claude` with.
    let args = gilvt_monitor::chat::claude::args(None, "I", &cfg);
    assert!(brain_chat::is_claude_chat(&args), "the fake takes gilvt's arguments as a chat");
    let mut claude = Claude::default();
    let input = claude.user("生成站会简报").join("\n") + "\n";
    let mut out = Vec::new();
    let code = brain_chat::claude_chat(&args, Some(dir.path()), BufReader::new(Cursor::new(input.into_bytes())), &mut out);
    assert_eq!(code, 0);
    let events = decode(&mut claude, &String::from_utf8(out).unwrap());
    assert!(matches!(&events[0], ChatEvent::Ready { note: None, .. }), "{events:?}");
    assert!(events.iter().any(|e| matches!(e, ChatEvent::ToolStarted { tool, .. } if tool == "list_sessions")));
    assert!(events.iter().any(|e| matches!(e, ChatEvent::ToolDone { ok: true, text, .. } if text.contains("已列出 1 个会话"))));
    assert!(events.iter().filter(|e| matches!(e, ChatEvent::Text { .. })).count() >= 2, "streamed");
    assert!(events.iter().any(|e| matches!(e, ChatEvent::TextDone { text, .. } if text.contains("### 要你处理") && text.contains("gilvt://session/pane:3"))));
    assert_eq!(events.last(), Some(&ChatEvent::TurnEnded(TurnEnd::Done)));
    let log = std::fs::read_to_string(dir.path().join(brain_chat::CHAT_LOG)).unwrap();
    assert!(log.contains("user: 生成站会简报") && log.contains("tool list_sessions:"), "{log}");
}

#[test]
fn a_non_json_stdin_line_exits_1_like_claude() {
    let mut out = Vec::new();
    let code = brain_chat::claude_chat(&["-p".into(), "--input-format".into(), "stream-json".into()], None, BufReader::new(Cursor::new(b"not json\n".to_vec())), &mut out);
    assert_eq!(code, 1);
}

#[test]
fn gilvt_decodes_the_fake_codex_chat() {
    let dir = tempfile::tempdir().unwrap();
    // Exactly the arguments gilvt starts `codex app-server` with (the stub stands in for `gilvt mcp`).
    let mcp = gilvt_monitor::chat::McpLaunch { gilvt: stub(dir.path()).into(), socket: dir.path().join("sock"), token: "t".into() };
    let argv = gilvt_monitor::chat::codex::args(&[], &mcp, &[]);
    let parsed = gilvt_fake_agent::args::parse(gilvt_fake_agent::Agent::Codex, &argv);
    assert_eq!(parsed.mode, gilvt_fake_agent::args::Mode::AppServer);
    let config = parsed.config;
    assert!(config.contains(&"notify=[]".to_string()), "{config:?}");
    assert!(hooks::Hooks::from_codex_config(&config).is_empty(), "no notify program, no hook: nothing gets the answers");
    let mut codex = Codex::new(None, "I".into(), "/tmp".into());
    let mut written: Vec<String> = codex.start();
    // Drive the fake app-server with exactly what gilvt writes, line by line.
    let mut events = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for round in 0..3 {
        let input: String = written.drain(..).map(|l| l + "\n").collect();
        let mut out = Vec::new();
        hooks::app_server(&config, Some(dir.path()), BufReader::new(Cursor::new(input.into_bytes())), &mut out).unwrap();
        for line in String::from_utf8(out).unwrap().lines() {
            let d = codex.line(line);
            written.extend(d.replies);
            events.extend(d.events);
        }
        if round == 0 {
            pending = codex.user("哪些需要我？");
        }
        written.extend(pending.drain(..));
    }
    assert!(events.iter().any(|e| matches!(e, ChatEvent::Ready { model: Some(m), .. } if m == "fake-codex-large")), "{events:?}");
    assert!(events.iter().any(|e| matches!(e, ChatEvent::ToolDone { ok: true, .. })));
    assert!(events.iter().any(|e| matches!(e, ChatEvent::TextDone { text, .. } if text.contains("会话"))));
    assert_eq!(events.last(), Some(&ChatEvent::TurnEnded(TurnEnd::Done)));
}

#[test]
fn the_fake_features_list_passes_or_fails_gilvt_s_check() {
    assert!(check_features(&brain_chat::features_list(None)).is_ok());
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(gilvt_fake_agent::brain::CONTROL_FILE), "features-no-shell\n").unwrap();
    assert!(check_features(&brain_chat::features_list(Some(home.path()))).is_err());
}
