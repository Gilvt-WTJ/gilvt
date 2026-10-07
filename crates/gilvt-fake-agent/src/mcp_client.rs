//! The fake brain's MCP client (S2 §9): starts the server a real CLI would — Claude's `--mcp-config`, Codex's
//! `-c mcp_servers.gilvt.*` — and calls its tools over stdio, so the GUI cases exercise the real `gilvt mcp`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerSpec {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default()
}

/// The `gilvt` server of `--mcp-config <file or inline JSON>`.
pub fn from_claude(args: &[String]) -> Option<ServerSpec> {
    let raw = args.iter().position(|a| a == "--mcp-config").and_then(|i| args.get(i + 1))?;
    let text = if raw.trim_start().starts_with('{') { raw.clone() } else { std::fs::read_to_string(raw).ok()? };
    let v: Value = serde_json::from_str(&text).ok()?;
    let s = v.pointer("/mcpServers/gilvt")?;
    let env = s.get("env").and_then(Value::as_object).map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default();
    Some(ServerSpec { command: s.get("command")?.as_str()?.to_string(), args: strings(s.get("args")), env })
}

/// `-c mcp_servers.gilvt.{command,args,env,env_vars}`: Codex gives a server only `env` and the `env_vars` it
/// finds in its own environment (`own_env`).
pub fn from_codex(config: &[String], own_env: &dyn Fn(&str) -> Option<String>) -> Option<ServerSpec> {
    let cfg = crate::hooks::codex_config(config);
    let s = cfg.get("mcp_servers")?.get("gilvt")?;
    let mut env: Vec<(String, String)> = s
        .get("env")
        .and_then(|e| e.as_table())
        .map(|t| t.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect())
        .unwrap_or_default();
    for name in s.get("env_vars").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|v| v.as_str()) {
        if let Some(value) = own_env(name) {
            env.push((name.to_string(), value));
        }
    }
    let args = s.get("args").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|v| v.as_str()).map(String::from).collect()).unwrap_or_default();
    Some(ServerSpec { command: s.get("command")?.as_str()?.to_string(), args, env })
}

pub struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
    /// `tools/list`'s names.
    pub tools: Vec<String>,
}

impl McpClient {
    /// Starts the server (the fake's own `GILVT_SOCKET` / `GILVT_MONITOR_TOKEN` removed: only `spec.env` counts)
    /// and does the MCP handshake.
    pub fn start(spec: &ServerSpec) -> std::io::Result<McpClient> {
        let mut cmd = Command::new(&spec.command);
        cmd.args(&spec.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        cmd.env_remove("GILVT_SOCKET").env_remove("GILVT_MONITOR_TOKEN");
        cmd.envs(spec.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut c = McpClient { child, stdin, stdout, next: 0, tools: Vec::new() };
        c.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "gilvt-fake-brain", "version": "0"}}))?;
        writeln!(c.stdin, "{}", json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        let list = c.request("tools/list", json!({}))?;
        c.tools = list.pointer("/result/tools").and_then(Value::as_array).map(|a| a.iter().filter_map(|t| t.get("name")?.as_str().map(String::from)).collect()).unwrap_or_default();
        Ok(c)
    }

    fn request(&mut self, method: &str, params: Value) -> std::io::Result<Value> {
        let id = self.next;
        self.next += 1;
        writeln!(self.stdin, "{}", json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        self.stdin.flush()?;
        let mut line = String::new();
        loop {
            line.clear();
            if self.stdout.read_line(&mut line)? == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v.get("id") == Some(&json!(id)) {
                return Ok(v);
            }
        }
    }

    /// (ok, text) of one call; a broken server reads as a failed call.
    pub fn call(&mut self, tool: &str, args: Value) -> (bool, String) {
        match self.request("tools/call", json!({"name": tool, "arguments": args})) {
            Ok(v) => {
                let text = v.pointer("/result/content/0/text").and_then(Value::as_str).unwrap_or("").to_string();
                let is_error = v.pointer("/result/isError").and_then(Value::as_bool).unwrap_or(v.get("error").is_some());
                (!is_error, text)
            }
            Err(e) => (false, format!("MCP server: {e}")),
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
