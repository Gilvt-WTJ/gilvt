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
    let launch = shell_launch(&shell, integration.as_ref(), exec.as_deref(), |k| std::env::var(k).ok());
    let mut cmd = Command::new(&launch.program);
    cmd.args(&launch.args).envs(launch.env.iter().cloned()).env("TERM_PROGRAM", "gilvt").env_remove("GILVT_SOCKET").env_remove("GILVT_BIN_DIR");
    if let Some(a0) = &launch.argv0 {
        cmd.arg0(a0);
    }
    if let Some(l) = &link {
        cmd.env("GILVT_LINK", l);
    }
    if let Some(layout) = &layout {
        cmd.env("GILVT_REMOTE_BIN", layout.stable_bin());
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
