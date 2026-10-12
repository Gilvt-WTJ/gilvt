//! `gilvt-remote` called as `gilvt` (the `<build>/bin/gilvt` link, spec R2a §3.2): the `gilvt` the shell wrappers
//! and the hook commands run on this host. R2a's second plan adds `view` and `diff`.

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
        _ => {
            eprintln!("gilvt: {}", unsupported());
            ExitCode::from(1)
        }
    }
}
