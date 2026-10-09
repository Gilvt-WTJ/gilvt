//! Paints the visible rows of a `PreviewView`: gutter, syntax-colored text, diff tints,
//! word emphasis, folds, the annotation badge and the change rail.

use gpui::{
    fill, outline, point, px, relative, size, App, BorderStyle, Bounds, ContentMask, Element, ElementId, Entity,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point, SharedString, Style, TextRun,
    Window,
};
use gilvt_term::{Palette, Rgb};
use gilvt_viewer::display::display_line;
use gilvt_viewer::{nav, DiffLine, LineKind, Row, SplitRow};

use crate::preview_select::{CellHit, Selection, Side};
use crate::preview_view::{Mode, PreviewLayout, PreviewView, Rows};
use crate::theme::{hsla, mix, terminal_font, AppSettings, CellMetrics};

pub struct PreviewElement {
    view: Entity<PreviewView>,
}

impl PreviewElement {
    pub fn new(view: Entity<PreviewView>) -> Self {
        Self { view }
    }
}

impl IntoElement for PreviewElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

const RAIL_WIDTH: f32 = 10.0;

/// What a change-rail mark stands for.
#[derive(Clone, Copy)]
enum Mark {
    Added,
    Removed,
    Modified,
}

fn mark_for(kind: LineKind) -> Option<Mark> {
    match kind {
        LineKind::Added => Some(Mark::Added),
        LineKind::Removed => Some(Mark::Removed),
        LineKind::Context => None,
    }
}

struct Colors {
    bg: Rgb,
    fg: Rgb,
    muted: Rgb,
    added: Rgb,
    removed: Rgb,
    added_emph: Rgb,
    removed_emph: Rgb,
    fold: Rgb,
    warn: Rgb,
    warn_bg: Rgb,
    changed: Rgb,
}

impl Colors {
    fn new(p: &Palette) -> Self {
        let green = p.ansi[2];
        let red = p.ansi[1];
        Colors {
            bg: p.background,
            fg: p.foreground,
            muted: mix(p.foreground, p.background, 0.5),
            added: mix(p.background, green, 0.14),
            removed: mix(p.background, red, 0.14),
            added_emph: mix(p.background, green, 0.34),
            removed_emph: mix(p.background, red, 0.34),
            fold: mix(p.background, p.foreground, 0.06),
            warn: p.ansi[3],
            warn_bg: mix(p.background, p.ansi[3], 0.18),
            changed: p.ansi[3],
        }
    }
}

fn to_rgb(c: gilvt_viewer::Color) -> Rgb {
    Rgb { r: c.r, g: c.g, b: c.b }
}

/// Everything painted in one frame, collected during prepaint.
#[derive(Default)]
pub struct Frame {
    quads: Vec<gpui::PaintQuad>,
    /// Text to paint, with an optional clip (split halves).
    texts: Vec<(Point<Pixels>, gpui::ShapedLine, Option<Bounds<Pixels>>)>,
    line_height: Pixels,
    /// Shaped cells of this frame, kept by the view for mouse hit-testing.
    hits: Vec<CellHit>,
}

struct Cell<'a> {
    line: &'a DiffLine,
    spans: &'a [gilvt_viewer::Span],
}

struct Painter<'a> {
    settings: &'a crate::settings::Settings,
    metrics: CellMetrics,
    colors: &'a Colors,
    frame: &'a mut Frame,
    annotation: Option<&'a (u32, String)>,
    selection: Option<Selection>,
    selection_bg: Rgb,
}

impl Painter<'_> {
    fn shape(&self, window: &mut Window, text: String, runs: &[(usize, Rgb, bool, bool)]) -> gpui::ShapedLine {
        let runs: Vec<TextRun> = runs
            .iter()
            .map(|&(len, color, bold, italic)| TextRun {
                len,
                font: terminal_font(self.settings, bold, italic),
                color: hsla(color),
                background_color: None,
                underline: None,
                strikethrough: None,
            })
            .collect();
        window.text_system().shape_line(SharedString::from(text), self.metrics.font_size, &runs, None)
    }

    fn plain(&self, window: &mut Window, text: String, color: Rgb) -> gpui::ShapedLine {
        let len = text.len();
        self.shape(window, text, &[(len, color, false, false)])
    }

    /// One code cell (a diff line in a column starting at `x`, `width` wide).
    fn cell(&mut self, window: &mut Window, cell: Cell, at: (usize, Side), x: Pixels, y: Pixels, width: Pixels, gutter_cols: usize) {
        let (row, side) = at;
        let (cw, lh) = (self.metrics.cell_width, self.metrics.line_height);
        let c = self.colors;
        let (bg, emph_bg, marker) = match cell.line.kind {
            LineKind::Added => (Some(c.added), c.added_emph, "+"),
            LineKind::Removed => (Some(c.removed), c.removed_emph, "-"),
            LineKind::Context => (None, c.bg, " "),
        };
        let clip = Bounds::new(point(x, y), size(width, lh));
        if let Some(bg) = bg {
            self.frame.quads.push(fill(clip, hsla(bg)));
        }
        let number = match cell.line.kind {
            LineKind::Removed => cell.line.old_no,
            _ => cell.line.new_no,
        };
        let gutter = format!("{:>w$} {marker}", number.map(|n| n.to_string()).unwrap_or_default(), w = gutter_cols);
        let g = self.plain(window, gutter, c.muted);
        self.frame.texts.push((point(x, y), g, Some(clip)));

        let text_x = x + cw * (gutter_cols + 3) as f32;
        let default_fg = gilvt_viewer::Color { r: c.fg.r, g: c.fg.g, b: c.fg.b };
        let d = display_line(&cell.line.text, cell.spans, &cell.line.emphasis, default_fg);
        let runs: Vec<(usize, Rgb, bool, bool)> = d.runs.iter().map(|r| (r.len, to_rgb(r.fg), r.bold, r.italic)).collect();
        let shaped = self.shape(window, d.text, &runs);
        for e in &d.emphasis {
            let x0 = text_x + shaped.x_for_index(e.start);
            let x1 = text_x + shaped.x_for_index(e.end);
            self.frame.quads.push(fill(Bounds::new(point(x0, y), size(x1 - x0, lh)), hsla(emph_bg)));
        }
        if let Some(sel) = self.selection.filter(|s| s.side == side && !s.is_empty()) {
            let (a, b) = sel.ordered();
            if (a.row..=b.row).contains(&row) {
                let len = shaped.len();
                let start = if row == a.row { a.off.min(len) } else { 0 };
                let (end, eol) = if row == b.row { (b.off.min(len), false) } else { (len, true) };
                let x0 = text_x + shaped.x_for_index(start);
                let x1 = (text_x + shaped.x_for_index(end) + if eol { cw } else { px(0.) }).min(x + width);
                if x1 > x0 {
                    self.frame.quads.push(fill(Bounds::new(point(x0, y), size(x1 - x0, lh)), hsla(self.selection_bg)));
                }
            }
        }
        let text_end = text_x + shaped.width;
        self.frame.hits.push(CellHit { row, side, text_x, line: shaped.clone() });
        self.frame.texts.push((point(text_x, y), shaped, Some(clip)));

        if let Some((line, msg)) = self.annotation {
            if cell.line.kind != LineKind::Removed && cell.line.new_no == Some(*line) {
                let badge = self.plain(window, format!(" ⚠ {msg} "), c.warn);
                let bx = text_end + cw * 2.;
                self.frame.quads.push(fill(Bounds::new(point(bx, y), size(badge.width, lh)), hsla(c.warn_bg)));
                self.frame.texts.push((point(bx, y), badge, Some(clip)));
            }
        }
    }

    fn fold(&mut self, window: &mut Window, len: usize, x: Pixels, y: Pixels, width: Pixels) {
        let lh = self.metrics.line_height;
        let bounds = Bounds::new(point(x, y), size(width, lh));
        self.frame.quads.push(fill(bounds, hsla(self.colors.fold)));
        let label = self.plain(window, fold_label(len), self.colors.muted);
        self.frame.texts.push((point(x, y), label, Some(bounds)));
    }
}

/// A folded run of unchanged lines.
fn fold_label(len: usize) -> String {
    if crate::i18n::english() {
        format!("    ⋯ {} (click to expand)", crate::i18n::count(len, "unchanged line", "unchanged lines"))
    } else {
        format!("    ⋯ {len} 行未改动（点击展开）")
    }
}

impl Element for PreviewElement {
    type RequestLayoutState = ();
    type PrepaintState = Frame;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Frame {
        let settings = cx.global::<AppSettings>().0.clone();
        let palette = crate::theme::current(cx).palette.clone();
        let colors = Colors::new(&palette);
        let metrics = CellMetrics::measure(window, &settings);
        let lh = metrics.line_height;
        let rail = Bounds::new(point(bounds.right() - px(RAIL_WIDTH), bounds.top()), size(px(RAIL_WIDTH), bounds.size.height));
        let content = Bounds::new(bounds.origin, size(bounds.size.width - px(RAIL_WIDTH), bounds.size.height));
        let visible_rows = ((bounds.size.height / lh).ceil() as usize).max(1);

        self.view.update(cx, |v, _| v.unfold_pending_line());
        let mode = self.view.read(cx).mode_for_width(content.size.width);
        let Some(rows) = self.view.read(cx).rows(mode) else { return Frame::default() };
        let total = rows.len();
        let layout = PreviewLayout { bounds: content, row_height: lh, visible_rows, total_rows: total, rail, mode };

        // Apply a pending "scroll to line" now that rows exist, then clamp.
        let scroll_top = self.view.update(cx, |v, _| {
            v.layout = Some(layout);
            if let (Some(line), Some(diff)) = (v.pending_line.take(), v.loaded.as_ref().and_then(|l| l.preview.diff.as_ref())) {
                let idx = diff.index_of_new_line(line);
                let row = match &rows {
                    Rows::Unified(r) => nav::unified_row_of(r, idx),
                    Rows::Split(r) => nav::split_row_of(r, idx),
                };
                v.scroll_top = row as f32 - 3.0;
            }
            let max = total.saturating_sub(visible_rows.saturating_sub(1)) as f32;
            v.scroll_top = v.scroll_top.clamp(0.0, max);
            v.scroll_top
        });

        let view = self.view.read(cx);
        let Some(loaded) = view.loaded.as_ref() else { return Frame::default() };
        let Some(diff) = loaded.preview.diff.as_ref() else { return Frame::default() };
        let max_no = diff.lines.iter().filter_map(|l| l.new_no.max(l.old_no)).max().unwrap_or(1);
        let gutter_cols = max_no.to_string().len();
        let annotation = view.annotation.clone();
        let selection = view.selection;
        let spans = &loaded.spans;
        let empty: Vec<gilvt_viewer::Span> = Vec::new();

        let mut frame = Frame { line_height: lh, ..Frame::default() };
        let first = scroll_top.floor() as usize;
        let offset = scroll_top - first as f32;
        let mut painter = Painter {
            settings: &settings,
            metrics,
            colors: &colors,
            frame: &mut frame,
            annotation: annotation.as_ref(),
            selection,
            selection_bg: palette.selection,
        };

        for (k, r) in (first..(first + visible_rows + 1).min(total)).enumerate() {
            let y = content.top() + lh * (k as f32 - offset);
            match &rows {
                Rows::Unified(rows) => match rows[r] {
                    Row::Line(i) => {
                        let cell = Cell { line: &diff.lines[i], spans: spans.get(i).unwrap_or(&empty) };
                        painter.cell(window, cell, (r, Side::Whole), content.left(), y, content.size.width, gutter_cols);
                    }
                    Row::Fold { len, .. } => painter.fold(window, len, content.left(), y, content.size.width),
                },
                Rows::Split(rows) => {
                    let half = content.size.width / 2.;
                    match rows[r] {
                        SplitRow::Pair { left, right } => {
                            for (idx, x, side) in [(left, content.left(), Side::Left), (right, content.left() + half, Side::Right)] {
                                if let Some(i) = idx {
                                    let cell = Cell { line: &diff.lines[i], spans: spans.get(i).unwrap_or(&empty) };
                                    painter.cell(window, cell, (r, side), x, y, half - px(1.), gutter_cols);
                                }
                            }
                        }
                        SplitRow::Fold { len, .. } => painter.fold(window, len, content.left(), y, content.size.width),
                    }
                }
            }
        }
        if mode == Mode::Split {
            let x = content.left() + content.size.width / 2. - px(1.);
            frame.quads.push(fill(Bounds::new(point(x, content.top()), size(px(1.), content.size.height)), hsla(colors.fold)));
        }

        // Change rail: a mark per change block, plus the viewport.
        frame.quads.push(fill(rail, hsla(colors.fold)));
        let mark_at: Box<dyn Fn(usize) -> Option<Mark>> = match &rows {
            Rows::Unified(rows) => Box::new(|r| match rows[r] {
                Row::Line(i) => mark_for(diff.lines[i].kind),
                Row::Fold { .. } => None,
            }),
            Rows::Split(rows) => Box::new(|r| match rows[r] {
                SplitRow::Pair { left, right } => match (left.map(|i| diff.lines[i].kind), right.map(|i| diff.lines[i].kind)) {
                    (Some(LineKind::Removed), Some(LineKind::Added)) => Some(Mark::Modified),
                    (l, r) => l.and_then(mark_for).or_else(|| r.and_then(mark_for)),
                },
                SplitRow::Fold { .. } => None,
            }),
        };
        let scale = rail.size.height / total.max(1) as f32;
        for r in 0..total {
            if let Some(mark) = mark_at(r) {
                let color: Hsla = match mark {
                    Mark::Added => hsla(palette.ansi[2]),
                    Mark::Removed => hsla(palette.ansi[1]),
                    Mark::Modified => hsla(colors.changed),
                };
                let y = rail.top() + scale * r as f32;
                frame.quads.push(fill(Bounds::new(point(rail.left() + px(2.), y), size(px(RAIL_WIDTH - 4.), scale.max(px(2.)))), color));
            }
        }
        let vp_top = rail.top() + scale * scroll_top;
        let vp_h = (scale * visible_rows as f32).min(rail.size.height);
        frame.quads.push(outline(Bounds::new(point(rail.left(), vp_top), size(px(RAIL_WIDTH), vp_h)), hsla(colors.muted), BorderStyle::Solid));
        frame
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        frame: &mut Frame,
        window: &mut Window,
        cx: &mut App,
    ) {
        let hits = std::mem::take(&mut frame.hits);
        self.view.update(cx, |v, _| v.hits = hits);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for q in frame.quads.drain(..) {
                window.paint_quad(q);
            }
            let lh = frame.line_height;
            for (at, line, clip) in frame.texts.drain(..) {
                match clip {
                    Some(clip) => window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                        let _ = line.paint(at, lh, window, cx);
                    }),
                    None => {
                        let _ = line.paint(at, lh, window, cx);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_fold_label_follows_the_language() {
        assert_eq!(super::fold_label(3), "    ⋯ 3 行未改动（点击展开）");
        let english = crate::i18n::with_language(crate::i18n::Language::English, || super::fold_label(3));
        assert_eq!(english, "    ⋯ 3 unchanged lines (click to expand)");
    }
}
