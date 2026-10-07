//! Line diff with word-level emphasis, and folding of unchanged regions.

use std::collections::HashSet;
use std::ops::Range;

use similar::{ChangeTag, DiffTag, TextDiff};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// 1-based line numbers in the old / new text.
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    /// Line text without the trailing newline.
    pub text: String,
    /// Byte ranges of `text` that changed within a modified line pair.
    pub emphasis: Vec<Range<usize>>,
}

/// A whole-file diff: every line of both texts, in unified order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diff {
    pub lines: Vec<DiffLine>,
}

fn strip_newline(s: &str) -> &str {
    s.strip_suffix('\n').map(|s| s.strip_suffix('\r').unwrap_or(s)).unwrap_or(s)
}

/// Lines less similar than this are treated as unrelated: no word emphasis (it would be noise).
const MIN_PAIR_SIMILARITY: f32 = 0.5;

/// Joins ranges separated only by whitespace in `text`, so a changed phrase reads as one block.
fn merge_ranges(text: &str, ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            Some(last) if text[last.end..r.start].trim().is_empty() => last.end = r.end,
            _ => out.push(r),
        }
    }
    out
}

/// Byte ranges that differ between two lines, word by word.
fn word_emphasis(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    if TextDiff::from_chars(old, new).ratio() < MIN_PAIR_SIMILARITY {
        return (Vec::new(), Vec::new());
    }
    let diff = TextDiff::from_words(old, new);
    let (mut o, mut n) = (Vec::new(), Vec::new());
    let (mut op, mut np) = (0, 0);
    for change in diff.iter_all_changes() {
        let len = change.value().len();
        match change.tag() {
            ChangeTag::Equal => {
                op += len;
                np += len;
            }
            ChangeTag::Delete => {
                if !change.value().trim().is_empty() {
                    o.push(op..op + len);
                }
                op += len;
            }
            ChangeTag::Insert => {
                if !change.value().trim().is_empty() {
                    n.push(np..np + len);
                }
                np += len;
            }
        }
    }
    (merge_ranges(old, o), merge_ranges(new, n))
}

pub fn diff_texts(old: &str, new: &str) -> Diff {
    let diff = TextDiff::from_lines(old, new);
    let old_lines: Vec<&str> = diff.old_slices().to_vec();
    let new_lines: Vec<&str> = diff.new_slices().to_vec();
    let mut lines = Vec::new();
    let ctx = |i: usize, j: usize| DiffLine {
        kind: LineKind::Context,
        old_no: Some(i as u32 + 1),
        new_no: Some(j as u32 + 1),
        text: strip_newline(new_lines[j]).to_string(),
        emphasis: Vec::new(),
    };
    let removed = |i: usize, emphasis| DiffLine {
        kind: LineKind::Removed,
        old_no: Some(i as u32 + 1),
        new_no: None,
        text: strip_newline(old_lines[i]).to_string(),
        emphasis,
    };
    let added = |j: usize, emphasis| DiffLine {
        kind: LineKind::Added,
        old_no: None,
        new_no: Some(j as u32 + 1),
        text: strip_newline(new_lines[j]).to_string(),
        emphasis,
    };
    for op in diff.ops() {
        let (tag, old_r, new_r) = op.as_tag_tuple();
        match tag {
            DiffTag::Equal => {
                for (i, j) in old_r.zip(new_r) {
                    lines.push(ctx(i, j));
                }
            }
            DiffTag::Delete => lines.extend(old_r.map(|i| removed(i, Vec::new()))),
            DiffTag::Insert => lines.extend(new_r.map(|j| added(j, Vec::new()))),
            DiffTag::Replace => {
                // Pair the i-th removed line with the i-th added line for word emphasis.
                let pairs = old_r.len().min(new_r.len());
                let mut old_emph = vec![Vec::new(); old_r.len()];
                let mut new_emph = vec![Vec::new(); new_r.len()];
                for k in 0..pairs {
                    let (o, n) = word_emphasis(
                        strip_newline(old_lines[old_r.start + k]),
                        strip_newline(new_lines[new_r.start + k]),
                    );
                    old_emph[k] = o;
                    new_emph[k] = n;
                }
                for (k, i) in old_r.enumerate() {
                    lines.push(removed(i, std::mem::take(&mut old_emph[k])));
                }
                for (k, j) in new_r.enumerate() {
                    lines.push(added(j, std::mem::take(&mut new_emph[k])));
                }
            }
        }
    }
    Diff { lines }
}

impl Diff {
    /// A diff with no changes, showing `text` as-is.
    pub fn unchanged(text: &str) -> Diff {
        diff_texts(text, text)
    }

    pub fn has_changes(&self) -> bool {
        self.lines.iter().any(|l| l.kind != LineKind::Context)
    }

    pub fn stats(&self) -> (usize, usize) {
        let added = self.lines.iter().filter(|l| l.kind == LineKind::Added).count();
        let removed = self.lines.iter().filter(|l| l.kind == LineKind::Removed).count();
        (added, removed)
    }

    /// Index (into `lines`) of the first line of each contiguous change block.
    pub fn hunk_starts(&self) -> Vec<usize> {
        let mut out = Vec::new();
        for (i, l) in self.lines.iter().enumerate() {
            let changed = l.kind != LineKind::Context;
            let prev_changed = i > 0 && self.lines[i - 1].kind != LineKind::Context;
            if changed && !prev_changed {
                out.push(i);
            }
        }
        out
    }

    /// Index of the line showing new-text line `line` (1-based), or the nearest one before it.
    pub fn index_of_new_line(&self, line: u32) -> usize {
        let mut best = 0;
        for (i, l) in self.lines.iter().enumerate() {
            match l.new_no {
                Some(n) if n <= line => best = i,
                Some(_) => break,
                None => {}
            }
        }
        best
    }
}

/// One row of the unified view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// Index into `Diff::lines`.
    Line(usize),
    /// A folded run of unchanged lines: `start` index and `len` lines.
    Fold { start: usize, len: usize },
}

/// Unified rows. Unchanged runs longer than `2 * context + 1` lines are folded, keeping `context`
/// lines around each change; folds whose `start` is in `expanded` are shown in full.
/// With no changes at all, nothing is folded.
pub fn unified_rows(diff: &Diff, context: usize, expanded: &HashSet<usize>) -> Vec<Row> {
    let n = diff.lines.len();
    if !diff.has_changes() {
        return (0..n).map(Row::Line).collect();
    }
    let mut keep = vec![false; n];
    for (i, l) in diff.lines.iter().enumerate() {
        if l.kind != LineKind::Context {
            for k in i.saturating_sub(context)..(i + context + 1).min(n) {
                keep[k] = true;
            }
        }
    }
    let mut rows = Vec::new();
    let mut i = 0;
    while i < n {
        if keep[i] {
            rows.push(Row::Line(i));
            i += 1;
            continue;
        }
        let start = i;
        while i < n && !keep[i] {
            i += 1;
        }
        let len = i - start;
        if len <= 1 || expanded.contains(&start) {
            rows.extend((start..i).map(Row::Line));
        } else {
            rows.push(Row::Fold { start, len });
        }
    }
    rows
}

/// One row of the side-by-side view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitRow {
    /// Indices into `Diff::lines` for the left (old) and right (new) cells.
    Pair { left: Option<usize>, right: Option<usize> },
    Fold { start: usize, len: usize },
}

/// Side-by-side rows derived from the unified rows: context lines appear on both sides;
/// each run of removed lines is paired row by row with the following run of added lines.
pub fn split_rows(diff: &Diff, context: usize, expanded: &HashSet<usize>) -> Vec<SplitRow> {
    let unified = unified_rows(diff, context, expanded);
    let mut out = Vec::new();
    let mut i = 0;
    while i < unified.len() {
        match unified[i] {
            Row::Fold { start, len } => {
                out.push(SplitRow::Fold { start, len });
                i += 1;
            }
            Row::Line(idx) if diff.lines[idx].kind == LineKind::Context => {
                out.push(SplitRow::Pair { left: Some(idx), right: Some(idx) });
                i += 1;
            }
            Row::Line(_) => {
                let (mut removed, mut added) = (Vec::new(), Vec::new());
                while let Some(Row::Line(idx)) = unified.get(i) {
                    match diff.lines[*idx].kind {
                        LineKind::Removed if added.is_empty() => removed.push(*idx),
                        LineKind::Added => added.push(*idx),
                        _ => break,
                    }
                    i += 1;
                }
                for k in 0..removed.len().max(added.len()) {
                    out.push(SplitRow::Pair { left: removed.get(k).copied(), right: added.get(k).copied() });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";

    fn kinds(d: &Diff) -> String {
        d.lines
            .iter()
            .map(|l| match l.kind {
                LineKind::Context => ' ',
                LineKind::Added => '+',
                LineKind::Removed => '-',
            })
            .collect()
    }

    #[test]
    fn replace_insert_delete() {
        let new = "a\nB\nc\nd\ne\nf\ng\nh\nx\ni\n";
        let d = diff_texts(OLD, new);
        assert_eq!(kinds(&d), " -+      + -");
        assert_eq!(d.stats(), (2, 2));
        let removed_b = &d.lines[1];
        assert_eq!((removed_b.old_no, removed_b.new_no, removed_b.text.as_str()), (Some(2), None, "b"));
        let added_x = d.lines.iter().find(|l| l.text == "x").unwrap();
        assert_eq!(added_x.new_no, Some(9));
        assert_eq!(d.hunk_starts(), vec![1, 9, 11]);
    }

    #[test]
    fn word_level_emphasis() {
        let d = diff_texts("let users = repo.All()\n", "let users = repo.Page(page)\n");
        let removed = &d.lines[0];
        let added = &d.lines[1];
        assert_eq!(&removed.text[removed.emphasis[0].clone()], "repo.All()");
        assert!(added.emphasis.iter().any(|r| added.text[r.clone()].contains("Page")));
        assert!(!added.emphasis.iter().any(|r| added.text[r.clone()].contains("users")));
    }

    #[test]
    fn unrelated_pairs_get_no_emphasis() {
        let d = diff_texts("users, err := repo.All()\n", "page := c.QueryInt(\"page\", 1)\n");
        assert!(d.lines.iter().all(|l| l.emphasis.is_empty()));
    }

    #[test]
    fn adjacent_changes_merge_across_spaces() {
        let d = diff_texts("func tail() {}\n", "func tail() { /* note here */ }\n");
        let added = &d.lines[1];
        assert_eq!(added.emphasis.len(), 1, "{:?}", added.emphasis);
        assert!(added.text[added.emphasis[0].clone()].contains("/* note here */"));
    }

    #[test]
    fn unchanged_text_is_not_folded() {
        let d = Diff::unchanged(OLD);
        assert!(!d.has_changes());
        assert_eq!(unified_rows(&d, 3, &HashSet::new()).len(), 10);
    }

    #[test]
    fn folds_long_unchanged_runs() {
        let new = OLD.replace("j\n", "J\n");
        let d = diff_texts(OLD, &new);
        let rows = unified_rows(&d, 2, &HashSet::new());
        assert_eq!(rows[0], Row::Fold { start: 0, len: 7 });
        assert_eq!(rows[1], Row::Line(7));
        assert_eq!(rows.len(), 1 + 2 + 2);
        let expanded: HashSet<usize> = [0].into();
        assert_eq!(unified_rows(&d, 2, &expanded).len(), 11, "9 context + 1 removed + 1 added");
    }

    #[test]
    fn split_pairs_removed_with_added() {
        let d = diff_texts("a\nb\nc\n", "a\nB\nB2\nc\n");
        let rows = split_rows(&d, 3, &HashSet::new());
        assert_eq!(
            rows,
            vec![
                SplitRow::Pair { left: Some(0), right: Some(0) },
                SplitRow::Pair { left: Some(1), right: Some(2) },
                SplitRow::Pair { left: None, right: Some(3) },
                SplitRow::Pair { left: Some(4), right: Some(4) },
            ]
        );
    }

    #[test]
    fn new_line_lookup() {
        let d = diff_texts("a\nb\nc\n", "a\nc\n");
        assert_eq!(d.index_of_new_line(2), 2);
        assert_eq!(d.index_of_new_line(1), 0);
        assert_eq!(d.index_of_new_line(99), 2);
    }

    #[test]
    fn new_file_is_all_added() {
        let d = diff_texts("", "x\ny\n");
        assert_eq!(kinds(&d), "++");
    }
}
