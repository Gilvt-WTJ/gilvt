//! `gilvt shim <name> [args…]`: what a PATH shim runs inside a gilvt pane (agent-shims spec §3.3).
//! Never stops the agent from starting: every failure falls back to exec'ing the real program unchanged.

use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode};

use gilvt_client::shim::{self, Kind, ShimEnv};

pub fn run(args: &[String]) -> ExitCode {
    let Some((name, rest)) = args.split_first() else {
        eprintln!("usage: gilvt shim <name> [args…]");
        return ExitCode::from(2);
    };
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let Some(real) = shim::find_real(name, &path_var) else {
        eprintln!("{name}: command not found");
        return ExitCode::from(127);
    };
    let env = shim_env();
    let tty = |fd| unsafe { libc::isatty(fd) == 1 };
    let mut argv: Vec<String> = rest.to_vec();
    let mut cmd = Command::new(&real);
    if let Some(kind) = shim::decide(&env, name, tty(0), tty(1)) {
        // The codex trust lookup finds `codex` on PATH: it must see the real one, not this shim.
        if let Some(dir) = std::env::var_os(shim::ENV_SHIMS_DIR) {
            std::env::set_var("PATH", strip_dir(&path_var, Path::new(&dir)));
        }
        let cfg = crate::hook::config();
        argv = match kind {
            Kind::Claude => gilvt_client::hook::claude::args_for(&cfg.gilvt, rest),
            Kind::Codex => gilvt_client::hook::codex::args_for(&cfg.gilvt, cfg.state_dir.as_deref(), rest),
        };
        std::env::set_var("PATH", &path_var);
        cmd.env(shim::ENV_HOOKED, "1");
    }
    // `argv[0]` stays the name the caller used, as the shell would have run it.
    let err = cmd.arg0(name).args(&argv).exec();
    eprintln!("{name}: {err}");
    ExitCode::from(126)
}

fn shim_env() -> ShimEnv {
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    let list = |k: &str| std::env::var(k).map(|v| v.split(' ').filter(|s| !s.is_empty()).map(str::to_string).collect()).unwrap_or_default();
    ShimEnv {
        in_gilvt: set(gilvt_ipc::ENV_SOCKET) && set(gilvt_ipc::ENV_PANE),
        disabled: set(shim::ENV_NO_WRAPPERS),
        hooked: set(shim::ENV_HOOKED),
        claude: list("GILVT_CLAUDE_COMMANDS"),
        codex: list("GILVT_CODEX_COMMANDS"),
    }
}

/// `path_var` without any entry equal to `dir`.
pub(crate) fn strip_dir(path_var: &OsStr, dir: &Path) -> OsString {
    std::env::join_paths(std::env::split_paths(path_var).filter(|p| p != dir)).unwrap_or_else(|_| path_var.to_os_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_dir_removes_every_copy() {
        let p = OsStr::new("/s:/usr/bin:/s:/bin");
        assert_eq!(strip_dir(p, Path::new("/s")), OsString::from("/usr/bin:/bin"));
    }
}
