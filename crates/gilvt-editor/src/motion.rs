//! Moving the caret and extending the selection. Every movement has an `extend` flag: with it the anchor
//! stays and only the caret moves (Shift+arrow); without it the selection collapses to a caret.

use std::ops::Range;

use ropey::Rope;

use crate::buffer::Buffer;
use crate::position::{self, Selection};

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Word,
    Space,
    Other,
    /// A combining mark, joiner or variation selector: part of whatever precedes it.
    Mark,
}

fn class(c: char) -> CharClass {
    if matches!(
        c,
        '\u{300}'..='\u{36f}' | '\u{1ab0}'..='\u{1aff}' | '\u{1dc0}'..='\u{1dff}' | '\u{20d0}'..='\u{20ff}' | '\u{fe00}'..='\u{fe0f}' | '\u{fe20}'..='\u{fe2f}' | '\u{200d}'
    ) {
        CharClass::Mark
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else if c.is_whitespace() {
        CharClass::Space
    } else {
        CharClass::Other
    }
}

/// Where a word-wise move to the left from `idx` stops: skip spaces (line breaks included), then one run of
/// word characters or of punctuation.
pub(crate) fn prev_word_start(rope: &Rope, idx: usize) -> usize {
    let mut i = idx.min(rope.len_chars());
    while i > 0 && matches!(class(rope.char(i - 1)), CharClass::Space) {
        i -= 1;
    }
    while i > 0 && class(rope.char(i - 1)) == CharClass::Mark {
        i -= 1;
    }
    if i > 0 {
        let c = class(rope.char(i - 1));
        while i > 0 && matches!(class(rope.char(i - 1)), k if k == c || k == CharClass::Mark) {
            i -= 1;
        }
    }
    snap_back(rope, i)
}

/// Where a word-wise move to the right from `idx` stops (the mirror of `prev_word_start`).
pub(crate) fn next_word_end(rope: &Rope, idx: usize) -> usize {
    let len = rope.len_chars();
    let mut i = idx.min(len);
    while i < len && class(rope.char(i)) == CharClass::Space {
        i += 1;
    }
    if i < len {
        let c = class(rope.char(i));
        while i < len && matches!(class(rope.char(i)), k if k == c || k == CharClass::Mark) {
            i += 1;
        }
    }
    snap_forward(rope, i)
}

/// `idx` moved back to the start of the grapheme it is inside, if it is inside one.
fn snap_back(rope: &Rope, idx: usize) -> usize {
    if idx >= rope.len_chars() {
        return rope.len_chars();
    }
    position::prev_boundary(rope, position::next_boundary(rope, idx))
}

/// `idx` moved on to the end of the grapheme it is inside, if it is inside one.
fn snap_forward(rope: &Rope, idx: usize) -> usize {
    let at = snap_back(rope, idx);
    if at == idx {
        idx
    } else {
        position::next_boundary(rope, at)
    }
}

impl Buffer {
    /// The run of same-kind characters around char offset `idx`, for double-click selection: a word (letters,
    /// digits, `_`), a run of punctuation, or a run of whitespace, never across a line break. A combining
    /// mark belongs to the character before it. At the end of a line it gives the run before the break; on an
    /// empty line, and at or past the end of the document, an empty range.
    pub fn word_range_at(&self, idx: usize) -> Range<usize> {
        let rope = &self.rope;
        let len = rope.len_chars();
        if idx >= len {
            return len..len;
        }
        let mut i = idx;
        if rope.char(i) == '\n' {
            if i > 0 && rope.char(i - 1) != '\n' {
                i -= 1;
            } else {
                return idx..idx;
            }
        }
        while i > 0 && class(rope.char(i)) == CharClass::Mark && rope.char(i - 1) != '\n' {
            i -= 1;
        }
        let kind = match class(rope.char(i)) {
            CharClass::Mark => CharClass::Other,
            k => k,
        };
        let joins = |c: char| c != '\n' && matches!(class(c), k if k == kind || k == CharClass::Mark);
        let mut start = i;
        while start > 0 && joins(rope.char(start - 1)) {
            start -= 1;
        }
        let mut end = i + 1;
        while end < len && joins(rope.char(end)) {
            end += 1;
        }
        snap_back(rope, start)..snap_forward(rope, end)
    }

    fn move_head(&mut self, head: usize, extend: bool) {
        let anchor = if extend { self.selection.anchor } else { head };
        self.selection = Selection { anchor, head };
        self.goal_col = None;
    }

    /// Selects the whole document.
    pub fn select_all(&mut self) {
        self.selection = Selection { anchor: 0, head: self.len_chars() };
        self.goal_col = None;
    }

    /// One grapheme left; a selection without `extend` collapses to its start.
    pub fn move_left(&mut self, extend: bool) {
        let sel = self.selection;
        if !extend && !sel.is_empty() {
            return self.move_head(sel.start(), false);
        }
        let head = position::prev_boundary(&self.rope, sel.head);
        self.move_head(head, extend);
    }

    /// One grapheme right; a selection without `extend` collapses to its end.
    pub fn move_right(&mut self, extend: bool) {
        let sel = self.selection;
        if !extend && !sel.is_empty() {
            return self.move_head(sel.end(), false);
        }
        let head = position::next_boundary(&self.rope, sel.head);
        self.move_head(head, extend);
    }

    /// To the start of the previous word (see `prev_word_start`).
    pub fn move_word_left(&mut self, extend: bool) {
        let head = prev_word_start(&self.rope, self.selection.head);
        self.move_head(head, extend);
    }

    /// To the end of the next word (see `next_word_end`).
    pub fn move_word_right(&mut self, extend: bool) {
        let head = next_word_end(&self.rope, self.selection.head);
        self.move_head(head, extend);
    }

    /// To the start of the caret's line.
    pub fn move_line_start(&mut self, extend: bool) {
        let line = self.rope.char_to_line(self.selection.head);
        let head = self.rope.line_to_char(line);
        self.move_head(head, extend);
    }

    /// To the end of the caret's line content (before the `\n`).
    pub fn move_line_end(&mut self, extend: bool) {
        let line = self.rope.char_to_line(self.selection.head);
        let head = self.rope.line_to_char(line) + position::line_text(&self.rope, line).chars().count();
        self.move_head(head, extend);
    }

    /// To the start of the document.
    pub fn move_doc_start(&mut self, extend: bool) {
        self.move_head(0, extend);
    }

    /// To the end of the document.
    pub fn move_doc_end(&mut self, extend: bool) {
        let end = self.len_chars();
        self.move_head(end, extend);
    }

    /// Up one line, keeping the display column it started from; on the first line, to the start of the document.
    pub fn move_up(&mut self, extend: bool) {
        self.move_vertically(-1, extend);
    }

    /// Down one line; on the last line, to the end of the document.
    pub fn move_down(&mut self, extend: bool) {
        self.move_vertically(1, extend);
    }

    fn move_vertically(&mut self, delta: isize, extend: bool) {
        let head = self.selection.head;
        let pos = position::char_to_position(&self.rope, head);
        let goal = self.goal_col.unwrap_or_else(|| position::display_col(&self.rope, pos, self.tab_width));
        let target = pos.line as isize + delta;
        let new_head = if target < 0 {
            0
        } else if target as usize >= self.line_count() {
            self.len_chars()
        } else {
            position::char_at_display_col(&self.rope, target as usize, goal, self.tab_width)
        };
        self.move_head(new_head, extend);
        self.goal_col = Some(goal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(b: &mut Buffer, idx: usize) {
        b.set_selection(Selection::caret(idx));
    }

    #[test]
    fn left_and_right_step_over_whole_graphemes_and_line_breaks() {
        let mut b = Buffer::from_text("e\u{301}x\ny");
        b.move_right(false);
        assert_eq!(b.selection().head, 2, "e + accent is one stop");
        b.move_right(false);
        b.move_right(false);
        assert_eq!(b.selection().head, 4, "x, then across the line break");
        b.move_left(false);
        b.move_left(false);
        assert_eq!(b.selection().head, 2);
        at(&mut b, 0);
        b.move_left(false);
        assert_eq!(b.selection().head, 0, "stops at the start");
        b.move_doc_end(false);
        b.move_right(false);
        assert_eq!(b.selection().head, b.len_chars());
    }

    #[test]
    fn extending_keeps_the_anchor_and_collapsing_goes_to_the_edge() {
        let mut b = Buffer::from_text("abcdef");
        at(&mut b, 2);
        b.move_right(true);
        b.move_right(true);
        assert_eq!(b.selection(), Selection { anchor: 2, head: 4 });
        b.move_left(true);
        assert_eq!(b.selection(), Selection { anchor: 2, head: 3 });
        b.move_right(false);
        assert_eq!(b.selection(), Selection::caret(3), "plain → from the end of the selection");
        b.set_selection(Selection { anchor: 4, head: 1 });
        b.move_left(false);
        assert_eq!(b.selection(), Selection::caret(1), "plain left → the start of the selection");
        b.select_all();
        assert_eq!(b.selection(), Selection { anchor: 0, head: 6 });
    }

    #[test]
    fn vertical_movement_keeps_the_display_column() {
        // Line 0 has a wide character; lines of different length follow.
        let mut b = Buffer::from_text("世界ab\nx\nabcdefg");
        at(&mut b, 3); // after 世界a: display column 5
        b.move_down(false);
        assert_eq!(b.selection().head, 6, "line 1 is only 'x': end of it");
        b.move_down(false);
        assert_eq!(b.selection().head, 12, "line 2 starts at 7: back to display column 5");
        b.move_up(false);
        b.move_up(false);
        assert_eq!(b.selection().head, 3, "and round trip");
        b.move_up(false);
        assert_eq!(b.selection().head, 0, "up on the first line → document start");
        b.move_doc_end(false);
        b.move_down(false);
        assert_eq!(b.selection().head, b.len_chars());
    }

    #[test]
    fn a_horizontal_move_forgets_the_remembered_column() {
        let mut b = Buffer::from_text("abcdef\nxy\nabcdef");
        at(&mut b, 5);
        b.move_down(false); // clamped to the end of "xy"
        b.move_left(false);
        b.move_down(false);
        assert_eq!(b.selection().head, 11, "the left move made column 1 the new goal, not the old 5");
    }

    #[test]
    fn home_end_and_document_ends() {
        let mut b = Buffer::from_text("ab\ncd");
        at(&mut b, 4);
        b.move_line_start(false);
        assert_eq!(b.selection().head, 3);
        b.move_line_end(true);
        assert_eq!(b.selection(), Selection { anchor: 3, head: 5 });
        b.move_doc_start(false);
        assert_eq!(b.selection().head, 0);
        b.move_line_end(false);
        assert_eq!(b.selection().head, 2, "before the line break, not after it");
    }

    #[test]
    fn words_are_runs_of_word_characters_or_punctuation() {
        let b = Buffer::from_text("foo_bar  baz.qux(1)\n  next");
        let r = &b.rope;
        let stops: Vec<usize> = std::iter::successors(Some(0), |&i| {
            let n = next_word_end(r, i);
            (n != i).then_some(n)
        })
        .collect();
        // foo_bar | baz | . | qux | ( | 1 | ) then across the break to "next".
        assert_eq!(stops, [0, 7, 12, 13, 16, 17, 18, 19, 26]);
        let back: Vec<usize> = std::iter::successors(Some(26), |&i| {
            let n = prev_word_start(r, i);
            (n != i).then_some(n)
        })
        .collect();
        assert_eq!(back, [26, 22, 18, 17, 16, 13, 12, 9, 0]);
    }

    #[test]
    fn word_moves_never_stop_inside_a_grapheme() {
        // Combining marks and ZWJ sequences belong to the word before them.
        let b = Buffer::from_text("ae\u{301}b 👨\u{200d}👩 cd");
        assert_eq!(next_word_end(&b.rope, 0), 4, "a, é (2 chars) and b are one word");
        assert_eq!(prev_word_start(&b.rope, 4), 0);
        assert_eq!(next_word_end(&b.rope, 4), 8, "then the 3-char emoji sequence");
        assert_eq!(prev_word_start(&b.rope, 8), 5);
        // Starting in the middle of a cluster still lands on cluster edges.
        let c = Buffer::from_text("x\u{301}");
        assert_eq!(next_word_end(&c.rope, 1), 2);
        assert_eq!(prev_word_start(&c.rope, 1), 0);
    }
}
