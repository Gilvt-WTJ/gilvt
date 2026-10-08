//! gpui elements for the rows of the rendered view. Built per visible row each frame; gpui's
//! `list` measures them and caches heights of rows scrolled away.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    canvas, div, img, prelude::*, px, relative, AnyElement, App, Div, Hsla, InteractiveText, ObjectFit, Pixels, ScrollHandle,
    StrikethroughStyle, StyledText, TextRun, UnderlineStyle, WeakEntity, Window,
};
use gilvt_markdown::{Align, Block, BlockKind, Footnote, Inline, ListItem, Table};
use gilvt_mermaid::{cache_key, Theme};
use gilvt_viewer::Span;

use crate::markdown::images::Image;
use crate::markdown::layout;
use crate::markdown::mermaid::Shown;
use crate::markdown::rows::{Mark, Row};
use crate::markdown::select::{highlight_runs, range_in, MdPos, TextEntry, Texts};
use crate::markdown::style::{Style, COLUMN_WIDTH, LINE_HEIGHT};
use crate::markdown::{CodeSpans, MdDoc};
use crate::preview_view::PreviewView;

/// What a click on inline text asks for.
#[derive(Clone, Debug)]
pub enum Action {
    Link(String),
    Footnotes,
}

/// Side padding around the reading column; the change bar sits in it.
const SIDE_PADDING: f32 = 40.0;
/// Height of the box standing in for a diagram being rendered.
const DIAGRAM_PLACEHOLDER_HEIGHT: f32 = 160.0;
/// Table cells wider than this wrap.
const MAX_CELL_WIDTH: f32 = 420.0;
const BULLETS: [&str; 3] = ["•", "◦", "▪"];
/// Put before links to files in the repository (they open in Quick Look).
const FILE_LINK_MARK: &str = "↗ ";

/// Everything a row needs, shared by the list's render callback.
pub struct RowRenderer {
    pub doc: Rc<MdDoc>,
    pub spans: Rc<CodeSpans>,
    pub style: Style,
    pub view: WeakEntity<PreviewView>,
    /// Deleted runs shown expanded (`Changes::deleted` indices).
    pub expanded: HashSet<usize>,
    /// Row briefly highlighted after a jump to a line.
    pub flash: Option<usize>,
    /// Row carrying the Cmd+clicked message, and the message.
    pub annotation: Option<(usize, String)>,
    /// Horizontal scroll of code blocks and tables, by first source line.
    pub scrolls: Rc<RefCell<HashMap<u32, ScrollHandle>>>,
    /// Mermaid diagrams by cache key, and the theme the keys are for.
    pub diagrams: Rc<HashMap<String, Shown>>,
    pub theme: Theme,
    /// The texts of the rows built (for selecting), and the selection (ordered) to paint.
    pub texts: Texts,
    pub selection: Option<(MdPos, MdPos)>,
}

#[derive(Clone, Copy)]
struct Tone {
    color: Hsla,
    size: Pixels,
}

impl RowRenderer {
    pub fn render(&self, ix: usize, window: &mut Window, _: &mut App) -> AnyElement {
        let doc = &self.doc;
        let Some(info) = doc.rows.get(ix) else { return div().into_any_element() };
        let s = &self.style;
        let c = &s.colors;
        let margins = |r: Row| layout::margins(match r {
            Row::Block(b) => Some(&doc.blocks[b].kind),
            Row::Deleted(_) => None,
        });
        let top = match ix {
            0 => s.scaled(26.0),
            _ => s.scaled(layout::gap(Some(margins(doc.rows[ix - 1].row)), margins(info.row))),
        };
        let bottom = if ix + 1 == doc.rows.len() { s.scaled(60.0) } else { px(0.) };

        {
            let mut texts = self.texts.borrow_mut();
            let stale: Vec<_> = texts.range((ix, 0)..(ix + 1, 0)).map(|(k, _)| *k).collect();
            for key in stale {
                texts.remove(&key);
            }
        }
        let mut b = Builder { r: self, window, next_id: 0, row: ix, seq: Cell::new(0) };
        let content = match info.row {
            Row::Block(i) => b.block(&doc.blocks[i], Tone { color: c.text, size: s.body }, 0),
            Row::Deleted(d) => b.deleted(ix, d),
        };
        let mut column = div().relative().flex().flex_col().max_w(px(COLUMN_WIDTH)).mx_auto();
        let bar = match info.mark {
            Some(Mark::Added) => Some(c.added),
            Some(Mark::Modified) => Some(c.modified),
            _ => None,
        };
        if let Some(color) = bar {
            column = column.child(div().absolute().left(px(-22.)).top(px(3.)).bottom(px(3.)).w(px(3.)).rounded(px(2.)).bg(color));
        }
        if let Some((_, message)) = self.annotation.as_ref().filter(|(row, _)| *row == ix) {
            column = column.child(
                div()
                    .mb(s.scaled(6.0))
                    .px(s.scaled(8.0))
                    .rounded_md()
                    .bg(c.warn_bg)
                    .text_color(c.warn)
                    .text_size(s.scaled(12.0))
                    .child(format!("⚠ {message}")),
            );
        }
        div()
            .id(("md-row", ix))
            .w_full()
            .px(px(SIDE_PADDING))
            .pt(top)
            .pb(bottom)
            .font_family(".SystemUIFont")
            .text_size(s.body)
            .line_height(relative(LINE_HEIGHT))
            .text_color(c.text)
            .child(column.child(content).when(self.flash == Some(ix), |col| {
                // Over the content, so backgrounds of tables and code blocks do not hide it.
                col.child(div().absolute().inset_0().rounded_md().bg(c.flash))
            }))
            .into_any_element()
    }
}

/// Builds the elements of one row.
struct Builder<'a> {
    r: &'a RowRenderer,
    window: &'a mut Window,
    /// Element ids of clickable texts, unique within the row.
    next_id: usize,
    /// The row, and how many texts of it have been made (the next one's number).
    row: usize,
    seq: Cell<usize>,
}

impl Builder<'_> {
    fn s(&self) -> &Style {
        &self.r.style
    }

    fn block(&mut self, block: &Block, tone: Tone, depth: usize) -> AnyElement {
        let s = self.s();
        let c = &s.colors;
        match &block.kind {
            BlockKind::Heading { level, content, .. } => self.heading(*level, content),
            BlockKind::Paragraph(content) => self.text(content, tone, false, false),
            BlockKind::List { ordered, start, items } => self.list(*ordered, *start, items, tone, depth),
            BlockKind::Quote(inner) => {
                let (bar, bg, pad_x, pad_y) = (c.quote_bar, c.quote_bg, s.scaled(14.0), s.scaled(4.0));
                let inner = self.blocks(inner, Tone { color: self.s().colors.quote_text, ..tone }, depth, 1.0);
                div()
                    .flex()
                    .rounded_r(px(6.))
                    .bg(bg)
                    .child(div().flex_none().w(px(3.)).bg(bar))
                    .child(div().flex_1().min_w(px(0.)).px(pad_x).py(pad_y).child(inner))
                    .into_any_element()
            }
            BlockKind::Code { lang, text } => {
                let body = self.code_text(text, self.r.spans.get(&block.lines.start));
                self.code_frame(block.lines.start, body, lang.clone())
            }
            BlockKind::Mermaid(source) => self.mermaid(source, block.lines.start),
            BlockKind::Table(table) => self.table(table, block.lines.start, tone),
            BlockKind::Rule => div().h(px(1.)).bg(c.border).into_any_element(),
            BlockKind::Image { src, alt } => self.image(src, alt),
            BlockKind::FrontMatter(pairs) => div()
                .flex()
                .flex_wrap()
                .gap(s.scaled(6.0))
                .text_size(s.scaled(12.0))
                .text_color(c.muted)
                .children(pairs.iter().map(|(k, v)| div().px(s.scaled(8.0)).rounded_full().bg(c.tag_bg).child(format!("{k}: {v}"))))
                .into_any_element(),
            BlockKind::Html(source) => {
                let text = self.mono_text(source, c.muted);
                div().text_size(s.code).child(text).into_any_element()
            }
            BlockKind::Footnotes(notes) => self.footnotes(notes),
        }
    }

    /// Nested blocks stacked with collapsed margins scaled by `spacing`.
    fn blocks(&mut self, blocks: &[Block], tone: Tone, depth: usize, spacing: f32) -> Div {
        let mut col = div().flex().flex_col();
        let mut prev = None;
        for b in blocks {
            let m = layout::margins(Some(&b.kind));
            let gap = self.s().scaled(layout::gap(prev, m) * spacing);
            col = col.child(div().pt(gap).child(self.block(b, tone, depth)));
            prev = Some(m);
        }
        col
    }

    fn heading(&mut self, level: u8, content: &[Inline]) -> AnyElement {
        let s = self.s();
        let size = s.scaled(match level {
            1 => 26.0,
            2 => 19.0,
            3 => 15.0,
            4 => 14.0,
            5 => 13.0,
            _ => 12.5,
        });
        let color = if level == 6 { s.colors.muted } else { s.colors.heading };
        let (pad, border) = (s.scaled(6.0), s.colors.border);
        let el = div().line_height(relative(1.3)).child(self.text(content, Tone { color, size }, true, false));
        if level == 2 { el.pb(pad).border_b_1().border_color(border) } else { el }.into_any_element()
    }

    fn list(&mut self, ordered: bool, start: u64, items: &[ListItem], tone: Tone, depth: usize) -> AnyElement {
        let s = self.s();
        let line = tone.size * LINE_HEIGHT;
        let digits = (start + items.len() as u64).to_string().len() as f32;
        let has_tasks = items.iter().any(|i| i.task.is_some());
        let marker_width = s.scaled(if ordered { 8.0 + 8.0 * digits } else if has_tasks { 22.0 } else { 18.0 });
        let item_gap = s.scaled(2.0);
        let mut col = div().flex().flex_col().gap(item_gap);
        for (i, item) in items.iter().enumerate() {
            let marker = match item.task {
                Some(checked) => self.checkbox(checked, line),
                None => {
                    let text = if ordered { format!("{}.", start + i as u64) } else { BULLETS[depth % BULLETS.len()].to_string() };
                    div().h(line).text_color(tone.color).child(text).into_any_element()
                }
            };
            let content = if item.blocks.is_empty() { div().h(line) } else { self.blocks(&item.blocks, tone, depth + 1, 0.5) };
            col = col.child(
                div().flex().child(div().flex_none().w(marker_width).child(marker)).child(div().flex_1().min_w(px(0.)).child(content)),
            );
        }
        col.into_any_element()
    }

    fn checkbox(&self, checked: bool, line: Pixels) -> AnyElement {
        let s = self.s();
        let c = &s.colors;
        let mut b = div().size(s.scaled(13.0)).flex().items_center().justify_center().rounded(px(3.)).border_1();
        b = if checked {
            b.bg(c.accent).border_color(c.accent).text_color(gpui::white()).text_size(s.scaled(10.0)).line_height(relative(1.)).child("✓")
        } else {
            b.border_color(c.check_border)
        };
        div().h(line).flex().items_center().child(b).into_any_element()
    }

    /// Styled runs for `inlines`; also the clickable ranges and what each does.
    fn runs(&self, inlines: &[Inline], tone: Tone, bold: bool, mono: bool) -> (String, Vec<TextRun>, Vec<Range<usize>>, Vec<Action>) {
        let s = self.s();
        let c = &s.colors;
        let (mut text, mut runs, mut ranges, mut actions) = (String::new(), Vec::new(), Vec::new(), Vec::new());
        let run = |len, font, color| TextRun { len, font, color, background_color: None, underline: None, strikethrough: None };
        for inl in inlines {
            let st = &inl.style;
            let font = if mono || st.code { s.mono_font(bold || st.bold, st.italic) } else { s.text_font(bold || st.bold, st.italic) };
            let start = text.len();
            if let Some(n) = st.footnote {
                let label = format!("[{n}]");
                runs.push(run(label.len(), font, c.link));
                text.push_str(&label);
                ranges.push(start..text.len());
                actions.push(Action::Footnotes);
                continue;
            }
            if st.link.as_ref().is_some_and(|d| self.r.doc.file_links.contains(d)) {
                text.push_str(FILE_LINK_MARK);
                runs.push(run(FILE_LINK_MARK.len(), s.text_font(false, false), c.link));
            }
            let shown = if st.code { format!("\u{2009}{}\u{2009}", inl.text) } else { inl.text.clone() };
            let mut r = run(shown.len(), font, if st.link.is_some() { c.link } else if st.code { c.code_text } else { tone.color });
            if st.code {
                r.background_color = Some(c.code_bg);
            }
            if st.link.is_some() {
                r.underline = Some(UnderlineStyle { thickness: px(1.), color: Some(c.link_line), wavy: false });
            }
            if st.strike {
                r.strikethrough = Some(StrikethroughStyle { thickness: px(1.), color: None });
            }
            runs.push(r);
            text.push_str(&shown);
            if let Some(dest) = &st.link {
                ranges.push(start..text.len());
                actions.push(Action::Link(dest.clone()));
            }
        }
        (text, runs, ranges, actions)
    }

    fn text(&mut self, inlines: &[Inline], tone: Tone, bold: bool, mono: bool) -> AnyElement {
        let (text, runs, ranges, actions) = self.runs(inlines, tone, bold, mono);
        let wrapper = div().text_size(tone.size);
        if text.is_empty() {
            return wrapper.h(tone.size * LINE_HEIGHT).into_any_element();
        }
        let (styled, recorder) = self.selectable(text, runs);
        if ranges.is_empty() {
            return wrapper.child(styled).child(recorder).into_any_element();
        }
        self.next_id += 1;
        let view = self.r.view.clone();
        let text = InteractiveText::new(("md-text", self.next_id), styled).on_click(ranges, move |i, _, cx| {
            if let Some(action) = actions.get(i) {
                let _ = view.update(cx, |v, cx| v.md_action(action, cx));
            }
        });
        wrapper.child(text).child(recorder).into_any_element()
    }

    /// A text that can be selected: registers it (so the mouse can hit it) and paints its part of the
    /// selection. The recorder goes right after it as a sibling and notes where it was laid out.
    fn selectable(&self, text: String, mut runs: Vec<TextRun>) -> (StyledText, impl IntoElement) {
        let key = (self.row, self.seq.get());
        self.seq.set(key.1 + 1);
        if let Some(range) = self.r.selection.and_then(|sel| range_in(sel, key, text.len())) {
            runs = highlight_runs(runs, range, self.r.style.colors.selection);
        }
        let styled = StyledText::new(text.clone()).with_runs(runs);
        let layout = styled.layout().clone();
        let bounds = Rc::new(Cell::new(None));
        self.r.texts.borrow_mut().insert(key, TextEntry { text, layout: layout.clone(), bounds: bounds.clone() });
        let recorder = canvas(move |_, _, _| bounds.set(Some(layout.bounds())), |_, _, _, _| {}).absolute().w(px(0.)).h(px(0.));
        (styled, recorder)
    }

    /// Monospace text in one color (raw HTML, deleted source).
    fn mono_text(&self, text: &str, color: Hsla) -> Div {
        let text = if text.is_empty() { " ".to_string() } else { text.to_string() };
        let run = TextRun { len: text.len(), font: self.s().mono_font(false, false), color, background_color: None, underline: None, strikethrough: None };
        let (styled, recorder) = self.selectable(text, vec![run]);
        div().child(styled).child(recorder)
    }

    /// Code with its highlight spans (one list per line), or plain until they arrive.
    fn code_text(&self, text: &str, spans: Option<&Vec<Vec<Span>>>) -> Div {
        let s = self.s();
        let plain = |len| TextRun { len, font: s.mono_font(false, false), color: s.colors.code_text, background_color: None, underline: None, strikethrough: None };
        let mut out = String::with_capacity(text.len());
        let mut runs = Vec::new();
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
                runs.push(plain(1));
            }
            let mut pos = 0;
            for span in spans.and_then(|s| s.get(i)).into_iter().flatten().filter(|sp| sp.range.end <= line.len()) {
                if span.range.start > pos {
                    runs.push(plain(span.range.start - pos));
                }
                let color = gpui::rgb(u32::from_be_bytes([0, span.fg.r, span.fg.g, span.fg.b])).into();
                runs.push(TextRun { font: s.mono_font(span.bold, span.italic), color, ..plain(span.range.len()) });
                pos = span.range.end;
            }
            if pos < line.len() {
                runs.push(plain(line.len() - pos));
            }
            out.push_str(line);
        }
        if out.is_empty() {
            out.push(' ');
            runs.push(plain(1));
        }
        let (styled, recorder) = self.selectable(out, runs);
        div().child(styled).child(recorder)
    }

    /// A horizontally scrolling box keyed by the block's first source line.
    fn scroller(&self, key: u32) -> gpui::Stateful<Div> {
        let handle = self.r.scrolls.borrow_mut().entry(key).or_default().clone();
        let mut el = div().id(("md-scroll", key as usize)).flex().overflow_x_scroll().track_scroll(&handle);
        // Vertical wheel movement scrolls the document, not the box.
        el.style().restrict_scroll_to_axis = Some(true);
        el
    }

    /// The frame of a code block: background, border, language label, sideways scrolling.
    fn code_frame(&mut self, key: u32, body: Div, lang: Option<String>) -> AnyElement {
        let s = self.s();
        let c = &s.colors;
        let (pad_x, pad_y, label_size) = (s.scaled(14.0), s.scaled(12.0), s.scaled(10.0));
        let (bg, border, muted, size) = (c.pre_bg, c.border, c.muted, s.code);
        let scroller = self.scroller(key).px(pad_x).py(pad_y).child(div().flex_none().whitespace_nowrap().child(body));
        div()
            .relative()
            .rounded(px(8.))
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_size(size)
            .line_height(relative(LINE_HEIGHT))
            .child(scroller)
            .children(lang.map(|l| {
                div().absolute().top(px(4.)).right(px(10.)).font_family(".SystemUIFont").text_size(label_size).text_color(muted).child(l)
            }))
            .into_any_element()
    }

    /// The diagram once rendered, a skeleton box until then; the source with the reason when it
    /// cannot be drawn.
    fn mermaid(&mut self, source: &str, line: u32) -> AnyElement {
        let s = self.s();
        let c = &s.colors;
        let frame = div()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .p(s.scaled(14.0))
            .rounded(px(8.))
            .border_1()
            .border_color(c.border)
            .bg(c.pre_bg);
        // Added after the content, so it is drawn over it.
        let label = div().absolute().top(px(4.)).right(px(10.)).text_size(s.scaled(10.0)).text_color(c.muted).child("mermaid");
        let (red, size) = (c.deleted_text, s.scaled(12.0));
        let reason = match self.r.diagrams.get(&cache_key(source, self.r.theme)) {
            Some(Shown::Ready { image, width, height, .. }) => {
                let mut fit = div().w_full().max_w(px(*width));
                fit.style().aspect_ratio = Some(width / height);
                return frame.child(fit.child(img(image.clone()).size_full())).child(label).into_any_element();
            }
            None | Some(Shown::Rendering) => {
                let skeleton = div()
                    .w_full()
                    .h(px(DIAGRAM_PLACEHOLDER_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .bg(c.tag_bg)
                    .text_size(s.scaled(12.0))
                    .text_color(c.muted)
                    .child(crate::i18n::text("正在渲染图表…", "Rendering diagram…"));
                return frame.child(skeleton).child(label).into_any_element();
            }
            // Monospace: mermaid points at the error with a caret under the source line.
            Some(Shown::Error(message)) => div()
                .text_size(s.code)
                .child(self.mono_text(&format!("⚠ {message}"), red)),
            Some(Shown::RemoteImage) => {
                div()
                    .text_size(size)
                    .text_color(red)
                    .child(crate::i18n::text(
                        "⚠ 图中含远程图片（未加载）",
                        "⚠ Diagram contains remote images (not loaded)",
                    ))
            }
            Some(Shown::Failed) => div()
                .text_size(size)
                .text_color(red)
                .child(crate::i18n::text(
                    "⚠ 图表渲染失败",
                    "⚠ Diagram rendering failed",
                )),
        };
        let gap = self.s().scaled(6.0);
        let body = self.code_text(source, None);
        let code = self.code_frame(line, body, Some("mermaid".into()));
        div().flex().flex_col().gap(gap).child(code).child(reason).into_any_element()
    }

    fn table(&mut self, t: &Table, key: u32, tone: Tone) -> AnyElement {
        let s = self.s();
        let c = &s.colors;
        let (pad_x, pad_y) = (s.scaled(10.0), s.scaled(6.0));
        let (border, th_bg, zebra) = (c.border, c.th_bg, c.zebra);
        let tone = Tone { size: s.scaled(13.0), ..tone };
        let rows: Vec<&Vec<Vec<Inline>>> = std::iter::once(&t.header).chain(&t.rows).collect();
        let mut widths = vec![px(0.); t.header.len()];
        for (r, row) in rows.iter().enumerate() {
            for (col, cell) in row.iter().enumerate() {
                let w = self.measure(cell, tone, r == 0, r > 0 && t.numeric[col]);
                // +1: shaped widths are fractional, the laid-out text is rounded up.
                widths[col] = widths[col].max(w.ceil().min(px(MAX_CELL_WIDTH)) + pad_x * 2. + px(2.));
            }
        }
        let total: Pixels = widths.iter().copied().fold(px(0.), |a, b| a + b);
        let mut body = div().flex().flex_col().flex_none().min_w_full().w(total + px(1.)).border_t_1().border_l_1().border_color(border);
        for (r, row) in rows.iter().enumerate() {
            let mut line = div().flex();
            if r == 0 {
                line = line.bg(th_bg);
            } else if r % 2 == 1 {
                line = line.bg(zebra);
            }
            for (col, cell) in row.iter().enumerate() {
                let numeric = t.numeric[col];
                let text = self.text(cell, tone, r == 0, r > 0 && numeric);
                let cell = div().flex().flex_basis(widths[col]).flex_grow().flex_shrink_0().min_w(px(0.)).px(pad_x).py(pad_y).border_r_1().border_b_1().border_color(border);
                let cell = match (t.align[col], numeric) {
                    (Align::Right, _) | (Align::None, true) => cell.justify_end(),
                    (Align::Center, _) => cell.justify_center(),
                    _ => cell,
                };
                line = line.child(cell.child(text));
            }
            body = body.child(line);
        }
        self.scroller(key).child(body).into_any_element()
    }

    /// Unwrapped width of a table cell.
    fn measure(&mut self, inlines: &[Inline], tone: Tone, bold: bool, mono: bool) -> Pixels {
        let (text, runs, _, _) = self.runs(inlines, tone, bold, mono);
        if text.is_empty() {
            return px(0.);
        }
        let text = text.replace('\n', " ");
        self.window.text_system().shape_line(text.into(), tone.size, &runs, None).width
    }

    fn image(&mut self, src: &str, alt: &str) -> AnyElement {
        let s = self.s();
        let c = &s.colors;
        let (border, muted, text, size) = (c.border, c.muted, c.text, s.scaled(12.0));
        match self.r.doc.images.get(src) {
            Some(Image::Local { path, width, height }) => {
                let mut frame = div().w_full().max_w(px(*width));
                frame.style().aspect_ratio = Some(width / height);
                let (alt, shown) = (alt.to_string(), path.display().to_string());
                let fallback = move || {
                    let detail = format!("{} · {shown}", crate::i18n::text("无法显示图片", "Could not show image"));
                    placeholder(border, muted, text, size, &alt, &detail)
                };
                frame.child(img(path.clone()).size_full().object_fit(ObjectFit::Contain).with_fallback(fallback)).into_any_element()
            }
            Some(Image::Remote(url)) => {
                let detail = format!("{} · {url}", crate::i18n::text("远程图片未加载", "Remote image not loaded"));
                placeholder(border, muted, text, size, alt, &detail)
            }
            Some(Image::Unavailable { path, reason }) => placeholder(border, muted, text, size, alt, &format!("{reason} · {path}")),
            None => placeholder(border, muted, text, size, alt, src),
        }
    }

    fn footnotes(&mut self, notes: &[Footnote]) -> AnyElement {
        let s = self.s();
        let c = &s.colors;
        let (border, link, muted, size, gap) = (c.border, c.link, c.muted, s.scaled(12.0), s.scaled(6.0));
        let mut col = div().flex().flex_col().gap(s.scaled(2.0)).pt(s.scaled(8.0)).border_t_1().border_color(border).text_size(size);
        for note in notes {
            let body = self.blocks(&note.blocks, Tone { color: muted, size }, 0, 0.5);
            col = col.child(
                div().flex().gap(gap).child(div().flex_none().text_color(link).child(format!("[{}]", note.number))).child(div().flex_1().min_w(px(0.)).child(body)),
            );
        }
        col.into_any_element()
    }

    fn deleted(&mut self, row: usize, index: usize) -> AnyElement {
        let Some(d) = self.r.doc.changes.as_ref().and_then(|c| c.deleted.get(index)) else { return div().into_any_element() };
        let s = self.s();
        let c = &s.colors;
        let open = self.r.expanded.contains(&index);
        let label = deleted_label(d.lines.len(), open);
        let view = self.r.view.clone();
        let mut el = div()
            .id(("md-deleted", index))
            .cursor_pointer()
            .rounded(px(6.))
            .border_1()
            .border_dashed()
            .border_color(c.deleted_border)
            .bg(c.deleted_bg)
            .text_color(c.deleted_text)
            .text_size(s.scaled(12.0))
            .child(div().px(s.scaled(10.0)).py(s.scaled(4.0)).child(label))
            .on_click(move |_, _, cx| {
                let _ = view.update(cx, |v, cx| v.toggle_deleted(index, row, cx));
            });
        if open {
            let (pad_x, pad_y, size, border, bg) = (s.scaled(10.0), s.scaled(6.0), s.code, c.deleted_border, c.deleted_bg);
            let source = self.mono_text(&d.lines.join("\n"), self.s().colors.deleted_text);
            el = el.child(div().px(pad_x).py(pad_y).border_t_1().border_color(border).bg(bg).text_size(size).child(source));
        }
        el.into_any_element()
    }
}

/// The bar of a deleted block: how many lines, and what a click does.
fn deleted_label(lines: usize, open: bool) -> String {
    if crate::i18n::english() {
        format!("− Deleted {} · {}", crate::i18n::count(lines, "line", "lines"), if open { "click to collapse" } else { "click to expand" })
    } else {
        format!("− 已删除 {lines} 行 · {}", if open { "点击收起" } else { "点击展开" })
    }
}

/// Box standing in for an image that cannot be drawn.
fn placeholder(border: Hsla, muted: Hsla, text: Hsla, size: Pixels, alt: &str, detail: &str) -> AnyElement {
    let alt = if alt.is_empty() { crate::i18n::text("图片", "Image").to_string() } else { alt.to_string() };
    div()
        .flex()
        .flex_col()
        .px(px(12.))
        .py(px(10.))
        .rounded(px(6.))
        .border_1()
        .border_dashed()
        .border_color(border)
        .text_size(size)
        .child(div().text_color(text).child(format!("🖼 {alt}")))
        .child(div().text_color(muted).child(detail.to_string()))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_deleted_label_follows_the_language() {
        assert_eq!(super::deleted_label(3, false), "− 已删除 3 行 · 点击展开");
        let english = crate::i18n::with_language(crate::i18n::Language::English, || super::deleted_label(3, true));
        assert_eq!(english, "− Deleted 3 lines · click to collapse");
    }
}
