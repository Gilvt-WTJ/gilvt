//! `gilvt-remote`: the piece of gilvt that runs on an ssh host (spec §2.1). Subcommands:
//! `login`, `bridge`, `daemon`. Called as `gilvt` (the symlink in `<version>/bin/`) it will answer the
//! `gilvt hook|view|diff` commands (R2).

mod bridge;
mod client;
mod daemon;
mod links;
mod login;
mod paths;
mod peer;
mod sys;

use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("--build-id") => match std::env::current_exe().ok().and_then(|e| paths::build_id_of(&e)) {
            Some(id) => {
                println!("{id}");
                ExitCode::SUCCESS
            }
            None => ExitCode::from(1),
        },
        Some("daemon") => {
            let mut build = std::env::current_exe().ok().and_then(|e| paths::build_id_of(&e));
            let mut foreground = false;
            let mut it = argv[1..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--build-id" => build = it.next().cloned(),
                    "--foreground" => foreground = true,
                    _ => return ExitCode::from(2),
                }
            }
            let (Some(build_id), Some(layout)) = (build, paths::Layout::current()) else { return ExitCode::from(1) };
            if !foreground { unsafe { libc::setsid(); } }
            let cfg = daemon::Config { layout, build_id, hostname: sys::hostname(), uname: sys::uname(), reap_every: std::time::Duration::from_secs(1), idle_exit: std::time::Duration::from_secs(24 * 3600) };
            match daemon::run(cfg) { Ok(()) => ExitCode::SUCCESS, Err(e) => { eprintln!("gilvt-remote daemon: {e}"); ExitCode::from(1) } }
        }
        Some("login") => login::run(&argv[1..]),
        Some("bridge") => bridge::run(),
        _ => {
            eprintln!("usage: gilvt-remote login|bridge|daemon");
            ExitCode::from(2)
        }
    }
}
