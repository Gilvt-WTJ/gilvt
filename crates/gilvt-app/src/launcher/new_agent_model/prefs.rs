//! What ⌘⇧N remembers between openings (spec §5): the Agent, whether 更多 is open, and each Agent's model
//! and permission mode, in `<state dir>/launcher.json` (kept apart from ui.json, whose `UiPrefs` is Clone).
//! Not the directory, not the task. Also the Codex model presets, taken from the history.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gilvt_agent::{AgentKind, HistoryEntry, Permission};
use serde::{Deserialize, Serialize};

/// Codex models used this recently are offered as presets.
pub const RECENT_MODELS: Duration = Duration::from_secs(30 * 24 * 3600);

/// One Agent's 更多 choices; None = 跟随配置 (the flag is not passed).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choice {
    pub model: Option<String>,
    pub permission: Option<Permission>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub agent: AgentKind,
    /// 更多 expanded.
    pub more: bool,
    pub claude: Choice,
    pub codex: Choice,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { agent: AgentKind::Claude, more: false, claude: Choice::default(), codex: Choice::default() }
    }
}

impl Choice {
    /// Blank models and another Agent's permission (a hand-edited file) are dropped.
    fn sanitized(self, agent: AgentKind) -> Choice {
        Choice {
            model: self.model.map(|m| m.trim().to_string()).filter(|m| !m.is_empty()),
            permission: self.permission.filter(|p| p.agent() == agent),
        }
    }
}

impl Prefs {
    fn path(dir: &Path) -> PathBuf {
        dir.join("launcher.json")
    }

    /// Missing or unreadable → defaults; fields missing from the file take theirs.
    pub fn load(dir: &Path) -> Prefs {
        let prefs: Prefs =
            std::fs::read_to_string(Self::path(dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Prefs { claude: prefs.claude.sanitized(AgentKind::Claude), codex: prefs.codex.sanitized(AgentKind::Codex), ..prefs }
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = dir.join(format!("launcher.json.tmp-{}-{seq}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, Self::path(dir))
    }
}

/// The Codex model presets: the distinct models of Codex sessions active within [`RECENT_MODELS`] of
/// `now`, most recently used first (`entries` come newest first, as the history lists them).
pub fn codex_models(entries: &[HistoryEntry], now: SystemTime) -> Vec<String> {
    let mut models: Vec<String> = Vec::new();
    for e in entries.iter().filter(|e| e.agent == AgentKind::Codex) {
        if now.duration_since(e.last_active).is_ok_and(|age| age > RECENT_MODELS) {
            continue;
        }
        if let Some(m) = e.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            if !models.iter().any(|x| x == m) {
                models.push(m.to_string());
            }
        }
    }
    models
}
