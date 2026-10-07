//! Absolute line numbers for the main screen, so a line recorded while a program ran can be
//! found again in the scrollback later.
//!
//! Output is parsed on alacritty's I/O thread, so we cannot count scrolls as they happen.
//! Instead we leave an invisible marker (a colour on column 0's underline attribute) on the
//! newest history row that starts a logical line, and on every sync we find where that marker
//! moved to. The renderer never reads a cell's underline *colour*, only its underline *flag*
//! (see `snapshot.rs`), so this is invisible regardless of whatever the cell already holds --
//! including real content that sets its own underline colour (e.g. via SGR 58) or is genuinely
//! underlined. `plant` therefore always succeeds as long as there is at least one history row to
//! plant on, by unconditionally claiming the newest logical line's column 0; it does not need to
//! search for a "free" cell. History rows are never rewritten by programs, and column 0 of a
//! logical line stays in column 0 through reflow, so the marker is found exactly unless the
//! history was cleared or the marker was evicted. When the marker is lost we cannot tell how
//! far the output moved, so every line number handed out before is invalidated (resolves to
//! `Evicted`) and numbering continues above all of them: an old anchor is never mapped onto
//! other content.

use std::ops::Range;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Grid, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{ClearMode, Color, Handler, Rgb};

/// Result of [`LineTracker::scroll_to`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollOutcome {
    /// The line is visible at this viewport row (0 = top).
    Shown { viewport_row: usize },
    /// The line has left the scrollback (evicted, cleared, or invalidated).
    Evicted,
    /// A full-screen program owns the alternate screen; the scrollback cannot be shown now.
    AltScreen,
}

#[derive(Clone, Copy, Debug)]
struct Marker {
    /// Absolute number of the marked row.
    line: u64,
    color: Rgb,
    /// Whether the marked cell already carried a real underline flag when we planted on it, so
    /// `find_marker` can require the same flag state back. This is cheap extra protection against
    /// an unrelated cell that happens to carry the exact same colour by coincidence (e.g. a
    /// program using SGR 58 with a colour that collides with ours): matching now requires both
    /// the colour *and* the flag state we actually observed, rather than colour alone.
    had_underline: bool,
}

/// Maps absolute line numbers to grid lines of the main screen.
#[derive(Debug)]
pub struct LineTracker {
    /// Absolute number of the main screen's top row (`Line(0)`) at the last sync.
    top: u64,
    /// Main-screen history size at the last sync.
    history: usize,
    marker: Option<Marker>,
    generation: u32,
    /// Numbers below this are invalid (their content is gone or can no longer be located).
    valid_from: u64,
    /// One past the highest number that may have been handed out.
    high_water: u64,
}

impl LineTracker {
    /// `max_history` matches the terminal's configured scrollback capacity; kept as a parameter
    /// (rather than dropped) so callers still state it explicitly at the construction site, even
    /// though `plant` no longer needs to know it directly (it always succeeds once `hs > 0`, so
    /// there is no "history is pinned at capacity" case to special-case any more).
    pub fn new(_max_history: usize) -> Self {
        Self { top: 0, history: 0, marker: None, generation: 0, valid_from: 0, high_water: 0 }
    }

    /// Brings the numbering up to date with the grid. A no-op while the alternate screen is
    /// active (the main grid is not reachable then and nothing but a resize can change it).
    pub fn sync<T: EventListener>(&mut self, term: &mut Term<T>) {
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        let screen = term.screen_lines();
        let grid = term.grid_mut();
        let hs = grid.history_size();
        match self.marker {
            Some(marker) => match find_marker(grid, marker.color, marker.had_underline) {
                Some(line) => {
                    self.top = (marker.line as i64 - line.0 as i64) as u64;
                    clear_marker(grid, line);
                }
                None => self.lost(hs),
            },
            // `plant` always succeeds once there is any history (see below), so a `None` marker
            // here only ever means there was no history yet at the previous sync; the plain
            // delta is then always exact (nothing can have been evicted with nowhere for it to
            // have gone).
            None if hs < self.history => self.lost(hs),
            None => self.top += (hs - self.history) as u64,
        }
        self.history = hs;
        self.marker = self.plant(grid, hs);
        self.high_water = self.high_water.max(self.top + screen as u64);
    }

    /// The marker is gone and the distance moved is unknown: invalidate every earlier number.
    fn lost(&mut self, hs: usize) {
        self.valid_from = self.high_water;
        self.top = self.high_water + hs as u64;
    }

    /// Plants a fresh marker on the newest logical line's column 0. Always succeeds when `hs >
    /// 0`: the renderer never reads underline colour, only the underline flag (see module docs),
    /// so this cell is always safe to claim regardless of what it already carries -- a stale
    /// marker, or real content's own underline colour and/or flag.
    fn plant(&mut self, grid: &mut Grid<Cell>, hs: usize) -> Option<Marker> {
        if hs == 0 {
            return None;
        }
        let topmost = -(hs as i32);
        let mut line = -1;
        while line > topmost && wraps(grid, Line(line - 1)) {
            line -= 1;
        }
        self.generation = self.generation.wrapping_add(1);
        let color = marker_color(self.generation);
        let cell = &mut grid[Line(line)][Column(0)];
        let had_underline = cell.flags.intersects(Flags::ALL_UNDERLINES);
        cell.set_underline_color(Some(Color::Spec(color)));
        Some(Marker { line: (self.top as i64 + line as i64) as u64, color, had_underline })
    }

    /// Absolute number of the cursor row; `None` while the alternate screen is active
    /// (alternate-screen content never reaches the scrollback, so it cannot be found later).
    pub fn cursor_line<T: EventListener>(&mut self, term: &mut Term<T>) -> Option<u64> {
        self.sync(term);
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return None;
        }
        Some(self.top + term.grid().cursor.point.line.0.max(0) as u64)
    }

    /// Grid line of an absolute number, if it still exists. Requires a fresh sync.
    fn resolve<T: EventListener>(&self, term: &Term<T>, line: u64) -> Option<Line> {
        if line < self.valid_from {
            return None;
        }
        let l = line as i64 - self.top as i64;
        let hs = term.grid().history_size() as i64;
        (l >= -hs && l < term.screen_lines() as i64).then_some(Line(l as i32))
    }

    /// Scrolls so `line` sits about a third from the top of the viewport.
    pub fn scroll_to<T: EventListener>(&mut self, term: &mut Term<T>, line: u64) -> ScrollOutcome {
        self.sync(term);
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return ScrollOutcome::AltScreen;
        }
        let Some(target) = self.resolve(term, line) else {
            return ScrollOutcome::Evicted;
        };
        let rows = term.screen_lines() as i32;
        let hs = term.grid().history_size() as i32;
        let offset = (rows / 3 - target.0).clamp(0, hs);
        let current = term.grid().display_offset() as i32;
        term.scroll_display(Scroll::Delta(offset - current));
        let offset = term.grid().display_offset() as i32;
        ScrollOutcome::Shown { viewport_row: (target.0 + offset) as usize }
    }

    /// The nearest line at or above `line`, at most `rows` lines up, whose text contains every one of
    /// `needles`. Lines that are gone (cleared, evicted, invalidated) are not searched; None on the alternate
    /// screen.
    pub fn find_above<T: EventListener>(&mut self, term: &mut Term<T>, line: u64, rows: usize, needles: &[&str]) -> Option<u64> {
        self.sync(term);
        if term.mode().contains(TermMode::ALT_SCREEN) || needles.iter().all(|n| n.is_empty()) {
            return None;
        }
        let start = self.resolve(term, line)?;
        let oldest = -(term.grid().history_size() as i32);
        let valid = self.valid_from as i64 - self.top as i64;
        let floor = (start.0 as i64 - rows as i64).max(oldest as i64).max(valid) as i32;
        let grid = term.grid();
        (floor..=start.0)
            .rev()
            .find(|&l| {
                let text = row_text(grid, Line(l));
                needles.iter().all(|n| text.contains(n))
            })
            .map(|l| (self.top as i64 + l as i64) as u64)
    }

    /// Text of absolute lines `start .. end` that still exist: soft-wrapped rows joined into one line,
    /// trailing blanks trimmed, trailing empty lines dropped, at most the last `max_lines` lines, joined with
    /// `\n`. Lines that are gone (evicted, cleared, invalidated) are skipped. None on the alternate screen or
    /// when nothing of the range is left.
    pub fn lines_text<T: EventListener>(&mut self, term: &mut Term<T>, start: u64, end: u64, max_lines: usize) -> Option<String> {
        self.sync(term);
        if term.mode().contains(TermMode::ALT_SCREEN) || end <= start || max_lines == 0 {
            return None;
        }
        let top = self.top as i64;
        let hs = term.grid().history_size() as i64;
        let first = (start as i64).max(self.valid_from as i64).max(top - hs);
        let last = (end as i64).min(top + term.screen_lines() as i64);
        if first >= last {
            return None;
        }
        // Backwards from the end, one logical line (soft-wrapped rows joined) at a time, so a long range
        // costs only the lines kept: this runs on the UI thread under the terminal lock.
        let grid = term.grid();
        let row = |abs: i64| Line((abs - top) as i32);
        let mut lines: Vec<String> = Vec::new();
        let mut end_row = last - 1;
        while end_row >= first && lines.len() < max_lines {
            let mut begin = end_row;
            while begin > first && wraps(grid, row(begin - 1)) {
                begin -= 1;
            }
            let mut logical = String::new();
            for abs in begin..=end_row {
                logical.push_str(&row_text(grid, row(abs)));
            }
            let logical = logical.trim_end();
            // Trailing empty lines are dropped, not counted.
            if !(lines.is_empty() && logical.is_empty()) {
                lines.push(logical.to_string());
            }
            end_row = begin - 1;
        }
        if lines.is_empty() {
            return None;
        }
        lines.reverse();
        Some(lines.join("\n"))
    }

    /// Viewport rows currently showing lines `line .. line + count` (clipped to the viewport);
    /// `None` when none of them is visible. Recompute every frame while drawing a highlight:
    /// the rows move when output arrives or the user scrolls.
    pub fn visible_rows<T: EventListener>(&mut self, term: &mut Term<T>, line: u64, count: usize) -> Option<Range<usize>> {
        self.sync(term);
        if term.mode().contains(TermMode::ALT_SCREEN) || count == 0 {
            return None;
        }
        let rows = term.screen_lines() as i64;
        let offset = term.grid().display_offset() as i64;
        let hs = term.grid().history_size() as i64;
        // Clip to lines that exist, then to the viewport.
        let first = (line as i64).max(self.valid_from as i64).max(self.top as i64 - hs);
        let end = (line as i64 + count as i64).min(self.top as i64 + rows);
        let start_row = (first - self.top as i64 + offset).max(0);
        let end_row = (end - self.top as i64 + offset).min(rows);
        (start_row < end_row).then(|| start_row as usize..end_row as usize)
    }

    /// Resizes `term`, keeping the numbering attached to content. A pure row change is exact;
    /// a column change reflows wrapped lines, after which numbers are exact at the newest
    /// history line and off by the net number of rows gained or lost by rewrapping between that
    /// line and the numbered one.
    pub fn resize<T: EventListener, S: Dimensions>(&mut self, term: &mut Term<T>, size: S) {
        self.sync(term);
        term.resize(size);
        // Re-plant right away: growing the screen can pull the marker onto the (writable) screen.
        self.sync(term);
    }

    /// Clears the scrollback (⌘K). Earlier numbers of lines that were in the scrollback become
    /// `Evicted`; the visible screen keeps its numbers. In the alternate screen this clears
    /// nothing from the main history (same as alacritty) and numbering is untouched.
    pub fn clear_history<T: EventListener>(&mut self, term: &mut Term<T>) {
        self.sync(term);
        term.clear_screen(ClearMode::Saved);
        if !term.mode().contains(TermMode::ALT_SCREEN) {
            self.marker = None;
            self.history = 0;
            self.valid_from = self.valid_from.max(self.top);
        }
    }
}

/// A row's characters (wide characters once), trailing blanks kept.
fn row_text(grid: &Grid<Cell>, line: Line) -> String {
    let row = &grid[line];
    (0..grid.columns())
        .map(|c| &row[Column(c)])
        .filter(|cell| !cell.flags.contains(Flags::WIDE_CHAR_SPACER))
        .map(|cell| cell.c)
        .collect()
}

fn wraps(grid: &Grid<Cell>, line: Line) -> bool {
    let last = grid.last_column();
    grid[line][last].flags.contains(Flags::WRAPLINE)
}

/// Spreads the rotating generation counter across the whole colour space (rather than a small,
/// guessable, fixed band) so a program's own underline colour is very unlikely to coincidentally
/// match whichever one we are currently looking for.
fn marker_color(generation: u32) -> Rgb {
    let v = generation.wrapping_mul(0x9E37_79B1);
    Rgb { r: (v >> 24) as u8, g: (v >> 16) as u8, b: (v >> 8) as u8 }
}

fn find_marker(grid: &Grid<Cell>, color: Rgb, had_underline: bool) -> Option<Line> {
    let bottom = grid.screen_lines() as i32 - 1;
    let top = -(grid.history_size() as i32);
    (top..=bottom).rev().map(Line).find(|&l| {
        let cell = &grid[l][Column(0)];
        cell.underline_color() == Some(Color::Spec(color)) && cell.flags.intersects(Flags::ALL_UNDERLINES) == had_underline
    })
}

fn clear_marker(grid: &mut Grid<Cell>, line: Line) {
    grid[line][Column(0)].set_underline_color(None);
}

#[cfg(test)]
#[path = "lines_tests.rs"]
mod tests;
