//! Editing commands of `EditorModel`. Every change goes through `Buffer`'s single replace path
//! (so it is undoable) and is then fed to the wrap map.

use super::indent::{self, Enter};
use super::model::EditorModel;
use gilvt_editor::{Buffer, Change, EditorError, LineEnding, OpenOptions, Position, Selection};
use std::path::Path;
use unicode_segmentation::UnicodeSegmentation;

impl EditorModel {
    fn applied(&mut self, r: Result<Change, EditorError>) -> Result<(), EditorError> {
        let change = r?;
        self.sync(change);
        Ok(())
    }

    pub fn insert(&mut self, text: &str) -> Result<(), EditorError> {
        self.guard()?;
        // `Buffer` normalizes \r\n / \r to \n itself.
        let r = self.buf.insert(text);
        self.applied(r)
    }

    pub fn enter(&mut self) -> Result<(), EditorError> {
        self.guard()?;
        let sel = self.buf.selection();
        let p = self.buf.position(sel.start());
        let line = self.buf.line(p.line);
        let before: String = line.graphemes(true).take(p.col).collect();
        match indent::enter_action(&line, &before, !sel.is_empty()) {
            Enter::Insert(s) => {
                let r = self.buf.insert(&s);
                self.applied(r)
            }
            Enter::ClearPrefix => {
                let start = self.buf.char_index(Position { line: p.line, col: 0 });
                let r = self.buf.replace(start..sel.end(), "");
                self.applied(r)
            }
        }
    }

    pub fn tab(&mut self, shift: bool) -> Result<(), EditorError> {
        self.guard()?;
        let sel = self.buf.selection();
        let (ps, pe) = (self.buf.position(sel.start()), self.buf.position(sel.end()));
        // A selection that ends at column 0 of a later line does not include that line.
        let last = if !sel.is_empty() && pe.col == 0 && pe.line > ps.line { pe.line - 1 } else { pe.line };
        let block = last > ps.line || shift;
        if !block {
            let unit = self.unit().text();
            let r = self.buf.insert(&unit);
            return self.applied(r);
        }
        let lines: Vec<String> = (ps.line..=last).map(|l| self.buf.line(l)).collect();
        let new: Vec<String> = if shift {
            lines.iter().map(|l| indent::outdent_line(l, self.unit())).collect()
        } else {
            indent::indent_lines(&lines, self.unit())
        };
        if new == lines {
            return Ok(()); // nothing to indent or outdent: no undo step, no dirty marker
        }
        let start = self.buf.char_index(Position { line: ps.line, col: 0 });
        let end = self.buf.char_index(Position { line: last, col: 0 }) + self.buf.line_len_chars(last);
        let joined = new.join("\n");
        let new_len = joined.chars().count();
        let caret_only = sel.is_empty();
        let removed = lines[0].chars().count().saturating_sub(new[0].chars().count());
        let r = self.buf.replace(start..end, &joined);
        self.applied(r)?;
        if caret_only {
            let caret = if shift { sel.head.saturating_sub(removed).max(start) } else { sel.head };
            self.buf.set_selection(Selection { anchor: caret, head: caret });
        } else {
            self.buf.set_selection(Selection { anchor: start, head: start + new_len });
        }
        Ok(())
    }

    pub fn backspace(&mut self) -> Result<(), EditorError> {
        self.guard()?;
        let r = self.buf.backspace();
        self.applied(r)
    }

    pub fn delete_forward(&mut self) -> Result<(), EditorError> {
        self.guard()?;
        let r = self.buf.delete_forward();
        self.applied(r)
    }

    pub fn delete_word_back(&mut self) -> Result<(), EditorError> {
        self.guard()?;
        let r = self.buf.delete_word_back();
        self.applied(r)
    }

    pub fn copy_text(&self) -> Option<String> {
        let sel = self.buf.selection();
        (!sel.is_empty()).then(|| self.buf.slice(sel.range()))
    }

    pub fn cut(&mut self) -> Result<Option<String>, EditorError> {
        self.guard()?;
        let Some(text) = self.copy_text() else { return Ok(None) };
        let r = self.buf.insert("");
        self.applied(r)?;
        Ok(Some(text))
    }

    pub fn undo(&mut self) -> bool {
        match self.buf.undo() {
            Some(c) => {
                self.sync(c);
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.buf.redo() {
            Some(c) => {
                self.sync(c);
                true
            }
            None => false,
        }
    }

    pub fn save(&mut self) -> Result<(), EditorError> {
        self.buf.save()
    }

    pub fn save_overwrite(&mut self) -> Result<(), EditorError> {
        self.buf.save_overwrite()
    }

    /// Reloads from disk as one undoable edit (E2b-1 §4); counts as saved.
    pub fn reload_as_edit(&mut self) -> Result<(), EditorError> {
        let c = self.buf.reload_as_edit()?;
        self.sync(c);
        Ok(())
    }

    /// Reopens the file with `opts`, keeping the wrap width, view height and top line. On failure `self` is unchanged.
    pub fn reopen(&mut self, opts: OpenOptions) -> Result<(), EditorError> {
        let Some(path) = self.buf.path().map(Path::to_path_buf) else { return Err(EditorError::NoPath) };
        let buf = Buffer::open_with(&path, opts)?;
        let (cols, rows) = (self.viewport_cols(), self.view_rows());
        let top = self.row(self.scroll_row()).line;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        *self = EditorModel::new(buf, &name, cols, rows);
        self.scroll_to_line(top);
        Ok(())
    }

    /// Changes the line ending (marks the buffer dirty). Refused on a read-only / lossy model: it can never be
    /// saved, so it would stay dirty for good.
    pub fn set_line_ending(&mut self, le: LineEnding) -> Result<(), EditorError> {
        self.guard()?;
        self.buf.set_line_ending(le);
        Ok(())
    }

    /// Takes the disk version, clearing the history (E1 `reload`): the silent reload of a clean buffer (E2b-1 §4).
    pub fn reload(&mut self) -> Result<(), EditorError> {
        self.buf.reload()?;
        self.rebuild_wrap();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::*;
    use gilvt_editor::{Buffer, EditorError, LineEnding, OpenOptions, Position, Selection};

    fn model(text: &str) -> EditorModel {
        EditorModel::new(Buffer::from_text(text), "t.md", 40, 10)
    }
    fn select(m: &mut EditorModel, a: usize, b: usize) {
        m.buf.set_selection(Selection { anchor: a, head: b });
    }
    fn end(m: &mut EditorModel) {
        m.move_h(HMove::DocEnd, false);
    }

    #[test]
    fn typing_replaces_the_selection_and_rewraps() {
        let mut m = model("hello world");
        select(&mut m, 0, 5);
        m.insert("goodbye").unwrap();
        assert_eq!(m.buf.text(), "goodbye world");
        let long = "x".repeat(100);
        m.insert(&long).unwrap();
        assert!(m.total_rows() > 1);
    }

    #[test]
    fn enter_keeps_indentation_and_list_markers() {
        let mut m = model("  - item");
        end(&mut m);
        m.enter().unwrap();
        assert_eq!(m.buf.text(), "  - item\n  - ");
        m.enter().unwrap(); // empty item: the marker goes away
        assert_eq!(m.buf.text(), "  - item\n");
    }

    #[test]
    fn tab_inserts_one_unit_and_shift_tab_removes_it() {
        let mut m = model("a");
        m.tab(false).unwrap();
        assert_eq!(m.buf.text(), "  a"); // .md => 2 spaces, the caret was at the start
        m.tab(true).unwrap();
        assert_eq!(m.buf.text(), "a");
    }

    #[test]
    fn tab_that_changes_nothing_leaves_the_buffer_clean() {
        let mut m = model("abc\ndef");
        m.tab(true).unwrap(); // ⇧Tab on an unindented line
        assert!(!m.buf.dirty() && !m.buf.can_undo());
        let mut m = model("\n\n");
        select(&mut m, 0, 2);
        m.tab(false).unwrap(); // Tab on a block of only empty lines
        assert!(!m.buf.dirty() && !m.buf.can_undo());
        let mut m = model("abc\ndef");
        select(&mut m, 0, 5);
        m.tab(false).unwrap(); // a real indent still marks dirty
        assert!(m.buf.dirty() && m.buf.can_undo());
    }

    #[test]
    fn tab_on_several_lines_indents_the_block_in_one_undo_step() {
        let mut m = model("a\nb\n\nc");
        select(&mut m, 0, 5); // a, b, empty (and the start of c's line is NOT selected)
        m.tab(false).unwrap();
        assert_eq!(m.buf.text(), "  a\n  b\n\nc");
        assert_eq!(m.buf.slice(m.buf.selection().range()), "  a\n  b\n"); // selection still covers the lines
        assert!(m.undo());
        assert_eq!(m.buf.text(), "a\nb\n\nc");
        m.buf.set_selection(Selection { anchor: 0, head: 3 });
        m.tab(false).unwrap();
        m.tab(true).unwrap();
        assert_eq!(m.buf.text(), "a\nb\n\nc");
    }

    #[test]
    fn selection_ending_at_column_zero_leaves_that_line_alone() {
        let mut m = model("a\nb\nc");
        let end_of_b_line = m.buf.char_index(Position { line: 2, col: 0 });
        select(&mut m, 0, end_of_b_line);
        m.tab(false).unwrap();
        assert_eq!(m.buf.text(), "  a\n  b\nc");
    }

    #[test]
    fn copy_cut_and_paste_roundtrip() {
        let mut m = model("one two");
        select(&mut m, 0, 3);
        assert_eq!(m.copy_text().as_deref(), Some("one"));
        assert_eq!(m.cut().unwrap().as_deref(), Some("one"));
        assert_eq!(m.buf.text(), " two");
        m.insert("a\r\nb").unwrap(); // pasted CRLF becomes \n
        assert_eq!(m.buf.text(), "a\nb two");
        let mut empty = model("x");
        assert_eq!(empty.copy_text(), None);
        assert_eq!(empty.cut().unwrap(), None);
    }

    #[test]
    fn undo_redo_resync_the_wrap() {
        let mut m = model("short");
        m.insert(&"y".repeat(100)).unwrap();
        let rows = m.total_rows();
        assert!(m.undo());
        assert_eq!(m.total_rows(), 1);
        assert!(m.redo());
        assert_eq!(m.total_rows(), rows);
        assert!(!model("x").undo());
    }

    #[test]
    fn read_only_models_refuse_every_edit_but_still_copy() {
        let mut m = EditorModel::new(Buffer::from_text(&"x".repeat(LONG_LINE_CHARS + 1)), "t.txt", 40, 10);
        select(&mut m, 0, 3);
        assert!(matches!(m.insert("a"), Err(EditorError::ReadOnly)));
        assert!(matches!(m.enter(), Err(EditorError::ReadOnly)));
        assert!(matches!(m.tab(false), Err(EditorError::ReadOnly)));
        assert!(matches!(m.backspace(), Err(EditorError::ReadOnly)));
        assert!(matches!(m.cut(), Err(EditorError::ReadOnly)));
        assert_eq!(m.copy_text().as_deref(), Some("xxx"));
    }

    #[test]
    fn save_goes_through_the_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.md");
        std::fs::write(&p, "hi\n").unwrap();
        let mut m = EditorModel::new(Buffer::open(&p).unwrap(), "a.md", 40, 10);
        end(&mut m);
        m.insert("!").unwrap();
        m.save().unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hi\n!");
        std::fs::write(&p, "changed outside\n").unwrap();
        m.insert("?").unwrap();
        assert!(matches!(m.save(), Err(EditorError::ModifiedOnDisk)));
        m.save_overwrite().unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hi\n!?");
    }

    #[test]
    fn reload_rebuilds_the_wrap() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.md");
        std::fs::write(&p, "one\n").unwrap();
        let mut m = EditorModel::new(Buffer::open(&p).unwrap(), "a.md", 40, 10);
        std::fs::write(&p, "one\ntwo\nthree\n").unwrap();
        m.reload().unwrap();
        assert_eq!(m.total_rows(), 4);
    }

    #[test]
    fn reload_as_edit_rewraps_and_undo_restores() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.md");
        std::fs::write(&p, "short\n").unwrap();
        let mut m = EditorModel::new(Buffer::open(&p).unwrap(), "a.md", 40, 10);
        m.insert("mine ").unwrap();
        std::fs::write(&p, format!("{}\n", "word ".repeat(30))).unwrap();
        m.reload_as_edit().unwrap();
        assert!(m.total_rows() > 1, "the wrap map follows the new text");
        assert!(!m.buf.dirty());
        assert!(m.undo());
        assert_eq!(m.buf.text(), "mine short\n");
        assert_eq!(m.total_rows(), 2);
    }

    #[test]
    fn reopen_with_another_encoding_keeps_the_size_and_the_top_line() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("g.txt");
        let mut bytes = Vec::new();
        for _ in 0..50 {
            bytes.extend_from_slice(&[0xC4, 0xE3, 0xBA, 0xC3, b'\n']);
        }
        std::fs::write(&p, &bytes).unwrap();
        let mut m = EditorModel::new(Buffer::open_with(&p, OpenOptions { encoding: None, read_only: true }).unwrap(), "g.txt", 30, 5);
        m.scroll_by(10);
        let top = m.row(m.scroll_row()).line;
        let gbk = gilvt_editor::encoding_by_label("gbk").unwrap();
        m.reopen(OpenOptions { encoding: Some(gbk), read_only: false }).unwrap();
        assert_eq!(m.buf.line(0), "你好");
        assert_eq!(m.row(m.scroll_row()).line, top);
        assert_eq!(m.view_rows(), 5);
        assert_eq!(m.viewport_cols(), 30);
        assert!(!m.is_read_only());
    }

    #[test]
    fn a_failed_reopen_leaves_the_model_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "abc\n").unwrap();
        let mut m = EditorModel::new(Buffer::open(&p).unwrap(), "a.txt", 40, 5);
        m.insert("x").unwrap();
        std::fs::remove_file(&p).unwrap();
        assert!(m.reopen(OpenOptions::default()).is_err());
        assert_eq!(m.buf.text(), "xabc\n");
        assert!(m.buf.dirty());
    }

    #[test]
    fn reopening_a_buffer_without_a_path_is_no_path() {
        let mut m = model("a\n");
        assert!(matches!(m.reopen(OpenOptions::default()), Err(EditorError::NoPath)));
    }

    #[test]
    fn set_line_ending_on_the_model_marks_dirty() {
        let mut m = model("a\nb");
        m.set_line_ending(LineEnding::CrLf).unwrap();
        assert!(m.buf.dirty());
    }

    #[test]
    fn set_line_ending_is_refused_on_a_read_only_model() {
        let mut m = EditorModel::new(Buffer::from_text(&"x".repeat(LONG_LINE_CHARS + 1)), "t.txt", 40, 10);
        assert!(m.is_read_only());
        assert!(matches!(m.set_line_ending(LineEnding::CrLf), Err(EditorError::ReadOnly)));
        assert!(!m.buf.dirty());
    }
}
