//! The inspector: the right-hand column of every window (`⌘I`, M3b spec §2). It follows the window's
//! focused pane and shows, in its 「过程」 tab, the waiting banner, the status card, the TODO block and (M3b
//! Task 6) the timeline of that pane's Claude / Codex session (`timeline_model` pure, `timeline_list` the
//! virtualized list's state, `timeline_view` the rows). 「产物」 (M4a: `artifacts_model` pure, `artifacts_view` the
//! cards) lists what each turn changed; 「配置」 (M5a) summarizes the effective Agent configuration without
//! writing it.
//! `model` is the pure part; `view` draws it inside `Workspace`, whose `workspace/inspector.rs` holds the
//! actions, the boundary drag and the per-second redraw. Nothing here writes to a PTY.

pub mod artifacts_model;
pub(crate) mod artifacts_view;
pub(crate) mod colors;
pub mod config_model;
mod config_view;
pub mod model;
mod timeline_list;
pub mod timeline_model;
mod timeline_view;
mod view;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use gilvt_agent::SessionKey;
use gpui::{FocusHandle, Point, ScrollHandle, Task};

use artifacts_model::Sel;

use timeline_list::TimelineUi;
pub use config_view::dialog as config_dialog;
pub use view::render;

use crate::sidebar::UiPrefs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectorTab {
    Process,
    Artifacts,
    Config,
}

impl InspectorTab {
    pub const ALL: [InspectorTab; 3] = [InspectorTab::Process, InspectorTab::Artifacts, InspectorTab::Config];

    pub fn label(self) -> &'static str {
        match self {
            InspectorTab::Process => "过程",
            InspectorTab::Artifacts => "产物",
            InspectorTab::Config => "配置",
        }
    }

    /// The name `gilvt debug state` reports.
    pub fn name(self) -> &'static str {
        match self {
            InspectorTab::Process => "process",
            InspectorTab::Artifacts => "artifacts",
            InspectorTab::Config => "config",
        }
    }

    /// Tabs not provided yet are greyed: choosing one shows this note for a moment and stays on the current tab.
    pub fn unavailable_note(self) -> Option<&'static str> {
        match self {
            InspectorTab::Process | InspectorTab::Artifacts | InspectorTab::Config => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigTarget {
    pub key: SessionKey,
    pub agent: gilvt_agent::AgentKind,
    pub cwd: PathBuf,
    pub home: Option<PathBuf>,
    pub agent_home: Option<PathBuf>,
    pub model: Option<String>,
    pub permission: Option<String>,
}

/// One window's asynchronously loaded, read-only Agent configuration summary.
pub struct ConfigUi {
    pub target: Option<ConfigTarget>,
    pub summary: Option<Arc<gilvt_config::Summary>>,
    pub loading: bool,
    pub generation: u64,
    /// Expanded rows and the open resource dialog (reset when another session is focused).
    pub disclosure: config_model::Disclosure,
    /// Takes the keyboard while the resource dialog is open (Esc closes it).
    pub dialog_focus: FocusHandle,
}

impl ConfigUi {
    pub fn new(dialog_focus: FocusHandle) -> Self {
        Self { target: None, summary: None, loading: false, generation: 0, disclosure: Default::default(), dialog_focus }
    }
}

/// The 「产物」 tab's view state of one window.
pub struct ArtifactsUi {
    /// The list takes the keyboard when clicked (`↑↓ Space ⏎`); Esc gives it back to the terminal.
    pub focus: FocusHandle,
    pub selected: Option<Sel>,
    /// Cards the user opened / closed (the newest card is open unless closed).
    pub open: HashSet<u32>,
    pub closed: HashSet<u32>,
    /// Cards listing every file (grouped by directory) instead of the first 8.
    pub all_files: HashSet<u32>,
    pub scroll: ScrollHandle,
    /// The session `selected` / `open` / `closed` / `all_files` belong to (card keys are per-session
    /// ledger numbers, so another session's card 3 is a different card). See [`ArtifactsUi::follow`].
    pub session: Option<SessionKey>,
    /// `↑↓` moved the selection: the next frame scrolls the selected row into view.
    pub reveal: bool,
    /// The 「本会话净改动」 card is open.
    pub net_open: bool,
    /// Quiet groups (by key) the user unfolded.
    pub open_quiet: HashSet<u32>,
}

impl ArtifactsUi {
    pub fn new(focus: FocusHandle) -> Self {
        ArtifactsUi {
            focus,
            selected: None,
            open: HashSet::new(),
            closed: HashSet::new(),
            all_files: HashSet::new(),
            scroll: ScrollHandle::new(),
            session: None,
            reveal: false,
            net_open: false,
            open_quiet: HashSet::new(),
        }
    }

    /// Opened by the user: open; closed by the user: closed; otherwise only the newest card is open.
    pub fn is_open(&self, key: u32, newest: bool) -> bool {
        is_open(&self.open, &self.closed, key, newest)
    }

    /// The tab now shows `key`'s cards: when that is another session than before, the selection, the opened /
    /// closed cards, 「查看全部」, the open session net / quiet groups and the scroll position are forgotten.
    /// Every reader of the per-card state goes through this first (`Workspace::focused_artifacts`).
    pub fn follow(&mut self, key: &SessionKey) {
        if follow_folds(&mut self.session, key, &mut self.net_open, &mut self.open_quiet) {
            self.selected = None;
            self.open.clear();
            self.closed.clear();
            self.all_files.clear();
            self.reveal = false;
            self.scroll.set_offset(Point::default());
        }
    }
}

fn is_open(open: &HashSet<u32>, closed: &HashSet<u32>, key: u32, newest: bool) -> bool {
    open.contains(&key) || (newest && !closed.contains(&key))
}

/// Records `key` as the session shown; true when it differs from the one before (the first one included).
fn switched(session: &mut Option<SessionKey>, key: &SessionKey) -> bool {
    if session.as_ref() == Some(key) {
        return false;
    }
    *session = Some(key.clone());
    true
}

/// [`switched`], and on a switch also closes the session net and the quiet groups (the part of
/// [`ArtifactsUi::follow`] that needs no `FocusHandle`, so it can be tested).
fn follow_folds(session: &mut Option<SessionKey>, key: &SessionKey, net_open: &mut bool, open_quiet: &mut HashSet<u32>) -> bool {
    let switched = switched(session, key);
    if switched {
        *net_open = false;
        open_quiet.clear();
    }
    switched
}

/// The selection after folding quiet group `group` (cards `members`): a card of the group, or one of its
/// files, hands the selection to the group's row (it would sit on a hidden row otherwise).
pub(crate) fn fold_selection(selected: Option<Sel>, group: u32, members: &[u32]) -> Option<Sel> {
    match selected {
        Some(Sel::Card(k) | Sel::File(k, _)) if members.contains(&k) => Some(Sel::Quiet(group)),
        s => s,
    }
}

/// The scroll offset (y, ≤ 0) that brings `row` (top, bottom) into the viewport (top, bottom) with the least
/// movement; a row taller than the viewport shows its top.
pub(crate) fn reveal_offset(viewport: (f32, f32), row: (f32, f32), offset: f32) -> f32 {
    if row.0 < viewport.0 {
        offset + (viewport.0 - row.0)
    } else if row.1 > viewport.1 {
        offset - (row.1 - viewport.1).min(row.0 - viewport.0)
    } else {
        offset
    }
}

/// One window's inspector. Hidden and width become the default for new windows (ui.json).
pub struct InspectorState {
    pub hidden: bool,
    /// px, within 240–560.
    pub width: f32,
    /// The note of a greyed tab just chosen, and which showing it is (its timer clears only its own).
    pub note: Option<(&'static str, u64)>,
    pub note_seq: u64,
    /// The boundary with the pane area is being dragged.
    pub dragging: bool,
    /// Redraws the window every second while the inspector shows a running turn; dropping it stops.
    pub ticker: Option<Task<()>>,
    /// The timeline list: filter, expanded rows / turns, scroll position.
    pub timeline: TimelineUi,
    /// The tab shown (a greyed tab is never chosen: it only shows its note).
    pub tab: InspectorTab,
    pub artifacts: ArtifactsUi,
    pub config: ConfigUi,
}

impl InspectorState {
    pub fn new(prefs: &UiPrefs, focus: FocusHandle) -> Self {
        InspectorState {
            hidden: prefs.inspector_hidden,
            width: prefs.inspector_px(),
            note: None,
            note_seq: 0,
            dragging: false,
            ticker: None,
            timeline: TimelineUi::default(),
            tab: InspectorTab::Process,
            artifacts: ArtifactsUi::new(focus.clone()),
            config: ConfigUi::new(focus.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_newest_card_is_open_by_default() {
        let none = HashSet::new();
        assert!(is_open(&none, &none, 7, true));
        assert!(!is_open(&none, &none, 6, false));
    }

    #[test]
    fn the_users_choice_wins() {
        let open: HashSet<u32> = [6].into();
        let closed: HashSet<u32> = [7].into();
        assert!(is_open(&open, &closed, 6, false));
        assert!(!is_open(&open, &closed, 7, true));
        assert!(!is_open(&open, &closed, 5, false));
    }

    #[test]
    fn card_state_is_forgotten_only_when_the_session_changes() {
        let a: SessionKey = (gilvt_agent::AgentKind::Claude, "a".into());
        let b: SessionKey = (gilvt_agent::AgentKind::Claude, "b".into());
        let mut session = None;
        assert!(switched(&mut session, &a), "the first session shown");
        assert!(!switched(&mut session, &a), "the same session again keeps the state");
        assert!(switched(&mut session, &b));
        assert_eq!(session, Some(b));
    }

    #[test]
    fn follow_resets_net_and_quiet_state() {
        // A FocusHandle needs a running gpui App (no `test-support` here), so the reset `follow` does is
        // tested through the part it shares with this test.
        let a: SessionKey = (gilvt_agent::AgentKind::Claude, "a".into());
        let b: SessionKey = (gilvt_agent::AgentKind::Claude, "b".into());
        let (mut session, mut net_open, mut open_quiet) = (None, false, HashSet::new());
        follow_folds(&mut session, &a, &mut net_open, &mut open_quiet);
        net_open = true;
        open_quiet.insert(3);
        assert!(!follow_folds(&mut session, &a, &mut net_open, &mut open_quiet));
        assert!(net_open && open_quiet.contains(&3), "same session keeps them");
        assert!(follow_folds(&mut session, &b, &mut net_open, &mut open_quiet));
        assert!(!net_open && open_quiet.is_empty());
    }

    #[test]
    fn folding_a_quiet_group_moves_a_selection_inside_it_to_the_group_row() {
        let members = [7, 6];
        assert_eq!(fold_selection(Some(Sel::Card(6)), 7, &members), Some(Sel::Quiet(7)));
        assert_eq!(fold_selection(Some(Sel::File(7, 0)), 7, &members), Some(Sel::Quiet(7)));
        assert_eq!(fold_selection(Some(Sel::Card(5)), 7, &members), Some(Sel::Card(5)), "outside the group");
        assert_eq!(fold_selection(Some(Sel::Net), 7, &members), Some(Sel::Net));
        assert_eq!(fold_selection(None, 7, &members), None);
    }

    #[test]
    fn revealing_a_row_scrolls_the_least() {
        let view = (100.0, 300.0);
        assert_eq!(reveal_offset(view, (150.0, 170.0), -40.0), -40.0, "already visible");
        assert_eq!(reveal_offset(view, (80.0, 100.0), -40.0), -20.0, "above: its top comes to the viewport's top");
        assert_eq!(reveal_offset(view, (290.0, 320.0), -40.0), -60.0, "below: its bottom comes to the viewport's bottom");
        assert_eq!(reveal_offset(view, (250.0, 600.0), 0.0), -150.0, "taller than the viewport: its top shows");
    }

    #[test]
    fn every_inspector_tab_is_available() {
        assert_eq!(InspectorTab::Artifacts.unavailable_note(), None);
        assert_eq!(InspectorTab::Config.unavailable_note(), None);
        assert_eq!(InspectorTab::ALL.map(InspectorTab::name), ["process", "artifacts", "config"]);
    }
}
