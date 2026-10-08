//! The per-user daemon (spec §2.1, §7): owns `run/daemon.sock`, keeps the link table, answers bridges.
//! One per user: `run/daemon.lock` (flock) decides who serves; a daemon of another build takes over.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gilvt_ipc::remote::*;

use crate::links::Links;
use crate::paths::Layout;
use crate::peer::peer_uid;

pub struct Config {
    pub layout: Layout,
    pub build_id: String,
    pub hostname: String,
    pub uname: String,
    pub reap_every: Duration,
    pub idle_exit: Duration,
}

struct State {
    links: Links,
    seq: u64,
    next_bridge: u64,
    bridges: Vec<(u64, Sender<DaemonMsg>)>,
    last_activity: Instant,
}

impl State {
    fn broadcast(&mut self, event: RemoteEvent) {
        self.seq += 1;
        let msg = DaemonMsg::Event { seq: self.seq, event };
        self.bridges.retain(|(_, tx)| tx.send(msg.clone()).is_ok());
        self.last_activity = Instant::now();
    }
}

pub fn run(cfg: Config) -> io::Result<()> {
    gilvt_ipc::secure_dir(&cfg.layout.run_dir())?;
    let lock = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(cfg.layout.lock())?;
    let mut inherited = Vec::new();
    if !try_lock(&lock) {
        match ask_to_hand_over(&cfg)? {
            None => return Ok(()), // same build already serving
            Some(links) => inherited = links,
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !try_lock(&lock) {
            if Instant::now() > deadline { return Err(io::Error::other("the old daemon did not let go of the lock")); }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let sock = cfg.layout.socket();
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock)?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
    let _ = cfg.layout.prune_versions(&cfg.build_id);

    let mut links = Links::default();
    for l in inherited { links.up(l); }
    let state = Arc::new(Mutex::new(State { links, seq: 0, next_bridge: 0, bridges: Vec::new(), last_activity: Instant::now() }));
    let cfg = Arc::new(cfg);

    {
        let (state, cfg, sock) = (state.clone(), cfg.clone(), sock.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(cfg.reap_every);
            let mut s = state.lock().unwrap();
            for ev in s.links.reap(pid_alive) { s.broadcast(ev); }
            if s.links.is_empty() && s.bridges.is_empty() && s.last_activity.elapsed() >= cfg.idle_exit {
                let _ = std::fs::remove_file(&sock);
                std::process::exit(0);
            }
        });
    }

    let me = unsafe { libc::getuid() };
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        if peer_uid(&conn).ok() != Some(me) { continue; }
        let (state, cfg) = (state.clone(), cfg.clone());
        std::thread::spawn(move || { let _ = serve(conn, &state, &cfg); });
    }
    Ok(())
}

fn try_lock(file: &std::fs::File) -> bool {
    // SAFETY: flock on an fd we own.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

/// `None`: a daemon of this build is serving. `Some(links)`: the old daemon handed over and is exiting.
fn ask_to_hand_over(cfg: &Config) -> io::Result<Option<Vec<LinkState>>> {
    // The lock holder may not have bound its socket yet (two daemons started together): retry briefly.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut s = loop {
        match UnixStream::connect(cfg.layout.socket()) {
            Ok(s) => break s,
            Err(e) if Instant::now() > deadline => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    write_frame(&mut s, &LocalMsg::Takeover { build_id: cfg.build_id.clone() })?;
    match read_frame::<LocalReply>(&mut s)? {
        Some(LocalReply::Handover { links }) => Ok(Some(links)),
        _ => Ok(None),
    }
}

fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks existence and permission.
    let ok = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
    ok || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn serve(mut conn: UnixStream, state: &Arc<Mutex<State>>, cfg: &Config) -> io::Result<()> {
    match read_frame::<LocalMsg>(&mut conn)? {
        Some(LocalMsg::Login { link, .. }) => {
            let mut s = state.lock().unwrap();
            let ev = s.links.up(link);
            s.broadcast(ev);
            drop(s);
            write_frame(&mut conn, &LocalReply::Ok)
        }
        Some(LocalMsg::Takeover { build_id }) if build_id == cfg.build_id => write_frame(&mut conn, &LocalReply::Ok),
        Some(LocalMsg::Takeover { .. }) => {
            let links = state.lock().unwrap().links.all();
            write_frame(&mut conn, &LocalReply::Handover { links })?;
            let _ = std::fs::remove_file(cfg.layout.socket());
            std::process::exit(0);
        }
        Some(LocalMsg::Bridge { hello: AppMsg::Hello { build_id, .. } }) => {
            if build_id != cfg.build_id {
                return write_frame(&mut conn, &DaemonMsg::Mismatch { build_id: cfg.build_id.clone() });
            }
            let (tx, rx) = channel::<DaemonMsg>();
            let id = {
                let mut s = state.lock().unwrap();
                let welcome = DaemonMsg::Welcome { build_id: cfg.build_id.clone(), hostname: cfg.hostname.clone(), uname: cfg.uname.clone(), links: s.links.all(), seq: s.seq };
                tx.send(welcome).ok();
                s.next_bridge += 1;
                let id = s.next_bridge;
                s.bridges.push((id, tx.clone()));
                s.last_activity = Instant::now();
                id
            };
            let mut writer = conn.try_clone()?;
            std::thread::spawn(move || { for msg in rx { if write_frame(&mut writer, &msg).is_err() { break; } } });
            while let Some(msg) = read_frame::<AppMsg>(&mut conn)? {
                if let AppMsg::Request { id: rid, req } = msg {
                    let resp = match req {
                        RemoteRequest::Ping => RemoteResponse::Pong,
                        RemoteRequest::LinkInfo => RemoteResponse::Links { links: state.lock().unwrap().links.all() },
                    };
                    if tx.send(DaemonMsg::Response { id: rid, resp }).is_err() { break; }
                }
            }
            let mut s = state.lock().unwrap();
            s.bridges.retain(|(b, _)| *b != id);
            s.last_activity = Instant::now();
            Ok(())
        }
        Some(LocalMsg::Bridge { .. }) | None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_ipc::remote::*;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    fn cfg(home: &std::path::Path, build: &str) -> Config {
        Config { layout: Layout::from_home(home), build_id: build.into(), hostname: "devbox".into(), uname: "Linux x86_64".into(), reap_every: Duration::from_millis(50), idle_exit: Duration::from_secs(3600) }
    }
    fn start(home: &std::path::Path, build: &str) -> std::thread::JoinHandle<std::io::Result<()>> {
        let c = cfg(home, build);
        let h = std::thread::spawn(move || run(c));
        let sock = Layout::from_home(home).socket();
        for _ in 0..100 { if UnixStream::connect(&sock).is_ok() { break } std::thread::sleep(Duration::from_millis(20)); }
        h
    }
    fn bridge(home: &std::path::Path, build: &str) -> UnixStream {
        let mut s = UnixStream::connect(Layout::from_home(home).socket()).unwrap();
        write_frame(&mut s, &LocalMsg::Bridge { hello: AppMsg::Hello { build_id: build.into(), app_instance: "t".into(), cursor: 0 } }).unwrap();
        s
    }
    fn login(home: &std::path::Path, link: &str, pid: u32) {
        let mut s = UnixStream::connect(Layout::from_home(home).socket()).unwrap();
        write_frame(&mut s, &LocalMsg::Login { build_id: "0.1.0-aaaaaaaa".into(), link: LinkState { link: link.into(), hostname: "devbox".into(), tty: None, pid } }).unwrap();
        assert_eq!(read_frame::<LocalReply>(&mut s).unwrap(), Some(LocalReply::Ok));
    }

    #[test]
    fn bridge_gets_welcome_then_link_events() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        match read_frame::<DaemonMsg>(&mut b).unwrap().unwrap() {
            DaemonMsg::Welcome { hostname, links, .. } => { assert_eq!(hostname, "devbox"); assert!(links.is_empty()); }
            other => panic!("{other:?}"),
        }
        let me = std::process::id();
        login(home.path(), "t-1", me);
        match read_frame::<DaemonMsg>(&mut b).unwrap().unwrap() {
            DaemonMsg::Event { event: RemoteEvent::LinkUp(s), .. } => assert_eq!(s.link, "t-1"),
            other => panic!("{other:?}"),
        }
        write_frame(&mut b, &AppMsg::Request { id: 3, req: RemoteRequest::LinkInfo }).unwrap();
        match read_frame::<DaemonMsg>(&mut b).unwrap().unwrap() {
            DaemonMsg::Response { id: 3, resp: RemoteResponse::Links { links: l } } => assert_eq!(l.len(), 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dead_login_shell_is_reaped() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        read_frame::<DaemonMsg>(&mut b).unwrap();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        login(home.path(), "t-2", pid);
        read_frame::<DaemonMsg>(&mut b).unwrap(); // link_up
        b.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        match read_frame::<DaemonMsg>(&mut b).unwrap().unwrap() {
            DaemonMsg::Event { event: RemoteEvent::LinkDown { link }, .. } => assert_eq!(link, "t-2"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn other_build_bridge_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let mut b = bridge(home.path(), "0.2.0-bbbbbbbb");
        assert_eq!(read_frame::<DaemonMsg>(&mut b).unwrap(), Some(DaemonMsg::Mismatch { build_id: "0.1.0-aaaaaaaa".into() }));
    }

    #[test]
    fn same_build_second_daemon_exits_at_once() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        assert!(run(cfg(home.path(), "0.1.0-aaaaaaaa")).is_ok());
    }
}
