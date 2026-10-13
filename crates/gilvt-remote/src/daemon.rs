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

use crate::follow::{check_path, Follower, Push, FOLLOW_POLL, MAX_FOLLOWS};
use crate::history::{self, fit_entries, HistoryWorker, HISTORY_BYTES};
use crate::hooklog::{attribute, hook_size, HookLog, HOOK_LOG_BYTES, HOOK_LOG_MAX};
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

/// What a bridge's writer thread sends. Events are shared (`Arc`), so a hook of up to 1 MB is not copied per bridge
/// or for the replay buffer under the State lock: the writer builds its frame outside the lock.
enum Out {
    Msg(DaemonMsg),
    Event(u64, Arc<RemoteEvent>),
}

impl Out {
    fn write(self, w: &mut UnixStream) -> io::Result<()> {
        match self {
            Out::Msg(m) => write_frame(w, &m),
            Out::Event(seq, event) => write_frame(w, &DaemonMsg::Event { seq, event: Arc::unwrap_or_clone(event) }),
        }
    }
}

struct State {
    links: Links,
    seq: u64,
    next_bridge: u64,
    bridges: Vec<(u64, Sender<Out>)>,
    last_activity: Instant,
    /// This daemon's start time in ms (R2a §4.3).
    epoch: u64,
    hooks: HookLog,
    /// Transcripts being followed, over all bridges.
    follows: usize,
}

impl State {
    fn new(links: Links) -> State {
        let epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_millis() as u64);
        State { links, seq: 0, next_bridge: 0, bridges: Vec::new(), last_activity: Instant::now(), epoch, hooks: HookLog::new(HOOK_LOG_MAX, HOOK_LOG_BYTES), follows: 0 }
    }

    /// A link event (not kept for replay).
    fn broadcast(&mut self, event: RemoteEvent) {
        self.send(Arc::new(event));
    }

    /// A hook: kept for replay (`size`: [`hook_size`], computed before the lock was taken) and sent.
    fn broadcast_hook(&mut self, event: RemoteEvent, size: usize) {
        let event = Arc::new(event);
        self.send(event.clone());
        self.hooks.push(self.seq, event, size);
    }

    fn send(&mut self, event: Arc<RemoteEvent>) {
        self.seq += 1;
        let seq = self.seq;
        self.bridges.retain(|(_, tx)| tx.send(Out::Event(seq, event.clone())).is_ok());
        self.last_activity = Instant::now();
    }
}

/// Serves until the process ends: a takeover by another build or the idle timeout calls
/// `process::exit(0)` (it never returns then). Returns `Ok(())` only when a daemon of the same build is
/// already serving, and `Err` when setup fails.
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
    let hook_sock = cfg.layout.hook_socket();
    let _ = std::fs::remove_file(&hook_sock);
    let hook_listener = UnixListener::bind(&hook_sock)?;
    std::fs::set_permissions(&hook_sock, std::fs::Permissions::from_mode(0o600))?;
    let _ = cfg.layout.prune_versions(&cfg.build_id);

    let mut links = Links::default();
    for l in inherited { links.up(l); }
    let state = Arc::new(Mutex::new(State::new(links)));
    let cfg = Arc::new(cfg);
    let history = Arc::new(HistoryWorker::new(history::scanner(cfg.layout.state_dir(), cfg.layout.home())));

    {
        let state = state.clone();
        std::thread::spawn(move || serve_gilvt_sock(hook_listener, &state));
    }

    {
        let (state, cfg, sock, hook_sock) = (state.clone(), cfg.clone(), sock.clone(), hook_sock.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(cfg.reap_every);
            let mut s = state.lock().unwrap();
            for ev in s.links.reap(pid_alive) { s.broadcast(ev); }
            if s.links.is_empty() && s.bridges.is_empty() && s.last_activity.elapsed() >= cfg.idle_exit {
                let _ = std::fs::remove_file(&sock);
                let _ = std::fs::remove_file(&hook_sock);
                std::process::exit(0);
            }
        });
    }

    let me = unsafe { libc::getuid() };
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        if peer_uid(&conn).ok() != Some(me) { continue; }
        let (state, cfg, history) = (state.clone(), cfg.clone(), history.clone());
        std::thread::spawn(move || { let _ = serve(conn, &state, &cfg, &history); });
    }
    Ok(())
}

/// How long one connection may take to deliver its request line. A real `gilvt hook` writes the line right after
/// connecting and gives up itself after 200 ms (`send_nowait`), so waiting longer gains nothing.
const HOOK_READ_TIMEOUT: Duration = Duration::from_millis(200);

/// `run/gilvt.sock`: one request line per connection, like the app's socket (R2a §4.1). Hooks are attributed to a
/// link and broadcast; anything else is ignored (R2a's second plan adds View / Diff here).
///
/// Connections are handled one at a time, inline, in the order they were accepted (no thread per connection): hooks
/// reach the app in the order they were sent, and a stuck client delays the others by at most `HOOK_READ_TIMEOUT`
/// (R2a plan deviation 16).
fn serve_gilvt_sock(listener: UnixListener, state: &Arc<Mutex<State>>) {
    let me = unsafe { libc::getuid() };
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        if peer_uid(&conn).ok() != Some(me) {
            continue;
        }
        let _ = conn.set_read_timeout(Some(HOOK_READ_TIMEOUT));
        // A client that connects and then says nothing fails the read at the timeout and is dropped.
        if let Ok(gilvt_ipc::Request::Hook { agent, event, payload, link, tmux, .. }) = gilvt_ipc::read_request(&conn) {
            let size = hook_size(&payload);
            let mut s = state.lock().unwrap();
            if let Some(link) = attribute(&s.links, link.as_deref(), tmux) {
                s.broadcast_hook(RemoteEvent::Hook { link, agent, event, payload }, size);
            }
        }
    }
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

fn serve(mut conn: UnixStream, state: &Arc<Mutex<State>>, cfg: &Config, history: &Arc<HistoryWorker>) -> io::Result<()> {
    match read_frame::<LocalMsg>(&mut conn)? {
        Some(LocalMsg::Login { link, .. }) => {
            let mut s = state.lock().unwrap();
            let ev = s.links.up(link);
            s.broadcast(ev);
            drop(s);
            write_frame(&mut conn, &LocalReply::Ok)
        }
        Some(LocalMsg::Takeover { build_id }) if build_id == cfg.build_id => write_frame(&mut conn, &LocalReply::Ok),
        // `ensure_daemon`: registers nothing and replays nothing; another build gets no reply (it starts its own).
        Some(LocalMsg::Probe { build_id }) if build_id == cfg.build_id => write_frame(&mut conn, &LocalReply::Ok),
        Some(LocalMsg::Probe { .. }) => Ok(()),
        Some(LocalMsg::Takeover { .. }) => {
            let links = state.lock().unwrap().links.all();
            write_frame(&mut conn, &LocalReply::Handover { links })?;
            let _ = std::fs::remove_file(cfg.layout.socket());
            let _ = std::fs::remove_file(cfg.layout.hook_socket());
            std::process::exit(0);
        }
        Some(LocalMsg::Bridge { hello: AppMsg::Hello { build_id, cursor, epoch, .. } }) => {
            if build_id != cfg.build_id {
                return write_frame(&mut conn, &DaemonMsg::Mismatch { build_id: cfg.build_id.clone() });
            }
            let (tx, rx) = channel::<Out>();
            let id = {
                let mut s = state.lock().unwrap();
                // Welcome, the replay and the registration happen under one lock: no hook is lost or doubled. The
                // replay holds shared events (no payload is copied here), and only those of links still registered.
                let replay = s.hooks.replay(s.epoch, epoch, cursor, |l| s.links.contains(l));
                let welcome = DaemonMsg::Welcome { build_id: cfg.build_id.clone(), hostname: cfg.hostname.clone(), uname: cfg.uname.clone(), links: s.links.all(), seq: s.seq, epoch: s.epoch, gap: replay.gap, home: Some(cfg.layout.home()) };
                tx.send(Out::Msg(welcome)).ok();
                for (seq, event) in replay.events {
                    tx.send(Out::Event(seq, event)).ok();
                }
                s.next_bridge += 1;
                let id = s.next_bridge;
                s.bridges.push((id, tx.clone()));
                s.last_activity = Instant::now();
                id
            };
            let mut writer = conn.try_clone()?;
            std::thread::spawn(move || { for out in rx { if out.write(&mut writer).is_err() { break; } } });
            type Follows = Arc<Mutex<std::collections::HashMap<(String, String), Follower>>>;
            let follows: Follows = Arc::default();
            let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            {
                let (follows, closed, tx) = (follows.clone(), closed.clone(), tx.clone());
                std::thread::spawn(move || loop {
                    std::thread::sleep(FOLLOW_POLL);
                    if closed.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    let mut f = follows.lock().unwrap();
                    for ((agent, session_id), fl) in f.iter_mut() {
                        for push in fl.poll().unwrap_or_default() {
                            let msg = match push {
                                Push::Append { offset, bytes } => DaemonMsg::TranscriptAppend { agent: agent.clone(), session_id: session_id.clone(), offset, bytes: encode_bytes(&bytes) },
                                Push::Reset { size } => DaemonMsg::TranscriptReset { agent: agent.clone(), session_id: session_id.clone(), size },
                            };
                            if tx.send(Out::Msg(msg)).is_err() {
                                return;
                            }
                        }
                    }
                });
            }
            // However the read loop ends (EOF, garbage, reset), the bridge must be deregistered and the
            // connection shut down, or its writer thread and the idle-exit check would hang on it forever.
            let result = (|| -> io::Result<()> {
                while let Some(msg) = read_frame::<AppMsg>(&mut conn)? {
                    if let AppMsg::Request { id: rid, req } = msg {
                        let resp = match req {
                            RemoteRequest::Ping => Some(RemoteResponse::Pong),
                            RemoteRequest::LinkInfo => Some(RemoteResponse::Links { links: state.lock().unwrap().links.all() }),
                            RemoteRequest::TranscriptFollow { agent, session_id, path, from } => {
                                match check_path(&path, &cfg.layout.home()) {
                                    Err(message) => Some(RemoteResponse::Error { message }),
                                    Ok(path) => {
                                        // Insert and answer under the follows lock: `Following` precedes the first push.
                                        let mut f = follows.lock().unwrap();
                                        let key = (agent, session_id);
                                        let mut s = state.lock().unwrap();
                                        if !f.contains_key(&key) && s.follows >= MAX_FOLLOWS {
                                            Some(RemoteResponse::Error { message: format!("already following {MAX_FOLLOWS} transcripts") })
                                        } else {
                                            if !f.contains_key(&key) {
                                                s.follows += 1;
                                            }
                                            drop(s);
                                            let fl = Follower::new(path, from);
                                            let size = fl.size();
                                            f.insert(key, fl);
                                            if tx.send(Out::Msg(DaemonMsg::Response { id: rid, resp: RemoteResponse::Following { size } })).is_err() {
                                                break;
                                            }
                                            None
                                        }
                                    }
                                }
                            }
                            RemoteRequest::TranscriptUnfollow { agent, session_id } => {
                                if follows.lock().unwrap().remove(&(agent, session_id)).is_some() {
                                    state.lock().unwrap().follows -= 1;
                                }
                                Some(RemoteResponse::Ok)
                            }
                            RemoteRequest::History => {
                                let tx = tx.clone();
                                history.request(move |r| {
                                    let resp = match r {
                                        Ok(entries) => RemoteResponse::HistoryList { entries: fit_entries((*entries).clone(), HISTORY_BYTES) },
                                        Err(message) => RemoteResponse::Error { message },
                                    };
                                    let _ = tx.send(Out::Msg(DaemonMsg::Response { id: rid, resp }));
                                });
                                None
                            }
                            RemoteRequest::ReadFile { .. } | RemoteRequest::ReadForPreview { .. } | RemoteRequest::Stat { .. } | RemoteRequest::WriteFile { .. } | RemoteRequest::GitChanges { .. } | RemoteRequest::Watch { .. } | RemoteRequest::Unwatch { .. } => {
                                Some(RemoteResponse::Error { message: "not supported by this daemon yet".into() })
                            }
                        };
                        if let Some(resp) = resp {
                            if tx.send(Out::Msg(DaemonMsg::Response { id: rid, resp })).is_err() { break; }
                        }
                    }
                }
                Ok(())
            })();
            closed.store(true, std::sync::atomic::Ordering::Relaxed);
            {
                let n = follows.lock().unwrap().len();
                let mut s = state.lock().unwrap();
                s.follows -= n;
                s.bridges.retain(|(b, _)| *b != id);
                s.last_activity = Instant::now();
            }
            let _ = conn.shutdown(std::net::Shutdown::Both);
            result
        }
        Some(LocalMsg::Bridge { .. }) | None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
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
    fn bridge_at(home: &std::path::Path, build: &str, cursor: u64, epoch: Option<u64>) -> UnixStream {
        let mut s = UnixStream::connect(Layout::from_home(home).socket()).unwrap();
        write_frame(&mut s, &LocalMsg::Bridge { hello: AppMsg::Hello { build_id: build.into(), app_instance: "t".into(), cursor, epoch } }).unwrap();
        s
    }
    fn bridge(home: &std::path::Path, build: &str) -> UnixStream {
        bridge_at(home, build, 0, None)
    }
    fn send_hook(home: &std::path::Path, link: Option<&str>, tmux: bool, event: &str) {
        let mut s = UnixStream::connect(Layout::from_home(home).hook_socket()).unwrap();
        let req = gilvt_ipc::Request::Hook { pane: None, agent: "claude".into(), event: event.into(), payload: serde_json::json!({"session_id": "s"}), link: link.map(Into::into), tmux };
        let mut line = serde_json::to_vec(&req).unwrap();
        line.push(b'\n');
        std::io::Write::write_all(&mut s, &line).unwrap();
    }
    /// Until the daemon has handled events up to `want` (looked at through short-lived bridges: no fixed sleeps).
    fn wait_for_seq(home: &std::path::Path, build: &str, want: u64) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            let mut probe = bridge(home, build);
            probe.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            if let Some(DaemonMsg::Welcome { seq, .. }) = read_frame::<DaemonMsg>(&mut probe).unwrap() {
                if seq >= want {
                    return;
                }
            }
            assert!(Instant::now() < end, "the daemon never handled the hooks (wanted seq {want})");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn next_hook(b: &mut UnixStream) -> (u64, String) {
        loop {
            match read_frame::<DaemonMsg>(b).unwrap().unwrap() {
                DaemonMsg::Event { seq, event: RemoteEvent::Hook { event, .. } } => return (seq, event),
                _ => continue,
            }
        }
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
    fn hooks_of_a_registered_link_reach_the_bridge_and_others_are_dropped() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        read_frame::<DaemonMsg>(&mut b).unwrap(); // welcome
        login(home.path(), "t-1", std::process::id());
        send_hook(home.path(), None, false, "NoLink");
        send_hook(home.path(), Some("t-9"), false, "Unknown");
        send_hook(home.path(), Some("t-1"), true, "Tmux");
        send_hook(home.path(), Some("t-1"), false, "Stop");
        b.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        assert_eq!(next_hook(&mut b).1, "Stop", "the three before were dropped");
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(Layout::from_home(home.path()).hook_socket()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_reconnecting_bridge_gets_the_hooks_it_missed() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        let Some(DaemonMsg::Welcome { epoch, .. }) = read_frame::<DaemonMsg>(&mut b).unwrap() else { panic!() };
        assert!(epoch > 0);
        login(home.path(), "t-1", std::process::id());
        send_hook(home.path(), Some("t-1"), false, "One");
        b.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let (seen, _) = next_hook(&mut b);
        drop(b);
        // `send_hook` returns once its line is written, and the daemon takes connections in accept order, so Two is
        // always seq+1 and Three seq+2; wait for them by polling the Welcome's seq instead of sleeping.
        send_hook(home.path(), Some("t-1"), false, "Two");
        send_hook(home.path(), Some("t-1"), false, "Three");
        wait_for_seq(home.path(), "0.1.0-aaaaaaaa", seen + 2);
        let mut b = bridge_at(home.path(), "0.1.0-aaaaaaaa", seen, Some(epoch));
        b.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let Some(DaemonMsg::Welcome { gap, .. }) = read_frame::<DaemonMsg>(&mut b).unwrap() else { panic!() };
        assert!(!gap);
        assert_eq!(next_hook(&mut b).1, "Two");
        assert_eq!(next_hook(&mut b).1, "Three");
        let mut other = bridge_at(home.path(), "0.1.0-aaaaaaaa", seen, Some(epoch + 1));
        let Some(DaemonMsg::Welcome { gap, .. }) = read_frame::<DaemonMsg>(&mut other).unwrap() else { panic!() };
        assert!(gap, "another epoch: a gap");
    }

    #[test]
    fn a_probe_registers_nothing_and_gets_no_replay() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        login(home.path(), "t-1", std::process::id());
        send_hook(home.path(), Some("t-1"), false, "One");
        wait_for_seq(home.path(), "0.1.0-aaaaaaaa", 2);
        let probe = |build: &str| {
            let mut s = UnixStream::connect(Layout::from_home(home.path()).socket()).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            write_frame(&mut s, &LocalMsg::Probe { build_id: build.into() }).unwrap();
            let reply = read_frame::<LocalReply>(&mut s).unwrap();
            let more = read_frame::<DaemonMsg>(&mut s).unwrap();
            (reply, more)
        };
        assert_eq!(probe("0.1.0-aaaaaaaa"), (Some(LocalReply::Ok), None), "Ok, then EOF: no Welcome, no replayed hook");
        assert_eq!(probe("0.2.0-bbbbbbbb"), (None, None), "another build: closed without a reply");
        // ensure_daemon uses the probe: with a daemon of this build serving it never tries to start one.
        let layout = Layout::from_home(home.path());
        crate::client::ensure_daemon(&layout, std::path::Path::new("/nonexistent/gilvt-remote"), "0.1.0-aaaaaaaa").unwrap();
    }

    #[test]
    fn a_probe_leaves_the_bridge_table_alone() {
        let home = tempfile::tempdir().unwrap();
        let cfg = cfg(home.path(), "0.1.0-aaaaaaaa");
        let state = Arc::new(Mutex::new(State::new(Links::default())));
        let (mut client, server) = UnixStream::pair().unwrap();
        write_frame(&mut client, &LocalMsg::Probe { build_id: "0.1.0-aaaaaaaa".into() }).unwrap();
        serve(server, &state, &cfg, &Arc::new(HistoryWorker::new(Arc::new(|| Ok(Vec::new()))))).unwrap();
        assert_eq!(read_frame::<LocalReply>(&mut client).unwrap(), Some(LocalReply::Ok));
        let s = state.lock().unwrap();
        assert!(s.bridges.is_empty() && s.next_bridge == 0);
    }

    #[test]
    fn hooks_of_an_ended_link_are_not_replayed() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let mut shell = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        login(home.path(), "t-live", std::process::id());
        login(home.path(), "t-gone", shell.id());
        send_hook(home.path(), Some("t-gone"), false, "Gone");
        send_hook(home.path(), Some("t-live"), false, "Live");
        wait_for_seq(home.path(), "0.1.0-aaaaaaaa", 4);
        shell.kill().unwrap();
        shell.wait().unwrap();
        // Until the reaper has ended t-gone (seen in a Welcome's links).
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
            b.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            if let Some(DaemonMsg::Welcome { links, .. }) = read_frame::<DaemonMsg>(&mut b).unwrap() {
                if links.iter().all(|l| l.link != "t-gone") {
                    break;
                }
            }
            assert!(Instant::now() < end, "t-gone was never reaped");
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        b.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        assert!(matches!(read_frame::<DaemonMsg>(&mut b).unwrap(), Some(DaemonMsg::Welcome { .. })));
        let mut replayed = Vec::new();
        while let Ok(Some(msg)) = read_frame::<DaemonMsg>(&mut b) {
            if let DaemonMsg::Event { event: RemoteEvent::Hook { event, .. }, .. } = msg {
                replayed.push(event);
            }
        }
        assert_eq!(replayed, ["Live"]);
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
    fn bridge_is_deregistered_when_its_connection_breaks() {
        let home = tempfile::tempdir().unwrap();
        let cfg = cfg(home.path(), "0.1.0-aaaaaaaa");
        let state = Arc::new(Mutex::new(State::new(Links::default())));
        let (mut client, server) = UnixStream::pair().unwrap();
        let st = state.clone();
        let h = std::thread::spawn(move || serve(server, &st, &cfg, &Arc::new(HistoryWorker::new(Arc::new(|| Ok(Vec::new()))))));
        write_frame(&mut client, &LocalMsg::Bridge { hello: AppMsg::Hello { build_id: "0.1.0-aaaaaaaa".into(), app_instance: "t".into(), cursor: 0, epoch: None } }).unwrap();
        assert!(matches!(read_frame::<DaemonMsg>(&mut client).unwrap(), Some(DaemonMsg::Welcome { .. })));
        assert_eq!(state.lock().unwrap().bridges.len(), 1);
        client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        // a valid length prefix followed by a body that is not JSON
        use std::io::Write;
        client.write_all(&5u32.to_be_bytes()).unwrap();
        client.write_all(b"xxxxx").unwrap();
        assert!(h.join().unwrap().is_err(), "garbage ends the connection with an error");
        assert!(state.lock().unwrap().bridges.is_empty(), "the bridge is deregistered");
        assert_eq!(read_frame::<DaemonMsg>(&mut client).unwrap(), None, "the peer sees EOF");
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

    #[test]
    fn follow_streams_appends_only_to_its_bridge_and_unfollow_stops() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let t = home.path().join(".claude/projects/p/s.jsonl");
        std::fs::create_dir_all(t.parent().unwrap()).unwrap();
        std::fs::write(&t, "{\"n\":1}\n").unwrap();
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        b.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        read_frame::<DaemonMsg>(&mut b).unwrap(); // welcome
        write_frame(&mut b, &AppMsg::Request { id: 1, req: RemoteRequest::TranscriptFollow { agent: "claude".into(), session_id: "s".into(), path: t.clone(), from: 0 } }).unwrap();
        assert_eq!(read_frame::<DaemonMsg>(&mut b).unwrap(), Some(DaemonMsg::Response { id: 1, resp: RemoteResponse::Following { size: 8 } }), "the answer comes before any push");
        match read_frame::<DaemonMsg>(&mut b).unwrap().unwrap() {
            DaemonMsg::TranscriptAppend { offset: 0, bytes, .. } => assert_eq!(decode_bytes(&bytes).unwrap(), b"{\"n\":1}\n"),
            other => panic!("{other:?}"),
        }
        write_frame(&mut b, &AppMsg::Request { id: 2, req: RemoteRequest::TranscriptFollow { agent: "claude".into(), session_id: "x".into(), path: "/etc/passwd".into(), from: 0 } }).unwrap();
        assert!(matches!(read_frame::<DaemonMsg>(&mut b).unwrap(), Some(DaemonMsg::Response { id: 2, resp: RemoteResponse::Error { .. } })));
        write_frame(&mut b, &AppMsg::Request { id: 3, req: RemoteRequest::TranscriptUnfollow { agent: "claude".into(), session_id: "s".into() } }).unwrap();
        assert_eq!(read_frame::<DaemonMsg>(&mut b).unwrap(), Some(DaemonMsg::Response { id: 3, resp: RemoteResponse::Ok }));
        std::fs::OpenOptions::new().append(true).open(&t).unwrap().write_all(b"{\"n\":2}\n").unwrap();
        b.set_read_timeout(Some(Duration::from_millis(800))).unwrap();
        assert!(read_frame::<DaemonMsg>(&mut b).is_err(), "nothing more after unfollow");
    }

    #[test]
    fn history_answers_a_list() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        b.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        read_frame::<DaemonMsg>(&mut b).unwrap();
        write_frame(&mut b, &AppMsg::Request { id: 5, req: RemoteRequest::History }).unwrap();
        assert!(matches!(read_frame::<DaemonMsg>(&mut b).unwrap(), Some(DaemonMsg::Response { id: 5, resp: RemoteResponse::HistoryList { .. } })));
    }
}
