use gilvt_ipc::remote::*;
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::time::Duration;

fn daemon(home: &std::path::Path, build: &str) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_gilvt-remote"))
        .args(["daemon", "--build-id", build, "--foreground"])
        .env("HOME", home)
        .spawn()
        .unwrap()
}

#[test]
fn new_build_takes_over_links() {
    let home = tempfile::tempdir().unwrap();
    let sock = home.path().join(".gilvt-server/run/daemon.sock");
    let mut old = daemon(home.path(), "0.1.0-aaaaaaaa");
    for _ in 0..100 { if UnixStream::connect(&sock).is_ok() { break } std::thread::sleep(Duration::from_millis(20)); }
    let mut s = UnixStream::connect(&sock).unwrap();
    write_frame(&mut s, &LocalMsg::Login { build_id: "0.1.0-aaaaaaaa".into(), link: LinkState { link: "t-1".into(), hostname: "h".into(), tty: None, pid: std::process::id() } }).unwrap();
    read_frame::<LocalReply>(&mut s).unwrap();

    let mut new = daemon(home.path(), "0.2.0-bbbbbbbb");
    assert!(old.wait().unwrap().success(), "old daemon exits after the handover");
    let mut b = None;
    for _ in 0..150 {
        if let Ok(mut c) = UnixStream::connect(&sock) {
            write_frame(&mut c, &LocalMsg::Bridge { hello: AppMsg::Hello { build_id: "0.2.0-bbbbbbbb".into(), app_instance: "t".into(), cursor: 0 } }).unwrap();
            if let Ok(Some(DaemonMsg::Welcome { links, build_id, .. })) = read_frame::<DaemonMsg>(&mut c) { b = Some((links, build_id)); break; }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let (links, build) = b.expect("new daemon answers");
    assert_eq!(build, "0.2.0-bbbbbbbb");
    assert_eq!(links.iter().map(|l| l.link.as_str()).collect::<Vec<_>>(), ["t-1"]);
    new.kill().unwrap();
    let _ = new.wait();
}
