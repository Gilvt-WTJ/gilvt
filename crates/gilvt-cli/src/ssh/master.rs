//! Starting the ControlMaster in its own process group (spec §3.2 step 3): ssh asks for passwords on
//! the terminal, so its group owns the terminal during auth; afterwards it is not in the foreground
//! group, so closing the pane mid-session does not SIGHUP the master's ProxyJump child (spike, §10).

use std::io;
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn running(ssh: &str, control: &Path, opts: &[String], dest: &str) -> bool {
    Command::new(ssh)
        .arg("-O")
        .arg("check")
        .arg("-o")
        .arg(format!("ControlPath={}", control.display()))
        .args(opts)
        .arg(dest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// A control socket left behind by a master that died (SIGKILL, crash) makes the next master fail to
/// bind — non-fatally: it still authenticates and backgrounds as a `-N` connection that never exits.
/// Remove it first. Only called on our own 0700 control dir. Returns whether something was removed.
pub fn clear_stale(control: &Path, running: bool) -> io::Result<bool> {
    if running || std::fs::symlink_metadata(control).is_err() {
        return Ok(false);
    }
    std::fs::remove_file(control)?;
    Ok(true)
}

/// gilvt's `-o` come first: for ssh the first value of an option wins, so they beat the user's own
/// ControlMaster / ControlPath in `~/.ssh/config` and on the command line (Review Focus 1).
pub fn master_args(control: &Path, opts: &[String], dest: &str) -> Vec<String> {
    let mut v: Vec<String> = ["-o", "ControlMaster=yes", "-o"].iter().map(|s| s.to_string()).collect();
    v.push(format!("ControlPath={}", control.display()));
    v.extend(["-o", "ControlPersist=60", "-f", "-N"].iter().map(|s| s.to_string()));
    v.extend(opts.iter().cloned());
    v.push(dest.to_string());
    v
}

/// Runs the master until it has authenticated and forked into the background; returns ssh's exit code.
pub fn start(ssh: &str, control: &Path, opts: &[String], dest: &str) -> io::Result<i32> {
    let tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").ok();
    let mut cmd = Command::new(ssh);
    cmd.args(master_args(control, opts, dest));
    if tty.is_some() {
        // SAFETY: setpgid is async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
    }
    let mut child = cmd.spawn()?;
    let Some(t) = &tty else { return Ok(child.wait()?.code().unwrap_or(255)) };
    let pid = child.id() as libc::pid_t;
    let fd = t.as_raw_fd();
    // SAFETY: plain libc calls on our own process group, our child and our controlling terminal.
    // SIGTTOU is ignored while we hand the terminal back and forth: a background tcsetpgrp would stop us.
    let (me, old) = unsafe { (libc::getpgrp(), libc::signal(libc::SIGTTOU, libc::SIG_IGN)) };
    unsafe {
        libc::setpgid(pid, pid); // races the child's own setpgid; either one suffices
        libc::tcsetpgrp(fd, pid);
    }
    let status = child.wait();
    unsafe {
        libc::tcsetpgrp(fd, me);
        libc::signal(libc::SIGTTOU, old);
    }
    Ok(status?.code().unwrap_or(255))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_stale_socket_is_removed_but_a_live_one_is_kept() {
        let d = tempfile::tempdir().unwrap();
        let ctl = d.path().join("cm-0011223344556677");
        assert!(!super::clear_stale(&ctl, false).unwrap(), "nothing there");
        std::fs::write(&ctl, b"").unwrap();
        assert!(!super::clear_stale(&ctl, true).unwrap(), "a running master keeps its socket");
        assert!(ctl.exists());
        assert!(super::clear_stale(&ctl, false).unwrap());
        assert!(!ctl.exists());
        std::os::unix::fs::symlink("/nonexistent", &ctl).unwrap();
        assert!(super::clear_stale(&ctl, false).unwrap(), "a dangling symlink counts as stale");
        assert!(std::fs::symlink_metadata(&ctl).is_err());
    }

    #[test]
    fn gilvt_options_come_before_the_users() {
        let opts: Vec<String> = ["-o", "ControlPath=/mine/%C", "-o", "ControlMaster=no", "-J", "jump"].iter().map(|s| s.to_string()).collect();
        let v = super::master_args(std::path::Path::new("/tmp/gilvt-501/cm-0011223344556677"), &opts, "devbox");
        let ours = v.iter().position(|a| a == "ControlPath=/tmp/gilvt-501/cm-0011223344556677").unwrap();
        let theirs = v.iter().position(|a| a == "ControlPath=/mine/%C").unwrap();
        assert!(ours < theirs, "first -o wins in ssh");
        assert!(v.iter().position(|a| a == "ControlMaster=yes").unwrap() < v.iter().position(|a| a == "ControlMaster=no").unwrap());
        assert_eq!(v.last().unwrap(), "devbox");
        assert!(v.contains(&"-J".to_string()), "ProxyJump passes through");
    }
}
