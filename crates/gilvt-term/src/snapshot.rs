//! A plain-data copy of the visible screen, resolved to RGB, for the renderer.

use std::ops::RangeInclusive;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::CursorShape;

use crate::palette::{Palette, Rgb};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub undercurl: bool,
    pub strikethrough: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellView {
    /// Base character plus any combining characters. Empty for the spacer half of a wide char.
    pub text: String,
    pub fg: Rgb,
    pub bg: Rgb,
    pub style: Style,
    pub wide: bool,
    pub selected: bool,
    pub search_match: bool,
    pub hyperlink: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorKind {
    Block,
    Beam,
    Underline,
    HollowBlock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorView {
    pub row: usize,
    pub col: usize,
    pub kind: CursorKind,
    /// True when the cursor sits on a wide character (cursor is two cells wide).
    pub wide: bool,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub rows: Vec<Vec<CellView>>,
    pub cursor: Option<CursorView>,
    /// Viewport (row, col) of the terminal cursor even while it is hidden (`\e[?25l`): TUIs such as
    /// Claude Code hide it and draw their own, but the IME candidate window still has to follow it.
    /// Clamped into the viewport.
    pub ime_anchor: (usize, usize),
    pub display_offset: usize,
    /// Per row: alacritty soft-wrapped it (WRAPLINE on its last cell), so it continues on the next row.
    pub wrapped: Vec<bool>,
    pub mode: TermMode,
    pub cols: usize,
    pub background: Rgb,
}

impl Snapshot {
    /// Viewport row/col → grid point (accounts for scrollback offset).
    pub fn grid_point(&self, row: usize, col: usize) -> Point {
        Point::new(Line(row as i32 - self.display_offset as i32), Column(col.min(self.cols - 1)))
    }

    /// The visible text of one row, and for every char the column it starts at.
    pub fn row_text(&self, row: usize) -> (String, Vec<usize>) {
        let mut text = String::new();
        let mut cols = Vec::new();
        for (col, cell) in self.rows[row].iter().enumerate() {
            for ch in cell.text.chars() {
                text.push(ch);
                cols.push(col);
            }
        }
        (text, cols)
    }
}

pub fn take_snapshot<T: EventListener>(term: &Term<T>, palette: &Palette, search: Option<&RangeInclusive<Point>>) -> Snapshot {
    let content = term.renderable_content();
    let colors = content.colors;
    let display_offset = content.display_offset;
    let cols = term.columns();
    let screen_lines = term.screen_lines();
    let blank_bg = palette.resolve(
        alacritty_terminal::vte::ansi::Color::Named(alacritty_terminal::vte::ansi::NamedColor::Background),
        colors,
    );
    let mut rows = vec![Vec::with_capacity(cols); screen_lines];
    let mut wrapped = vec![false; screen_lines];

    for indexed in content.display_iter {
        let row = (indexed.point.line.0 + display_offset as i32) as usize;
        if row >= screen_lines {
            continue;
        }
        let cell = indexed.cell;
        let flags = cell.flags;
        if indexed.point.column.0 + 1 == cols && flags.contains(Flags::WRAPLINE) {
            wrapped[row] = true;
        }
        let mut fg = palette.resolve(cell.fg, colors);
        let mut bg = palette.resolve(cell.bg, colors);
        if flags.contains(Flags::DIM) {
            fg = crate::palette::dim(fg);
        }
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if flags.contains(Flags::HIDDEN) {
            fg = bg;
        }
        let text = if flags.contains(Flags::WIDE_CHAR_SPACER) || flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            String::new()
        } else {
            let mut s = String::new();
            s.push(if cell.c == '\t' { ' ' } else { cell.c });
            if let Some(extra) = cell.zerowidth() {
                s.extend(extra.iter());
            }
            s
        };
        let selected = content
            .selection
            .as_ref()
            .is_some_and(|sel| sel.contains(indexed.point));
        let search_match = search.is_some_and(|m| m.contains(&indexed.point));
        rows[row].push(CellView {
            text,
            fg,
            bg,
            style: Style {
                bold: flags.contains(Flags::BOLD),
                italic: flags.contains(Flags::ITALIC),
                underline: flags.intersects(Flags::ALL_UNDERLINES) && !flags.contains(Flags::UNDERCURL),
                undercurl: flags.contains(Flags::UNDERCURL),
                strikethrough: flags.contains(Flags::STRIKEOUT),
            },
            wide: flags.contains(Flags::WIDE_CHAR),
            selected,
            search_match,
            hyperlink: cell.hyperlink().map(|h| h.uri().to_string()),
        });
    }

    let cursor = {
        let c = content.cursor;
        let row = c.point.line.0 + display_offset as i32;
        let visible = content.mode.contains(TermMode::SHOW_CURSOR)
            && c.shape != CursorShape::Hidden
            && row >= 0
            && (row as usize) < screen_lines;
        visible.then(|| {
            let row = row as usize;
            let col = c.point.column.0;
            CursorView {
                row,
                col,
                kind: match c.shape {
                    CursorShape::Beam => CursorKind::Beam,
                    CursorShape::Underline => CursorKind::Underline,
                    CursorShape::HollowBlock => CursorKind::HollowBlock,
                    _ => CursorKind::Block,
                },
                wide: rows[row].get(col).is_some_and(|cell| cell.wide),
            }
        })
    };

    let ime_anchor = {
        let c = content.cursor;
        let row = (c.point.line.0 + display_offset as i32).clamp(0, screen_lines.saturating_sub(1) as i32) as usize;
        (row, c.point.column.0.min(cols.saturating_sub(1)))
    };

    Snapshot { rows, cursor, ime_anchor, display_offset, wrapped, mode: content.mode, cols, background: blank_bg }
}

/// The last `n` logical lines of the visible screen as plain text (`gilvt debug state`): rows that
/// alacritty soft-wrapped (WRAPLINE on the last cell) are joined into one line, trailing whitespace
/// trimmed, trailing empty lines dropped, the spacer halves of wide chars skipped. Only visible rows
/// are read: a line whose start scrolled off begins mid-line, and one running past the bottom ends
/// there. Scans from the bottom, so only the lines returned (and the blank ones below them) are read.
pub fn screen_tail<T: EventListener>(term: &Term<T>, n: usize) -> Vec<String> {
    let grid = term.grid();
    let offset = grid.display_offset() as i32;
    let cols = term.columns();
    let row_at = |row: usize| &grid[Line(row as i32 - offset)];
    let wraps = |row: usize| cols > 0 && row_at(row)[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
    let mut out = Vec::with_capacity(n);
    // The rows of the line being assembled, bottom row first.
    let mut parts: Vec<String> = Vec::new();
    let push = |parts: &mut Vec<String>, out: &mut Vec<String>| {
        let joined: String = parts.drain(..).rev().collect();
        let text = joined.trim_end();
        if !(out.is_empty() && text.is_empty()) {
            out.push(text.to_string());
        }
    };
    for row in (0..term.screen_lines()).rev() {
        if out.len() == n {
            break;
        }
        // A row that wraps continues into the one below: it belongs to the line being assembled.
        if !parts.is_empty() && !wraps(row) {
            push(&mut parts, &mut out);
            if out.len() == n {
                break;
            }
        }
        let line = row_at(row);
        let mut text = String::with_capacity(cols);
        for col in 0..cols {
            let cell = &line[Column(col)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            text.push(if cell.c == '\t' { ' ' } else { cell.c });
            if let Some(extra) = cell.zerowidth() {
                text.extend(extra.iter());
            }
        }
        parts.push(text);
    }
    if !parts.is_empty() && out.len() < n {
        push(&mut parts, &mut out);
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::Processor;

    use crate::palette::Palette;
    use crate::size::TermSize;

    fn term_with(bytes: &[u8]) -> Term<VoidListener> {
        let size = TermSize { cols: 10, rows: 3, cell_width: 8, cell_height: 16 };
        let mut term = Term::new(Config::default(), &size, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, bytes);
        term
    }

    #[test]
    fn colors_and_text() {
        let p = Palette::dark();
        let snap = take_snapshot(&term_with(b"\x1b[31mA\x1b[0mB"), &p, None);
        assert_eq!(snap.rows.len(), 3);
        assert_eq!(snap.rows[0].len(), 10);
        assert_eq!(snap.rows[0][0].text, "A");
        assert_eq!(snap.rows[0][0].fg, p.ansi[1]);
        assert_eq!(snap.rows[0][1].fg, p.foreground);
        assert_eq!(snap.cursor.map(|c| (c.row, c.col)), Some((0, 2)));
    }

    #[test]
    fn wide_chars_have_spacer() {
        let snap = take_snapshot(&term_with("中x".as_bytes()), &Palette::dark(), None);
        assert!(snap.rows[0][0].wide);
        assert_eq!(snap.rows[0][0].text, "中");
        assert_eq!(snap.rows[0][1].text, "");
        assert_eq!(snap.rows[0][2].text, "x");
        assert_eq!(snap.row_text(0).1[..2], [0, 2]);
    }

    #[test]
    fn inverse_and_styles() {
        let p = Palette::dark();
        let snap = take_snapshot(&term_with(b"\x1b[7;1;4mI"), &p, None);
        let c = &snap.rows[0][0];
        assert_eq!((c.fg, c.bg), (p.background, p.foreground));
        assert!(c.style.bold && c.style.underline);
    }

    #[test]
    fn soft_wrapped_rows() {
        let snap = take_snapshot(&term_with(b"0123456789ab\r\nx"), &Palette::dark(), None);
        assert_eq!(snap.wrapped, [true, false, false]);
    }

    #[test]
    fn hidden_cursor() {
        let snap = take_snapshot(&term_with(b"\x1b[?25l"), &Palette::dark(), None);
        assert!(snap.cursor.is_none());
    }

    #[test]
    fn hidden_cursor_keeps_ime_anchor() {
        let snap = take_snapshot(&term_with(b"\x1b[?25l\x1b[2;4H"), &Palette::dark(), None);
        assert!(snap.cursor.is_none());
        assert_eq!(snap.ime_anchor, (1, 3));
    }

    #[test]
    fn osc8_hyperlink() {
        let snap = take_snapshot(&term_with(b"\x1b]8;;https://a.io\x1b\\L\x1b]8;;\x1b\\"), &Palette::dark(), None);
        assert_eq!(snap.rows[0][0].hyperlink.as_deref(), Some("https://a.io"));
        assert_eq!(snap.rows[0][1].hyperlink, None);
    }

    #[test]
    fn screen_tail_reads_cjk_and_drops_trailing_blanks() {
        let size = TermSize { cols: 12, rows: 6, cell_width: 8, cell_height: 16 };
        let mut term = Term::new(Config::default(), &size, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, "one\r\n中文 ok   \r\n\r\n❯ 1. Cats  \r\n".as_bytes());
        assert_eq!(screen_tail(&term, 20), ["one", "中文 ok", "", "❯ 1. Cats"]);
        assert_eq!(screen_tail(&term, 2), ["", "❯ 1. Cats"]);
        assert!(screen_tail(&term, 0).is_empty());
        assert!(screen_tail(&term_with(b""), 5).is_empty(), "a blank screen has no rows");
    }

    fn narrow(cols: u16, rows: u16, bytes: &[u8]) -> Term<VoidListener> {
        let size = TermSize { cols, rows, cell_width: 8, cell_height: 16 };
        let mut term = Term::new(Config::default(), &size, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, bytes);
        term
    }

    #[test]
    fn screen_tail_joins_soft_wrapped_rows() {
        // A command wrapped over 4 rows of a narrow pane is one line; hard line breaks still split.
        let cmd = "$ claude --resume c1a0de00-0000-4000-8000-000000000903";
        let term = narrow(16, 8, format!("top\r\n{cmd}\r\nnext  \r\n").as_bytes());
        assert_eq!(screen_tail(&term, 20), ["top", cmd, "next"]);
        // `n` counts lines, not rows: the wrapped line comes whole.
        assert_eq!(screen_tail(&term, 2), [cmd, "next"]);
        // Blanks at a wrap point are content; only the line's end is trimmed.
        let term = narrow(4, 4, b"ab  cd\r\n");
        assert_eq!(screen_tail(&term, 5), ["ab  cd"]);
        // A line filling the row exactly without a wrap stays one row.
        let term = narrow(4, 4, b"abcd\r\nef");
        assert_eq!(screen_tail(&term, 5), ["abcd", "ef"]);
    }

    #[test]
    fn screen_tail_wraps_wide_chars_at_the_edge() {
        // "ab中" fills 4 of 5 columns; 文 does not fit in the last one, which becomes a leading spacer
        // and wraps: the line reads back without a gap. A wide char ending exactly at the edge too.
        let term = narrow(5, 6, "ab中文字 x\r\n中文字xy".as_bytes());
        assert_eq!(screen_tail(&term, 5), ["ab中文字 x", "中文字xy"]);
        assert_eq!(screen_tail(&term, 1), ["中文字xy"]);
    }

    #[test]
    fn screen_tail_keeps_only_visible_rows_of_a_wrapped_line() {
        // The first 2 rows of the wrapped line scrolled off: the tail starts mid-line.
        let term = narrow(4, 2, b"abcdefghij");
        assert_eq!(screen_tail(&term, 5), ["efghij"]);
    }
}
