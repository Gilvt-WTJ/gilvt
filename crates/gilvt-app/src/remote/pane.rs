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
