//! Navigation over displayed rows: change-block starts and locating a line.

use crate::diff::{Diff, LineKind, Row, SplitRow};

/// Row indices where a run of changed rows begins.
pub fn change_starts(len: usize, is_changed: impl Fn(usize) -> bool) -> Vec<usize> {
    (0..len).filter(|&i| is_changed(i) && (i == 0 || !is_changed(i - 1))).collect()
}

pub fn unified_changed(diff: &Diff, row: &Row) -> bool {
    matches!(row, Row::Line(i) if diff.lines[*i].kind != LineKind::Context)
}

pub fn split_changed(diff: &Diff, row: &SplitRow) -> bool {
    match row {
        SplitRow::Pair { left, right } => [left, right]
            .into_iter()
            .flatten()
            .any(|&i| diff.lines[i].kind != LineKind::Context),
        SplitRow::Fold { .. } => false,
    }
}

/// First change start strictly below `top` (the row currently at the top of the view).
pub fn next_change(starts: &[usize], top: usize) -> Option<usize> {
    starts.iter().copied().find(|&s| s > top)
}

/// Last change start strictly above `top`.
pub fn prev_change(starts: &[usize], top: usize) -> Option<usize> {
    starts.iter().copied().rev().find(|&s| s < top)
}

/// Unified row showing diff line `index` (or the fold containing it).
pub fn unified_row_of(rows: &[Row], index: usize) -> usize {
    rows.iter()
        .position(|r| match *r {
            Row::Line(i) => i >= index,
            Row::Fold { start, len } => index < start + len,
        })
        .unwrap_or(rows.len().saturating_sub(1))
}

/// `start` of the fold hiding diff line `index`, if it is folded away.
pub fn fold_containing(rows: &[Row], index: usize) -> Option<usize> {
    rows.iter().find_map(|r| match *r {
        Row::Fold { start, len } if (start..start + len).contains(&index) => Some(start),
        _ => None,
    })
}

/// Split row showing diff line `index` on either side (or the fold containing it).
pub fn split_row_of(rows: &[SplitRow], index: usize) -> usize {
    rows.iter()
        .position(|r| match *r {
            SplitRow::Pair { left, right } => left.max(right).is_some_and(|i| i >= index),
            SplitRow::Fold { start, len } => index < start + len,
        })
        .unwrap_or(rows.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{diff_texts, split_rows, unified_rows};
    use std::collections::HashSet;

    #[test]
    fn change_starts_group_runs() {
        let changed = [false, true, true, false, true, false];
        assert_eq!(change_starts(changed.len(), |i| changed[i]), vec![1, 4]);
    }

    #[test]
    fn next_and_prev() {
        let starts = [3, 10, 20];
        assert_eq!(next_change(&starts, 0), Some(3));
        assert_eq!(next_change(&starts, 3), Some(10));
        assert_eq!(next_change(&starts, 20), None);
        assert_eq!(prev_change(&starts, 10), Some(3));
        assert_eq!(prev_change(&starts, 3), None);
    }

    #[test]
    fn locating_lines_in_folded_rows() {
        let old: String = (1..=30).map(|i| format!("l{i}\n")).collect();
        let new = old.replace("l25\n", "L25\n");
        let d = diff_texts(&old, &new);
        let rows = unified_rows(&d, 3, &HashSet::new());
        // Line 5 is folded away: it maps to the fold row.
        assert!(matches!(rows[unified_row_of(&rows, 4)], Row::Fold { .. }));
        let start = fold_containing(&rows, 4).unwrap();
        assert_eq!(fold_containing(&rows, d.index_of_new_line(25)), None);
        // Expanding that fold puts line 5 on its own row.
        let rows = unified_rows(&d, 3, &HashSet::from([start]));
        assert_eq!(fold_containing(&rows, 4), None);
        assert_eq!(rows[unified_row_of(&rows, 4)], Row::Line(4));
        let split = split_rows(&d, 3, &HashSet::from([start]));
        assert!(matches!(split[split_row_of(&split, 4)], SplitRow::Pair { right: Some(4), .. }));
        let starts = change_starts(rows.len(), |i| unified_changed(&d, &rows[i]));
        assert_eq!(starts.len(), 1);
        let split = split_rows(&d, 3, &HashSet::new());
        let s = change_starts(split.len(), |i| split_changed(&d, &split[i]));
        assert_eq!(s.len(), 1);
        assert!(split_changed(&d, &split[s[0]]));
        assert_eq!(split_row_of(&split, d.index_of_new_line(25)), s[0]);
    }
}
