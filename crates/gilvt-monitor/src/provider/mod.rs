//! Running the user's own `claude` / `codex` CLI without a terminal (S2 §4.1).

pub mod claude;
pub mod codex;
pub mod models;
pub mod process;

use std::path::PathBuf;
use std::time::Duration;

use gilvt_i18n::english;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    Claude,
    Codex,
}

impl ProviderKind {
    pub fn default_program(self) -> &'static str {
        match self {
            ProviderKind::Claude => "claude",
            ProviderKind::Codex => "codex",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    /// `[monitor] command`; None = `claude` / `codex` on PATH.
    pub program: Option<String>,
    pub model: Option<String>,
    /// The child's working directory (empty, under gilvt's state).
    pub run_dir: PathBuf,
    /// `PATH` to find the CLI on and to give it (the login shell's, see [`process::login_shell_path`]);
    /// None = gilvt's own.
    pub path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OneShot {
    /// What to do (Claude: `--system-prompt`; Codex: put before the data).
    pub instructions: String,
    /// The data to summarize.
    pub prompt: String,
}

const AUTH_WORDS: [&str; 7] = ["not logged in", "login", "authentication", "unauthorized", "401", "invalid api key", "api key"];

/// A failure's text as an error: auth words → [`ProviderError::Auth`], else its last non-empty line.
pub fn classify(text: &str, code: Option<i32>) -> ProviderError {
    let lower = text.to_lowercase();
    if AUTH_WORDS.iter().any(|w| lower.contains(w)) {
        return ProviderError::Auth(text.trim().to_string());
    }
    let tail = text.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
    ProviderError::Exited { code, stderr_tail: crate::output::clip_chars(&tail, 200) }
}

impl ProviderConfig {
    /// The program run: `[monitor] command`, else `claude` / `codex` on PATH.
    pub fn program(&self) -> String {
        self.program.clone().unwrap_or_else(|| self.kind.default_program().to_string())
    }
}

/// `<program> --version`, as its version number (`2.1.291`, `0.160.0`).
pub fn version(cfg: &ProviderConfig, timeout: Duration) -> Result<String, ProviderError> {
    let program = cfg.program();
    std::fs::create_dir_all(&cfg.run_dir).map_err(|e| ProviderError::Exited { code: None, stderr_tail: e.to_string() })?;
    let out = process::run(&program, &["--version".to_string()], "", &cfg.run_dir, cfg.path.as_deref(), timeout)?;
    if out.code != Some(0) {
        return Err(classify(&format!("{}\n{}", out.stdout, out.stderr), out.code));
    }
    version_number(&out.stdout).ok_or_else(|| ProviderError::Protocol(if english() {
        format!("--version printed: {}", out.stdout.trim())
    } else {
        format!("--version 输出：{}", out.stdout.trim())
    }))
}

/// The first word that starts with a digit: `2.1.291 (Claude Code)` → `2.1.291`, `codex-cli 0.160.0` → `0.160.0`.
pub fn version_number(text: &str) -> Option<String> {
    text.split_whitespace().find(|w| w.starts_with(|c: char| c.is_ascii_digit())).map(str::to_string)
}

/// One summary from the configured CLI: the model's raw answer.
pub fn summarize(cfg: &ProviderConfig, req: &OneShot, timeout: Duration) -> Result<String, ProviderError> {
    let program = cfg.program();
    std::fs::create_dir_all(&cfg.run_dir).map_err(|e| ProviderError::Exited { code: None, stderr_tail: e.to_string() })?;
    match cfg.kind {
        ProviderKind::Claude => {
            let out = process::run(&program, &claude::args(cfg.model.as_deref(), &req.instructions), &req.prompt, &cfg.run_dir, cfg.path.as_deref(), timeout)?;
            claude::parse(&out)
        }
        ProviderKind::Codex => {
            let file = cfg.run_dir.join(format!("last-{}-{}.txt", std::process::id(), unique()));
            let stdin = format!("{}\n\n{}", req.instructions, req.prompt);
            let mcp = codex::configured_mcp_servers();
            let out = process::run(&program, &codex::args(cfg.model.as_deref(), &file, &mcp), &stdin, &cfg.run_dir, cfg.path.as_deref(), timeout);
            let last = std::fs::read_to_string(&file).ok();
            let _ = std::fs::remove_file(&file);
            codex::parse(&out?, last)
        }
    }
}

/// Distinct per call within this process (two summaries run at once).
fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    /// The program is not on the login shell's PATH (or the configured path does not exist).
    NotFound { program: String },
    /// Not logged in / rejected credentials.
    Auth(String),
    Timeout,
    Exited { code: Option<i32>, stderr_tail: String },
    /// It ran but its output was not what the protocol says.
    Protocol(String),
    /// The CLI cannot be used the way gilvt needs (e.g. a Codex without a switch that turns its shell off).
    Unsupported(String),
}

impl ProviderError {
    /// A short reason for the card header / the settings page.
    pub fn message(&self) -> String {
        if english() {
            return match self {
                ProviderError::NotFound { program } => {
                    format!("{program} not found (set its full path in [monitor] command)")
                }
                ProviderError::Auth(_) => "Authentication failed (log in to this CLI in a terminal)".into(),
                ProviderError::Timeout => "Timed out".into(),
                ProviderError::Exited { code: Some(c), stderr_tail } if !stderr_tail.is_empty() => {
                    format!("Exit code {c}: {stderr_tail}")
                }
                ProviderError::Exited { code: Some(c), .. } => format!("Exit code {c}"),
                ProviderError::Exited { code: None, .. } => "The process was ended by a signal".into(),
                ProviderError::Protocol(m) => format!("Could not parse the output: {m}"),
                ProviderError::Unsupported(m) => m.clone(),
            };
        }
        match self {
            ProviderError::NotFound { program } => format!("未找到 {program}（请在 [monitor] command 里填写完整路径）"),
            ProviderError::Auth(_) => "认证失败（请在终端里登录这个 CLI）".into(),
            ProviderError::Timeout => "超时".into(),
            ProviderError::Exited { code: Some(c), stderr_tail } if !stderr_tail.is_empty() => format!("退出码 {c}：{stderr_tail}"),
            ProviderError::Exited { code: Some(c), .. } => format!("退出码 {c}"),
            ProviderError::Exited { code: None, .. } => "进程被信号结束".into(),
            ProviderError::Protocol(m) => format!("输出无法解析：{m}"),
            ProviderError::Unsupported(m) => m.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_auth_words() {
        for text in ["Not logged in", "Invalid API key", "401 Unauthorized", "Please run /login", "authentication failed"] {
            assert!(matches!(classify(text, Some(1)), ProviderError::Auth(_)), "{text}");
        }
        assert_eq!(classify("line1\nboom\n", Some(3)), ProviderError::Exited { code: Some(3), stderr_tail: "boom".into() });
    }

    /// A fake CLI in its own directory that logs its argv (one per line), stdin and cwd into `bin/`.
    fn fake_cli(bin: &std::path::Path, name: &str, body: &str) -> String {
        let script = format!(
            "#!/bin/sh\nLOG='{}'\nprintf '%s\\n' \"$@\" > \"$LOG/argv\"\npwd > \"$LOG/pwd\"\ncat > \"$LOG/stdin\"\n{body}",
            bin.display()
        );
        let path = bin.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    #[test]
    fn summarize_runs_a_fake_claude() {
        let bin = tempfile::tempdir().unwrap();
        let run = tempfile::tempdir().unwrap();
        let program = fake_cli(bin.path(), "claude", "echo '{\"type\":\"result\",\"is_error\":false,\"result\":\"近期：ok\"}'\n");
        let cfg = ProviderConfig { kind: ProviderKind::Claude, program: Some(program), model: None, run_dir: run.path().to_path_buf(), path: None };
        let got = summarize(&cfg, &OneShot { instructions: "I".into(), prompt: "P".into() }, std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(got, "近期：ok");
        assert_eq!(std::fs::read_to_string(bin.path().join("stdin")).unwrap(), "P", "the prompt goes through stdin");
        let argv: Vec<String> = std::fs::read_to_string(bin.path().join("argv")).unwrap().lines().map(String::from).collect();
        let at = argv.iter().position(|a| a == "--system-prompt").expect("--system-prompt in argv");
        assert_eq!(argv[at + 1], "I", "the instructions follow --system-prompt");
        let cwd = std::fs::read_to_string(bin.path().join("pwd")).unwrap();
        assert_eq!(std::fs::canonicalize(cwd.trim()).unwrap(), std::fs::canonicalize(run.path()).unwrap());
    }

    #[test]
    fn summarize_finds_the_cli_on_the_login_path() {
        let bin = tempfile::tempdir().unwrap();
        let run = tempfile::tempdir().unwrap();
        fake_cli(bin.path(), "claude", "echo '{\"type\":\"result\",\"is_error\":false,\"result\":\"近期：path ok\"}'\n");
        let path = format!("{}:/usr/bin:/bin", bin.path().display());
        let cfg = ProviderConfig { kind: ProviderKind::Claude, program: None, model: None, run_dir: run.path().to_path_buf(), path: Some(path) };
        let got = summarize(&cfg, &OneShot { instructions: "I".into(), prompt: "P".into() }, std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(got, "近期：path ok");
        let empty = tempfile::tempdir().unwrap();
        let cfg = ProviderConfig { path: Some(empty.path().display().to_string()), ..cfg };
        let err = summarize(&cfg, &OneShot { instructions: "I".into(), prompt: "P".into() }, std::time::Duration::from_secs(5)).unwrap_err();
        assert_eq!(err, ProviderError::NotFound { program: "claude".into() });
    }

    #[test]
    fn summarize_runs_a_fake_codex() {
        let bin = tempfile::tempdir().unwrap();
        let run = tempfile::tempdir().unwrap();
        // `-o FILE` is the argument after `-o`.
        let body = "while [ $# -gt 0 ]; do if [ \"$1\" = -o ]; then out=$2; fi; shift; done\nprintf '近期：codex ok' >\"$out\"\n";
        let program = fake_cli(bin.path(), "codex", body);
        let cfg = ProviderConfig { kind: ProviderKind::Codex, program: Some(program), model: None, run_dir: run.path().to_path_buf(), path: None };
        let got = summarize(&cfg, &OneShot { instructions: "I".into(), prompt: "P".into() }, std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(got, "近期：codex ok");
        assert_eq!(std::fs::read_to_string(bin.path().join("stdin")).unwrap(), "I\n\nP", "instructions, a blank line, then the prompt");
        let cwd = std::fs::read_to_string(bin.path().join("pwd")).unwrap();
        assert_eq!(std::fs::canonicalize(cwd.trim()).unwrap(), std::fs::canonicalize(run.path()).unwrap());
    }

    #[test]
    fn unsupported_reads_as_its_reason() {
        let e = ProviderError::Unsupported("当前 Codex 里找不到 shell_tool 这个开关".into());
        assert_eq!(e.message(), "当前 Codex 里找不到 shell_tool 这个开关");
    }
}
