//! Paints an `EditorView`: line-number gutter, soft-wrapped rows on the terminal's cell grid, current-line tint,
//! selection, caret and IME preedit. Same structure as `terminal_element.rs`.

use gpui::{
    fill, point, px, relative, size, App, Bounds, Element, ElementId, ElementInputHandler, Entity, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, PaintQuad, Pixels, Point, ShapedLine, Style, TextRun, UnderlineStyle,
    Window,
};
use gilvt_editor::cell_width;
use gilvt_term::Palette;
use gilvt_viewer::TokenClass;
use unicode_segmentation::UnicodeSegmentation;

use super::syntax::class_style;
use super::view::{EditorView, LayoutInfo};
use crate::settings::Settings;
use crate::theme::{hsla, mix, terminal_font, AppSettings, CellMetrics};

/// Left padding of the text area, between the gutter and column 0.
pub(crate) const TEXT_PAD: f32 = 8.;
/// Width of the caret bar.
const CARET_W: f32 = 2.;

/// Editor colors derived from the terminal palette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EditorColors {
    pub fg: Hsla,
    pub bg: Hsla,
    pub dim: Hsla,
    pub sel: Hsla,
    pub cur_line: Hsla,
    pub caret: Hsla,
    pub warn: Hsla,
    pub err: Hsla,
    pub accent: Hsla,
}

impl EditorColors {
    pub(crate) fn from_palette(p: &Palette) -> Self {
        let accent = hsla(p.ansi[4]);
        Self {
            fg: hsla(p.foreground),
            bg: hsla(p.background),
            dim: hsla(mix(p.background, p.foreground, 0.45)),
            sel: accent.alpha(0.35),
            cur_line: hsla(mix(p.background, p.foreground, 0.06)),
            caret: hsla(p.cursor),
            warn: hsla(p.ansi[3]),
            err: hsla(p.ansi[1]),
            accent,
        }
    }
}

/// Gutter width in cells: room for the largest line number (at least 3 digits) plus one cell each side.
pub(crate) fn gutter_cols(line_count: usize) -> usize {
    let digits = line_count.max(1).to_string().len();
    digits.max(3) + 2
}

/// Text-area size in cells for an element `width` × `height` (all in pixels) with a `gutter` of that many cells.
pub(crate) fn grid_size(width: f32, height: f32, cell_w: f32, line_h: f32, gutter: usize) -> (usize, usize) {
    let text_w = width - cell_w * gutter as f32 - TEXT_PAD;
    let cols = if cell_w > 0. { (text_w / cell_w).max(0.) as usize } else { 0 };
    let rows = if line_h > 0. { (height / line_h).max(0.) as usize } else { 0 };
    (cols, rows)
}

/// A shaping run `(cell column, text)` plus its class runs `(byte length, class)`; the byte lengths sum to `text.len()`.
pub(crate) struct StyledSeg {
    pub col: usize,
    pub text: String,
    pub runs: Vec<(usize, Option<TokenClass>)>,
}

fn push_run(runs: &mut Vec<(usize, Option<TokenClass>)>, len: usize, class: Option<TokenClass>) {
    match runs.last_mut() {
        Some((n, c)) if *c == class => *n += len,
        _ => runs.push((len, class)),
    }
}

/// Splits a display row (tabs already expanded) into shaping runs: consecutive single-cell
/// ASCII graphemes form one run; every other grapheme is its own run pinned to its cell, so a fallback font's
/// advance (CJK, emoji) cannot push the rest of the row off the grid. Whitespace-only runs are dropped.
/// `classes` is one entry per grapheme (may be shorter or empty: missing means unstyled).
pub(crate) fn styled_segments(text: &str, classes: &[Option<TokenClass>]) -> Vec<StyledSeg> {
    let mut out = Vec::new();
    let mut run: Option<StyledSeg> = None;
    let mut col = 0;
    for (i, g) in text.graphemes(true).enumerate() {
        let class = classes.get(i).copied().flatten();
        let w = cell_width(g, col, 1);
        if g.is_ascii() && w == 1 {
            match &mut run {
                Some(seg) => {
                    seg.text.push_str(g);
                    push_run(&mut seg.runs, g.len(), class);
                }
                None => run = Some(StyledSeg { col, text: g.to_string(), runs: vec![(g.len(), class)] }),
            }
        } else {
            out.extend(run.take());
            out.push(StyledSeg { col, text: g.to_string(), runs: vec![(g.len(), class)] });
        }
        col += w;
    }
    out.extend(run);
    out.retain(|s| !s.text.trim().is_empty());
    out
}

/// Like `styled_segments` without classes: `(cell column, text)`.
#[cfg(test)]
pub(crate) fn segments(text: &str) -> Vec<(usize, String)> {
    styled_segments(text, &[]).into_iter().map(|s| (s.col, s.text)).collect()
}

pub struct EditorElement {
    view: Entity<EditorView>,
}

impl EditorElement {
    pub fn new(view: Entity<EditorView>) -> Self {
        Self { view }
    }
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

pub struct Frame {
    colors: EditorColors,
    line_h: Pixels,
    layout: LayoutInfo,
    cur_line: Vec<PaintQuad>,
    selection: Vec<PaintQuad>,
    numbers: Vec<(Point<Pixels>, ShapedLine)>,
    texts: Vec<(Point<Pixels>, ShapedLine)>,
    caret: Option<PaintQuad>,
    ime: Option<(PaintQuad, Point<Pixels>, ShapedLine)>,
}

fn text_run(len: usize, color: Hsla, underline: bool, bold: bool, italic: bool, s: &Settings) -> TextRun {
    TextRun {
        len,
        font: terminal_font(s, bold, italic),
        color,
        background_color: None,
        underline: underline.then_some(UnderlineStyle { color: Some(color), thickness: px(1.), wavy: false }),
        strikethrough: None,
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Frame;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // Full width; takes the height left over by the header / bars / status bar in the view's flex column.
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.flex_grow = 1.;
        style.flex_shrink = 1.;
        style.flex_basis = px(0.).into();
        style.min_size.height = px(0.).into();
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
        let colors = EditorColors::from_palette(&palette);
        let metrics = CellMetrics::measure(window, &settings);
        let (cw, lh) = (metrics.cell_width, metrics.line_height);

        let gutter = gutter_cols(self.view.read(cx).model.buf.line_count());
        let gutter_w = cw * gutter as f32;
        let (cols, rows) = grid_size(bounds.size.width / px(1.), bounds.size.height / px(1.), cw / px(1.), lh / px(1.), gutter);
        // Applied here, during layout, so the wrap always matches what is painted; no notify (that would loop).
        self.view.update(cx, |v, _| v.model.set_viewport(cols, rows));

        let row_classes: Vec<Vec<Option<TokenClass>>> = self.view.update(cx, |v, _| {
            let m = &mut v.model;
            let scroll = m.scroll_row();
            let visible = rows.min(m.total_rows().saturating_sub(scroll));
            let rc: Vec<_> = (0..visible).map(|vr| m.row_classes(scroll + vr)).collect();
            v.hl_stats = super::syntax::count_runs(&rc);
            rc
        });

        let view = self.view.read(cx);
        let model = &view.model;
        let focused = view.focus_handle.is_focused(window);
        let text_origin = point(bounds.origin.x + gutter_w + px(TEXT_PAD), bounds.origin.y);
        let row_y = |view_row: usize| bounds.origin.y + lh * view_row as f32;
        let caret_line = model.caret_position().line;
        let scroll = model.scroll_row();
        let visible = rows.min(model.total_rows().saturating_sub(scroll));

        let mut frame = Frame {
            colors,
            line_h: lh,
            layout: LayoutInfo { bounds, text_origin, cell_w: cw, line_h: lh, cols, rows },
            cur_line: Vec::new(),
            selection: Vec::new(),
            numbers: Vec::new(),
            texts: Vec::new(),
            caret: None,
            ime: None,
        };

        for (vr, classes) in row_classes.iter().enumerate() {
            let i = scroll + vr;
            let row = model.row(i);
            let y = row_y(vr);
            if row.line == caret_line {
                frame.cur_line.push(fill(Bounds::new(point(bounds.origin.x, y), size(bounds.size.width, lh)), colors.cur_line));
            }
            if let Some((a, b)) = model.selection_span(i) {
                let x = text_origin.x + cw * a as f32;
                frame.selection.push(fill(Bounds::new(point(x, y), size(cw * (b - a) as f32, lh)), colors.sel));
            }
            if model.is_first_row(i) {
                let n = (row.line + 1).to_string();
                let color = if row.line == caret_line { colors.fg } else { colors.dim };
                let shaped = window.text_system().shape_line(n.clone().into(), metrics.font_size, &[text_run(n.len(), color, false, false, false, &settings)], None);
                // Right-aligned so the last digit ends one cell before the text area.
                let x = bounds.origin.x + gutter_w - cw - shaped.width;
                frame.numbers.push((point(x, y), shaped));
            }
            for seg in styled_segments(&model.row_text(i), classes) {
                let runs: Vec<TextRun> = seg
                    .runs
                    .iter()
                    .map(|(len, class)| match class {
                        Some(c) => {
                            let s = class_style(*c, &palette);
                            text_run(*len, hsla(s.fg), false, s.bold, s.italic, &settings)
                        }
                        None => text_run(*len, colors.fg, false, false, false, &settings),
                    })
                    .collect();
                let shaped = window.text_system().shape_line(seg.text.into(), metrics.font_size, &runs, None);
                frame.texts.push((point(text_origin.x + cw * (row.indent + seg.col) as f32, y), shaped));
            }
        }

        let caret_row = model.caret_row();
        if focused && caret_row >= scroll && caret_row < scroll + visible {
            let mut at = point(text_origin.x + cw * model.caret_visual_col() as f32, row_y(caret_row - scroll));
            if let Some(text) = view.marked.clone().filter(|t| !t.is_empty()) {
                let r = text_run(text.len(), colors.fg, true, false, false, &settings);
                let shaped = window.text_system().shape_line(text.into(), metrics.font_size, &[r], None);
                let bg = fill(Bounds::new(at, size(shaped.width, lh)), colors.bg);
                let after = at.x + shaped.width;
                frame.ime = Some((bg, at, shaped));
                at.x = after;
            }
            frame.caret = Some(fill(Bounds::new(at, size(px(CARET_W), lh)), colors.caret));
        }
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
        let focus_handle = self.view.read(cx).focus_handle.clone();
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.view.clone()), cx);

        let lh = frame.line_h;
        window.paint_quad(fill(bounds, frame.colors.bg));
        for q in frame.cur_line.drain(..).chain(frame.selection.drain(..)) {
            window.paint_quad(q);
        }
        for (at, line) in frame.numbers.drain(..).chain(frame.texts.drain(..)) {
            let _ = line.paint(at, lh, window, cx);
        }
        if let Some((bg, at, line)) = frame.ime.take() {
            window.paint_quad(bg);
            let _ = line.paint(at, lh, window, cx);
        }
        if let Some(c) = frame.caret.take() {
            window.paint_quad(c);
        }
        let layout = frame.layout;
        let flipped = self.view.update(cx, |v, _| {
            let was = v.too_narrow();
            v.layout = Some(layout);
            was != v.too_narrow()
        });
        // The view lays out the chrome from the last painted width: re-render once when 「窗口太窄」 flips.
        // (A notify during paint does not schedule a frame, so it is deferred.)
        if flipped {
            let view = self.view.clone();
            window.defer(cx, move |_, cx| view.update(cx, |_, cx| cx.notify()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gutter_fits_the_largest_line_number_with_a_three_digit_minimum() {
        assert_eq!(gutter_cols(0), 5);
        assert_eq!(gutter_cols(1), 5);
        assert_eq!(gutter_cols(999), 5);
        assert_eq!(gutter_cols(1000), 6);
        assert_eq!(gutter_cols(123_456), 8);
    }

    #[test]
    fn grid_size_subtracts_the_gutter_and_padding() {
        // 5-cell gutter of 10px + 8px padding leaves 492px = 49 whole cells; 300px / 20px = 15 rows.
        assert_eq!(grid_size(550., 300., 10., 20., 5), (49, 15));
        assert_eq!(grid_size(550., 319., 10., 20., 5), (49, 15));
    }

    #[test]
    fn grid_size_never_goes_negative() {
        assert_eq!(grid_size(30., 10., 10., 20., 5), (0, 0));
        assert_eq!(grid_size(100., 100., 0., 0., 5), (0, 0));
    }

    #[test]
    fn segments_group_ascii_and_pin_wide_characters_to_their_cells() {
        assert_eq!(segments("ab 你好c"), vec![(0, "ab ".into()), (3, "你".into()), (5, "好".into()), (7, "c".into())]);
    }

    #[test]
    fn segments_drop_whitespace_only_runs() {
        assert_eq!(segments("    x  "), vec![(0, "    x  ".into())]);
        assert_eq!(segments("   "), Vec::<(usize, String)>::new());
        assert_eq!(segments("你  好"), vec![(0, "你".into()), (4, "好".into())]);
    }

    use gilvt_viewer::TokenClass::{Comment, Keyword, Number};

    #[test]
    fn styled_segments_split_an_ascii_run_at_class_boundaries() {
        let s = styled_segments("let x", &[Some(Keyword), Some(Keyword), Some(Keyword), None, None]);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].col, s[0].text.as_str()), (0, "let x"));
        assert_eq!(s[0].runs, vec![(3, Some(Keyword)), (2, None)]);
    }

    #[test]
    fn styled_segments_pin_wide_graphemes_and_carry_their_class() {
        let s = styled_segments("a你b", &[Some(Number), Some(Comment), Some(Keyword)]);
        let got: Vec<_> = s.iter().map(|x| (x.col, x.text.as_str(), x.runs.clone())).collect();
        assert_eq!(got, vec![(0, "a", vec![(1, Some(Number))]), (1, "你", vec![(3, Some(Comment))]), (3, "b", vec![(1, Some(Keyword))])]);
    }

    #[test]
    fn styled_segments_without_classes_equal_the_plain_segments() {
        for t in ["ab 你好c", "    x  ", "   ", "你  好", ""] {
            let plain: Vec<_> = segments(t);
            let styled: Vec<_> = styled_segments(t, &[]).into_iter().map(|s| (s.col, s.text)).collect();
            assert_eq!(plain, styled, "{t:?}");
            assert!(styled_segments(t, &[]).iter().all(|s| s.runs == vec![(s.text.len(), None)]));
        }
    }

    #[test]
    fn run_lengths_always_sum_to_the_text_length() {
        let t = "é = \"ü\" 你";
        let classes: Vec<_> = (0..t.chars().count() + 2).map(|i| if i % 3 == 0 { Some(Keyword) } else { None }).collect();
        for s in styled_segments(t, &classes) {
            assert_eq!(s.runs.iter().map(|r| r.0).sum::<usize>(), s.text.len());
        }
    }

    #[test]
    fn colors_derive_from_the_palette() {
        let p = Palette::dark();
        let c = EditorColors::from_palette(&p);
        assert_eq!(c.bg, hsla(p.background));
        assert_eq!(c.cur_line, hsla(mix(p.background, p.foreground, 0.06)));
        assert_eq!(c.sel.a, 0.35);
        assert_ne!(c.cur_line, c.bg);
    }
}
