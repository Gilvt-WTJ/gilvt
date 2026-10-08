use gilvt_ipc::remote::*;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Stops the detached daemon (it would idle for 24 h) even when an assertion fails.
struct KillDaemon(String);
impl Drop for KillDaemon {
    fn drop(&mut self) {
        let _ = Command::new("pkill").args(["-f", &self.0]).status();
    }
}

#[test]
fn login_shows_up_on_the_bridge_and_goes_down_when_the_shell_exits() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".gilvt-server/0.1.0-aaaaaaaa");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("gilvt-remote");
    std::fs::copy(env!("CARGO_BIN_EXE_gilvt-remote"), &exe).unwrap();
    let _kill = KillDaemon(exe.display().to_string());

    let mut bridge = Command::new(&exe).arg("bridge").env("HOME", home.path()).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = bridge.stdin.take().unwrap();
    let mut stdout = bridge.stdout.take().unwrap();
    // Frames are read on a thread so that a regression times out instead of hanging the test.
    let (tx, rx) = mpsc::channel::<DaemonMsg>();
    std::thread::spawn(move || {
        while let Ok(Some(m)) = read_frame::<DaemonMsg>(&mut stdout) {
            if tx.send(m).is_err() { break; }
        }
    });
    let next = |what: &str| rx.recv_timeout(Duration::from_secs(10)).unwrap_or_else(|e| panic!("no {what} from the bridge: {e}"));

    let mut hello = Vec::new();
    write_frame(&mut hello, &LocalMsg::Bridge { hello: AppMsg::Hello { build_id: "0.1.0-aaaaaaaa".into(), app_instance: "t".into(), cursor: 0 } }).unwrap();
    stdin.write_all(&hello).unwrap();
    assert!(matches!(next("Welcome"), DaemonMsg::Welcome { .. }));

    let mut login = Command::new(&exe).args(["login", "--link", "t-9", "--exec", "sleep 1"]).env("HOME", home.path()).env("SHELL", "/bin/sh").spawn().unwrap();
    // Reap it as soon as it exits: the daemon sees a zombie as alive (sshd does the reaping in real life).
    let reaper = std::thread::spawn(move || login.wait().unwrap());
    let mut seen = Vec::new();
    while seen.len() < 2 {
        if let DaemonMsg::Event { event, .. } = next("link event") { seen.push(event); }
    }
    assert!(matches!(&seen[0], RemoteEvent::LinkUp(s) if s.link == "t-9"));
    assert_eq!(seen[1], RemoteEvent::LinkDown { link: "t-9".into() });
    assert!(reaper.join().unwrap().success());
    drop(stdin);
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end {
        if bridge.try_wait().unwrap().is_some() { return; }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = bridge.kill();
    let _ = bridge.wait();
}
