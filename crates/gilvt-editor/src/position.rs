//! Positions, selections and grapheme-aware navigation over a `Rope`.
//!
//! The buffer stores text with `\n` line breaks only (see `encoding`), so a line is whatever lies between
//! two `\n`. Offsets are **char indices** into the rope; `Position` is what a person sees: a line and a
//! column counted in grapheme clusters, so a cursor never lands inside `e` + combining accent or inside a
//! family emoji.

use std::ops::Range;

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// A line and a column, both 0-based; the column counts grapheme clusters of the line's content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
    pub line: usize,
    pub col: usize,
}

/// A selection in char offsets. `head` is the caret; `anchor` is where the selection started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn caret(at: usize) -> Selection {
        Selection { anchor: at, head: at }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn range(&self) -> Range<usize> {
        self.start()..self.end()
    }
}

/// The content of `line` without its `\n`. `line` must be below `rope.len_lines()`.
pub(crate) fn line_text(rope: &Rope, line: usize) -> String {
    let mut s = rope.line(line).to_string();
    if s.ends_with('\n') {
        s.pop();
    }
    s
}

/// The line's first char offset and the char offsets of every grapheme boundary of its content, the start
/// and the end of the content included.
fn boundaries(rope: &Rope, line: usize) -> (usize, Vec<usize>) {
    let start = rope.line_to_char(line);
    let mut out = vec![start];
    let mut at = start;
    for g in line_text(rope, line).graphemes(true) {
        at += g.chars().count();
        out.push(at);
    }
    (start, out)
}

/// The next place the caret can stop after `idx`: the next grapheme boundary, or the start of the next
/// line when `idx` is at the end of a line's content. `rope.len_chars()` at the end of the document.
pub(crate) fn next_boundary(rope: &Rope, idx: usize) -> usize {
    let len = rope.len_chars();
    if idx >= len {
        return len;
    }
    let (_, b) = boundaries(rope, rope.char_to_line(idx));
    if idx >= *b.last().unwrap() {
        return idx + 1;
    }
    *b.iter().find(|&&x| x > idx).unwrap()
}

/// The previous place the caret can stop before `idx`; 0 at the start of the document.
pub(crate) fn prev_boundary(rope: &Rope, idx: usize) -> usize {
    if idx == 0 {
        return 0;
    }
    let idx = idx.min(rope.len_chars());
    let (start, b) = boundaries(rope, rope.char_to_line(idx));
    if idx == start {
        return idx - 1;
    }
    *b.iter().rev().find(|&&x| x < idx).unwrap()
}

/// The `Position` of char offset `idx` (clamped to the document).
pub(crate) fn char_to_position(rope: &Rope, idx: usize) -> Position {
    let idx = idx.min(rope.len_chars());
    let line = rope.char_to_line(idx);
    let (_, b) = boundaries(rope, line);
    Position { line, col: b.iter().take_while(|&&x| x <= idx).count() - 1 }
}

/// The char offset of `pos`; a line or column past the end is clamped to the nearest valid place.
pub(crate) fn position_to_char(rope: &Rope, pos: Position) -> usize {
    let line = pos.line.min(rope.len_lines() - 1);
    let (_, b) = boundaries(rope, line);
    b[pos.col.min(b.len() - 1)]
}

/// Terminal cells taken by one grapheme cluster that starts at display column `col`.
/// A tab advances to the next multiple of `tab_width`.
pub fn cell_width(g: &str, col: usize, tab_width: usize) -> usize {
    if g == "\t" {
        let t = tab_width.max(1);
        t - col % t
    } else {
        UnicodeWidthStr::width(g)
    }
}

/// How many terminal-style columns the first `pos.col` graphemes of the line take: tabs advance to the next
/// multiple of `tab_width`, East Asian wide characters take 2.
pub(crate) fn display_col(rope: &Rope, pos: Position, tab_width: usize) -> usize {
    let line = pos.line.min(rope.len_lines() - 1);
    let mut col = 0;
    for g in line_text(rope, line).graphemes(true).take(pos.col) {
        col += cell_width(g, col, tab_width);
    }
    col
}

/// The char offset in `line` of the last grapheme boundary that starts at or before display column
/// `target`: how the caret keeps its visual column when it moves between lines of different content.
pub(crate) fn char_at_display_col(rope: &Rope, line: usize, target: usize, tab_width: usize) -> usize {
    let line = line.min(rope.len_lines() - 1);
    let mut at = rope.line_to_char(line);
    let mut col = 0;
    for g in line_text(rope, line).graphemes(true) {
        let w = cell_width(g, col, tab_width);
        if col + w > target {
            break;
        }
        col += w;
        at += g.chars().count();
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(s: &str) -> Rope {
        Rope::from_str(s)
    }

    #[test]
    fn combining_marks_and_zwj_emoji_are_single_stops() {
        // "e" + combining acute is 2 chars, one grapheme; the family emoji is 5 chars, one grapheme.
        let r = rope("e\u{301}x👨\u{200d}👩\u{200d}👧y");
        assert_eq!(next_boundary(&r, 0), 2);
        assert_eq!(next_boundary(&r, 2), 3);
        assert_eq!(next_boundary(&r, 3), 8);
        assert_eq!(prev_boundary(&r, 8), 3);
        assert_eq!(prev_boundary(&r, 2), 0);
        assert_eq!(char_to_position(&r, 8), Position { line: 0, col: 3 });
        assert_eq!(position_to_char(&r, Position { line: 0, col: 3 }), 8);
    }

    #[test]
    fn boundaries_cross_line_breaks_and_stop_at_the_document_ends() {
        let r = rope("ab\ncd\n");
        assert_eq!(next_boundary(&r, 2), 3, "end of 'ab' → start of 'cd'");
        assert_eq!(prev_boundary(&r, 3), 2, "start of 'cd' → end of 'ab'");
        assert_eq!(next_boundary(&r, 6), 6);
        assert_eq!(prev_boundary(&r, 0), 0);
        // "a\n" has an empty second line at offset 2.
        assert_eq!(char_to_position(&rope("a\n"), 2), Position { line: 1, col: 0 });
    }

    #[test]
    fn positions_round_trip_and_clamp() {
        let r = rope("héllo\n世界\n\nx");
        for idx in 0..=r.len_chars() {
            assert_eq!(position_to_char(&r, char_to_position(&r, idx)), idx, "idx {idx}");
        }
        assert_eq!(position_to_char(&r, Position { line: 1, col: 99 }), 8, "column clamps to the line end");
        assert_eq!(position_to_char(&r, Position { line: 99, col: 0 }), r.len_chars() - 1, "line clamps to the last");
    }

    #[test]
    fn cell_width_matches_display_col() {
        assert_eq!(cell_width("a", 0, 4), 1);
        assert_eq!(cell_width("你", 0, 4), 2);
        assert_eq!(cell_width("e\u{301}", 0, 4), 1);
        assert_eq!(cell_width("\t", 1, 4), 3);
        assert_eq!(cell_width("\t", 4, 4), 4);
    }

    #[test]
    fn display_columns_follow_tabs_and_wide_characters() {
        let r = rope("a\tb世界c");
        let p = |col| Position { line: 0, col };
        assert_eq!(display_col(&r, p(1), 4), 1);
        assert_eq!(display_col(&r, p(2), 4), 4, "tab advances to the next multiple of 4");
        assert_eq!(display_col(&r, p(4), 4), 7, "世 is 2 wide");
        assert_eq!(display_col(&r, p(5), 4), 9);
        assert_eq!(display_col(&r, p(6), 4), 10);
        // From a display column back to a caret: never inside a wide character.
        assert_eq!(char_at_display_col(&r, 0, 6, 4), 3, "column 6 falls inside 世 → before it");
        assert_eq!(char_at_display_col(&r, 0, 7, 4), 4, "column 7 is the boundary after 世");
        assert_eq!(char_at_display_col(&r, 0, 99, 4), 6, "past the end → line end");
    }
}
