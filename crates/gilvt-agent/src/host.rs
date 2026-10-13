//! Which machine a path or session is on (spec §6). Panes since R1, sessions since R2a.

use std::path::{Path, PathBuf};

/// `user@hostname:port` from `ssh -G` (spec §3.2 step 2).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct HostId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Host {
    Local,
    Remote(HostId),
}

impl Host {
    pub fn is_remote(&self) -> bool {
        matches!(self, Host::Remote(_))
    }

    pub fn id_str(&self) -> &str {
        match self {
            Host::Local => "local",
            Host::Remote(h) => &h.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HostPath {
    pub host: Host,
    pub path: PathBuf,
}

impl HostPath {
    pub fn local(path: impl Into<PathBuf>) -> HostPath {
        HostPath { host: Host::Local, path: path.into() }
    }

    pub fn remote(host: impl Into<String>, path: impl Into<PathBuf>) -> HostPath {
        HostPath { host: Host::Remote(HostId(host.into())), path: path.into() }
    }

    /// The path on this Mac; None for a remote file (whose path must never be opened here).
    pub fn local_path(&self) -> Option<&Path> {
        matches!(self.host, Host::Local).then_some(self.path.as_path())
    }

    pub fn remote_host(&self) -> Option<&str> {
        match &self.host {
            Host::Remote(h) => Some(&h.0),
            Host::Local => None,
        }
    }

    pub fn file_name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.path.display().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_path_knows_which_machine_it_is_on() {
        let l = HostPath::local("/etc/hosts");
        let r = HostPath::remote("dev@devbox:22", "/etc/hosts");
        assert_eq!((l.local_path(), l.remote_host()), (Some(Path::new("/etc/hosts")), None));
        assert_eq!((r.local_path(), r.remote_host()), (None, Some("dev@devbox:22")));
        assert_ne!(l, r, "the same path on two machines is two files");
        assert_eq!(r.file_name(), "hosts");
    }
}
