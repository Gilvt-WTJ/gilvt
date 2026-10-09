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

/// An exclusive `flock` on `<control>.lock` (0600, in the same 0700 dir), held across check → clear →
/// start → re-check so two panes opening the same host do not start two masters (the loser's `-f -N`
/// would linger, and its clear_stale could delete the winner's freshly bound socket). A second pane
/// blocks here — it is in the foreground running gilvt — then finds the first master and reuses it.
/// Released when the returned file is dropped; `None` when it cannot be taken (callers go on unlocked).
/// The fd is close-on-exec (Rust opens files with O_CLOEXEC), so no ssh we run inherits it.
pub fn lock(control: &Path) -> Option<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut p = control.as_os_str().to_owned();
    p.push(".lock");
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).mode(0o600).open(p).ok()?;
    loop {
        // SAFETY: flock on a descriptor we own.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Some(f);
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return None;
        }
    }
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
    // The foreground group to give the terminal back to is whatever owned it before the handoff.
    let (fg, old) = unsafe {
        let fg = libc::tcgetpgrp(fd);
        (if fg > 0 { fg } else { libc::getpgrp() }, libc::signal(libc::SIGTTOU, libc::SIG_IGN))
    };
    unsafe {
        libc::setpgid(pid, pid); // races the child's own setpgid; either one suffices
        libc::tcsetpgrp(fd, pid);
    }
    let result = wait_for_master(pid);
    unsafe {
        libc::tcsetpgrp(fd, fg);
        libc::signal(libc::SIGTTOU, old);
    }
    let waited = result?;
    if waited == Waited::Stopped {
        // Ctrl-Z at the password / host-key prompt: the stopped ssh group would own the terminal forever
        // while we wait. We have taken the terminal back above; end the half-authenticated master.
        // SAFETY: the group is our own child's; it is reaped right after.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
        let _ = child.wait();
        eprintln!("\r\ngilvt: {}", gilvt_i18n::text("ssh 被挂起，已取消这次连接", "ssh was suspended; this connection is cancelled"));
    }
    Ok(waited.code())
}

/// What became of the master while it held the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waited {
    /// Exited (the code) or was killed (255, as `ssh` reports a lost connection).
    Done(i32),
    /// Stopped by a job-control signal (Ctrl-Z at a prompt).
    Stopped,
}

impl Waited {
    pub fn code(self) -> i32 {
        match self {
            Waited::Done(c) => c,
            Waited::Stopped => 255,
        }
    }
}

/// Maps a `waitpid` status (with `WUNTRACED`) to a [`Waited`]; `None` for anything else (keep waiting).
pub fn classify(status: libc::c_int) -> Option<Waited> {
    if libc::WIFSTOPPED(status) {
        Some(Waited::Stopped)
    } else if libc::WIFEXITED(status) {
        Some(Waited::Done(libc::WEXITSTATUS(status)))
    } else if libc::WIFSIGNALED(status) {
        Some(Waited::Done(255))
    } else {
        None
    }
}

/// `waitpid(WUNTRACED)` until the master exits, dies or stops. A plain `wait` would block forever on a
/// stopped master. A stopped child is not reaped here: the caller kills and reaps it.
fn wait_for_master(pid: libc::pid_t) -> io::Result<Waited> {
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: waitpid on our own child with a valid status pointer.
        let r = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
        if r == -1 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if let Some(w) = classify(status) {
            return Ok(w);
        }
    }
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
    fn the_master_lock_is_exclusive_and_private() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::io::AsRawFd;
        let d = tempfile::tempdir().unwrap();
        let ctl = d.path().join("cm-0011223344556677");
        let held = super::lock(&ctl).expect("lock");
        let path = d.path().join("cm-0011223344556677.lock");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let other = std::fs::File::open(&path).unwrap();
        // SAFETY: flock on a descriptor we own.
        assert_ne!(unsafe { libc::flock(other.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0, "a second taker must wait");
        let fdflags = unsafe { libc::fcntl(held.as_raw_fd(), libc::F_GETFD) };
        assert_ne!(fdflags & libc::FD_CLOEXEC, 0, "the exec'd ssh must not inherit the lock");
        drop(held);
        assert_eq!(unsafe { libc::flock(other.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0, "dropping releases it");
        assert!(super::lock(&d.path().join("missing-dir/cm-x")).is_none(), "no lock → callers go on unlocked");
    }

    #[test]
    fn waitpid_statuses() {
        use super::{classify, Waited};
        // Encodings per <sys/wait.h> (macOS and Linux agree): exit code << 8; signal in the low 7 bits;
        // 0x7f in the low byte with the stop signal above it.
        assert_eq!(classify(0), Some(Waited::Done(0)));
        assert_eq!(classify(255 << 8), Some(Waited::Done(255)));
        assert_eq!(classify(1 << 8), Some(Waited::Done(1)));
        assert_eq!(classify(libc::SIGKILL), Some(Waited::Done(255)), "killed reads as a lost connection");
        let stopped = (libc::SIGTSTP << 8) | 0x7f;
        assert_eq!(classify(stopped), Some(Waited::Stopped));
        assert_eq!(Waited::Stopped.code(), 255, "run() then ends the link and exits");
    }

    #[test]
    fn a_stopped_child_is_seen_not_waited_on_forever() {
        // The real Ctrl-Z needs a terminal; a child that stops itself exercises the same waitpid path.
        let mut child = std::process::Command::new("/bin/sh").args(["-c", "kill -STOP $$"]).spawn().unwrap();
        let pid = child.id() as libc::pid_t;
        assert_eq!(super::wait_for_master(pid).unwrap(), super::Waited::Stopped);
        unsafe { libc::kill(pid, libc::SIGKILL) };
        child.wait().unwrap();
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
