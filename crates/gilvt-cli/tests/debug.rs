//! Runs the built `gilvt debug …` against an in-process socket whose test "app" answers state queries.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gilvt_ipc::{Query, Request, Response, Server};
use serde_json::{json, Value};

fn gilvt() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_gilvt"));
    c.env_remove("GILVT_SOCKET").env_remove("GILVT_PANE_ID");
    c
}

/// Serves `sock` on this thread's behalf: each query is answered by `answer(tail_lines)`.
fn serve(sock: &Path, answer: impl Fn(u16) -> Response + Send + 'static) -> Server {
    let (tx, _rx) = async_channel::unbounded();
    let (qtx, qrx) = async_channel::unbounded::<Query>();
    let server = Server::start_with_queries(sock, tx, qtx).unwrap();
    std::thread::spawn(move || {
        while let Ok(q) = qrx.recv_blocking() {
            let Request::DebugState { tail_lines } = q.request else { continue };
            q.respond(answer(tail_lines));
        }
    });
    server
}

fn canned() -> Value {
    json!({"version": 1, "pid": 1, "front": false, "dock_badge": null, "dock_bounces": 0, "windows": [], "名字": "新会话"})
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn state_prints_pretty_json_and_passes_the_tail() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("t.sock");
    let _server = serve(&sock, |tail| {
        let mut v = canned();
        v["tail"] = json!(tail);
        Response::DebugState { state: v }
    });
    let out = gilvt().env("GILVT_SOCKET", &sock).args(["debug", "state"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("\n  \"version\": 1"), "pretty-printed: {text}");
    assert!(text.contains("新会话"), "unicode kept as is: {text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["tail"], json!(20), "default tail");

    let out = gilvt().env("GILVT_SOCKET", &sock).args(["debug", "state", "--tail", "999"]).output().unwrap();
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["tail"], json!(200), "clamped");
}

#[test]
fn state_by_pid_uses_the_per_process_socket() {
    let dir = tempfile::tempdir().unwrap();
    // `socket_path_for` in the child: $TMPDIR/gilvt-<uid>/<pid>.sock.
    let uid = std::fs::metadata(dir.path()).unwrap().uid();
    let runtime: PathBuf = dir.path().join(format!("gilvt-{uid}"));
    let _server = serve(&runtime.join("4242.sock"), |_| Response::DebugState { state: canned() });
    let out = gilvt().env("TMPDIR", dir.path()).env("GILVT_SOCKET", dir.path().join("other.sock")).args(["debug", "state", "--pid", "4242"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(serde_json::from_str::<Value>(&stdout(&out)).unwrap(), canned());
}

#[test]
fn state_errors() {
    let out = gilvt().args(["debug", "state"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stderr(&out).trim(), "gilvt debug: 不在 gilvt 中运行，请用 --pid 指定进程");

    let dir = tempfile::tempdir().unwrap();
    let out = gilvt().env("GILVT_SOCKET", dir.path().join("gone.sock")).args(["debug", "state"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("cannot reach the gilvt app"), "{}", stderr(&out));

    let sock = dir.path().join("t.sock");
    let _server = serve(&sock, |_| Response::Error { message: "debug state: boom".into() });
    let out = gilvt().env("GILVT_SOCKET", &sock).args(["debug", "state"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("boom"));
    assert!(stdout(&out).is_empty());

    let out = gilvt().args(["debug", "frob"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("usage:"));
}

#[test]
fn wait_returns_once_the_condition_holds() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("t.sock");
    let polls = Arc::new(AtomicU64::new(0));
    let counter = polls.clone();
    let _server = serve(&sock, move |_| {
        let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
        Response::DebugState { state: json!({"n": n, "rows": [{"name": "新会话", "status": if n >= 3 { "idle" } else { "thinking" }}]}) }
    });
    let out = gilvt()
        .env("GILVT_SOCKET", &sock)
        .args(["debug", "wait", r#"rows[?name=="新会话"].status == "idle" && n exists"#, "--interval", "10ms"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).is_empty() && stderr(&out).is_empty());
    assert_eq!(polls.load(Ordering::SeqCst), 3);
}

#[test]
fn wait_times_out_with_the_last_state() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("t.sock");
    let _server = serve(&sock, |_| Response::DebugState { state: json!({"front": false}) });
    let started = Instant::now();
    let out = gilvt().env("GILVT_SOCKET", &sock).args(["debug", "wait", "front == true", "--timeout", "300ms", "--interval", "50ms"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(started.elapsed() >= Duration::from_millis(300));
    let err = stderr(&out);
    assert!(err.contains("front == true"), "{err}");
    assert!(err.contains("\"front\": false"), "last state, pretty: {err}");
    assert!(stdout(&out).is_empty());

    // Never reachable: the last error instead of a state.
    let out = gilvt().env("GILVT_SOCKET", dir.path().join("gone.sock")).args(["debug", "wait", "front == true", "--timeout", "100ms"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("no state"), "{}", stderr(&out));
}

#[test]
fn wait_rides_out_an_app_that_is_still_starting() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("t.sock");
    let child = gilvt().env("GILVT_SOCKET", &sock).args(["debug", "wait", "version == 1", "--timeout", "10s", "--interval", "20ms"]).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let _server = serve(&sock, |_| Response::DebugState { state: canned() });
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
}

#[test]
fn wait_rejects_a_bad_condition_before_polling() {
    let started = Instant::now();
    // No socket either: the condition is checked first.
    let out = gilvt().args(["debug", "wait", "front ~= true", "--timeout", "5s"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.contains("bad condition: expected an operator"), "{err}");
    assert!(err.contains("\n  front ~= true\n        ^"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(2));

    let out = gilvt().args(["debug", "wait", "front == true", "--timeout", "10"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("bad duration"));
}

#[test]
fn a_gilvt_without_debug_state_says_so_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("t.sock");
    let (tx, _rx) = async_channel::unbounded();
    let _server = Server::start_refusing_queries(&sock, tx, gilvt_ipc::DEBUG_STATE_DISABLED).unwrap();
    let out = gilvt().env("GILVT_SOCKET", &sock).args(["debug", "state"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("debug state is disabled (start gilvt with GILVT_DEBUG_STATE=1)"), "{}", stderr(&out));
    assert!(stdout(&out).is_empty());
    let started = Instant::now();
    let out = gilvt().env("GILVT_SOCKET", &sock).args(["debug", "wait", "front == true", "--timeout", "5s"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("GILVT_DEBUG_STATE=1"), "{}", stderr(&out));
    assert!(started.elapsed() < Duration::from_secs(3), "no polling until the timeout");
}

#[test]
fn eval_reads_a_saved_state() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("s.json");
    std::fs::write(&file, canned().to_string()).unwrap();
    let f = file.to_str().unwrap();
    let run = |args: &[&str]| gilvt().args(["debug", "eval", "--state-file", f]).args(args).output().unwrap();
    assert_eq!(run(&["front == false && dock_badge == null"]).status.code(), Some(0));
    assert_eq!(run(&["front == 0"]).status.code(), Some(1), "a bool is not a number");
    assert_eq!(run(&["front =="]).status.code(), Some(2));
    let out = run(&["--path", "名字"]);
    assert_eq!((out.status.code(), stdout(&out).trim()), (Some(0), r#"["新会话"]"#));
    let out = run(&["--path", "windows[*]"]);
    assert_eq!(stdout(&out).trim(), "[]");
    assert_eq!(run(&["--path", ".front"]).status.code(), Some(2));
    let out = gilvt().args(["debug", "eval", "--state-file", "/nonexistent/s.json", "front exists"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}
