//! The editing commands a key map calls: typing, deleting, new lines. Each one is a single `replace`, so
//! each is undoable; typing and single-character deletes carry a kind so a run of them merges into one step.

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::{Buffer, Change};
use crate::error::EditorError;
use crate::history::EditKind;
use crate::motion::{next_word_end, prev_word_start};
use crate::position;

impl Buffer {
    /// What a command that finds nothing to do returns: no edit, no history entry, no revision.
    fn nothing_changed(&self) -> Change {
        Change { start_line: self.rope.char_to_line(self.selection.head), old_lines: 1, new_lines: 1 }
    }

    /// Types `text` at the caret, replacing the selection. A single typed grapheme (no selection, no line
    /// break) merges with the keystrokes before it into one undo step; a paste is its own step.
    ///
    /// # Errors
    /// `ReadOnly` when the buffer was opened read-only.
    pub fn insert(&mut self, text: &str) -> Result<Change, EditorError> {
        let sel = self.selection;
        let one_key = sel.is_empty() && !text.contains(['\n', '\r']) && text.graphemes(true).count() == 1;
        self.replace_as(sel.range(), text, if one_key { EditKind::Typing } else { EditKind::Other })
    }

    /// Inserts a line break at the caret, replacing the selection.
    ///
    /// # Errors
    /// `ReadOnly` when the buffer was opened read-only.
    pub fn newline(&mut self) -> Result<Change, EditorError> {
        self.insert("\n")
    }

    /// Deletes the selection, or the grapheme before the caret (joining lines at a line start).
    ///
    /// # Errors
    /// `ReadOnly` when the buffer was opened read-only.
    pub fn backspace(&mut self) -> Result<Change, EditorError> {
        let sel = self.selection;
        if !sel.is_empty() {
            return self.replace(sel.range(), "");
        }
        if sel.head == 0 {
            return self.check_writable().map(|_| self.nothing_changed());
        }
        let start = position::prev_boundary(&self.rope, sel.head);
        self.replace_as(start..sel.head, "", EditKind::Deleting)
    }

    /// Deletes the selection, or the grapheme after the caret (joining lines at a line end).
    ///
    /// # Errors
    /// `ReadOnly` when the buffer was opened read-only.
    pub fn delete_forward(&mut self) -> Result<Change, EditorError> {
        let sel = self.selection;
        if !sel.is_empty() {
            return self.replace(sel.range(), "");
        }
        let end = position::next_boundary(&self.rope, sel.head);
        if end == sel.head {
            return self.check_writable().map(|_| self.nothing_changed());
        }
        self.replace_as(sel.head..end, "", EditKind::Deleting)
    }

    /// Deletes the selection, or back to where Ctrl/Option+Left would stop.
    ///
    /// # Errors
    /// `ReadOnly` when the buffer was opened read-only.
    pub fn delete_word_back(&mut self) -> Result<Change, EditorError> {
        let sel = self.selection;
        let start = if sel.is_empty() { prev_word_start(&self.rope, sel.head) } else { sel.start() };
        self.replace(start..sel.end(), "")
    }

    /// Deletes the selection, or forward to where Ctrl/Option+Right would stop.
    ///
    /// # Errors
    /// `ReadOnly` when the buffer was opened read-only.
    pub fn delete_word_forward(&mut self) -> Result<Change, EditorError> {
        let sel = self.selection;
        let end = if sel.is_empty() { next_word_end(&self.rope, sel.head) } else { sel.end() };
        self.replace(sel.start()..end, "")
    }

    /// Deletes the selection, or from the caret to the end of the line; at the end of a line, the line break.
    ///
    /// # Errors
    /// `ReadOnly` when the buffer was opened read-only.
    pub fn delete_to_line_end(&mut self) -> Result<Change, EditorError> {
        let sel = self.selection;
        if !sel.is_empty() {
            return self.replace(sel.range(), "");
        }
        let line = self.rope.char_to_line(sel.head);
        let content_end = self.rope.line_to_char(line) + position::line_text(&self.rope, line).chars().count();
        let end = if sel.head < content_end { content_end } else { position::next_boundary(&self.rope, sel.head) };
        self.replace(sel.head..end, "")
    }

    /// Deletes the whole line the caret is on, its line break included; the last line takes the break before
    /// it instead, so the line really disappears.
    ///
    /// # Errors
    /// `ReadOnly` when the buffer was opened read-only.
    pub fn delete_line(&mut self) -> Result<Change, EditorError> {
        let line = self.rope.char_to_line(self.selection.head);
        let start = self.rope.line_to_char(line);
        let end = if line + 1 < self.line_count() { self.rope.line_to_char(line + 1) } else { self.len_chars() };
        let start = if end == self.len_chars() && start > 0 { start - 1 } else { start };
        self.replace(start..end, "")
    }

    fn check_writable(&self) -> Result<(), EditorError> {
        if self.read_only {
            Err(EditorError::ReadOnly)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::Selection;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// A buffer whose clock the test moves by hand.
    fn timed(text: &str) -> (Buffer, Arc<AtomicU64>) {
        let now = Arc::new(AtomicU64::new(0));
        let mut b = Buffer::from_text(text);
        let handle = now.clone();
        b.set_clock(move || handle.load(Ordering::SeqCst));
        (b, now)
    }

    fn caret_at_end(b: &mut Buffer) {
        b.move_doc_end(false);
    }

    #[test]
    fn typing_merges_until_a_pause_and_a_paste_is_its_own_step() {
        let (mut b, now) = timed("");
        for (ch, ms) in [("h", 0), ("i", 100), ("!", 300)] {
            now.store(ms, Ordering::SeqCst);
            b.insert(ch).unwrap();
        }
        now.store(2000, Ordering::SeqCst);
        b.insert("?").unwrap(); // after a pause: a new step
        b.insert(" and a paste").unwrap(); // more than one grapheme: always its own step
        assert_eq!(b.text(), "hi!? and a paste");
        b.undo().unwrap();
        assert_eq!(b.text(), "hi!?");
        b.undo().unwrap();
        assert_eq!(b.text(), "hi!");
        b.undo().unwrap();
        assert_eq!(b.text(), "");
        assert!(!b.dirty());
    }

    #[test]
    fn typing_over_a_selection_replaces_it_as_one_step() {
        let (mut b, _) = timed("hello world");
        b.set_selection(Selection { anchor: 0, head: 5 });
        b.insert("J").unwrap();
        assert_eq!(b.text(), "J world");
        b.undo().unwrap();
        assert_eq!((b.text().as_str(), b.selection()), ("hello world", Selection { anchor: 0, head: 5 }));
    }

    #[test]
    fn backspace_and_delete_remove_whole_graphemes_and_join_lines() {
        let (mut b, _) = timed("ae\u{301}\n👨\u{200d}👩x");
        b.set_selection(Selection::caret(3)); // after "é"
        b.backspace().unwrap();
        assert_eq!(b.text(), "a\n👨\u{200d}👩x", "accent and letter go together");
        b.set_selection(Selection::caret(2)); // start of line 2
        b.backspace().unwrap();
        assert_eq!(b.text(), "a👨\u{200d}👩x", "joined the lines");
        b.set_selection(Selection::caret(1));
        b.delete_forward().unwrap();
        assert_eq!(b.text(), "ax", "the whole emoji sequence in one key press");
        b.set_selection(Selection::caret(0));
        let before = b.revision();
        b.backspace().unwrap();
        assert_eq!(b.revision(), before, "nothing before the caret: no edit");
        b.move_doc_end(false);
        b.delete_forward().unwrap();
        assert_eq!(b.text(), "ax");
    }

    #[test]
    fn a_selection_is_deleted_by_either_key() {
        let (mut b, _) = timed("abcdef");
        b.set_selection(Selection { anchor: 4, head: 1 });
        b.delete_forward().unwrap();
        assert_eq!((b.text().as_str(), b.selection()), ("aef", Selection::caret(1)));
        b.set_selection(Selection { anchor: 0, head: 2 });
        b.backspace().unwrap();
        assert_eq!(b.text(), "f");
    }

    #[test]
    fn a_run_of_backspaces_is_one_step_separate_from_the_typing_before_it() {
        let (mut b, now) = timed("");
        for (i, ch) in ["a", "b", "c"].iter().enumerate() {
            now.store(i as u64 * 10, Ordering::SeqCst);
            b.insert(ch).unwrap();
        }
        for ms in [100, 150] {
            now.store(ms, Ordering::SeqCst);
            b.backspace().unwrap();
        }
        assert_eq!(b.text(), "a");
        b.undo().unwrap();
        assert_eq!(b.text(), "abc", "both backspaces at once");
        b.undo().unwrap();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn newline_splits_a_line_in_its_own_step() {
        let (mut b, _) = timed("ab");
        b.set_selection(Selection::caret(1));
        let change = b.newline().unwrap();
        assert_eq!((b.text().as_str(), b.selection()), ("a\nb", Selection::caret(2)));
        assert_eq!(change, Change { start_line: 0, old_lines: 1, new_lines: 2 });
        b.undo().unwrap();
        assert_eq!(b.text(), "ab");
    }

    #[test]
    fn word_and_line_deletions() {
        let (mut b, _) = timed("foo bar baz\nnext");
        b.set_selection(Selection::caret(7));
        b.delete_word_back().unwrap();
        assert_eq!(b.text(), "foo  baz\nnext");
        b.delete_word_forward().unwrap();
        assert_eq!(b.text(), "foo \nnext");
        b.set_selection(Selection::caret(2));
        b.delete_to_line_end().unwrap();
        assert_eq!(b.text(), "fo\nnext");
        b.delete_to_line_end().unwrap();
        assert_eq!(b.text(), "fonext", "at a line end it removes the line break");
        b.undo().unwrap();
        b.undo().unwrap();
        assert_eq!(b.text(), "foo \nnext");
    }

    #[test]
    fn delete_line_removes_the_line_including_its_break() {
        let (mut b, _) = timed("one\ntwo\nthree");
        b.set_selection(Selection::caret(5));
        b.delete_line().unwrap();
        assert_eq!(b.text(), "one\nthree");
        caret_at_end(&mut b);
        b.delete_line().unwrap();
        assert_eq!(b.text(), "one", "the last line takes the break before it");
        b.delete_line().unwrap();
        assert_eq!(b.text(), "", "a lone line just empties");
    }

    #[test]
    fn a_read_only_buffer_rejects_every_command_even_when_there_is_nothing_to_do() {
        let (mut b, _) = timed("x");
        b.read_only = true;
        b.set_selection(Selection::caret(0));
        assert!(matches!(b.insert("a"), Err(EditorError::ReadOnly)));
        assert!(matches!(b.backspace(), Err(EditorError::ReadOnly)), "at the start too");
        b.move_doc_end(false);
        assert!(matches!(b.delete_forward(), Err(EditorError::ReadOnly)), "at the end too");
        assert!(matches!(b.delete_line(), Err(EditorError::ReadOnly)));
        assert_eq!(b.text(), "x");
    }
}
