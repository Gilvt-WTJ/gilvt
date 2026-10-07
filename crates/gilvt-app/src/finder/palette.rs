//! The palette view: query box, results and hints. The Workspace shows it over the pane area and
//! acts on its events.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gilvt_finder::{list, search_root, Context as Ranking, Finder, Hit, Listing, Root, MAX_FILES};
use gpui::{
    canvas, div, prelude::*, px, Bounds, ClickEvent, Context, ElementInputHandler, EventEmitter, FocusHandle,
    Focusable, FontWeight, HighlightStyle, Hsla, KeyDownEvent, Pixels, Point, ScrollStrategy, StyledText,
    Task, UTF16Selection, UniformListScrollHandle, Window,
};

use super::state::{
    accept_waits, awaiting_first_hits, byte_ranges, changed, command, insert_text, keep_selected, ranks_in_background,
    rel_to_root, reselect, split_path, step, unusable_banner, Command,
};
use super::{Listings, Recent};
use crate::actions::{Paste, PALETTE_CONTEXT};
use crate::terminal_view::{search_paste_text, TerminalView};
use crate::theme::{hsla, mix};

/// Results shown at most.
const LIMIT: usize = 50;
const ROW_HEIGHT: f32 = 26.;
/// Rows visible before the list scrolls.
const VISIBLE_ROWS: usize = 12;
const BANNER: Duration = Duration::from_secs(4);

pub enum FinderEvent {
    /// ⏎ (Quick Look) or ⌘⏎ (pinned) on an existing file.
    Open { path: PathBuf, pin: bool },
    /// ⌥⏎: text for the pane's paste path.
    Insert(String),
    Close,
}

pub struct FinderView {
    focus_handle: FocusHandle,
    /// The pane's cwd: where the search root is looked up, and what inserted paths are relative to.
    cwd: PathBuf,
    recent: Recent,
    /// Known once `search_root` returns, with the ranking context relative to it.
    root: Option<Root>,
    cwd_rel: String,
    recent_rel: Vec<String>,
    listing: Option<Arc<Listing>>,
    /// `None` while a background search has it.
    finder: Option<Finder>,
    /// Bumped by every search; a background result for an older one is dropped.
    generation: u64,
    /// The search whose hits are shown.
    shown: u64,
    /// The first search of the current listing.
    listed: u64,
    /// ⏎ / ⌘⏎ / ⌥⏎ pressed while the latest search ran; done once its hits are shown.
    pending: Option<Command>,
    /// File to select once the latest search's hits arrive (see `keep_selected`).
    keep: Option<String>,
    query: String,
    /// IME composition, shown after the query.
    marked: Option<String>,
    hits: Vec<Hit>,
    selected: usize,
    scroll: UniformListScrollHandle,
    /// "找不到 …" when the selected file is gone.
    banner: Option<String>,
    _banner: Option<Task<()>>,
    _load: Task<()>,
    _search: Option<Task<()>>,
}

impl EventEmitter<FinderEvent> for FinderView {}

impl Focusable for FinderView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl FinderView {
    /// `gilvt debug state`: the query, the selected hit and how many are listed.
    pub fn debug_overlay(&self) -> crate::debug_state::Overlay {
        crate::debug_state::Overlay::Finder { query: self.query.clone(), selected: self.selected, items: self.hits.len() }
    }

    pub fn new(cwd: PathBuf, recent: Recent, cx: &mut Context<Self>) -> Self {
        let load = Self::load(cwd.clone(), cx);
        FinderView {
            focus_handle: cx.focus_handle(),
            cwd,
            recent,
            root: None,
            cwd_rel: String::new(),
            recent_rel: Vec::new(),
            listing: None,
            finder: Some(Finder::new()),
            generation: 0,
            shown: 0,
            listed: 0,
            pending: None,
            keep: None,
            query: String::new(),
            marked: None,
            hits: Vec::new(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            banner: None,
            _banner: None,
            _load: load,
            _search: None,
        }
    }

    /// Finds the search root, shows its cached listing right away, then lists it again on the
    /// background executor (or waits for the listing of that root already running) and shows the
    /// result when it differs.
    fn load(cwd: PathBuf, cx: &mut Context<Self>) -> Task<()> {
        let background = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let root = background.spawn(async move { search_root(&cwd) }).await;
            let Ok(cached) = cx.update_global::<Listings, _>(|l, _| l.cache.get(&root.dir)) else { return };
            if this.update(cx, |v, cx| v.set_root(root.clone(), cached.clone(), cx)).is_err() {
                return;
            }
            let Ok(start) = cx.update_global::<Listings, _>(|l, _| l.in_flight.join(&root.dir, this.clone())) else { return };
            if start {
                Self::refresh(root, cached, cx);
            }
        })
    }

    /// Lists `root` in a task no palette owns, so the work is cached even when every palette
    /// waiting for it has closed, then hands a changed listing to the ones still open.
    fn refresh(root: Root, cached: Option<Arc<Listing>>, cx: &mut gpui::AsyncApp) {
        let job = cx.background_executor().spawn(async move {
            let fresh = changed(cached.as_deref(), list(&root)).map(Arc::new);
            (root.dir, fresh)
        });
        cx.spawn(async move |cx| {
            let (dir, fresh) = job.await;
            let _ = cx.update(|cx| {
                let waiters = cx.update_global::<Listings, _>(|l, _| {
                    if let Some(fresh) = &fresh {
                        l.cache.insert(dir.clone(), fresh.clone());
                    }
                    l.in_flight.finish(&dir)
                });
                if let Some(fresh) = fresh {
                    for view in waiters {
                        let _ = view.update(cx, |v, cx| v.set_listing(fresh.clone(), cx));
                    }
                }
            });
        })
        .detach();
    }

    fn set_root(&mut self, root: Root, cached: Option<Arc<Listing>>, cx: &mut Context<Self>) {
        self.cwd_rel = rel_to_root(&self.cwd, &root.dir).unwrap_or_default();
        self.recent_rel = self.recent.under(&root.dir);
        self.root = Some(root);
        if let Some(listing) = cached {
            self.set_listing(listing, cx);
        }
    }

    /// Re-ranks against a new listing, keeping the selected file selected.
    fn set_listing(&mut self, listing: Arc<Listing>, cx: &mut Context<Self>) {
        let selected = self.hits.get(self.selected).map(|h| h.path.clone());
        self.listing = Some(listing);
        self.search(selected, cx);
        self.listed = self.generation;
    }

    /// Runs on every keystroke. Small listings are ranked right here; large ones on the background
    /// executor (ranking is linear in the file count), one search at a time: the hits shown stay
    /// until the latest query's arrive, and keys typed meanwhile are ranked together once the
    /// running search returns.
    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        // A waiting ⏎ was for the query as it was.
        self.pending = None;
        self.search(None, cx);
    }

    /// Ranks the listing for the query, then selects `keep` (else the first hit).
    fn search(&mut self, keep: Option<String>, cx: &mut Context<Self>) {
        cx.notify();
        let Some(listing) = self.listing.clone() else { return };
        self.keep = keep_selected(self.finder.is_none(), self.keep.take(), keep);
        self.generation += 1;
        // Without the finder a search is running; it starts the next one when it returns.
        let Some(mut finder) = self.finder.take() else { return };
        if !ranks_in_background(listing.files.len()) {
            let ranking = Ranking { cwd_rel: &self.cwd_rel, recent: &self.recent_rel };
            let hits = finder.search(&listing, &self.query, &ranking, LIMIT);
            self.finder = Some(finder);
            self.show(hits, self.generation, cx);
            return;
        }
        let generation = self.generation;
        let (query, cwd_rel, recent) = (self.query.clone(), self.cwd_rel.clone(), self.recent_rel.clone());
        let job = cx.background_executor().spawn(async move {
            let hits = finder.search(&listing, &query, &Ranking { cwd_rel: &cwd_rel, recent: &recent }, LIMIT);
            (finder, hits)
        });
        self._search = Some(cx.spawn(async move |this, cx| {
            let (finder, hits) = job.await;
            let _ = this.update(cx, |v, cx| {
                v.finder = Some(finder);
                if v.generation == generation {
                    v.show(hits, generation, cx);
                    cx.notify();
                } else {
                    let keep = v.keep.take();
                    v.search(keep, cx);
                }
            });
        }));
    }

    /// Shows the hits of search `generation`, then does a ⏎ / ⌘⏎ / ⌥⏎ that waited for them.
    fn show(&mut self, hits: Vec<Hit>, generation: u64, cx: &mut Context<Self>) {
        self.hits = hits;
        self.shown = generation;
        self.selected = reselect(self.keep.take().as_deref(), &self.hits);
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Top);
        if let Some(cmd) = self.pending.take() {
            self.accept(cmd, cx);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return; // IME composition owns the keyboard.
        }
        let Some(cmd) = command(&event.keystroke.key, event.keystroke.modifiers) else { return };
        match cmd {
            Command::Up | Command::Down => {
                self.selected = step(self.selected, self.hits.len(), cmd == Command::Down);
                self.scroll.scroll_to_item(self.selected, ScrollStrategy::Top);
                cx.notify();
            }
            Command::Backspace => {
                let mut query = self.query.clone();
                if query.pop().is_some() {
                    self.set_query(query, cx);
                }
            }
            Command::Close => cx.emit(FinderEvent::Close),
            Command::Open | Command::Pin | Command::Insert => self.accept(cmd, cx),
        }
        cx.stop_propagation();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return };
        let query = self.query.clone() + &search_paste_text(&text);
        self.set_query(query, cx);
    }

    /// Acts on the selected file; while the latest search runs, once its hits are shown (the ones
    /// shown now may be for an older query).
    fn accept(&mut self, cmd: Command, cx: &mut Context<Self>) {
        if accept_waits(self.shown, self.generation) {
            self.pending = Some(cmd);
            return;
        }
        self.act(cmd, cx);
    }

    /// Acts on the selected hit as shown, unless it is gone or not a file.
    fn act(&mut self, cmd: Command, cx: &mut Context<Self>) {
        let (Some(root), Some(hit)) = (&self.root, self.hits.get(self.selected)) else { return };
        let path = root.dir.join(&hit.path);
        if let Some(banner) = unusable_banner(&hit.path, &path) {
            self.show_banner(banner, cx);
            return;
        }
        cx.emit(match cmd {
            Command::Insert => FinderEvent::Insert(insert_text(&path, &self.cwd)),
            _ => FinderEvent::Open { path, pin: cmd == Command::Pin },
        });
    }

    fn show_banner(&mut self, text: String, cx: &mut Context<Self>) {
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

    fn render_rows(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let p = TerminalView::palette(window, cx);
        let modified = crate::theme::hsla(crate::theme::current(cx).ui.attention.fg);
        let muted = hsla(mix(p.foreground, p.background, 0.45));
        let selected_bg = hsla(mix(p.background, p.foreground, 0.12));
        let matched = HighlightStyle { color: Some(hsla(p.ansi[4])), font_weight: Some(FontWeight::BOLD), ..Default::default() };
        let highlighted = |text: &str, indices: &[usize]| {
            let ranges = byte_ranges(text, indices);
            StyledText::new(text.to_string()).with_highlights(ranges.into_iter().map(|r| (r, matched)))
        };
        range
            .filter_map(|ix| self.hits.get(ix).map(|hit| (ix, hit)))
            .map(|(ix, hit)| {
                let (dir, name) = split_path(&hit.path);
                // The trailing '/' is not shown; a match on it is dropped with it.
                let dir_shown = dir.strip_suffix('/').unwrap_or(dir);
                let dot = div().flex_none().w(px(14.)).text_color(modified).child(if hit.changed { "●" } else { "" });
                div()
                    .id(ix)
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .when(ix == self.selected, |row| row.bg(selected_bg))
                    // A click names the row's file: no waiting for a running search.
                    .on_click(cx.listener(move |v, _: &ClickEvent, _, cx| {
                        v.selected = ix;
                        v.act(Command::Open, cx);
                    }))
                    .child(div().flex().flex_none().items_center().child(dot).child(highlighted(name, &hit.name_matches)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .flex()
                            .justify_end()
                            .text_color(muted)
                            .child(highlighted(dir_shown, &hit.dir_matches)),
                    )
                    .into_any_element()
            })
            .collect()
    }

    fn query_box(&self, muted: Hsla, accent: Hsla, cx: &Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let focus = self.focus_handle.clone();
        let caret = div().flex_none().w(px(1.5)).h(px(16.)).bg(accent);
        let row = div()
            .relative()
            .flex_none()
            .h(px(38.))
            .px_3()
            .flex()
            .items_center()
            .text_size(px(14.))
            // IME: typed text reaches `replace_text_in_range` through this input handler.
            .child(
                canvas(|_, _, _| {}, move |bounds, _, window, cx| {
                    window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                })
                .absolute()
                .size_full(),
            );
        if self.query.is_empty() && self.marked.is_none() {
            let name = self.root.as_ref().and_then(|r| r.dir.file_name()).map(|n| n.to_string_lossy().into_owned());
            let placeholder = match name {
                Some(name) => format!("在 {name} 中搜索文件"),
                None => "搜索文件".to_string(),
            };
            row.child(caret.mr(px(4.))).child(div().text_color(muted).child(placeholder))
        } else {
            row.child(self.query.clone()).children(self.marked.clone().map(|m| div().underline().child(m))).child(caret)
        }
    }

    /// Shown instead of the list: loading (the listing, or its first ranking), not listed, nothing
    /// listed, nothing matched.
    fn message(&self) -> Option<&'static str> {
        match &self.listing {
            None => Some("正在列出文件…"),
            Some(_) if self.hits.is_empty() && awaiting_first_hits(self.shown, self.listed) => Some("正在列出文件…"),
            Some(l) if l.skipped => Some("主目录和根目录不搜索，请先 cd 到项目目录"),
            Some(l) if l.files.is_empty() => Some("没有文件"),
            Some(_) if self.hits.is_empty() => Some("无匹配"),
            Some(_) => None,
        }
    }
}

impl gpui::EntityInputHandler for FinderView {
    fn text_for_range(&mut self, _: Range<usize>, _: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        None
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let len = self.marked.as_ref().map_or(0, |t| t.encode_utf16().count());
        Some(UTF16Selection { range: len..len, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|t| 0..t.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        let typed = search_paste_text(text);
        if typed.is_empty() {
            cx.notify();
        } else {
            let query = self.query.clone() + &typed;
            self.set_query(query, cx);
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    fn bounds_for_range(&mut self, _: Range<usize>, element: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        // The candidate window opens below the query box.
        Some(element)
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl Render for FinderView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = TerminalView::palette(window, cx);
        let muted = hsla(mix(p.foreground, p.background, 0.45));
        let rule = hsla(mix(p.background, p.foreground, 0.15));
        let body = match self.message() {
            Some(text) => div().h(px(ROW_HEIGHT * 2.)).flex().items_center().justify_center().text_color(muted).child(text).into_any_element(),
            None => gpui::uniform_list("finder-hits", self.hits.len(), cx.processor(Self::render_rows))
                .track_scroll(self.scroll.clone())
                .h(px(ROW_HEIGHT * self.hits.len().min(VISIBLE_ROWS) as f32))
                .into_any_element(),
        };
        let warn = crate::theme::hsla(crate::theme::current(cx).ui.attention.fg);
        let truncated = self.listing.as_ref().is_some_and(|l| l.truncated).then(|| {
            div().text_color(warn).child(format!("文件过多，只搜索了前 {MAX_FILES} 个"))
        });
        let banner = self.banner.clone().map(|text| {
            div().flex_none().px_3().py_1().text_size(px(12.)).bg(hsla(mix(p.background, p.ansi[1], 0.25))).child(text)
        });
        div()
            .id("finder")
            .key_context(PALETTE_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::paste))
            .flex()
            .flex_col()
            .bg(hsla(p.background))
            .text_color(hsla(p.foreground))
            .text_size(px(13.))
            .children(banner)
            .child(self.query_box(muted, hsla(p.ansi[4]), cx))
            .child(div().h(px(1.)).bg(rule))
            .child(div().py_1().child(body))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .justify_between()
                    .gap_4()
                    .px_3()
                    .py_1()
                    .text_size(px(12.))
                    .text_color(muted)
                    .bg(hsla(mix(p.background, p.foreground, 0.06)))
                    .child(div().children(truncated))
                    .child(div().whitespace_nowrap().child(crate::i18n::text(
                        "⏎ 预览 · ⌘⏎ 固定 · ⌥⏎ 插入路径 · Esc 关闭",
                        "⏎ preview · ⌘⏎ pin · ⌥⏎ insert path · Esc close",
                    ))),
            )
    }
}
