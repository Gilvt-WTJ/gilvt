//! The 会话 palette view (⌘⇧R, M3c spec §4): query box, filter chips, the virtualized list of past sessions,
//! the row menu, the confirm bar and inline rename. What it lists and does is `sessions_model`; this file
//! holds its state and acts on keys and clicks; `sessions_render` draws it. The Workspace shows it over the
//! pane area and acts on its events (resume, go to a pane, close).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gilvt_agent::{HistoryEntry, PaneId, RuntimeRef, SessionKey};
use gpui::{
    prelude::*, App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Pixels, Point,
    ScrollStrategy, Subscription, Task, UniformListScrollHandle, WeakEntity, Window,
};

use super::archive_sessions;
use super::cleanup::trash_sessions;
use super::dir_copy_text;
use super::projects::Projects;
use super::sessions_model::confirm_text_sizing;
use super::sessions_model::{
    self as model, command, confirm_text, escape, rename_initial, rename_value, when_label, Click, Esc, Filters, Input, Key,
    List, LiveAt, local, MenuItem, Project, Resume, Scope, Selection,
};
use super::{History, Location};
use crate::actions::{
    Paste, ResumeBelow, SessionCenterAll, SessionCenterNeedsYou, SessionCenterReview,
    SessionCenterRunning,
};
use crate::agents::Agents;
use crate::review::ReviewService;
use crate::debug_state::map;
use crate::debug_state::rects::{Rect4, RectId};
use crate::sidebar::rename::{RenameEvent, RenameField};
use crate::terminal_view::search_paste_text;
use crate::theme::AppSettings;
use crate::workspace::{live_sessions, Workspace};

const BANNER: Duration = Duration::from_secs(4);
/// The key hints' entry to the cleanup wizard.
pub(super) const CLEANUP_LABEL: &str = "清理… ⌘⇧K";

/// [`CLEANUP_LABEL`] in the interface language.
pub(super) fn cleanup_label() -> &'static str {
    crate::i18n::text(CLEANUP_LABEL, "Clean Up… ⌘⇧K")
}

/// The chips after the project chip, as drawn (`sessions_render.rs`): 全部项目, ≥ 7 天未活动, 已归档.
pub(super) fn filter_chips() -> [&'static str; 3] {
    let t = crate::i18n::text;
    [t("全部项目", "All projects"), t("≥ 7 天未活动", "Inactive ≥ 7 days"), t("已归档", "Archived")]
}


#[derive(Clone)]
pub enum SessionsEvent {
    /// Type a resume `command` at `location`; a new pane's shell starts in `dir`.
    Run { location: Location, dir: PathBuf, command: String },
    /// ↩ on a running session: go to its pane.
    Focus(PaneId),
    /// Navigate to an Agent running in another terminal application.
    Navigate(RuntimeRef),
    Close,
    SelectCenterTab(SessionCenterTab),
    /// ⌘⇧K / 「清理…」: the cleanup wizard in place of the palette.
    OpenCleanup,
    /// Esc in the cleanup wizard: back to the palette.
    CloseCleanup,
}

#[derive(Clone, Copy)]
pub enum SessionCenterTab {
    NeedsYou,
    Review,
    Running,
    All,
}

/// The row menu: which row, and where the right click was (window coordinates).
pub struct RowMenu {
    pub row: usize,
    pub at: Point<Pixels>,
}

/// The confirm bar: the sessions it would move, and its copy (also the sidebar's, for an ended session).
pub struct Confirm {
    /// Tells this bar from any other, so a companion size computed for a bar since cancelled or replaced is
    /// dropped.
    pub id: u64,
    pub entries: Vec<HistoryEntry>,
    pub text: String,
    /// The companion bytes; None while they are being sized in the background.
    companion: Option<u64>,
}

impl Confirm {
    /// The bar for `entries`, open at once: 「… + 附属 计算中…」 until [`Confirm::companion_sized`] (nothing
    /// walks the disk here; see [`Confirm::size_in_background`]).
    pub fn new(entries: Vec<HistoryEntry>) -> Confirm {
        let text = confirm_text_sizing(entries.len(), entries.iter().map(|e| e.size).sum());
        Confirm {
            id: next_confirm_id(),
            entries,
            text,
            companion: None,
        }
    }

    /// The same bar when the companion bytes are already known (the cleanup wizard sized them off the UI
    /// thread), so nothing walks the disk here.
    pub fn with_companion(entries: Vec<HistoryEntry>, companion_bytes: u64) -> Confirm {
        let text = confirm_text(entries.len(), entries.iter().map(|e| e.size).sum(), companion_bytes);
        Confirm {
            id: next_confirm_id(),
            entries,
            text,
            companion: Some(companion_bytes),
        }
    }

    /// The companion bytes are still being sized.
    pub fn sizing(&self) -> bool {
        self.companion.is_none()
    }

    /// The companion size computed for bar `id` arrived: fills in the copy when it is this bar's (still
    /// sizing). Returns whether anything changed.
    pub fn companion_sized(&mut self, id: u64, bytes: u64) -> bool {
        if id != self.id || !self.sizing() {
            return false;
        }
        self.companion = Some(bytes);
        self.text = confirm_text(
            self.entries.len(),
            self.entries.iter().map(|e| e.size).sum(),
            bytes,
        );
        true
    }

    /// Sizes the companion data of a bar just opened by [`Confirm::new`] on the background executor, then
    /// hands `(id, bytes)` to `apply` on the UI thread (which passes them to [`Confirm::companion_sized`] of
    /// whatever bar is open by then).
    pub fn size_in_background<V: 'static>(
        &self,
        cx: &mut Context<V>,
        apply: impl FnOnce(&mut V, u64, u64, &mut Context<V>) + 'static,
    ) {
        if !self.sizing() {
            return;
        }
        let (id, entries) = (self.id, self.entries.clone());
        cx.spawn(async move |this, cx| {
            let bytes = cx
                .background_executor()
                .spawn(async move { entries.iter().map(gilvt_agent::companion_size).sum::<u64>() })
                .await;
            let _ = this.update(cx, |view, cx| apply(view, id, bytes, cx));
        })
        .detach();
    }
}

fn next_confirm_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub struct Renaming {
    pub key: SessionKey,
    pub field: Entity<RenameField>,
    _events: Subscription,
}

/// What the list was built from; a new list is built when it changes.
#[derive(Clone, PartialEq)]
struct Stamp {
    entries: usize,
    query: String,
    filters: Filters,
    current: Option<Project>,
    live: HashMap<SessionKey, LiveAt>,
    projects: u64,
    dirs: u64,
    names: u64,
    review: u64,
    minute: u64,
}

pub struct SessionsView {
    pub(super) focus_handle: FocusHandle,
    /// The window's workspace: where running sessions' panes are (read while rendering).
    workspace: WeakEntity<Workspace>,
    /// The focused pane's cwd when opened: the current project.
    cwd: Option<PathBuf>,
    pub(super) current: Option<Project>,
    pub(super) query: String,
    /// IME composition, shown after the query.
    pub(super) marked: Option<String>,
    pub(super) filters: Filters,
    pub(super) entries: Arc<Vec<HistoryEntry>>,
    pub(super) list: List,
    stamp: Option<Stamp>,
    pub(super) selection: Selection,
    pub(super) scroll: UniformListScrollHandle,
    pub(super) menu: Option<RowMenu>,
    pub(super) confirm: Option<Confirm>,
    pub(super) renaming: Option<Renaming>,
    /// Bumped by every rename made here (titles changed).
    names: u64,
    /// The entries whose projects were asked for.
    resolved_for: usize,
    pub(super) banner: Option<String>,
    _banner: Option<Task<()>>,
}

impl EventEmitter<SessionsEvent> for SessionsView {}

impl Focusable for SessionsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SessionsView {
    /// `gilvt debug state`: the query, the cursor row, the rows listed and the filters applied.
    /// `rect` looks up the rects of the rows, chips and buttons recorded in the latest frame.
    pub fn debug_overlay(&self, rect: &dyn Fn(RectId) -> Option<Rect4>) -> crate::debug_state::Overlay {
        let effective = model::effective_scope(self.filters, self.current.as_ref());
        let scope = match effective {
            Scope::Current => "current",
            Scope::All => "all",
        };
        // (label, active, rect index): the project chip only shows with a current project.
        let mut chips = Vec::new();
        if let Some(c) = &self.current {
            chips.push((c.name.as_str(), effective == Scope::Current, 0));
        }
        let [all, stale, archived] = filter_chips();
        chips.push((all, effective == Scope::All, 1));
        chips.push((stale, self.filters.stale, 2));
        chips.push((archived, self.filters.archived, 3));
        let chips = chips.into_iter().map(|(label, active, n)| crate::debug_state::Chip { label: label.to_string(), active, rect: rect(RectId::PaletteChip(n)) }).collect();
        let confirm = self.confirm.as_ref().map(|c| crate::debug_state::Confirm {
            text: c.text.clone(),
            buttons: map::buttons(&map::confirm_buttons(), RectId::PaletteConfirmButton, rect),
        });
        crate::debug_state::Overlay::Sessions {
            query: self.query.clone(),
            selected: self.selection.cursor(),
            items: self.list.rows.len(),
            filter: crate::debug_state::SessionsFilter { scope, stale: self.filters.stale, archived: self.filters.archived },
            rows: map::palette_rows(&self.list.rows, self.selection.cursor(), &|k| self.selection.is_picked(k), rect),
            chips,
            cleanup: self.confirm.is_none().then(|| crate::debug_state::Button { label: cleanup_label().into(), rect: rect(RectId::PaletteCleanup) }),
            confirm,
            banner: self.banner.clone(),
        }
    }

    /// `gilvt debug state`: the row menu's items (label, checked, enabled), while it is open.
    pub fn debug_menu(&self) -> Option<Vec<(&'static str, bool, bool)>> {
        let menu = self.menu.as_ref()?;
        Some(model::menu_items(self.menu_trashable(menu.row), self.filters.archived).into_iter().map(|(_, label, enabled)| (label, false, enabled)).collect())
    }

    pub fn new(cwd: Option<PathBuf>, workspace: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        SessionsView {
            focus_handle: cx.focus_handle(),
            workspace,
            cwd,
            current: None,
            query: String::new(),
            marked: None,
            filters: Filters::default(),
            entries: Arc::default(),
            list: List::default(),
            stamp: None,
            selection: Selection::default(),
            scroll: UniformListScrollHandle::new(),
            menu: None,
            confirm: None,
            renaming: None,
            names: 0,
            resolved_for: 0,
            banner: None,
            _banner: None,
        }
    }

    /// Rebuilds the list when anything it shows changed (called by `render`). A refreshed history keeps the
    /// cursor on its session; a new query or filter starts from the first row.
    pub(super) fn sync(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.entries = cx.try_global::<History>().map(History::entries).unwrap_or_default();
        let entries_id = Arc::as_ptr(&self.entries) as usize;
        if entries_id != self.resolved_for {
            self.resolved_for = entries_id;
            let dirs = self.entries.iter().map(|e| e.cwd.clone()).chain(self.cwd.clone()).collect();
            Projects::resolve(dirs, cx);
        }
        let projects = cx.default_global::<Projects>();
        self.current = self.cwd.as_deref().map(|c| projects.get(c));
        let live = match self.workspace.upgrade() {
            Some(ws) => live_sessions(ws.read(cx), window.window_handle(), cx),
            None => HashMap::new(),
        };
        let now = SystemTime::now();
        let stamp = Stamp {
            entries: entries_id,
            query: self.query.clone(),
            filters: self.filters,
            current: self.current.clone(),
            live,
            projects: cx.global::<Projects>().generation(),
            dirs: cx.try_global::<History>().map_or(0, History::dirs_generation),
            names: self.names,
            review: cx.try_global::<ReviewService>().map_or(0, ReviewService::revision),
            minute: now.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 60),
        };
        if self.stamp.as_ref() == Some(&stamp) {
            return;
        }
        let follow = self.stamp.as_ref().is_some_and(|s| s.query == stamp.query && s.filters == stamp.filters);
        let registry = cx.try_global::<Agents>().map(Agents::registry);
        let projects = cx.global::<Projects>();
        let now_local = local(now);
        let history = cx.try_global::<History>();
        let agents = cx.try_global::<Agents>();
        let review_states = cx.try_global::<ReviewService>().map(|s| s.states(self.entries.iter().map(|e| (e.agent, e.session_id.clone())))).unwrap_or_default();
        let home = super::dir_label::label_home();
        self.list = model::build(&Input {
            entries: &self.entries,
            live: &stamp.live,
            saved_name: &|k| registry.and_then(|r| r.saved_name(k)),
            project: &|cwd| projects.get(cwd),
            current: self.current.as_ref(),
            query: &self.query,
            filters: self.filters,
            now,
            when: &|t| when_label(local(t), now_local, now.duration_since(t).unwrap_or_default()),
            home,
            dir_exists: &|p| history.is_none_or(|h| h.dir_exists(p)),
            // The sidebar's git cache (live sessions' directories): a cheap read, never a git call here.
            branch: &|p| agents.and_then(|a| a.git_of_dir(p)).and_then(|g| g.branch.clone()),
            archived: &|e| review_states.get(&(e.agent, e.session_id.clone())).is_some_and(|s| s.archived_for(e.turns)),
        });
        self.selection.sync(&self.list.rows, follow);
        if !follow {
            self.scroll_to_cursor();
        }
        self.stamp = Some(stamp);
    }

    fn scroll_to_cursor(&mut self) {
        if let Some(line) = self.list.line_of(self.selection.cursor()) {
            self.scroll.scroll_to_item(line, ScrollStrategy::Top);
        }
    }

    pub(super) fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        // Typing searches every project (spec §4.1).
        if !query.trim().is_empty() {
            self.filters.scope = Scope::All;
        }
        self.query = query;
        self.menu = None;
        cx.notify();
    }

    pub(super) fn set_filters(&mut self, change: impl FnOnce(&mut Filters), cx: &mut Context<Self>) {
        change(&mut self.filters);
        self.menu = None;
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() || self.renaming.is_some() {
            return; // The IME composition, or the rename field, owns the keyboard.
        }
        let Some(key) = command(&event.keystroke.key, event.keystroke.modifiers) else { return };
        match key {
            Key::Up | Key::Down => {
                self.menu = None;
                self.selection.step(&self.list.rows, key == Key::Down);
                self.scroll_to_cursor();
            }
            // Only a deliberate plain ↩ confirms (the sidebar bar's rule); ⌘↩ / ⇧↩ / a held ↩ do nothing.
            Key::Enter(_) if self.confirm.is_some() => {
                if model::confirms_trash(&event.keystroke.modifiers, event.is_held, self.menu.is_some()) {
                    self.trash_confirmed(cx);
                }
            }
            Key::Enter(location) => self.resume(location, cx),
            Key::Rename => self.start_rename(self.selection.cursor(), window, cx),
            Key::CopyId => self.copy_id(self.selection.cursor(), cx),
            Key::Archive if self.confirm.is_some() => {}
            Key::Archive => self.toggle_archive(None, cx),
            Key::Trash if self.confirm.is_some() => {}
            Key::Trash => self.ask_trash(None, cx),
            Key::Cleanup => cx.emit(SessionsEvent::OpenCleanup),
            Key::Escape => match escape(self.menu.is_some(), self.confirm.is_some()) {
                Esc::Menu => self.menu = None,
                Esc::Confirm => self.confirm = None,
                Esc::Palette => cx.emit(SessionsEvent::Close),
            },
            Key::Backspace => {
                let mut query = self.query.clone();
                if query.pop().is_some() {
                    self.set_query(query, cx);
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn resume_below(&mut self, _: &ResumeBelow, _: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.is_none() && self.confirm.is_none() {
            self.resume(Location::Below, cx);
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return };
        let query = self.query.clone() + &search_paste_text(&text);
        self.set_query(query, cx);
    }

    /// A click on row `ix` (double click: resume it in the smart location).
    pub(super) fn click(&mut self, ix: usize, click: Click, count: usize, cx: &mut Context<Self>) {
        self.menu = None;
        self.selection.click(&self.list.rows, ix, click);
        if click == Click::Plain && count >= 2 {
            self.resume(Location::Smart, cx);
        }
        cx.notify();
    }

    pub(super) fn open_menu(&mut self, row: usize, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.selection.set_cursor(&self.list.rows, row);
        self.menu = Some(RowMenu { row, at });
        cx.notify();
    }

    /// Whether the menu of row `row` has anything to move to the Trash.
    pub(super) fn menu_trashable(&self, row: usize) -> bool {
        !self.selection.targets(&self.list.rows, Some(row)).is_empty()
    }

    pub(super) fn menu_command(&mut self, item: MenuItem, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.selection.set_cursor(&self.list.rows, row);
        match item {
            MenuItem::Resume => self.resume(Location::Smart, cx),
            MenuItem::ResumeRight => self.resume(Location::Right, cx),
            MenuItem::Rename => self.start_rename(row, window, cx),
            MenuItem::CopyId => self.copy_id(row, cx),
            MenuItem::CopyDir => self.copy_dir(row, cx),
            MenuItem::Reveal => self.reveal(row, cx),
            MenuItem::Archive => self.toggle_archive(Some(row), cx),
            MenuItem::Trash => self.ask_trash(Some(row), cx),
        }
        cx.notify();
    }

    fn entry(&self, row: usize) -> Option<&HistoryEntry> {
        self.list.rows.get(row).and_then(|r| self.entries.get(r.entry))
    }

    /// ↩ / ⌘↩ / ⌘⇧↩ on the cursor row.
    fn resume(&mut self, location: Location, cx: &mut Context<Self>) {
        let row = self.selection.cursor();
        let (Some(e), Some(r)) = (self.entry(row), self.list.rows.get(row)) else { return };
        let launch = cx.global::<AppSettings>().0.agent.launch(e.agent);
        let outcome = model::resume(e, r.live.as_ref(), launch, Path::exists);
        match outcome {
            Resume::Focus(pane) => cx.emit(SessionsEvent::Focus(pane)),
            Resume::Navigate(runtime) => cx.emit(SessionsEvent::Navigate(runtime)),
            Resume::Run { dir, command } => cx.emit(SessionsEvent::Run { location, dir, command }),
            other => {
                if other == Resume::Gone {
                    History::refresh(cx);
                }
                if let Some(toast) = other.toast() {
                    self.show_banner(toast, cx);
                }
            }
        }
    }

    fn copy_id(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(id) = self.entry(row).map(|e| e.session_id.clone()) else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(id));
        self.show_banner(crate::i18n::text("已复制会话 ID", "Copied session ID").into(), cx);
    }

    fn copy_dir(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(dir) = self.entry(row).map(|e| dir_copy_text(&e.cwd)) else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(dir));
        self.show_banner(crate::i18n::text("已复制目录路径", "Copied directory path").into(), cx);
    }

    /// ⌘E / 菜单「归档」: archive the picked sessions (or the cursor row), or un-archive them under the 已归档
    /// filter. A running session is skipped (re-checked right before acting, like the Trash).
    fn toggle_archive(&mut self, at: Option<usize>, cx: &mut Context<Self>) {
        let unarchive = self.filters.archived;
        let targets = self.selection.targets(&self.list.rows, at);
        if targets.is_empty() {
            if !self.list.rows.is_empty() {
                self.show_banner(super::archive::running_not_archived().into(), cx);
            }
            return;
        }
        let entries: Vec<HistoryEntry> = targets.iter().filter_map(|&ix| self.entry(ix).cloned()).collect();
        let toast = if unarchive {
            self.unarchive(&entries, cx)
        } else {
            archive_sessions(&entries, cx).toast()
        };
        self.selection.clear_picks();
        self.names += 1; // the list is rebuilt (archived state changed)
        if let Some(toast) = toast {
            self.show_banner(toast, cx);
        }
        cx.notify();
    }

    /// 取消归档 under the 已归档 filter (running sessions are not archived, so none is skipped); the banner.
    fn unarchive(&mut self, entries: &[HistoryEntry], cx: &mut Context<Self>) -> Option<String> {
        if !cx.has_global::<ReviewService>() {
            return Some(super::archive::save_failed(super::archive::state_dir_unavailable()));
        }
        let (mut done, mut failed) = (0, None);
        for e in entries {
            let key: SessionKey = (e.agent, e.session_id.clone());
            match cx.global_mut::<ReviewService>().clear_archived(&key) {
                Ok(()) => done += 1,
                Err(err) => failed = Some(err),
            }
        }
        if let Some(err) = failed {
            Some(super::archive::save_failed(err))
        } else {
            (done > 0).then(|| {
                if crate::i18n::english() { format!("Unarchived {done} sessions") } else { format!("已取消归档 {done} 个会话") }
            })
        }
    }

    /// 在访达中显示: the transcript, selected in a Finder window.
    fn reveal(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(path) = self.entry(row).map(|e| e.transcript.clone()) else { return };
        let spawned = std::process::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match spawned {
            // Reap it off the main thread, so no zombie is left per use.
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => self.show_banner(
                if crate::i18n::english() { format!("Could not open Finder: {e}") } else { format!("无法打开访达：{e}") },
                cx,
            ),
        }
    }

    /// ⌘⌫ (`at` None) or the menu of row `at`: the confirm bar for what it would move.
    fn ask_trash(&mut self, at: Option<usize>, cx: &mut Context<Self>) {
        let targets = self.selection.targets(&self.list.rows, at);
        if targets.is_empty() {
            if !self.list.rows.is_empty() {
                self.show_banner(crate::i18n::text("运行中的会话不能移到废纸篓", "Running sessions cannot be moved to Trash").into(), cx);
            }
            return;
        }
        let entries: Vec<HistoryEntry> = targets.iter().filter_map(|&i| self.entry(i).cloned()).collect();
        let confirm = Confirm::new(entries);
        confirm.size_in_background(cx, |view: &mut SessionsView, id, bytes, cx| {
            if view
                .confirm
                .as_mut()
                .is_some_and(|c| c.companion_sized(id, bytes))
            {
                cx.notify();
            }
        });
        self.confirm = Some(confirm);
        cx.notify();
    }

    /// 移到废纸篓 on the confirm bar (or ↩ while it shows).
    pub(super) fn trash_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm.take() else { return };
        let report = trash_sessions(confirm.entries, cx);
        self.selection.clear_picks();
        if let Some(toast) = report.toast() {
            self.show_banner(toast, cx);
        }
        cx.notify();
    }

    pub(super) fn cancel_confirm(&mut self, cx: &mut Context<Self>) {
        self.confirm = None;
        cx.notify();
    }

    /// ⌘R / 重命名…: the rename field in row `row`. ⏎ saves to the M3a store (the sidebar follows).
    fn start_rename(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.entry(row).cloned() else { return };
        let key: SessionKey = (e.agent, e.session_id.clone());
        let saved = cx.try_global::<Agents>().and_then(|a| a.registry().saved_name(&key));
        // What the row is called without a rename: committing that unchanged must not freeze it as one.
        let auto = crate::launcher::history::title(&e, None);
        let initial = rename_initial(saved.as_deref(), &auto);
        let field = cx.new(|cx| RenameField::new(&initial, window, cx));
        let id = field.entity_id();
        let events = cx.subscribe_in(&field, window, move |v, _, event, window, cx| {
            if v.renaming.as_ref().is_none_or(|r| r.field.entity_id() != id) {
                return;
            }
            let Some(r) = v.renaming.take() else { return };
            if let RenameEvent::Commit(name) = event {
                if let Some(value) = rename_value(name, saved.as_deref(), &auto) {
                    Agents::rename(&r.key, value, cx);
                    v.names += 1;
                }
            }
            if event.refocus_terminal() {
                window.focus(&v.focus_handle);
            }
            cx.notify();
        });
        window.focus(&field.focus_handle(cx));
        self.renaming = Some(Renaming { key, field, _events: events });
        cx.notify();
    }

    pub(super) fn show_banner(&mut self, text: String, cx: &mut Context<Self>) {
        self.banner = Some(text);
        self._banner = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BANNER).await;
            let _ = this.update(cx, |v, cx| {
                v.banner = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The picked rows' count and bytes (the line above the list).
    pub(super) fn picks(&self) -> (usize, u64) {
        let picked = self.selection.picked(&self.list.rows);
        (picked.len(), picked.iter().map(|&i| self.list.rows[i].size).sum())
    }

    pub(super) fn listeners(&self, root: gpui::Stateful<gpui::Div>, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        root.on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::resume_below))
            .on_action(cx.listener(|_, _: &SessionCenterNeedsYou, _, cx| {
                cx.emit(SessionsEvent::SelectCenterTab(SessionCenterTab::NeedsYou))
            }))
            .on_action(cx.listener(|_, _: &SessionCenterReview, _, cx| {
                cx.emit(SessionsEvent::SelectCenterTab(SessionCenterTab::Review))
            }))
            .on_action(cx.listener(|_, _: &SessionCenterRunning, _, cx| {
                cx.emit(SessionsEvent::SelectCenterTab(SessionCenterTab::Running))
            }))
            .on_action(cx.listener(|_, _: &SessionCenterAll, _, cx| {
                cx.emit(SessionsEvent::SelectCenterTab(SessionCenterTab::All))
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_mirrors_follow_the_language() {
        assert_eq!(filter_chips(), ["全部项目", "≥ 7 天未活动", "已归档"]);
        assert_eq!((cleanup_label(), map::confirm_buttons()), (CLEANUP_LABEL, map::CONFIRM_BUTTONS));
        crate::i18n::with_language(crate::i18n::Language::English, || {
            assert_eq!(filter_chips(), ["All projects", "Inactive ≥ 7 days", "Archived"]);
            assert_eq!(cleanup_label(), "Clean Up… ⌘⇧K");
            assert_eq!(map::confirm_buttons(), ["Cancel", "Move to Trash"]);
        });
    }
}
