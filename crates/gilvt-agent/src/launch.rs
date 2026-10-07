//! The command lines gilvt types into a shell pane to start or resume an agent (spec §2.1, §3.4). Pure:
//! the app decides where to run them. Every argument is POSIX-quoted, so what the preview shows is
//! exactly what the shell receives.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::event::AgentKind;

/// `s` as one shell word: unchanged when it only has safe chars, else single-quoted (`'` → `'\''`).
/// Same rules as `gilvt-cli`'s `hook::shell_quote`.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@+%,".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Claude Code's `--permission-mode` values (CLI 2.1.284).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaudePermission {
    Manual,
    AcceptEdits,
    Plan,
    Auto,
    DontAsk,
    BypassPermissions,
}

impl ClaudePermission {
    /// In the order the UI lists them.
    pub const ALL: [ClaudePermission; 6] = [
        ClaudePermission::Manual,
        ClaudePermission::AcceptEdits,
        ClaudePermission::Plan,
        ClaudePermission::Auto,
        ClaudePermission::DontAsk,
        ClaudePermission::BypassPermissions,
    ];

    /// The `--permission-mode` value.
    pub fn mode(self) -> &'static str {
        match self {
            ClaudePermission::Manual => "manual",
            ClaudePermission::AcceptEdits => "acceptEdits",
            ClaudePermission::Plan => "plan",
            ClaudePermission::Auto => "auto",
            ClaudePermission::DontAsk => "dontAsk",
            ClaudePermission::BypassPermissions => "bypassPermissions",
        }
    }

    /// UI text: the raw mode name, as Claude Code itself shows it.
    pub fn label(self) -> &'static str {
        self.mode()
    }
}

/// Codex's sandbox + approval presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexPermission {
    ReadOnly,
    Auto,
    FullAccess,
}

impl CodexPermission {
    /// In the order the UI lists them.
    pub const ALL: [CodexPermission; 3] =
        [CodexPermission::ReadOnly, CodexPermission::Auto, CodexPermission::FullAccess];

    /// The `-s <sandbox> -a <approval>` arguments.
    pub fn args(self) -> [&'static str; 4] {
        match self {
            CodexPermission::ReadOnly => ["-s", "read-only", "-a", "on-request"],
            CodexPermission::Auto => ["-s", "workspace-write", "-a", "on-request"],
            CodexPermission::FullAccess => ["-s", "danger-full-access", "-a", "never"],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CodexPermission::ReadOnly => "只读",
            CodexPermission::Auto => "自动",
            CodexPermission::FullAccess => "完全访问",
        }
    }
}

/// A permission choice for either agent (`None` in [`NewAgent`] = follow the agent's own config).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Claude(ClaudePermission),
    Codex(CodexPermission),
}

impl Permission {
    /// The agent this choice applies to.
    pub fn agent(self) -> AgentKind {
        match self {
            Permission::Claude(_) => AgentKind::Claude,
            Permission::Codex(_) => AgentKind::Codex,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Permission::Claude(p) => p.label(),
            Permission::Codex(p) => p.label(),
        }
    }
}

/// What the 「新建 Agent」 panel asks for. `dir` is absolute (`~` already expanded by the caller).
#[derive(Clone, Debug, PartialEq)]
pub struct NewAgent {
    pub agent: AgentKind,
    pub dir: PathBuf,
    pub model: Option<String>,
    /// Ignored when it belongs to the other agent.
    pub permission: Option<Permission>,
    pub prompt: Option<String>,
}

/// `cd <dir> && <launch> [--model m | -m m] [permission flags] [<prompt>]`. A blank model or prompt is
/// omitted; the prompt (trimmed, newlines kept) is one argument, after `--` when it starts with `-` so
/// the CLI cannot take it for an option.
pub fn new_agent_command(launch: &str, a: &NewAgent) -> String {
    let mut args = vec![shell_quote(launch)];
    if let Some(model) = non_blank(a.model.as_deref()) {
        let flag = match a.agent {
            AgentKind::Claude => "--model",
            AgentKind::Codex => "-m",
        };
        args.extend([flag.to_string(), shell_quote(model)]);
    }
    match a.permission {
        Some(Permission::Claude(p)) if a.agent == AgentKind::Claude => {
            args.extend(["--permission-mode".to_string(), p.mode().to_string()]);
        }
        Some(Permission::Codex(p)) if a.agent == AgentKind::Codex => args.extend(p.args().map(str::to_string)),
        _ => {}
    }
    if let Some(prompt) = non_blank(a.prompt.as_deref()) {
        if prompt.starts_with('-') {
            args.push("--".to_string());
        }
        args.push(shell_quote(prompt));
    }
    in_dir(&a.dir, &args.join(" "))
}

/// `cd <cwd> && claude --resume <id>` / `cd <cwd> && codex resume <id>`. No model / permission flags: the
/// session keeps its own. The `cd` matters: Claude runs tools in the directory it was resumed from.
pub fn resume_command(launch: &str, agent: AgentKind, session_id: &str, cwd: &Path) -> String {
    let resume = match agent {
        AgentKind::Claude => "--resume",
        AgentKind::Codex => "resume",
    };
    in_dir(cwd, &format!("{} {resume} {}", shell_quote(launch), shell_quote(session_id)))
}

fn in_dir(dir: &Path, command: &str) -> String {
    format!("cd {} && {command}", shell_quote(&dir.to_string_lossy()))
}

fn non_blank(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}
