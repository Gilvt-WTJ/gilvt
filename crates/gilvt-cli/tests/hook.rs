//! Runs the built `gilvt hook …`: silent forwarding to a test socket, and the argv it generates
//! for claude / codex (against a fake codex; the real agents are never run).

use std::io::Write;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use gilvt_ipc::{Request, Server};
use serde_json::{json, Value};

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        for d in ["home", "tmp", "bin"] {
            std::fs::create_dir(dir.path().join(d)).unwrap();
        }
        Env { dir }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    /// `gilvt` with HOME / TMPDIR inside the temp dir and no gilvt environment.
    fn gilvt(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_gilvt"));
        c.env_remove("GILVT_SOCKET").env_remove("GILVT_PANE_ID");
        c.env("HOME", self.path("home")).env("TMPDIR", self.path("tmp"));
        c.env("PATH", format!("{}:/usr/bin:/bin", self.path("bin").display()));
        c
    }

    fn state_file(&self) -> PathBuf {
        self.path("home/Library/Application Support/gilvt/state/codex-trust.json")
    }
}

fn gilvt_path() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_gilvt")).canonicalize().unwrap()
}

/// The first exec of a freshly built binary is slowed by macOS's code scan; time later ones.
fn warm_up() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = Command::new(env!("CARGO_BIN_EXE_gilvt")).args(["hook"]).stdin(Stdio::null()).output();
    });
}

/// Runs `cmd` with `stdin` as input.
fn run(mut cmd: Command, stdin: &[u8]) -> (Output, Duration) {
    let started = Instant::now();
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let _ = input.write_all(stdin);
    drop(input);
    let out = child.wait_with_output().unwrap();
    (out, started.elapsed())
}

fn assert_silent(out: &Output) {
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty(), "stdout: {:?}", String::from_utf8_lossy(&out.stdout));
    assert!(out.stderr.is_empty(), "stderr: {:?}", String::from_utf8_lossy(&out.stderr));
}

fn split_nul(bytes: &[u8]) -> Vec<String> {
    let s = String::from_utf8(bytes.to_vec()).unwrap();
    let body = s.strip_suffix('\0').unwrap_or_else(|| panic!("NUL-terminated: {s:?}"));
    body.split('\0').map(str::to_string).collect()
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

const PAYLOAD: &str = r#"{"session_id":"11111111-2222-4333-8444-555555555503","transcript_path":"/Users/u/.claude/projects/-p/x.jsonl","cwd":"/p","hook_event_name":"Stop"}"#;

#[test]
fn hook_delivers_payload_and_pane_silently_and_fast() {
    warm_up();
    let env = Env::new();
    let sock = env.path("t.sock");
    let (tx, rx) = async_channel::unbounded();
    let _server = Server::start(&sock, tx).unwrap();
    let mut cmd = env.gilvt();
    cmd.env("GILVT_SOCKET", &sock).env("GILVT_PANE_ID", "42").args(["hook", "claude", "Stop"]);
    let (out, took) = run(cmd, PAYLOAD.as_bytes());
    assert_silent(&out);
    // Waiting for a reply would take the 5 s read timeout; the margin absorbs a loaded machine.
    assert!(took < Duration::from_secs(1), "took {took:?}");
    match rx.recv_blocking().unwrap() {
        Request::Hook { pane, agent, event, payload } => {
            assert_eq!((pane, agent.as_str(), event.as_str()), (Some(42), "claude", "Stop"));
            assert_eq!(payload, serde_json::from_str::<Value>(PAYLOAD).unwrap());
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn hook_is_silent_without_a_reachable_app() {
    warm_up();
    let env = Env::new();
    // No GILVT_SOCKET, a missing socket, and a socket file nobody listens on.
    let dead = env.path("dead.sock");
    drop(UnixListener::bind(&dead).unwrap());
    for sock in [None, Some(env.path("missing.sock")), Some(dead)] {
        let mut cmd = env.gilvt();
        if let Some(s) = &sock {
            cmd.env("GILVT_SOCKET", s);
        }
        cmd.args(["hook", "codex", "PreToolUse"]);
        let (out, took) = run(cmd, PAYLOAD.as_bytes());
        assert_silent(&out);
        assert!(took < Duration::from_secs(1), "{sock:?} took {took:?}");
    }
}

#[test]
fn hook_is_silent_on_bad_input_and_usage() {
    let env = Env::new();
    let sock = env.path("t.sock");
    let (tx, rx) = async_channel::unbounded();
    let _server = Server::start(&sock, tx).unwrap();
    let cases: [(&[&str], &[u8]); 5] = [
        (&["hook", "claude", "Stop"], b"not json"),
        (&["hook", "claude", "Stop"], b"[1,2]"),
        (&["hook", "claude"], PAYLOAD.as_bytes()),
        (&["hook", "claude", "Bad Event"], PAYLOAD.as_bytes()),
        (&["hook", "nope", "x"], PAYLOAD.as_bytes()),
    ];
    for (args, input) in cases {
        let mut cmd = env.gilvt();
        cmd.env("GILVT_SOCKET", &sock).args(args);
        assert_silent(&run(cmd, input).0);
    }
    std::thread::sleep(Duration::from_millis(50));
    assert!(rx.try_recv().is_err(), "nothing forwarded");
}

#[test]
fn oversized_payloads_are_reduced_to_their_identity() {
    let env = Env::new();
    let sock = env.path("t.sock");
    let (tx, rx) = async_channel::unbounded();
    let _server = Server::start(&sock, tx).unwrap();
    let big = format!(r#"{{"session_id":"s-9","cwd":"/p","hook_event_name":"PostToolUse","tool_name":"Read","tool_response":"{}"}}"#, "x".repeat(3 << 20));
    let mut cmd = env.gilvt();
    cmd.env("GILVT_SOCKET", &sock).args(["hook", "claude", "PostToolUse"]);
    let (out, _) = run(cmd, big.as_bytes());
    assert_silent(&out);
    match rx.recv_blocking().unwrap() {
        Request::Hook { payload, .. } => {
            assert_eq!(payload, json!({"session_id": "s-9", "cwd": "/p", "hook_event_name": "PostToolUse", "tool_name": "Read", "gilvt_truncated": true}))
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn an_app_that_never_reads_does_not_stall_the_agent() {
    let env = Env::new();
    let sock = env.path("stuck.sock");
    let _listener = UnixListener::bind(&sock).unwrap(); // never accepts
    let big = format!(r#"{{"session_id":"s","x":"{}"}}"#, "y".repeat(900 << 10));
    let mut cmd = env.gilvt();
    cmd.env("GILVT_SOCKET", &sock).args(["hook", "claude", "PostToolUse"]);
    let (out, took) = run(cmd, big.as_bytes());
    assert_silent(&out);
    // Gives up after NOWAIT_TIMEOUT (200 ms) instead of the 5 s write timeout.
    assert!(took < Duration::from_secs(2), "took {took:?}");
}

#[test]
fn claude_args_add_settings_and_merge_the_users() {
    let env = Env::new();
    let run_args = |user: &[&str]| -> Vec<String> {
        let mut cmd = env.gilvt();
        cmd.current_dir(env.path("home")).args(["hook", "claude-args", "--"]).args(user);
        let (out, _) = run(cmd, b"");
        assert!(out.status.success());
        assert!(out.stderr.is_empty());
        split_nul(&out.stdout)
    };
    let read = |p: &str| serde_json::from_str::<Value>(&std::fs::read_to_string(p).unwrap()).unwrap();
    let ours = format!("{} hook claude Stop", gilvt_path().display());

    let out = run_args(&["-p", "multi\nline 'q' \"dq\"", "", "--", "--settings"]);
    assert_eq!(out[0], "--settings");
    assert_eq!(&out[2..], &strs(&["-p", "multi\nline 'q' \"dq\"", "", "--", "--settings"])[..]);
    assert!(out[1].starts_with(env.path("tmp").canonicalize().unwrap().to_str().unwrap()) || out[1].starts_with(env.path("tmp").to_str().unwrap()));
    let hooks = read(&out[1]);
    assert_eq!(hooks["hooks"]["Stop"][0]["hooks"][0]["command"], ours);
    assert_eq!(hooks["hooks"].as_object().unwrap().len(), 15);

    std::fs::write(env.path("home/mine.json"), r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"say hi"}]}]}}"#).unwrap();
    for user in [&["--settings", "mine.json", "hi"][..], &["--settings=mine.json", "hi"][..]] {
        let out = run_args(user);
        let (flag, file) = match out[0].strip_prefix("--settings=") {
            Some(f) => (1, f.to_string()),
            None => (2, out[1].clone()),
        };
        assert_eq!(out[flag..], strs(&["hi"])[..]);
        let merged = read(&file);
        assert_eq!(merged["model"], "opus");
        assert_eq!(merged["hooks"]["Stop"][0]["hooks"][0]["command"], "say hi");
        assert_eq!(merged["hooks"]["Stop"][1]["hooks"][0]["command"], ours);
    }
    // Inline JSON, the last of several.
    let out = run_args(&["--settings", "old.json", "--settings", r#"{"env":{"A":"1"}}"#]);
    assert_eq!(&out[..2], &strs(&["--settings", "old.json"])[..]);
    let merged = read(&out[3]);
    assert_eq!(merged["env"]["A"], "1");
    assert_eq!(merged["hooks"]["SessionStart"][0]["hooks"][0]["command"], format!("{} hook claude SessionStart", gilvt_path().display()));
    // Unreadable user settings: left for Claude to report.
    assert_eq!(run_args(&["--settings", "missing.json"]), strs(&["--settings", "missing.json"]));
}

/// A fake `codex`: `--version`, and an `app-server` that answers initialize + hooks/list with
/// `response.json` (after `delay` seconds) and records its argv.
fn install_fake_codex(env: &Env, delay: &str) {
    use std::os::unix::fs::PermissionsExt;
    let gilvt = gilvt_path();
    let hooks: Vec<Value> = ["SessionStart", "PreToolUse", "Stop"]
        .iter()
        .map(|e| {
            let snake = match *e {
                "SessionStart" => "session_start",
                "PreToolUse" => "pre_tool_use",
                _ => "stop",
            };
            json!({"key": format!("/<session-flags>/config.toml:{snake}:0:0"), "source": "sessionFlags", "command": format!("{} hook codex {e}", gilvt.display()), "currentHash": format!("sha256:{snake}")})
        })
        .chain([json!({"key": "/Users/u/.codex/hooks.json:stop:0:0", "source": "user", "command": "other", "currentHash": "sha256:u"})])
        .collect();
    let response = json!({"id": 2, "result": {"data": [{"cwd": "/w", "hooks": hooks}]}});
    std::fs::write(env.path("response.json"), response.to_string()).unwrap();
    let script = format!(
        r#"#!/bin/bash
if [ "$1" = "--version" ]; then echo "codex-cli 9.9.9"; exit 0; fi
printf '%s\0' "$@" > '{argv}'
sleep {delay}
while IFS= read -r line; do
  case "$line" in
    *'"id":1,'*) echo '{{"id":1,"result":{{}}}}' ;;
    *'"id":2,'*) cat '{resp}'; echo ;;
  esac
done
"#,
        argv = env.path("app-server.argv").display(),
        resp = env.path("response.json").display(),
    );
    let bin = env.path("bin/codex");
    std::fs::write(&bin, script).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn codex_args(env: &Env, user: &[&str]) -> (Vec<String>, Duration) {
    let mut cmd = env.gilvt();
    cmd.args(["hook", "codex-args", "--"]).args(user);
    let (out, took) = run(cmd, b"");
    assert!(out.status.success());
    (split_nul(&out.stdout), took)
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(Instant::now() < deadline, "timed out waiting for {}", path.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn codex_args_learn_trust_in_the_background_then_add_hooks() {
    warm_up();
    let env = Env::new();
    install_fake_codex(&env, "2");
    let user = ["exec", "a b\nc", ""];
    // Cache miss: the user's args at once, while the trust job runs detached.
    let (out, took) = codex_args(&env, &user);
    assert_eq!(out, strs(&user));
    // The fake codex answers after 2 s.
    assert!(took < Duration::from_secs(1), "never waits for codex: {took:?}");
    wait_for(&env.state_file());

    let argv = split_nul(&std::fs::read(env.path("app-server.argv")).unwrap());
    assert_eq!(argv.last().unwrap(), "app-server");
    assert_eq!(argv.len(), 19, "nine -c hooks.<Event> flags: {argv:?}");
    assert!(argv[1].starts_with("hooks.SessionStart=[{hooks=[{type=\"command\",command="));

    let cache: Value = serde_json::from_str(&std::fs::read_to_string(env.state_file()).unwrap()).unwrap();
    assert_eq!(cache["entries"][0]["codex_version"], "codex-cli 9.9.9");
    assert_eq!(cache["entries"][0]["gilvt"], gilvt_path().display().to_string());

    let (out, _) = codex_args(&env, &user);
    let g = gilvt_path().display().to_string();
    assert_eq!(out, [
        "-c".to_string(),
        format!("hooks.SessionStart=[{{hooks=[{{type=\"command\",command=\"{g} hook codex SessionStart\"}}]}}]"),
        "-c".into(),
        format!("hooks.PreToolUse=[{{hooks=[{{type=\"command\",command=\"{g} hook codex PreToolUse\"}}]}}]"),
        "-c".into(),
        format!("hooks.Stop=[{{hooks=[{{type=\"command\",command=\"{g} hook codex Stop\"}}]}}]"),
        "-c".into(),
        r#"hooks.state={"/<session-flags>/config.toml:pre_tool_use:0:0"={trusted_hash="sha256:pre_tool_use"},"/<session-flags>/config.toml:session_start:0:0"={trusted_hash="sha256:session_start"},"/<session-flags>/config.toml:stop:0:0"={trusted_hash="sha256:stop"}}"#.into(),
        "exec".into(),
        "a b\nc".into(),
        "".into(),
    ]);
    // The user's own Stop groups come first in one merged flag; gilvt's trust key moves to their index.
    let user_stop = r#"hooks.Stop=[{hooks=[{type="command",command="say done"}]}]"#;
    let (out, _) = codex_args(&env, &["-c", user_stop, "exec"]);
    assert_eq!(out.len(), 2 * 4 + 1);
    assert_eq!(out[5], format!("hooks.Stop=[{{hooks=[{{command=\"say done\",type=\"command\"}}]}},{{hooks=[{{type=\"command\",command=\"{g} hook codex Stop\"}}]}}]"));
    assert!(out[7].contains(r#""/<session-flags>/config.toml:stop:1:0"={trusted_hash="sha256:stop"}"#), "{}", out[7]);
    assert_eq!(out[8], "exec");
    // A value gilvt cannot read stays; gilvt skips (and does not trust) that event.
    let (out, _) = codex_args(&env, &["-c", "hooks.Stop=oops", "exec"]);
    assert_eq!(out.len(), 4 + 1 + 1 + 3);
    assert!(!out[5].contains(":stop:"));
    assert_eq!(&out[out.len() - 3..], &strs(&["-c", "hooks.Stop=oops", "exec"])[..]);
}

#[test]
fn codex_args_without_codex_or_with_a_failing_one() {
    let env = Env::new();
    assert_eq!(codex_args(&env, &["hi"]).0, strs(&["hi"]), "no codex on PATH");
    assert!(!env.path("home/Library").exists(), "nothing written");

    use std::os::unix::fs::PermissionsExt;
    std::fs::write(env.path("bin/codex"), "#!/bin/sh\nexit 3\n").unwrap();
    std::fs::set_permissions(env.path("bin/codex"), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(codex_args(&env, &["hi"]).0, strs(&["hi"]));
    wait_for(&env.state_file());
    let deadline = Instant::now() + Duration::from_secs(10);
    let lock = env.path("home/Library/Application Support/gilvt/state/codex-trust.lock");
    while lock.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let cache: Value = serde_json::from_str(&std::fs::read_to_string(env.state_file()).unwrap()).unwrap();
    assert!(cache["failures"][0]["error"].as_str().unwrap().contains("--version"), "{cache}");
    assert!(cache["entries"].as_array().unwrap().is_empty());
    assert_eq!(codex_args(&env, &["hi"]).0, strs(&["hi"]), "backs off; still the user's args");
}
