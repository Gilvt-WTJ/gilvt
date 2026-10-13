//! `gilvt-remote login --link L [--exec CMD]`: the remote command of the interactive ssh (spec §3.2 step 7).
//! Registers the link with the daemon, then becomes the user's shell with gilvt's integration.

use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

use gilvt_ipc::remote::LinkState;
use gilvt_shell::{shell_command, Integration, Launch};

use crate::{client, paths, sys};

pub fn shell_launch(shell: &str, integration: Option<&Integration>, exec: Option<&str>, getenv: impl Fn(&str) -> Option<String>) -> Launch {
    let name = std::path::Path::new(shell).file_name().and_then(|n| n.to_str()).unwrap_or("sh").to_string();
    let mut l = match integration {
        Some(i) => shell_command(shell, Some(i), getenv),
        None => Launch { program: shell.to_string(), argv0: None, args: Vec::new(), env: Vec::new() },
    };
    match exec {
        Some(cmd) => {
            // A command, not a login: drop --rcfile etc., keep the env (tmux started here passes it on).
            l.args = vec!["-c".into(), cmd.into()];
            l.argv0 = Some(name);
        }
        None if integration.is_some() && name == "bash" => {} // --rcfile; gilvt.bash emulates login startup
        None => l.argv0 = Some(format!("-{name}")),
    }
    l
}

fn login_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| gilvt_shell::user_and_shell().map(|(_, s)| s).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "/bin/sh".into())
}

/// The gilvt variables of the remote shell (R1 + R2a §3.3); `None` removes the variable. `bin` is
/// `<build>/bin` when `bin/gilvt` exists: without it the wrappers and hooks stay off.
pub fn gilvt_env(layout: &paths::Layout, build: Option<&str>, link: Option<&str>, bin: Option<&std::path::Path>) -> Vec<(&'static str, Option<String>)> {
    let on = build.and(bin);
    vec![
        ("TERM_PROGRAM", Some("gilvt".into())),
        ("GILVT_LINK", link.map(str::to_string)),
        ("GILVT_REMOTE_BIN", Some(layout.stable_bin().display().to_string())),
        ("GILVT_BIN_DIR", on.map(|b| b.display().to_string())),
        ("GILVT_SOCKET", on.map(|_| layout.hook_socket().display().to_string())),
        // Nested ssh is a non-goal: the remote shell's `ssh` is the real one.
        ("GILVT_SSH", Some("0".into())),
        ("GILVT_PANE_ID", None),
    ]
}

pub fn run(args: &[String]) -> ExitCode {
    let mut link = None;
    let mut exec = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--link" => link = it.next().cloned(),
            "--exec" => exec = it.next().cloned(),
            _ => return ExitCode::from(2),
        }
    }
    let shell = login_shell();
    let exe = std::env::current_exe().ok();
    let build = exe.as_deref().and_then(paths::build_id_of);
    let layout = paths::Layout::current();
    let mut integration = None;
    if let (Some(exe), Some(build), Some(layout)) = (&exe, &build, &layout) {
        match client::ensure_daemon(layout, exe, build) {
            Ok(()) => {
                if let Some(link) = &link {
                    let state = LinkState { link: link.clone(), hostname: sys::hostname(), tty: sys::tty(), pid: std::process::id() };
                    let _ = client::login(layout, build, state);
                }
            }
            Err(e) => eprintln!("gilvt: 远端 daemon 未启动（{e}）"),
        }
        integration = Integration::install(&layout.version_dir(build).join("shell-integration")).ok();
    }
    let bin = match (&build, &layout) {
        (Some(build), Some(layout)) => layout.ensure_gilvt_link(build).ok().map(|_| layout.bin_dir(build)),
        _ => None,
    };
    let launch = shell_launch(&shell, integration.as_ref(), exec.as_deref(), |k| std::env::var(k).ok());
    let mut cmd = Command::new(&launch.program);
    cmd.args(&launch.args).envs(launch.env.iter().cloned());
    if let Some(layout) = &layout {
        for (k, v) in gilvt_env(layout, build.as_deref(), link.as_deref(), bin.as_deref()) {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            };
        }
    } else {
        cmd.env("TERM_PROGRAM", "gilvt").env_remove("GILVT_SOCKET").env_remove("GILVT_BIN_DIR");
    }
    if let Some(a0) = &launch.argv0 {
        cmd.arg0(a0);
    }
    let err = cmd.exec();
    eprintln!("gilvt-remote: cannot start {shell}: {err}");
    ExitCode::from(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn integ() -> (tempfile::TempDir, Integration) {
        let d = tempfile::tempdir().unwrap();
        let i = Integration::install(d.path()).unwrap();
        (d, i)
    }

    #[test]
    fn the_shell_gets_the_wrapper_and_hook_variables() {
        let l = paths::Layout::from_home(std::path::Path::new("/home/dev"));
        let bin = l.bin_dir("0.3.0-aaaaaaaa");
        let env = gilvt_env(&l, Some("0.3.0-aaaaaaaa"), Some("i-1"), Some(&bin));
        let get = |k: &str| env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        assert_eq!(get("GILVT_BIN_DIR"), Some(Some("/home/dev/.gilvt-server/0.3.0-aaaaaaaa/bin".into())));
        assert_eq!(get("GILVT_SOCKET"), Some(Some("/home/dev/.gilvt-server/run/gilvt.sock".into())));
        assert_eq!(get("GILVT_SSH"), Some(Some("0".into())));
        assert_eq!(get("GILVT_LINK"), Some(Some("i-1".into())));
        assert_eq!(get("GILVT_PANE_ID"), Some(None), "removed: remote hooks are routed by link");
        assert_eq!(get("GILVT_NO_AGENT_WRAPPERS"), None, "the user's own setting is left alone");
        let none = gilvt_env(&l, Some("0.3.0-aaaaaaaa"), Some("i-1"), None);
        let get = |k: &str| none.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        assert_eq!((get("GILVT_BIN_DIR"), get("GILVT_SOCKET")), (Some(None), Some(None)), "no bin/gilvt: wrappers stay off");
    }

    #[test]
    fn bash_gets_rcfile_zsh_and_fish_are_login_shells() {
        let (_d, i) = integ();
        let b = shell_launch("/bin/bash", Some(&i), None, |_| None);
        assert!(b.args.windows(2).any(|w| w[0] == "--rcfile"), "{:?}", b.args);
        let z = shell_launch("/usr/bin/zsh", Some(&i), None, |_| None);
        assert_eq!(z.argv0.as_deref(), Some("-zsh"));
        assert!(z.env.iter().any(|(k, _)| k == "ZDOTDIR"));
        let f = shell_launch("/usr/bin/fish", Some(&i), None, |_| None);
        assert_eq!(f.argv0.as_deref(), Some("-fish"));
    }

    #[test]
    fn exec_runs_the_command_with_integration_env() {
        let (_d, i) = integ();
        let z = shell_launch("/usr/bin/zsh", Some(&i), Some("tmux a"), |_| None);
        assert_eq!(z.args, ["-c", "tmux a"]);
        assert_eq!(z.argv0.as_deref(), Some("zsh"));
        assert!(z.env.iter().any(|(k, _)| k == "ZDOTDIR"), "tmux's shells inherit the integration");
    }

    #[test]
    fn without_integration_it_is_a_plain_login_shell() {
        let l = shell_launch("/bin/bash", None, None, |_| None);
        assert_eq!(l.argv0.as_deref(), Some("-bash"));
        assert!(l.args.is_empty());
    }
}
