//! Paints a terminal snapshot: backgrounds, text runs aligned to the cell grid, cursor, IME text.

use gpui::{
    fill, outline, point, px, relative, size, App, BorderStyle, Bounds, Element, ElementId, ElementInputHandler,
    Entity, GlobalElementId, InspectorElementId, IntoElement, LayoutId, PaintQuad, Pixels, Point, ShapedLine,
    StrikethroughStyle, Style, TextRun, UnderlineStyle, Window,
};
use gilvt_term::snapshot::{CellView, CursorKind, Snapshot, Style as CellStyle};
use gilvt_term::{Palette, Rgb, TermSize};

use crate::settings::Settings;
use crate::terminal_view::{LayoutInfo, TerminalView, JUMP_ROWS};
use crate::theme::{hsla, terminal_font, AppSettings, CellMetrics};

pub struct TerminalElement {
    view: Entity<TerminalView>,
}

impl TerminalElement {
    pub fn new(view: Entity<TerminalView>) -> Self {
        Self { view }
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

pub struct Frame {
    snapshot: Option<Snapshot>,
    metrics: CellMetrics,
    background: Rgb,
    rects: Vec<PaintQuad>,
    texts: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<PaintQuad>,
    cursor_text: Option<(Point<Pixels>, ShapedLine)>,
    ime: Option<(PaintQuad, Point<Pixels>, ShapedLine)>,
}

fn run(len: usize, fg: Rgb, style: CellStyle, s: &Settings) -> TextRun {
    let color = hsla(fg);
    TextRun {
        len,
        font: terminal_font(s, style.bold, style.italic),
        color,
        background_color: None,
        underline: (style.underline || style.undercurl).then_some(UnderlineStyle {
            color: Some(color),
            thickness: px(1.),
            wavy: style.undercurl,
        }),
        strikethrough: style.strikethrough.then_some(StrikethroughStyle { color: Some(color), thickness: px(1.) }),
    }
}

fn cell_bg(cell: &CellView, p: &Palette) -> Rgb {
    if cell.search_match {
        p.search_match
    } else if cell.selected {
        p.selection
    } else {
        cell.bg
    }
}

/// A cell's text color: the theme's selection foreground on selected cells (not on search matches,
/// whose background wins), else the cell's own.
fn cell_fg(cell: &CellView, p: &Palette) -> Rgb {
    match p.selection_foreground {
        Some(fg) if cell.selected && !cell.search_match => fg,
        _ => cell.fg,
    }
}

/// A cell that can be drawn as part of a single-font ASCII run.
fn is_ascii_cell(c: &CellView) -> bool {
    !c.wide && !c.text.is_empty() && c.text.is_ascii()
}

/// The style to actually paint a cell with: its own style, plus a forced underline for
/// hyperlinks and Cmd-hovered link spans (see `is_underlined` in `prepaint`).
fn effective_style(cell: &CellView, underlined: bool) -> CellStyle {
    if underlined && !cell.style.underline {
        CellStyle { underline: true, ..cell.style }
    } else {
        cell.style
    }
}

impl Element for TerminalElement {
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
        let metrics = CellMetrics::measure(window, &settings);
        let (cw, lh) = (metrics.cell_width, metrics.line_height);
        let grid = TermSize::from_pixels(bounds.size.width / px(1.), bounds.size.height / px(1.), cw / px(1.), lh / px(1.));

        let (snapshot, focused, marked, hovered_link, jump_rows) = self.view.update(cx, |view, _| {
            view.session.resize(grid);
            // Recomputed every frame: the rows move when output arrives or the user scrolls meanwhile.
            let jump_rows = view.jump.and_then(|(line, _)| view.session.absolute_rows_visible(line, JUMP_ROWS));
            (view.snapshot(&palette), view.focus_handle.is_focused(window), view.marked_text.clone(), view.hovered_link.clone(), jump_rows)
        });
        // Cells that carry an OSC 8 hyperlink are always underlined; cells inside the Cmd-hovered
        // link span are underlined only while that hover is active.
        let is_underlined = |row: usize, col: usize, cell: &CellView| {
            cell.hyperlink.is_some() || hovered_link.as_ref().is_some_and(|(r, span)| *r == row && span.contains(&col))
        };

        let origin = bounds.origin;
        let cell_origin = |row: usize, col: usize| point(origin.x + cw * col as f32, origin.y + lh * row as f32);
        let mut rects = Vec::new();
        let mut texts = Vec::new();

        for (row, cells) in snapshot.rows.iter().enumerate() {
            // Backgrounds: merge adjacent cells of the same non-default color.
            let mut col = 0;
            while col < cells.len() {
                let bg = cell_bg(&cells[col], &palette);
                if bg == snapshot.background {
                    col += 1;
                    continue;
                }
                let start = col;
                while col < cells.len() && cell_bg(&cells[col], &palette) == bg {
                    col += 1;
                }
                rects.push(fill(Bounds::new(cell_origin(row, start), size(cw * (col - start) as f32, lh)), hsla(bg)));
            }

            // Text: ASCII cells with identical style are shaped as one run; every other
            // character is shaped alone and pinned to its own cell so fallback fonts stay aligned.
            let mut col = 0;
            while col < cells.len() {
                let cell = &cells[col];
                let style = effective_style(cell, is_underlined(row, col, cell));
                let decorated = style.underline || style.undercurl || style.strikethrough;
                if cell.text.is_empty() || (cell.text == " " && !decorated) {
                    col += 1;
                    continue;
                }
                let start = col;
                let mut text = String::new();
                if is_ascii_cell(cell) {
                    while col < cells.len()
                        && is_ascii_cell(&cells[col])
                        && cell_fg(&cells[col], &palette) == cell_fg(cell, &palette)
                        && effective_style(&cells[col], is_underlined(row, col, &cells[col])) == style
                    {
                        text.push_str(&cells[col].text);
                        col += 1;
                    }
                } else {
                    text.push_str(&cell.text);
                    col += 1;
                }
                let r = run(text.len(), cell_fg(cell, &palette), style, &settings);
                let line = window.text_system().shape_line(text.into(), metrics.font_size, &[r], None);
                texts.push((cell_origin(row, start), line));
            }
        }

        // Timeline jump: a tint under the text of the highlighted rows, and an outline around them.
        if let Some(rows) = jump_rows.filter(|r| !r.is_empty()) {
            let tint = hsla(palette.ansi[4]);
            let area = Bounds::new(cell_origin(rows.start, 0), size(bounds.size.width, lh * rows.len() as f32));
            rects.push(fill(area, tint.opacity(0.22)));
            rects.push(outline(area, tint.opacity(0.7), BorderStyle::Solid));
        }

        let mut cursor = None;
        let mut cursor_text = None;
        if let Some(c) = snapshot.cursor {
            let w = if c.wide { cw * 2. } else { cw };
            let at = cell_origin(c.row, c.col);
            let color = hsla(palette.cursor);
            let kind = if focused { c.kind } else { CursorKind::HollowBlock };
            cursor = Some(match kind {
                CursorKind::Block => {
                    if let Some(cell) = snapshot.rows[c.row].get(c.col).filter(|cell| !cell.text.is_empty() && cell.text != " ") {
                        let r = run(cell.text.len(), palette.cursor_text.unwrap_or(snapshot.background), cell.style, &settings);
                        let line = window.text_system().shape_line(cell.text.clone().into(), metrics.font_size, &[r], None);
                        cursor_text = Some((at, line));
                    }
                    fill(Bounds::new(at, size(w, lh)), color)
                }
                CursorKind::Beam => fill(Bounds::new(at, size(px(2.), lh)), color),
                CursorKind::Underline => fill(Bounds::new(point(at.x, at.y + lh - px(2.)), size(w, px(2.))), color),
                CursorKind::HollowBlock => outline(Bounds::new(at, size(w, lh)), color, BorderStyle::Solid),
            });
        }

        let ime = match (marked, snapshot.cursor) {
            (Some(text), Some(c)) => {
                let at = cell_origin(c.row, c.col);
                let style = CellStyle { underline: true, ..CellStyle::default() };
                let r = run(text.len(), palette.foreground, style, &settings);
                let line = window.text_system().shape_line(text.into(), metrics.font_size, &[r], None);
                let bg = fill(Bounds::new(at, size(line.width, lh)), hsla(palette.background));
                cursor = None;
                cursor_text = None;
                Some((bg, at, line))
            }
            _ => None,
        };

        Frame { background: snapshot.background, snapshot: Some(snapshot), metrics, rects, texts, cursor, cursor_text, ime }
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
        let (focus_handle, searching) = {
            let view = self.view.read(cx);
            (view.focus_handle.clone(), view.searching())
        };
        if !searching {
            window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.view.clone()), cx);
        }
        let lh = frame.metrics.line_height;
        window.paint_quad(fill(bounds, hsla(frame.background)));
        for rect in frame.rects.drain(..) {
            window.paint_quad(rect);
        }
        for (at, line) in frame.texts.drain(..) {
            let _ = line.paint(at, lh, window, cx);
        }
        if let Some(c) = frame.cursor.take() {
            window.paint_quad(c);
        }
        if let Some((at, line)) = frame.cursor_text.take() {
            let _ = line.paint(at, lh, window, cx);
        }
        if let Some((bg, at, line)) = frame.ime.take() {
            window.paint_quad(bg);
            let _ = line.paint(at, lh, window, cx);
        }
        if let Some(snapshot) = frame.snapshot.take() {
            let metrics = frame.metrics;
            self.view.update(cx, |view, _| view.layout = Some(LayoutInfo { bounds, metrics, snapshot }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_foreground_recolors_selected_cells_only() {
        let mut p = gilvt_term::Palette::dark();
        let mut cell = CellView { fg: p.ansi[1], selected: true, search_match: false, ..CellView::default() };
        assert_eq!(cell_fg(&cell, &p), p.ansi[1], "None keeps the cell's color");
        p.selection_foreground = Some(gilvt_term::Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(cell_fg(&cell, &p), gilvt_term::Rgb { r: 1, g: 2, b: 3 });
        cell.search_match = true;
        assert_eq!(cell_fg(&cell, &p), p.ansi[1], "a search match keeps its own text color");
        cell.search_match = false;
        cell.selected = false;
        assert_eq!(cell_fg(&cell, &p), p.ansi[1]);
    }
}
