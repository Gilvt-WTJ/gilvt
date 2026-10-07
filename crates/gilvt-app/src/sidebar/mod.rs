//! The session sidebar on the left of every window (`⌘B`): Claude / Codex sessions of all windows,
//! 需要你 first. `model` is the pure layout; `view` draws it inside `Workspace`; `menu` / `rename` are
//! the row's right-click menu and inline rename field; `ended` the 已结束 rows' resume and Trash decisions. `UiPrefs` (ui.json) also carries the inspector's
//! width and visibility.

pub mod ended;
pub mod menu;
pub mod model;
pub mod rename;
pub mod tooltip;
pub mod view;

use std::path::{Path, PathBuf};

use gpui::{App, Global};
use serde::{Deserialize, Serialize};

pub use model::{Grouping, SidebarState};
pub use view::render;

/// Sidebar and inspector settings shared by new windows, persisted in `<state dir>/ui.json`. Fields missing
/// from an older file take their defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiPrefs {
    pub sidebar_hidden: bool,
    pub grouping: Grouping,
    /// The right-hand inspector (`⌘I`).
    pub inspector_hidden: bool,
    /// Its width in px, kept within 240–560 (`crate::inspector::model::clamp_width`).
    pub inspector_width: u32,
    /// The preview chosen per file type for the editor's live preview (`live_preview::pref_key` → `Provider::id`).
    pub live_preview: std::collections::BTreeMap<String, String>,
}

impl Default for UiPrefs {
    fn default() -> Self {
        UiPrefs {
            sidebar_hidden: false,
            grouping: Grouping::default(),
            inspector_hidden: false,
            inspector_width: crate::inspector::model::DEFAULT_WIDTH as u32,
            live_preview: Default::default(),
        }
    }
}

impl Global for UiPrefs {}

impl UiPrefs {
    fn path(dir: &Path) -> PathBuf {
        dir.join("ui.json")
    }

    /// Missing or unreadable → defaults; an out-of-range width is clamped.
    pub fn load(dir: &Path) -> UiPrefs {
        let mut prefs: UiPrefs =
            std::fs::read_to_string(Self::path(dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        prefs.inspector_width = crate::inspector::model::clamp_width(prefs.inspector_width as f32) as u32;
        prefs
    }

    /// The inspector width as the layout takes it.
    pub fn inspector_px(&self) -> f32 {
        crate::inspector::model::clamp_width(self.inspector_width as f32)
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = dir.join(format!("ui.json.tmp-{}-{seq}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, Self::path(dir))
    }

    /// Records a window's choice as the default for new windows and writes it off the main thread
    /// (only when something changed).
    pub fn remember(change: impl FnOnce(&mut UiPrefs), cx: &mut App) {
        let mut prefs = cx.try_global::<UiPrefs>().cloned().unwrap_or_default();
        change(&mut prefs);
        if cx.try_global::<UiPrefs>() == Some(&prefs) {
            return;
        }
        let saved = prefs.clone();
        cx.set_global(prefs);
        if let Some(dir) = crate::agents::state_dir() {
            cx.background_executor()
                .spawn(async move {
                    if let Err(e) = saved.save(&dir) {
                        eprintln!("gilvt: cannot save {}: {e}", UiPrefs::path(&dir).display());
                    }
                })
                .detach();
        }
    }

    /// [`UiPrefs::remember`] a window's sidebar (hidden, grouping).
    pub fn remember_sidebar(state: &SidebarState, cx: &mut App) {
        let (hidden, grouping) = (state.hidden, state.grouping);
        Self::remember(
            |p| {
                p.sidebar_hidden = hidden;
                p.grouping = grouping;
            },
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(UiPrefs::load(dir.path()), UiPrefs::default());
        let prefs = UiPrefs { sidebar_hidden: true, grouping: Grouping::Status, inspector_hidden: true, inspector_width: 400, live_preview: Default::default() };
        prefs.save(&dir.path().join("state")).unwrap();
        assert_eq!(UiPrefs::load(&dir.path().join("state")), prefs);
        let text = std::fs::read_to_string(dir.path().join("state/ui.json")).unwrap();
        assert!(text.contains("\"grouping\": \"status\""), "{text}");
        assert!(text.contains("\"inspector_width\": 400"), "{text}");
        std::fs::write(dir.path().join("ui.json"), "{\"grouping\":\"nope\"}").unwrap();
        assert_eq!(UiPrefs::load(dir.path()), UiPrefs::default(), "corrupt → defaults");
        // An M3a file: the inspector fields take their defaults.
        std::fs::write(dir.path().join("ui.json"), "{\"sidebar_hidden\":true,\"grouping\":\"status\"}").unwrap();
        let old = UiPrefs::load(dir.path());
        assert_eq!((old.sidebar_hidden, old.grouping), (true, Grouping::Status));
        assert_eq!((old.inspector_hidden, old.inspector_width), (false, 320));
        std::fs::write(dir.path().join("ui.json"), "{\"sidebar_hidden\":true}").unwrap();
        assert_eq!(UiPrefs::load(dir.path()).grouping, Grouping::Project, "missing fields default");
        std::fs::write(dir.path().join("ui.json"), "{\"inspector_width\":9000}").unwrap();
        assert_eq!(UiPrefs::load(dir.path()).inspector_width, 560, "clamped");
        std::fs::write(dir.path().join("ui.json"), "{\"inspector_width\":10}").unwrap();
        assert_eq!(UiPrefs::load(dir.path()).inspector_width, 240, "clamped");
    }

    #[test]
    fn live_preview_choices_round_trip_and_default_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        // An older file without the field.
        std::fs::write(dir.path().join("ui.json"), "{\"sidebar_hidden\":true}").unwrap();
        assert!(UiPrefs::load(dir.path()).live_preview.is_empty());

        let mut prefs = UiPrefs::default();
        prefs.live_preview.insert("md".into(), "changes".into());
        prefs.save(dir.path()).unwrap();
        assert_eq!(UiPrefs::load(dir.path()).live_preview.get("md").map(String::as_str), Some("changes"));
    }
}
