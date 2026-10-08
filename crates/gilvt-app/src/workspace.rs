//! A window: the session sidebar, a tab bar plus, per tab, a split tree of panes (terminals and
//! pinned previews, editors), an optional Quick Look overlay, `⌘P` file palette, `⌘⇧R` 会话 palette or `⌘⇧N` 新建 Agent
//! panel, and the inspector on the right.

mod close;
mod command_bar;
mod ended;
mod history;
mod live_preview;
mod monitor;
mod inspector;
mod debug;
mod editor;
pub mod marks;
mod new_agent;
mod pane_drag;
pub mod nav;
mod restore;
mod run;
mod sessions;
mod timeline;

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    canvas, div, prelude::*, px, relative, Animation, AnimationExt, AnyElement, App, Bounds, Context, CursorStyle,
    Entity, FocusHandle, Focusable, FontWeight, MouseButton, MouseMoveEvent, Pixels, Subscription, Task, Window,
};

use crate::actions::*;
use crate::agents::Agents;
use crate::editor::view::EditorView;
use crate::debug_state::rects::{self, RectId};
use crate::finder::{palette_stays, FinderEvent, FinderView, Recent};
use crate::inspector::InspectorState;
use crate::launcher::new_agent_view::NewAgentView;
use crate::launcher::sessions_view::Confirm;
use crate::launch::ShellEnv;
use crate::pane_tree::{next_pane_id, Axis, Dir, Node, PaneId, PaneTree, Rect};
use crate::preview_view::{open_in_editor, OpenRequest, PreviewEvent, PreviewView, Source};
use crate::settings::Settings;
use crate::session_center::view::SessionCenterView;
use crate::sidebar::{menu::SessionMenu, SidebarState, UiPrefs};
use crate::terminal_view::{spawn_session, TerminalView, TerminalViewEvent};
use crate::theme::{hsla, mix, AppSettings};

pub(crate) use inspector::build_artifacts as build_artifacts_for;
pub use close::{quit_requested, CloseAction};
pub use debug::WindowInfo;
pub use pane_drag::DraggedPane;
pub use history::live_sessions;
pub use monitor::monitor_jump;
pub use nav::{absolute_cursor_line, focus_pane_anywhere, notify_all, notify_sessions, resume_pending_anywhere, terminal_dirs, terminal_pids, visible_panes, workspaces};

/// What a pane shows.
#[derive(Clone)]
enum PaneView {
    Terminal(Entity<TerminalView>),
    Preview(Entity<PreviewView>),
    Editor(Entity<EditorView>),
    /// The 「◎ 监控官」 tab; drawn by the workspace (`monitor::view::render`).
    Monitor(crate::monitor::MonitorPane),
}

impl PaneView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            PaneView::Terminal(v) => v.focus_handle(cx),
            PaneView::Preview(v) => v.focus_handle(cx),
            PaneView::Editor(v) => v.focus_handle(cx),
            PaneView::Monitor(m) => m.focus.clone(),
        }
    }

    fn title(&self, cx: &App) -> String {
        match self {
            PaneView::Terminal(v) => v.read(cx).title(),
            PaneView::Preview(v) => format!("👁 {}", v.read(cx).title()),
            PaneView::Editor(v) => {
                let v = v.read(cx);
                format!("{}{}", if v.is_dirty() { "● " } else { "" }, v.title())
            }
            PaneView::Monitor(_) => "◎ 监控官".to_string(),
        }
    }

    /// Directory new panes opened from this one should start in.
    fn cwd(&self, cx: &App) -> Option<PathBuf> {
        match self {
            PaneView::Terminal(v) => v.read(cx).cwd(),
            PaneView::Preview(v) => v.read(cx).file_path().and_then(|p| p.parent()).map(PathBuf::from),
            PaneView::Editor(v) => v.read(cx).path().and_then(|p| p.parent()).map(PathBuf::from),
            PaneView::Monitor(_) => None,
        }
    }

    /// The pane's own view; None for the monitor, which the workspace draws itself.
    fn element(&self) -> Option<AnyElement> {
        match self {
            PaneView::Terminal(v) => Some(v.clone().into_any_element()),
            PaneView::Preview(v) => Some(v.clone().into_any_element()),
            PaneView::Editor(v) => Some(v.clone().into_any_element()),
            PaneView::Monitor(_) => None,
        }
    }
}

struct Tab {
    tree: PaneTree,
    focused: PaneId,
    zoomed: bool,
    /// Stable for the tab's life (in memory only; indices shift as tabs close).
    id: u64,
    /// For a file tab (an editor opened in a new tab): the tab it was opened from, activated again when it closes.
    origin_tab: Option<u64>,
}

impl Tab {
    fn new(tree: PaneTree, focused: PaneId) -> Self {
        Tab { tree, focused, zoomed: false, id: next_tab_id(), origin_tab: None }
    }
}

fn next_tab_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub struct Workspace {
    tabs: Vec<Tab>,
    active: usize,
    panes: HashMap<PaneId, PaneView>,
    subscriptions: HashMap<PaneId, Subscription>,
    /// Editor pane ↔ its live preview pane (`workspace/live_preview.rs`).
    live_pairs: crate::live_preview::LivePairs,
    /// Per editor pane: the pending debounced refresh (dropping the task cancels it).
    live_debounce: HashMap<PaneId, Task<()>>,
    /// Quick Look overlay over the pane area, and its event subscription.
    quicklook: Option<(Entity<PreviewView>, Subscription)>,
    /// `⌘P` palette over the pane area, its event subscription, and the terminal it was opened from.
    finder: Option<(Entity<FinderView>, Subscription, PaneId)>,
    /// `⌘⇧R` 会话 palette over the pane area, and its event subscription.
    sessions: Option<(Entity<SessionCenterView>, Subscription)>,
    /// `⌘⇧N` 新建 Agent panel over the pane area, and its event subscription.
    new_agent: Option<(Entity<NewAgentView>, Subscription)>,
    /// Files opened in Quick Look, for the palette's ranking.
    recent: Recent,
    /// Bounds of the pane area from the last frame (for divider dragging).
    content_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    drag: Option<(Vec<usize>, usize)>,
    error: Option<String>,
    /// A file the built-in editor refused to open (red banner below the tab bar).
    editor_refusal: Option<editor::EditorRefusal>,
    focus_handle: FocusHandle,
    sidebar: SidebarState,
    /// The pane flashing after `focus_pane`, and which flash it is.
    flash: Option<(PaneId, u64)>,
    flash_seq: u64,
    /// The sidebar row menu, and the row being renamed.
    session_menu: Option<SessionMenu>,
    renaming: Option<sessions::Renaming>,
    /// The sidebar's confirm bar before an ended session goes to the Trash (M3c §6).
    ended_confirm: Option<Confirm>,
    /// Whether the root's keyboard was seen since the confirm bar opened (it closes the bar once lost).
    ended_confirm_focus: crate::sidebar::ended::ConfirmFocus,
    /// The confirm bar before a close that would end a working or waiting agent (P0 spec §5).
    close_confirm: Option<close::CloseConfirm>,
    close_confirm_focus: crate::sidebar::ended::ConfirmFocus,
    /// The right-hand column (M3b).
    inspector: InspectorState,
    /// Kept alive so the window repaints when the system appearance (light/dark) changes;
    /// gpui only runs the registered observer, it does not re-render on its own.
    _appearance: Subscription,
    /// Bringing the window to the front counts as seeing its focused pane.
    _activation: Subscription,
    /// This frame's monitor model (computed once in `render` while a monitor pane is shown).
    monitor_frame: Option<Rc<crate::monitor::model::MonitorModel>>,
    /// Per-second redraw while the monitor draws cards (clocks and relative times).
    monitor_tick: Option<Task<()>>,
    /// The monitor chat input's events (⏎ sends, Esc goes back to the wall); set when the monitor pane is made.
    monitor_chat_sub: Option<Subscription>,
    /// The 监控官's bottom command bar (S2 §6.4): one per window; the conversation is the app's.
    command_bar: crate::monitor::command_bar::CommandBar,
}

const RESIZE_STEP: f32 = 0.05;

fn to_rect(b: Bounds<Pixels>) -> Rect {
    Rect::new(b.origin.x / px(1.), b.origin.y / px(1.), b.size.width / px(1.), b.size.height / px(1.))
}

/// Active tab index after removing tab `removed` from a list that now has `len` tabs (len > 0).
fn active_after_removal(active: usize, removed: usize, len: usize) -> usize {
    let active = if removed < active { active - 1 } else { active };
    active.min(len - 1)
}

/// Tab and pane a pinned preview splits: the tab holding `origin`, else the active tab's focused pane.
fn split_target(tabs: &[Tab], active: usize, origin: Option<PaneId>) -> Option<(usize, PaneId)> {
    origin
        .and_then(|o| tabs.iter().position(|t| t.tree.contains(o)).map(|ti| (ti, o)))
        .or_else(|| tabs.get(active).map(|t| (active, t.focused)))
}

impl Workspace {
    pub fn new(error: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut ws = Self::blank(error, window, cx);
        let home = std::env::var_os("HOME").map(PathBuf::from);
        ws.open_tab(home, window, cx);
        ws
    }

    /// A window with no tab yet.
    fn blank(error: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let command_bar = crate::monitor::command_bar::CommandBar::new(window, cx);
        let appearance = cx.observe_window_appearance(window, |_, window, _| window.refresh());
        crate::native::apply_appearance(window, cx);
        let activation = cx.observe_window_activation(window, |ws, window, cx| ws.report_focus(window, cx));
        let prefs = cx.try_global::<UiPrefs>().cloned().unwrap_or_default();
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |ws, cx| ws.should_close_window(window, cx)).unwrap_or(true)
        });
        Self {
            tabs: Vec::new(),
            active: 0,
            panes: HashMap::new(),
            subscriptions: HashMap::new(),
            live_pairs: Default::default(),
            live_debounce: HashMap::new(),
            quicklook: None,
            finder: None,
            sessions: None,
            new_agent: None,
            recent: Recent::default(),
            content_bounds: Rc::new(Cell::new(None)),
            drag: None,
            error,
            editor_refusal: None,
            focus_handle: cx.focus_handle(),
            sidebar: SidebarState::new(prefs.sidebar_hidden, prefs.grouping),
            flash: None,
            flash_seq: 0,
            session_menu: None,
            renaming: None,
            ended_confirm: None,
            ended_confirm_focus: Default::default(),
            close_confirm: None,
            close_confirm_focus: Default::default(),
            inspector: InspectorState::new(&prefs, cx.focus_handle()),
            _appearance: appearance,
            _activation: activation,
            monitor_frame: None,
            monitor_tick: None,
            monitor_chat_sub: None,
            command_bar,
        }
    }

    pub fn set_pane_remote(&mut self, pane: PaneId, r: Option<crate::remote::PaneRemote>, cx: &mut Context<Self>) {
        if let Some(PaneView::Terminal(t)) = self.panes.get(&pane) { t.update(cx, |t, cx| t.set_remote(r, cx)); }
    }

    // used by later tasks (DebugState)
    #[allow(dead_code)]
    pub fn pane_remote(&self, pane: PaneId, cx: &App) -> Option<crate::remote::PaneRemote> {
        match self.panes.get(&pane) { Some(PaneView::Terminal(t)) => t.read(cx).remote().cloned(), _ => None }
    }

    pub fn has_pane(&self, id: PaneId) -> bool {
        self.panes.contains_key(&id)
    }

    pub fn sidebar(&self) -> &SidebarState {
        &self.sidebar
    }

    /// Changes this window's sidebar; hidden and grouping become the default for new windows.
    pub fn update_sidebar(&mut self, change: impl FnOnce(&mut SidebarState), cx: &mut Context<Self>) {
        change(&mut self.sidebar);
        if self.sidebar.hidden {
            self.ended_confirm = None;
        }
        UiPrefs::remember_sidebar(&self.sidebar, cx);
        cx.notify();
    }

    /// Shows `req` as a Quick Look overlay, or pinned as a pane to the right of `origin`
    /// (the pane that asked for it; the focused pane when unknown).
    pub fn open_preview(&mut self, req: OpenRequest, pin: bool, origin: Option<PaneId>, window: &mut Window, cx: &mut Context<Self>) {
        // A preview opened from elsewhere (`gilvt view`, a drop) would otherwise sit beneath the palette.
        self.ended_confirm = None;
        self.finder = None;
        self.sessions = None;
        self.new_agent = None;
        if let Some(Source::File(path)) = req.sources.get(req.index) {
            self.recent.push(path.clone());
        }
        let view = cx.new(|cx| PreviewView::new(req, pin, window, cx));
        if pin {
            self.insert_preview_pane(view, origin, window, cx);
            return;
        }
        let sub = cx.subscribe_in(&view, window, move |ws, _, event, window, cx| match event {
            PreviewEvent::Close | PreviewEvent::Leave => ws.close_quicklook(window, cx),
            PreviewEvent::Pin => ws.pin_quicklook(origin, false, window, cx),
            PreviewEvent::NewTab => ws.pin_quicklook(origin, true, window, cx),
            PreviewEvent::Opened(path) => ws.recent.push(path.clone()),
            // `open_editor` closes Quick Look once the editor exists; a refused file keeps it open under the banner.
            PreviewEvent::Edit { path, line, flip } => ws.open_editor(path.clone(), *line, origin, *flip, window, cx),
            PreviewEvent::OpenExternal { path, line } => open_in_editor(path, *line),
        });
        self.quicklook = Some((view, sub));
        self.focus_active(window, cx);
    }

    /// Opens the `⌘P` palette for terminal pane `origin`, searching from its cwd (else `$HOME`).
    fn open_finder(&mut self, origin: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if self.quicklook.is_some() {
            return;
        }
        let Some(pane) = self.panes.get(&origin) else { return };
        self.sessions = None;
        self.ended_confirm = None;
        self.new_agent = None;
        let cwd = pane.cwd(cx).or_else(|| std::env::var_os("HOME").map(PathBuf::from)).unwrap_or_else(|| PathBuf::from("/"));
        let recent = self.recent.clone();
        let view = cx.new(|cx| FinderView::new(cwd, recent, cx));
        let sub = cx.subscribe_in(&view, window, move |ws, _, event, window, cx| {
            ws.finder = None;
            match event {
                FinderEvent::Open { path, pin } => ws.open_preview(OpenRequest::file(path.clone(), None, None), *pin, Some(origin), window, cx),
                FinderEvent::Insert(text) => {
                    if let Some(PaneView::Terminal(t)) = ws.panes.get(&origin) {
                        t.update(cx, |t, _| t.paste_text(text));
                    }
                    ws.focus_active(window, cx);
                }
                FinderEvent::Close => ws.focus_active(window, cx),
            }
        });
        self.finder = Some((view, sub, origin));
        self.focus_active(window, cx);
    }

    fn close_quicklook(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.quicklook = None;
        self.focus_active(window, cx);
    }

    /// ⏎ pins Quick Look beside `origin`; `T` (`new_tab`) gives it a tab of its own right of the active one.
    fn pin_quicklook(&mut self, origin: Option<PaneId>, new_tab: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((view, _)) = self.quicklook.take() {
            view.update(cx, |v, cx| v.set_pinned(true, cx));
            let id = self.insert_preview_pane(view, origin, window, cx);
            if new_tab {
                self.move_pane_to_new_tab(id, window, cx);
            }
        }
    }

    /// Adds a pinned preview to the right of `origin` (see `split_target`), opening a tab if there is none.
    /// The keyboard stays with the pane it splits: a pinned preview is watched while you keep working.
    fn insert_preview_pane(&mut self, view: Entity<PreviewView>, origin: Option<PaneId>, window: &mut Window, cx: &mut Context<Self>) -> PaneId {
        let id = next_pane_id();
        view.update(cx, |v, _| v.pane = Some(id));
        let sub = cx.subscribe_in(&view, window, move |ws, _, event, window, cx| match event {
            PreviewEvent::Leave => ws.leave_preview(id, window, cx),
            PreviewEvent::Opened(path) => ws.recent.push(path.clone()),
            PreviewEvent::Edit { path, line, flip } => ws.open_editor(path.clone(), *line, Some(id), *flip, window, cx),
            PreviewEvent::OpenExternal { path, line } => open_in_editor(path, *line),
            PreviewEvent::NewTab => ws.move_pane_to_new_tab(id, window, cx),
            PreviewEvent::Close | PreviewEvent::Pin => {}
        });
        self.subscriptions.insert(id, sub);
        self.panes.insert(id, PaneView::Preview(view));
        match split_target(&self.tabs, self.active, origin) {
            Some((ti, target)) => {
                self.active = ti;
                let tab = &mut self.tabs[ti];
                tab.tree.split(target, id, Axis::Row);
                tab.focused = target;
                tab.zoomed = false;
            }
            None => {
                self.tabs.push(Tab::new(PaneTree::new(id), id));
                self.active = self.tabs.len() - 1;
            }
        }
        self.focus_active(window, cx);
        id
    }

    /// Moves the keyboard from preview or editor pane `id` to its nearest terminal (left first); with none beside
    /// it (a preview moved to a tab of its own), back to the tab it was moved from.
    fn leave_preview(&mut self, id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bounds) = self.content_bounds.get() else { return };
        let Some(tab) = self.tabs.iter_mut().find(|t| t.tree.panes().contains(&id)) else { return };
        let next = [Dir::Left, Dir::Up, Dir::Right, Dir::Down]
            .into_iter()
            .filter_map(|d| tab.tree.neighbor(id, d, to_rect(bounds)))
            .find(|p| matches!(self.panes.get(p), Some(PaneView::Terminal(_))));
        if let Some(next) = next {
            tab.focused = next;
            cx.notify();
            self.focus_active(window, cx);
        } else if let Some(origin) = self.origin_tab_of(id) {
            self.activate(origin, window, cx);
        }
    }

    fn settings(cx: &App) -> Settings {
        cx.global::<AppSettings>().0.clone()
    }

    /// Creates a terminal view; on failure records the error and returns None.
    fn spawn_pane(&mut self, cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Option<PaneId> {
        self.spawn_pane_with_id(next_pane_id(), cwd, window, cx)
    }

    /// Like `spawn_pane`, with a given id (restoring a saved layout).
    fn spawn_pane_with_id(&mut self, id: PaneId, cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Option<PaneId> {
        let session = match spawn_session(&Self::settings(cx), cx.global::<ShellEnv>(), id, cwd) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(format!("无法启动 shell：{e}"));
                cx.notify();
                return None;
            }
        };
        let view = cx.new(|cx| TerminalView::new(session, window, cx));
        let sub = cx.subscribe_in(&view, window, move |ws, _, event, window, cx| match event {
            TerminalViewEvent::TitleChanged => {
                ws.sync_title(window, cx);
                cx.notify();
            }
            TerminalViewEvent::Exited => ws.close_pane(id, window, cx),
            TerminalViewEvent::RemoteEnded { link } => {
                let link = link.clone();
                cx.defer(move |cx| crate::remote::link_ended(&link, cx));
            }
            TerminalViewEvent::OpenPath { hit, in_editor } => {
                if *in_editor {
                    ws.open_editor(hit.path.clone(), hit.line, Some(id), false, window, cx);
                } else {
                    let req = OpenRequest::file(hit.path.clone(), hit.line, hit.annotation.clone());
                    ws.open_preview(req, false, Some(id), window, cx);
                }
            }
            TerminalViewEvent::FindFile => ws.open_finder(id, window, cx),
            TerminalViewEvent::Preview(files) => ws.open_preview(OpenRequest::files(files.clone()), false, Some(id), window, cx),
            TerminalViewEvent::Notify { note, title, focused } => crate::notify::osc(id, note.clone(), title.clone(), *focused, cx),
            // Command blocks are drawn only by monitor tabs, in any window.
            TerminalViewEvent::CommandsChanged => cx.defer(nav::notify_monitors),
            // Deferred: answering reads every window's visible panes, this one is being updated.
            TerminalViewEvent::Key(key) => {
                let key = *key;
                cx.defer(move |cx| Agents::pane_key(id, key, cx));
            }
        });
        self.panes.insert(id, PaneView::Terminal(view));
        self.subscriptions.insert(id, sub);
        Some(id)
    }

    fn focused_cwd(&self, cx: &App) -> Option<PathBuf> {
        let tab = self.tabs.get(self.active)?;
        self.panes.get(&tab.focused)?.cwd(cx)
    }

    /// Opens a tab with a new terminal in `cwd`; returns the terminal's pane.
    fn open_tab(&mut self, cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Option<PaneId> {
        let id = self.spawn_pane(cwd, window, cx)?;
        self.tabs.push(Tab::new(PaneTree::new(id), id));
        self.active = self.tabs.len() - 1;
        self.focus_active(window, cx);
        Some(id)
    }

    /// Focuses the palette (⌘P, else ⌘⇧R, else ⌘⇧N), else Quick Look, else the active tab's focused pane. Every change of
    /// active tab or closed pane ends here, so this is also where a palette whose terminal is gone
    /// or no longer shown closes (⌥⏎ must not paste into a terminal the user cannot see).
    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(&(_, _, origin)) = self.finder.as_ref() {
            let shown = self.tabs.get(self.active).map(|t| if t.zoomed { vec![t.focused] } else { t.tree.panes() });
            if !palette_stays(origin, self.panes.contains_key(&origin), shown.as_deref().unwrap_or(&[])) {
                self.finder = None;
            }
        }
        if let Some((finder, _, _)) = &self.finder {
            window.focus(&finder.focus_handle(cx));
        } else if let Some((sessions, _)) = &self.sessions {
            window.focus(&sessions.focus_handle(cx));
        } else if let Some((panel, _)) = &self.new_agent {
            window.focus(&panel.focus_handle(cx));
        } else if self.inspector.config.disclosure.dialog.is_some() {
            window.focus(&self.inspector.config.dialog_focus);
        } else if let Some((ql, _)) = &self.quicklook {
            window.focus(&ql.focus_handle(cx));
        } else if let Some(pane) = self.tabs.get(self.active).and_then(|t| self.panes.get(&t.focused)) {
            window.focus(&pane.focus_handle(cx));
        }
        self.sync_title(window, cx);
        self.report_focus(window, cx);
        cx.notify();
    }

    fn sync_title(&self, window: &mut Window, cx: &App) {
        if let Some(pane) = self.tabs.get(self.active).and_then(|t| self.panes.get(&t.focused)) {
            window.set_window_title(&pane.title(cx));
        }
    }

    fn close_pane(&mut self, id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        self.live_pair_closing(id, window, cx);
        let Some(ti) = self.tabs.iter().position(|t| t.tree.contains(id)) else { return };
        self.panes.remove(&id);
        self.subscriptions.remove(&id);
        Agents::pane_closed(id, cx);
        let tab = &mut self.tabs[ti];
        if tab.tree.remove(id) {
            if tab.focused == id {
                if let Some(&first) = tab.tree.panes().first() {
                    tab.focused = first;
                }
            }
            tab.zoomed = false;
        } else {
            self.remove_tab(ti, window);
            if self.tabs.is_empty() {
                return;
            }
        }
        // Panes also close by themselves (a shell exits, an editor or its live preview goes): an open command bar
        // keeps the keyboard then.
        self.focus_active_keeping_bar(window, cx);
    }

    /// Drops tab `ti` and picks the active tab: a closed file tab returns to the tab it was opened from
    /// (else its left neighbour); otherwise the same tab stays active. Closes the window when no tab is left.
    fn remove_tab(&mut self, ti: usize, window: &mut Window) {
        let tab = self.tabs.remove(ti);
        if self.tabs.is_empty() {
            window.remove_window();
            return;
        }
        self.active = match tab.origin_tab {
            Some(origin) if ti == self.active => {
                let ids: Vec<u64> = self.tabs.iter().map(|t| t.id).collect();
                editor::tab_after_close(&ids, ti, Some(origin))
            }
            _ => active_after_removal(self.active, ti, self.tabs.len()),
        };
    }

    fn close_tab_at(&mut self, ti: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ti) else { return };
        for id in tab.tree.panes() {
            // The whole tab goes, so both halves of a pair do: just forget the pairing.
            self.live_debounce.remove(&id);
            self.live_pairs.remove_editor(id);
            self.live_pairs.remove_preview(id);
            self.panes.remove(&id);
            self.subscriptions.remove(&id);
            Agents::pane_closed(id, cx);
        }
        self.remove_tab(ti, window);
        if self.tabs.is_empty() {
            return;
        }
        self.focus_active(window, cx);
    }

    /// ⌘D / ⌘⇧D: splits the focused pane with a terminal in the same directory.
    fn split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.focused_cwd(cx);
        self.split_in(axis, cwd, window, cx);
    }

    /// Splits the focused pane with a new terminal in `cwd` and focuses it; returns the terminal's pane.
    fn split_in(&mut self, axis: Axis, cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Option<PaneId> {
        self.tabs.get(self.active)?;
        let new = self.spawn_pane(cwd, window, cx)?;
        let tab = self.tabs.get_mut(self.active)?;
        tab.tree.split(tab.focused, new, axis);
        tab.focused = new;
        tab.zoomed = false;
        self.focus_active(window, cx);
        Some(new)
    }

    fn move_focus(&mut self, dir: Dir, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bounds) = self.content_bounds.get() else { return };
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if let Some(next) = tab.tree.neighbor(tab.focused, dir, to_rect(bounds)) {
            tab.focused = next;
            tab.zoomed = false;
            self.focus_active(window, cx);
        }
    }

    fn resize(&mut self, dir: Dir, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        tab.tree.resize(tab.focused, dir, RESIZE_STEP);
        cx.notify();
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            self.active = index;
            self.focus_active(window, cx);
        }
    }

    fn change_font_size(&mut self, delta: Option<f32>, window: &mut Window, cx: &mut Context<Self>) {
        let default = Settings::default().font_size;
        cx.update_global::<AppSettings, _>(|s, _| {
            s.0.font_size = match delta {
                Some(d) => (s.0.font_size + d).clamp(Settings::MIN_FONT_SIZE, Settings::MAX_FONT_SIZE),
                None => default,
            };
        });
        for handle in cx.windows() {
            // Ignore windows that are in the middle of closing or otherwise unavailable.
            let _ = handle.update(cx, |_, window, _| window.refresh());
        }
        window.refresh();
        cx.notify();
    }

    /// Shows `message` in the error banner over the pane area (dismissed by a click, like the startup error).
    pub fn show_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.error = Some(message);
        cx.notify();
    }

    fn on_drag_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (Some((path, index)), Some(bounds)) = (self.drag.clone(), self.content_bounds.get()) else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let Some(d) = tab.tree.dividers(to_rect(bounds)).into_iter().find(|d| d.path == path && d.index == index) else {
            return;
        };
        let pos = match d.axis {
            Axis::Row => event.position.x / px(1.),
            Axis::Column => event.position.y / px(1.),
        };
        tab.tree.drag_divider(&path, index, pos, d.split_rect);
        cx.notify();
    }

    fn render_node(&self, node: &Node, path: Vec<usize>, tab: &Tab, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = crate::terminal_view::TerminalView::palette(window, cx);
        let theme = crate::theme::current(cx);
        match node {
            Node::Leaf(id) => {
                let multi = tab.tree.panes().len() > 1 && !tab.zoomed;
                let ring = marks::ring_mark(cx.global::<Agents>().registry().by_pane(*id));
                let edge = marks::edge(ring, *id == tab.focused, multi);
                let border = match edge.border {
                    Some(marks::EdgeColor::Status(m)) => hsla(m.color(&theme.ui)),
                    Some(marks::EdgeColor::Focus) => hsla(p.ansi[4]),
                    None => hsla(p.background),
                };
                let pane = *id;
                // Capture phase: panes stop propagation of their own mouse downs. Clicking a pane
                // also closes the palettes, whose focus the pane is about to take.
                let mut el = div().size_full().relative().overflow_hidden().children(rects::recorder(RectId::Pane(pane))).capture_any_mouse_down(cx.listener(move |ws, _, window, cx| {
                    if ws.finder.take().is_some() | ws.sessions.take().is_some() | ws.new_agent.take().is_some() {
                        cx.notify();
                    }
                    if let Some(tab) = ws.tabs.get_mut(ws.active) {
                        if tab.focused != pane {
                            tab.focused = pane;
                            ws.report_focus(window, cx);
                            cx.notify();
                        }
                    }
                }));
                if multi {
                    el = el.border_1().border_color(border);
                }
                match self.panes.get(id) {
                    Some(PaneView::Monitor(m)) => el = el.child(crate::monitor::view::render(self, pane, m, window, cx)),
                    Some(pane_view) => el = el.children(pane_view.element()),
                    None => {}
                }
                // Over the pane's edge, inside the layout border: the terminal keeps its rows / columns.
                if let Some((mark, width)) = edge.ring {
                    el = el.child(div().absolute().size_full().top_0().left_0().border(px(width)).border_color(hsla(mark.color(&theme.ui))));
                }
                if let Some((_, seq)) = self.flash.filter(|(p, _)| p == id) {
                    let tint = if theme.dark { crate::theme::with_alpha(theme.ui.accent, 0.35) } else { crate::theme::with_alpha(theme.ui.selected, 0.7) };
                    el = el.child(
                        div()
                            .absolute()
                            .size_full()
                            .top_0()
                            .left_0()
                            .bg(tint)
                            .with_animation(("flash", seq), Animation::new(nav::FLASH), |el, t| el.opacity(1.0 - t)),
                    );
                }
                el.into_any_element()
            }
            Node::Split { axis, children, ratios } => {
                let mut container = div().flex().size_full();
                container = match axis {
                    Axis::Row => container.flex_row(),
                    Axis::Column => container.flex_col(),
                };
                let divider_color = hsla(mix(p.background, p.foreground, 0.2));
                for (i, (child, ratio)) in children.iter().zip(ratios).enumerate() {
                    let mut child_path = path.clone();
                    child_path.push(i);
                    let cell = match axis {
                        Axis::Row => div().h_full().w(relative(*ratio)),
                        Axis::Column => div().w_full().h(relative(*ratio)),
                    }
                    .flex_shrink()
                    .overflow_hidden()
                    .child(self.render_node(child, child_path, tab, window, cx));
                    container = container.child(cell);
                    if i + 1 < children.len() {
                        let divider_path = path.clone();
                        let handle = match axis {
                            Axis::Row => div().w(px(3.)).h_full().cursor(CursorStyle::ResizeLeftRight),
                            Axis::Column => div().h(px(3.)).w_full().cursor(CursorStyle::ResizeUpDown),
                        }
                        .flex_none()
                        .bg(divider_color)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ws, _, _, cx| {
                                ws.drag = Some((divider_path.clone(), i));
                                cx.stop_propagation();
                            }),
                        );
                        container = container.child(handle);
                    }
                }
                container.into_any_element()
            }
        }
    }

    fn render_tab_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = crate::terminal_view::TerminalView::palette(window, cx);
        let bar_bg = hsla(mix(p.background, p.foreground, 0.06));
        let active_bg = hsla(p.background);
        let muted = hsla(mix(p.foreground, p.background, 0.45));
        let theme = crate::theme::current(cx);
        let drop_bg = hsla(mix(mix(p.background, p.foreground, 0.06), p.ansi[4], 0.25));
        // A pane header dropped anywhere on the bar moves that pane into a tab of its own (`pane_drag.rs`).
        let mut bar = div()
            .id("tab-bar")
            .relative()
            .children(rects::recorder(RectId::TabBar))
            .flex()
            .flex_none()
            .h(px(30.))
            .items_end()
            .px_2()
            .gap_1()
            .bg(bar_bg)
            .text_size(px(12.))
            .drag_over::<pane_drag::DraggedPane>(move |style, _, _, _| style.bg(drop_bg))
            .on_drop(cx.listener(|ws, dragged: &pane_drag::DraggedPane, window, cx| ws.move_pane_to_new_tab(dragged.pane, window, cx)));
        for (i, tab) in self.tabs.iter().enumerate() {
            let title = self.tab_bar_title(tab, cx);
            let is_active = i == self.active;
            let registry = cx.global::<Agents>().registry();
            let dot = marks::tab_dot(tab.tree.panes().into_iter().filter_map(|p| registry.by_pane(p)));
            bar = bar.child(
                div()
                    .id(("tab", i))
                    .relative()
                    .children(rects::recorder(RectId::Tab(i)))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(26.))
                    .px_3()
                    .rounded_t_md()
                    .max_w(px(220.))
                    .overflow_hidden()
                    .bg(if is_active { active_bg } else { bar_bg })
                    .text_color(if is_active { hsla(p.foreground) } else { muted })
                    .on_click(cx.listener(move |ws, _, window, cx| ws.activate(i, window, cx)))
                    .children(dot.map(|m| div().flex_none().size(px(7.)).rounded_full().bg(hsla(m.color(&theme.ui)))))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .id(("close-tab", i))
                            .flex_none()
                            .px_1()
                            .text_color(muted)
                            .child("×")
                            .on_click(cx.listener(move |ws, _, window, cx| {
                                cx.stop_propagation();
                                ws.request_close(CloseAction::Tab(i), window, cx);
                            })),
                    ),
            );
        }
        bar.child(
            div()
                .id("new-tab")
                .px_2()
                .h(px(26.))
                .flex()
                .items_center()
                .text_color(muted)
                .child("+")
                .on_click(cx.listener(|ws, _, window, cx| {
                    let cwd = ws.focused_cwd(cx);
                    ws.open_tab(cwd, window, cx);
                })),
        )
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        rects::begin_frame(window, cx);
        let monitor_tick = self.prepare_monitor_frame(window, cx);
        self.sync_monitor_ticker(monitor_tick, cx);
        self.prepare_command_bar(window, cx);
        let p = crate::terminal_view::TerminalView::palette(window, cx);
        let content = match self.tabs.get(self.active) {
            Some(tab) if tab.zoomed => self.render_node(&Node::Leaf(tab.focused), Vec::new(), tab, window, cx),
            Some(tab) => self.render_node(tab.tree.root(), Vec::new(), tab, window, cx),
            None => div().into_any_element(),
        };
        let sidebar = (!self.sidebar.hidden).then(|| crate::sidebar::render(self, window, cx));
        let (inspector, tick) = match self.inspector.hidden {
            false => {
                let (el, tick) = crate::inspector::render(self, window, cx);
                (Some(el), tick)
            }
            true => (None, false),
        };
        self.sync_inspector_ticker(tick, cx);
        // A click into a pane (or anything else that took the keyboard from the root) dismisses the trash bar.
        // The frame the bar opens in may not see the root focused yet (it waits for the root to have been seen).
        if self.ended_confirm.is_some() {
            use crate::sidebar::ended::{confirm_frame, ConfirmFocus, ConfirmFrame};
            match confirm_frame(self.ended_confirm_focus, self.focus_handle.is_focused(window)) {
                ConfirmFrame::Keep(state) => self.ended_confirm_focus = state,
                ConfirmFrame::Refocus => {
                    self.ended_confirm_focus = ConfirmFocus::Retried;
                    cx.defer_in(window, |ws, window, cx| {
                        if ws.ended_confirm.is_some() {
                            window.focus(&ws.focus_handle);
                            cx.notify();
                        }
                    });
                }
                ConfirmFrame::Dismiss => self.ended_confirm = None,
            }
        }
        if self.close_confirm.is_some() {
            use crate::sidebar::ended::{confirm_frame, ConfirmFocus, ConfirmFrame};
            match confirm_frame(self.close_confirm_focus, self.focus_handle.is_focused(window)) {
                ConfirmFrame::Keep(state) => self.close_confirm_focus = state,
                ConfirmFrame::Refocus => {
                    self.close_confirm_focus = ConfirmFocus::Retried;
                    cx.defer_in(window, |ws, window, cx| {
                        if ws.close_confirm.is_some() {
                            window.focus(&ws.focus_handle);
                            cx.notify();
                        }
                    });
                }
                ConfirmFrame::Dismiss => self.close_confirm = None,
            }
        }
        let menu = self.session_menu.as_ref().map(|m| crate::sidebar::menu::render(m, window, cx));
        let root = div().size_full().flex().bg(hsla(p.background)).text_color(hsla(p.foreground)).track_focus(&self.focus_handle);
        Self::inspector_actions(Self::session_actions(root, cx), cx)
            .on_action(cx.listener(|ws, _: &NewTab, window, cx| {
                let cwd = ws.focused_cwd(cx);
                ws.open_tab(cwd, window, cx);
            }))
            .on_action(cx.listener(|ws, _: &ClosePane, window, cx| ws.close_pane_action(window, cx)))
            .on_action(cx.listener(|ws, _: &CloseTab, window, cx| ws.request_close(CloseAction::Tab(ws.active), window, cx)))
            .on_action(cx.listener(|ws, _: &OpenSessions, window, cx| ws.toggle_sessions(window, cx)))
            .on_action(cx.listener(|ws, _: &OpenCleanup, window, cx| ws.open_cleanup(window, cx)))
            .on_action(cx.listener(|ws, _: &NewAgent, window, cx| ws.toggle_new_agent(window, cx)))
            .on_action(cx.listener(|ws, _: &OpenMonitor, window, cx| ws.open_monitor(window, cx)))
            // Always registered (also with the 监控官 off, where it does nothing): the root handling the chord is what
            // keeps ⌘⇧M out of a focused terminal's `key_down`, and it enables the 会话 menu item.
            .on_action(cx.listener(|ws, _: &ToggleCommandBar, window, cx| ws.command_bar_event(crate::monitor::command_bar::BarEvent::Toggle, window, cx)))
            .on_action(cx.listener(|ws, _: &SplitRight, window, cx| ws.split(Axis::Row, window, cx)))
            .on_action(cx.listener(|ws, _: &SplitDown, window, cx| ws.split(Axis::Column, window, cx)))
            .on_action(cx.listener(|ws, _: &FocusLeft, window, cx| ws.move_focus(Dir::Left, window, cx)))
            .on_action(cx.listener(|ws, _: &FocusRight, window, cx| ws.move_focus(Dir::Right, window, cx)))
            .on_action(cx.listener(|ws, _: &FocusUp, window, cx| ws.move_focus(Dir::Up, window, cx)))
            .on_action(cx.listener(|ws, _: &FocusDown, window, cx| ws.move_focus(Dir::Down, window, cx)))
            .on_action(cx.listener(|ws, _: &ResizeLeft, _, cx| ws.resize(Dir::Left, cx)))
            .on_action(cx.listener(|ws, _: &ResizeRight, _, cx| ws.resize(Dir::Right, cx)))
            .on_action(cx.listener(|ws, _: &ResizeUp, _, cx| ws.resize(Dir::Up, cx)))
            .on_action(cx.listener(|ws, _: &ResizeDown, _, cx| ws.resize(Dir::Down, cx)))
            .on_action(cx.listener(|ws, _: &ZoomPane, window, cx| {
                if let Some(tab) = ws.tabs.get_mut(ws.active) {
                    tab.zoomed = !tab.zoomed && tab.tree.panes().len() > 1;
                }
                ws.focus_active(window, cx);
            }))
            .on_action(cx.listener(|ws, _: &NextTab, window, cx| {
                let n = ws.tabs.len();
                if n == 0 {
                    return;
                }
                ws.activate((ws.active + 1) % n, window, cx)
            }))
            .on_action(cx.listener(|ws, _: &PrevTab, window, cx| {
                let n = ws.tabs.len();
                if n == 0 {
                    return;
                }
                ws.activate((ws.active + n - 1) % n, window, cx)
            }))
            .on_action(cx.listener(|ws, _: &Tab1, w, cx| ws.activate(0, w, cx)))
            .on_action(cx.listener(|ws, _: &Tab2, w, cx| ws.activate(1, w, cx)))
            .on_action(cx.listener(|ws, _: &Tab3, w, cx| ws.activate(2, w, cx)))
            .on_action(cx.listener(|ws, _: &Tab4, w, cx| ws.activate(3, w, cx)))
            .on_action(cx.listener(|ws, _: &Tab5, w, cx| ws.activate(4, w, cx)))
            .on_action(cx.listener(|ws, _: &Tab6, w, cx| ws.activate(5, w, cx)))
            .on_action(cx.listener(|ws, _: &Tab7, w, cx| ws.activate(6, w, cx)))
            .on_action(cx.listener(|ws, _: &Tab8, w, cx| ws.activate(7, w, cx)))
            .on_action(cx.listener(|ws, _: &Tab9, w, cx| {
                let last = ws.tabs.len().saturating_sub(1);
                ws.activate(last, w, cx)
            }))
            .on_action(cx.listener(|ws, _: &IncreaseFontSize, w, cx| ws.change_font_size(Some(1.0), w, cx)))
            .on_action(cx.listener(|ws, _: &DecreaseFontSize, w, cx| ws.change_font_size(Some(-1.0), w, cx)))
            .on_action(cx.listener(|ws, _: &ResetFontSize, w, cx| ws.change_font_size(None, w, cx)))
            .on_mouse_move(cx.listener(Self::on_drag_move))
            .on_mouse_up(MouseButton::Left, cx.listener(|ws, _, _, _| ws.drag = None))
            .children(sidebar)
            .child(self.render_main(content, window, cx))
            .children(inspector)
            .children(menu)
    }
}

impl Workspace {
    /// Tab bar, error and editor-refusal banners and the pane area with its overlays.
    fn render_main(&self, content: AnyElement, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = crate::terminal_view::TerminalView::palette(window, cx);
        let bounds_cell = self.content_bounds.clone();
        div()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .flex()
            .flex_col()
            .children(self.close_confirm.as_ref().map(|c| self.render_close_confirm(c, &p, cx)))
            .child(self.render_tab_bar(window, cx))
            .children(self.error.clone().map(|e| {
                div()
                    .id("error")
                    .flex_none()
                    .px_3()
                    .py_1()
                    .text_size(px(12.))
                    .bg(hsla(mix(p.background, p.ansi[1], 0.25)))
                    .child(format!("{e}（点击关闭）"))
                    .on_click(cx.listener(|ws, _, _, cx| {
                        ws.error = None;
                        cx.notify();
                    }))
            }))
            .children(crate::install_notice::render(&p, cx))
            .children(self.editor_refusal.as_ref().map(|r| self.render_editor_refusal(r, &p, cx)))
            .child(
                div()
                    .flex_1()
                    .relative()
                    .overflow_hidden()
                    .child(
                        canvas(move |bounds, _, _| bounds_cell.set(Some(bounds)), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(content)
                    .children(self.quicklook.as_ref().map(|(ql, _)| {
                        div()
                            .occlude()
                            .absolute()
                            .top(px(16.))
                            .bottom(px(16.))
                            .left(px(24.))
                            .right(px(24.))
                            .rounded_lg()
                            .overflow_hidden()
                            .border_1()
                            .border_color(hsla(mix(p.background, p.foreground, 0.3)))
                            .shadow_lg()
                            .child(ql.clone())
                    }))
                    .children(self.finder.as_ref().map(|(finder, _, _)| palette_frame(&p, PALETTE_WIDTH, finder.clone().into_any_element())))
                    .children(self.sessions.as_ref().map(|(view, _)| palette_frame(&p, view.read(cx).frame_width(window), view.clone().into_any_element())))
                    .children(self.new_agent.as_ref().map(|(view, _)| palette_frame(&p, PANEL_WIDTH, view.clone().into_any_element())))
                    .children(crate::inspector::config_dialog(self, window, cx).map(|dialog| palette_frame(&p, 600., dialog)))
                    // The open command bar, over the bottom of the panes (S2 §6.4). Outside every pane, so what is
                    // typed in it never reaches a terminal's or the wall's key handler.
                    .children(crate::monitor::command_bar::popup(&self.command_bar, window, cx)),
            )
            // Its 20px line: the panes end above it.
            .children(crate::monitor::command_bar::line(&self.command_bar, window, cx))
    }
}

impl Workspace {
    /// The bar above the tabs: what would end, ［取消］（default） and ［仍然关闭］. With unsaved editor files (E2a §6)
    /// it lists them first, then any agents, and the buttons are ［取消］［不保存并关闭］［全部保存并关闭］（default）.
    fn render_close_confirm(&self, c: &close::CloseConfirm, p: &gilvt_term::Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let border = hsla(mix(p.background, p.foreground, 0.3));
        let button = |id: &'static str, label: &'static str| {
            div().id(id).flex_none().px_3().py(px(2.)).rounded(px(6.)).border_1().child(label)
        };
        let what = match c.action {
            CloseAction::Quit => crate::i18n::text(
                "退出 gilvt 会结束这些 agent：",
                "Quitting gilvt will stop these agents:",
            ),
            _ => crate::i18n::text("关闭会结束这些 agent：", "Closing will stop these agents:"),
        };
        let files = !c.dirty.is_empty();
        let heading = |text: &'static str| div().font_weight(FontWeight::SEMIBOLD).child(text);
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px_3()
            .py(px(8.))
            .text_size(px(12.))
            .border_b_1()
            .border_color(border)
            .bg(hsla(mix(p.background, p.ansi[1], 0.18)))
            .children(files.then(|| {
                heading(crate::i18n::text(
                    "关闭会丢失未保存的修改：",
                    "Closing will discard unsaved changes:",
                ))
            }))
            .children(
                close::dirty_lines(&c.dirty)
                    .into_iter()
                    .map(|line| div().truncate().child(line)),
            )
            .children((!files || !c.items.is_empty()).then(|| heading(what)))
            .children(c.items.iter().map(|s| div().truncate().child(close::item_line(s))))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child({
                        // The default (↩): 取消 on the agents-only bar; 全部保存并关闭 when files are listed (E2a §6).
                        let cancel =
                            button("close-confirm-cancel", crate::i18n::text("取消", "Cancel"));
                        let cancel = if files {
                            cancel.border_color(border)
                        } else {
                            cancel.border_color(hsla(p.foreground)).bg(hsla(mix(p.background, p.foreground, 0.15)))
                        };
                        cancel.on_click(cx.listener(|ws, _, window, cx| ws.cancel_close(window, cx)))
                    })
                    .child(
                        button(
                            "close-confirm-close",
                            if files {
                                crate::i18n::text("不保存并关闭", "Close Without Saving")
                            } else {
                                crate::i18n::text("仍然关闭", "Close Anyway")
                            },
                        )
                        .border_color(hsla(p.ansi[1]))
                        .text_color(hsla(p.ansi[1]))
                        .on_click(cx.listener(|ws, _, window, cx| ws.confirm_close(window, cx))),
                    )
                    .children(files.then(|| {
                        button(
                            "close-confirm-save",
                            crate::i18n::text("全部保存并关闭", "Save All and Close"),
                        )
                        .border_color(hsla(p.foreground))
                        .bg(hsla(mix(p.background, p.foreground, 0.15)))
                        .on_click(
                            cx.listener(|ws, _, window, cx| ws.save_all_and_close(window, cx)),
                        )
                    })),
            )
    }
}

/// Width of the ⌘P and ⌘⇧R palettes.
const PALETTE_WIDTH: f32 = 640.;
/// Width of the ⌘⇧N panel (the mockup's).
const PANEL_WIDTH: f32 = 520.;

/// The frame of the ⌘P and ⌘⇧R palettes and the ⌘⇧N panel: centered near the top of the pane area; only
/// the palette itself takes the mouse.
fn palette_frame(p: &gilvt_term::Palette, width: f32, palette: AnyElement) -> impl IntoElement {
    div().absolute().top(px(16.)).left(px(24.)).right(px(24.)).flex().justify_center().child(
        div()
            .occlude()
            .w(px(width))
            .max_w_full()
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(hsla(mix(p.background, p.foreground, 0.3)))
            .shadow_lg()
            .child(palette),
    )
}

#[cfg(test)]
mod tests {
    use super::{active_after_removal, split_target, Axis, PaneTree, Tab};

    #[test]
    fn removing_a_tab_keeps_the_same_tab_active() {
        // [A, B, C], B active; close A -> [B, C], B is index 0.
        assert_eq!(active_after_removal(1, 0, 2), 0);
        // Close the active tab B -> [A, C]; C takes its slot.
        assert_eq!(active_after_removal(1, 1, 2), 1);
        // Close C (right of active) -> active unchanged.
        assert_eq!(active_after_removal(1, 2, 2), 1);
        // Close the last tab while it is active -> clamp to the new last tab.
        assert_eq!(active_after_removal(2, 2, 2), 1);
    }

    #[test]
    fn pinned_previews_split_the_requesting_pane() {
        let mut first = PaneTree::new(1);
        first.split(1, 2, Axis::Row);
        let tabs = vec![Tab::new(first, 2), Tab::new(PaneTree::new(3), 3)];
        // Pane 1 lives in tab 0 even though tab 1 is active and pane 2 is focused there.
        assert_eq!(split_target(&tabs, 1, Some(1)), Some((0, 1)));
        // Unknown or missing origin: the active tab's focused pane.
        assert_eq!(split_target(&tabs, 1, Some(99)), Some((1, 3)));
        assert_eq!(split_target(&tabs, 0, None), Some((0, 2)));
        assert_eq!(split_target(&[], 0, Some(1)), None);
    }
}
