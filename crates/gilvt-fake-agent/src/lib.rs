//! `gilvt-fake-agent`: a scripted stand-in for the `claude` / `codex` CLIs in gilvt's GUI acceptance tests
//! (acceptance spec §4). Copied (see [`args::persona`]) as `claude` / `codex` on the
//! sandbox PATH, it follows a TOML [`scenario`] and produces what gilvt observes of the real CLIs: the
//! process name, the transcript / rollout files under `$HOME`, the hook commands of `--settings` (Claude)
//! or `-c hooks.<Event>=…` (Codex), and a minimal TUI in the pane.
//!
//! The [`engine`] is a state machine driven by an [`engine::Io`] (keys + clock), so tests run scenarios
//! headless and fast; `main.rs` plugs in the terminal.
//!
//! [`brain_chat`] is the 监控官 chat's fake brain (Claude stream-json turns, Codex app-server turns), which calls
//! gilvt's tools through [`mcp_client`].

pub mod args;
pub mod brain;
pub mod brain_chat;
pub mod claude;
pub mod clock;
pub mod codex;
pub mod engine;
pub mod hooks;
pub mod keys;
pub mod mcp_client;
pub mod recorder;
pub mod scenario;
pub mod tui;

/// Which CLI the process pretends to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    /// `"claude"` / `"codex"`.
    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    /// Inverse of [`Agent::name`].
    pub fn from_name(name: &str) -> Option<Agent> {
        match name {
            "claude" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            _ => None,
        }
    }
}

/// Version strings the fake reports (and writes into records), in the real CLIs' formats.
pub const CLAUDE_VERSION: &str = "2.1.284";
pub const CODEX_VERSION: &str = "0.145.0";
