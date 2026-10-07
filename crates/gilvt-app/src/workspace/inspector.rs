//! The window's side of the inspector (M3b spec §2): `⌘I`, `⌥⌘1/2/3`, dragging its boundary with the pane
//! area, remembering width / visibility, and the per-second redraw of a running turn's elapsed time. The
//! pane area is a flex item beside it, so terminals get their new rows / columns from their own bounds.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gilvt_snapshot::{ObjectStore, TreeId};
use gilvt_viewer::TurnRange;
use gpui::{prelude::*, App, ClipboardItem, Context, Div, KeyDownEvent, MouseButton, MouseMoveEvent, Window};

use super::{PaneView, Workspace};
use crate::actions::*;
use crate::agents::Agents;
use crate::inspector::artifacts_model::{self as am, Sel};
use crate::preview_view::{OpenRequest, Source};
use crate::inspector::model::{drag_width, Plain};
use crate::inspector::config_model::{Group as ConfigGroup, Section as ConfigSection};
use crate::inspector::{ConfigTarget, InspectorState, InspectorTab};
use crate::pane_tree::PaneId;
use crate::sidebar::UiPrefs;

/// How long a greyed tab's note stays.
const NOTE: Duration = Duration::from_millis(2500);
/// Redraw interval while a turn runs (the card's elapsed time).
const TICK: Duration = Duration::from_secs(1);

/// The 「产物」 cards of `session` from the timeline and the recorder's ledger (read-only; shared by the tab
/// and the DebugState export).
pub(crate) fn build_artifacts(agents: &Agents, session: &gilvt_agent::Session) -> am::Artifacts {
    let timeline = agents.timeline(&session.key);
    let records = agents.recorder().records(&session.key);
    let live = agents.recorder().live(&session.key);
    let recorder = agents.recorder();
    am::build(&am::Inputs {
        turns: timeline.as_ref().map_or(&[][..], |v| &v.turns[..]),
        records: &records,
        live: live.as_deref(),
        fallback_root: session.cwd.as_deref(),
        clock: &|t| crate::inspector::timeline_model::local_clock(t, false),
        // Cached; the first ask starts the computation and a later frame picks the result up.
        diff: &|r| recorder.diff(&r.repo_root, &r.before, &r.after),
    })
}

impl Workspace {
    pub fn inspector(&self) -> &InspectorState {
        &self.inspector
    }

    /// Every pane of this window.
    pub fn pane_ids(&self) -> impl Iterator<Item = PaneId> + '_ {
        self.panes.keys().copied()
    }

    /// What the focused pane is when the inspector finds no session for it.
    pub fn plain_kind(&self, pane: PaneId) -> Plain {
        match self.panes.get(&pane) {
            Some(PaneView::Preview(_)) => Plain::Preview,
            Some(PaneView::Editor(_)) => Plain::Editor,
            Some(PaneView::Monitor(_)) => Plain::Monitor,
            _ => Plain::Shell,
        }
    }

    fn set_inspector_hidden(&mut self, hidden: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.inspector.hidden = hidden;
        if hidden {
            self.inspector.ticker = None;
            self.inspector.dragging = false;
            self.release_artifacts_focus(window, cx);
        }
        UiPrefs::remember(|p| p.inspector_hidden = hidden, cx);
        cx.notify();
    }

    /// `⌥⌘1/2/3` or a tab click: shows the inspector; a greyed tab only shows its note.
    pub fn choose_inspector_tab(&mut self, tab: InspectorTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.inspector.hidden {
            self.set_inspector_hidden(false, window, cx);
        }
        let Some(note) = tab.unavailable_note() else {
            self.inspector.note = None;
            self.inspector.tab = tab;
            if tab != InspectorTab::Artifacts {
                self.release_artifacts_focus(window, cx);
            }
            cx.notify();
            if tab == InspectorTab::Config {
                self.refresh_config_summary(cx);
            }
            return;
        };
        self.inspector_note(note, cx);
    }

    fn config_target(&self, cx: &App) -> Option<ConfigTarget> {
        let agents = cx.global::<Agents>();
        let session = self.focused_pane().and_then(|pane| crate::inspector::model::follow(agents.registry().sessions(), pane))?;
        let cwd = session.cwd.clone()?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let agent_home = match session.agent() {
            gilvt_agent::AgentKind::Claude => std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|home| home.join(".claude"))),
            gilvt_agent::AgentKind::Codex => std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|home| home.join(".codex"))),
        };
        Some(ConfigTarget {
            key: session.key.clone(),
            agent: session.agent(),
            cwd,
            home,
            agent_home,
            model: session.model.clone(),
            permission: agents.permission_mode(&session.key).map(str::to_string),
        })
    }

    /// Loads a new focused session's configuration. Re-renders do not touch the filesystem.
    pub fn sync_config_summary(&mut self, cx: &mut Context<Self>) {
        let target = self.config_target(cx);
        if self.inspector.config.target == target {
            return;
        }
        // A model / permission change reloads in place; only a different session closes the open rows.
        if self.inspector.config.target.as_ref().map(|t| &t.key) != target.as_ref().map(|t| &t.key) {
            self.inspector.config.disclosure.reset();
        }
        self.start_config_load(target, cx);
    }

    /// Expands or collapses one MCP / hook / memory row.
    pub fn toggle_config_row(&mut self, section: ConfigSection, name: &str, cx: &mut Context<Self>) {
        self.inspector.config.disclosure.toggle(section, name);
        cx.notify();
    }

    /// Opens the Skills / commands / sub-agents dialog and gives it the keyboard (Esc closes).
    pub fn open_config_dialog(&mut self, group: ConfigGroup, window: &mut Window, cx: &mut Context<Self>) {
        self.inspector.config.disclosure.dialog = Some(group);
        window.focus(&self.inspector.config.dialog_focus);
        cx.notify();
    }

    pub fn close_config_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inspector.config.disclosure.dialog.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// Shows Markdown file `index` of `paths` in Quick Look (←/→ walk the rest). Closes the dialog first, so the
    /// preview is not drawn beneath it.
    pub fn open_config_files(&mut self, paths: Vec<PathBuf>, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        self.inspector.config.disclosure.dialog = None;
        let req = OpenRequest {
            index: index.min(paths.len() - 1),
            sources: paths.into_iter().map(Source::File).collect(),
            line: None,
            annotation: None,
            rev: None,
            turn: None,
        };
        let origin = self.focused_pane();
        self.open_preview(req, false, origin, window, cx);
    }

    /// 「编辑」 in the resource dialog or an expanded row: Markdown file `path` in the built-in editor beside the
    /// focused pane (`flip`: ⌥ was held). Closes the dialog first, like a preview.
    pub fn open_config_edit(&mut self, path: PathBuf, flip: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.close_config_dialog(window, cx);
        let origin = self.focused_pane();
        self.open_editor(path, None, origin, flip, window, cx);
    }

    /// Explicit refresh from the M5a header.
    pub fn refresh_config_summary(&mut self, cx: &mut Context<Self>) {
        let target = self.config_target(cx);
        self.start_config_load(target, cx);
    }

    fn start_config_load(&mut self, target: Option<ConfigTarget>, cx: &mut Context<Self>) {
        self.inspector.config.generation += 1;
        let generation = self.inspector.config.generation;
        self.inspector.config.target = target.clone();
        self.inspector.config.summary = None;
        self.inspector.config.loading = target.is_some();
        let Some(target) = target else {
            cx.notify();
            return;
        };
        cx.spawn(async move |ws, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    gilvt_config::load(gilvt_config::Request {
                        agent: target.agent,
                        cwd: &target.cwd,
                        home: target.home.as_deref(),
                        agent_home: target.agent_home.as_deref(),
                        runtime_model: target.model.as_deref(),
                        runtime_permission: target.permission.as_deref(),
                    })
                })
                .await;
            let _ = ws.update(cx, |ws, cx| {
                if ws.inspector.config.generation == generation {
                    ws.inspector.config.summary = Some(Arc::new(loaded));
                    ws.inspector.config.loading = false;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// The 「产物」 list is no longer drawn: if it had the keyboard, the terminal gets it back (a focused
    /// handle nothing tracks would swallow every key).
    fn release_artifacts_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inspector.artifacts.focus.is_focused(window) {
            self.focus_active(window, cx);
        }
    }

    /// Shows `note` under the tab strip for a moment (a greyed tab's note, or a timeline toast such as
    /// 「已超出回滚范围」); a later note replaces it.
    pub fn inspector_note(&mut self, note: &'static str, cx: &mut Context<Self>) {
        self.inspector.note_seq += 1;
        let seq = self.inspector.note_seq;
        self.inspector.note = Some((note, seq));
        cx.spawn(async move |ws, cx| {
            cx.background_executor().timer(NOTE).await;
            let _ = ws.update(cx, |ws, cx| {
                if ws.inspector.note.is_some_and(|(_, s)| s == seq) {
                    ws.inspector.note = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub fn start_inspector_drag(&mut self) {
        self.inspector.dragging = true;
    }

    fn on_inspector_drag(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.inspector.dragging {
            return;
        }
        if e.pressed_button != Some(MouseButton::Left) {
            // Released outside the window.
            return self.end_inspector_drag(cx);
        }
        let width = drag_width(window.viewport_size().width / gpui::px(1.), e.position.x / gpui::px(1.));
        if width != self.inspector.width {
            self.inspector.width = width;
            cx.notify();
        }
    }

    /// The drag ended: the width becomes the default for new windows.
    fn end_inspector_drag(&mut self, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.inspector.dragging) {
            return;
        }
        let width = self.inspector.width.round() as u32;
        UiPrefs::remember(|p| p.inspector_width = width, cx);
    }

    /// Starts or stops the per-second redraw (called from render with whether the inspector shows a running
    /// turn); nothing is redrawn on a timer otherwise.
    pub(super) fn sync_inspector_ticker(&mut self, tick: bool, cx: &mut Context<Self>) {
        match (tick, self.inspector.ticker.is_some()) {
            (true, false) => {
                self.inspector.ticker = Some(cx.spawn(async move |ws, cx| loop {
                    cx.background_executor().timer(TICK).await;
                    if ws.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }));
            }
            (false, true) => self.inspector.ticker = None,
            _ => {}
        }
    }

    /// The inspector's actions and the boundary drag, on the window root.
    pub(super) fn inspector_actions(root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|ws, _: &ToggleInspector, window, cx| {
            let hidden = !ws.inspector.hidden;
            ws.set_inspector_hidden(hidden, window, cx)
        }))
        .on_action(cx.listener(|ws, _: &InspectorProcess, window, cx| ws.choose_inspector_tab(InspectorTab::Process, window, cx)))
        .on_action(cx.listener(|ws, _: &InspectorArtifacts, window, cx| ws.choose_inspector_tab(InspectorTab::Artifacts, window, cx)))
        .on_action(cx.listener(|ws, _: &InspectorConfig, window, cx| ws.choose_inspector_tab(InspectorTab::Config, window, cx)))
        .on_mouse_move(cx.listener(Self::on_inspector_drag))
        .on_mouse_up(MouseButton::Left, cx.listener(|ws, _, _, cx| ws.end_inspector_drag(cx)))
    }

    /// The artifacts of the focused pane's session, as drawn now (None: the pane has no session). The tab and
    /// every handler get them here, which first switches the tab's per-card state to that session
    /// ([`ArtifactsUi::follow`](crate::inspector::ArtifactsUi::follow)), so a card key is never looked up
    /// in another session's state. While a turn runs it also asks the recorder for a fresh live file list.
    pub(crate) fn focused_artifacts(&mut self, cx: &App) -> Option<am::Artifacts> {
        let agents = cx.global::<Agents>();
        let session = self.focused_pane().and_then(|p| crate::inspector::model::follow(agents.registry().sessions(), p))?;
        self.inspector.artifacts.follow(&session.key);
        let built = build_artifacts(agents, session);
        // Measured from the running task's first `before`, so its follow-ups' list covers the whole task.
        if let Some(c) = built.cards.iter().find(|c| c.state == am::CardState::Running) {
            agents.recorder().request_live(&session.key, session.cwd.clone(), c.live_base.clone());
        }
        Some(built)
    }

    /// A click on a card / file row: the rows were drawn for the session `focused_artifacts` last switched
    /// the state to (render goes through it every frame), so the key belongs to it.
    pub fn select_artifact(&mut self, sel: Sel, cx: &mut Context<Self>) {
        self.inspector.artifacts.selected = Some(sel);
        cx.notify();
    }

    pub fn toggle_artifact_card(&mut self, key: u32, cx: &mut Context<Self>) {
        let newest = self.focused_artifacts(cx).and_then(|a| am::newest_open_by_default(&a));
        self.toggle_card(key, newest, cx);
    }

    /// Opens / closes the 「本会话净改动」 card.
    pub fn toggle_artifact_net(&mut self, cx: &mut Context<Self>) {
        self.inspector.artifacts.net_open = !self.inspector.artifacts.net_open;
        cx.notify();
    }

    /// Unfolds / folds the quiet group keyed `key`; folding moves a selection inside it to the group's row.
    pub fn toggle_artifact_quiet(&mut self, key: u32, cx: &mut Context<Self>) {
        if self.inspector.artifacts.open_quiet.remove(&key) {
            let members = self.focused_artifacts(cx).and_then(|a| a.quiet_groups.into_iter().find(|g| g.key == key)).map(|g| g.cards);
            let ui = &mut self.inspector.artifacts;
            ui.selected = crate::inspector::fold_selection(ui.selected, key, members.as_deref().unwrap_or_default());
        } else {
            self.inspector.artifacts.open_quiet.insert(key);
        }
        cx.notify();
    }

    /// Opens file `file` of the session net in Quick Look (←/→ walk the net's files).
    pub fn open_net_file(&mut self, file: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(net) = self.focused_artifacts(cx).and_then(|a| a.net) else { return };
        if net.files.is_empty() {
            return;
        }
        self.open_range_files(&net.range, &net.files, file, window, cx);
    }

    /// Quick Look on `files[file]` of `range`, ←/→ walking `files`.
    fn open_range_files(&mut self, range: &am::CardRange, files: &[am::FileRow], file: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = crate::agents::state_dir() else { return };
        let Ok(store) = ObjectStore::open(&range.repo_root, &state) else { return self.inspector_note("快照已清理", cx) };
        let turn = TurnRange { store, before: range.before.clone(), after: range.after.clone(), label: range.label.clone(), scope: range.scope };
        let req = OpenRequest {
            sources: files.iter().map(|f| Source::File(f.abs.clone())).collect(),
            index: file.min(files.len() - 1),
            line: None,
            annotation: None,
            rev: None,
            turn: Some(turn),
        };
        let origin = self.focused_pane();
        self.open_preview(req, false, origin, window, cx);
    }

    /// Opens a closed card / closes an open one; `newest` is the key of the newest card (open by default).
    fn toggle_card(&mut self, key: u32, newest: Option<u32>, cx: &mut Context<Self>) {
        let ui = &mut self.inspector.artifacts;
        if ui.is_open(key, Some(key) == newest) {
            ui.open.remove(&key);
            ui.closed.insert(key);
        } else {
            ui.closed.remove(&key);
            ui.open.insert(key);
        }
        cx.notify();
    }

    pub fn show_all_artifact_files(&mut self, key: u32, cx: &mut Context<Self>) {
        self.inspector.artifacts.all_files.insert(key);
        cx.notify();
    }

    /// Opens file `file` of card `key` in Quick Look as the turn left it; the card's files are the sources, so
    /// ←/→ walk them.
    pub fn open_artifact_file(&mut self, key: u32, file: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(artifacts) = self.focused_artifacts(cx) else { return };
        if let Some(card) = artifacts.cards.iter().find(|c| c.key == key) {
            self.open_card_file(card, file, window, cx);
        }
    }

    fn open_card_file(&mut self, card: &am::ArtCard, file: usize, window: &mut Window, cx: &mut Context<Self>) {
        if card.files.is_empty() {
            return;
        }
        // The task's net diff is not in yet: its files are a placeholder union, not what the range compares.
        if card.computing {
            return self.inspector_note("净改动还在计算", cx);
        }
        let Some(range) = &card.range else {
            let note = if card.state == am::CardState::Running { "这一轮还在进行，结束后才能看 diff" } else { "这一轮没有快照，无法查看 diff" };
            return self.inspector_note(note, cx);
        };
        self.open_range_files(range, &card.files, file, window, cx);
    }

    /// 「复制路径:行号」: the file's path with the line of its first change in the card's range.
    pub fn copy_artifact_path(&mut self, key: u32, file: usize, cx: &mut Context<Self>) {
        let Some(artifacts) = self.focused_artifacts(cx) else { return };
        let Some(card) = artifacts.cards.iter().find(|c| c.key == key) else { return };
        let Some(f) = card.files.get(file) else { return };
        // While the net diff is computed the rows are a placeholder union the range does not compare: line 1.
        let line = card.range.as_ref().filter(|_| !card.computing).map_or(1, |r| first_line(r, f));
        cx.write_to_clipboard(ClipboardItem::new_string(format!("{}:{line}", f.abs.display())));
        self.inspector_note("已复制", cx);
    }

    /// 「复制路径:行号」 of file `file` of the session net (the line of its first change over the session).
    pub fn copy_net_path(&mut self, file: usize, cx: &mut Context<Self>) {
        let Some(net) = self.focused_artifacts(cx).and_then(|a| a.net) else { return };
        let Some(f) = net.files.get(file) else { return };
        let line = first_line(&net.range, f);
        cx.write_to_clipboard(ClipboardItem::new_string(format!("{}:{line}", f.abs.display())));
        self.inspector_note("已复制", cx);
    }

    /// `↑↓` select, `Space` previews, `⏎` opens / closes the session net, a card or a quiet group (or previews
    /// a file), Esc gives the keyboard back to the terminal.
    pub fn artifacts_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = e.keystroke.modifiers;
        if m.platform || m.control || m.alt {
            return;
        }
        let key = e.keystroke.key.as_str();
        if key == "escape" {
            self.focus_active(window, cx);
            return cx.stop_propagation();
        }
        if !matches!(key, "down" | "up" | "enter" | "space") {
            return;
        }
        cx.stop_propagation();
        let Some(artifacts) = self.focused_artifacts(cx) else { return };
        let newest = am::newest_open_by_default(&artifacts);
        let selected = self.inspector.artifacts.selected;
        let target = match (key, selected) {
            ("down" | "up", _) => {
                let ui = &self.inspector.artifacts;
                let open = am::Open {
                    net: ui.net_open,
                    card: &|c| ui.is_open(c.key, Some(c.key) == newest),
                    all: &|k| ui.all_files.contains(&k),
                    quiet: &|k| ui.open_quiet.contains(&k),
                };
                let rows = am::nav_rows(&artifacts, &open);
                self.inspector.artifacts.selected = am::step(&rows, selected, if key == "down" { 1 } else { -1 });
                self.inspector.artifacts.reveal = true;
                return cx.notify();
            }
            ("enter", Some(Sel::Net)) => return self.toggle_artifact_net(cx),
            ("enter", Some(Sel::Quiet(k))) => return self.toggle_artifact_quiet(k, cx),
            ("enter", Some(Sel::Card(k))) => {
                // A quiet card has nothing to unfold.
                if !artifacts.cards.iter().any(|c| c.key == k && c.state == am::CardState::Quiet) {
                    self.toggle_card(k, newest, cx);
                }
                return;
            }
            (_, Some(Sel::Net)) => return self.open_net_file(0, window, cx),
            (_, Some(Sel::NetFile(i))) => return self.open_net_file(i, window, cx),
            (_, Some(Sel::Quiet(_)) | None) => return,
            (_, Some(Sel::Card(k))) => (k, 0),
            (_, Some(Sel::File(k, i))) => (k, i),
        };
        if let Some(card) = artifacts.cards.iter().find(|c| c.key == target.0) {
            self.open_card_file(card, target.1, window, cx);
        }
    }
}

/// The 1-based line of `f`'s first change from `range.before` to `range.after` (1 when the snapshots are gone).
fn first_line(range: &am::CardRange, f: &am::FileRow) -> u32 {
    let line = || {
        let state = crate::agents::state_dir()?;
        let store = ObjectStore::open(&range.repo_root, &state).ok()?;
        let text = |tree: &TreeId, path: &str| {
            store.blob_at(tree, path).ok().flatten().map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
        };
        let new = text(&range.after, &f.path);
        let old = text(&range.before, f.old_path.as_deref().unwrap_or(&f.path));
        // A change at the very end (a tail deletion) points past the new text: keep it on its last line.
        let last = u32::try_from(new.lines().count()).unwrap_or(u32::MAX).max(1);
        Some(am::first_changed_line(&old, &new).min(last))
    };
    line().unwrap_or(1)
}
