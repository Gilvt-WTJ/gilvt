//! Documents left by following links, for `⌘[`.

use crate::preview_view::Source;

/// Where the view was before a link was followed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub sources: Vec<Source>,
    pub index: usize,
    /// Source line at the top of the view.
    pub line: u32,
}

/// Oldest entries are dropped beyond this depth.
const MAX_DEPTH: usize = 100;

#[derive(Default)]
pub struct History {
    entries: Vec<Entry>,
}

impl History {
    pub fn push(&mut self, entry: Entry) {
        if self.entries.len() == MAX_DEPTH {
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }

    pub fn pop(&mut self) -> Option<Entry> {
        self.entries.pop()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(name: &str, line: u32) -> Entry {
        Entry { sources: vec![Source::File(PathBuf::from(name))], index: 0, line }
    }

    #[test]
    fn last_in_first_out() {
        let mut h = History::default();
        assert!(h.is_empty());
        h.push(entry("a.md", 1));
        h.push(entry("b.md", 40));
        assert_eq!(h.pop(), Some(entry("b.md", 40)));
        assert_eq!(h.pop(), Some(entry("a.md", 1)));
        assert_eq!(h.pop(), None);
    }

    #[test]
    fn depth_is_bounded() {
        let mut h = History::default();
        for i in 0..=MAX_DEPTH as u32 {
            h.push(entry("a.md", i + 1));
        }
        let mut n = 0;
        let mut last = None;
        while let Some(e) = h.pop() {
            n += 1;
            last = Some(e.line);
        }
        assert_eq!(n, MAX_DEPTH);
        assert_eq!(last, Some(2), "the oldest entry was dropped");
    }
}
