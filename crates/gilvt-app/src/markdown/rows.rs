//! Rows of the rendered view — top-level blocks interleaved with deleted runs — and conversions
//! between rows, source lines and change-rail fractions.

use gilvt_markdown::{block_at_line, Block, ChangeKind, Changes, Lines};
use gilvt_viewer::Diff;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// Top-level block index.
    Block(usize),
    /// Index into `Changes::deleted`.
    Deleted(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Added,
    Modified,
    Deleted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowInfo {
    pub row: Row,
    /// Source lines the row stands for; a deleted run sits on the line after the block it follows.
    pub lines: Lines,
    pub mark: Option<Mark>,
}

/// Blocks in order, each deleted run placed right after the block it follows.
pub fn build_rows(blocks: &[Block], changes: Option<&Changes>) -> Vec<RowInfo> {
    let deleted = changes.map_or(&[][..], |c| &c.deleted[..]);
    let mut rows = Vec::with_capacity(blocks.len() + deleted.len());
    let mut pending = deleted.iter().enumerate().peekable();
    let mut push_deleted = |after: Option<usize>, rows: &mut Vec<RowInfo>| {
        let line = after.map_or(1, |b| blocks[b].lines.end + 1);
        while let Some((i, _)) = pending.next_if(|(_, d)| d.after.is_none_or(|a| after.is_some_and(|b| a <= b))) {
            rows.push(RowInfo { row: Row::Deleted(i), lines: Lines { start: line, end: line }, mark: Some(Mark::Deleted) });
        }
    };
    push_deleted(None, &mut rows);
    for (b, block) in blocks.iter().enumerate() {
        let mark = match changes.map(|c| c.blocks[b]) {
            Some(ChangeKind::Added) => Some(Mark::Added),
            Some(ChangeKind::Modified) => Some(Mark::Modified),
            _ => None,
        };
        rows.push(RowInfo { row: Row::Block(b), lines: block.lines, mark });
        push_deleted(Some(b), &mut rows);
    }
    rows
}

/// Row showing top-level block `block` (0 if absent).
pub fn row_of_block(rows: &[RowInfo], block: usize) -> usize {
    rows.iter().position(|r| r.row == Row::Block(block)).unwrap_or(0)
}

/// Row of the block containing `line` (see `block_at_line`).
pub fn row_of_line(rows: &[RowInfo], blocks: &[Block], line: u32) -> usize {
    row_of_block(rows, block_at_line(blocks, line))
}

/// First source line of row `row` (1 when out of range).
pub fn line_of_row(rows: &[RowInfo], row: usize) -> u32 {
    rows.get(row).map_or(1, |r| r.lines.start)
}

/// Rows `n` / `p` stop at: every changed block and every deleted run.
pub fn change_stops(rows: &[RowInfo]) -> Vec<usize> {
    rows.iter().enumerate().filter(|(_, r)| r.mark.is_some()).map(|(i, _)| i).collect()
}

/// Stop to jump to from `top` (the row at the top of the view). A partly scrolled-away top row
/// counts as passed going backwards, so `p` first returns to its start.
pub fn jump_target(stops: &[usize], top: usize, top_partly_hidden: bool, forward: bool) -> Option<usize> {
    if forward {
        stops.iter().copied().find(|&s| s > top)
    } else {
        let current = if top_partly_hidden { top + 1 } else { top };
        stops.iter().copied().rev().find(|&s| s < current)
    }
}

/// Change-rail marks as `(top, bottom, mark)` fractions of the source; deleted runs have no extent.
/// Same-kind blocks separated by at most one line merge into one mark (fewer elements per frame).
pub fn rail_marks(rows: &[RowInfo], total_lines: u32) -> Vec<(f32, f32, Mark)> {
    let total = total_lines.max(1) as f32;
    let mut out: Vec<(f32, f32, Mark)> = Vec::new();
    for r in rows {
        let Some(mark) = r.mark else { continue };
        let top = ((r.lines.start - 1) as f32 / total).min(1.0);
        let bottom = if mark == Mark::Deleted { top } else { (r.lines.end as f32 / total).min(1.0) };
        match out.last_mut() {
            Some(last) if mark != Mark::Deleted && last.2 == mark && top - last.1 <= 1.0 / total + f32::EPSILON => last.1 = bottom,
            _ => out.push((top, bottom, mark)),
        }
    }
    out
}

/// Rail viewport `(top, bottom)` fractions for rows `first..=last` being visible.
pub fn viewport(rows: &[RowInfo], first: usize, last: usize, total_lines: u32) -> (f32, f32) {
    let total = total_lines.max(1) as f32;
    let (Some(a), Some(b)) = (rows.get(first), rows.get(last.max(first))) else { return (0.0, 1.0) };
    let top = (a.lines.start - 1) as f32 / total;
    let bottom = (b.lines.end.max(a.lines.start) as f32 / total).max(top);
    (top.min(1.0), bottom.min(1.0))
}

/// Source line at `fraction` (0..=1) of the rail.
pub fn line_at_fraction(fraction: f32, total_lines: u32) -> u32 {
    ((fraction.clamp(0.0, 1.0) * total_lines as f32) as u32 + 1).min(total_lines.max(1))
}

/// New-file line shown at diff line `index`: its own, else the next line that has one, else the last.
pub fn new_line_at(diff: &Diff, index: usize) -> u32 {
    diff.lines[index.min(diff.lines.len())..]
        .iter()
        .find_map(|l| l.new_no)
        .or_else(|| diff.lines.iter().rev().find_map(|l| l.new_no))
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_markdown::{map_changes, parse};
    use gilvt_viewer::diff::diff_texts;

    const OLD: &str = "# Title\n\nGone para.\n\nKept para.\n\nEdited para.\n\nTail.\n";
    const NEW: &str = "# Title\n\nKept para.\n\nEdited paragraph.\n\nTail.\n\nNew para.\n";

    fn doc() -> (Vec<Block>, Vec<RowInfo>) {
        let blocks = parse(NEW);
        let changes = map_changes(&blocks, &diff_texts(OLD, NEW));
        let rows = build_rows(&blocks, Some(&changes));
        (blocks, rows)
    }

    #[test]
    fn deleted_runs_sit_between_blocks() {
        let (_, rows) = doc();
        let kinds: Vec<(Row, Option<Mark>)> = rows.iter().map(|r| (r.row, r.mark)).collect();
        assert_eq!(
            kinds,
            [
                (Row::Block(0), None),
                (Row::Deleted(0), Some(Mark::Deleted)),
                (Row::Block(1), None),
                (Row::Block(2), Some(Mark::Modified)),
                (Row::Block(3), None),
                (Row::Block(4), Some(Mark::Added)),
            ]
        );
        assert_eq!(rows[1].lines, Lines { start: 2, end: 2 }, "on the line after the title");
    }

    #[test]
    fn deletion_before_the_first_block_leads() {
        let blocks = parse("B\n");
        let changes = map_changes(&blocks, &diff_texts("A\n\nB\n", "B\n"));
        let rows = build_rows(&blocks, Some(&changes));
        assert_eq!(rows.iter().map(|r| r.row).collect::<Vec<_>>(), [Row::Deleted(0), Row::Block(0)]);
        assert_eq!(rows[0].lines.start, 1);
    }

    #[test]
    fn file_only_has_no_marks() {
        let blocks = parse(NEW);
        let rows = build_rows(&blocks, None);
        assert_eq!(rows.len(), blocks.len());
        assert!(change_stops(&rows).is_empty());
    }

    #[test]
    fn rows_and_lines_round_trip() {
        let (blocks, rows) = doc();
        assert_eq!(row_of_line(&rows, &blocks, 5), 3, "line 5 is the edited paragraph");
        assert_eq!(row_of_line(&rows, &blocks, 2), 0, "a blank line belongs to the block before it");
        for (i, r) in rows.iter().enumerate() {
            if let Row::Block(_) = r.row {
                assert_eq!(row_of_line(&rows, &blocks, line_of_row(&rows, i)), i);
            }
        }
        assert_eq!(line_of_row(&rows, 99), 1);
        assert_eq!(row_of_block(&rows, 4), 5);
    }

    #[test]
    fn change_navigation_includes_deleted_runs() {
        let (_, rows) = doc();
        let stops = change_stops(&rows);
        assert_eq!(stops, [1, 3, 5]);
        assert_eq!(jump_target(&stops, 0, false, true), Some(1));
        assert_eq!(jump_target(&stops, 1, false, true), Some(3));
        assert_eq!(jump_target(&stops, 5, false, true), None);
        assert_eq!(jump_target(&stops, 5, false, false), Some(3));
        assert_eq!(jump_target(&stops, 3, true, false), Some(3), "back to the start of a partly hidden stop");
        assert_eq!(jump_target(&stops, 1, false, false), None);
    }

    #[test]
    fn rail_fractions() {
        let (_, rows) = doc();
        // NEW has 9 lines.
        let marks = rail_marks(&rows, 9);
        assert_eq!(marks, [(1.0 / 9.0, 1.0 / 9.0, Mark::Deleted), (4.0 / 9.0, 5.0 / 9.0, Mark::Modified), (8.0 / 9.0, 1.0, Mark::Added)]);
        assert_eq!(viewport(&rows, 0, 3, 9), (0.0, 5.0 / 9.0));
        assert_eq!(viewport(&[], 0, 0, 9), (0.0, 1.0));
        assert_eq!(line_at_fraction(0.0, 9), 1);
        assert_eq!(line_at_fraction(0.5, 9), 5);
        assert_eq!(line_at_fraction(1.0, 9), 9);
        assert_eq!(line_at_fraction(0.3, 0), 1);
    }

    #[test]
    fn adjacent_marks_merge() {
        let new = "a\n\nb\n\nc\n\nd\n";
        let blocks = parse(new);
        let changes = map_changes(&blocks, &diff_texts("d\n", new));
        let marks = rail_marks(&build_rows(&blocks, Some(&changes)), 7);
        assert_eq!(marks, [(0.0, 5.0 / 7.0, Mark::Added)]);
    }

    #[test]
    fn diff_index_to_new_line() {
        let diff = diff_texts("a\nb\nc\n", "a\nc\nd\n");
        // Lines: a (ctx), b (removed), c (ctx), d (added).
        assert_eq!(new_line_at(&diff, 0), 1);
        assert_eq!(new_line_at(&diff, 1), 2, "a removed line maps to the next surviving one");
        assert_eq!(new_line_at(&diff, 3), 3);
        assert_eq!(new_line_at(&diff, 99), 3);
        assert_eq!(new_line_at(&diff_texts("a\n", ""), 0), 1);
    }
}
