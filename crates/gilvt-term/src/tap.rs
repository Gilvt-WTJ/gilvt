//! A PTY wrapper that lets us observe the output byte stream before alacritty parses it.

use std::fs::File;
use std::io::{self, Read};
use std::sync::Arc;

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite, Pty};
use polling::{Event, PollMode, Poller};

use crate::listener::TermEvent;
use crate::osc::{parse_notification, OscScanner};
use crate::blocks::{TAIL_BYTES, TAIL_LINES};
use crate::capture::OutputCapture;
use crate::shellmarks::{parse_cmdline, parse_cwd, parse_prompt_mark, PromptMark};

/// What the PTY bytes say before alacritty parses them: OSC events, and the output of each command between
/// its C and D marks.
#[derive(Default)]
pub struct Tap {
    scanner: OscScanner,
    capture: OutputCapture,
}

impl std::fmt::Debug for Tap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tap").finish_non_exhaustive()
    }
}

impl Tap {
    /// The events `bytes` carry, in order. A D mark is preceded by `CommandOutput` with the bytes since C.
    pub fn process(&mut self, bytes: &[u8]) -> Vec<TermEvent> {
        let mut events = Vec::new();
        let mut pos = 0;
        for (end, seq) in self.scanner.feed_at(bytes) {
            self.capture.feed(&bytes[pos..end]);
            pos = end;
            if let Some(n) = parse_notification(&seq) {
                events.push(TermEvent::Notification(n));
            } else if let Some(c) = parse_cwd(&seq) {
                events.push(TermEvent::Cwd(c));
            } else if let Some(m) = parse_prompt_mark(&seq) {
                if let Some(line) = parse_cmdline(&seq) {
                    events.push(TermEvent::CommandLine(line));
                }
                match m {
                    PromptMark::CommandStart => self.capture.start(),
                    PromptMark::CommandEnd(_) => events.push(TermEvent::CommandOutput(self.capture.finish(TAIL_LINES, TAIL_BYTES))),
                    PromptMark::PromptStart | PromptMark::InputStart => {}
                }
                events.push(TermEvent::Prompt(m));
            }
        }
        self.capture.feed(&bytes[pos..]);
        events
    }
}

pub struct TapReader {
    file: File,
    tap: Tap,
    events: async_channel::Sender<TermEvent>,
}

impl Read for TapReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read(buf)?;
        for event in self.tap.process(&buf[..n]) {
            let _ = self.events.try_send(event);
        }
        Ok(n)
    }
}

pub struct TapPty {
    inner: Pty,
    reader: TapReader,
}

impl TapPty {
    pub fn new(inner: Pty, events: async_channel::Sender<TermEvent>) -> io::Result<Self> {
        // The clone shares the open file description (and its O_NONBLOCK flag) with the original.
        let file = inner.file().try_clone()?;
        Ok(Self { inner, reader: TapReader { file, tap: Tap::default(), events } })
    }
}

impl EventedReadWrite for TapPty {
    type Reader = TapReader;
    type Writer = File;

    unsafe fn register(&mut self, poll: &Arc<Poller>, interest: Event, mode: PollMode) -> io::Result<()> {
        unsafe { self.inner.register(poll, interest, mode) }
    }

    fn reregister(&mut self, poll: &Arc<Poller>, interest: Event, mode: PollMode) -> io::Result<()> {
        self.inner.reregister(poll, interest, mode)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        self.inner.deregister(poll)
    }

    fn reader(&mut self) -> &mut TapReader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut File {
        self.inner.writer()
    }
}

impl EventedPty for TapPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.inner.next_child_event()
    }
}

impl OnResize for TapPty {
    fn on_resize(&mut self, size: WindowSize) {
        self.inner.on_resize(size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shellmarks::PromptMark;

    fn kinds(events: &[TermEvent]) -> Vec<String> {
        events.iter().map(|e| format!("{e:?}")).collect()
    }

    #[test]
    fn output_stops_at_d_even_with_the_next_prompt_in_the_same_read() {
        let mut tap = Tap::default();
        let ev = tap.process(b"\x1b]133;C;cmdline_url=make%20test\x07FAIL pkg/cache\r\n\x1b]133;D;2\x07\x1b]133;A\x07sandbox$ ");
        assert_eq!(
            kinds(&ev),
            vec![
                "CommandLine(\"make test\")".to_string(),
                "Prompt(CommandStart)".to_string(),
                "CommandOutput(Some(\"FAIL pkg/cache\"))".to_string(),
                "Prompt(CommandEnd(Some(2)))".to_string(),
                "Prompt(PromptStart)".to_string(),
            ]
        );
    }

    #[test]
    fn output_split_across_reads() {
        let mut tap = Tap::default();
        assert_eq!(kinds(&tap.process(b"\x1b]133;C\x07par")), vec!["Prompt(CommandStart)"]);
        assert!(tap.process(b"tial\r\nmore\r\n\x1b]13").is_empty());
        let ev = tap.process(b"3;D;0\x07");
        assert_eq!(kinds(&ev), vec!["CommandOutput(Some(\"partial\\nmore\"))", "Prompt(CommandEnd(Some(0)))"]);
    }

    #[test]
    fn prompt_text_before_c_is_not_output() {
        let mut tap = Tap::default();
        let ev = tap.process(b"sandbox$ ls\r\n\x1b]133;C\x07a b\r\n\x1b]133;D;0\x07");
        assert!(matches!(&ev[1], TermEvent::CommandOutput(Some(t)) if t == "a b"), "{:?}", kinds(&ev));
    }

    #[test]
    fn stray_d_has_no_output() {
        let mut tap = Tap::default();
        let ev = tap.process(b"\x1b]133;D;0\x07");
        assert!(matches!(ev[0], TermEvent::CommandOutput(None)));
        assert!(matches!(ev[1], TermEvent::Prompt(PromptMark::CommandEnd(Some(0)))));
    }

    #[test]
    fn notifications_and_cwd_still_pass() {
        let mut tap = Tap::default();
        let ev = tap.process(b"\x1b]9;done\x07\x1b]7;file://localhost/tmp\x07");
        assert!(matches!(ev[0], TermEvent::Notification(_)));
        assert!(matches!(ev[1], TermEvent::Cwd(_)));
    }
}
