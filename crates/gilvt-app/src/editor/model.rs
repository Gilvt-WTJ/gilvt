//! Editing model: a `gilvt_editor::Buffer` plus the soft-wrap map, scroll position and goal column.
//! All keyboard and mouse commands live here so they can be unit-tested without gpui.

use super::indent::{self, Unit};
use super::wrap::{Row, WrapMap, MIN_WIDTH};
use gilvt_editor::{cell_width, Buffer, Change, EditorError, Position, Selection};
use gilvt_viewer::{highlight::language_name, Highlighter, TokenClass};
use std::path::Path;
use unicode_segmentation::UnicodeSegmentation;

pub const HIGHLIGHT_MAX_BYTES: usize = 2 * 1024 * 1024;
pub const HIGHLIGHT_MAX_LINES: usize = 50_000;
const PLAIN_TEXT: &str = "Plain Text";
const LARGE_FILE: &str = "Large file";

/// Extension, else the whole file name (`Makefile`, `Dockerfile`).
fn syntax_token(file_name: &str) -> &str {
    Path::new(file_name).extension().and_then(|e| e.to_str()).unwrap_or(file_name)
}

/// "Bourne Again Shell (bash)" is too long for the status bar.
fn display_language(name: &str) -> String {
    if name.starts_with("Bourne Again Shell") {
        "Shell".into()
    } else {
        name.into()
    }
}

fn localized_language(name: &str) -> &str {
    if name == PLAIN_TEXT {
        crate::i18n::text("纯文本", PLAIN_TEXT)
    } else {
        name
    }
}

fn make_highlighter(
    buf: &Buffer,
    file_name: &str,
) -> (Option<Highlighter>, String, Option<&'static str>) {
    let token = syntax_token(file_name);
    let first = buf.line(0);
    if buf.len_bytes() > HIGHLIGHT_MAX_BYTES || buf.line_count() > HIGHLIGHT_MAX_LINES {
        return (
            None,
            display_language(&language_name(token, &first)),
            Some(LARGE_FILE),
        );
    }
    match Highlighter::new(token, &first) {
        Some(mut h) => {
            h.set_line_count(buf.line_count());
            let name = display_language(h.language());
            (Some(h), name, None)
        }
        None => (None, PLAIN_TEXT.into(), Some(PLAIN_TEXT)),
    }
}

/// A line longer than this makes the buffer read-only: per-keystroke grapheme scans on it are too slow (E4).
pub const LONG_LINE_CHARS: usize = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HMove {
    Left,
    Right,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
}

pub struct EditorModel {
    pub buf: Buffer,
    wrap: WrapMap,
    scroll_row: usize,
    view_rows: usize,
    /// Visual column kept while moving vertically; any other command clears it.
    goal_x: Option<usize>,
    unit: Unit,
    long_line: bool,
    hl: Option<Highlighter>,
    /// Display name of the language, kept also when the big-file guard turned highlighting off.
    hl_language: String,
    hl_off: Option<&'static str>,
}

impl EditorModel {
    pub fn new(buf: Buffer, file_name: &str, cols: usize, rows: usize) -> Self {
        let lines = buf.line_count();
        let unit = indent::detect_unit((0..lines.min(500)).map(|l| buf.line(l)).collect::<Vec<_>>().iter().map(String::as_str), file_name);
        let long_line = (0..lines).any(|l| buf.line_len_chars(l) > LONG_LINE_CHARS);
        let mut wrap = WrapMap::new(cols.max(MIN_WIDTH), buf.tab_width());
        wrap.rebuild(lines, &|l| buf.line(l));
        let (hl, hl_language, hl_off) = make_highlighter(&buf, file_name);
        Self { buf, wrap, scroll_row: 0, view_rows: rows.max(1), goal_x: None, unit, long_line, hl, hl_language, hl_off }
    }

    /// Language name for the status bar: "Rust", "Shell", "纯文本"; "（未高亮）" is appended when the big-file guard is on.
    pub fn language_label(&self) -> String {
        let language = localized_language(&self.hl_language);
        if self.hl.is_some() || self.hl_off == Some(PLAIN_TEXT) {
            language.into()
        } else {
            if crate::i18n::current() == crate::i18n::Language::English {
                format!("{language} (not highlighted)")
            } else {
                format!("{language}（未高亮）")
            }
        }
    }

    /// The display language without any suffix.
    pub fn language_name(&self) -> String {
        localized_language(&self.hl_language).into()
    }

    pub fn highlight_enabled(&self) -> bool {
        self.hl.is_some()
    }

    pub fn highlight_off_reason(&self) -> Option<&'static str> {
        match self.hl_off {
            Some(PLAIN_TEXT) => Some(crate::i18n::text("纯文本", PLAIN_TEXT)),
            Some(LARGE_FILE) => Some(crate::i18n::text("文件较大", LARGE_FILE)),
            reason => reason,
        }
    }

    /// One class per grapheme of `row_text(row)` (a tab's expanded spaces take the tab's class). Empty (meaning no
    /// class anywhere) without a highlighter or when the line has no spans, e.g. a line too long to parse.
    pub fn row_classes(&mut self, row: usize) -> Vec<Option<TokenClass>> {
        let Some(h) = self.hl.as_mut() else { return Vec::new() };
        let r = self.wrap.row(row);
        let buf = &self.buf;
        let spans = h.spans(r.line, &|l| buf.line(l));
        if spans.is_empty() {
            return Vec::new();
        }
        let line = buf.line(r.line);
        let tab = buf.tab_width();
        let (mut out, mut byte, mut col, mut si) = (Vec::new(), 0, r.indent, 0);
        for (gi, g) in line.graphemes(true).enumerate() {
            if gi >= r.end {
                break;
            }
            if gi >= r.start {
                let w = cell_width(g, col, tab);
                // Spans are sorted and disjoint: skip the ones that end before this grapheme.
                while spans.get(si).is_some_and(|s| s.range.end <= byte) {
                    si += 1;
                }
                let class = spans.get(si).filter(|s| s.range.start <= byte).map(|s| s.class);
                out.extend(std::iter::repeat_n(class, if g == "\t" { w } else { 1 }));
                col += w;
            }
            byte += g.len();
        }
        out
    }

    pub fn is_read_only(&self) -> bool {
        self.buf.read_only() || self.long_line
    }

    pub fn read_only_reason(&self) -> Option<&'static str> {
        if self.long_line {
            Some(crate::i18n::text("含超长行，暂不支持编辑", "Contains an extremely long line; editing is not supported"))
        } else if self.buf.read_only() {
            Some(crate::i18n::text("只读", "Read only"))
        } else {
            None
        }
    }

    pub(super) fn unit(&self) -> &Unit {
        &self.unit
    }

    // ---- layout ----

    pub fn set_viewport(&mut self, cols: usize, rows: usize) {
        let was_visible = self.caret_row() >= self.scroll_row && self.caret_row() < self.scroll_row + self.view_rows;
        self.view_rows = rows.max(1);
        let cols = cols.max(MIN_WIDTH);
        if cols != self.wrap.width() {
            let anchor = self.wrap.row(self.scroll_row);
            let buf = &self.buf;
            self.wrap.set_width(cols, buf.line_count(), &|l| buf.line(l));
            // Keep the top line in place even when the caret is scrolled out of view.
            self.scroll_row = self.wrap.row_of(anchor.line, anchor.start);
        }
        self.clamp_scroll();
        if was_visible {
            self.ensure_cursor_visible();
        }
    }

    fn clamp_scroll(&mut self) {
        self.scroll_row = self.scroll_row.min(self.max_scroll());
    }

    fn max_scroll(&self) -> usize {
        self.wrap.total_rows().saturating_sub(self.view_rows)
    }

    pub fn scroll_row(&self) -> usize {
        self.scroll_row
    }
    pub fn total_rows(&self) -> usize {
        self.wrap.total_rows()
    }
    /// Wrap width in text columns.
    pub fn viewport_cols(&self) -> usize {
        self.wrap.width()
    }

    /// Puts the first visual row of logical `line` at the top (clamped).
    pub fn scroll_to_line(&mut self, line: usize) {
        let line = line.min(self.buf.line_count().saturating_sub(1));
        self.scroll_row = self.wrap.row_of(line, 0);
        self.clamp_scroll();
    }

    pub fn view_rows(&self) -> usize {
        self.view_rows
    }

    pub fn scroll_by(&mut self, delta: i64) {
        self.scroll_row = (self.scroll_row as i64 + delta).clamp(0, self.max_scroll() as i64) as usize;
    }

    /// The 1-based logical line of the first shown row.
    pub fn top_line(&self) -> usize {
        self.wrap.row(self.scroll_row).line + 1
    }

    pub fn row(&self, i: usize) -> Row {
        self.wrap.row(i)
    }

    pub fn is_first_row(&self, i: usize) -> bool {
        self.wrap.row(i).start == 0
    }

    /// Text of display row `i` with tabs expanded to spaces so it can be shaped on the cell grid.
    pub fn row_text(&self, i: usize) -> String {
        let r = self.wrap.row(i);
        let line = self.buf.line(r.line);
        let mut out = String::new();
        let mut col = r.indent;
        for g in line.graphemes(true).skip(r.start).take(r.end - r.start) {
            if g == "\t" {
                let w = cell_width(g, col, self.buf.tab_width());
                out.extend(std::iter::repeat_n(' ', w));
                col += w;
            } else {
                out.push_str(g);
                col += cell_width(g, col, self.buf.tab_width());
            }
        }
        out
    }

    // ---- caret ----

    pub fn caret_position(&self) -> Position {
        self.buf.position(self.buf.selection().head)
    }

    pub fn caret_row(&self) -> usize {
        let p = self.caret_position();
        self.wrap.row_of(p.line, p.col)
    }

    pub fn caret_visual_col(&self) -> usize {
        let p = self.caret_position();
        let r = self.wrap.row_of(p.line, p.col);
        self.wrap.visual_col(r, &self.buf.line(p.line), p.col)
    }

    pub fn ensure_cursor_visible(&mut self) {
        let row = self.caret_row();
        if row < self.scroll_row {
            self.scroll_row = row;
        } else if row >= self.scroll_row + self.view_rows {
            self.scroll_row = row + 1 - self.view_rows;
        }
        self.clamp_scroll();
    }

    /// Visual columns `[start, end)` of the selection on display row `row`; a selected line break shows as one extra cell.
    pub fn selection_span(&self, row: usize) -> Option<(usize, usize)> {
        let sel = self.buf.selection();
        if sel.is_empty() {
            return None;
        }
        let (ps, pe) = (self.buf.position(sel.start()), self.buf.position(sel.end()));
        let r = self.wrap.row(row);
        if r.line < ps.line || r.line > pe.line {
            return None;
        }
        let from = if r.line == ps.line { ps.col.max(r.start) } else { r.start };
        let to = if r.line == pe.line { pe.col.min(r.end) } else { r.end };
        let eol = r.line < pe.line && self.wrap.is_last_row(row);
        if from > to || (from == to && !eol) {
            return None;
        }
        let text = self.buf.line(r.line);
        let a = self.wrap.visual_col(row, &text, from);
        let b = self.wrap.visual_col(row, &text, to);
        Some((a, b + usize::from(eol)))
    }

    // ---- movement ----

    fn set_head(&mut self, head: usize, extend: bool) {
        let anchor = if extend { self.buf.selection().anchor } else { head };
        self.buf.set_selection(Selection { anchor, head });
    }

    fn moved(&mut self) {
        self.goal_x = None;
        self.ensure_cursor_visible();
    }

    pub fn move_h(&mut self, kind: HMove, extend: bool) {
        match kind {
            HMove::Left => self.buf.move_left(extend),
            HMove::Right => self.buf.move_right(extend),
            HMove::WordLeft => self.buf.move_word_left(extend),
            HMove::WordRight => self.buf.move_word_right(extend),
            HMove::LineStart => self.buf.move_line_start(extend),
            HMove::LineEnd => self.buf.move_line_end(extend),
            HMove::DocStart => self.buf.move_doc_start(extend),
            HMove::DocEnd => self.buf.move_doc_end(extend),
        }
        self.moved();
    }

    /// Puts a caret at char offset `idx` (clamped) and scrolls it into view.
    pub fn set_caret(&mut self, idx: usize) {
        let idx = idx.min(self.buf.len_chars());
        self.buf.set_selection(Selection::caret(idx));
        self.moved();
    }

    pub fn select_all(&mut self) {
        self.buf.select_all();
        self.moved();
    }

    /// Moves the caret `delta` display rows, keeping the visual column. Past either end it goes to the document's start / end.
    pub fn move_rows(&mut self, delta: i64, extend: bool) {
        let x = self.goal_x.unwrap_or_else(|| self.caret_visual_col());
        let target = self.caret_row() as i64 + delta;
        if target < 0 {
            self.set_head(0, extend);
            self.moved();
            return;
        }
        if target as usize >= self.wrap.total_rows() {
            self.set_head(self.buf.len_chars(), extend);
            self.moved();
            return;
        }
        let r = self.wrap.row(target as usize);
        let col = self.wrap.col_at(target as usize, &self.buf.line(r.line), x as f32);
        let idx = self.buf.char_index(Position { line: r.line, col });
        self.set_head(idx, extend);
        self.ensure_cursor_visible();
        self.goal_x = Some(x);
    }

    /// First press: start of the display row; again (or already there): start of the logical line.
    pub fn home(&mut self, extend: bool) {
        let p = self.caret_position();
        let r = self.wrap.row(self.caret_row());
        let col = if p.col != r.start { r.start } else { 0 };
        let idx = self.buf.char_index(Position { line: p.line, col });
        self.set_head(idx, extend);
        self.moved();
    }

    /// First press: end of the display row; again (or already there): end of the logical line.
    pub fn end(&mut self, extend: bool) {
        let p = self.caret_position();
        let row = self.caret_row();
        let r = self.wrap.row(row);
        let row_end = if self.wrap.is_last_row(row) { r.end } else { r.end.saturating_sub(1).max(r.start) };
        let line_end = self.wrap.row(self.wrap.rows_of_line(p.line).end - 1).end;
        let col = if p.col != row_end { row_end } else { line_end };
        let idx = self.buf.char_index(Position { line: p.line, col });
        self.set_head(idx, extend);
        self.moved();
    }

    // ---- mouse ----

    fn hit(&self, view_row: usize, x: f32) -> usize {
        let row = (self.scroll_row + view_row).min(self.wrap.total_rows() - 1);
        let r = self.wrap.row(row);
        let col = self.wrap.col_at(row, &self.buf.line(r.line), x);
        self.buf.char_index(Position { line: r.line, col })
    }

    pub fn click(&mut self, view_row: usize, x: f32, extend: bool) {
        let idx = self.hit(view_row, x);
        self.set_head(idx, extend);
        self.moved();
    }

    pub fn double_click(&mut self, view_row: usize, x: f32) {
        let idx = self.hit(view_row, x);
        let range = self.buf.word_range_at(idx);
        self.buf.set_selection(Selection { anchor: range.start, head: range.end });
        self.moved();
    }

    /// Selects the whole logical line (with its line break); also what a click in the gutter does.
    pub fn triple_click(&mut self, view_row: usize) {
        let row = (self.scroll_row + view_row).min(self.wrap.total_rows() - 1);
        let line = self.wrap.row(row).line;
        let start = self.buf.char_index(Position { line, col: 0 });
        let mut end = start + self.buf.line_len_chars(line);
        if line + 1 < self.buf.line_count() {
            end += 1;
        }
        self.buf.set_selection(Selection { anchor: start, head: end });
        self.moved();
    }

    pub fn drag_to(&mut self, view_row: usize, x: f32) {
        let idx = self.hit(view_row, x);
        self.set_head(idx, true);
        self.moved();
    }

    // ---- shared with Task 5 ----

    /// Re-wraps after `change` and keeps the caret visible.
    pub(super) fn sync(&mut self, change: Change) {
        let buf = &self.buf;
        self.wrap.apply(change, buf.line_count(), &|l| buf.line(l));
        if let Some(h) = self.hl.as_mut() {
            h.invalidate_from(change.start_line);
            h.set_line_count(buf.line_count());
        }
        // A paste can create an over-long line: only the touched lines can have grown. While the flag is set an
        // undo may have removed the line, so then (rare path) recompute it over the whole document.
        let end = (change.start_line + change.new_lines).min(buf.line_count());
        if self.long_line {
            self.long_line = (0..buf.line_count()).any(|l| buf.line_len_chars(l) > LONG_LINE_CHARS);
        } else if (change.start_line..end).any(|l| buf.line_len_chars(l) > LONG_LINE_CHARS) {
            self.long_line = true;
        }
        self.goal_x = None;
        self.clamp_scroll();
        self.ensure_cursor_visible();
    }

    pub(super) fn rebuild_wrap(&mut self) {
        let buf = &self.buf;
        self.wrap.rebuild(buf.line_count(), &|l| buf.line(l));
        if let Some(h) = self.hl.as_mut() {
            h.invalidate_from(0);
            h.set_line_count(buf.line_count());
        }
        self.long_line = (0..buf.line_count()).any(|l| buf.line_len_chars(l) > LONG_LINE_CHARS);
        self.goal_x = None;
        self.clamp_scroll();
        self.ensure_cursor_visible();
    }

    pub(super) fn guard(&self) -> Result<(), EditorError> {
        if self.is_read_only() { Err(EditorError::ReadOnly) } else { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(text: &str, cols: usize, rows: usize) -> EditorModel {
        EditorModel::new(Buffer::from_text(text), "t.md", cols, rows)
    }
    #[test]
    fn top_line_is_one_based_and_follows_scroll() {
        let mut m = model("a\nb\nc\nd\ne\n", 20, 2);
        assert_eq!(m.top_line(), 1);
        m.scroll_by(2);
        assert_eq!(m.top_line(), 3);
    }
    fn caret(m: &EditorModel) -> (usize, usize) {
        let p = m.caret_position();
        (p.line, p.col)
    }
    fn goto(m: &mut EditorModel, line: usize, col: usize) {
        let i = m.buf.char_index(Position { line, col });
        m.buf.set_selection(Selection { anchor: i, head: i });
    }

    // 12-col view: "aaaa bbbb cccc dddd" wraps into rows [0,10) [10,19)
    const WRAPPED: &str = "aaaa bbbb cccc dddd\nshort\nxxxxxxxxxxxxxxxxxxxxxxxxxx";

    #[test]
    fn rows_follow_the_wrap() {
        let m = model(WRAPPED, 12, 10);
        assert_eq!(m.total_rows(), 2 + 1 + 3);
        assert_eq!(m.row_text(0), "aaaa bbbb ");
        assert!(m.is_first_row(0) && !m.is_first_row(1) && m.is_first_row(2));
    }

    #[test]
    fn down_keeps_the_visual_column_across_a_short_line() {
        let mut m = model("abcdefgh\nab\nabcdefgh", 40, 10);
        goto(&mut m, 0, 6);
        m.move_rows(1, false);
        assert_eq!(caret(&m), (1, 2)); // clamped to the short line
        m.move_rows(1, false);
        assert_eq!(caret(&m), (2, 6)); // the goal column is remembered
    }

    #[test]
    fn vertical_movement_walks_display_rows_not_lines() {
        let mut m = model(WRAPPED, 12, 10);
        goto(&mut m, 0, 3);
        m.move_rows(1, false);
        assert_eq!(caret(&m), (0, 13)); // second display row of line 0, same visual column
        m.move_rows(1, false);
        assert_eq!(caret(&m).0, 1);
    }

    #[test]
    fn up_on_the_first_row_goes_to_the_start_and_down_on_the_last_to_the_end() {
        let mut m = model("abc\ndef", 40, 10);
        goto(&mut m, 0, 2);
        m.move_rows(-1, false);
        assert_eq!(caret(&m), (0, 0));
        m.move_rows(5, false);
        assert_eq!(caret(&m), (1, 3));
    }

    #[test]
    fn home_and_end_are_two_stage_on_a_wrapped_line() {
        let mut m = model(WRAPPED, 12, 10);
        goto(&mut m, 0, 15); // on the second display row
        m.home(false);
        assert_eq!(caret(&m), (0, 10)); // first press: start of the display row
        m.home(false);
        assert_eq!(caret(&m), (0, 0)); // second press: start of the logical line
        goto(&mut m, 0, 2);
        m.end(false);
        assert_eq!(caret(&m), (0, 9)); // end of the first display row (before its trailing space)
        m.end(false);
        assert_eq!(caret(&m), (0, 19)); // end of the logical line
    }

    #[test]
    fn extending_moves_keep_the_anchor() {
        let mut m = model("abc def", 40, 10);
        goto(&mut m, 0, 1);
        m.move_h(HMove::WordRight, true);
        assert_eq!(m.buf.selection().anchor, 1);
        assert!(m.buf.selection().head > 1);
    }

    #[test]
    fn clicking_maps_pixels_to_the_nearest_boundary() {
        let mut m = model("ab你cd", 40, 10);
        m.click(0, 2.9, false);
        assert_eq!(caret(&m), (0, 2));
        m.click(0, 3.1, false);
        assert_eq!(caret(&m), (0, 3));
        m.click(5, 0.0, false); // below the last row: clamps to it
        assert_eq!(caret(&m).0, 0);
    }

    #[test]
    fn double_and_triple_click_select_a_word_and_a_line() {
        let mut m = model("foo bar\nbaz", 40, 10);
        m.double_click(0, 5.0);
        assert_eq!(m.buf.slice(m.buf.selection().range()), "bar");
        m.triple_click(0);
        assert_eq!(m.buf.slice(m.buf.selection().range()), "foo bar\n");
    }

    #[test]
    fn drag_extends_from_the_click_point() {
        let mut m = model("abcdef", 40, 10);
        m.click(0, 1.0, false);
        m.drag_to(0, 4.0);
        assert_eq!(m.buf.slice(m.buf.selection().range()), "bcd");
    }

    #[test]
    fn the_caret_scrolls_into_view_and_resizing_keeps_the_top_line() {
        let text: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let mut m = model(&text, 40, 5);
        m.move_rows(20, false);
        assert!(m.caret_row() >= m.scroll_row() && m.caret_row() < m.scroll_row() + 5);
        m.scroll_by(-100);
        assert_eq!(m.scroll_row(), 0);
        m.scroll_by(1000);
        assert_eq!(m.scroll_row(), m.total_rows() - 5);
        let top_line = m.row(m.scroll_row()).line;
        m.set_viewport(20, 5);
        assert_eq!(m.row(m.scroll_row()).line, top_line);
    }

    #[test]
    fn selection_spans_cross_wrapped_rows() {
        let mut m = model(WRAPPED, 12, 10);
        m.buf.set_selection(Selection { anchor: 8, head: 13 }); // "bb cc"
        assert_eq!(m.selection_span(0), Some((8, 10)));
        assert_eq!(m.selection_span(1), Some((0, 3)));
        assert_eq!(m.selection_span(2), None);
    }

    #[test]
    fn read_only_reasons() {
        let long = "x".repeat(LONG_LINE_CHARS + 1);
        let m = model(&long, 40, 10);
        assert!(m.is_read_only());
        assert_eq!(m.read_only_reason(), Some("含超长行，暂不支持编辑"));
        assert!(!model("ok", 40, 10).is_read_only());
    }

    #[test]
    fn pasting_an_over_long_line_makes_the_model_read_only() {
        let mut m = model("ab\ncd", 40, 10);
        goto(&mut m, 1, 1);
        m.insert(&"x".repeat(LONG_LINE_CHARS + 1)).unwrap();
        assert!(m.is_read_only());
        assert_eq!(m.read_only_reason(), Some("含超长行，暂不支持编辑"));
        assert!(matches!(m.insert("y"), Err(EditorError::ReadOnly)));
    }

    #[test]
    fn undoing_an_over_long_paste_makes_the_model_editable_again() {
        let mut m = model("ab\ncd", 40, 10);
        goto(&mut m, 1, 1);
        m.insert(&"x".repeat(LONG_LINE_CHARS + 1)).unwrap();
        assert!(m.is_read_only());
        assert!(m.undo());
        assert!(!m.is_read_only());
        m.insert("y").unwrap();
    }

    #[test]
    fn set_caret_clamps_and_scrolls_into_view() {
        let text: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let mut m = model(&text, 40, 5);
        let idx = m.buf.char_index(Position { line: 30, col: 2 });
        m.set_caret(idx);
        assert_eq!(caret(&m), (30, 2));
        assert!(in_view(&m));
        m.set_caret(usize::MAX);
        assert_eq!(m.buf.selection().head, m.buf.len_chars());
    }

    fn in_view(m: &EditorModel) -> bool {
        m.caret_row() >= m.scroll_row() && m.caret_row() < m.scroll_row() + m.view_rows()
    }

    #[test]
    fn narrowing_keeps_a_visible_caret_visible() {
        let text: String = (0..30).map(|i| format!("{i:02} aaaa bbbb cccc dddd eeee ff\n")).collect();
        let mut m = model(&text, 40, 6);
        m.buf.set_selection(Selection::caret(m.buf.char_index(Position { line: 25, col: 3 })));
        m.ensure_cursor_visible();
        assert!(in_view(&m));
        m.set_viewport(12, 6);
        assert!(m.total_rows() > 40);
        assert!(in_view(&m));
    }

    #[test]
    fn shrinking_the_height_keeps_the_caret_on_the_last_row_visible() {
        let text: String = (0..30).map(|i| format!("line {i}\n")).collect();
        let mut m = model(&text, 40, 10);
        m.scroll_by(5);
        m.buf.set_selection(Selection::caret(m.buf.char_index(Position { line: 14, col: 0 })));
        assert_eq!(m.caret_row(), m.scroll_row() + 9);
        m.set_viewport(40, 4);
        assert!(in_view(&m));
    }

    #[test]
    fn row_text_expands_tabs_to_cells() {
        let m = model("a\tb", 40, 10);
        assert_eq!(m.row_text(0), "a   b");
    }

    use gilvt_viewer::TokenClass;

    fn rs(text: &str, cols: usize) -> EditorModel {
        EditorModel::new(Buffer::from_text(text), "main.rs", cols, 20)
    }
    fn class_at(m: &mut EditorModel, row: usize, grapheme: usize) -> Option<TokenClass> {
        m.row_classes(row)[grapheme]
    }

    #[test]
    fn language_labels_and_reasons() {
        let m = rs("fn main() {}", 40);
        assert_eq!((m.language_label().as_str(), m.highlight_enabled(), m.highlight_off_reason()), ("Rust", true, None));
        let m = EditorModel::new(Buffer::from_text("hello"), "notes.txt", 40, 10);
        assert_eq!((m.language_label().as_str(), m.highlight_enabled(), m.highlight_off_reason()), ("纯文本", false, Some("纯文本")));
        let m = EditorModel::new(Buffer::from_text("#!/bin/bash\necho hi"), "run", 40, 10);
        assert_eq!(m.language_label(), "Shell");
        let m = EditorModel::new(Buffer::from_text("a = 1"), "Cargo.toml", 40, 10);
        assert_eq!(m.language_label(), "TOML");
        assert!(EditorModel::new(Buffer::from_text(""), "a.rs", 40, 10).row_classes(0).is_empty());
    }

    #[test]
    fn a_big_file_is_not_highlighted_and_says_so() {
        let big = "let x = 1; // c\n".repeat(HIGHLIGHT_MAX_BYTES / 16 + 10);
        let mut m = EditorModel::new(Buffer::from_text(&big), "big.rs", 40, 10);
        assert!(!m.highlight_enabled());
        assert_eq!(m.highlight_off_reason(), Some("文件较大"));
        assert_eq!(m.language_label(), "Rust（未高亮）");
        assert_eq!(m.language_name(), "Rust");
        assert!(m.row_classes(0).is_empty());
        let many_lines = "x\n".repeat(HIGHLIGHT_MAX_LINES + 1);
        let m = EditorModel::new(Buffer::from_text(&many_lines), "many.rs", 40, 10);
        assert_eq!(m.highlight_off_reason(), Some("文件较大"));
        let ok = EditorModel::new(Buffer::from_text(&"x\n".repeat(HIGHLIGHT_MAX_LINES - 1)), "ok.rs", 40, 10);
        assert!(ok.highlight_enabled());
    }

    #[test]
    fn row_classes_line_up_with_row_text_graphemes() {
        // tab, CJK, emoji, a combining accent (e + U+0301) before the keyword-looking part.
        let mut m = rs("\t你好 😀 e\u{301} let x = 1; // c", 80);
        let n = m.row_text(0).graphemes(true).count();
        let classes = m.row_classes(0);
        assert_eq!(classes.len(), n);
        let text = m.row_text(0);
        let gs: Vec<&str> = text.graphemes(true).collect();
        let at = |needle: &str| gs.iter().position(|g| *g == needle).unwrap();
        let l = (0..gs.len() - 2).find(|&i| gs[i] == "l" && gs[i + 1] == "e" && gs[i + 2] == "t").unwrap();
        assert_eq!(classes[l], Some(TokenClass::Keyword));
        assert_eq!(classes[l + 2], Some(TokenClass::Keyword));
        assert_eq!(classes[l + 3], None); // the space after `let`
        assert_eq!(classes[at("1")], Some(TokenClass::Number));
        assert_eq!(classes[gs.len() - 1], Some(TokenClass::Comment));
        assert_eq!(classes[0], None); // the expanded tab
    }

    #[test]
    fn continuation_rows_of_a_wrapped_line_keep_their_classes() {
        // 12 columns: the comment wraps; every row of the comment line must still be Comment.
        let mut m = rs("/* aaaa bbbb cccc dddd eeee */", 12);
        assert!(m.total_rows() >= 3);
        for row in 0..m.total_rows() {
            let c = m.row_classes(row);
            assert!(c.iter().all(|x| *x == Some(TokenClass::Comment)), "row {row}: {c:?}");
        }
    }

    #[test]
    fn a_line_without_spans_gives_an_empty_class_list() {
        let long = format!("let s = \"{}\";", "x".repeat(gilvt_viewer::highlight::MAX_LINE_BYTES));
        let mut m = rs(&format!("{long}\nlet t = 1;"), 80);
        assert!(m.row_classes(0).is_empty(), "too long to parse");
        let last = m.total_rows() - 1;
        assert!(m.row_classes(last).contains(&Some(TokenClass::Keyword)));
    }

    #[test]
    fn a_continuation_row_starting_inside_a_comment_maps_its_own_graphemes() {
        let mut m = rs("let a = 1; /* cccc dd */ let b = 2;", 16);
        let rows: Vec<String> = (0..m.total_rows()).map(|r| m.row_text(r)).collect();
        // The second row starts inside the comment, then has code after it; its first graphemes differ in class
        // from the line's first graphemes (`let a`), so ignoring the row's start offset would fail.
        assert_eq!(rows, ["let a = 1; /* ", "cccc dd */ let ", "b = 2;"]);
        let (cm, k) = (Some(TokenClass::Comment), Some(TokenClass::Keyword));
        let want = [vec![cm; 10], vec![None, k, k, k, None]].concat();
        assert_eq!(m.row_classes(1), want);
    }

    #[test]
    fn typing_an_open_comment_recolors_the_lines_below_and_undo_restores_them() {
        let mut m = rs("fn a() {}\nlet x = 1;\nlet y = 2;", 40);
        let kw = |m: &mut EditorModel| class_at(m, 2, 0);
        assert_eq!(kw(&mut m), Some(TokenClass::Keyword));
        m.set_caret(0);
        m.insert("/*").unwrap();
        assert_eq!(kw(&mut m), Some(TokenClass::Comment));
        assert!(m.undo());
        assert_eq!(kw(&mut m), Some(TokenClass::Keyword));
        assert!(m.redo());
        assert_eq!(kw(&mut m), Some(TokenClass::Comment));
    }

    #[test]
    fn undoing_a_multi_line_paste_invalidates_from_the_pasted_line_only() {
        let text: String = (0..200).map(|i| format!("let v{i} = {i};\n")).collect();
        let mut m = rs(&text, 80);
        let all = |m: &mut EditorModel| (0..m.total_rows()).map(|r| m.row_classes(r)).collect::<Vec<_>>();
        let before = all(&mut m);
        // Paste an unclosed block comment plus two lines at the start of line 150: everything below turns Comment.
        m.set_caret(m.buf.char_index(Position { line: 150, col: 0 }));
        m.insert("/* a\nb\nc ").unwrap();
        assert_eq!(class_at(&mut m, 199, 0), Some(TokenClass::Comment));
        let during = all(&mut m);
        assert!(m.undo());
        // Lines above the paste keep their cached spans; the paste's line and those below recolour.
        let h = m.hl.as_ref().unwrap();
        assert!((0..150).all(|l| h.is_cached(l)), "lines above the undone paste stay cached");
        assert!(!h.is_cached(150) && !h.is_cached(199));
        assert_eq!(all(&mut m), before);
        assert!(m.redo());
        assert!(m.hl.as_ref().unwrap().is_cached(149));
        assert_eq!(all(&mut m), during);
    }

    #[test]
    fn an_unterminated_comment_at_the_end_of_the_file_is_fine() {
        let mut m = rs("let a = 1;\n/* never closed", 40);
        assert_eq!(class_at(&mut m, 1, 0), Some(TokenClass::Comment));
        m.set_caret(m.buf.len_chars());
        m.insert(" */ let b = 2;").unwrap();
        let last = m.row_classes(1);
        assert!(last.contains(&Some(TokenClass::Keyword)), "{last:?}");
    }

    #[test]
    fn replacing_the_whole_text_starts_the_highlighter_over() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.rs");
        std::fs::write(&p, "let a = 1;\n").unwrap();
        let mut m = EditorModel::new(Buffer::open(&p).unwrap(), "a.rs", 40, 10);
        assert_eq!(class_at(&mut m, 0, 0), Some(TokenClass::Keyword));
        std::fs::write(&p, "// now a comment\n").unwrap();
        m.reload().unwrap();
        assert_eq!(class_at(&mut m, 0, 0), Some(TokenClass::Comment));
    }
}
