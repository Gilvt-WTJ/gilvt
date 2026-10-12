//! `gilvt hook …` on the Mac. The implementation is shared with the ssh host's `gilvt` (`gilvt_client::hook`);
//! this module supplies the Mac's values: this binary's path, the app's state directory, leases.

use std::path::PathBuf;
use std::process::ExitCode;

pub use gilvt_client::hook::{claude, codex, shell_quote, HookConfig};

/// The absolute path of this `gilvt` binary: the one place hooks learn what command to run.
pub fn gilvt_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gilvt"));
    exe.canonicalize().unwrap_or(exe)
}

/// The Mac's values: this binary, `~/Library/Application Support/gilvt/state` (the app's, via
/// `gilvt_agent::default_state_dir`), leases on.
pub fn config() -> HookConfig {
    HookConfig { gilvt: gilvt_path(), state_dir: gilvt_agent::default_state_dir(), lease: true, remote: false }
}

/// Entry point for `gilvt hook <args>`.
pub fn run(args: &[String]) -> ExitCode {
    gilvt_client::hook::run(args, &config())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mac_config_writes_leases_and_is_not_remote() {
        let cfg = config();
        assert!(cfg.lease && !cfg.remote);
        assert_eq!(cfg.state_dir, gilvt_agent::default_state_dir(), "the app's state directory");
        assert!(cfg.gilvt.is_absolute(), "{:?}", cfg.gilvt);
    }
}
