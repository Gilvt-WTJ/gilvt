//! Commands run at a shell prompt, from OSC 133 marks (C … D) and the command line the hook sends with C.
//! In memory only. The output tail is captured from the PTY bytes between C and D (`capture`), so its edges
//! are exact; the line numbers kept for jumping back come from the grid when the marks reach the UI and can
//! be a line off.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::listener::TermEvent;
use crate::session::TermSession;
use crate::shellmarks::PromptMark;

/// Blocks kept per pane.
pub const MAX_BLOCKS: usize = 50;
/// Output lines kept per block.
pub const TAIL_LINES: usize = 40;
/// Output bytes kept per block.
pub const TAIL_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq)]
pub struct CommandBlock {
    /// Increasing within the pane.
    pub id: u64,
    /// As typed; None when the shell did not send it.
    pub command: Option<String>,
    /// The shell's directory when the command started (the last OSC 7).
    pub cwd: Option<PathBuf>,
    /// The directory the shell reported after the command ended (the first OSC 7 after D, known at the next
    /// prompt or the next C); None until then. A command may `cd` somewhere and work there.
    pub end_cwd: Option<PathBuf>,
    pub started: SystemTime,
    /// None while it runs.
    pub ended: Option<SystemTime>,
    /// None while it runs, or when its end was not seen (another C came first).
    pub exit: Option<i32>,
    /// The last lines of its output; None on the alternate screen or when unreadable.
    pub output_tail: Option<String>,
    /// Absolute lines (`TermSession::absolute_cursor_line`) at C and D.
    pub start_line: Option<u64>,
    pub end_line: Option<u64>,
}

impl CommandBlock {
    pub fn running(&self) -> bool {
        self.ended.is_none()
    }

    /// Ended with a non-zero exit code.
    pub fn failed(&self) -> bool {
        self.exit.is_some_and(|c| c != 0)
    }
}

/// What the caller knows when a mark arrives.
#[derive(Clone, Debug)]
pub struct MarkCtx {
    pub now: SystemTime,
    pub cwd: Option<PathBuf>,
    pub line: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkEffect {
    None,
    Started,
    /// Block `id` ended: read its output between these lines.
    Ended { id: u64, start_line: Option<u64>, end_line: Option<u64> },
}

#[derive(Debug, Default)]
pub struct CommandLog {
    /// Oldest first.
    blocks: VecDeque<CommandBlock>,
    next_id: u64,
    /// The command line sent just before the C mark it belongs to.
    pending: Option<String>,
    /// The output captured for the command whose D comes next.
    pending_output: Option<Option<String>>,
}

impl CommandLog {
    pub fn on_cmdline(&mut self, text: String) {
        self.pending = Some(text);
    }

    /// The captured output of the command whose D comes next (`TermEvent::CommandOutput`).
    pub fn on_output(&mut self, text: Option<String>) {
        self.pending_output = Some(text);
    }

    pub fn on_mark(&mut self, mark: PromptMark, ctx: MarkCtx) -> MarkEffect {
        match mark {
            PromptMark::CommandStart => {
                self.pending_output = None;
                if let Some(last) = self.blocks.back_mut() {
                    if last.running() {
                        last.ended = Some(ctx.now);
                    }
                    if last.end_cwd.is_none() {
                        last.end_cwd = ctx.cwd.clone();
                    }
                }
                self.next_id += 1;
                self.blocks.push_back(CommandBlock {
                    id: self.next_id,
                    command: self.pending.take(),
                    cwd: ctx.cwd,
                    end_cwd: None,
                    started: ctx.now,
                    ended: None,
                    exit: None,
                    output_tail: None,
                    start_line: ctx.line,
                    end_line: None,
                });
                while self.blocks.len() > MAX_BLOCKS {
                    self.blocks.pop_front();
                }
                MarkEffect::Started
            }
            PromptMark::CommandEnd(exit) => match self.blocks.back_mut().filter(|b| b.running()) {
                Some(b) => {
                    b.ended = Some(ctx.now);
                    b.exit = exit;
                    b.end_line = ctx.line;
                    b.output_tail = self.pending_output.take().flatten();
                    MarkEffect::Ended { id: b.id, start_line: b.start_line, end_line: b.end_line }
                }
                None => {
                    self.pending_output = None;
                    MarkEffect::None
                }
            },
            PromptMark::PromptStart => {
                self.pending = None;
                if let Some(last) = self.blocks.back_mut().filter(|b| !b.running() && b.end_cwd.is_none()) {
                    last.end_cwd = ctx.cwd;
                }
                MarkEffect::None
            }
            PromptMark::InputStart => MarkEffect::None,
        }
    }

    pub fn set_output(&mut self, id: u64, text: Option<String>) {
        if let Some(b) = self.blocks.iter_mut().find(|b| b.id == id) {
            b.output_tail = text;
        }
    }

    pub fn running(&self) -> Option<&CommandBlock> {
        self.blocks.back().filter(|b| b.running())
    }

    pub fn last_finished(&self) -> Option<&CommandBlock> {
        self.blocks.iter().rev().find(|b| !b.running())
    }

    /// The newest `n` blocks, newest first.
    pub fn recent(&self, n: usize) -> Vec<CommandBlock> {
        self.blocks.iter().rev().take(n).cloned().collect()
    }

    /// Feeds one terminal event; takes the cursor line at C / D from `session`. Returns whether the log changed.
    /// Must not be called while holding `session.term().lock()`.
    pub fn observe(&mut self, event: &TermEvent, session: &TermSession, cwd: Option<PathBuf>) -> bool {
        match event {
            TermEvent::CommandLine(text) => {
                self.on_cmdline(text.clone());
                false
            }
            TermEvent::CommandOutput(text) => {
                self.on_output(text.clone());
                false
            }
            TermEvent::Prompt(mark) => {
                let line = matches!(mark, PromptMark::CommandStart | PromptMark::CommandEnd(_)).then(|| session.absolute_cursor_line()).flatten();
                !matches!(self.on_mark(*mark, MarkCtx { now: SystemTime::now(), cwd, line }), MarkEffect::None)
            }
            _ => false,
        }
    }
}

/// The last `max_bytes` bytes of `s`, starting on a char boundary.
pub fn clip_tail(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut start = s.len() - max_bytes;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ctx(secs: u64, line: Option<u64>) -> MarkCtx {
        MarkCtx { now: SystemTime::UNIX_EPOCH + Duration::from_secs(secs), cwd: Some("/w".into()), line }
    }

    #[test]
    fn a_command_from_c_to_d() {
        let mut log = CommandLog::default();
        log.on_cmdline("make test".into());
        assert_eq!(log.on_mark(PromptMark::CommandStart, ctx(10, Some(5))), MarkEffect::Started);
        let r = log.running().unwrap();
        assert_eq!(r.command.as_deref(), Some("make test"));
        assert_eq!(r.cwd.as_deref(), Some(std::path::Path::new("/w")));
        assert!(r.running());
        let effect = log.on_mark(PromptMark::CommandEnd(Some(2)), ctx(13, Some(9)));
        let MarkEffect::Ended { id, start_line, end_line } = effect else { panic!("{effect:?}") };
        assert_eq!((start_line, end_line), (Some(5), Some(9)));
        log.set_output(id, Some("error: boom".into()));
        assert!(log.running().is_none());
        let b = log.last_finished().unwrap();
        assert_eq!(b.exit, Some(2));
        assert!(b.failed());
        assert_eq!(b.output_tail.as_deref(), Some("error: boom"));
        assert_eq!(b.ended, Some(SystemTime::UNIX_EPOCH + Duration::from_secs(13)));
    }

    fn at(secs: u64, cwd: &str) -> MarkCtx {
        MarkCtx { now: SystemTime::UNIX_EPOCH + Duration::from_secs(secs), cwd: Some(cwd.into()), line: None }
    }

    #[test]
    fn the_directory_after_a_command_is_recorded() {
        let mut log = CommandLog::default();
        log.on_cmdline("cd secret && cat notes".into());
        log.on_mark(PromptMark::CommandStart, at(1, "/w"));
        log.on_mark(PromptMark::CommandEnd(Some(0)), at(2, "/w"));
        assert_eq!(log.last_finished().unwrap().end_cwd, None, "the shell reports it after D");
        // D, then OSC 7, then A: the cwd known at A is the one the command left.
        log.on_mark(PromptMark::PromptStart, at(3, "/w/secret"));
        assert_eq!(log.last_finished().unwrap().end_cwd.as_deref(), Some(std::path::Path::new("/w/secret")));
        log.on_mark(PromptMark::PromptStart, at(4, "/elsewhere"));
        log.on_mark(PromptMark::CommandStart, at(5, "/elsewhere"));
        assert_eq!(log.recent(2)[1].end_cwd.as_deref(), Some(std::path::Path::new("/w/secret")), "the first report wins");
    }

    #[test]
    fn without_a_prompt_mark_the_next_start_tells_the_directory() {
        let mut log = CommandLog::default();
        log.on_mark(PromptMark::CommandStart, at(1, "/w"));
        log.on_mark(PromptMark::CommandEnd(Some(0)), at(2, "/w"));
        log.on_mark(PromptMark::CommandStart, at(3, "/w/secret"));
        assert_eq!(log.recent(2)[1].end_cwd.as_deref(), Some(std::path::Path::new("/w/secret")));
        assert_eq!(log.recent(2)[0].end_cwd, None, "still running");
    }

    #[test]
    fn c_without_d_closes_the_previous_block() {
        let mut log = CommandLog::default();
        log.on_mark(PromptMark::CommandStart, ctx(1, None));
        log.on_mark(PromptMark::CommandStart, ctx(2, None));
        let recent = log.recent(10);
        assert_eq!(recent.len(), 2);
        assert!(recent[0].running(), "the new one runs");
        assert!(!recent[1].running(), "the old one is closed");
        assert_eq!(recent[1].exit, None, "with an unknown end");
    }

    #[test]
    fn stray_d_is_ignored() {
        let mut log = CommandLog::default();
        assert_eq!(log.on_mark(PromptMark::CommandEnd(Some(0)), ctx(1, None)), MarkEffect::None);
        assert!(log.recent(10).is_empty());
    }

    #[test]
    fn stale_cmdline_is_dropped_at_prompt() {
        let mut log = CommandLog::default();
        log.on_cmdline("old".into());
        log.on_mark(PromptMark::PromptStart, ctx(1, None));
        log.on_mark(PromptMark::CommandStart, ctx(2, None));
        assert_eq!(log.running().unwrap().command, None);
    }

    #[test]
    fn keeps_the_newest_fifty() {
        let mut log = CommandLog::default();
        for i in 0..60 {
            log.on_cmdline(format!("c{i}"));
            log.on_mark(PromptMark::CommandStart, ctx(i, None));
            log.on_mark(PromptMark::CommandEnd(Some(0)), ctx(i, None));
        }
        let all = log.recent(100);
        assert_eq!(all.len(), MAX_BLOCKS);
        assert_eq!(all[0].command.as_deref(), Some("c59"));
        assert_eq!(all[MAX_BLOCKS - 1].command.as_deref(), Some("c10"));
    }

    #[test]
    fn tail_is_clipped_from_the_end_on_a_char_boundary() {
        assert_eq!(clip_tail("abc", 10), "abc");
        let s = format!("{}末尾", "中".repeat(10));
        let got = clip_tail(&s, 7);
        assert!(got.len() <= 7);
        assert!(got.ends_with("末尾"), "{got}");
    }

    #[test]
    fn captured_output_fills_the_ending_block() {
        let mut log = CommandLog::default();
        log.on_mark(PromptMark::CommandStart, ctx(1, None));
        log.on_output(Some("FAIL pkg/cache".into()));
        log.on_mark(PromptMark::CommandEnd(Some(2)), ctx(2, None));
        assert_eq!(log.last_finished().unwrap().output_tail.as_deref(), Some("FAIL pkg/cache"));
    }

    #[test]
    fn output_without_a_running_block_is_dropped() {
        let mut log = CommandLog::default();
        log.on_output(Some("stray".into()));
        log.on_mark(PromptMark::CommandStart, ctx(1, None));
        log.on_mark(PromptMark::CommandEnd(Some(0)), ctx(2, None));
        assert_eq!(log.last_finished().unwrap().output_tail, None);
    }
}
