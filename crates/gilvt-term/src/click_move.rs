//! Click-to-move: a plain click on the line being edited moves the cursor there, by sending the
//! line editor (zle / readline, Claude Code, Codex, ...) as many Left / Right arrow presses as
//! there are characters between the cursor and the click, like iTerm2 and Ghostty do.

use crate::snapshot::Snapshot;

/// Signed number of Right (positive) / Left (negative) presses that move the cursor at viewport
/// `cursor` to viewport `target`, or `None` when `target` is not on the cursor's logical line
/// (its row and the rows soft-wrapped onto it) or the viewport is scrolled back. Wide characters
/// count once; a click past the end of the line's text stops right after it.
pub fn arrow_presses(snap: &Snapshot, cursor: (usize, usize), target: (usize, usize)) -> Option<isize> {
    let rows = snap.rows.len();
    let cols = snap.cols;
    if snap.display_offset != 0 || cols == 0 || cursor.0 >= rows || target.0 >= rows {
        return None;
    }
    let wraps = |row: usize| snap.wrapped.get(row).copied().unwrap_or(false);
    let mut first = cursor.0;
    while first > 0 && wraps(first - 1) {
        first -= 1;
    }
    let mut last = cursor.0;
    while last + 1 < rows && wraps(last) {
        last += 1;
    }
    if !(first..=last).contains(&target.0) {
        return None;
    }
    let cell = |i: usize| snap.rows[first + i / cols].get(i % cols);
    let index = |(row, col): (usize, usize)| (row - first) * cols + col.min(cols - 1);
    let starts_char = |i: usize| cell(i).is_some_and(|c| !c.text.is_empty());

    let mut to = index(target);
    // The right half of a wide character belongs to it.
    if !starts_char(to) && to > 0 && cell(to - 1).is_some_and(|c| c.wide) {
        to -= 1;
    }
    let from = index(cursor);
    let end = (index((first, 0))..=index((last, cols - 1)))
        .rev()
        .find(|&i| cell(i).is_some_and(|c| !c.text.trim().is_empty()))
        .map_or(0, |i| i + if cell(i).is_some_and(|c| c.wide) { 2 } else { 1 });
    let to = to.min(end.max(from));
    let count = (from.min(to)..from.max(to)).filter(|&i| starts_char(i)).count() as isize;
    Some(if to >= from { count } else { -count })
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::Processor;

    use super::arrow_presses;
    use crate::palette::Palette;
    use crate::size::TermSize;
    use crate::snapshot::{take_snapshot, Snapshot};

    fn snap(bytes: &[u8]) -> Snapshot {
        let size = TermSize { cols: 10, rows: 4, cell_width: 8, cell_height: 16 };
        let mut term = Term::new(Config::default(), &size, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, bytes);
        take_snapshot(&term, &Palette::dark(), None)
    }

    #[test]
    fn same_row_left_and_right() {
        // "$ hello" with the cursor after the "o" (col 7).
        let s = snap(b"$ hello");
        assert_eq!(arrow_presses(&s, (0, 7), (0, 3)), Some(-4));
        assert_eq!(arrow_presses(&s, (0, 2), (0, 5)), Some(3));
        assert_eq!(arrow_presses(&s, (0, 7), (0, 7)), Some(0));
    }

    #[test]
    fn click_past_the_text_stops_after_it() {
        let s = snap(b"$ hi\x1b[1;3H");
        assert_eq!(arrow_presses(&s, (0, 2), (0, 9)), Some(2));
    }

    #[test]
    fn cursor_past_the_text_is_kept() {
        // Trailing spaces the user typed: clicking further right does nothing.
        let s = snap(b"$ hi   ");
        assert_eq!(arrow_presses(&s, (0, 7), (0, 9)), Some(0));
    }

    #[test]
    fn wide_chars_count_once() {
        // "中文x": 中 at 0-1, 文 at 2-3, x at 4; cursor at 5.
        let s = snap("中文x".as_bytes());
        assert_eq!(arrow_presses(&s, (0, 5), (0, 2)), Some(-2));
        // The right half of 文 means 文.
        assert_eq!(arrow_presses(&s, (0, 5), (0, 3)), Some(-2));
        assert_eq!(arrow_presses(&s, (0, 0), (0, 4)), Some(2));
    }

    #[test]
    fn soft_wrapped_rows_are_one_line() {
        // 12 chars on 10 columns: the cursor ends on row 1, col 2.
        let s = snap(b"0123456789ab");
        assert_eq!(arrow_presses(&s, (1, 2), (0, 4)), Some(-8));
        assert_eq!(arrow_presses(&s, (0, 4), (1, 1)), Some(7));
    }

    #[test]
    fn other_lines_are_ignored() {
        let s = snap(b"out\r\n$ ls");
        assert_eq!(arrow_presses(&s, (1, 4), (0, 1)), None);
        assert_eq!(arrow_presses(&s, (1, 4), (2, 1)), None);
    }
}
