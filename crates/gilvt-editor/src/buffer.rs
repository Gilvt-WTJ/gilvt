//! The text buffer: a rope, one selection, a revision counter and the undo history.
//!
//! Text inside is always `\n`-separated (`encoding::normalize_newlines`); the file's own line ending and
//! encoding are kept only to write it back. Every change goes through `replace`, which records it for undo.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ropey::Rope;

use crate::encoding::{normalize_newlines, Encoding, LineEnding};
use crate::error::EditorError;
use crate::history::{Edit, EditKind, History};
use crate::position::{self, Position, Selection};

/// What the file looked like on disk when it was loaded or last saved; `Buffer::check_external` compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DiskState {
    pub mtime: SystemTime,
    pub size: u64,
    pub hash: u64,
}

/// Which lines a change touched, so a view can redraw only those: lines `start_line..start_line + old_lines`
/// of the text before the change are now lines `start_line..start_line + new_lines`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub start_line: usize,
    pub old_lines: usize,
    pub new_lines: usize,
}

pub struct Buffer {
    pub(crate) rope: Rope,
    /// E1 keeps exactly one selection; `selections()` is a slice so a multi-cursor extension is not a break.
    pub(crate) selection: Selection,
    pub(crate) revision: u64,
    pub(crate) history: History,
    pub(crate) encoding: Encoding,
    pub(crate) line_ending: LineEnding,
    pub(crate) mixed_line_endings: bool,
    /// Metadata (the line ending to save with) changed since the last load or save; the text may be saved.
    pub(crate) meta_dirty: bool,
    pub(crate) read_only: bool,
    /// The user asked for read-only (or the text is lossy): reload and save-as never lift it.
    pub(crate) forced_read_only: bool,
    /// Some characters were replaced while decoding; the buffer is always read-only.
    pub(crate) lossy: bool,
    /// The encoding the user picked by hand (`None`: detected); `reload` decodes with it again.
    pub(crate) explicit_encoding: Option<Encoding>,
    pub(crate) path: Option<PathBuf>,
    pub(crate) disk: Option<DiskState>,
    /// The display column vertical movement tries to keep; cleared by anything that is not vertical movement.
    pub(crate) goal_col: Option<usize>,
    pub(crate) tab_width: usize,
    clock: Box<dyn Fn() -> u64 + Send>,
}

fn system_clock_ms() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Default for Buffer {
    fn default() -> Self {
        Buffer::from_text("")
    }
}

impl Buffer {
    /// A new, unsaved buffer holding `text` (UTF-8, LF). Any `\r\n` or `\r` in `text` becomes `\n`.
    pub fn from_text(text: &str) -> Buffer {
        Buffer {
            rope: Rope::from_str(&normalize_newlines(text)),
            selection: Selection::caret(0),
            revision: 0,
            history: History::new(),
            encoding: Encoding::Utf8,
            line_ending: LineEnding::Lf,
            mixed_line_endings: false,
            meta_dirty: false,
            read_only: false,
            forced_read_only: false,
            lossy: false,
            explicit_encoding: None,
            path: None,
            disk: None,
            goal_col: None,
            tab_width: 4,
            clock: Box::new(system_clock_ms),
        }
    }

    // ---- reading ----------------------------------------------------------------------------------------

    /// The whole text, `\n`-separated (the file's own line ending is applied only when saving).
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// The length of the text in chars (Unicode scalar values, not graphemes or bytes).
    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    /// The length of the text in UTF-8 bytes (no BOM or encoding overhead).
    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// Lines in the text; a text ending in `\n` has an empty last line, as in every editor.
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Line `index` without its `\n`; an index past the end gives an empty string.
    pub fn line(&self, index: usize) -> String {
        if index < self.line_count() {
            position::line_text(&self.rope, index)
        } else {
            String::new()
        }
    }

    /// The text of a char range (clamped to the document).
    pub fn slice(&self, range: Range<usize>) -> String {
        let end = range.end.min(self.len_chars());
        let start = range.start.min(end);
        self.rope.slice(start..end).to_string()
    }

    /// The line and grapheme column of char offset `char_index` (clamped to the document).
    pub fn position(&self, char_index: usize) -> Position {
        position::char_to_position(&self.rope, char_index)
    }

    /// The char offset of `pos`; a line or column past the end clamps to the nearest valid place.
    pub fn char_index(&self, pos: Position) -> usize {
        position::position_to_char(&self.rope, pos)
    }

    /// The display column of `pos` on its line (what a monospace view draws): tabs advance to the next
    /// multiple of `tab_width()`, East Asian wide characters and emoji take 2, combining marks 0. A column
    /// past the line's end counts the whole line; a line past the end is the last line.
    pub fn display_col(&self, pos: Position) -> usize {
        position::display_col(&self.rope, pos, self.tab_width)
    }

    /// The char offset in `line` of the last grapheme boundary that starts at or before display column `col`
    /// (the inverse of `display_col`, for placing the caret from a mouse click). A column past the end of the
    /// line gives the end of its content; a line past the end is the last line.
    pub fn char_at_display_col(&self, line: usize, col: usize) -> usize {
        position::char_at_display_col(&self.rope, line, col, self.tab_width)
    }

    /// The number of chars in `line` without its `\n`; 0 for a line past the end.
    pub fn line_len_chars(&self, line: usize) -> usize {
        if line < self.line_count() {
            position::line_text(&self.rope, line).chars().count()
        } else {
            0
        }
    }

    /// The UTF-8 byte offset of char offset `idx` (clamped to the document).
    pub fn char_to_byte(&self, idx: usize) -> usize {
        self.rope.char_to_byte(idx.min(self.len_chars()))
    }

    /// The char offset of UTF-8 byte offset `byte` (clamped); a byte inside a character gives that character.
    pub fn byte_to_char(&self, byte: usize) -> usize {
        self.rope.byte_to_char(byte.min(self.rope.len_bytes()))
    }

    /// The UTF-16 code-unit offset of char offset `idx` (clamped), as LSP and many platform APIs count.
    pub fn char_to_utf16(&self, idx: usize) -> usize {
        self.rope.char_to_utf16_cu(idx.min(self.len_chars()))
    }

    /// The char offset of UTF-16 code-unit offset `utf16` (clamped); an offset inside a surrogate pair gives
    /// that character.
    pub fn utf16_to_char(&self, utf16: usize) -> usize {
        self.rope.utf16_cu_to_char(utf16.min(self.rope.len_utf16_cu()))
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    pub fn selections(&self) -> &[Selection] {
        std::slice::from_ref(&self.selection)
    }

    /// Replaces the selection (offsets are clamped to the document).
    pub fn set_selection(&mut self, selection: Selection) {
        let len = self.len_chars();
        self.selection = Selection { anchor: selection.anchor.min(len), head: selection.head.min(len) };
        self.goal_col = None;
    }

    /// Chooses the line ending the next save writes. Not an undo step; the buffer is dirty until saved
    /// (unless this changes nothing: the same ending on a file that already used it throughout).
    pub fn set_line_ending(&mut self, le: LineEnding) {
        if self.line_ending != le || self.mixed_line_endings {
            self.meta_dirty = true;
        }
        self.line_ending = le;
        self.mixed_line_endings = false;
        self.revision += 1;
    }

    pub fn tab_width(&self) -> usize {
        self.tab_width
    }

    pub fn set_tab_width(&mut self, width: usize) {
        self.tab_width = width.max(1);
    }

    /// Counts every change to the text, undo and redo included.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The text differs from what was loaded or last saved.
    pub fn dirty(&self) -> bool {
        !self.history.is_saved() || self.meta_dirty
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// Decoding replaced characters it could not read; the buffer is read-only and cannot be saved.
    pub fn lossy(&self) -> bool {
        self.lossy
    }

    /// The encoding chosen by hand when opening, or `None` when it was detected.
    pub fn explicit_encoding(&self) -> Option<Encoding> {
        self.explicit_encoding
    }

    /// Read-only because the user asked for it (or the text is lossy), not because of file permissions.
    pub fn forced_read_only(&self) -> bool {
        self.forced_read_only
    }

    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub fn mixed_line_endings(&self) -> bool {
        self.mixed_line_endings
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Replaces the time source that decides whether keystrokes merge into one undo step (milliseconds).
    pub fn set_clock(&mut self, clock: impl Fn() -> u64 + Send + 'static) {
        self.clock = Box::new(clock);
    }

    // ---- changing ---------------------------------------------------------------------------------------

    /// Replaces the chars in `range` with `text` as one undo step, and puts the caret after the new text.
    /// Offsets are clamped to the document; `\r\n` / `\r` in `text` become `\n`.
    ///
    /// # Errors
    /// `ReadOnly` when the buffer was opened read-only; the text is then untouched.
    pub fn replace(&mut self, range: Range<usize>, text: &str) -> Result<Change, EditorError> {
        self.replace_as(range, text, EditKind::Other)
    }

    pub(crate) fn replace_as(&mut self, range: Range<usize>, text: &str, kind: EditKind) -> Result<Change, EditorError> {
        if self.read_only {
            return Err(EditorError::ReadOnly);
        }
        let end = range.end.min(self.len_chars());
        let start = range.start.min(end);
        let inserted = normalize_newlines(text);
        let removed = self.rope.slice(start..end).to_string();
        let change = Change {
            start_line: self.rope.char_to_line(start),
            old_lines: removed.matches('\n').count() + 1,
            new_lines: inserted.matches('\n').count() + 1,
        };
        if removed.is_empty() && inserted.is_empty() {
            return Ok(Change { old_lines: 1, new_lines: 1, ..change });
        }
        let after = Selection::caret(start + inserted.chars().count());
        let edit = Edit { start, removed, inserted };
        self.apply_raw(&edit);
        let before = self.selection;
        self.history.record(edit, kind, before, after, (self.clock)());
        self.selection = after;
        self.goal_col = None;
        Ok(change)
    }

    fn apply_raw(&mut self, edit: &Edit) {
        let end = edit.start + edit.removed_chars();
        if end > edit.start {
            self.rope.remove(edit.start..end);
        }
        if !edit.inserted.is_empty() {
            self.rope.insert(edit.start, &edit.inserted);
        }
        self.revision += 1;
    }

    /// Undoes the newest step and restores the selection from before it. `None` when there is nothing to undo.
    /// The returned change covers the lines between the first and the last char the step's edits touched.
    pub fn undo(&mut self) -> Option<Change> {
        let (edits, selection) = self.history.undo()?;
        Some(self.apply_history(edits, selection))
    }

    /// Redoes the step `undo` took back and restores the selection from after it. `None` when there is
    /// nothing to redo. The returned change is computed as for `undo`.
    pub fn redo(&mut self) -> Option<Change> {
        let (edits, selection) = self.history.redo()?;
        Some(self.apply_history(edits, selection))
    }

    fn apply_history(&mut self, edits: Vec<Edit>, selection: Selection) -> Change {
        // The edits apply one after another. Chars before the smallest start and the shortest untouched tail
        // (measured from the end, which later edits do not move) are the same before and after the step.
        let old_len = self.len_chars();
        let (mut head, mut tail, mut len) = (old_len, old_len, old_len);
        for e in &edits {
            head = head.min(e.start);
            tail = tail.min(len.saturating_sub(e.start + e.removed_chars()));
            len = len - e.removed_chars() + e.inserted_chars();
        }
        let start_line = self.rope.char_to_line(head.min(old_len));
        let old_end_line = self.rope.char_to_line((old_len - tail).max(head).min(old_len));
        for edit in &edits {
            self.apply_raw(edit);
        }
        self.set_selection(selection);
        let new_len = self.len_chars();
        let new_end_line = self.rope.char_to_line((new_len - tail.min(new_len)).max(head).min(new_len));
        Change { start_line, old_lines: old_end_line - start_line + 1, new_lines: new_end_line - start_line + 1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_text_normalizes_line_breaks_and_exposes_lines() {
        let b = Buffer::from_text("a\r\nb\rc\n");
        assert_eq!(b.text(), "a\nb\nc\n");
        assert_eq!(b.line_count(), 4, "a trailing newline leaves an empty last line");
        assert_eq!((b.line(0).as_str(), b.line(2).as_str(), b.line(3).as_str(), b.line(9).as_str()), ("a", "c", "", ""));
        assert!(!b.dirty());
        assert_eq!(b.slice(1..4), "\nb\n");
        assert_eq!(b.slice(3..999), "\nc\n", "ranges are clamped");
    }

    #[test]
    fn replace_edits_the_text_moves_the_caret_and_reports_the_lines() {
        let mut b = Buffer::from_text("one\ntwo");
        let change = b.replace(1..5, "X\nY\nZ").unwrap(); // "ne\nt" → "X\nY\nZ"
        assert_eq!(b.text(), "oX\nY\nZwo");
        assert_eq!(change, Change { start_line: 0, old_lines: 2, new_lines: 3 });
        assert_eq!(b.selection(), Selection::caret(6));
        assert_eq!(b.revision(), 1);
        assert!(b.dirty());
        // Out-of-range offsets clamp; \r\n in new text becomes \n.
        b.replace(98..99, "\r\nend").unwrap();
        assert!(b.text().ends_with("wo\nend"));
    }

    #[test]
    fn an_empty_replacement_changes_nothing() {
        let mut b = Buffer::from_text("abc");
        let change = b.replace(1..1, "").unwrap();
        assert_eq!((b.revision(), b.dirty(), change.old_lines, change.new_lines), (0, false, 1, 1));
        assert!(!b.can_undo());
    }

    #[test]
    fn undo_and_redo_restore_text_selection_and_the_clean_state() {
        let mut b = Buffer::from_text("hello");
        b.set_selection(Selection { anchor: 1, head: 3 });
        b.replace(1..3, "EY").unwrap();
        assert_eq!(b.text(), "hEYlo");
        let undone = b.undo().unwrap();
        assert_eq!(b.text(), "hello");
        assert_eq!(b.selection(), Selection { anchor: 1, head: 3 }, "the selection from before the edit");
        assert_eq!(undone, Change { start_line: 0, old_lines: 1, new_lines: 1 });
        assert!(!b.dirty(), "back at the loaded text");
        assert!(b.undo().is_none());
        b.redo().unwrap();
        assert_eq!((b.text().as_str(), b.selection()), ("hEYlo", Selection::caret(3)));
        assert!(b.dirty());
        assert_eq!(b.revision(), 3, "undo and redo count as changes too");
    }

    #[test]
    fn undo_and_redo_report_only_the_lines_the_step_touched() {
        let mut b = Buffer::from_text("l0\nl1\nl2\nl3\nl4");
        // Paste three lines into the middle of line 2: "l2" -> "lA\nB\nC2".
        b.replace(b.char_index(Position { line: 2, col: 1 })..b.char_index(Position { line: 2, col: 1 }), "A\nB\nC").unwrap();
        assert_eq!(b.text(), "l0\nl1\nlA\nB\nC2\nl3\nl4");
        assert_eq!(b.undo().unwrap(), Change { start_line: 2, old_lines: 3, new_lines: 1 });
        assert_eq!(b.text(), "l0\nl1\nl2\nl3\nl4");
        assert_eq!(b.redo().unwrap(), Change { start_line: 2, old_lines: 1, new_lines: 3 });
        // A merged typing step on line 4 (several edits in one group).
        b.set_clock(|| 0);
        b.set_selection(Selection::caret(b.len_chars()));
        for c in ["x", "y", "z"] {
            b.insert(c).unwrap();
        }
        assert!(b.text().ends_with("l4xyz"));
        assert_eq!(b.undo().unwrap(), Change { start_line: 6, old_lines: 1, new_lines: 1 });
        assert!(b.text().ends_with("\nl4"));
        // Deleting a block of lines and undoing it.
        let (s, e) = (b.char_index(Position { line: 1, col: 0 }), b.char_index(Position { line: 4, col: 0 }));
        b.replace(s..e, "").unwrap();
        assert_eq!(b.text(), "l0\nC2\nl3\nl4");
        assert_eq!(b.undo().unwrap(), Change { start_line: 1, old_lines: 1, new_lines: 4 });
        assert_eq!(b.text(), "l0\nl1\nlA\nB\nC2\nl3\nl4");
    }

    #[test]
    fn undo_and_redo_changes_describe_the_text_difference_exactly() {
        // Lines outside the reported range must be identical before and after, for many random steps.
        let mut seed = 11u64;
        let mut rnd = |n: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as usize) % n.max(1)
        };
        let check = |before: &str, after: &str, c: Change| {
            let (a, b): (Vec<&str>, Vec<&str>) = (before.split('\n').collect(), after.split('\n').collect());
            assert!(c.start_line + c.old_lines <= a.len() && c.start_line + c.new_lines <= b.len(), "{c:?}");
            assert_eq!(a[..c.start_line], b[..c.start_line], "{c:?}");
            assert_eq!(a[c.start_line + c.old_lines..], b[c.start_line + c.new_lines..], "{c:?}");
        };
        let mut b = Buffer::from_text("a\nbb\nccc\nd\ne");
        b.set_clock(|| 0);
        for _ in 0..300 {
            match rnd(4) {
                0 | 1 => {
                    let s = rnd(b.len_chars() + 1);
                    let e = (s + rnd(4)).min(b.len_chars());
                    let text = ["x", "\n", "y\nz", "", "q\n\n"][rnd(5)];
                    b.set_selection(Selection { anchor: s, head: e });
                    let _ = if text.chars().count() == 1 { b.insert(text) } else { b.replace(s..e, text) };
                }
                2 => {
                    let before = b.text();
                    if let Some(c) = b.undo() {
                        check(&before, &b.text(), c);
                    }
                }
                _ => {
                    let before = b.text();
                    if let Some(c) = b.redo() {
                        check(&before, &b.text(), c);
                    }
                }
            }
        }
    }

    #[test]
    fn a_read_only_buffer_refuses_every_change() {
        let mut b = Buffer::from_text("keep");
        b.read_only = true;
        assert!(matches!(b.replace(0..1, "x"), Err(EditorError::ReadOnly)));
        assert_eq!((b.text().as_str(), b.revision()), ("keep", 0));
    }

    #[test]
    fn display_columns_account_for_tabs_wide_characters_and_clamp() {
        let mut b = Buffer::from_text("a\t世e\u{301}😀x\nshort");
        // graphemes: a, \t, 世, é (e + accent), 😀, x
        let cols: Vec<usize> = (0..=6).map(|c| b.display_col(Position { line: 0, col: c })).collect();
        assert_eq!(cols, [0, 1, 4, 6, 7, 9, 10]);
        b.set_tab_width(8);
        assert_eq!(b.display_col(Position { line: 0, col: 2 }), 8);
        assert_eq!(b.display_col(Position { line: 0, col: 99 }), 14, "a column past the end clamps");
        assert_eq!(b.display_col(Position { line: 99, col: 2 }), 2, "a line past the end clamps to the last line");
        b.set_tab_width(4);
        // char_at_display_col: the last boundary at or before the column; chars, not graphemes
        assert_eq!(b.char_at_display_col(0, 0), 0);
        assert_eq!(b.char_at_display_col(0, 2), 1, "inside the tab: before it");
        assert_eq!(b.char_at_display_col(0, 4), 2);
        assert_eq!(b.char_at_display_col(0, 5), 2, "inside the wide character");
        assert_eq!(b.char_at_display_col(0, 6), 3);
        assert_eq!(b.char_at_display_col(0, 7), 5, "after e + accent, before the emoji");
        assert_eq!(b.char_at_display_col(0, 999), 7, "past the end: the end of the line content");
        assert_eq!(b.char_at_display_col(1, 3), 11);
        assert_eq!(b.char_at_display_col(99, 3), 11, "a line past the end clamps");
    }

    #[test]
    fn len_bytes_counts_utf8_bytes() {
        assert_eq!(Buffer::from_text("a你").len_bytes(), 4);
    }

    #[test]
    fn line_len_chars_counts_content_without_the_newline() {
        let b = Buffer::from_text("héllo\n\n世界😀\nx");
        assert_eq!((b.line_len_chars(0), b.line_len_chars(1), b.line_len_chars(2), b.line_len_chars(3)), (5, 0, 3, 1));
        assert_eq!(b.line_len_chars(99), 0);
    }

    #[test]
    fn word_ranges_cover_the_run_under_a_position() {
        let b = Buffer::from_text("foo_bar  +=  日本語e\u{301}x\n\nend");
        // "foo_bar" 0..7, spaces 7..9, "+=" 9..11, spaces 11..13, "日本語éx" 13..19, newline at 19
        assert_eq!(b.word_range_at(0), 0..7);
        assert_eq!(b.word_range_at(6), 0..7);
        assert_eq!(b.word_range_at(7), 7..9, "whitespace run");
        assert_eq!(b.word_range_at(10), 9..11, "punctuation run");
        assert_eq!(b.word_range_at(14), 13..19);
        assert_eq!(b.word_range_at(17), 13..19, "on the combining accent: the word it belongs to");
        assert_eq!(b.word_range_at(19), 13..19, "at the end of a line: the word before it");
        assert_eq!(b.word_range_at(20), 20..20, "an empty line");
        assert_eq!(b.word_range_at(21), 21..24);
        assert_eq!(b.word_range_at(24), 24..24, "the end of the document");
        assert_eq!(b.word_range_at(999), 24..24, "past the end clamps");
        assert_eq!(Buffer::from_text("").word_range_at(0), 0..0);
    }

    #[test]
    fn byte_and_utf16_offsets_convert_both_ways_and_clamp() {
        let b = Buffer::from_text("a世😀e\u{301}");
        // chars: a(1 byte, 1 unit) 世(3, 1) 😀(4, 2) e(1, 1) accent(2, 1)
        let chars = [0, 1, 2, 3, 4, 5];
        let bytes = [0, 1, 4, 8, 9, 11];
        let units = [0, 1, 2, 4, 5, 6];
        for i in 0..chars.len() {
            assert_eq!(b.char_to_byte(chars[i]), bytes[i]);
            assert_eq!(b.byte_to_char(bytes[i]), chars[i]);
            assert_eq!(b.char_to_utf16(chars[i]), units[i]);
            assert_eq!(b.utf16_to_char(units[i]), chars[i]);
        }
        assert_eq!(b.byte_to_char(2), 1, "inside a character: that character");
        assert_eq!(b.utf16_to_char(3), 2, "inside a surrogate pair: that character");
        assert_eq!((b.char_to_byte(99), b.byte_to_char(99), b.char_to_utf16(99), b.utf16_to_char(99)), (11, 5, 6, 5));
    }
}
