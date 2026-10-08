//! One CLI run: stdin in, stdout / stderr out, killed at a deadline. Every running CLI's process group is
//! listed ([`live_groups`]) so gilvt can end them when it quits ([`kill_all`]).

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use super::ProviderError;

/// Variables gilvt's panes or a parent agent set that must not reach the CLI (it would think it is nested, or
/// talk to gilvt's socket).
pub const SCRUB_ENV: &[&str] = &["GILVT_SOCKET", "GILVT_MONITOR_TOKEN", "GILVT_PANE_ID", "CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CODEX_SANDBOX", "CODEX_SANDBOX_NETWORK_DISABLED"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Process groups of running CLIs.
#[derive(Debug, Default)]
pub struct Registry(Mutex<BTreeSet<i32>>);

impl Registry {
    pub fn add(&self, pgid: i32) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(pgid);
    }

    pub fn remove(&self, pgid: i32) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(&pgid);
    }

    pub fn live(&self) -> Vec<i32> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).iter().copied().collect()
    }

    /// SIGTERM to every group, up to `grace` for them to go, then SIGKILL to what is left (spec §7).
    pub fn kill_all(&self, grace: Duration) {
        let groups = self.live();
        if groups.is_empty() {
            return;
        }
        for g in &groups {
            // SAFETY: killpg only sends a signal.
            unsafe { libc::killpg(*g, libc::SIGTERM) };
        }
        let deadline = Instant::now() + grace;
        // SAFETY: signal 0 only checks that some member of the group is still there.
        let alive = |g: &i32| unsafe { libc::killpg(*g, 0) } == 0;
        while groups.iter().any(alive) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        for g in groups.iter().filter(|g| alive(g)) {
            killpg(*g);
        }
        for g in &groups {
            self.remove(*g);
        }
    }
}

static GROUPS: Registry = Registry(Mutex::new(BTreeSet::new()));

/// The process groups of the CLIs running now.
pub fn live_groups() -> Vec<i32> {
    GROUPS.live()
}

/// Lists a process group led by a CLI started elsewhere in this crate, so [`kill_all`] covers it; pair with [`untrack_group`].
pub(crate) fn track_group(pgid: i32) {
    GROUPS.add(pgid)
}

pub(crate) fn untrack_group(pgid: i32) {
    GROUPS.remove(pgid)
}

/// Ends every running CLI: gilvt is quitting.
pub fn kill_all(grace: Duration) {
    GROUPS.kill_all(grace)
}

fn killpg(pgid: i32) {
    // SAFETY: killpg only sends a signal.
    unsafe {
        libc::killpg(pgid, libc::SIGKILL);
    }
}

/// `program` as found on `path` (`PATH` syntax); a name with a `/` is taken as is.
pub fn resolve(program: &str, path: &str) -> Option<PathBuf> {
    if program.contains('/') {
        return Some(PathBuf::from(program));
    }
    let executable = |p: &Path| std::fs::metadata(p).is_ok_and(|m| m.is_file() && std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o111 != 0);
    path.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join(program)).find(|p| executable(p))
}

/// Brackets the value the login shell prints, so profile chatter around it is ignored.
pub const MARK: &str = "__GILVT_PATH__";

/// `PATH` as the user's login shell sets it (`<shell> -lic …`): a gilvt started from the Finder or the Dock
/// only has launchd's `/usr/bin:/bin:/usr/sbin:/sbin`. None when the shell fails or takes over `timeout`.
pub fn login_shell_path(shell: &str, timeout: Duration) -> Option<String> {
    let script = format!("printf '{MARK}%s{MARK}' \"$PATH\"");
    let cwd = std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_dir()).unwrap_or_else(|| PathBuf::from("/"));
    let out = run(shell, &["-lic".to_string(), script], "", &cwd, None, timeout).ok()?;
    let mut parts = out.stdout.split(MARK);
    let value = parts.nth(1)?.trim();
    // The closing mark is there too (a cut-off answer is no answer).
    parts.next()?;
    (!value.is_empty()).then(|| value.to_string())
}

/// Runs `program` (found on `path` when given, which the child also gets as its `PATH`).
pub fn run(program: &str, args: &[String], stdin: &str, cwd: &Path, path: Option<&str>, timeout: Duration) -> Result<Output, ProviderError> {
    let not_found = || ProviderError::NotFound { program: program.to_string() };
    let mut cmd = match path {
        Some(path) => {
            let mut cmd = Command::new(resolve(program, path).ok_or_else(not_found)?);
            cmd.env("PATH", path);
            cmd
        }
        None => Command::new(program),
    };
    cmd.args(args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.process_group(0);
    for key in SCRUB_ENV {
        cmd.env_remove(key);
    }
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => not_found(),
        _ => ProviderError::Exited { code: None, stderr_tail: e.to_string() },
    })?;
    let pid = child.id() as i32;
    GROUPS.add(pid);
    let result = wait_for(&mut child, pid, stdin, timeout);
    GROUPS.remove(pid);
    result
}

fn wait_for(child: &mut std::process::Child, pid: i32, stdin: &str, timeout: Duration) -> Result<Output, ProviderError> {
    let mut input = child.stdin.take().expect("piped stdin");
    let text = stdin.to_string();
    let _writer = std::thread::spawn(move || {
        let _ = input.write_all(text.as_bytes());
    });
    let (out_tx, out_rx) = mpsc::channel();
    let (err_tx, err_rx) = mpsc::channel();
    let mut stdout_pipe = child.stdout.take().expect("piped stdout");
    let mut stderr_pipe = child.stderr.take().expect("piped stderr");
    let _out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        let _ = out_tx.send(String::from_utf8_lossy(&buf).into_owned());
    });
    let _err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        let _ = err_tx.send(String::from_utf8_lossy(&buf).into_owned());
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                killpg(pid);
                let _ = child.wait();
                return Err(ProviderError::Timeout);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(ProviderError::Exited { code: None, stderr_tail: e.to_string() }),
        }
    };
    // The CLI is done: whatever it left running in its group goes too (and lets go of the pipes).
    killpg(pid);
    let remaining = deadline.saturating_duration_since(Instant::now());
    let Ok(stdout) = out_rx.recv_timeout(remaining) else { return Err(ProviderError::Timeout) };
    let remaining = deadline.saturating_duration_since(Instant::now());
    let Ok(stderr) = err_rx.recv_timeout(remaining) else { return Err(ProviderError::Timeout) };
    Ok(Output { code: status.code(), stdout, stderr })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn sh(script: &str) -> Vec<String> {
        vec!["-c".into(), script.into()]
    }

    #[test]
    fn passes_stdin_and_collects_output() {
        let dir = tempfile::tempdir().unwrap();
        let out = run("/bin/sh", &sh("cat; echo err >&2; pwd"), "hello\n", dir.path(), None, Duration::from_secs(5)).unwrap();
        assert_eq!(out.code, Some(0));
        assert!(out.stdout.starts_with("hello\n"));
        assert!(out.stdout.trim_end().ends_with(dir.path().file_name().unwrap().to_str().unwrap()));
        assert_eq!(out.stderr, "err\n");
    }

    #[test]
    fn large_stdin_does_not_deadlock() {
        let dir = tempfile::tempdir().unwrap();
        let big = "x".repeat(300_000);
        let out = run("/bin/sh", &sh("cat"), &big, dir.path(), None, Duration::from_secs(10)).unwrap();
        assert_eq!(out.stdout.len(), big.len());
    }

    #[test]
    fn missing_program_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = run("gilvt-no-such-cli", &[], "", dir.path(), None, Duration::from_secs(1)).unwrap_err();
        assert_eq!(err, ProviderError::NotFound { program: "gilvt-no-such-cli".into() });
    }

    #[test]
    fn hanging_program_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let t0 = Instant::now();
        let err = run("/bin/sh", &sh("sleep 30"), "", dir.path(), None, Duration::from_millis(300)).unwrap_err();
        assert_eq!(err, ProviderError::Timeout);
        assert!(t0.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn scrubbed_variables_do_not_reach_the_child() {
        let dir = tempfile::tempdir().unwrap();
        // Set all SCRUB_ENV variables
        for key in SCRUB_ENV {
            std::env::set_var(key, "should-not-leak");
        }
        // Run a child that prints all env vars matching the scrubbed patterns
        let out = run("/bin/sh", &sh("env | grep -E '^(GILVT_|CLAUDE|CODEX_)' || true"), "", dir.path(), None, Duration::from_secs(5)).unwrap();
        // Assert none of the scrubbed names appear
        for key in SCRUB_ENV {
            assert!(!out.stdout.contains(key), "Variable {} should not appear in child output", key);
        }
        // Clean up: remove all the variables we set
        for key in SCRUB_ENV {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn hanging_program_with_subprocesses_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let t0 = Instant::now();
        // Process spawns background job and waits for it; whole process group should be killed
        let err = run("/bin/sh", &sh("sleep 30 & wait"), "", dir.path(), None, Duration::from_millis(300)).unwrap_err();
        assert_eq!(err, ProviderError::Timeout);
        assert!(t0.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn child_exits_but_grandchild_holds_pipes_returns_promptly() {
        let dir = tempfile::tempdir().unwrap();
        let t0 = Instant::now();
        // Child exits and prints "done", but grandchild sleeps and holds the pipes
        let result = run("/bin/sh", &sh("(sleep 30 &); echo done"), "", dir.path(), None, Duration::from_secs(5));
        // The group is killed once the CLI exits, so its leftover holds the pipes no longer.
        assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
        assert_eq!(result.unwrap().stdout, "done\n");
    }

    fn fake(dir: &Path, name: &str, body: &str) {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }

    #[test]
    fn the_program_is_found_on_the_given_path() {
        let dir = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        fake(bin.path(), "gilvt-test-cli", "echo \"found:$PATH\"");
        let path = format!("{}:/usr/bin:/bin", bin.path().display());
        assert!(run("gilvt-test-cli", &[], "", dir.path(), None, Duration::from_secs(5)).is_err(), "not on this process's PATH");
        let out = run("gilvt-test-cli", &[], "", dir.path(), Some(&path), Duration::from_secs(5)).unwrap();
        assert_eq!(out.stdout.trim(), format!("found:{path}"), "found there, and the child gets that PATH");
    }

    #[test]
    fn not_on_the_given_path_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let empty = tempfile::tempdir().unwrap();
        let err = run("sh", &sh("true"), "", dir.path(), Some(&empty.path().display().to_string()), Duration::from_secs(1)).unwrap_err();
        assert_eq!(err, ProviderError::NotFound { program: "sh".into() });
    }

    #[test]
    fn login_path_is_read_between_markers() {
        let bin = tempfile::tempdir().unwrap();
        // A chatty shell: profile output around the marked value; it gets `-lic <script>`.
        fake(bin.path(), "fakesh", &format!("echo 'welcome'\nprintf '{MARK}%s{MARK}' /opt/x/bin:/usr/bin\necho bye"));
        let shell = bin.path().join("fakesh").display().to_string();
        assert_eq!(login_shell_path(&shell, Duration::from_secs(5)).as_deref(), Some("/opt/x/bin:/usr/bin"));
        fake(bin.path(), "silent", "exit 0");
        assert_eq!(login_shell_path(&bin.path().join("silent").display().to_string(), Duration::from_secs(5)), None);
        fake(bin.path(), "slow", "sleep 30");
        let t0 = Instant::now();
        assert_eq!(login_shell_path(&bin.path().join("slow").display().to_string(), Duration::from_millis(300)), None);
        assert!(t0.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_real_login_shell_reports_its_path() {
        let got = login_shell_path("/bin/sh", Duration::from_secs(10)).expect("sh -lic prints PATH");
        assert!(got.contains("/usr/bin"), "{got}");
    }

    #[test]
    fn registry_terminates_live_groups() {
        let reg = Registry::default();
        // Ignores SIGTERM: needs the SIGKILL after the grace period.
        let mut stubborn = Command::new("/bin/sh").args(["-c", "trap '' TERM; sleep 30 & wait"]).process_group(0).spawn().unwrap();
        let mut polite = Command::new("/bin/sh").args(["-c", "sleep 30"]).process_group(0).spawn().unwrap();
        reg.add(stubborn.id() as i32);
        reg.add(polite.id() as i32);
        assert_eq!(reg.live().len(), 2);
        let t0 = Instant::now();
        reg.kill_all(Duration::from_millis(300));
        assert!(stubborn.wait().is_ok() && polite.wait().is_ok());
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
        assert!(reg.live().is_empty());
    }

    #[test]
    fn a_run_is_registered_while_it_lasts() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let handle = {
            let dir = dir.path().to_path_buf();
            // The shell leads its own group: its pid is the group id.
            std::thread::spawn(move || run("/bin/sh", &sh("echo $$ > pid; sleep 1"), "", &dir, None, Duration::from_secs(5)))
        };
        let t0 = Instant::now();
        let pgid = loop {
            if let Some(p) = std::fs::read_to_string(&pid_file).ok().and_then(|s| s.trim().parse::<i32>().ok()) {
                break p;
            }
            assert!(t0.elapsed() < Duration::from_secs(3), "the child never started");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(live_groups().contains(&pgid), "registered while running");
        handle.join().unwrap().unwrap();
        assert!(!live_groups().contains(&pgid), "removed when done");
    }

    #[test]
    fn messages_are_short_and_chinese() {
        assert_eq!(ProviderError::NotFound { program: "claude".into() }.message(), "未找到 claude（请在 [monitor] command 里填写完整路径）");
        assert_eq!(ProviderError::Timeout.message(), "超时");
        assert!(ProviderError::Auth("x".into()).message().starts_with("认证失败"));
        assert_eq!(ProviderError::Exited { code: Some(2), stderr_tail: "bad flag".into() }.message(), "退出码 2：bad flag");
    }

    #[test]
    fn messages_in_english() {
        use gilvt_i18n::{has_chinese, with_language, Language};
        let all = [
            ProviderError::NotFound { program: "claude".into() },
            ProviderError::Auth("x".into()),
            ProviderError::Timeout,
            ProviderError::Exited { code: Some(2), stderr_tail: "bad flag".into() },
            ProviderError::Exited { code: Some(2), stderr_tail: String::new() },
            ProviderError::Exited { code: None, stderr_tail: String::new() },
            ProviderError::Protocol("x".into()),
        ];
        let english: Vec<String> = with_language(Language::English, || all.iter().map(ProviderError::message).collect());
        assert!(english.iter().all(|m| !has_chinese(m)), "{english:?}");
        assert_eq!(english[0], "claude not found (set its full path in [monitor] command)");
        assert_eq!(english[3], "Exit code 2: bad flag");
        let unsupported = with_language(Language::English, || crate::chat::codex::check_features("").unwrap_err().message());
        assert!(!has_chinese(&unsupported), "{unsupported}");
    }
}
