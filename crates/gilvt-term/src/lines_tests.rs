use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::Config;
use alacritty_terminal::vte::ansi::Processor;

use super::*;
use crate::size::TermSize;

struct Fixture {
    term: Term<VoidListener>,
    parser: Processor,
    lines: LineTracker,
}

fn size(cols: u16, rows: u16) -> TermSize {
    TermSize { cols, rows, cell_width: 8, cell_height: 16 }
}

impl Fixture {
    fn new(cols: u16, rows: u16, history: usize) -> Self {
        let config = Config { scrolling_history: history, ..Config::default() };
        Self { term: Term::new(config, &size(cols, rows), VoidListener), parser: Processor::new(), lines: LineTracker::new(history) }
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// Prints `tag0 .. tag{n-1}`, one per line.
    fn print(&mut self, tag: &str, n: usize) {
        for i in 0..n {
            self.feed(format!("{tag}{i}\r\n").as_bytes());
        }
    }

    fn cursor(&mut self) -> u64 {
        self.lines.cursor_line(&mut self.term).expect("main screen")
    }

    fn row(&self, viewport_row: usize) -> String {
        let line = Line(viewport_row as i32 - self.term.grid().display_offset() as i32);
        let row = &self.term.grid()[line];
        (0..self.term.columns()).map(|c| row[Column(c)].c).collect::<String>().trim_end().to_string()
    }

    /// Scrolls to `line` and returns the text shown there.
    fn jump(&mut self, line: u64) -> Option<String> {
        match self.lines.scroll_to(&mut self.term, line) {
            ScrollOutcome::Shown { viewport_row } => Some(self.row(viewport_row)),
            _ => None,
        }
    }
}

/// Prints a line, records the cursor line before it (the line the text lands on).
fn anchored(f: &mut Fixture, text: &str) -> u64 {
    let at = f.cursor();
    f.feed(format!("{text}\r\n").as_bytes());
    at
}

#[test]
fn numbers_follow_output_into_scrollback() {
    let mut f = Fixture::new(20, 10, 1000);
    f.print("a", 5);
    let mark = anchored(&mut f, "MARK");
    assert_eq!(mark, 5);
    f.print("b", 200);
    assert_eq!(f.jump(mark).as_deref(), Some("MARK"));
    // About a third from the top.
    assert_eq!(f.lines.scroll_to(&mut f.term, mark), ScrollOutcome::Shown { viewport_row: 3 });
    // The bottom row cannot be moved up: the viewport never scrolls below the screen.
    let last = f.cursor();
    assert_eq!(f.lines.scroll_to(&mut f.term, last), ScrollOutcome::Shown { viewport_row: 9 });
    assert_eq!(f.term.grid().display_offset(), 0);
}

#[test]
fn many_syncs_while_output_flows() {
    let mut f = Fixture::new(20, 6, 500);
    let mut marks = Vec::new();
    for i in 0..40 {
        f.print("x", 3);
        marks.push((anchored(&mut f, &format!("M{i}")), format!("M{i}")));
    }
    for (line, text) in marks {
        assert_eq!(f.jump(line), Some(text));
    }
}

#[test]
fn cursor_line_is_monotonic_across_redraws() {
    let mut f = Fixture::new(20, 5, 100);
    let a = anchored(&mut f, "one");
    // A TUI-style redraw in place: cursor up, clear line, rewrite.
    f.feed(b"\x1b[1A\x1b[2Kone again\r\n");
    let b = f.cursor();
    assert!(b > a);
    f.print("y", 30);
    assert!(f.cursor() > b);
}

#[test]
fn evicted_lines_report_evicted() {
    let mut f = Fixture::new(20, 5, 20);
    let mark = anchored(&mut f, "OLD");
    f.print("c", 15);
    assert_eq!(f.jump(mark).as_deref(), Some("OLD"));
    f.print("d", 100);
    assert_eq!(f.lines.scroll_to(&mut f.term, mark), ScrollOutcome::Evicted);
    // Newer lines still resolve.
    let fresh = anchored(&mut f, "NEW");
    f.print("e", 3);
    assert_eq!(f.jump(fresh).as_deref(), Some("NEW"));
}

#[test]
fn eviction_without_intermediate_sync_invalidates_instead_of_misplacing() {
    let mut f = Fixture::new(20, 5, 20);
    f.print("p", 10);
    let mark = anchored(&mut f, "GONE");
    // Far more than the history holds between two syncs: the marker is evicted too.
    f.print("q", 500);
    assert_eq!(f.lines.scroll_to(&mut f.term, mark), ScrollOutcome::Evicted);
    let fresh = anchored(&mut f, "AFTER");
    assert!(fresh > mark);
    f.print("r", 4);
    assert_eq!(f.jump(fresh).as_deref(), Some("AFTER"));
}

#[test]
fn clear_history_keeps_screen_and_drops_scrollback() {
    let mut f = Fixture::new(20, 5, 100);
    let old = anchored(&mut f, "SCROLLED");
    f.print("s", 10);
    let on_screen = anchored(&mut f, "VISIBLE");
    f.lines.clear_history(&mut f.term);
    assert_eq!(f.lines.scroll_to(&mut f.term, old), ScrollOutcome::Evicted);
    assert_eq!(f.jump(on_screen).as_deref(), Some("VISIBLE"));
    f.print("t", 30);
    assert_eq!(f.jump(on_screen).as_deref(), Some("VISIBLE"));
    assert_eq!(f.lines.scroll_to(&mut f.term, old), ScrollOutcome::Evicted);
}

#[test]
fn program_clearing_scrollback_invalidates_old_lines() {
    let mut f = Fixture::new(20, 5, 100);
    let old = anchored(&mut f, "OLD");
    f.print("u", 10);
    f.cursor(); // sync: marker planted in the history
    f.feed(b"\x1b[3J");
    f.print("v", 12);
    assert_eq!(f.lines.scroll_to(&mut f.term, old), ScrollOutcome::Evicted);
    let fresh = anchored(&mut f, "FRESH");
    f.print("w", 8);
    assert_eq!(f.jump(fresh).as_deref(), Some("FRESH"));
}

#[test]
fn clear_screen_pushes_lines_into_scrollback() {
    let mut f = Fixture::new(20, 5, 100);
    f.print("h", 2);
    let mark = anchored(&mut f, "BEFORE CLEAR");
    f.feed(b"\x1b[H\x1b[2J");
    let after = anchored(&mut f, "AFTER CLEAR");
    f.print("i", 2);
    assert_eq!(f.jump(mark).as_deref(), Some("BEFORE CLEAR"));
    assert_eq!(f.jump(after).as_deref(), Some("AFTER CLEAR"));
    assert!(after > mark);
}

#[test]
fn row_resize_is_exact() {
    let mut f = Fixture::new(20, 10, 100);
    f.print("j", 30);
    let mark = anchored(&mut f, "ROWS");
    f.print("k", 3);
    f.lines.resize(&mut f.term, size(20, 4));
    assert_eq!(f.jump(mark).as_deref(), Some("ROWS"));
    f.lines.resize(&mut f.term, size(20, 12));
    assert_eq!(f.jump(mark).as_deref(), Some("ROWS"));
    let next = anchored(&mut f, "NEXT");
    assert!(next > mark);
    f.print("l", 20);
    assert_eq!(f.jump(next).as_deref(), Some("NEXT"));
    assert_eq!(f.jump(mark).as_deref(), Some("ROWS"));
}

#[test]
fn column_resize_is_exact_without_wrapped_lines() {
    let mut f = Fixture::new(20, 6, 100);
    f.print("m", 10);
    let mark = anchored(&mut f, "COLS");
    f.print("n", 10);
    f.lines.resize(&mut f.term, size(12, 6));
    assert_eq!(f.jump(mark).as_deref(), Some("COLS"));
    f.lines.resize(&mut f.term, size(30, 6));
    assert_eq!(f.jump(mark).as_deref(), Some("COLS"));
}

#[test]
fn column_resize_with_rewrapped_lines_is_off_by_the_rewrap() {
    let mut f = Fixture::new(20, 6, 100);
    f.print("pre", 5);
    let mark = anchored(&mut f, "WRAP");
    // Two 30-char lines: two rows each at 20 columns, one row each at 40.
    f.feed(format!("{}\r\n{}\r\n", "z".repeat(30), "z".repeat(30)).as_bytes());
    f.print("o", 10);
    f.lines.resize(&mut f.term, size(40, 6));
    // Documented degradation: the reference is the newest history line at resize time; the
    // two rows lost by rewrapping between it and the anchor shift the anchor up by two rows.
    let ScrollOutcome::Shown { viewport_row } = f.lines.scroll_to(&mut f.term, mark) else { panic!() };
    assert_eq!(f.row(viewport_row), "pre3");
    assert_eq!(f.row(viewport_row + 2), "WRAP");
    // Lines recorded after the resize are exact again.
    let fresh = anchored(&mut f, "FRESH");
    f.print("o", 3);
    assert_eq!(f.jump(fresh).as_deref(), Some("FRESH"));
}

#[test]
fn alternate_screen_has_no_numbers_and_leaves_main_intact() {
    let mut f = Fixture::new(20, 5, 100);
    let mark = anchored(&mut f, "MAIN");
    f.print("f", 10);
    f.feed(b"\x1b[?1049h");
    assert_eq!(f.lines.cursor_line(&mut f.term), None);
    assert_eq!(f.lines.scroll_to(&mut f.term, mark), ScrollOutcome::AltScreen);
    for i in 0..50 {
        f.feed(format!("alt{i}\r\n").as_bytes());
    }
    f.lines.resize(&mut f.term, size(20, 8));
    f.feed(b"\x1b[?1049l");
    assert_eq!(f.jump(mark).as_deref(), Some("MAIN"));
}

#[test]
fn visible_rows_track_the_highlight() {
    let mut f = Fixture::new(20, 9, 100);
    f.print("g", 3);
    let mark = anchored(&mut f, "HL");
    f.print("g", 40);
    let ScrollOutcome::Shown { viewport_row } = f.lines.scroll_to(&mut f.term, mark) else { panic!() };
    assert_eq!(f.lines.visible_rows(&mut f.term, mark, 3), Some(viewport_row..viewport_row + 3));
    // New output while scrolled up keeps the viewport still.
    f.print("g", 2);
    assert_eq!(f.lines.visible_rows(&mut f.term, mark, 3), Some(viewport_row..viewport_row + 3));
    f.term.scroll_display(Scroll::Bottom);
    assert_eq!(f.lines.visible_rows(&mut f.term, mark, 3), None);
    // Clipped at the viewport top.
    f.term.scroll_display(Scroll::Delta(-(viewport_row as i32) - 1));
    f.term.scroll_display(Scroll::Bottom);
    let bottom = f.cursor();
    assert_eq!(f.lines.visible_rows(&mut f.term, bottom, 3), Some(8..9));
}

#[test]
fn marker_is_invisible_to_rendering() {
    let mut f = Fixture::new(20, 4, 100);
    f.print("q", 10);
    f.cursor();
    let snap = crate::snapshot::take_snapshot(&f.term, &crate::palette::Palette::dark(), None);
    f.term.scroll_display(Scroll::Top);
    let scrolled = crate::snapshot::take_snapshot(&f.term, &crate::palette::Palette::dark(), None);
    for s in [snap, scrolled] {
        assert!(s.rows.iter().flatten().all(|c| !c.style.underline && !c.style.undercurl));
    }
}

#[test]
fn underlined_output_does_not_defeat_marker_placement() {
    // Reviewer round 1 repro: `plant` used to skip any cell whose underline *flag* the program's
    // own output had already set, so a screen full of genuinely underlined text (e.g. a status
    // line) could exhaust the search window and leave no marker planted at all -- silently
    // corrupting the numbering once the history was full (see the bug this guards against below).
    // The renderer never reads underline *colour* (only the flag, see `marker_is_invisible_to_rendering`
    // and `snapshot.rs`), so planting on an already-underlined cell is exactly as invisible.
    let mut f = Fixture::new(20, 5, 20);
    let mut mark = 0u64;
    for i in 0..60 {
        if i == 57 {
            mark = f.cursor();
        }
        f.feed(format!("\x1b[4mU{i}\x1b[0m\r\n").as_bytes());
    }
    for i in 60..70 {
        f.feed(format!("\x1b[4mU{i}\x1b[0m\r\n").as_bytes());
        f.cursor(); // sync after each line, as in the reviewer's repro
    }
    assert_eq!(f.jump(mark).as_deref(), Some("U57"));
}

#[test]
fn sgr58_underline_colour_does_not_defeat_marker_placement() {
    // Reviewer round 2 repro: SGR 58 sets underline *colour* directly and independently of the
    // underline flag, and (like real terminals) it stays active for everything printed
    // afterwards. A program that turns it on once therefore leaves every column-0 cell with
    // `underline_color() != None` -- which used to make the old "skip cells that already have a
    // colour" `plant` fail on every single candidate once the colour-set text filled the search
    // window. `plant` no longer searches for a free cell at all: it always claims the newest
    // logical line's column 0, so this can no longer happen.
    let mut f = Fixture::new(20, 5, 20);
    f.feed(b"\x1b[58;2;255;0;0m");
    for _ in 0..40 {
        f.feed(b"dup\r\n");
    }
    let mark = anchored(&mut f, "MARK");
    for _ in 0..3 {
        f.feed(b"dup\r\n");
        f.cursor(); // sync after each line, as in the reviewer's repro
    }
    assert_eq!(f.jump(mark).as_deref(), Some("MARK"));
}

#[test]
fn duplicate_rows_with_sgr58_do_not_misplace_the_anchor() {
    // Reviewer round 2 repro, variant: identical rows used to also defeat the round-1 fix's
    // content-fingerprint fallback (an evicted row and the row that replaces it can hash the
    // same), landing the anchor on unrelated content instead of resolving it correctly. That
    // fallback is gone now that `plant` never fails; nothing about identical row content should
    // matter any more.
    let mut f = Fixture::new(20, 5, 20);
    f.feed(b"\x1b[58;2;255;0;0m");
    for _ in 0..30 {
        f.feed(b"same\r\n");
    }
    f.print("d", 10);
    let mark = anchored(&mut f, "MARK");
    for _ in 0..2 {
        f.feed(b"more\r\n");
        f.cursor();
    }
    assert_eq!(f.jump(mark).as_deref(), Some("MARK"));
}

#[test]
fn full_history_does_not_drift_on_idle_syncs() {
    // Regression: repeated idle syncs (no new output) while the history is pinned at capacity
    // must not move or invalidate an anchor that is still on screen -- `plant` always succeeding
    // now means the marker mechanism itself keeps this exact, but it is worth pinning down.
    let mut f = Fixture::new(20, 5, 20);
    f.print("p", 25);
    let mark = f.cursor();
    for _ in 0..5 {
        assert_eq!(f.cursor(), mark);
        assert!(matches!(f.lines.scroll_to(&mut f.term, mark), ScrollOutcome::Shown { .. }));
    }
}

#[test]
fn finds_the_nearest_matching_line_above() {
    let mut f = Fixture::new(30, 5, 100);
    f.feed("● Bash(make test)\r\n".as_bytes());
    f.print("out", 3);
    f.feed("● Bash(make test)\r\n".as_bytes());
    let header = f.cursor() - 1;
    f.print("spin", 4);
    let anchor = f.cursor();
    f.print("later", 20);
    assert_eq!(f.lines.find_above(&mut f.term, anchor, 60, &["Bash(make"]), Some(header), "the nearest one");
    assert_eq!(f.lines.find_above(&mut f.term, anchor, 3, &["Bash(make"]), None, "out of reach");
    assert_eq!(f.lines.find_above(&mut f.term, anchor, 60, &["nothing"]), None);
    assert_eq!(f.lines.find_above(&mut f.term, anchor, 60, &["spin3"]), Some(anchor - 1));
    assert_eq!(f.jump(header).as_deref(), Some("● Bash(make test)"));
}

#[test]
fn finding_skips_wide_char_spacers_and_cleared_lines() {
    let mut f = Fixture::new(30, 5, 100);
    let old = anchored(&mut f, "修复 divide");
    f.print("x", 8);
    let anchor = f.cursor();
    assert_eq!(f.lines.find_above(&mut f.term, anchor, 60, &["修复 divide"]), Some(old));
    f.lines.clear_history(&mut f.term);
    assert_eq!(f.lines.find_above(&mut f.term, anchor, 60, &["修复 divide"]), None, "cleared");
}

#[test]
fn every_needle_must_be_on_the_line() {
    // Claude: 「Read(docs/notes.md)」 then 「Update(docs/notes.md)」 below it, both above the Read's anchor.
    let mut f = Fixture::new(40, 5, 100);
    let read = anchored(&mut f, "● Read(docs/notes.md)");
    f.feed("  ⎿  Read 3 lines\r\n".as_bytes());
    f.feed("● Update(docs/notes.md)\r\n".as_bytes());
    let anchor = f.cursor();
    f.print("later", 10);
    assert_eq!(f.lines.find_above(&mut f.term, anchor, 60, &["Read(", "notes.md"]), Some(read));
    assert_eq!(f.lines.find_above(&mut f.term, anchor, 60, &["notes.md"]), Some(read + 2), "the name alone is ambiguous");
}

fn text(f: &mut Fixture, start: u64, end: u64, max: usize) -> Option<String> {
    f.lines.lines_text(&mut f.term, start, end, max)
}

#[test]
fn text_of_a_range() {
    let mut f = Fixture::new(20, 10, 1000);
    f.print("a", 3);
    let start = f.cursor();
    f.feed(b"x1\r\nx2   \r\n");
    let end = f.cursor();
    assert_eq!(text(&mut f, start, end, 40).as_deref(), Some("x1\nx2"));
    assert_eq!(text(&mut f, start, end, 1).as_deref(), Some("x2"), "the last lines win");
    assert_eq!(text(&mut f, end, end, 40), None, "empty range");
}

#[test]
fn text_from_scrollback() {
    let mut f = Fixture::new(20, 5, 1000);
    let start = f.cursor();
    f.feed(b"OUT1\r\nOUT2\r\n");
    let end = f.cursor();
    f.print("b", 100);
    assert_eq!(text(&mut f, start, end, 40).as_deref(), Some("OUT1\nOUT2"));
}

#[test]
fn soft_wrapped_rows_are_one_line() {
    let mut f = Fixture::new(10, 5, 1000);
    let start = f.cursor();
    f.feed(b"0123456789abcdefghij-tail\r\n");
    let end = f.cursor();
    assert_eq!(text(&mut f, start, end, 40).as_deref(), Some("0123456789abcdefghij-tail"));
}

#[test]
fn evicted_start_is_clipped() {
    let mut f = Fixture::new(20, 5, 50);
    let start = f.cursor();
    f.print("e", 120);
    let end = f.cursor();
    let got = text(&mut f, start, end, 1000).unwrap();
    assert!(!got.contains("e0\n"), "evicted lines are not read");
    assert!(got.ends_with("e119"), "{got}");
}

#[test]
fn cleared_history_is_gone() {
    let mut f = Fixture::new(20, 5, 1000);
    let start = f.cursor();
    f.feed(b"OLD\r\n");
    let end = f.cursor();
    f.print("b", 20);
    f.lines.clear_history(&mut f.term);
    assert_eq!(text(&mut f, start, end, 40), None);
}

#[test]
fn alt_screen_has_no_text() {
    let mut f = Fixture::new(20, 5, 1000);
    let start = f.cursor();
    f.feed(b"main\r\n");
    let end = f.cursor();
    f.feed(b"\x1b[?1049h");
    assert_eq!(text(&mut f, start, end, 40), None);
}

#[test]
fn long_range_keeps_only_the_last_lines() {
    let mut f = Fixture::new(10, 5, 10_000);
    let start = f.cursor();
    f.print("L", 4998);
    // The second-to-last line is soft-wrapped across two rows; empty lines at the end are dropped.
    f.feed(b"0123456789wrapped\r\nlast\r\n\r\n\r\n");
    let end = f.cursor();
    assert_eq!(text(&mut f, start, end, 3).as_deref(), Some("L4997\n0123456789wrapped\nlast"));
    assert_eq!(text(&mut f, start, end, 2).as_deref(), Some("0123456789wrapped\nlast"));
    let all = text(&mut f, start, end, 100_000).unwrap();
    assert_eq!(all.lines().count(), 5000);
    assert!(all.starts_with("L0\nL1\n"), "{:.20}", all);
}
