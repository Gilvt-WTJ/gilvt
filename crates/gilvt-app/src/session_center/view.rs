use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use gilvt_agent::{HistoryEntry, ReviewState, SessionKey};
use gpui::{
    prelude::*, App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    Point, ScrollHandle, ScrollStrategy, Subscription, UTF16Selection, UniformListScrollHandle,
    WeakEntity, Window,
};

use super::model::{self, List, Selection, Tab};
use crate::actions::{
    Paste, SessionCenterAll, SessionCenterNeedsYou, SessionCenterReview, SessionCenterRunning,
};
use crate::agents::Agents;
use crate::launcher::cleanup_wizard::CleanupWizard;
use crate::launcher::sessions_model::{self as sessions_model, Resume};
use crate::launcher::sessions_view::{SessionCenterTab, SessionsEvent, SessionsView};
use crate::launcher::{History, HistorySnapshot, Location};
use crate::review::{build_inbox, ReviewService, ReviewSort};
use crate::session_review::model::{key_action, Layout, Mods, Scope, Surface};
use crate::terminal_view::search_paste_text;
use crate::theme::AppSettings;
use crate::workspace::Workspace;

pub struct SessionCenterView {
    pub(crate) focus_handle: FocusHandle,
    pub(super) all_sessions: Entity<SessionsView>,
    _all_events: Subscription,
    /// The cleanup wizard (⌘⇧K), shown in place of the All tab's palette while open.
    pub(super) cleanup: Option<(Entity<CleanupWizard>, Subscription)>,
    pub(crate) tab: Tab,
    pub(crate) query: String,
    pub(crate) marked: Option<String>,
    pub(crate) sort: ReviewSort,
    pub(crate) list: List,
    pub(crate) selection: Selection,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) snapshot: HistorySnapshot,
    /// The open read-only review, if any. Opening it never changes the persisted cursor.
    pub(crate) surface: Option<Surface>,
    /// The last failed save, shown in the review until the next successful action.
    pub(crate) notice: Option<String>,
    /// The exact external session awaiting the second terminate click.
    pub(crate) terminate_confirm: Option<SessionKey>,
    pub(crate) detail_scroll: ScrollHandle,
    /// The layout of the latest frame (the render records it; DebugState reports it).
    pub(crate) layout: Layout,
    stamp: Option<ProjectionStamp>,
}

#[derive(Clone, PartialEq, Eq)]
struct ProjectionStamp {
    generation: u64,
    refreshing: bool,
    language: crate::i18n::Language,
    tab: Tab,
    query: String,
    sort: ReviewSort,
    minute: u64,
    live: Vec<model::LiveSession>,
    states: HashMap<SessionKey, ReviewState>,
    saved_names: HashMap<SessionKey, String>,
}

impl EventEmitter<SessionsEvent> for SessionCenterView {}

impl Focusable for SessionCenterView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match (self.tab, &self.cleanup) {
            (Tab::All, Some((wizard, _))) => wizard.focus_handle(cx),
            (Tab::All, None) => self.all_sessions.focus_handle(cx),
            _ => self.focus_handle.clone(),
        }
    }
}

impl SessionCenterView {
    pub fn new(
        cwd: Option<PathBuf>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let all_sessions = cx.new(|cx| SessionsView::new(cwd, workspace, cx));
        let all_events = cx.subscribe_in(&all_sessions, window, |view, _, event, window, cx| {
            match event {
                SessionsEvent::SelectCenterTab(tab) => view.set_tab(
                    match tab {
                        SessionCenterTab::NeedsYou => Tab::NeedsYou,
                        SessionCenterTab::Review => Tab::Review,
                        SessionCenterTab::Running => Tab::Running,
                        SessionCenterTab::All => Tab::All,
                    },
                    window,
                    cx,
                ),
                SessionsEvent::OpenCleanup => view.open_cleanup(window, cx),
                _ => cx.emit(event.clone()),
            }
        });
        let snapshot = cx.global::<History>().snapshot();
        SessionCenterView {
            focus_handle: cx.focus_handle(),
            all_sessions,
            _all_events: all_events,
            cleanup: None,
            tab: Tab::All,
            query: String::new(),
            marked: None,
            sort: ReviewSort::Smart,
            list: List::default(),
            selection: Selection::default(),
            scroll: UniformListScrollHandle::new(),
            snapshot,
            surface: None,
            notice: None,
            terminate_confirm: None,
            detail_scroll: ScrollHandle::new(),
            layout: Layout::Wide,
            stamp: None,
        }
    }

    /// Width of the palette frame: wider while a review sits beside the queue.
    pub fn frame_width(&self, window: &Window) -> f32 {
        let reviewing = self.surface.is_some() && self.tab != Tab::All;
        crate::session_review::model::frame_width(f32::from(window.viewport_size().width), reviewing)
    }

    /// Forces the next `sync` to rebuild the projection (the review store changed under it).
    pub(crate) fn invalidate(&mut self) {
        self.stamp = None;
    }

    pub fn debug_menu(&self, cx: &App) -> Option<Vec<(&'static str, bool, bool)>> {
        match self.tab {
            Tab::All if self.cleanup.is_none() => self.all_sessions.read(cx).debug_menu(),
            _ => None,
        }
    }

    pub fn debug_overlay(
        &self,
        rect: &dyn Fn(
            crate::debug_state::rects::RectId,
        ) -> Option<crate::debug_state::rects::Rect4>,
        cx: &App,
    ) -> crate::debug_state::Overlay {
        if self.tab == Tab::All {
            if let Some((wizard, _)) = &self.cleanup {
                return wizard.read(cx).debug_overlay(rect);
            }
            return self.all_sessions.read(cx).debug_overlay(rect);
        }
        let tab_name = |tab| match tab {
            Tab::NeedsYou => "needs_you",
            Tab::Review => "review",
            Tab::Running => "running",
            Tab::All => "all",
        };
        let priority = |priority| match priority {
            Some(crate::review::ReviewPriority::NeedsYou) => "needs_you",
            Some(crate::review::ReviewPriority::Failed) => "failed",
            Some(crate::review::ReviewPriority::Completed) => "completed",
            Some(crate::review::ReviewPriority::RunningWithResults) => "running_with_results",
            None => "running",
        };
        let sort = match self.sort {
            ReviewSort::Smart => "smart",
            ReviewSort::Recent => "recent",
            ReviewSort::Project => "project",
            ReviewSort::Oldest => "oldest",
        };
        let review = cx.global::<ReviewService>();
        crate::debug_state::Overlay::SessionCenter {
            generation: self.snapshot.generation,
            tab: tab_name(self.tab),
            refreshing: self.snapshot.refreshing,
            sort,
            query: self.query.clone(),
            store_initialized: review.initialized(),
            store_last_error: review.last_error().map(str::to_string),
            store_pending_count: self.list.counts.review,
            tabs: Tab::ALL
                .into_iter()
                .enumerate()
                .map(|(index, tab)| crate::debug_state::SessionCenterTab {
                    name: tab_name(tab),
                    count: self.list.counts.for_tab(tab),
                    active: tab == self.tab,
                    rect: rect(crate::debug_state::rects::RectId::SessionCenterTab(index)),
                })
                .collect(),
            rows: self
                .list
                .rows
                .iter()
                .enumerate()
                .map(|(index, row)| {
                    let dir = Self::row_dir(row, cx);
                    crate::debug_state::SessionCenterRow {
                        session_key: crate::debug_state::map::session_id(&row.key),
                        priority: priority(row.priority),
                        unreviewed_count: row.unreviewed_count,
                        selected: index == self.selection.cursor(),
                        pinned: row.pinned,
                        dir: dir.dir,
                        dir_missing: dir.missing,
                        binding: match row.runtime.as_ref().map(|runtime| runtime.confidence) {
                            Some(gilvt_agent::BindingConfidence::Exact) => "exact",
                            Some(gilvt_agent::BindingConfidence::Inferred) => "inferred",
                            Some(gilvt_agent::BindingConfidence::Unresolved) => "unresolved",
                            None => "none",
                        },
                        pid: row.runtime.as_ref().map(|runtime| runtime.pid),
                        tty: row.runtime.as_ref().and_then(|runtime| runtime.tty.as_ref()).map(|tty| tty.display().to_string()),
                        rect: rect(crate::debug_state::rects::RectId::SessionCenterRow(index)),
                    }
                })
                .collect(),
            session_review: self.debug_review(rect, cx),
        }
    }

    /// Where `row`'s session ran, as its queue row and the review header draw it: the palette's shortened
    /// directory, the git cache's branch and the history scan's 「目录已不存在」 (no disk access here).
    pub(crate) fn row_dir(row: &model::Row, cx: &App) -> model::RowDir {
        let history = cx.try_global::<History>();
        let agents = cx.try_global::<Agents>();
        model::row_dir(
            &row.cwd,
            crate::launcher::label_home(),
            |p| history.is_none_or(|h| h.dir_exists(p)),
            |p| {
                agents
                    .and_then(|a| a.git_of_dir(p))
                    .and_then(|g| g.branch.clone())
            },
        )
    }

    pub(crate) fn sync(&mut self, cx: &mut Context<Self>) {
        self.snapshot = cx.global::<History>().snapshot();
        let now = Instant::now();
        let live = cx
            .try_global::<Agents>()
            .map(|agents| model::live_sessions_with_runtime(
                agents.registry().sessions(),
                now,
                |key| agents.runtime(key).cloned(),
            ))
            .unwrap_or_default();
        let runtime = model::runtime_map(&live);
        let states = cx
            .global::<ReviewService>()
            .states(self.snapshot.reviews.iter().map(|index| index.key.clone()));
        let saved_names = cx
            .try_global::<Agents>()
            .map(|agents| {
                self.snapshot
                    .reviews
                    .iter()
                    .filter_map(|index| {
                        agents
                            .registry()
                            .saved_name(&index.key)
                            .map(|name| (index.key.clone(), name))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let system_now = SystemTime::now();
        let stamp = ProjectionStamp {
            generation: self.snapshot.generation,
            refreshing: self.snapshot.refreshing,
            language: crate::i18n::current(),
            tab: self.tab,
            query: self.query.clone(),
            sort: self.sort,
            minute: system_now
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_secs() / 60),
            live: live.clone(),
            states: states.clone(),
            saved_names,
        };
        if self.stamp.as_ref() == Some(&stamp) {
            return;
        }
        let mut inbox = build_inbox(
            &self.snapshot.entries,
            &self.snapshot.reviews,
            &states,
            &runtime,
            system_now,
            self.sort,
        );
        if let Some(agents) = cx.try_global::<Agents>() {
            let history: HashMap<SessionKey, &HistoryEntry> = self
                .snapshot
                .entries
                .iter()
                .map(|entry| ((entry.agent, entry.session_id.clone()), entry))
                .collect();
            for item in &mut inbox {
                if let Some(entry) = history.get(&item.key) {
                    item.title = crate::launcher::history::title(
                        entry,
                        agents.registry().saved_name(&item.key),
                    );
                }
            }
        }
        let before = self
            .list
            .rows
            .get(self.selection.cursor())
            .map(|row| row.key.clone());
        self.list = model::build(
            self.tab,
            &self.query,
            self.snapshot.entries.len(),
            &inbox,
            &live,
        );
        self.selection.sync(&self.list.rows, before.is_some());
        self.stamp = Some(stamp);
    }

    /// ⌘⇧K / 「清理…」: the cleanup wizard in place of the All tab's palette (nothing when already open).
    pub fn open_cleanup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_tab(Tab::All, window, cx);
        if self.cleanup.is_none() {
            let wizard = cx.new(CleanupWizard::new);
            let sub = cx.subscribe_in(&wizard, window, |view, _, event, window, cx| match event {
                SessionsEvent::CloseCleanup => view.close_cleanup(window, cx),
                _ => cx.emit(event.clone()),
            });
            self.cleanup = Some((wizard, sub));
        }
        if let Some((wizard, _)) = &self.cleanup {
            window.focus(&wizard.focus_handle(cx));
        }
        cx.notify();
    }

    /// Esc in the wizard: back to the palette.
    pub(crate) fn close_cleanup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cleanup.take().is_some() && self.tab == Tab::All {
            window.focus(&self.all_sessions.focus_handle(cx));
        }
        cx.notify();
    }

    pub fn set_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        // Another tab leaves the wizard (it belongs to the All tab).
        self.cleanup = None;
        self.tab = tab;
        self.query.clear();
        self.marked = None;
        self.surface = None;
        self.notice = None;
        self.terminate_confirm = None;
        self.selection = Selection::default();
        self.stamp = None;
        match tab {
            Tab::All => window.focus(&self.all_sessions.focus_handle(cx)),
            _ => window.focus(&self.focus_handle),
        }
        cx.notify();
    }

    pub(super) fn set_sort(&mut self, sort: ReviewSort, cx: &mut Context<Self>) {
        self.sort = sort;
        cx.notify();
    }

    pub(super) fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selection.set(&self.list.rows, index);
        // Clicking another queue row while a review is open switches the review to it.
        if self.surface.is_some() {
            self.open_review(cx);
        }
        cx.notify();
    }

    pub(crate) fn scroll_to_cursor(&mut self) {
        self.scroll
            .scroll_to_item(self.selection.cursor(), ScrollStrategy::Top);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() || self.tab == Tab::All {
            return;
        }
        let key = event.keystroke.key.as_str();
        let scope = self.surface.as_ref().map_or(Scope::List, Surface::scope);
        if scope == Scope::List {
            match key {
                "up" => {
                    self.selection.step(&self.list.rows, false);
                    self.scroll_to_cursor();
                }
                "down" => {
                    self.selection.step(&self.list.rows, true);
                    self.scroll_to_cursor();
                }
                "escape" if !self.query.is_empty() => self.query.clear(),
                "backspace" => {
                    self.query.pop();
                }
                _ => {
                    return self.run_key_action(scope, event, window, cx);
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        self.run_key_action(scope, event, window, cx);
    }

    fn run_key_action(
        &mut self,
        scope: Scope,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        let Some(action) = key_action(
            scope,
            event.keystroke.key.as_str(),
            Mods {
                command: modifiers.platform,
                alt: modifiers.alt,
                control: modifiers.control,
                shift: modifiers.shift,
            },
        ) else {
            return;
        };
        self.dispatch(action, window, cx);
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn activate(&mut self, location: Location, cx: &mut Context<Self>) {
        let Some(row) = self.selection.selected(&self.list.rows) else {
            return;
        };
        if let Some(pane) = cx
            .global::<Agents>()
            .registry()
            .get(&row.key)
            .and_then(|session| session.pane)
        {
            cx.emit(SessionsEvent::Focus(pane));
            return;
        }
        if let Some(runtime) = row.runtime.clone() {
            cx.emit(SessionsEvent::Navigate(runtime));
            return;
        }
        let Some(entry) = self
            .snapshot
            .entries
            .iter()
            .find(|entry| entry.agent == row.key.0 && entry.session_id == row.key.1)
        else {
            return;
        };
        let launch = cx.global::<AppSettings>().0.agent.launch(entry.agent);
        match sessions_model::resume(entry, None, launch, Path::exists) {
            Resume::Run { dir, command } => cx.emit(SessionsEvent::Run {
                location,
                dir,
                command,
            }),
            Resume::Gone => History::refresh(cx),
            _ => {}
        }
    }

    pub(super) fn listeners(
        &self,
        root: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        root.on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(|view, _: &SessionCenterNeedsYou, window, cx| {
                view.set_tab(Tab::NeedsYou, window, cx)
            }))
            .on_action(cx.listener(|view, _: &SessionCenterReview, window, cx| {
                view.set_tab(Tab::Review, window, cx)
            }))
            .on_action(cx.listener(|view, _: &SessionCenterRunning, window, cx| {
                view.set_tab(Tab::Running, window, cx)
            }))
            .on_action(cx.listener(|view, _: &SessionCenterAll, window, cx| {
                view.set_tab(Tab::All, window, cx)
            }))
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        // The search box is hidden behind an open review: pasting must not edit it invisibly.
        if self.tab == Tab::All || self.surface.is_some() {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.query.push_str(&search_paste_text(&text));
        cx.notify();
    }
}

impl gpui::EntityInputHandler for SessionCenterView {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let len = self
            .marked
            .as_ref()
            .map_or(0, |text| text.encode_utf16().count());
        Some(UTF16Selection {
            range: len..len,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        if self.surface.is_none() {
            self.query.push_str(&search_paste_text(text));
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!text.is_empty() && self.surface.is_none()).then(|| text.to_string());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        element: Bounds<gpui::Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<gpui::Pixels>> {
        Some(element)
    }

    fn character_index_for_point(
        &mut self,
        _: Point<gpui::Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
