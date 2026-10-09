//! `gilvt ssh` without a network: a fake `ssh` on PATH logs its arguments.

use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

fn fake_ssh(dir: &std::path::Path) {
    let p = dir.join("ssh");
    std::fs::write(&p, "#!/bin/sh\necho \"$*\" >> \"$LOG\"\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn run(args: &[&str], env: &[(&str, &str)]) -> String {
    let d = tempfile::tempdir().unwrap();
    fake_ssh(d.path());
    let log = d.path().join("log");
    let mut c = Command::new(env!("CARGO_BIN_EXE_gilvt"));
    c.arg("ssh").args(args).env("PATH", format!("{}:/usr/bin:/bin", d.path().display())).env("LOG", &log).env_remove("GILVT_SOCKET").stdin(Stdio::null());
    for (k, v) in env {
        c.env(k, v);
    }
    assert!(c.status().unwrap().success());
    std::fs::read_to_string(log).unwrap_or_default()
}

#[test]
fn non_interactive_and_opted_out_calls_pass_through() {
    assert_eq!(run(&["devbox", "uname"], &[]), "devbox uname\n");
    assert_eq!(run(&["devbox"], &[]), "devbox\n", "no GILVT_SOCKET");
    assert_eq!(run(&["devbox"], &[("GILVT_SOCKET", "/nonexistent"), ("GILVT_SSH", "0")]), "devbox\n");
}

#[test]
fn the_shell_wrappers_double_dash_is_not_forwarded() {
    assert_eq!(run(&["--", "devbox"], &[]), "devbox\n");
    assert_eq!(run(&["--", "-t", "devbox", "--", "tmux a"], &[]), "-t devbox -- tmux a\n");
}
