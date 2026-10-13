//! Reaching the daemon from `login` / `bridge`: connect, else start one and retry (spec §7).

use std::io;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gilvt_ipc::remote::*;

use crate::paths::Layout;

pub fn connect(layout: &Layout, spawn: impl Fn() -> io::Result<()>) -> io::Result<UnixStream> {
    if let Ok(s) = UnixStream::connect(layout.socket()) { return Ok(s); }
    spawn()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match UnixStream::connect(layout.socket()) {
            Ok(s) => return Ok(s),
            Err(e) if Instant::now() > deadline => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(30)),
        }
    }
}

/// Starts `exe daemon` detached (new session, stdio to /dev/null).
pub fn spawn_daemon(exe: &Path) -> io::Result<()> {
    let mut cmd = Command::new(exe);
    cmd.arg("daemon").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe.
    unsafe { cmd.pre_exec(|| { libc::setsid(); Ok(()) }); }
    cmd.spawn().map(|_| ())
}

/// Ensures a daemon of `build_id` serves: a daemon of another build is replaced (spec §7, 升级交接).
pub fn ensure_daemon(layout: &Layout, exe: &Path, build_id: &str) -> io::Result<()> {
    if probe(layout, build_id) {
        return Ok(());
    }
    spawn_daemon(exe)?;
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if probe(layout, build_id) {
            return Ok(());
        }
        if Instant::now() > deadline { return Err(io::Error::other("daemon did not come up")); }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Is a daemon of `build_id` serving? A `Probe`, not a bridge hello: the daemon registers nothing and replays no
/// hooks to it (R2a §9). A daemon of another build (or one too old to know `Probe`) closes without `Ok`.
fn probe(layout: &Layout, build_id: &str) -> bool {
    let Ok(mut s) = UnixStream::connect(layout.socket()) else { return false };
    if s.set_read_timeout(Some(Duration::from_secs(2))).is_err() || write_frame(&mut s, &LocalMsg::Probe { build_id: build_id.into() }).is_err() {
        return false;
    }
    matches!(read_frame::<LocalReply>(&mut s), Ok(Some(LocalReply::Ok)))
}

pub fn login(layout: &Layout, build_id: &str, link: LinkState) -> io::Result<()> {
    let mut s = UnixStream::connect(layout.socket())?;
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    write_frame(&mut s, &LocalMsg::Login { build_id: build_id.into(), link })?;
    read_frame::<LocalReply>(&mut s).map(|_| ())
}
