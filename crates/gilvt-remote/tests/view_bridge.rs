//! A View sent to `run/gilvt.sock` end to end (R2a §8.2): the daemon answers Ok and the bridge carries an `Open`
//! event with the shell's link. Runs on macOS and, in scripts/remote-test.sh, Linux.
use gilvt_ipc::remote::*;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

struct KillDaemon(String);
impl Drop for KillDaemon {
    fn drop(&mut self) {
        let _ = Command::new("pkill").args(["-f", &self.0]).status();
    }
}

/// Nothing `Open` arrives within a short while (other events are ignored).
fn no_open(rx: &mpsc::Receiver<DaemonMsg>) -> bool {
    let end = std::time::Instant::now() + Duration::from_millis(500);
    while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
        if let Ok(DaemonMsg::Event { event: RemoteEvent::Open { .. }, .. }) = rx.recv_timeout(left) {
            return false;
        }
    }
    true
}

#[test]
fn a_view_for_a_registered_link_reaches_the_bridge_as_open() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".gilvt-server/0.1.0-bbbbbbbb");
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let exe = dir.join("gilvt-remote");
    std::fs::copy(env!("CARGO_BIN_EXE_gilvt-remote"), &exe).unwrap();
    std::os::unix::fs::symlink("../gilvt-remote", dir.join("bin/gilvt")).unwrap();
    let _kill = KillDaemon(exe.display().to_string());
    std::fs::write(home.path().join("a.md"), "# hi\n").unwrap();

    let mut bridge = Command::new(&exe).arg("bridge").env("HOME", home.path()).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = bridge.stdin.take().unwrap();
    let mut stdout = bridge.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<DaemonMsg>();
    std::thread::spawn(move || {
        while let Ok(Some(m)) = read_frame::<DaemonMsg>(&mut stdout) {
            if tx.send(m).is_err() {
                break;
            }
        }
    });
    let mut hello = Vec::new();
    write_frame(&mut hello, &LocalMsg::Bridge { hello: AppMsg::Hello { build_id: "0.1.0-bbbbbbbb".into(), app_instance: "t".into(), cursor: 0, epoch: None } }).unwrap();
    stdin.write_all(&hello).unwrap();
    assert!(matches!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), DaemonMsg::Welcome { .. }));

    // A login shell that stays up (the link is registered while it runs) and says where its socket is.
    let mut shell = Command::new(&exe)
        .args(["login", "--link", "t-1", "--exec", r#"printf '%s\n' "$GILVT_SOCKET" > "$HOME/sock"; while [ ! -e "$HOME/go" ]; do sleep 0.1; done"#])
        .env("HOME", home.path())
        .env("SHELL", "/bin/sh")
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let socket = loop {
        if let Ok(s) = std::fs::read_to_string(home.path().join("sock")) {
            if s.ends_with('\n') {
                break s.trim().to_string();
            }
        }
        assert!(std::time::Instant::now() < deadline, "the shell never wrote its socket");
        std::thread::sleep(Duration::from_millis(50));
    };
    // What the remote `gilvt view --pin a.md` sends (the CLI itself comes with the app side, Task 8).
    let req = gilvt_ipc::Request::View { pane: None, path: Some(home.path().join("a.md")), content: None, as_type: None, line: None, col: None, pin: true, link: Some("t-1".into()) };
    assert_eq!(gilvt_ipc::send(std::path::Path::new(&socket), &req).unwrap(), gilvt_ipc::Response::Ok);
    let open = loop {
        match rx.recv_timeout(Duration::from_secs(10)).expect("no Open frame") {
            DaemonMsg::Event { event: RemoteEvent::Open { link, request }, .. } => break (link, *request),
            _ => continue,
        }
    };
    assert_eq!(open.0, "t-1");
    assert!(matches!(open.1, gilvt_ipc::Request::View { pin: true, link: Some(ref l), path: Some(ref p), .. } if l == "t-1" && p.ends_with("a.md")), "{:?}", open.1);
    // an unregistered link is refused and nothing is broadcast
    let stray = gilvt_ipc::Request::View { pane: None, path: Some(home.path().join("a.md")), content: None, as_type: None, line: None, col: None, pin: true, link: Some("t-9".into()) };
    assert_eq!(gilvt_ipc::send(std::path::Path::new(&socket), &stray).unwrap(), gilvt_ipc::Response::Error { message: OPEN_NOT_LINKED.into() });
    assert!(no_open(&rx), "a refused View was broadcast");
    // a request line over the cap never reaches open_reply: the daemon still answers too_large, and broadcasts nothing
    let huge = gilvt_ipc::Request::View { pane: None, path: None, content: Some("\u{1}".repeat(3 * 1024 * 1024)), as_type: None, line: None, col: None, pin: false, link: Some("t-1".into()) };
    assert_eq!(gilvt_ipc::send(std::path::Path::new(&socket), &huge).unwrap(), gilvt_ipc::Response::Error { message: OPEN_TOO_LARGE.into() });
    assert!(no_open(&rx), "an oversized View was broadcast");
    std::fs::write(home.path().join("go"), "").unwrap();
    assert!(shell.wait().unwrap().success());
    let _ = bridge.kill();
}
