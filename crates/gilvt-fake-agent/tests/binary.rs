//! The real binary: personas, the process name gilvt sees, stdin input, exit codes, the app-server.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_gilvt-fake-agent");

/// `<dir>/<name>` as a copy of the binary (what the sandbox installs), or a symlink.
///
/// Never a hard link: every link shares the binary's vnode, and when the parent is in the provenance
/// sandbox, Gatekeeper (syspolicyd) assesses a new executable path by reading the file at the vnode's cached
/// path, which can be another link (another test's, or a deleted one). That read fails with ENOENT and
/// syspolicyd SIGKILLs the child before it prints anything. `fs::copy` clones on APFS: a fresh inode, cheap.
fn install(dir: &Path, name: &str, symlink: bool) -> PathBuf {
    let path = dir.join(name);
    if symlink {
        std::os::unix::fs::symlink(BIN, &path).unwrap();
    } else {
        std::fs::copy(BIN, &path).unwrap();
    }
    path
}

/// A command with a clean environment: HOME, PATH and the fake's variables only.
fn cmd(program: &Path, home: &Path, cwd: &Path) -> Command {
    let mut c = Command::new(program);
    c.env_clear().env("HOME", home).env("PATH", "/usr/bin:/bin").current_dir(cwd);
    c
}

fn run_with_input(mut c: Command, input: &str) -> Output {
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn versions_by_persona() {
    let s = common::Setup::new();
    let bin = s.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let out = cmd(&install(&bin, "claude", false), &s.home, &s.cwd).arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{} (Claude Code)\n", gilvt_fake_agent::CLAUDE_VERSION));
    let out = cmd(&install(&bin, "codex", false), &s.home, &s.cwd).arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("codex-cli {}\n", gilvt_fake_agent::CODEX_VERSION));
    let out = cmd(Path::new(BIN), &s.home, &s.cwd).env("GILVT_FAKE_AGENT", "codex").arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("codex-cli"));
    let out = cmd(Path::new(BIN), &s.home, &s.cwd).arg("--version").output().unwrap();
    assert_eq!(out.status.code(), Some(3), "no persona");
    assert!(String::from_utf8_lossy(&out.stderr).contains("GILVT_FAKE_AGENT"));
}

/// The kernel's name for `pid`, via `proc_name` like gilvt's foreground detection.
fn proc_name(pid: u32) -> String {
    let mut buf = [0u8; 256];
    // SAFETY: a valid buffer of the given length.
    let n = unsafe { libc::proc_name(pid as libc::c_int, buf.as_mut_ptr().cast(), buf.len() as u32) };
    String::from_utf8_lossy(&buf[..n.max(0) as usize]).into_owned()
}

/// Starts `program` sleeping in a scenario and returns its kernel name.
fn running_name(program: &Path, s: &common::Setup) -> String {
    let scenario = s.dir.path().join("sleep.toml");
    std::fs::write(&scenario, "[[step]]\nsleep = \"5s\"").unwrap();
    let mut child = cmd(program, &s.home, &s.cwd)
        .env("GILVT_FAKE_SCENARIO", &scenario)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let name = proc_name(child.id());
    let _ = child.kill();
    let _ = child.wait();
    name
}

#[test]
fn a_copy_named_claude_is_seen_as_claude() {
    let s = common::Setup::new();
    let bin = s.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let claude = running_name(&install(&bin, "claude", false), &s);
    assert_eq!(claude, "claude");
    assert_eq!(gilvt_agent::agent_of_process(&claude, None), Some(gilvt_agent::AgentKind::Claude));
    let codex = running_name(&install(&bin, "codex", false), &s);
    assert_eq!(gilvt_agent::agent_of_process(&codex, None), Some(gilvt_agent::AgentKind::Codex));
    // A symlink is not enough: macOS names the process after the file the link resolves to.
    let linked = s.dir.path().join("links");
    std::fs::create_dir_all(&linked).unwrap();
    let via_symlink = running_name(&install(&linked, "claude", true), &s);
    assert_eq!(via_symlink, "gilvt-fake-agent");
    assert_eq!(gilvt_agent::agent_of_process(&via_symlink, None), None);
}

#[test]
fn a_bad_scenario_exits_3_before_writing_anything() {
    let s = common::Setup::new();
    let scenario = s.dir.path().join("bad.toml");
    std::fs::write(&scenario, "[[step]]\ntool = \"Bash\"\ncolour = \"red\"").unwrap();
    let out = cmd(Path::new(BIN), &s.home, &s.cwd).env("GILVT_FAKE_AGENT", "claude").env("GILVT_FAKE_SCENARIO", &scenario).output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown key `colour` for a `tool` step"));
    assert!(out.stdout.is_empty());
    assert_eq!(std::fs::read_dir(&s.home).unwrap().count(), 0, "nothing under HOME");
    let wrong = cmd(Path::new(BIN), &s.home, &s.cwd).env("GILVT_FAKE_AGENT", "claude").arg("@scenario:codex-basic").output().unwrap();
    assert_eq!(wrong.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("is for codex"));
}

#[test]
fn claude_end_to_end_over_a_pipe() {
    let s = common::Setup::new();
    let bin = s.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let settings = s.claude_settings();
    let mut c = cmd(&install(&bin, "claude", false), &s.home, &s.cwd);
    c.args(["--settings", settings.to_str().unwrap(), "--permission-mode", "default", "@scenario:approve-bash"]);
    let out = run_with_input(c, "1");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let screen = String::from_utf8_lossy(&out.stdout);
    assert!(screen.contains("❯ Clean the build directory") && screen.contains("⏺ Removed build/."), "{screen}");
    let hooks: Vec<Value> = std::fs::read_to_string(&s.hooks_file).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let events: Vec<&str> = hooks.iter().map(|h| h["hook_event_name"].as_str().unwrap()).collect();
    assert_eq!(events.first(), Some(&"SessionStart"));
    assert_eq!(events.last(), Some(&"SessionEnd"));
    assert!(events.contains(&"PermissionRequest") && events.contains(&"Stop"));
    let id = hooks[0]["session_id"].as_str().unwrap();
    let transcript = gilvt_fake_agent::claude::transcript_path(&s.home, &s.cwd, id);
    assert_eq!(hooks[0]["transcript_path"], transcript.to_str().unwrap());
    assert!(transcript.is_file());
    assert!(screen.contains(&format!("claude --resume {id}")));
    assert_eq!(hooks[0]["cwd"], s.cwd.to_str().unwrap());
}

/// H17 (first GUI run): `claude @scenario:default h17 一轮就结束` typed unquoted in a shell passes three words; the
/// whole text after the scenario token is the first prompt, not just the first word.
#[test]
fn unquoted_prompt_words_all_reach_the_first_turn() {
    let s = common::Setup::new();
    let bin = s.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for name in ["claude", "codex"] {
        let mut c = cmd(&install(&bin, name, false), &s.home, &s.cwd);
        c.args(["@scenario:default", "h17", "一轮就结束"]);
        let out = run_with_input(c, "");
        assert_eq!(out.status.code(), Some(0), "{name}: {}", String::from_utf8_lossy(&out.stderr));
        let screen = String::from_utf8_lossy(&out.stdout);
        assert!(screen.contains("❯ h17 一轮就结束") && screen.contains("⏺ OK: h17 一轮就结束"), "{name}: {screen}");
    }
}

/// I18 (second GUI run): a lite session (no hooks) is found by gilvt as the newest transcript of its cwd
/// modified at or after `first seen − 2 s` (gilvt-app `agents/foreground.rs`: SINCE_SLACK), where "first seen"
/// is the foreground poll that saw the agent (every second or more, later when the app is busy). A real Claude
/// turn takes seconds, so its transcript keeps being written after that point; the fake's default turn used to
/// end within milliseconds of the start, and a poll 2.5 s later never found it. The default scenario now
/// thinks for 2 s before it replies, like a real turn.
#[test]
fn a_default_turn_is_still_being_written_when_a_late_poll_looks_for_it() {
    let s = common::Setup::new();
    let bin = s.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cwd = s.cwd.canonicalize().unwrap();
    let mut c = cmd(&install(&bin, "claude", false), &s.home, &cwd);
    c.args(["@scenario:default", "i18"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    let started = std::time::SystemTime::now();
    let mut child = c.spawn().unwrap();
    let dir = s.home.join(".claude/projects").join(gilvt_fake_agent::claude::project_dir_name(&cwd));
    let done = |_: ()| {
        std::fs::read_dir(&dir).ok()?.flatten().find_map(|e| {
            std::fs::read_to_string(e.path()).ok().filter(|t| t.contains("turn_duration")).map(|_| e.path())
        })
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let transcript = loop {
        if let Some(p) = done(()) {
            break p;
        }
        assert!(std::time::Instant::now() < deadline, "the first turn never ended");
        std::thread::sleep(Duration::from_millis(50));
    };
    child.kill().unwrap();
    child.wait().unwrap();
    let first_seen = started + Duration::from_millis(2500);
    let since = first_seen - Duration::from_secs(2);
    let found = gilvt_agent::newest_claude_transcript(&s.home, &cwd, since);
    assert_eq!(found.map(|(_, p)| p), Some(transcript), "a poll 2.5 s after the start still finds the session");
}

#[test]
fn codex_app_server_answers_the_trust_query() {
    let s = common::Setup::new();
    let bin = s.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let mut c = cmd(&install(&bin, "codex", false), &s.home, &s.cwd);
    let stop = "hooks.Stop=[{hooks=[{type=\"command\",command=\"/g/gilvt hook codex Stop\"}]}]";
    c.args(["-c", stop, "app-server"]).stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    writeln!(stdin, "{}", json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "gilvt"}}})).unwrap();
    let init: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(init["id"], 1);
    writeln!(stdin, "{}", json!({"method": "initialized"})).unwrap();
    writeln!(stdin, "{}", json!({"id": 2, "method": "hooks/list", "params": {"cwds": ["/x"]}})).unwrap();
    let list: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    let hook = &list["result"]["data"][0]["hooks"][0];
    assert_eq!((hook["source"].as_str(), hook["command"].as_str()), (Some("sessionFlags"), Some("/g/gilvt hook codex Stop")));
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn codex_resume_appends_to_the_rollout() {
    let s = common::Setup::new();
    let bin = s.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let codex = install(&bin, "codex", false);
    let mut first = cmd(&codex, &s.home, &s.cwd);
    first.args(["-s", "read-only", "-a", "on-request", "hello"]);
    first.args(s.codex_config().iter().flat_map(|f| ["-c".to_string(), f.clone()]));
    let out = run_with_input(first, "");
    assert_eq!(out.status.code(), Some(0));
    let hooks: Vec<Value> = std::fs::read_to_string(&s.hooks_file).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let id = hooks[0]["session_id"].as_str().unwrap().to_string();
    let rollout = PathBuf::from(hooks[0]["transcript_path"].as_str().unwrap());
    let before = std::fs::read_to_string(&rollout).unwrap().lines().count();
    let mut again = cmd(&codex, &s.home, &s.cwd);
    again.args(["resume", &id, "more"]);
    let out = run_with_input(again, "");
    assert_eq!(out.status.code(), Some(0));
    let text = std::fs::read_to_string(&rollout).unwrap();
    assert!(text.lines().count() > before);
    assert_eq!(text.matches("\"session_meta\"").count(), 1, "one session_meta");
    let entry = gilvt_agent::parse_codex_rollout(&rollout).expect("listed in the history");
    assert_eq!((entry.session_id.as_str(), entry.first_prompt.as_str()), (id.as_str(), "hello"));
}

#[test]
fn sigterm_ends_the_session_cleanly() {
    let s = common::Setup::new();
    let bin = s.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let settings = s.claude_settings();
    let mut c = cmd(&install(&bin, "claude", false), &s.home, &s.cwd);
    c.args(["--settings", settings.to_str().unwrap(), "waiting"]).stdin(Stdio::piped()).stdout(Stdio::null());
    let mut child = c.spawn().unwrap();
    let _stdin = child.stdin.take();
    // The default scenario answers, then idles at the prompt (stdin stays open).
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !std::fs::read_to_string(&s.hooks_file).unwrap_or_default().contains("\"Stop\"") {
        assert!(std::time::Instant::now() < deadline, "no Stop hook");
        std::thread::sleep(Duration::from_millis(50));
    }
    // SAFETY: signalling our own child.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(128 + libc::SIGTERM));
    let hooks = std::fs::read_to_string(&s.hooks_file).unwrap();
    let last: Value = serde_json::from_str(hooks.lines().filter(|l| !l.is_empty()).last().unwrap()).unwrap();
    assert_eq!((last["hook_event_name"].as_str(), last["reason"].as_str()), (Some("SessionEnd"), Some("other")));
}

#[test]
fn codex_app_server_lists_models() {
    let s = common::Setup::new();
    let bin = s.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let mut c = cmd(&install(&bin, "codex", false), &s.home, &s.cwd);
    // The global flags gilvt puts before the subcommand (models.rs `app_server_args`).
    c.args(["--disable", "hooks", "-c", "mcp_servers={}", "app-server"]).stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", json!({"id": 1, "method": "initialize", "params": {}})).unwrap();
    writeln!(stdin, "{}", json!({"id": 2, "method": "model/list", "params": {"cursor": null}})).unwrap();
    writeln!(stdin, "{}", json!({"id": 3, "method": "model/list", "params": {"cursor": "2"}})).unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("fake-codex-large") && text.contains("\"nextCursor\":\"2\"") && text.contains("fake-codex-small"), "{text}");
}
