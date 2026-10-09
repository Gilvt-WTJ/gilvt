//! Which machine a path or session is on (spec §6). R1 only tags panes; sessions follow in R2.

use std::path::PathBuf;

/// `user@hostname:port` from `ssh -G` (spec §3.2 step 2).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HostId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Host {
    Local,
    Remote(HostId),
}

impl Host {
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
