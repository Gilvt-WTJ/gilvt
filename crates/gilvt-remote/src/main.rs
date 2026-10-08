//! `gilvt-remote`: the piece of gilvt that runs on an ssh host (spec §2.1). Subcommands:
//! `login`, `bridge`, `daemon`. Called as `gilvt` (the symlink in `<version>/bin/`) it will answer the
//! `gilvt hook|view|diff` commands (R2).

// Items here are used by the daemon, login and bridge (Tasks 3-4); until then they are dead code.
#[allow(dead_code)]
mod links;
#[allow(dead_code)]
mod paths;
#[allow(dead_code)]
mod peer;

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
        _ => {
            eprintln!("usage: gilvt-remote login|bridge|daemon");
            ExitCode::from(2)
        }
    }
}
