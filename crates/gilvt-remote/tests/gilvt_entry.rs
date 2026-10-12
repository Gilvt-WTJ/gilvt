//! The `gilvt` name of gilvt-remote (spec R2a §3.2): `hook` forwards to $GILVT_SOCKET with the link, other
//! commands are refused with exit 1, unknown hook usage stays silent.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// `accept` with a deadline: a regression must fail the test, not hang `cargo test`.
fn accept_within(listener: &UnixListener, within: Duration) -> UnixStream {
    listener.set_nonblocking(true).unwrap();
    let end = Instant::now() + within;
    loop {
        match listener.accept() {
            Ok((conn, _)) => {
                // The accepted socket inherits non-blocking on macOS. The hook may already have sent and exited, and
                // macOS then refuses these calls with EINVAL on the half-closed socket: its data is still readable.
                let _ = conn.set_nonblocking(false);
                let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
                return conn;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < end => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => panic!("no hook arrived within {within:?}: {e}"),
        }
    }
}

fn install(home: &std::path::Path) -> std::path::PathBuf {
    let dir = home.join(".gilvt-server/0.1.0-aaaaaaaa");
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_gilvt-remote"), dir.join("gilvt-remote")).unwrap();
    std::os::unix::fs::symlink("../gilvt-remote", dir.join("bin/gilvt")).unwrap();
    dir.join("bin/gilvt")
}

#[test]
fn hook_forwards_with_the_link_and_exits_0() {
    let home = tempfile::tempdir().unwrap();
    let gilvt = install(home.path());
    let sock = home.path().join("g.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let mut child = Command::new(&gilvt)
        .args(["hook", "claude", "Stop"])
        .env("HOME", home.path())
        .env("GILVT_SOCKET", &sock)
        .env("GILVT_LINK", "i-7")
        .env_remove("TMUX")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(br#"{"session_id":"s1","hook_event_name":"Stop"}"#).unwrap();
    let conn = accept_within(&listener, Duration::from_secs(10));
    let mut line = String::new();
    BufReader::new(conn).read_line(&mut line).unwrap();
    let req: gilvt_ipc::Request = serde_json::from_str(&line).unwrap();
    assert!(matches!(req, gilvt_ipc::Request::Hook { link: Some(ref l), tmux: false, .. } if l == "i-7"), "{req:?}");
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success() && out.stdout.is_empty());
}

#[test]
fn other_commands_are_refused_and_unknown_hook_usage_is_silent() {
    let home = tempfile::tempdir().unwrap();
    let gilvt = install(home.path());
    let out = Command::new(&gilvt).arg("ssh").env("HOME", home.path()).env("GILVT_LANGUAGE", "zh-CN").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("远端不支持这个命令"), "{out:?}");
    let out = Command::new(&gilvt).args(["hook", "nonsense"]).env("HOME", home.path()).stdin(Stdio::null()).output().unwrap();
    assert!(out.status.success() && out.stdout.is_empty() && out.stderr.is_empty());
}
