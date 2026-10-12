//! Which machine a path or session is on (spec §6). Panes since R1, sessions since R2a.

use std::path::PathBuf;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPath {
    pub host: Host,
    pub path: PathBuf,
}
