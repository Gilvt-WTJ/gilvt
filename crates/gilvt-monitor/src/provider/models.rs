//! Which models the settings window offers (S2 §5.2): Claude's fixed aliases; Codex's `model/list` from
//! `codex app-server` (JSON-RPC 2.0, one object per line on stdio, paged with `cursor` / `nextCursor`).

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{classify, codex, process, ProviderConfig, ProviderError, ProviderKind};

/// Claude Code's model aliases (its list cannot be queried).
pub const CLAUDE_ALIASES: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];
/// More pages than this is not a model list.
pub const MAX_PAGES: usize = 10;
const PAGE_SIZE: u32 = 100;
/// Non-JSON lines tolerated on stdout (banners, logs) while waiting for one answer.
const MAX_BAD_LINES: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChoice {
    /// What goes into config.toml and `--model` / `-m`.
    pub id: String,
    pub display: String,
    pub is_default: bool,
}

/// `--disable hooks`, the user's MCP servers switched off (as the summary run does), then `app-server`.
pub fn app_server_args(mcp_servers: &[String]) -> Vec<String> {
    let mut a: Vec<String> = ["--disable", "hooks"].map(String::from).to_vec();
    a.extend(codex::mcp_off_args(mcp_servers));
    a.push("app-server".into());
    a
}

pub fn initialize_request(id: u64) -> Value {
    json!({"id": id, "method": "initialize", "params": {"clientInfo": {"name": "gilvt", "version": env!("CARGO_PKG_VERSION")}}})
}

pub fn model_list_request(id: u64, cursor: Option<&str>) -> Value {
    json!({"id": id, "method": "model/list", "params": {"cursor": cursor, "includeHidden": false, "limit": PAGE_SIZE}})
}

/// One `model/list` answer: its visible models and the next page's cursor.
pub fn parse_model_page(resp: &Value) -> Result<(Vec<ModelChoice>, Option<String>), ProviderError> {
    if let Some(err) = resp.get("error") {
        let message = err.get("message").and_then(Value::as_str).unwrap_or("").to_string();
        return Err(match classify(&message, None) {
            auth @ ProviderError::Auth(_) => auth,
            _ => ProviderError::Protocol(format!("model/list：{message}")),
        });
    }
    let result = resp.get("result").ok_or_else(|| ProviderError::Protocol(gilvt_i18n::text("model/list 没有 result", "model/list has no result").into()))?;
    let data = result.get("data").and_then(Value::as_array).ok_or_else(|| ProviderError::Protocol(gilvt_i18n::text("model/list 没有 data", "model/list has no data").into()))?;
    let models = data
        .iter()
        .filter(|m| !m.get("hidden").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|m| {
            let id = m.get("model").or_else(|| m.get("id")).and_then(Value::as_str)?.to_string();
            let display = m.get("displayName").and_then(Value::as_str).filter(|d| !d.is_empty()).unwrap_or(id.as_str()).to_string();
            Some(ModelChoice { id, display, is_default: m.get("isDefault").and_then(Value::as_bool).unwrap_or(false) })
        })
        .collect();
    Ok((models, result.get("nextCursor").and_then(Value::as_str).map(str::to_string)))
}

/// The models to offer for `cfg`'s CLI. Claude: the aliases (no process). Codex: every page of `model/list`.
pub fn list_models(cfg: &ProviderConfig, timeout: Duration) -> Result<Vec<ModelChoice>, ProviderError> {
    match cfg.kind {
        ProviderKind::Claude => Ok(CLAUDE_ALIASES.iter().map(|a| ModelChoice { id: a.to_string(), display: a.to_string(), is_default: false }).collect()),
        ProviderKind::Codex => {
            std::fs::create_dir_all(&cfg.run_dir).map_err(|e| ProviderError::Exited { code: None, stderr_tail: e.to_string() })?;
            codex_models(&cfg.program(), &cfg.run_dir, cfg.path.as_deref(), &codex::configured_mcp_servers(), timeout)
        }
    }
}

enum Fail {
    /// stdout closed (the process exited, or its stdin did).
    Closed,
    Err(ProviderError),
}

/// Runs `<program> --disable hooks [-c mcp_servers=…] app-server` in `cwd` (found on `path` when given, which the
/// child also gets as its `PATH`, like [`process::run`]) and asks for every page of models. The process group is
/// listed for [`process::kill_all`] while it lives and killed afterwards, whatever happened.
pub fn codex_models(program: &str, cwd: &Path, path: Option<&str>, mcp_servers: &[String], timeout: Duration) -> Result<Vec<ModelChoice>, ProviderError> {
    let not_found = || ProviderError::NotFound { program: program.to_string() };
    let mut cmd = match path {
        Some(path) => {
            let mut cmd = Command::new(process::resolve(program, path).ok_or_else(not_found)?);
            cmd.env("PATH", path);
            cmd
        }
        None => Command::new(program),
    };
    cmd.args(app_server_args(mcp_servers)).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.process_group(0);
    for key in process::SCRUB_ENV {
        cmd.env_remove(key);
    }
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => not_found(),
        _ => ProviderError::Exited { code: None, stderr_tail: e.to_string() },
    })?;
    let pid = child.id() as i32;
    process::track_group(pid);
    let (tx, rx) = mpsc::channel::<String>();
    let stdout = child.stdout.take().expect("piped stdout");
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let (err_tx, err_rx) = mpsc::channel::<String>();
    let stderr = child.stderr.take().expect("piped stderr");
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.take(64 * 1024).read_to_end(&mut buf);
        let _ = err_tx.send(String::from_utf8_lossy(&buf).into_owned());
    });
    let mut stdin = child.stdin.take().expect("piped stdin");
    let result = exchange(&mut stdin, &rx, Instant::now() + timeout);
    drop(stdin);
    // SAFETY: killpg only sends a signal, to the group this child leads (process_group(0) above).
    unsafe {
        libc::killpg(pid, libc::SIGKILL);
    }
    let code = child.wait().ok().and_then(|s| s.code());
    process::untrack_group(pid);
    match result {
        Ok(models) => Ok(models),
        Err(Fail::Err(e)) => Err(e),
        Err(Fail::Closed) => {
            let stderr = err_rx.recv_timeout(Duration::from_millis(500)).unwrap_or_default();
            Err(classify(&stderr, code))
        }
    }
}

fn exchange(stdin: &mut impl Write, rx: &mpsc::Receiver<String>, deadline: Instant) -> Result<Vec<ModelChoice>, Fail> {
    send(stdin, &initialize_request(1))?;
    recv(rx, 1, deadline)?;
    send(stdin, &json!({"method": "initialized"}))?;
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for page in 0..MAX_PAGES {
        let id = 2 + page as u64;
        send(stdin, &model_list_request(id, cursor.as_deref()))?;
        let (models, next) = parse_model_page(&recv(rx, id, deadline)?).map_err(Fail::Err)?;
        out.extend(models);
        match next {
            Some(c) => cursor = Some(c),
            None => return Ok(out),
        }
    }
    Err(Fail::Err(ProviderError::Protocol(if gilvt_i18n::english() {
        format!("model/list has more than {MAX_PAGES} pages")
    } else {
        format!("model/list 超过 {MAX_PAGES} 页")
    })))
}

fn send(stdin: &mut impl Write, msg: &Value) -> Result<(), Fail> {
    writeln!(stdin, "{msg}").and_then(|()| stdin.flush()).map_err(|_| Fail::Closed)
}

/// The answer with `id`; notifications, other answers and up to [`MAX_BAD_LINES`] non-JSON lines are skipped.
fn recv(rx: &mpsc::Receiver<String>, id: u64, deadline: Instant) -> Result<Value, Fail> {
    let mut bad = 0;
    loop {
        let left = deadline.checked_duration_since(Instant::now()).ok_or(Fail::Err(ProviderError::Timeout))?;
        let line = match rx.recv_timeout(left) {
            Ok(l) => l,
            Err(RecvTimeoutError::Timeout) => return Err(Fail::Err(ProviderError::Timeout)),
            Err(RecvTimeoutError::Disconnected) => return Err(Fail::Closed),
        };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            bad += 1;
            if bad > MAX_BAD_LINES {
                return Err(Fail::Err(ProviderError::Protocol(gilvt_i18n::text("app-server 的输出无法解析", "could not parse app-server's output").into())));
            }
            continue;
        };
        if msg.get("id").and_then(Value::as_u64) == Some(id) {
            return Ok(msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{version, version_number, ProviderKind};
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    /// An executable shell script `fake-codex` in `dir`.
    fn script(dir: &Path, body: &str) -> String {
        let p = dir.join("fake-codex");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.display().to_string()
    }

    const PAGE1: &str = r#"{"id":2,"result":{"data":[{"id":"m-a","model":"m-a","displayName":"Model A","description":"","hidden":false,"isDefault":true,"defaultReasoningEffort":"medium","supportedReasoningEfforts":[]},{"id":"m-h","model":"m-h","displayName":"Hidden","description":"","hidden":true,"isDefault":false,"defaultReasoningEffort":"medium","supportedReasoningEfforts":[]}],"nextCursor":"p2"}}"#;
    const PAGE2: &str = r#"{"id":3,"result":{"data":[{"id":"m-b","model":"m-b","displayName":"","description":"","hidden":false,"isDefault":false,"defaultReasoningEffort":"low","supportedReasoningEfforts":[]}],"nextCursor":null}}"#;

    /// Answers in the order gilvt asks: initialize, (initialized), page 1, page 2 only for cursor "p2".
    const SERVER: &str = r#"echo "$@" > args.txt
echo "$PATH" > path.txt
read l; echo '{"id":1,"result":{"userAgent":"fake"}}'
read l
read l; echo 'Starting app-server…'; echo '{"method":"thread/status","params":{}}'; echo 'PAGE1'
read l; case "$l" in *'"cursor":"p2"'*) echo 'PAGE2';; *) echo '{"id":3,"error":{"code":-32600,"message":"bad cursor"}}';; esac
read l"#;

    fn server() -> String {
        SERVER.replace("PAGE1", PAGE1).replace("PAGE2", PAGE2)
    }

    const T: Duration = Duration::from_secs(10);

    #[test]
    fn pages_are_followed_and_hidden_models_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), &server());
        let models = codex_models(&program, dir.path(), None, &[], T).unwrap();
        assert_eq!(
            models,
            vec![
                ModelChoice { id: "m-a".into(), display: "Model A".into(), is_default: true },
                ModelChoice { id: "m-b".into(), display: "m-b".into(), is_default: false },
            ]
        );
        assert_eq!(std::fs::read_to_string(dir.path().join("args.txt")).unwrap(), "--disable hooks app-server\n");
    }

    #[test]
    fn the_users_mcp_servers_are_switched_off() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), &server());
        codex_models(&program, dir.path(), None, &["lark-docs".into(), "odd name".into()], T).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("args.txt")).unwrap(),
            "--disable hooks -c mcp_servers={\"lark-docs\"={enabled=false},\"odd name\"={enabled=false}} app-server\n"
        );
    }

    #[test]
    fn the_program_is_found_on_the_given_path_and_gets_it() {
        let dir = tempfile::tempdir().unwrap();
        script(dir.path(), &server());
        let path = format!("{}:/usr/bin:/bin", dir.path().display());
        let models = codex_models("fake-codex", dir.path(), Some(&path), &[], T).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(std::fs::read_to_string(dir.path().join("path.txt")).unwrap().trim(), path);
        assert_eq!(
            codex_models("fake-codex", dir.path(), Some("/usr/bin:/bin"), &[], T),
            Err(ProviderError::NotFound { program: "fake-codex".into() })
        );
    }

    #[test]
    fn the_group_is_unlisted_and_killed_after_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "echo $$ > pid; sleep 30");
        // Wide enough for a slow first exec when many tests spawn fresh scripts at once.
        let _ = codex_models(&program, dir.path(), None, &[], Duration::from_millis(3000));
        let pid: i32 = std::fs::read_to_string(dir.path().join("pid")).unwrap().trim().parse().unwrap();
        assert!(!crate::provider::process::live_groups().contains(&pid));
        // SAFETY: signal 0 only checks that the group still has a member.
        assert_ne!(unsafe { libc::killpg(pid, 0) }, 0, "the group was killed");
    }

    #[test]
    fn missing_codex_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            codex_models("/nonexistent/codex", dir.path(), None, &[], Duration::from_secs(1)),
            Err(ProviderError::NotFound { program: "/nonexistent/codex".into() })
        );
    }

    #[test]
    fn hanging_app_server_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "sleep 30");
        let t = Instant::now();
        assert_eq!(codex_models(&program, dir.path(), None, &[], Duration::from_millis(300)), Err(ProviderError::Timeout));
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    }

    #[test]
    fn early_exit_reports_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "echo 'error: unrecognized subcommand app-server' >&2; exit 2");
        assert_eq!(
            codex_models(&program, dir.path(), None, &[], Duration::from_secs(5)),
            Err(ProviderError::Exited { code: Some(2), stderr_tail: "error: unrecognized subcommand app-server".into() })
        );
    }

    #[test]
    fn rpc_errors() {
        let r = parse_model_page(&json!({"id": 2, "error": {"code": -32603, "message": "catalog unavailable"}}));
        assert_eq!(r, Err(ProviderError::Protocol("model/list：catalog unavailable".into())));
        let r = parse_model_page(&json!({"id": 2, "error": {"code": -32603, "message": "401 Unauthorized"}}));
        assert!(matches!(r, Err(ProviderError::Auth(_))), "{r:?}");
        assert!(matches!(parse_model_page(&json!({"id": 2, "result": {}})), Err(ProviderError::Protocol(_))));
    }

    #[test]
    fn schema_example_parses() {
        let v: Value = serde_json::from_str(include_str!("../../tests/fixtures/codex-model-list.json")).unwrap();
        let (models, next) = parse_model_page(&v).unwrap();
        assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["gpt-5.5"]);
        assert_eq!(next, None);
    }

    #[test]
    fn claude_list_is_the_aliases_without_a_process() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ProviderConfig { kind: ProviderKind::Claude, program: Some("/nonexistent/claude".into()), model: None, run_dir: dir.path().into(), path: None };
        let ids: Vec<String> = list_models(&cfg, Duration::from_secs(1)).unwrap().into_iter().map(|m| m.id).collect();
        assert_eq!(ids, CLAUDE_ALIASES);
    }

    #[test]
    fn version_reads_the_number() {
        assert_eq!(version_number("2.1.291 (Claude Code)\n").as_deref(), Some("2.1.291"));
        assert_eq!(version_number("codex-cli 0.160.0\n").as_deref(), Some("0.160.0"));
        assert_eq!(version_number("hello\n"), None);
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "echo 'codex-cli 9.8.7'");
        let cfg = ProviderConfig { kind: ProviderKind::Codex, program: Some(program), model: None, run_dir: dir.path().join("run"), path: None };
        assert_eq!(cfg.program(), cfg.program.clone().unwrap());
        assert_eq!(version(&cfg, Duration::from_secs(5)).unwrap(), "9.8.7");
        let missing = ProviderConfig { program: Some("/nonexistent/claude".into()), ..cfg.clone() };
        assert_eq!(version(&missing, Duration::from_secs(5)), Err(ProviderError::NotFound { program: "/nonexistent/claude".into() }));
        assert_eq!(ProviderConfig { program: None, ..cfg }.program(), "codex");
    }
}
