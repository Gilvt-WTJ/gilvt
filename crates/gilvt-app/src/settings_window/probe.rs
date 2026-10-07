//! The settings window's slow work, off the main thread (S2 §5.2): Codex's model list, the 「其他…」 trial and
//! 「测试连接」. Each runs on its own thread and answers once through a channel; a window that closed meanwhile
//! dropped its receiving task, so late answers go nowhere.
//!
//! `models` / `trial` / `test_connection` fill in the login-shell PATH themselves (it can block up to 10 s on
//! first use), so call them only from a [`spawn`]ed job, never on the UI thread.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gilvt_monitor::chat::{self, ChatConfig, McpLaunch, CHAT_INSTRUCTIONS};
use gilvt_monitor::input::TERMINAL_INSTRUCTIONS;
use gilvt_monitor::provider::models::{self, ModelChoice};
use gilvt_monitor::provider::{self, OneShot, ProviderConfig, ProviderError, ProviderKind};

use crate::monitor::summaries::login_path;
use crate::settings::{MonitorProvider, MonitorSettings};

pub const MODEL_LIST_TIMEOUT: Duration = Duration::from_secs(15);
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
/// As a summary (S2 §4.1).
pub const TRIAL_TIMEOUT: Duration = Duration::from_secs(90);

/// The smallest real summary request: a terminal with one command (the fake brain answers it too).
pub const TRIAL_PROMPT: &str = "终端：gilvt 设置（试跑）\n## $ echo ok（exit 0，0.1s）\nok\n";

/// Where the CLIs run (the same directory the summaries use).
pub fn run_dir() -> PathBuf {
    crate::agents::state_dir().map(|d| d.join("monitor").join("run")).unwrap_or_else(|| std::env::temp_dir().join("gilvt-monitor").join("run"))
}

/// `path` stays None here: the probes add the login PATH on their own thread.
pub fn provider_config(m: &MonitorSettings, model: Option<&str>) -> ProviderConfig {
    ProviderConfig {
        kind: match m.provider {
            MonitorProvider::Claude => ProviderKind::Claude,
            MonitorProvider::Codex => ProviderKind::Codex,
        },
        program: m.command().map(str::to_string),
        model: model.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
        run_dir: run_dir(),
        path: None,
    }
}

/// `cfg` with the user's login-shell PATH (what the summaries run with). May block; probe threads only.
fn with_login_path(cfg: &ProviderConfig) -> ProviderConfig {
    ProviderConfig { path: login_path(), ..cfg.clone() }
}

/// One summary with `cfg` (its model is the one being tried).
pub fn trial(cfg: &ProviderConfig) -> Result<(), ProviderError> {
    let cfg = with_login_path(cfg);
    let req = OneShot { instructions: TERMINAL_INSTRUCTIONS.to_string(), prompt: TRIAL_PROMPT.to_string() };
    provider::summarize(&cfg, &req, TRIAL_TIMEOUT).map(|_| ())
}

/// The chat half of 「测试连接」: one turn that must call `list_sessions`.
pub const CHAT_TEST_TIMEOUT: Duration = Duration::from_secs(120);

/// What the chat half needs (built on the main thread: gilvt's CLI, its socket, a fresh test token).
pub struct ChatProbe {
    pub mcp: McpLaunch,
    /// The chat model (`[monitor] model`), not the summary one.
    pub model: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum ChatTest {
    /// Not tried (why: no gilvt CLI next to the app, no IPC socket, the CLI is missing).
    Skipped(String),
    Done(Result<Duration, ProviderError>),
}

pub struct TestResult {
    pub version: Result<String, ProviderError>,
    pub summary: Result<Duration, ProviderError>,
    pub chat: ChatTest,
}

/// `--version`, one timed summary, then one chat turn with a `list_sessions` call (each skipped when the program
/// is missing).
pub fn test_connection(cfg: &ProviderConfig, chat: Result<ChatProbe, String>) -> TestResult {
    let login = with_login_path(cfg);
    let version = provider::version(&login, VERSION_TIMEOUT);
    let missing = matches!(&version, Err(ProviderError::NotFound { .. }));
    let summary = match &version {
        Err(e @ ProviderError::NotFound { .. }) => Err(e.clone()),
        _ => {
            let started = Instant::now();
            trial(cfg).map(|()| started.elapsed())
        }
    };
    let chat = match (missing, chat) {
        (true, _) => ChatTest::Skipped("没有找到 CLI".into()),
        (false, Err(why)) => ChatTest::Skipped(why),
        (false, Ok(p)) => {
            let chat_cfg = ChatConfig {
                kind: cfg.kind,
                program: cfg.program.clone(),
                model: p.model,
                run_dir: cfg.run_dir.clone(),
                path: login.path.clone(),
                instructions: CHAT_INSTRUCTIONS.to_string(),
                mcp: p.mcp,
                log: crate::monitor::chat::log_path(),
                record: None,
            };
            ChatTest::Done(chat::probe(&chat_cfg, CHAT_TEST_TIMEOUT))
        }
    };
    TestResult { version, summary, chat }
}

pub fn models(cfg: &ProviderConfig) -> Result<Vec<ModelChoice>, ProviderError> {
    models::list_models(&with_login_path(cfg), MODEL_LIST_TIMEOUT)
}

/// Runs `job` on a thread of its own; the receiver gets its result once.
pub fn spawn<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> async_channel::Receiver<T> {
    let (tx, rx) = async_channel::bounded(1);
    let spawned = std::thread::Builder::new().name("gilvt-settings-probe".into()).spawn(move || {
        let _ = tx.send_blocking(job());
    });
    if let Err(e) = spawned {
        eprintln!("gilvt: could not start a settings probe: {e}");
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn provider_config_follows_the_settings() {
        let mut m = MonitorSettings::default();
        m.provider = MonitorProvider::Codex;
        m.command = " /opt/codex-w ".into();
        let cfg = provider_config(&m, Some(" gpt-x "));
        assert_eq!(cfg.kind, ProviderKind::Codex);
        assert_eq!(cfg.program.as_deref(), Some("/opt/codex-w"));
        assert_eq!(cfg.model.as_deref(), Some("gpt-x"));
        assert_eq!(cfg.path, None, "the login PATH is filled in on the probe thread");
        assert_eq!(provider_config(&MonitorSettings::default(), Some("")).model, None);
        assert!(cfg.run_dir.ends_with("run"));
    }

    #[test]
    fn test_connection_runs_version_then_a_summary() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("claude");
        std::fs::write(
            &program,
            "#!/bin/sh\ncase \"$1\" in --version) echo '9.9.9 (Claude Code)';; *) cat >/dev/null; echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"近期：ok\"}';; esac\n",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cfg = ProviderConfig { kind: ProviderKind::Claude, program: Some(program.display().to_string()), model: None, run_dir: dir.path().join("run"), path: None };
        let r = test_connection(&cfg, Err("测试".into()));
        assert_eq!(r.chat, ChatTest::Skipped("测试".into()));
        assert_eq!(r.version.unwrap(), "9.9.9");
        assert!(r.summary.is_ok(), "{:?}", r.summary);
        assert_eq!(trial(&cfg), Ok(()));
        let missing = ProviderConfig { program: Some("/nonexistent/claude".into()), ..cfg };
        let r = test_connection(&missing, Err("测试".into()));
        assert!(matches!(r.summary, Err(ProviderError::NotFound { .. })), "no summary attempt without a program");
        assert!(matches!(r.chat, ChatTest::Skipped(_)));
    }

    #[test]
    fn test_connection_chats_once_with_list_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("claude");
        let body = r#"#!/bin/sh
case "$1" in --version) echo '9.9.9 (Claude Code)'; exit 0;; esac
for a in "$@"; do [ "$a" = stream-json ] && chat=1; done
if [ -z "$chat" ]; then cat >/dev/null; echo '{"type":"result","subtype":"success","is_error":false,"result":"近期：ok"}'; exit 0; fi
read line
echo '{"type":"system","subtype":"init","model":"m","mcp_servers":[{"name":"gilvt","status":"connected"}]}'
echo '{"type":"assistant","message":{"id":"a","content":[{"type":"tool_use","id":"t1","name":"mcp__gilvt__list_sessions","input":{}}]}}'
echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"{}","is_error":false}]}}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"ok"}'
read line
"#;
        std::fs::write(&program, body).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cfg = ProviderConfig { kind: ProviderKind::Claude, program: Some(program.display().to_string()), model: None, run_dir: dir.path().join("run"), path: None };
        let mcp = McpLaunch { gilvt: "/bin/gilvt".into(), socket: "/tmp/s.sock".into(), token: "t".into() };
        let r = test_connection(&cfg, Ok(ChatProbe { mcp, model: None }));
        assert!(matches!(r.chat, ChatTest::Done(Ok(_))), "{:?}", r.chat);
    }

    #[test]
    fn spawn_delivers_the_result() {
        let rx = spawn(|| 41 + 1);
        assert_eq!(rx.recv_blocking().unwrap(), 42);
    }
}
