//! `gilvt-remote bridge`: the app's side channel (spec §3.2 step 6). Ensures a daemon of this build,
//! then copies bytes stdin → socket and socket → stdout until either side closes.

use std::io::{self, Read, Write};
use std::process::ExitCode;

use crate::{client, paths};

pub fn run() -> ExitCode {
    let Some(exe) = std::env::current_exe().ok() else { return ExitCode::from(1) };
    let (Some(build), Some(layout)) = (paths::build_id_of(&exe), paths::Layout::current()) else { return ExitCode::from(1) };
    if let Err(e) = client::ensure_daemon(&layout, &exe, &build) {
        eprintln!("gilvt-remote bridge: {e}");
        return ExitCode::from(1);
    }
    let Ok(sock) = client::connect(&layout, || client::spawn_daemon(&exe)) else { return ExitCode::from(1) };
    let Ok(mut up) = sock.try_clone() else { return ExitCode::from(1) };
    // Detached on purpose: when the socket side ends we exit even if stdin is still open.
    std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut up);
        let _ = up.shutdown(std::net::Shutdown::Write);
    });
    let mut down = sock;
    let mut out = io::stdout().lock();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match down.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if out.write_all(&buf[..n]).and_then(|_| out.flush()).is_err() {
                    break;
                }
            }
        }
    }
    ExitCode::SUCCESS
}
