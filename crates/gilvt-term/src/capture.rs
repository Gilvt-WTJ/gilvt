//! The output of a command run at a shell prompt, taken from the PTY bytes between its OSC 133 C and D marks
//! (before alacritty parses them, so its edges are exact) and turned into plain text.

use std::collections::VecDeque;

use alacritty_terminal::vte::{Params, Parser, Perform};

use crate::blocks::clip_tail;

/// Raw bytes kept per command (the newest).
pub const CAPTURE_BYTES: usize = 64 * 1024;

pub struct OutputCapture {
    active: bool,
    buf: VecDeque<u8>,
    alt: bool,
    alt_parser: Parser,
    alt_detector: AltDetector,
}

impl core::fmt::Debug for OutputCapture {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OutputCapture")
            .field("active", &self.active)
            .field("alt", &self.alt)
            .finish()
    }
}

impl Default for OutputCapture {
    fn default() -> Self {
        Self {
            active: false,
            buf: VecDeque::new(),
            alt: false,
            alt_parser: Parser::new(),
            alt_detector: AltDetector::default(),
        }
    }
}

impl OutputCapture {
    /// A command started (OSC 133;C): collect from here.
    pub fn start(&mut self) {
        self.active = true;
        self.buf.clear();
        self.alt = false;
        // A sequence cut off by the last command's D must not continue into this one.
        self.alt_parser = Parser::new();
        self.alt_detector.reset();
    }

    /// PTY bytes; kept only while a command runs, the newest [`CAPTURE_BYTES`].
    pub fn feed(&mut self, bytes: &[u8]) {
        if !self.active {
            return;
        }
        // Detect alt-screen sequences even across feed calls
        self.alt_parser.advance(&mut self.alt_detector, bytes);
        if self.alt_detector.saw_alt {
            self.alt = true;
        }

        self.buf.extend(bytes);
        let over = self.buf.len().saturating_sub(CAPTURE_BYTES);
        self.buf.drain(..over);
    }

    /// The command ended (OSC 133;D): its output as text (see [`render`]); None when none was running.
    pub fn finish(&mut self, max_lines: usize, max_bytes: usize) -> Option<String> {
        if !self.active {
            return None;
        }
        self.active = false;

        // If we saw an alt-screen sequence, return None regardless of buffer content
        if self.alt {
            return None;
        }

        let bytes: Vec<u8> = std::mem::take(&mut self.buf).into();
        render(&bytes, max_lines, max_bytes)
    }
}

#[derive(Debug, Default)]
struct AltDetector {
    saw_alt: bool,
}

impl AltDetector {
    fn reset(&mut self) {
        self.saw_alt = false;
    }
}

impl Perform for AltDetector {
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        match (intermediates, action) {
            ([b'?'], 'h') if params.iter().any(|p| matches!(p.first(), Some(1049 | 1047 | 47))) => {
                self.saw_alt = true;
            }
            _ => {}
        }
    }
}

/// `bytes` as the text they leave on a line-oriented screen: escape sequences dropped, `\r` returns to the
/// line start (later characters overwrite), `ESC[K` erases, tabs stop every 8 columns. The last `max_lines`
/// non-blank-edged lines, clipped to `max_bytes` from the end. None when the program switched to the
/// alternate screen (a full-screen program's output is not a log).
pub fn render(bytes: &[u8], max_lines: usize, max_bytes: usize) -> Option<String> {
    let mut screen = Plain::default();
    Parser::new().advance(&mut screen, bytes);
    if screen.alt {
        return None;
    }
    screen.newline();
    let mut lines: Vec<String> = screen.lines.into_iter().map(|l| l.trim_end().to_string()).collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let first = lines.iter().position(|l| !l.is_empty()).unwrap_or(lines.len());
    let lines = &lines[first..];
    let start = lines.len().saturating_sub(max_lines);
    Some(clip_tail(&lines[start..].join("\n"), max_bytes))
}

#[derive(Default)]
struct Plain {
    lines: Vec<String>,
    line: Vec<char>,
    col: usize,
    alt: bool,
}

impl Plain {
    fn newline(&mut self) {
        self.lines.push(self.line.iter().collect());
        self.line.clear();
        self.col = 0;
    }

    fn put(&mut self, c: char) {
        while self.line.len() < self.col {
            self.line.push(' ');
        }
        if self.col < self.line.len() {
            self.line[self.col] = c;
        } else {
            self.line.push(c);
        }
        self.col += 1;
    }
}

impl Perform for Plain {
    fn print(&mut self, c: char) {
        self.put(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.col = 0,
            b'\t' => {
                let next = (self.col / 8 + 1) * 8;
                while self.col < next {
                    self.put(' ');
                }
            }
            0x08 => self.col = self.col.saturating_sub(1),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let first = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0);
        match (intermediates, action) {
            ([b'?'], 'h') if params.iter().any(|p| matches!(p.first(), Some(1049 | 1047 | 47))) => self.alt = true,
            ([], 'K') => match first {
                0 => self.line.truncate(self.col),
                1 => self.line.iter_mut().take(self.col + 1).for_each(|c| *c = ' '),
                _ => self.line.clear(),
            },
            ([], 'G') => self.col = usize::from(first.max(1)) - 1,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(bytes: &[u8]) -> Option<String> {
        render(bytes, 40, 4096)
    }

    #[test]
    fn a_new_command_does_not_continue_the_last_ones_escape_sequence() {
        let mut c = OutputCapture::default();
        c.start();
        c.feed(b"out\x1b[?104");
        assert_eq!(c.finish(40, 4096).as_deref(), Some("out"));
        c.start();
        // Would complete `ESC [ ? 1049 h` (alternate screen) if the parser kept the cut sequence.
        c.feed(b"9h done\r\n");
        assert!(c.finish(40, 4096).is_some(), "the new command's output is kept");
    }

    #[test]
    fn plain_lines() {
        assert_eq!(r(b"one\r\ntwo\r\n").as_deref(), Some("one\ntwo"));
    }

    #[test]
    fn carriage_return_overwrites() {
        assert_eq!(r(b" 10%\r 50%\r100%\r\ndone\r\n").as_deref(), Some("100%\ndone"));
    }

    #[test]
    fn erase_line_after_carriage_return() {
        assert_eq!(r(b"downloading 1234567\r\x1b[Kok\r\n").as_deref(), Some("ok"));
    }

    #[test]
    fn colors_are_stripped() {
        assert_eq!(r(b"\x1b[31merror\x1b[0m: \x1b[1mboom\x1b[22m\r\n").as_deref(), Some("error: boom"));
    }

    #[test]
    fn osc_sequences_are_dropped() {
        assert_eq!(r(b"a\x1b]133;D;1\x07\x1b]7;file:///x\x07").as_deref(), Some("a"));
    }

    #[test]
    fn tabs_and_backspace() {
        assert_eq!(r(b"a\tb\r\nab\x08c\r\n").as_deref(), Some("a       b\nac"));
    }

    #[test]
    fn leading_and_trailing_blank_lines_go() {
        assert_eq!(r(b"\r\n\r\nx   \r\n\r\n").as_deref(), Some("x"));
    }

    #[test]
    fn alternate_screen_has_no_output() {
        assert_eq!(r(b"\x1b[?1049hvim stuff\x1b[?1049l"), None);
        assert_eq!(r(b"\x1b[?47h"), None);
    }

    #[test]
    fn keeps_the_last_lines_and_bytes() {
        let many: String = (1..=50).map(|i| format!("line {i}\r\n")).collect();
        let out = render(many.as_bytes(), 3, 4096).unwrap();
        assert_eq!(out, "line 48\nline 49\nline 50");
        let wide = format!("{}\r\n末尾\r\n", "中".repeat(100));
        let out = render(wide.as_bytes(), 40, 10).unwrap();
        assert!(out.len() <= 10 && out.ends_with("末尾"), "{out:?}");
    }

    #[test]
    fn empty_output_is_empty_text() {
        assert_eq!(r(b"").as_deref(), Some(""));
    }

    #[test]
    fn capture_only_between_start_and_finish() {
        let mut c = OutputCapture::default();
        c.feed(b"before\r\n");
        assert_eq!(c.finish(40, 4096), None, "never started");
        c.start();
        c.feed(b"hel");
        c.feed(b"lo\r\n");
        assert_eq!(c.finish(40, 4096).as_deref(), Some("hello"));
        assert_eq!(c.finish(40, 4096), None, "finished once");
        c.feed(b"after\r\n");
        c.start();
        assert_eq!(c.finish(40, 4096).as_deref(), Some(""), "start clears");
    }

    #[test]
    fn capture_keeps_the_newest_bytes() {
        let mut c = OutputCapture::default();
        c.start();
        c.feed(&vec![b'x'; CAPTURE_BYTES]);
        c.feed(b"\r\nlast\r\n");
        let out = c.finish(1, 4096).unwrap();
        assert_eq!(out, "last");
    }

    #[test]
    fn alternate_screen_is_remembered_past_the_buffer() {
        // After alt-screen entry, feed more than CAPTURE_BYTES to discard the sequence
        // The sticky alt flag should still cause finish to return None
        let mut c = OutputCapture::default();
        c.start();
        c.feed(b"\x1b[?1049h");
        c.feed(&vec![b'x'; CAPTURE_BYTES + 10]);
        c.feed(b"\r\nmore\r\n");
        assert_eq!(c.finish(40, 4096), None, "alt-screen should be remembered even after buffer overflow");
    }

    #[test]
    fn alternate_screen_entry_split_across_feeds() {
        // Entry sequence split: `\x1b[?10` in one feed, `49h` in the next
        let mut c = OutputCapture::default();
        c.start();
        c.feed(b"prefix\r\n\x1b[?10");
        c.feed(b"49h\r\nafter\r\n");
        assert_eq!(c.finish(40, 4096), None, "split alt-screen sequence should be detected");
    }

    #[test]
    fn alternate_screen_with_multiple_params() {
        // Entry sequence `\x1b[?1;1049h` has multiple params
        let mut c = OutputCapture::default();
        c.start();
        c.feed(b"text\r\n\x1b[?1;1049h");
        c.feed(b"vim stuff\r\n");
        assert_eq!(c.finish(40, 4096), None, "alt-screen with multiple params should be detected");
    }
}
