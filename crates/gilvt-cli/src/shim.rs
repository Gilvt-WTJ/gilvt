//! `gilvt shim <name> [args…]`: what a PATH shim runs inside a gilvt pane (agent-shims spec §3.3).
//! Never stops the agent from starting: every failure falls back to exec'ing the real program unchanged.

use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode};

use gilvt_client::shim::{self, Kind, ShimEnv};

/// `args` come from `args_os`: a wrapper may pass arguments that are not UTF-8, and they reach the agent unchanged.
pub fn run(args: &[OsString]) -> ExitCode {
    let Some((name_os, rest)) = args.split_first() else {
        eprintln!("usage: gilvt shim <name> [args…]");
        return ExitCode::from(2);
    };
    // Shims are only installed for plain ASCII names (`name_ok`), so a lossy name only matters for messages.
    let name = &*name_os.to_string_lossy();
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let Some(real) = shim::find_real(name, &path_var) else {
        eprintln!("{name}: command not found");
        return ExitCode::from(127);
    };
    let env = shim_env();
    let tty = |fd| unsafe { libc::isatty(fd) == 1 };
    let mut argv: Vec<OsString> = rest.to_vec();
    let mut cmd = Command::new(&real);
    cmd.env(shim::ENV_SHIMMED, "1");
    if let Some(kind) = shim::decide(&env, name, tty(0), tty(1)) {
        // The codex trust lookup finds `codex` on PATH: it must see the real one, not this shim.
        if let Some(dir) = std::env::var_os(shim::ENV_SHIMS_DIR) {
            std::env::set_var("PATH", strip_dir(&path_var, Path::new(&dir)));
        }
        let cfg = crate::hook::config();
        argv = with_placeholders(rest, |user| match kind {
            Kind::Claude => gilvt_client::hook::claude::args_for(&cfg.gilvt, user),
            Kind::Codex => gilvt_client::hook::codex::args_for(&cfg.gilvt, cfg.state_dir.as_deref(), user),
        });
        std::env::set_var("PATH", &path_var);
        cmd.env(shim::ENV_HOOKED, "1");
    }
    // `argv[0]` stays the name the caller used, as the shell would have run it.
    let err = cmd.arg0(name_os).args(&argv).exec();
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

/// Runs `rewrite` (which works on `String`s) over `user`, each argument that is not UTF-8 standing in as a
/// placeholder no real argument can equal (it holds a NUL) and put back afterwards. If a placeholder ends up
/// inside a longer argument, the rewrite is dropped and `user` is returned unchanged (no hooks, but it starts).
fn with_placeholders(user: &[OsString], rewrite: impl FnOnce(&[String]) -> Vec<String>) -> Vec<OsString> {
    let mark = |i: usize| format!("\0gilvt-shim-arg-{i}\0");
    let strings: Vec<String> = user.iter().enumerate().map(|(i, a)| a.to_str().map_or_else(|| mark(i), str::to_string)).collect();
    let mut out = Vec::new();
    for s in rewrite(&strings) {
        match user.iter().enumerate().find(|&(i, a)| a.to_str().is_none() && s == mark(i)) {
            Some((_, a)) => out.push(a.clone()),
            None if s.contains('\0') => return user.to_vec(),
            None => out.push(OsString::from(s)),
        }
    }
    out
}

/// `path_var` without any entry equal to `dir`.
pub(crate) fn strip_dir(path_var: &OsStr, dir: &Path) -> OsString {
    std::env::join_paths(std::env::split_paths(path_var).filter(|p| p != dir)).unwrap_or_else(|_| path_var.to_os_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_utf8_arguments_survive_the_rewrite() {
        use std::os::unix::ffi::OsStringExt;
        let bad = OsString::from_vec(vec![b'a', 0xff, b'b']);
        let user = vec![OsString::from("-p"), bad.clone()];
        let out = with_placeholders(&user, |u| {
            let mut v = vec!["--settings".to_string(), "/s.json".to_string()];
            v.extend_from_slice(u);
            v
        });
        assert_eq!(out, vec![OsString::from("--settings"), OsString::from("/s.json"), OsString::from("-p"), bad.clone()]);
        // A rewrite that glues the placeholder into another argument is not used.
        let glued = with_placeholders(&user, |u| vec![format!("--x={}", u[1])]);
        assert_eq!(glued, user);
        // All UTF-8: the rewrite is taken as is.
        let ok = vec![OsString::from("hi")];
        assert_eq!(with_placeholders(&ok, |u| [&["--a".to_string()][..], u].concat()), vec![OsString::from("--a"), OsString::from("hi")]);
    }

    #[test]
    fn strip_dir_removes_every_copy() {
        let p = OsStr::new("/s:/usr/bin:/s:/bin");
        assert_eq!(strip_dir(p, Path::new("/s")), OsString::from("/usr/bin:/bin"));
    }
}
