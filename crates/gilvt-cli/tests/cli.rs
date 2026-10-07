//! Runs the built `gilvt` binary: talking to a test socket, and printing when outside gilvt.

use std::io::Write;
use std::process::{Command, Stdio};

use gilvt_ipc::{Request, Server};

fn gilvt() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_gilvt"));
    c.env_remove("GILVT_SOCKET").env_remove("GILVT_PANE_ID");
    c
}

#[test]
fn view_sends_absolute_path_and_location_to_the_app() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
    let sock = dir.path().join("t.sock");
    let (tx, rx) = async_channel::unbounded();
    let _server = Server::start(&sock, tx).unwrap();

    let out = gilvt().current_dir(dir.path()).env("GILVT_SOCKET", &sock).env("GILVT_PANE_ID", "4").args(["view", "a.rs:3:2"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    match rx.try_recv().unwrap() {
        Request::View { pane, path, line, col, pin, .. } => {
            assert_eq!(pane, Some(4));
            assert_eq!(path.unwrap().canonicalize().unwrap(), dir.path().join("a.rs").canonicalize().unwrap());
            assert_eq!((line, col, pin), (Some(3), Some(2), false));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn stdin_content_is_sent_with_its_type() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("t.sock");
    let (tx, rx) = async_channel::unbounded();
    let _server = Server::start(&sock, tx).unwrap();
    let mut child = gilvt().env("GILVT_SOCKET", &sock).args(["view", "--pin", "--as", "md", "-"]).stdin(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"# title\n").unwrap();
    assert!(child.wait().unwrap().success());
    match rx.try_recv().unwrap() {
        Request::View { content, as_type, pin, path, .. } => {
            assert_eq!(content.as_deref(), Some("# title\n"));
            assert_eq!(as_type.as_deref(), Some("md"));
            assert!(pin);
            assert!(path.is_none());
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn prints_highlighted_file_outside_gilvt() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
    let out = gilvt().current_dir(dir.path()).args(["view", "a.rs"]).output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("\x1b[38;2;"), "24-bit colors: {s:?}");
    assert!(s.contains("fn"));
}

#[test]
fn errors_and_usage() {
    let out = gilvt().args(["view", "/definitely/not/here.rs"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no such file"));
    let out = gilvt().args(["bogus"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage:"));
}

#[test]
fn unreachable_socket_falls_back_to_printing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
    let out = gilvt().current_dir(dir.path()).env("GILVT_SOCKET", dir.path().join("gone.sock")).args(["view", "a.txt"]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("hello"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("printing instead"));
}

#[test]
fn diff_reports_a_missing_repository_or_revision() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("t.sock");
    let (tx, rx) = async_channel::unbounded();
    let _server = Server::start(&sock, tx).unwrap();

    let out = gilvt().current_dir(dir.path()).env("GILVT_SOCKET", &sock).args(["diff"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not inside a git repository"));

    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    for args in [&["init", "-q"][..], &["-c", "user.email=t@example.com", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]] {
        assert!(Command::new("git").arg("-C").arg(&repo).args(args).status().unwrap().success());
    }
    let out = gilvt().current_dir(&repo).env("GILVT_SOCKET", &sock).args(["diff", "no-such-rev"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown revision: no-such-rev"));
    assert!(rx.try_recv().is_err(), "nothing is sent to the app");

    let out = gilvt().current_dir(&repo).env("GILVT_SOCKET", &sock).args(["diff"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(matches!(rx.try_recv().unwrap(), Request::Diff { rev: None, .. }));
}
