//! Why an agent in a pane runs without hooks, and what to tell the user (agent-shims spec §4.2–4.3).

use std::collections::BTreeSet;
use std::path::Path;

use gilvt_agent::AgentKind;

use super::foreground::Via;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cause {
    /// A wrapper started the agent without the shim (it put another directory first on PATH, or used a path).
    Bypassed { parent: String },
    /// The command typed is not in the agent's command list (a script that runs the agent by path).
    Unlisted { word: String, kind: AgentKind },
}

impl Cause {
    fn key(&self) -> String {
        match self {
            Cause::Bypassed { parent } => format!("bypassed:{parent}"),
            Cause::Unlisted { word, .. } => format!("unlisted:{word}"),
        }
    }

    /// DebugState `cause`.
    pub fn id(&self) -> &'static str {
        match self {
            Cause::Bypassed { .. } => "bypassed",
            Cause::Unlisted { .. } => "unlisted",
        }
    }
}

const PREFIXES: [&str; 7] = ["env", "time", "nice", "nohup", "command", "exec", "sudo"];

/// The command word of a typed line: after `VAR=value` words; None for a prefix command (can't tell what it ran).
pub fn first_word(cmdline: &str) -> Option<String> {
    let words = gilvt_agent::launch::split_command(cmdline)?;
    let word = words.into_iter().find(|w| !is_assignment(w))?;
    (!PREFIXES.contains(&word.as_str())).then_some(word)
}

fn is_assignment(w: &str) -> bool {
    w.split_once('=').is_some_and(|(k, _)| {
        !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !k.starts_with(|c: char| c.is_ascii_digit())
    })
}

/// Why the agent `via` found runs in lite mode; None when gilvt cannot tell (or wrappers are off).
pub fn cause(via: &Via, agent: AgentKind, cmdline: Option<&str>, claude: &[String], codex: &[String], disabled: bool) -> Option<Cause> {
    if disabled {
        return None;
    }
    match via {
        Via::Descendant { parent } => Some(Cause::Bypassed { parent: parent.clone() }),
        Via::Leader => {
            let word = first_word(cmdline?)?;
            // A path (`./cc`, `~/bin/cc`) bypasses the shims whatever the lists say, and 「加入」 could not take it.
            if !gilvt_client::shim::name_ok(&word) {
                return None;
            }
            let listed = claude.iter().chain(codex).any(|n| *n == word);
            (!listed).then_some(Cause::Unlisted { word, kind: agent })
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub cause: Cause,
    pub text: String,
    /// What 「复制配置」 copies (Bypassed only).
    pub copy: Option<String>,
}

/// Display name of an agent kind ("Claude" / "Codex").
fn label(k: AgentKind) -> &'static str {
    match k {
        AgentKind::Claude => "Claude",
        AgentKind::Codex => "Codex",
    }
}

/// ttadk's `ai_tool_commands` line pointing `agent` at `shim`. ttadk splits the value into words
/// (`splitCommandLine`: whitespace separates, quotes group, a backslash escapes outside quotes), and the shims
/// directory has a space in it (`Application Support`), so the path goes in single quotes, a `'` in it as
/// `'\''`; the whole value is then a JSON string.
pub fn ttadk_line(agent: AgentKind, shim: &Path) -> String {
    let word = format!("'{}'", shim.to_string_lossy().replace('\'', r"'\''"));
    format!("\"{}\": {}", agent.name(), serde_json::Value::String(word))
}

/// `names` is the agent's command list: only those names have a shim, so 「复制配置」 points at the one named
/// after the agent (`claude` / `codex`) when listed, else the first (no copy without a name or a shims directory).
pub fn hint(cause: &Cause, agent: AgentKind, shims_dir: Option<&Path>, names: &[String]) -> Hint {
    let name = names.iter().find(|n| n.as_str() == agent.name()).or(names.first());
    let shim = shims_dir.zip(name).map(|(d, n)| d.join(n));
    match cause {
        Cause::Bypassed { parent } if parent == "ttadk" => Hint {
            cause: cause.clone(),
            text: if crate::i18n::english() {
                format!("The {0} started by ttadk did not go through gilvt, so it runs in lite mode. In ~/.ttadk/setting.json, point ai_tool_commands.{0} at gilvt's shim.", agent.name())
            } else {
                format!("ttadk 启动的 {0} 没有经过 gilvt，只能用精简模式。把 ~/.ttadk/setting.json 里 ai_tool_commands 的 {0} 指向 gilvt 的垫片即可。", agent.name())
            },
            copy: shim.map(|s| ttadk_line(agent, &s)),
        },
        Cause::Bypassed { parent } => Hint {
            cause: cause.clone(),
            text: if crate::i18n::english() {
                format!("{parent} started {} directly, not through gilvt, so it runs in lite mode. Make it run gilvt's shim instead.", agent.name())
            } else {
                format!("{parent} 直接启动了 {}，没有经过 gilvt，只能用精简模式。让它改为调用 gilvt 的垫片即可。", agent.name())
            },
            copy: shim.map(|s| s.display().to_string()),
        },
        Cause::Unlisted { word, kind } => Hint {
            cause: cause.clone(),
            text: if crate::i18n::english() {
                format!("{word} started {}, but it is not one of the {} commands gilvt wraps.", kind.name(), label(*kind))
            } else {
                format!("{word} 启动了 {}，但不在 {} 命令名里。", kind.name(), label(*kind))
            },
            copy: None,
        },
    }
}

/// Hints the user chose not to see again, in `<state_dir>/agent-hints.json`.
#[derive(Clone, Debug, Default)]
pub struct Dismissed(BTreeSet<String>);

impl Dismissed {
    fn path(state_dir: &Path) -> std::path::PathBuf {
        state_dir.join("agent-hints.json")
    }

    pub fn load(state_dir: &Path) -> Dismissed {
        let set = std::fs::read(Self::path(state_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Dismissed(set)
    }

    pub fn save(&self, state_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(state_dir)?;
        std::fs::write(Self::path(state_dir), serde_json::to_vec(&self.0)?)
    }

    pub fn contains(&self, c: &Cause) -> bool {
        self.0.contains(&c.key())
    }

    pub fn insert(&mut self, c: &Cause) {
        self.0.insert(c.key());
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
#[path = "hints_tests.rs"]
mod tests;
