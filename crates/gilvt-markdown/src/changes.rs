//! Mapping a line diff onto top-level blocks, and block ↔ source line lookups.

use gilvt_viewer::{Diff, DiffLine, LineKind};

use crate::model::Block;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Unchanged,
    Added,
    Modified,
}

/// Removed source lines that no longer fall inside any block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deleted {
    /// Top-level block index the lines were removed after; `None` = before the first block.
    pub after: Option<usize>,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changes {
    /// One entry per top-level block.
    pub blocks: Vec<ChangeKind>,
    /// In document order.
    pub deleted: Vec<Deleted>,
}

/// Top-level block owning each new-file line (index `line - 1`); the earlier block wins on overlap.
fn owners(blocks: &[Block]) -> Vec<Option<usize>> {
    let len = blocks.iter().map(|b| b.lines.end).max().unwrap_or(0);
    let mut owner = vec![None; len as usize];
    for (i, b) in blocks.iter().enumerate().rev() {
        owner[b.lines.start as usize - 1..b.lines.end as usize].fill(Some(i));
    }
    owner
}

pub fn map_changes(blocks: &[Block], diff: &Diff) -> Changes {
    let owner = owners(blocks);
    let owner_of = |line: Option<u32>| line.and_then(|l| owner.get(l as usize - 1).copied().flatten());
    let mut added = vec![0u32; blocks.len()];
    let mut removed = vec![false; blocks.len()];
    let mut deleted = Vec::new();
    let lines = &diff.lines;
    let mut i = 0;
    while i < lines.len() {
        match lines[i].kind {
            LineKind::Context => i += 1,
            LineKind::Added => {
                if let Some(b) = owner_of(lines[i].new_no) {
                    added[b] += 1;
                }
                i += 1;
            }
            LineKind::Removed => {
                let end = lines[i..].iter().position(|l| l.kind != LineKind::Removed).map_or(lines.len(), |n| i + n);
                let next = lines.get(end);
                // A replacement modifies the block receiving its first added line.
                if let Some(b) = next.filter(|l| l.kind == LineKind::Added).and_then(|l| owner_of(l.new_no)) {
                    removed[b] = true;
                } else {
                    let prev = lines[..i].last().and_then(|l| l.new_no);
                    let next = next.and_then(|l| l.new_no);
                    match (owner_of(prev), owner_of(next)) {
                        (Some(a), Some(b)) if a == b => removed[a] = true,
                        _ => deleted.push(Deleted { after: block_ending_by(blocks, prev), lines: texts(&lines[i..end]) }),
                    }
                }
                i = end;
            }
        }
    }
    let mut owned = vec![0u32; blocks.len()];
    for b in owner.iter().flatten() {
        owned[*b] += 1;
    }
    let kinds = (0..blocks.len())
        .map(|b| match (added[b], removed[b]) {
            (0, false) => ChangeKind::Unchanged,
            (n, false) if n == owned[b] => ChangeKind::Added,
            _ => ChangeKind::Modified,
        })
        .collect();
    Changes { blocks: kinds, deleted }
}

/// The block ending last at or before `line`.
fn block_ending_by(blocks: &[Block], line: Option<u32>) -> Option<usize> {
    let line = line?;
    blocks.iter().enumerate().filter(|(_, b)| b.lines.end <= line).max_by_key(|(_, b)| b.lines.end).map(|(i, _)| i)
}

fn texts(lines: &[DiffLine]) -> Vec<String> {
    lines.iter().map(|l| l.text.clone()).collect()
}

/// The block containing `line`, else the block starting last before it, else 0.
pub fn block_at_line(blocks: &[Block], line: u32) -> usize {
    blocks
        .iter()
        .position(|b| b.lines.contains(line))
        .or_else(|| {
            let before = blocks.iter().enumerate().filter(|(_, b)| b.lines.start < line);
            before.max_by_key(|(_, b)| b.lines.start).map(|(i, _)| i)
        })
        .unwrap_or(0)
}

/// First source line of block `index`; 1 if out of range.
pub fn line_of_block(blocks: &[Block], index: usize) -> u32 {
    blocks.get(index).map_or(1, |b| b.lines.start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use gilvt_viewer::diff::diff_texts;
    use ChangeKind::{Added, Modified, Unchanged};

    fn changes(old: &str, new: &str) -> Changes {
        map_changes(&parse(new), &diff_texts(old, new))
    }

    fn deleted(after: Option<usize>, lines: &[&str]) -> Deleted {
        Deleted { after, lines: lines.iter().map(|l| l.to_string()).collect() }
    }

    const DOC: &str = "# Title\n\nFirst para.\n\nSecond para\nspans lines\nthree of them.\n\nLast para.\n";

    #[test]
    fn unchanged_document() {
        let c = changes(DOC, DOC);
        assert_eq!(c, Changes { blocks: vec![Unchanged; 4], deleted: vec![] });
    }

    #[test]
    fn new_paragraph_is_added() {
        let new = DOC.replace("First para.\n", "First para.\n\nInserted\nparagraph.\n");
        let c = changes(DOC, &new);
        assert_eq!(c.blocks, [Unchanged, Unchanged, Added, Unchanged, Unchanged]);
        assert!(c.deleted.is_empty());
    }

    #[test]
    fn changed_word_modifies_without_deleting() {
        let c = changes(DOC, &DOC.replace("spans lines", "covers lines"));
        assert_eq!(c.blocks, [Unchanged, Unchanged, Modified, Unchanged]);
        assert!(c.deleted.is_empty());
    }

    #[test]
    fn line_removed_inside_paragraph_modifies_it() {
        let c = changes(DOC, &DOC.replace("spans lines\n", ""));
        assert_eq!(c.blocks, [Unchanged, Unchanged, Modified, Unchanged]);
        assert!(c.deleted.is_empty());
    }

    #[test]
    fn removed_paragraph_is_deleted_after_its_predecessor() {
        let c = changes(DOC, &DOC.replace("Second para\nspans lines\nthree of them.\n\n", ""));
        assert_eq!(c.blocks, [Unchanged, Unchanged, Unchanged]);
        assert_eq!(c.deleted, [deleted(Some(1), &["Second para", "spans lines", "three of them.", ""])]);
    }

    #[test]
    fn deletion_at_start() {
        let c = changes(DOC, &DOC.replace("# Title\n\n", ""));
        assert_eq!(c.blocks, [Unchanged; 3]);
        assert_eq!(c.deleted, [deleted(None, &["# Title", ""])]);
    }

    #[test]
    fn deletion_at_end() {
        let c = changes(DOC, &DOC.replace("\nLast para.\n", ""));
        assert_eq!(c.blocks, [Unchanged; 3]);
        assert_eq!(c.deleted, [deleted(Some(2), &["", "Last para."])]);
    }

    #[test]
    fn new_file_is_all_added() {
        let diff = diff_texts("", DOC);
        assert!(diff.lines.iter().all(|l| l.kind == LineKind::Added));
        let c = map_changes(&parse(DOC), &diff);
        assert_eq!(c, Changes { blocks: vec![Added; 4], deleted: vec![] });
    }

    #[test]
    fn appended_item_modifies_the_list() {
        let old = "- a\n- b\n";
        let c = changes(old, "- a\n- b\n- c\n");
        assert_eq!(c.blocks, [Modified]);
    }

    #[test]
    fn replacement_marks_the_block_receiving_it() {
        // Two old lines replaced by one: the paragraph is modified, nothing is listed as deleted.
        let c = changes(DOC, &DOC.replace("Second para\nspans lines\n", "Rewritten\n"));
        assert_eq!(c.blocks, [Unchanged, Unchanged, Modified, Unchanged]);
        assert!(c.deleted.is_empty());
    }

    #[test]
    fn footnote_definitions_moved_to_end_still_count_as_one_added_block() {
        let new = "Text[^n].\n\n[^n]: Note.\n\n# After\n";
        let c = map_changes(&parse(new), &diff_texts("", new));
        assert_eq!(c.blocks, [Added, Added, Added]);
    }

    fn doc() -> Vec<Block> {
        // Blocks at lines 1, 3, 5–7, 9.
        parse(DOC)
    }

    #[test]
    fn block_at_line_lookup() {
        let blocks = doc();
        assert_eq!(block_at_line(&blocks, 1), 0);
        assert_eq!(block_at_line(&blocks, 2), 0, "blank line: the block before it");
        assert_eq!(block_at_line(&blocks, 6), 2);
        assert_eq!(block_at_line(&blocks, 8), 2);
        assert_eq!(block_at_line(&blocks, 9), 3);
        assert_eq!(block_at_line(&blocks, 99), 3);
        assert_eq!(block_at_line(&blocks, 0), 0);
        assert_eq!(block_at_line(&[], 5), 0);
        assert_eq!(block_at_line(&parse("\n\n# H\n"), 1), 0, "before the first block");
    }

    #[test]
    fn block_at_line_prefers_body_over_trailing_footnotes() {
        let blocks = parse("[^n]: Note.\n\nBody[^n].\n\nMore.\n");
        assert_eq!(block_at_line(&blocks, 1), 2);
        assert_eq!(block_at_line(&blocks, 4), 0);
        assert_eq!(block_at_line(&blocks, 6), 1);
    }

    #[test]
    fn line_of_block_lookup() {
        let blocks = doc();
        assert_eq!(line_of_block(&blocks, 0), 1);
        assert_eq!(line_of_block(&blocks, 2), 5);
        assert_eq!(line_of_block(&blocks, 4), 1);
        assert_eq!(line_of_block(&[], 0), 1);
        for (i, _) in blocks.iter().enumerate() {
            assert_eq!(block_at_line(&blocks, line_of_block(&blocks, i)), i);
        }
    }
}
