//! `gilvt hook …`: the agent side of M3a. Shared by the Mac CLI and the ssh host's `gilvt`; platform values come
//! in through [`HookConfig`].
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

use gilvt_agent::{hook_meta, AgentKind, LeaseStore};
use gilvt_ipc::{Request, ENV_PANE, ENV_SOCKET};

/// Largest hook payload forwarded whole; bigger ones are reduced to their identity fields.
pub const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
/// Oversized payloads are drained up to this much so the agent never sees a broken pipe.
const MAX_DRAIN_BYTES: u64 = 64 * 1024 * 1024;

/// What differs between the Mac's `gilvt` and the ssh host's (spec R2a §3.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookConfig {
    /// Written into the hook commands agents run: `'<gilvt>' hook <agent> <Event>`.
    pub gilvt: PathBuf,
    /// Holds the codex trust cache (and `leases/` when `lease`); None: no cache, codex runs without hooks.
    pub state_dir: Option<PathBuf>,
    /// Record a lease per hook (external-process control; the Mac only).
    pub lease: bool,
    /// On an ssh host: hooks name their link and tmux (R2a §4.1), and never go to every local app socket.
    pub remote: bool,
}

/// Entry point for `gilvt hook <args>`.
pub fn run(args: &[String], cfg: &HookConfig) -> ExitCode {
    let rest = |from: usize| -> &[String] {
        let r = args.get(from..).unwrap_or_default();
        r.strip_prefix(&["--".to_string()][..]).unwrap_or(r)
    };
    match args.first().map(String::as_str) {
        Some("claude-args") => print_argv(&claude::args_for(&cfg.gilvt, rest(1))),
        Some("codex-args") => print_argv(&codex::args_for(&cfg.gilvt, cfg.state_dir.as_deref(), rest(1))),
        Some("codex-trust") => {
            trust::run_job(&cfg.gilvt, cfg.state_dir.as_deref());
            ExitCode::SUCCESS
        }
        Some(agent @ ("claude" | "codex")) => {
            if let Some(event) = args.get(1).filter(|e| valid_event(e)) {
                forward(agent, event, cfg);
            }
            ExitCode::SUCCESS
        }
        // Unknown hook usage is still silent: this may be running inside an agent.
        _ => ExitCode::SUCCESS,
    }
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

/// The request a hook sends. On an ssh host it names the link (`GILVT_LINK`) and whether it ran in tmux.
pub fn hook_request(agent: &str, event: &str, payload: serde_json::Value, pane: Option<u64>, remote: bool, env: impl Fn(&str) -> Option<String>) -> Request {
    let (link, tmux) = if remote {
        (env("GILVT_LINK").filter(|l| !l.is_empty()), env("TMUX").is_some_and(|t| !t.is_empty()))
    } else {
        (None, false)
    };
    Request::Hook { pane, agent: agent.into(), event: event.into(), payload, link, tmux }
}

/// Reads the payload and sends it; every failure is silent.
fn forward(agent: &str, event: &str, cfg: &HookConfig) {
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
    if cfg.lease {
        if let (Some(kind), Some(state), Some(meta)) = (AgentKind::from_name(agent), cfg.state_dir.as_deref(), hook_meta(&payload)) {
            let _ = LeaseStore::open(state).update_hook(kind, meta.session_id, meta.transcript_path, meta.cwd, event);
        }
    }
    let pane = std::env::var(ENV_PANE).ok().and_then(|p| p.parse().ok());
    let req = hook_request(agent, event, payload, pane, cfg.remote, |k| std::env::var(k).ok());
    match std::env::var_os(ENV_SOCKET).filter(|s| !s.is_empty()) {
        Some(socket) => {
            let _ = gilvt_ipc::send_nowait(Path::new(&socket), &req);
        }
        // Integrated globally on the Mac (no pane): every running app gets it. A remote host has no app socket.
        None if !cfg.remote => {
            for socket in gilvt_ipc::live_socket_paths() {
                let _ = gilvt_ipc::send_nowait(&socket, &req);
            }
        }
        None => {}
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
    fn remote_hooks_name_their_link_and_tmux() {
        let env = |k: &str| match k { "GILVT_LINK" => Some("i-3".to_string()), "TMUX" => Some("/tmp/tmux-1/default,1,0".to_string()), _ => None };
        let r = hook_request("claude", "Stop", serde_json::json!({}), None, true, env);
        assert!(matches!(r, Request::Hook { link: Some(ref l), tmux: true, .. } if l == "i-3"), "{r:?}");
        let mac = hook_request("claude", "Stop", serde_json::json!({}), Some(2), false, env);
        assert!(matches!(mac, Request::Hook { link: None, tmux: false, pane: Some(2), .. }), "the Mac never fills them: {mac:?}");
        let no_tmux = hook_request("claude", "Stop", serde_json::json!({}), None, true, |k| (k == "TMUX").then(String::new));
        assert!(matches!(no_tmux, Request::Hook { link: None, tmux: false, .. }), "empty TMUX is not tmux");
    }

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

    #[test]
    fn codex_args_without_a_state_dir_leave_the_users_args_alone() {
        let user: Vec<String> = ["exec", "hi"].iter().map(|s| s.to_string()).collect();
        assert_eq!(codex::args_for(Path::new("/x/gilvt"), None, &user), user, "no trust cache: no hooks, no job");
    }
}
