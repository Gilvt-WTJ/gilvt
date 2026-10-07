//! Mouse selection and ⌘C in the code / diff view of `PreviewView` (the rendered Markdown view's is in `markdown::select`) (the text is painted by
//! `PreviewElement`, so the selection is kept as row + display-byte positions and hit-tested
//! against the shaped lines of the last paint).

use gilvt_viewer::display::source_offset;
use gilvt_viewer::{Row, SplitRow};
use gpui::{ClipboardItem, Context, MouseMoveEvent, Pixels, Point, ShapedLine, Window};

use crate::actions::Copy;
use crate::preview_view::{Mode, PreviewView, Rows};

/// Which column a selection lives in (a split diff selects within one half).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Whole,
    Left,
    Right,
}

/// A point in the text: a row of the current `Rows` and a byte offset in that cell's display text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub row: usize,
    pub off: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct Selection {
    pub side: Side,
    pub anchor: Pos,
    pub head: Pos,
}

impl Selection {
    /// (start, end) in reading order.
    pub fn ordered(&self) -> (Pos, Pos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// A shaped code cell of the last paint, for hit-testing.
#[derive(Clone)]
pub struct CellHit {
    pub row: usize,
    pub side: Side,
    pub text_x: Pixels,
    pub line: ShapedLine,
}

impl PreviewView {
    /// Text position under `at`; `side` pins the column while dragging.
    fn pos_at(&self, at: Point<Pixels>, side: Option<Side>) -> Option<(Side, Pos)> {
        let layout = self.layout?;
        let top = layout.bounds.origin.y;
        let row = (self.scroll_top + (at.y - top) / layout.row_height)
            .floor()
            .max(0.0) as usize;
        let row = row.min(layout.total_rows.saturating_sub(1));
        let side = side.unwrap_or(match layout.mode {
            Mode::Unified => Side::Whole,
            Mode::Split if at.x >= layout.bounds.origin.x + layout.bounds.size.width / 2. => {
                Side::Right
            }
            Mode::Split => Side::Left,
        });
        let off = self
            .hits
            .iter()
            .find(|h| h.row == row && h.side == side)
            .map_or(0, |h| {
                h.line
                    .closest_index_for_x((at.x - h.text_x).max(Pixels::ZERO))
            });
        Some((side, Pos { row, off }))
    }

    /// Starts a selection at `at`; false when `at` is outside the code (gutter rail, nothing loaded).
    pub(crate) fn begin_select(&mut self, at: Point<Pixels>) {
        self.md_sel = None;
        self.selection = None;
        self.dragging = false;
        if let Some((side, pos)) = self.pos_at(at, None) {
            self.selection = Some(Selection {
                side,
                anchor: pos,
                head: pos,
            });
            self.dragging = true;
        }
    }

    pub(crate) fn drag_select(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.dragging {
            return;
        }
        if event.pressed_button != Some(gpui::MouseButton::Left) {
            self.dragging = false;
            return;
        }
        if self.rendered() {
            self.md_drag_select(event.position, cx);
            return;
        }
        let Some(sel) = self.selection else { return };
        if let Some((_, pos)) = self.pos_at(event.position, Some(sel.side)) {
            if pos != sel.head {
                self.selection = Some(Selection { head: pos, ..sel });
                cx.notify();
            }
        }
    }

    pub(crate) fn end_select(&mut self, cx: &mut Context<Self>) {
        self.dragging = false;
        self.md_end_select(cx);
        if self.selection.is_some_and(|s| s.is_empty()) {
            self.selection = None;
            cx.notify();
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection = None;
        self.md_sel = None;
        self.dragging = false;
    }

    /// Source text of the selection (tabs intact), lines joined with `\n`.
    pub(crate) fn selection_text(&self) -> Option<String> {
        if self.rendered() {
            return self.md_selection_text();
        }
        let sel = self.selection.filter(|s| !s.is_empty())?;
        let (a, b) = sel.ordered();
        let diff = self.loaded.as_ref()?.preview.diff.as_ref()?;
        let rows = self.rows(self.layout?.mode)?;
        let mut lines = Vec::new();
        for r in a.row..=b.row.min(rows.len().saturating_sub(1)) {
            let idx = match &rows {
                Rows::Unified(rows) => match rows[r] {
                    Row::Line(i) => Some(i),
                    Row::Fold { .. } => continue,
                },
                Rows::Split(rows) => match rows[r] {
                    SplitRow::Pair { left, right } => {
                        if sel.side == Side::Right {
                            right
                        } else {
                            left
                        }
                    }
                    SplitRow::Fold { .. } => continue,
                },
            };
            let text = idx.map_or("", |i| diff.lines[i].text.as_str());
            let from = if r == a.row {
                source_offset(text, a.off)
            } else {
                0
            };
            let to = if r == b.row {
                source_offset(text, b.off)
            } else {
                text.len()
            };
            lines.push(text.get(from..to.max(from)).unwrap_or(""));
        }
        Some(lines.join("\n"))
    }

    pub(crate) fn copy(&mut self, _: &Copy, window: &mut Window, cx: &mut Context<Self>) {
        if self.rendered() {
            self.md_register_rows(window, cx);
        }
        if let Some(text) = self.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
}
