//! Undo / redo as groups of edits. Typing a word is one undo step, not one per letter.
//!
//! The history never touches the text: `record` is told what the buffer just did, `undo` / `redo` hand back
//! the edits the buffer must apply. Time comes in as milliseconds so tests can control it.

use crate::position::Selection;

/// Two typing (or two deleting) edits closer together than this, and adjacent, share an undo step.
pub const MERGE_WINDOW_MS: u64 = 1000;
/// Older undo steps are dropped beyond this many.
pub const MAX_UNDO_GROUPS: usize = 1000;

/// One replacement of a range of the text, in char offsets. The smallest unit of change and of undo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub removed: String,
    pub inserted: String,
}

impl Edit {
    /// The edit that puts the text back as it was before this one.
    pub fn inverse(&self) -> Edit {
        Edit { start: self.start, removed: self.inserted.clone(), inserted: self.removed.clone() }
    }

    pub fn removed_chars(&self) -> usize {
        self.removed.chars().count()
    }

    pub fn inserted_chars(&self) -> usize {
        self.inserted.chars().count()
    }
}

/// What kind of change an edit is; only typing and deleting merge into the previous step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// One grapheme typed at the caret.
    Typing,
    /// One grapheme removed with Backspace or Delete.
    Deleting,
    /// Paste, newline, deleting a selection or a word, programmatic replacement: always its own step.
    Other,
}

#[derive(Clone, Debug)]
struct Group {
    edits: Vec<Edit>,
    before: Selection,
    after: Selection,
    kind: EditKind,
    at_ms: u64,
}

#[derive(Debug)]
pub struct History {
    undo: Vec<Group>,
    redo: Vec<Group>,
    /// `undo.len()` when the text was last saved (or loaded); `None` once that state is unreachable.
    saved_depth: Option<usize>,
    /// The next edit must start a new step (after a save, an undo or a redo).
    sealed: bool,
}

impl Default for History {
    fn default() -> Self {
        History::new()
    }
}

impl History {
    /// An empty history whose current state counts as saved.
    pub fn new() -> History {
        History { undo: Vec::new(), redo: Vec::new(), saved_depth: Some(0), sealed: false }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The text is in the state it was in when `mark_saved` was last called (or when it was loaded).
    pub fn is_saved(&self) -> bool {
        self.saved_depth == Some(self.undo.len())
    }

    /// The current state is the saved one. The next edit starts a new step, so undoing it returns here.
    pub fn mark_saved(&mut self) {
        self.saved_depth = Some(self.undo.len());
        self.sealed = true;
    }

    /// Notes that the buffer applied `edit`, with the selection before and after it.
    pub fn record(&mut self, edit: Edit, kind: EditKind, before: Selection, after: Selection, now_ms: u64) {
        let can_merge = !self.sealed
            && self.redo.is_empty()
            && matches!(kind, EditKind::Typing | EditKind::Deleting)
            && self.undo.last().is_some_and(|g| {
                g.kind == kind
                    && now_ms.saturating_sub(g.at_ms) < MERGE_WINDOW_MS
                    && continues(g.edits.last().unwrap(), &edit, kind)
            });
        self.redo.clear();
        self.sealed = false;
        if can_merge {
            let g = self.undo.last_mut().unwrap();
            g.edits.push(edit);
            g.after = after;
            g.at_ms = now_ms;
            return;
        }
        // A new step after undoing: the saved state (if it was ahead of here) can no longer be reached.
        if self.saved_depth.is_some_and(|d| d > self.undo.len()) {
            self.saved_depth = None;
        }
        self.undo.push(Group { edits: vec![edit], before, after, kind, at_ms: now_ms });
        if self.undo.len() > MAX_UNDO_GROUPS {
            self.undo.remove(0);
            self.saved_depth = self.saved_depth.and_then(|d| d.checked_sub(1));
        }
    }

    /// Pops the newest step: the edits that reverse it, in the order to apply them, and the selection to
    /// restore. `None` when there is nothing to undo.
    pub fn undo(&mut self) -> Option<(Vec<Edit>, Selection)> {
        let group = self.undo.pop()?;
        self.sealed = true;
        let edits = group.edits.iter().rev().map(Edit::inverse).collect();
        let before = group.before;
        self.redo.push(group);
        Some((edits, before))
    }

    /// Re-applies the most recently undone step: its edits in order and the selection to restore.
    pub fn redo(&mut self) -> Option<(Vec<Edit>, Selection)> {
        let group = self.redo.pop()?;
        self.sealed = true;
        let edits = group.edits.clone();
        let after = group.after;
        self.undo.push(group);
        Some((edits, after))
    }
}

/// `next` carries on from `prev`: typing right after it, or deleting the neighbouring grapheme.
fn continues(prev: &Edit, next: &Edit, kind: EditKind) -> bool {
    match kind {
        EditKind::Typing => {
            prev.removed.is_empty() && next.removed.is_empty() && next.start == prev.start + prev.inserted_chars()
        }
        EditKind::Deleting => {
            prev.inserted.is_empty()
                && next.inserted.is_empty()
                && (next.start == prev.start || next.start + next.removed_chars() == prev.start)
        }
        EditKind::Other => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caret(at: usize) -> Selection {
        Selection::caret(at)
    }

    fn typed(h: &mut History, at: usize, ch: &str, ms: u64) {
        let edit = Edit { start: at, removed: String::new(), inserted: ch.into() };
        h.record(edit, EditKind::Typing, caret(at), caret(at + 1), ms);
    }

    #[test]
    fn adjacent_typing_within_a_second_is_one_step() {
        let mut h = History::new();
        typed(&mut h, 0, "a", 0);
        typed(&mut h, 1, "b", 400);
        typed(&mut h, 2, "c", 900);
        let (edits, sel) = h.undo().unwrap();
        assert_eq!(edits.iter().map(|e| (e.start, e.removed.as_str())).collect::<Vec<_>>(), [(2, "c"), (1, "b"), (0, "a")]);
        assert_eq!(sel, caret(0), "the selection from before the first keystroke");
        assert!(!h.can_undo());
    }

    #[test]
    fn a_pause_a_jump_or_a_different_kind_starts_a_new_step() {
        let mut h = History::new();
        typed(&mut h, 0, "a", 0);
        typed(&mut h, 1, "b", 1000); // a full second after: new step
        typed(&mut h, 9, "c", 1100); // not adjacent: new step
        let paste = Edit { start: 10, removed: String::new(), inserted: "xyz".into() };
        h.record(paste, EditKind::Other, caret(10), caret(13), 1150);
        typed(&mut h, 13, "d", 1200); // after a paste: new step
        let mut steps = 0;
        while h.undo().is_some() {
            steps += 1;
        }
        assert_eq!(steps, 5);
    }

    #[test]
    fn backspace_and_forward_delete_runs_merge() {
        let mut h = History::new();
        // Backspace over "cba" from offset 3: each removes the char before the previous start.
        for (start, ch, ms) in [(2, "c", 0), (1, "b", 100), (0, "a", 200)] {
            let e = Edit { start, removed: ch.into(), inserted: String::new() };
            h.record(e, EditKind::Deleting, caret(start + 1), caret(start), ms);
        }
        assert_eq!(h.undo().unwrap().0.len(), 3);
        assert!(!h.can_undo());
        // Delete key: the start stays put.
        for ms in [0, 100] {
            let e = Edit { start: 4, removed: "x".into(), inserted: String::new() };
            h.record(e, EditKind::Deleting, caret(4), caret(4), 5000 + ms);
        }
        assert_eq!(h.undo().unwrap().0.len(), 2);
    }

    #[test]
    fn redo_replays_in_order_and_a_new_edit_clears_it() {
        let mut h = History::new();
        typed(&mut h, 0, "a", 0);
        typed(&mut h, 1, "b", 10);
        let (undo_edits, _) = h.undo().unwrap();
        assert_eq!(undo_edits.len(), 2);
        let (redo_edits, sel) = h.redo().unwrap();
        assert_eq!(redo_edits.iter().map(|e| e.inserted.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(sel, caret(2));
        h.undo().unwrap();
        assert!(h.can_redo());
        typed(&mut h, 0, "z", 20); // a fresh edit after undo: the old future is gone
        assert!(!h.can_redo());
        assert_eq!(h.undo().unwrap().0.len(), 1, "and it did not merge into the undone step");
    }

    #[test]
    fn saved_state_follows_undo_and_redo() {
        let mut h = History::new();
        assert!(h.is_saved());
        typed(&mut h, 0, "a", 0);
        assert!(!h.is_saved());
        h.mark_saved();
        assert!(h.is_saved());
        typed(&mut h, 1, "b", 10); // right after a save: must not merge into the saved step
        assert!(!h.is_saved());
        h.undo().unwrap();
        assert!(h.is_saved(), "undoing back to the save point is clean again");
        h.undo().unwrap();
        assert!(!h.is_saved());
        h.redo().unwrap();
        assert!(h.is_saved());
    }

    #[test]
    fn the_saved_state_is_lost_when_its_future_is_overwritten() {
        let mut h = History::new();
        typed(&mut h, 0, "a", 0);
        typed(&mut h, 5, "b", 5000);
        h.mark_saved(); // saved at depth 2
        h.undo().unwrap();
        h.undo().unwrap();
        typed(&mut h, 0, "x", 9000); // depth 1 now; the saved text ("ab") can never come back
        typed(&mut h, 9, "y", 12000); // depth 2 again, but different text
        assert!(!h.is_saved());
    }

    #[test]
    fn old_steps_are_dropped_past_the_limit() {
        let mut h = History::new();
        for i in 0..MAX_UNDO_GROUPS + 5 {
            let e = Edit { start: i, removed: String::new(), inserted: "x".into() };
            h.record(e, EditKind::Other, caret(i), caret(i + 1), 0);
        }
        let mut steps = 0;
        while h.undo().is_some() {
            steps += 1;
        }
        assert_eq!(steps, MAX_UNDO_GROUPS);
        assert!(!h.is_saved(), "the loaded state fell off the end of the history");
    }
}
