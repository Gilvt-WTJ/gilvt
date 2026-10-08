//! What a terminal pane knows about the ssh link it is in.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneRemote {
    pub link: String,
    /// HostId (`user@hostname:port`).
    pub host: String,
    /// What the user typed (`devbox`).
    pub display: String,
    /// The remote `gethostname()`: OSC 7 from this host carries it.
    pub hostname: Option<String>,
    /// gilvt-remote is installed and the login went through it.
    pub enhanced: bool,
}

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Same machine: equal ignoring case, or one is the other's short name.
pub fn same_host(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    a == b || a.starts_with(&format!("{b}.")) || b.starts_with(&format!("{a}."))
}

pub struct RemoteCwd {
    pub remote: PaneRemote,
    cwd: Option<PathBuf>,
    pending: Option<(String, PathBuf)>,
    first_strike: Option<Instant>,
}

impl RemoteCwd {
    pub fn new(remote: PaneRemote) -> Self {
        RemoteCwd { remote, cwd: None, pending: None, first_strike: None }
    }

    pub fn observe(&mut self, host: &str, path: PathBuf) {
        match self.remote.hostname.as_deref() {
            Some(h) if same_host(h, host) => self.cwd = Some(path),
            Some(_) => {}
            None => self.pending = Some((host.to_string(), path)),
        }
    }

    pub fn set_hostname(&mut self, hostname: Option<String>) {
        if hostname.is_none() { return; }
        self.remote.hostname = hostname;
        if let Some((host, path)) = self.pending.take() {
            self.observe(&host, path);
        }
    }

    pub fn cwd(&self) -> Option<&Path> { self.cwd.as_deref() }

    /// A non-ssh check has been seen: the pane must be checked again even if the terminal goes quiet.
    pub fn needs_recheck(&self) -> bool { self.first_strike.is_some() }

    /// `gilvt` (gilvt ssh before it execs) and `ssh` keep the link alive; two checks at least 500 ms apart
    /// without either end it.
    pub fn link_alive(&mut self, foreground: Option<&str>, now: Instant) -> bool {
        if matches!(foreground, Some("ssh") | Some("gilvt")) {
            self.first_strike = None;
            return true;
        }
        match self.first_strike {
            None => { self.first_strike = Some(now); true }
            Some(t) if now.duration_since(t) >= Duration::from_millis(500) => false,
            Some(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn pr(hostname: Option<&str>) -> PaneRemote {
        PaneRemote { link: "i-1".into(), host: "dev@10.0.0.1:22".into(), display: "devbox".into(), hostname: hostname.map(Into::into), enhanced: true }
    }

    #[test]
    fn osc7_from_the_linked_host_becomes_the_cwd() {
        let mut c = RemoteCwd::new(pr(Some("n37-026-177")));
        c.observe("n37-026-177", "/data00/home/u".into());
        assert_eq!(c.cwd(), Some(Path::new("/data00/home/u")));
        c.observe("N37-026-177.byted.org", "/tmp".into());
        assert_eq!(c.cwd(), Some(Path::new("/tmp")), "FQDN and case differences match");
        c.observe("thirdhost", "/elsewhere".into());
        assert_eq!(c.cwd(), Some(Path::new("/tmp")), "a further hop is not this link");
    }

    #[test]
    fn osc7_before_the_hostname_is_kept_until_it_arrives() {
        let mut c = RemoteCwd::new(pr(None));
        c.observe("devbox", "/home/dev".into());
        assert_eq!(c.cwd(), None);
        c.set_hostname(Some("devbox".into()));
        assert_eq!(c.cwd(), Some(Path::new("/home/dev")));
    }

    #[test]
    fn pending_from_another_host_is_dropped() {
        let mut c = RemoteCwd::new(pr(None));
        c.observe("other", "/x".into());
        c.set_hostname(Some("devbox".into()));
        assert_eq!(c.cwd(), None);
    }

    #[test]
    fn a_first_strike_asks_for_a_recheck() {
        let mut c = RemoteCwd::new(pr(Some("h")));
        let t = Instant::now();
        assert!(!c.needs_recheck());
        c.link_alive(Some("zsh"), t);
        assert!(c.needs_recheck());
        c.link_alive(Some("ssh"), t);
        assert!(!c.needs_recheck());
    }

    #[test]
    fn link_ends_after_two_checks_without_ssh() {
        let mut c = RemoteCwd::new(pr(Some("h")));
        let t = Instant::now();
        assert!(c.link_alive(Some("gilvt"), t));
        assert!(c.link_alive(Some("ssh"), t));
        assert!(c.link_alive(Some("zsh"), t), "one check is not enough");
        assert!(c.link_alive(Some("zsh"), t + Duration::from_millis(100)), "too soon");
        assert!(!c.link_alive(Some("zsh"), t + Duration::from_millis(600)));
        let mut c = RemoteCwd::new(pr(Some("h")));
        c.link_alive(Some("bash"), t);
        assert!(c.link_alive(Some("ssh"), t + Duration::from_millis(600)), "ssh again resets");
        assert!(c.link_alive(None, t + Duration::from_millis(700)), "unknown counts as not-ssh, first strike");
    }
}
