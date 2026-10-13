//! The whole remote path (spec R2a §4): the real `gilvt` link runs `hook`, the daemon attributes it to the login's
//! link, and the bridge's stdout carries it as a Hook frame. Runs on macOS and, in scripts/remote-test.sh, Linux.
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

#[test]
fn a_hook_in_the_login_shell_reaches_the_bridge() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".gilvt-server/0.1.0-aaaaaaaa");
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let exe = dir.join("gilvt-remote");
    std::fs::copy(env!("CARGO_BIN_EXE_gilvt-remote"), &exe).unwrap();
    std::os::unix::fs::symlink("../gilvt-remote", dir.join("bin/gilvt")).unwrap();
    let _kill = KillDaemon(exe.display().to_string());

    let mut bridge = Command::new(&exe).arg("bridge").env("HOME", home.path()).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = bridge.stdin.take().unwrap();
    let mut stdout = bridge.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<DaemonMsg>();
    std::thread::spawn(move || {
        while let Ok(Some(m)) = read_frame::<DaemonMsg>(&mut stdout) {
            if tx.send(m).is_err() { break; }
        }
    });
    let mut hello = Vec::new();
    write_frame(&mut hello, &LocalMsg::Bridge { hello: AppMsg::Hello { build_id: "0.1.0-aaaaaaaa".into(), app_instance: "t".into(), cursor: 0, epoch: None } }).unwrap();
    stdin.write_all(&hello).unwrap();
    assert!(matches!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), DaemonMsg::Welcome { .. }));

    // `login` registers the link and runs the command in the user's shell with gilvt's variables.
    let status = Command::new(&exe)
        .args(["login", "--link", "t-1", "--exec", r#"printf '{"session_id":"s1","hook_event_name":"Stop"}' | "$GILVT_BIN_DIR/gilvt" hook claude Stop"#])
        .env("HOME", home.path())
        .env("SHELL", "/bin/sh")
        .env_remove("TMUX") // a developer running the tests inside tmux would make the daemon drop the hook
        .status()
        .unwrap();
    assert!(status.success());
    let hook = loop {
        match rx.recv_timeout(Duration::from_secs(10)).expect("no Hook frame") {
            DaemonMsg::Event { event: RemoteEvent::Hook { link, event, payload, .. }, .. } => break (link, event, payload),
            _ => continue,
        }
    };
    assert_eq!((hook.0.as_str(), hook.1.as_str(), hook.2["session_id"].as_str()), ("t-1", "Stop", Some("s1")));
    let _ = bridge.kill();
}
