use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::ops::Range;
use std::sync::{Arc, Mutex, MutexGuard};

use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Direction, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::search::{Match, RegexSearch};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::tty::{self, Shell};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

use crate::lines::{LineTracker, ScrollOutcome};
use crate::listener::{EventProxy, TermEvent};
use crate::palette::{Palette, Rgb};
use crate::size::TermSize;
use crate::tap::TapPty;

#[derive(Clone, Debug)]
pub struct SessionOptions {
    /// Program to run; `None` runs the user's login shell.
    pub program: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Extra environment for the child, on top of TERM/COLORTERM/TERM_PROGRAM.
    pub env: HashMap<String, String>,
    pub scrollback: usize,
    pub kitty_keyboard: bool,
    pub size: TermSize,
}

impl SessionOptions {
    pub fn new(size: TermSize) -> Self {
        Self {
            program: None,
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            scrollback: 100_000,
            kitty_keyboard: true,
            size,
        }
    }
}

pub type SharedTerm = Arc<FairMutex<Term<EventProxy>>>;

/// Color reported for an OSC 4/10/11/12 query: runtime override if set, else the theme color.
/// Indices follow alacritty: 0..=255 palette entries, 256 foreground, 257 background, 258 cursor.
fn query_color(index: usize, palette: &Palette, overrides: &Colors) -> Rgb {
    let color = match index {
        0..=255 => Color::Indexed(index as u8),
        257 => Color::Named(NamedColor::Background),
        258 => Color::Named(NamedColor::Cursor),
        _ => Color::Named(NamedColor::Foreground),
    };
    palette.resolve(color, overrides)
}

/// One PTY + terminal state. Output is parsed on alacritty's I/O thread.
pub struct TermSession {
    term: SharedTerm,
    sender: EventLoopSender,
    events: async_channel::Receiver<TermEvent>,
    size: TermSize,
    search: Option<RegexSearch>,
    search_match: Option<Match>,
    /// Duplicate of the PTY master, used for foreground-process queries.
    master: File,
    /// Absolute line numbering of the main screen (lock order: this, then `term`).
    lines: Mutex<LineTracker>,
}

impl TermSession {
    pub fn spawn(opts: SessionOptions) -> io::Result<Self> {
        let (tx, rx) = async_channel::unbounded();
        let proxy = EventProxy(tx.clone());
        let config = Config {
            scrolling_history: opts.scrollback,
            kitty_keyboard: opts.kitty_keyboard,
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &opts.size, proxy.clone())));

        let mut env = opts.env;
        env.insert("TERM".into(), "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        env.insert("TERM_PROGRAM".into(), "gilvt".into());
        env.insert("TERM_PROGRAM_VERSION".into(), env!("CARGO_PKG_VERSION").into());
        if !env.contains_key("LANG") {
            if let Some(lang) = crate::locale::detect_child_lang() {
                env.insert("LANG".into(), lang);
            }
        }
        let tty_opts = tty::Options {
            shell: opts.program.map(|p| Shell::new(p, opts.args)),
            working_directory: opts.cwd,
            drain_on_exit: true,
            env,
        };
        let pty = tty::new(&tty_opts, opts.size.into(), 0)?;
        let master = pty.file().try_clone()?;
        let pty = TapPty::new(pty, tx)?;
        let event_loop = EventLoop::new(term.clone(), proxy, pty, true, false)?;
        let sender = event_loop.channel();
        event_loop.spawn();

        let lines = Mutex::new(LineTracker::new(opts.scrollback));
        Ok(Self { term, sender, events: rx, size: opts.size, search: None, search_match: None, master, lines })
    }

    pub fn events(&self) -> async_channel::Receiver<TermEvent> {
        self.events.clone()
    }

    pub fn term(&self) -> &SharedTerm {
        &self.term
    }

    /// Process group currently in the foreground of the PTY (the shell, or the program it runs).
    /// The process-group id is used as the pid of its leader; if the leader of a pipeline exits
    /// first, lookups by this id fail and callers fall back to their defaults.
    pub fn foreground_pid(&self) -> Option<u32> {
        let pgid = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
        (pgid > 0).then_some(pgid as u32)
    }

    /// Whether the foreground program reads keys raw (canonical mode off): a line editor (zle,
    /// readline), a TUI (Claude Code, Codex, vim). False while a plain command such as `sleep`
    /// runs on the cooked tty, where arrow keys would only be echoed back as `^[[D`.
    pub fn reads_raw_keys(&self) -> bool {
        let mut t = std::mem::MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(self.master.as_raw_fd(), t.as_mut_ptr()) } != 0 {
            return false;
        }
        unsafe { t.assume_init() }.c_lflag & libc::ICANON == 0
    }

    /// Working directory of the foreground process.
    pub fn cwd(&self) -> Option<std::path::PathBuf> {
        crate::procinfo::process_cwd(self.foreground_pid()?)
    }

    /// The last `n` logical lines (soft-wrapped rows joined) of the visible screen as plain text ([`crate::snapshot::screen_tail`]).
    pub fn screen_tail(&self, n: usize) -> Vec<String> {
        crate::snapshot::screen_tail(&self.term.lock(), n)
    }

    /// Name of the foreground program ("bash", "claude", ...).
    pub fn foreground_name(&self) -> Option<String> {
        crate::procinfo::process_name(self.foreground_pid()?)
    }

    pub fn size(&self) -> TermSize {
        self.size
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let _ = self.sender.send(Msg::Input(bytes.into()));
    }

    /// Writes user input: also snaps the viewport back to the bottom, like every terminal.
    pub fn write_input(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.term.lock().scroll_display(Scroll::Bottom);
        self.write(bytes);
    }

    pub fn resize(&mut self, size: TermSize) {
        if size == self.size {
            return;
        }
        self.size = size;
        let lines = self.lines.get_mut().unwrap_or_else(|e| e.into_inner());
        lines.resize(&mut *self.term.lock(), size);
        let _ = self.sender.send(Msg::Resize(size.into()));
    }

    pub fn scroll(&self, scroll: Scroll) {
        self.term.lock().scroll_display(scroll);
    }

    /// Drops the scrollback (like Cmd+K in other terminals) and any search match that pointed into it.
    pub fn clear_history(&mut self) {
        let lines = self.lines.get_mut().unwrap_or_else(|e| e.into_inner());
        lines.clear_history(&mut *self.term.lock());
        self.end_search();
    }

    // ---- absolute lines ----

    fn line_tracker(&self) -> MutexGuard<'_, LineTracker> {
        self.lines.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Absolute number of the cursor row: rows scrolled off the top since the session started
    /// plus the cursor row. Stays attached to that content through scrolling, clear screen and
    /// resizes. `None` while a program uses the alternate screen.
    ///
    /// Must not be called while holding `session.term().lock()`: this locks `term` itself, and
    /// `FairMutex` is not reentrant.
    pub fn absolute_cursor_line(&self) -> Option<u64> {
        let mut lines = self.line_tracker();
        lines.cursor_line(&mut *self.term.lock())
    }

    /// Scrolls the viewport so absolute `line` sits about a third from the top.
    ///
    /// Must not be called while holding `session.term().lock()`: this locks `term` itself, and
    /// `FairMutex` is not reentrant.
    pub fn scroll_to_absolute(&self, line: u64) -> ScrollOutcome {
        let mut lines = self.line_tracker();
        lines.scroll_to(&mut *self.term.lock(), line)
    }

    /// The nearest absolute line at or above `line` (≤ `rows` up) whose text contains all of `needles`.
    ///
    /// Must not be called while holding `session.term().lock()`: this locks `term` itself, and
    /// `FairMutex` is not reentrant.
    pub fn find_line_above(&self, line: u64, rows: usize, needles: &[&str]) -> Option<u64> {
        let mut lines = self.line_tracker();
        lines.find_above(&mut *self.term.lock(), line, rows, needles)
    }

    /// Text of absolute lines `start .. end` (see [`LineTracker::lines_text`]).
    ///
    /// Must not be called while holding `session.term().lock()`: this locks `term` itself, and
    /// `FairMutex` is not reentrant.
    pub fn lines_text(&self, start: u64, end: u64, max_lines: usize) -> Option<String> {
        let mut lines = self.line_tracker();
        lines.lines_text(&mut *self.term.lock(), start, end, max_lines)
    }

    /// Viewport rows now showing absolute lines `line .. line + count` (for a highlight overlay).
    ///
    /// Must not be called while holding `session.term().lock()`: this locks `term` itself, and
    /// `FairMutex` is not reentrant.
    pub fn absolute_rows_visible(&self, line: u64, count: usize) -> Option<Range<usize>> {
        let mut lines = self.line_tracker();
        lines.visible_rows(&mut *self.term.lock(), line, count)
    }

    /// Optional: keeps the numbering fresh (e.g. on every wakeup). Cheap; every query syncs too.
    ///
    /// Must not be called while holding `session.term().lock()`: this locks `term` itself, and
    /// `FairMutex` is not reentrant.
    pub fn sync_lines(&self) {
        let mut lines = self.line_tracker();
        lines.sync(&mut *self.term.lock());
    }

    /// Answers terminal queries that need a reply on the PTY. Returns true if handled.
    pub fn handle_reply(&self, event: &TermEvent, palette: &Palette) -> bool {
        match event {
            TermEvent::PtyWrite(text) => self.write(text.clone().into_bytes()),
            TermEvent::ColorRequest(index, format) => {
                let color = query_color(*index, palette, self.term.lock().colors());
                self.write(format(color).into_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => self.write(format(self.size.into()).into_bytes()),
            _ => return false,
        }
        true
    }

    // ---- selection ----

    pub fn start_selection(&self, point: Point, side: Side, ty: SelectionType) {
        self.term.lock().selection = Some(Selection::new(ty, point, side));
    }

    pub fn update_selection(&self, point: Point, side: Side) {
        if let Some(sel) = self.term.lock().selection.as_mut() {
            sel.update(point, side);
        }
    }

    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    // ---- search ----

    /// Starts (or replaces) a search and jumps to the nearest match above the cursor.
    pub fn search(&mut self, pattern: &str) -> Option<Match> {
        self.search = RegexSearch::new(pattern).ok();
        self.search_match = None;
        self.search_step(Direction::Left)
    }

    /// Moves to the next match in `direction` (`Left` = towards older output).
    pub fn search_step(&mut self, direction: Direction) -> Option<Match> {
        let regex = self.search.as_mut()?;
        let mut term = self.term.lock();
        let origin = match (&self.search_match, direction) {
            (Some(m), Direction::Left) => *m.start(),
            (Some(m), Direction::Right) => *m.end(),
            (None, _) => term.grid().cursor.point,
        };
        let side = if direction == Direction::Left { Side::Left } else { Side::Right };
        let origin = if self.search_match.is_some() {
            match direction {
                Direction::Left => origin.sub(&*term, alacritty_terminal::index::Boundary::None, 1),
                Direction::Right => origin.add(&*term, alacritty_terminal::index::Boundary::None, 1),
            }
        } else {
            origin
        };
        let found = term.search_next(regex, origin, direction, side, None);
        if let Some(m) = &found {
            term.scroll_to_point(*m.start());
        }
        self.search_match = found.clone();
        found
    }

    pub fn end_search(&mut self) {
        self.search = None;
        self.search_match = None;
    }

    pub fn search_match(&self) -> Option<&Match> {
        self.search_match.as_ref()
    }
}

impl Drop for TermSession {
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_color_uses_palette_for_all_indices() {
        let p = Palette::dark();
        let none = Colors::default();
        assert_eq!(query_color(1, &p, &none), p.ansi[1]);
        assert_eq!(query_color(196, &p, &none), p.resolve(Color::Indexed(196), &none));
        assert_ne!(query_color(196, &p, &none), p.foreground);
        assert_eq!(query_color(244, &p, &none), p.resolve(Color::Indexed(244), &none));
        assert_eq!(query_color(256, &p, &none), p.foreground);
        assert_eq!(query_color(257, &p, &none), p.background);
        assert_eq!(query_color(258, &p, &none), p.cursor);
    }

    #[test]
    fn query_color_prefers_runtime_override() {
        let p = Palette::dark();
        let mut colors = Colors::default();
        let custom = Rgb { r: 1, g: 2, b: 3 };
        colors[200] = Some(custom);
        assert_eq!(query_color(200, &p, &colors), custom);
    }
}
