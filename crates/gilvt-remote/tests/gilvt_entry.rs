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

#[test]
fn view_without_a_link_says_so_and_exits_1() {
    let home = tempfile::tempdir().unwrap();
    let gilvt = install(home.path());
    std::fs::write(home.path().join("a.txt"), "x").unwrap();
    let out = Command::new(&gilvt).args(["view", "a.txt"]).current_dir(home.path()).env("HOME", home.path()).env("GILVT_LANGUAGE", "zh-CN").env_remove("GILVT_SOCKET").env_remove("GILVT_LINK").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("这个 shell 没有连着 gilvt"), "{out:?}");
}

#[test]
fn view_checks_the_remote_file_and_sends_the_link() {
    let home = tempfile::tempdir().unwrap();
    let gilvt = install(home.path());
    std::fs::write(home.path().join("a.txt"), "x").unwrap();
    let sock = home.path().join("g.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let missing = Command::new(&gilvt).args(["view", "nope.txt"]).current_dir(home.path()).env("HOME", home.path()).env("GILVT_SOCKET", &sock).env("GILVT_LINK", "i-3").output().unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("nope.txt: no such file"));
    let child = Command::new(&gilvt).args(["view", "a.txt:2"]).current_dir(home.path()).env("HOME", home.path()).env("GILVT_SOCKET", &sock).env("GILVT_LINK", "i-3").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut conn = accept_within(&listener, Duration::from_secs(10));
    let mut line = String::new();
    BufReader::new(conn.try_clone().unwrap()).read_line(&mut line).unwrap();
    let req: gilvt_ipc::Request = serde_json::from_str(&line).unwrap();
    let want_path = home.path().canonicalize().unwrap().join("a.txt");
    assert!(matches!(req, gilvt_ipc::Request::View { ref path, line: Some(2), link: Some(ref l), .. } if l == "i-3" && path.as_deref().map(|p| p.canonicalize().unwrap()) == Some(want_path.clone())), "{req:?}");
    conn.write_all(b"{\"type\":\"ok\"}\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success() && out.stdout.is_empty(), "{out:?}");
}

#[test]
fn view_help_prints_usage_even_outside_a_linked_shell() {
    let home = tempfile::tempdir().unwrap();
    let gilvt = install(home.path());
    for args in [&["view", "--help"][..], &["diff", "-h"][..]] {
        let out = Command::new(&gilvt).args(args).env_remove("GILVT_SOCKET").env_remove("GILVT_LINK").output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stdout).contains("usage:"), "{args:?}");
    }
}
