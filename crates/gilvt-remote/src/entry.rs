//! `gilvt-remote` called as `gilvt` (the `<build>/bin/gilvt` link, spec R2a §3.2): the `gilvt` the shell wrappers
//! and the hook commands run on this host: `gilvt hook …` and `view` / `diff` (R2a §8.2).

use std::process::ExitCode;

use gilvt_client::hook::HookConfig;

use crate::paths::{self, Layout};

pub fn unsupported() -> &'static str {
    gilvt_i18n::text("远端不支持这个命令", "this command is not available on the remote host")
}

/// The remote values for `gilvt hook`: the link path in hook commands, `~/.gilvt-server/state`, no leases.
pub fn hook_config(layout: &Layout, build_id: &str) -> HookConfig {
    HookConfig { gilvt: layout.gilvt_link(build_id), state_dir: Some(layout.state_dir()), lease: false, remote: true }
}

pub fn run(args: &[String]) -> ExitCode {
    // The login shell carries the window's language (`GILVT_LANGUAGE`, as for the Mac's `gilvt`).
    if let Some(language) = gilvt_i18n::language_from_env() {
        gilvt_i18n::set_current(language);
    }
    match args.first().map(String::as_str) {
        Some("hook") => {
            // Resolve the `bin/gilvt` link: on macOS (where these tests also run) `current_exe()` is the link's own
            // path, whose parent is `bin/`, not the build directory; on Linux it is already resolved.
            let exe = std::env::current_exe().ok().map(|e| e.canonicalize().unwrap_or(e));
            let build = exe.as_deref().and_then(paths::build_id_of);
            match (build, Layout::current()) {
                (Some(build), Some(layout)) => gilvt_client::hook::run(&args[1..], &hook_config(&layout, &build)),
                // Not installed the usual way: still silent, still 0 (a hook must never disturb the agent).
                _ => ExitCode::SUCCESS,
            }
        }
        Some("view") | Some("diff") => view(args),
        _ => {
            eprintln!("gilvt: {}", unsupported());
            ExitCode::from(1)
        }
    }
}

/// `gilvt view` / `gilvt diff` on the ssh host (R2a §8.2): checked here, against this host's files, then asked of
/// `run/gilvt.sock` with this shell's link. The app answers by opening a preview of the remote file.
fn view(args: &[String]) -> ExitCode {
    use gilvt_client::view::{args::parse, not_linked_text, open_error_text, read_stdin, request_for};
    let cmd = match parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("gilvt: {e}\n{REMOTE_USAGE}");
            return ExitCode::from(2);
        }
    };
    // Help is answered before looking for the link: usage works in any shell.
    if matches!(cmd, gilvt_client::view::args::Command::Help) {
        print!("{REMOTE_USAGE}");
        return ExitCode::SUCCESS;
    }
    let fail = |msg: &str| {
        eprintln!("gilvt: {msg}");
        ExitCode::from(1)
    };
    let (Some(socket), Some(link)) = (std::env::var_os("GILVT_SOCKET"), std::env::var("GILVT_LINK").ok()) else { return fail(not_linked_text()) };
    let Ok(cwd) = std::env::current_dir() else { return fail("current directory is gone") };
    let req = match request_for(&cmd, &cwd, || read_stdin(std::io::stdin()), None, Some(link)) {
        Ok(Some(req)) => req,
        Ok(None) => {
            print!("{REMOTE_USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => return fail(&e),
    };
    // A line the daemon cannot read whole (10 MB of stdin can grow past 16 MB as JSON) is refused here, not cut off there.
    if serde_json::to_vec(&req).map_or(true, |v| v.len() as u64 + 256 > gilvt_ipc::MAX_MESSAGE_BYTES) {
        return fail(&open_error_text(gilvt_ipc::remote::OPEN_TOO_LARGE));
    }
    match gilvt_ipc::send(std::path::Path::new(&socket), &req) {
        Ok(gilvt_ipc::Response::Ok) => ExitCode::SUCCESS,
        Ok(gilvt_ipc::Response::Error { message }) => fail(&open_error_text(&message)),
        Ok(_) | Err(_) => fail(not_linked_text()),
    }
}

const REMOTE_USAGE: &str = "\
usage:
  gilvt view [--pin] [--as TYPE] <file>[:line[:col]]   preview a file of this host in gilvt
  gilvt view --as TYPE -                             preview standard input
  gilvt diff [REV]                                   preview changes against REV (default HEAD)
";
