//! Soft-wrap map: logical lines <-> display rows. Pure logic; lines are cut by terminal-cell width
//! (a wide character is 2 cells), never inside a grapheme cluster, and continuation rows of an
//! indented or list line stay aligned under the line's text.

use gilvt_editor::{cell_width, Change};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub const MIN_WIDTH: usize = 8;

/// One display row: graphemes `start..end` of logical `line`, drawn `indent` columns from the left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub indent: usize,
}

pub struct WrapMap {
    width: usize,
    tab: usize,
    rows: Vec<Row>,
    /// Index into `rows` of each logical line's first row.
    first: Vec<usize>,
}

fn is_blank(g: &str) -> bool {
    g == " " || g == "\t"
}

fn is_space(g: &str) -> bool {
    g.chars().all(char::is_whitespace)
}

/// Columns a continuation row is indented: the line's leading blanks plus a list marker, capped at half the width.
fn continuation_indent(gs: &[&str], width: usize, tab: usize) -> usize {
    let mut p = 0;
    while p < gs.len() && is_blank(gs[p]) {
        p += 1;
    }
    let ws_end = p;
    let marker_end = if p < gs.len() && matches!(gs[p], "-" | "*" | "+") {
        p + 1
    } else {
        let mut q = p;
        while q < gs.len() && gs[q].chars().all(|c| c.is_ascii_digit()) {
            q += 1;
        }
        if q > p && q < gs.len() && matches!(gs[q], "." | ")") { q + 1 } else { p }
    };
    let prefix_end = if marker_end > p && marker_end < gs.len() && is_blank(gs[marker_end]) {
        let mut e = marker_end;
        while e < gs.len() && is_blank(gs[e]) {
            e += 1;
        }
        e
    } else {
        ws_end
    };
    let mut col = 0;
    for g in &gs[..prefix_end] {
        col += cell_width(g, col, tab);
    }
    col.min(width / 2)
}

fn break_line(line: usize, text: &str, width: usize, tab: usize) -> Vec<Row> {
    let gs: Vec<&str> = text.graphemes(true).collect();
    if gs.is_empty() {
        return vec![Row { line, start: 0, end: 0, indent: 0 }];
    }
    let indent = continuation_indent(&gs, width, tab);
    let mut rows = Vec::new();
    let (mut start, mut row_indent, mut col) = (0usize, 0usize, 0usize);
    let mut last_break: Option<usize> = None;
    let mut i = 0;
    while i < gs.len() {
        let w = cell_width(gs[i], col, tab);
        if col + w > width && i > start {
            let cut = match last_break {
                Some(b) if b > start => b,
                _ => i,
            };
            rows.push(Row { line, start, end: cut, indent: row_indent });
            start = cut;
            row_indent = indent;
            (i, col, last_break) = (start, indent, None);
            continue;
        }
        col += w;
        if is_space(gs[i]) {
            last_break = Some(i + 1);
        }
        i += 1;
    }
    rows.push(Row { line, start, end: gs.len(), indent: row_indent });
    rows
}

impl WrapMap {
    pub fn new(width: usize, tab: usize) -> Self {
        Self { width: width.max(MIN_WIDTH), tab: tab.max(1), rows: Vec::new(), first: Vec::new() }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn rebuild(&mut self, line_count: usize, text: &dyn Fn(usize) -> String) {
        self.rows.clear();
        self.first.clear();
        for l in 0..line_count {
            self.first.push(self.rows.len());
            self.rows.extend(break_line(l, &text(l), self.width, self.tab));
        }
    }

    pub fn set_width(&mut self, width: usize, line_count: usize, text: &dyn Fn(usize) -> String) {
        self.width = width.max(MIN_WIDTH);
        self.rebuild(line_count, text);
    }

    /// Re-wraps only the lines `change` replaced. `line_count` is the count AFTER the change.
    /// A change that does not match the map's current line count triggers a full rebuild.
    pub fn apply(&mut self, change: Change, line_count: usize, text: &dyn Fn(usize) -> String) {
        let Change { start_line: start, old_lines: old, new_lines: new } = change;
        let old_total = line_count + old - new.min(line_count + old);
        if self.first.len() != old_total || start + old > old_total || start + new > line_count {
            return self.rebuild(line_count, text);
        }
        let a = self.first[start];
        let b = if start + old < old_total { self.first[start + old] } else { self.rows.len() };
        let mut fresh = Vec::new();
        let mut counts = Vec::with_capacity(new);
        for l in start..start + new {
            let r = break_line(l, &text(l), self.width, self.tab);
            counts.push(r.len());
            fresh.extend(r);
        }
        let delta = fresh.len() as isize - (b - a) as isize;
        let mut first = Vec::with_capacity(line_count);
        first.extend_from_slice(&self.first[..start]);
        let mut at = a;
        for c in counts {
            first.push(at);
            at += c;
        }
        for l in start + old..old_total {
            first.push((self.first[l] as isize + delta) as usize);
        }
        self.rows.splice(a..b, fresh);
        // Rows after the splice belong to shifted logical lines.
        let line_delta = new as isize - old as isize;
        if line_delta != 0 {
            for r in &mut self.rows[a + (at - a)..] {
                r.line = (r.line as isize + line_delta) as usize;
            }
        }
        self.first = first;
    }

    pub fn total_rows(&self) -> usize {
        self.rows.len()
    }

    pub fn row(&self, i: usize) -> Row {
        self.rows[i.min(self.rows.len().saturating_sub(1))]
    }

    pub fn rows_of_line(&self, line: usize) -> Range<usize> {
        let a = self.first.get(line).copied().unwrap_or(self.rows.len());
        let b = self.first.get(line + 1).copied().unwrap_or(self.rows.len());
        a..b
    }

    pub fn is_last_row(&self, i: usize) -> bool {
        i + 1 >= self.rows.len() || self.rows[i + 1].line != self.rows[i].line
    }

    /// The row that shows the caret at grapheme `col` of `line`; a wrap boundary belongs to the row after it.
    pub fn row_of(&self, line: usize, col: usize) -> usize {
        let r = self.rows_of_line(line);
        if r.is_empty() {
            return self.rows.len().saturating_sub(1);
        }
        for i in r.clone() {
            if col < self.rows[i].end || i + 1 == r.end {
                return i;
            }
        }
        r.end - 1
    }

    /// Cells from the row's left edge to grapheme `col` (clamped to the row).
    pub fn visual_col(&self, row: usize, line_text: &str, col: usize) -> usize {
        let r = self.row(row);
        let mut acc = r.indent;
        for g in line_text.graphemes(true).skip(r.start).take(col.clamp(r.start, r.end) - r.start) {
            acc += cell_width(g, acc, self.tab);
        }
        acc
    }

    /// The grapheme boundary nearest to pixel-column `x` (in cells, may be fractional) on `row`.
    /// On a row that is not the last of its line the caret never lands past the row (that spot is the next row's start).
    pub fn col_at(&self, row: usize, line_text: &str, x: f32) -> usize {
        let r = self.row(row);
        let mut acc = r.indent;
        for (i, g) in line_text.graphemes(true).enumerate().skip(r.start).take(r.end - r.start) {
            let w = cell_width(g, acc, self.tab);
            if x < acc as f32 + w as f32 / 2.0 {
                return i;
            }
            acc += w;
        }
        if self.is_last_row(row.min(self.rows.len().saturating_sub(1))) { r.end } else { r.end.saturating_sub(1).max(r.start) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(lines: &[&str], width: usize) -> (WrapMap, Vec<String>) {
        let owned: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        let mut m = WrapMap::new(width, 4);
        m.rebuild(owned.len(), &|l| owned[l].clone());
        (m, owned)
    }
    /// (line, start, end, indent) of every row.
    fn rows(m: &WrapMap) -> Vec<(usize, usize, usize, usize)> {
        (0..m.total_rows()).map(|i| { let r = m.row(i); (r.line, r.start, r.end, r.indent) }).collect()
    }

    #[test]
    fn short_and_empty_lines_are_one_row() {
        assert_eq!(rows(&build(&["hello"], 40).0), [(0, 0, 5, 0)]);
        assert_eq!(rows(&build(&[""], 40).0), [(0, 0, 0, 0)]);
    }

    #[test]
    fn breaks_after_the_last_space_that_fits() {
        assert_eq!(rows(&build(&["aaaa bbbb cccc"], 10).0), [(0, 0, 10, 0), (0, 10, 14, 0)]);
    }

    #[test]
    fn a_long_word_is_cut_at_the_width() {
        assert_eq!(rows(&build(&["abcdefghijklmnopqrstuvwxyz"], 10).0), [(0, 0, 10, 0), (0, 10, 20, 0), (0, 20, 26, 0)]);
    }

    #[test]
    fn wide_characters_are_never_split() {
        assert_eq!(rows(&build(&["你好世界你好世界"], 9).0), [(0, 0, 4, 0), (0, 4, 8, 0)]);
        assert_eq!(rows(&build(&["abcdefghi你"], 10).0), [(0, 0, 9, 0), (0, 9, 10, 0)]);
    }

    #[test]
    fn grapheme_clusters_are_never_split() {
        let line = "e\u{301}".repeat(12);
        assert_eq!(rows(&build(&[line.as_str()], 10).0), [(0, 0, 10, 0), (0, 10, 12, 0)]);
        let family = "👨\u{200d}👩\u{200d}👧".repeat(6);
        let (m, _) = build(&[family.as_str()], 10);
        assert!(m.total_rows() >= 2);
        assert_eq!(m.row(m.total_rows() - 1).end, 6); // 6 clusters, none torn
    }

    #[test]
    fn list_items_continue_under_their_text() {
        assert_eq!(rows(&build(&["- aaaa bbbb cccc dddd"], 12).0), [(0, 0, 12, 0), (0, 12, 21, 2)]);
        let (m, _) = build(&["12. aaaa bbbb cccc dddd eeee"], 14);
        assert_eq!(m.row(1).indent, 4);
    }

    #[test]
    fn indent_is_capped_at_half_the_width() {
        let line = format!("{}text that wraps around", " ".repeat(20));
        let (m, _) = build(&[line.as_str()], 10);
        assert!(m.total_rows() > 1);
        assert_eq!(m.row(1).indent, 5);
    }

    #[test]
    fn visual_col_and_col_at_agree_on_wide_chars() {
        let (m, t) = build(&["ab你cd"], 40);
        assert_eq!(m.visual_col(0, &t[0], 3), 4);
        assert_eq!(m.col_at(0, &t[0], 2.9), 2);
        assert_eq!(m.col_at(0, &t[0], 3.1), 3);
        assert_eq!(m.col_at(0, &t[0], 99.0), 5);
    }

    #[test]
    fn a_wrap_boundary_belongs_to_the_next_row() {
        let (m, t) = build(&["aaaa bbbb cccc"], 10);
        assert_eq!(m.row_of(0, 9), 0);
        assert_eq!(m.row_of(0, 10), 1);
        assert_eq!(m.row_of(0, 14), 1);
        // a click past the end of a non-last row stays on that row
        assert_eq!(m.col_at(0, &t[0], 50.0), 9);
        assert!(m.is_last_row(1) && !m.is_last_row(0));
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 }
        fn below(&mut self, n: usize) -> usize { (self.next() % n as u64) as usize }
    }
    fn rand_line(rng: &mut Rng) -> String {
        let alphabet = ["a", "b", " ", "你", "e\u{301}", "-", "1", ". ", "\t", "x"];
        (0..rng.below(40)).map(|_| alphabet[rng.below(alphabet.len())]).collect()
    }

    #[test]
    fn incremental_apply_equals_a_full_rebuild() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut lines: Vec<String> = (0..6).map(|_| rand_line(&mut rng)).collect();
        let mut m = WrapMap::new(12, 4);
        m.rebuild(lines.len(), &|l| lines[l].clone());
        for _ in 0..600 {
            let start = rng.below(lines.len());
            let old = 1 + rng.below((lines.len() - start).min(3));
            let new = 1 + rng.below(3);
            let repl: Vec<String> = (0..new).map(|_| rand_line(&mut rng)).collect();
            lines.splice(start..start + old, repl);
            let change = gilvt_editor::Change { start_line: start, old_lines: old, new_lines: new };
            m.apply(change, lines.len(), &|l| lines[l].clone());
            let mut fresh = WrapMap::new(12, 4);
            fresh.rebuild(lines.len(), &|l| lines[l].clone());
            assert_eq!(m.rows, fresh.rows);
            assert_eq!(m.first, fresh.first);
        }
    }

    #[test]
    fn an_out_of_sync_change_falls_back_to_a_rebuild() {
        let (mut m, _) = build(&["a", "b"], 10);
        let lines = ["x".to_string(), "y".to_string(), "z".to_string()];
        m.apply(gilvt_editor::Change { start_line: 0, old_lines: 1, new_lines: 1 }, 3, &|l| lines[l].clone());
        assert_eq!(m.total_rows(), 3);
    }
}
