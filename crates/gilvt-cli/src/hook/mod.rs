//! `gilvt hook …`: the agent side of M3a.
//!
//! - `gilvt hook claude|codex <Event>`: forwards a hook payload (stdin) to the app. Always silent,
//!   always exits 0, never waits for the app: the agent blocks on this process and its stdout
//!   would become model context.
//! - `gilvt hook claude-args|codex-args -- <args…>`: the argv the shell wrappers run the agent with,
//!   NUL-separated (each argument followed by `\0`).
//! - `gilvt hook codex-trust`: background job that learns codex's trust hashes for gilvt's hooks.

pub mod claude;
pub mod codex;
pub mod codex_flags;
pub mod trust;

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gilvt_agent::{default_state_dir, hook_meta, AgentKind, HookInput, LeaseStore};
use gilvt_ipc::{Request, ENV_PANE, ENV_SOCKET};

/// Largest hook payload forwarded whole; bigger ones are reduced to their identity fields.
pub const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
/// Oversized payloads are drained up to this much so the agent never sees a broken pipe.
const MAX_DRAIN_BYTES: u64 = 64 * 1024 * 1024;

/// Entry point for `gilvt hook <args>`.
pub fn run(args: &[String]) -> ExitCode {
    let rest = |from: usize| -> &[String] {
        let r = args.get(from..).unwrap_or_default();
        r.strip_prefix(&["--".to_string()][..]).unwrap_or(r)
    };
    match args.first().map(String::as_str) {
        Some("claude-args") => print_argv(&claude::args_for(&gilvt_path(), rest(1))),
        Some("codex-args") => print_argv(&codex::args_for(&gilvt_path(), rest(1))),
        Some("codex-trust") => {
            trust::run_job(&gilvt_path());
            ExitCode::SUCCESS
        }
        Some(agent @ ("claude" | "codex")) => {
            if let Some(event) = args.get(1).filter(|e| valid_event(e)) {
                forward(agent, event);
            }
            ExitCode::SUCCESS
        }
        // Unknown hook usage is still silent: this may be running inside an agent.
        _ => ExitCode::SUCCESS,
    }
}

/// The absolute path of this `gilvt` binary: the one place hooks learn what command to run.
pub fn gilvt_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gilvt"));
    exe.canonicalize().unwrap_or(exe)
}

fn valid_event(e: &str) -> bool {
    !e.is_empty() && e.len() <= 64 && e.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn print_argv(argv: &[String]) -> ExitCode {
    let mut out = Vec::new();
    for a in argv {
        out.extend_from_slice(a.as_bytes());
        out.push(0);
    }
    let mut stdout = std::io::stdout().lock();
    match stdout.write_all(&out).and_then(|()| stdout.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(1),
    }
}

/// Reads the payload and sends it; every failure is silent.
fn forward(agent: &str, event: &str) {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return;
    }
    let mut buf = Vec::new();
    let mut input = stdin.lock();
    if input.by_ref().take(MAX_PAYLOAD_BYTES as u64 + 1).read_to_end(&mut buf).is_err() {
        return;
    }
    let payload = if buf.len() > MAX_PAYLOAD_BYTES {
        let _ = std::io::copy(&mut input.take(MAX_DRAIN_BYTES), &mut std::io::sink());
        reduced_payload(&buf)
    } else {
        serde_json::from_slice::<serde_json::Value>(&buf).ok().filter(|v| v.is_object())
    };
    let Some(payload) = payload else { return };
    if let Some(kind) = AgentKind::from_name(agent) {
        let input = HookInput { agent: kind, event, payload: &payload };
        if let (Some(state), Some(meta)) = (default_state_dir(), hook_meta(input.payload)) {
            let _ = LeaseStore::open(&state).update_hook(
                kind,
                meta.session_id,
                meta.transcript_path,
                meta.cwd,
                event,
            );
        }
    }
    let pane = std::env::var(ENV_PANE).ok().and_then(|p| p.parse().ok());
    let req = Request::Hook { pane, agent: agent.into(), event: event.into(), payload };
    if let Some(socket) = std::env::var_os(ENV_SOCKET).filter(|s| !s.is_empty()) {
        let _ = gilvt_ipc::send_nowait(Path::new(&socket), &req);
    } else {
        for socket in gilvt_ipc::live_socket_paths() {
            let _ = gilvt_ipc::send_nowait(&socket, &req);
        }
    }
}

/// Identity fields kept from an oversized payload (e.g. `PostToolUse` of a huge file read).
const IDENTITY_FIELDS: [&str; 7] = ["session_id", "transcript_path", "cwd", "hook_event_name", "tool_name", "tool_use_id", "turn_id"];

/// Builds `{<identity fields>…, "gilvt_truncated": true}` from the first occurrence of each field
/// in `prefix`. Both agents write these top-level fields before any tool input or output.
pub fn reduced_payload(prefix: &[u8]) -> Option<serde_json::Value> {
    let text = String::from_utf8_lossy(prefix);
    let mut out = serde_json::Map::new();
    for field in IDENTITY_FIELDS {
        if let Some(v) = first_string_field(&text, field) {
            out.insert(field.into(), v.into());
        }
    }
    out.contains_key("session_id").then(|| {
        out.insert("gilvt_truncated".into(), true.into());
        out.into()
    })
}

/// The string value of the first `"field": "…"` in `text`.
fn first_string_field(text: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let at = text.find(&needle)? + needle.len();
    let rest = text[at..].trim_start().strip_prefix(':')?.trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return serde_json::from_str(&rest[..=i]).ok(),
            _ => i += 1,
        }
    }
    None
}

/// Quotes `s` for a POSIX shell (hook commands are run with `sh -c`).
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@+%,".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The shell command an agent runs for `event`.
pub fn hook_command(gilvt: &Path, agent: &str, event: &str) -> String {
    format!("{} hook {agent} {event}", shell_quote(&gilvt.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_hook_commands() {
        assert_eq!(hook_command(Path::new("/opt/gilvt/bin/gilvt"), "claude", "Stop"), "/opt/gilvt/bin/gilvt hook claude Stop");
        assert_eq!(
            hook_command(Path::new("/Applications/My gilvt's.app/gilvt"), "codex", "Stop"),
            r#"'/Applications/My gilvt'\''s.app/gilvt' hook codex Stop"#
        );
    }

    #[test]
    fn validates_event_names() {
        assert!(valid_event("PreToolUse"));
        assert!(!valid_event(""));
        assert!(!valid_event("Pre Tool"));
        assert!(!valid_event(&"a".repeat(65)));
    }

    #[test]
    fn reduces_oversized_payloads_to_identity() {
        let big = format!(
            r#"{{"session_id":"s-1","transcript_path":"/Users/u/.claude/projects/-p/s-1.jsonl","cwd":"/p \"q\"","hook_event_name":"PostToolUse","tool_name":"Read","tool_input":{{"cwd":"/nested"}},"tool_response":"{}"#,
            "x".repeat(100)
        );
        let v = reduced_payload(big.as_bytes()).unwrap();
        assert_eq!(v["session_id"], "s-1");
        assert_eq!(v["cwd"], "/p \"q\"");
        assert_eq!(v["tool_name"], "Read");
        assert_eq!(v["gilvt_truncated"], true);
        assert!(v.get("tool_use_id").is_none());
        assert!(reduced_payload(br#"{"tool_name":"Read"}"#).is_none(), "no session id, nothing to send");
    }
}
