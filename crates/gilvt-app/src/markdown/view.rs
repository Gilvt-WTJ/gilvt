//! The rendered view's part of `PreviewView`: loading into the list, keys, links, history,
//! the change rail and switching to the source diff (`S`).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    canvas, div, list, prelude::*, px, relative, AnyElement, Bounds, Context, KeyDownEvent, ListAlignment, ListOffset,
    ListState, MouseButton, Pixels, Point, ScrollHandle, Task, WeakEntity, Window,
};
use gilvt_mermaid::Theme;
use gilvt_viewer::{Row, SplitRow};

use crate::markdown::history::Entry;
use crate::markdown::links::{self, Target};
use crate::markdown::render::{Action, RowRenderer};
use crate::markdown::rows::{self, Mark};
use crate::markdown::style::{body_size, Colors, Style, LINE_HEIGHT};
use crate::markdown::{CodeSpans, MdDoc};
use crate::preview_view::{PreviewView, Rows, Source};
use crate::theme::AppSettings;
use crate::terminal_view::open_link;

/// How long a jumped-to block stays highlighted.
const FLASH: Duration = Duration::from_millis(1500);
/// How long "找不到 …" stays up.
const BANNER: Duration = Duration::from_secs(4);
/// Rows measured beyond the viewport, so short scrolls do not pop in.
const OVERDRAW: f32 = 600.0;
/// The source view keeps this many rows above the line it jumps to (see `PreviewElement`).
const SOURCE_CONTEXT_ROWS: usize = 3;

/// The loaded document of a Markdown preview and its view state.
pub struct MdState {
    pub doc: Rc<MdDoc>,
    spans: Rc<CodeSpans>,
    list: ListState,
    /// Deleted runs shown expanded.
    expanded: HashSet<usize>,
    flash: Option<usize>,
    scrolls: Rc<RefCell<HashMap<u32, ScrollHandle>>>,
    rail: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Body font size the list's cached row heights were measured at.
    measured_at: Option<Pixels>,
    /// The row renderer of the last frame (to build rows that were never on screen when copying).
    pub(super) renderer: Option<Rc<RowRenderer>>,
    _flash: Option<Task<()>>,
}

impl MdState {
    fn new(doc: MdDoc) -> MdState {
        MdState {
            list: ListState::new(doc.rows.len(), ListAlignment::Top, px(OVERDRAW)),
            doc: Rc::new(doc),
            spans: Rc::default(),
            expanded: HashSet::new(),
            flash: None,
            scrolls: Rc::default(),
            rail: Rc::default(),
            measured_at: None,
            renderer: None,
            _flash: None,
        }
    }

    fn top_row(&self) -> usize {
        self.list.logical_scroll_top().item_ix.min(self.doc.rows.len().saturating_sub(1))
    }

    /// Source line of the block at the top of the view.
    pub fn top_line(&self) -> u32 {
        rows::line_of_row(&self.doc.rows, self.top_row())
    }

    fn scroll_to_row(&self, row: usize) {
        self.list.scroll_to(ListOffset { item_ix: row, offset_in_item: px(0.) });
    }

    fn row_of_line(&self, line: u32) -> usize {
        rows::row_of_line(&self.doc.rows, &self.doc.blocks, line)
    }

    /// Forgets cached row heights (they depend on the font size), keeping the scroll position.
    fn remeasure(&self) {
        let top = self.list.logical_scroll_top();
        let n = self.list.item_count();
        self.list.splice(0..n, n);
        self.list.scroll_to(top);
    }

    /// First and last row on screen.
    pub(super) fn visible_rows(&self) -> (usize, usize) {
        let first = self.top_row();
        let bottom = self.list.viewport_bounds().bottom();
        let mut last = first;
        while self.list.bounds_for_item(last + 1).is_some_and(|b| b.top() < bottom) {
            last += 1;
        }
        (first, last)
    }

    /// Where the list is drawn.
    pub(super) fn viewport(&self) -> Bounds<Pixels> {
        self.list.viewport_bounds()
    }

    /// Forgets the cached height of `row` (its content changed size).
    pub fn remeasure_row(&self, row: usize) {
        self.list.splice(row..row + 1, 1);
    }

    fn jump_change(&self, forward: bool) {
        let top = self.list.logical_scroll_top();
        let stops = rows::change_stops(&self.doc.rows);
        if let Some(row) = rows::jump_target(&stops, top.item_ix, top.offset_in_item > px(0.), forward) {
            self.scroll_to_row(row);
        }
    }
}

impl PreviewView {
    /// A Markdown document is loaded and not hidden behind a load error.
    pub(crate) fn has_markdown(&self) -> bool {
        self.md.is_some() && self.error.is_none()
    }

    /// Showing a Markdown document rendered (not its source diff).
    pub(crate) fn rendered(&self) -> bool {
        self.has_markdown() && !self.show_source
    }

    /// Scrolls the shown document to source line `line` (1-based): the block holding it when rendered, else the
    /// row of the diff showing that new-text line.
    pub(crate) fn scroll_to_source_line(&mut self, line: u32, cx: &mut Context<Self>) {
        if let Some(md) = self.md.as_ref().filter(|_| !self.show_source) {
            let row = md.row_of_line(line);
            md.scroll_to_row(row);
        } else {
            self.pending_line = Some(line);
        }
        cx.notify();
    }

    /// Installs a freshly loaded document. A reload of the same file keeps the top block;
    /// a pending jump (line, anchor, history position) wins.
    pub(crate) fn set_markdown(&mut self, doc: Option<MdDoc>, cx: &mut Context<Self>) {
        let keep = self.md.as_ref().filter(|_| !self.show_source).map(MdState::top_line);
        let Some(doc) = doc else {
            self.md = None;
            self.diagrams.release(cx);
            return;
        };
        let mut md = MdState::new(doc);
        let restore = self.restore_line.take().or(keep);
        if let Some(line) = self.pending_line.filter(|_| !self.show_source) {
            self.pending_line = None;
            let row = md.row_of_line(line);
            md.scroll_to_row(row);
            md.flash = Some(row);
            md._flash = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(FLASH).await;
                let _ = this.update(cx, |v, cx| {
                    if let Some(md) = v.md.as_mut() {
                        md.flash = None;
                        cx.notify();
                    }
                });
            }));
        } else if let Some(anchor) = self.pending_anchor.take() {
            match links::find_heading(&md.doc.blocks, &anchor) {
                Some(b) if self.show_source => self.pending_line = Some(md.doc.blocks[b].lines.start),
                Some(b) => md.scroll_to_row(rows::row_of_block(&md.doc.rows, b)),
                None => self.show_banner(format!("找不到 #{anchor}"), cx),
            }
        } else if let Some(line) = restore {
            md.scroll_to_row(md.row_of_line(line));
        }
        self.md = Some(md);
    }

    /// Applies code highlighting computed for `doc` (ignored if another document replaced it).
    pub(crate) fn set_code_spans(&mut self, doc: &Rc<MdDoc>, spans: CodeSpans, cx: &mut Context<Self>) {
        if let Some(md) = self.md.as_mut().filter(|md| Rc::ptr_eq(&md.doc, doc)) {
            md.spans = Rc::new(spans);
            cx.notify();
        }
    }

    /// Source line at the top of whichever view is showing.
    fn current_line(&self) -> u32 {
        match &self.md {
            Some(md) if !self.show_source => md.top_line(),
            _ => self.source_line(),
        }
    }

    /// New-file line the source view is positioned at (the row below its context rows).
    fn source_line(&self) -> u32 {
        let (Some(diff), Some(layout)) = (self.loaded.as_ref().and_then(|l| l.preview.diff.as_ref()), self.layout) else { return 1 };
        let Some(rows) = self.rows(layout.mode) else { return 1 };
        let top = self.scroll_top as usize;
        let row = if top == 0 { 0 } else { top + SOURCE_CONTEXT_ROWS }.min(rows.len().saturating_sub(1));
        let index = match rows {
            Rows::Unified(r) => match r.get(row) {
                Some(Row::Line(i)) => *i,
                Some(Row::Fold { start, .. }) => *start,
                None => 0,
            },
            Rows::Split(r) => match r.get(row) {
                Some(SplitRow::Pair { left, right }) => right.or(*left).unwrap_or(0),
                Some(SplitRow::Fold { start, .. }) => *start,
                None => 0,
            },
        };
        rows::new_line_at(diff, index)
    }

    /// `S`: rendered ↔ source diff, keeping the top block ↔ its first source line.
    fn toggle_source(&mut self, cx: &mut Context<Self>) {
        self.clear_selection();
        if self.show_source {
            let line = self.source_line();
            self.show_source = false;
            if let Some(md) = &self.md {
                md.scroll_to_row(md.row_of_line(line));
            }
        } else if let Some(md) = &self.md {
            self.pending_line = Some(md.top_line());
            self.show_source = true;
        }
        cx.notify();
    }

    /// Keys of Markdown previews: `S` and `⌘[` in both views, the rest only when rendered.
    /// Returns false for keys the shared Quick Look handling should take (`E` / `⌘O` among them: it edits
    /// from the rendered document's top line, see `PreviewView::edit_target`).
    pub(crate) fn markdown_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let ks = &event.keystroke;
        let m = ks.modifiers;
        if ks.key == "[" && m.platform && !m.control && !m.shift && !m.alt {
            return self.go_back(cx);
        }
        if !self.has_markdown() || m.alt {
            return false;
        }
        if ks.key == "s" && !m.control && !m.platform && crate::preview_view::source_toggle_applies(self.source()) {
            self.toggle_source(cx);
            return true;
        }
        if !self.rendered() {
            return false;
        }
        let Some(md) = self.md.as_ref() else { return false };
        let line = body_size(&cx.global::<AppSettings>().0) * LINE_HEIGHT;
        let height = md.list.viewport_bounds().size.height;
        match (ks.key.as_str(), m.control, m.shift, m.platform) {
            ("j" | "down", false, false, false) => md.list.scroll_by(line),
            ("k" | "up", false, false, false) => md.list.scroll_by(-line),
            ("d", true, false, false) => md.list.scroll_by(height / 2.),
            ("u", true, false, false) => md.list.scroll_by(-height / 2.),
            ("pagedown", false, false, false) => md.list.scroll_by(height - line * 2.),
            ("pageup", false, false, false) => md.list.scroll_by(line * 2. - height),
            ("g", false, false, false) => md.scroll_to_row(0),
            ("g", false, true, false) => md.scroll_to_row(md.doc.rows.len()),
            ("n", false, false, false) => md.jump_change(true),
            ("p", false, false, false) => md.jump_change(false),
            // Unified / split only applies to the source view.
            ("u", false, _, false) => {}
            _ => return false,
        }
        cx.notify();
        true
    }

    pub(crate) fn md_action(&mut self, action: &Action, cx: &mut Context<Self>) {
        match action {
            Action::Link(dest) => self.follow_link(dest, cx),
            Action::Footnotes => {
                if let Some(md) = &self.md {
                    md.scroll_to_row(md.doc.rows.len().saturating_sub(1));
                }
            }
        }
        cx.notify();
    }

    fn follow_link(&mut self, dest: &str, cx: &mut Context<Self>) {
        let base_dir = self.file_path().and_then(Path::parent).map(Path::to_path_buf);
        let repo_root = self.loaded.as_ref().and_then(|l| l.preview.repo_root.clone());
        match links::resolve(dest, base_dir.as_deref(), repo_root.as_deref()) {
            Target::External(url) => open_link(&url),
            Target::Anchor(anchor) => match self.md.as_ref().and_then(|md| links::find_heading(&md.doc.blocks, &anchor).map(|b| (md, b))) {
                Some((md, b)) => md.scroll_to_row(rows::row_of_block(&md.doc.rows, b)),
                None => self.show_banner(format!("找不到 #{anchor}"), cx),
            },
            Target::File { .. } if !crate::preview_view::link_navigation_applies(self.source()) => {}
            Target::File { path, anchor, line } => {
                self.history.push(Entry { sources: self.sources.clone(), index: self.index, line: self.current_line() });
                self.sources = vec![Source::File(path)];
                self.index = 0;
                self.reset_view_state();
                self.pending_line = line;
                self.pending_anchor = anchor;
                self.load(cx);
            }
            Target::Missing(what) => self.show_banner(format!("找不到 {what}"), cx),
        }
    }

    /// `⌘[`: back to the document a link was followed from, at the same place.
    fn go_back(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(entry) = self.history.pop() else { return false };
        self.sources = entry.sources;
        self.index = entry.index;
        self.reset_view_state();
        if self.show_source {
            self.pending_line = Some(entry.line);
        } else {
            self.restore_line = Some(entry.line);
        }
        self.load(cx);
        cx.notify();
        true
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

    pub(crate) fn toggle_deleted(&mut self, index: usize, row: usize, cx: &mut Context<Self>) {
        self.clear_selection();
        let Some(md) = self.md.as_mut() else { return };
        if !md.expanded.remove(&index) {
            md.expanded.insert(index);
        }
        md.remeasure_row(row);
        cx.notify();
    }

    fn rail_click(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(md) = &self.md else { return };
        let Some(bounds) = md.rail.get() else { return };
        let fraction = (position.y - bounds.origin.y) / bounds.size.height;
        md.scroll_to_row(md.row_of_line(rows::line_at_fraction(fraction, md.doc.line_count)));
        cx.notify();
    }

    pub(crate) fn render_markdown(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let settings = &cx.global::<AppSettings>().0;
        let theme = crate::theme::current(cx);
        let dark = theme.dark;
        let style = Style::new(settings, &theme);
        let theme = if dark { Theme::Dark } else { Theme::Light };
        self.sync_diagrams(theme, window, cx);
        let view = cx.entity().downgrade();
        let Some(md) = self.md.as_mut() else { return div().into_any_element() };
        if md.measured_at.is_some_and(|size| size != style.body) {
            md.remeasure();
        }
        md.measured_at = Some(style.body);
        let annotation = self.annotation.as_ref().map(|(line, msg)| (md.row_of_line(*line), msg.clone()));
        let rail = md.doc.changes.is_some().then(|| rail(md, &style.colors, view.clone()));
        let renderer = Rc::new(RowRenderer {
            doc: md.doc.clone(),
            spans: md.spans.clone(),
            style,
            view,
            expanded: md.expanded.clone(),
            flash: md.flash,
            annotation,
            scrolls: md.scrolls.clone(),
            diagrams: self.diagrams.shown.clone(),
            theme,
            texts: self.md_texts.clone(),
            selection: self.md_sel.filter(|s| !s.is_empty()).map(|s| s.ordered()),
        });
        md.renderer = Some(renderer.clone());
        let rows = list(md.list.clone(), move |ix, window, cx| renderer.render(ix, window, cx)).size_full();
        div()
            .relative()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .overflow_hidden()
            .child(div().flex_1().min_w(px(0.)).h_full().child(rows))
            .children(crate::debug_state::rects::recorder(crate::debug_state::rects::RectId::PreviewCode(self.pane)))
            .children(rail)
            .into_any_element()
    }
}

/// The change map beside the document: a mark per changed block / deleted run, the visible
/// part outlined; a click jumps there.
fn rail(md: &MdState, colors: &Colors, view: WeakEntity<PreviewView>) -> impl IntoElement {
    let bounds = md.rail.clone();
    let (first, last) = md.visible_rows();
    let (top, bottom) = rows::viewport(&md.doc.rows, first, last, md.doc.line_count);
    let mut el = div()
        .relative()
        .flex_none()
        .w(px(10.))
        .h_full()
        .bg(colors.rail_bg)
        .border_l_1()
        .border_color(colors.border)
        .child(canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {}).absolute().size_full());
    for &(a, b, mark) in &md.doc.rail {
        let color = match mark {
            Mark::Added => colors.added,
            Mark::Modified => colors.modified,
            Mark::Deleted => colors.deleted_text,
        };
        el = el.child(div().absolute().left(px(2.)).right(px(2.)).top(relative(a)).h(relative(b - a)).min_h(px(2.)).rounded(px(1.)).bg(color));
    }
    el.child(
        div()
            .absolute()
            .left_0()
            .right_0()
            .top(relative(top))
            .h(relative(bottom - top))
            .min_h(px(4.))
            .bg(colors.rail_view)
            .border_t_1()
            .border_b_1()
            .border_color(colors.muted),
    )
    .on_mouse_down(MouseButton::Left, move |e, _, cx| {
        let _ = view.update(cx, |v, cx| v.rail_click(e.position, cx));
    })
}
