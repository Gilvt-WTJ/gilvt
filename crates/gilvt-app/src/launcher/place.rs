//! Where a launcher command runs and when it is typed (spec §2.1). Pure: the workspace supplies the focused
//! pane and whether it is an idle shell; a new pane's terminal holds its command in a [`Queue`] until the
//! shell is ready.

use std::time::Duration;

use gilvt_term::PromptMark;

use crate::pane_tree::{Axis, PaneId};

/// Without shell integration no prompt mark will come: type after this long.
pub const PLAIN_WAIT: Duration = Duration::from_millis(800);
/// The longest a new pane waits for its first prompt mark; the command is typed anyway afterwards.
pub const MAX_WAIT: Duration = Duration::from_secs(3);

/// What the user asked for: ↩ / ⌘↩ / ⌘⇧↩.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Location {
    /// In the focused pane when it is an idle shell, else in a new tab.
    Smart,
    /// A split to the right of the focused pane.
    Right,
    /// A split below the focused pane.
    Below,
}

impl Location {
    /// The location of an ↩ press with these modifiers (⌘ = right, ⌘⇧ = below).
    pub fn from_enter(cmd: bool, shift: bool) -> Location {
        match (cmd, shift) {
            (true, true) => Location::Below,
            (true, false) => Location::Right,
            _ => Location::Smart,
        }
    }
}

/// Where a command actually runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Typed right away into this idle shell pane.
    InPlace(PaneId),
    NewTab,
    /// A new pane splitting the focused one (⌘D / ⌘⇧D).
    Split(Axis),
}

impl Placement {
    /// The ⌘⇧N preview's 「将在<…>执行：」 word.
    pub fn word(self) -> &'static str {
        match self {
            Placement::InPlace(_) => crate::i18n::text("当前 pane", "Current pane"),
            Placement::NewTab => crate::i18n::text("新标签", "New tab"),
            Placement::Split(Axis::Row) => crate::i18n::text("右侧", "Right"),
            Placement::Split(Axis::Column) => crate::i18n::text("下方", "Below"),
        }
    }
}

/// `focused`: the active tab's focused pane (None without a tab). `idle_shell(p)`: `p` is a terminal whose
/// latest foreground poll saw the shell and that has no live session (`Unknown` counts as busy).
pub fn place(location: Location, focused: Option<PaneId>, idle_shell: impl Fn(PaneId) -> bool) -> Placement {
    match (location, focused) {
        (_, None) => Placement::NewTab,
        (Location::Smart, Some(p)) if idle_shell(p) => Placement::InPlace(p),
        (Location::Smart, Some(_)) => Placement::NewTab,
        (Location::Right, Some(_)) => Placement::Split(Axis::Row),
        (Location::Below, Some(_)) => Placement::Split(Axis::Column),
    }
}

/// How long a new pane waits for its shell's first prompt mark (OSC 133;A) before typing anyway.
/// `marks_expected`: the pane's shell runs gilvt's integration, which emits the marks.
pub fn prompt_wait(marks_expected: bool) -> Duration {
    if marks_expected { MAX_WAIT } else { PLAIN_WAIT }
}

/// The bytes typed for `command`: the line plus Return, as if typed. The keys are not bracketed, so every
/// control character but a line break (Tab would trigger completion, Esc / ^C / ^U edit the line, even
/// inside quotes) is typed as a space: the shell runs what the preview showed.
pub fn typed(command: &str) -> String {
    let line: String = command.chars().map(|c| if c.is_control() && c != '\n' { ' ' } else { c }).collect();
    format!("{line}\r")
}

/// Commands waiting in one new pane for its shell. Each is typed exactly once: at the first prompt start,
/// or when the wait of the batch it joined ends, whichever comes first.
#[derive(Debug, Default)]
pub struct Queue {
    /// Bumped per batch, so a stale timer releases nothing.
    seq: u64,
    waiting: Vec<String>,
}

impl Queue {
    /// Queues `command`; returns the batch whose timer releases it (a command queued while others wait
    /// joins their batch and keeps their deadline).
    pub fn push(&mut self, command: String) -> u64 {
        if self.waiting.is_empty() {
            self.seq += 1;
        }
        self.waiting.push(command);
        self.seq
    }

    /// A prompt mark arrived: the text to type now, if the shell just became ready for it.
    pub fn on_prompt(&mut self, mark: PromptMark) -> Option<String> {
        match mark {
            PromptMark::PromptStart => self.release(),
            _ => None,
        }
    }

    /// Batch `seq`'s wait ended: the text to type now, unless a prompt released it already.
    pub fn on_timeout(&mut self, seq: u64) -> Option<String> {
        if seq == self.seq { self.release() } else { None }
    }

    /// Something is still waiting to be typed (the pane is not idle even if its shell shows).
    pub fn is_pending(&self) -> bool {
        !self.waiting.is_empty()
    }

    fn release(&mut self) -> Option<String> {
        if self.waiting.is_empty() {
            return None;
        }
        Some(self.waiting.drain(..).map(|c| typed(&c)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_runs_in_an_idle_shell_else_in_a_new_tab() {
        assert_eq!(place(Location::Smart, Some(3), |p| p == 3), Placement::InPlace(3));
        assert_eq!(place(Location::Smart, Some(3), |_| false), Placement::NewTab, "busy / unknown / preview pane");
        assert_eq!(place(Location::Smart, None, |_| true), Placement::NewTab);
    }

    #[test]
    fn right_and_below_always_split_even_an_idle_shell() {
        assert_eq!(place(Location::Right, Some(3), |_| true), Placement::Split(Axis::Row));
        assert_eq!(place(Location::Below, Some(3), |_| false), Placement::Split(Axis::Column));
        assert_eq!(place(Location::Right, None, |_| true), Placement::NewTab, "nothing to split");
    }

    #[test]
    fn enter_modifiers_and_preview_words() {
        assert_eq!(Location::from_enter(false, false), Location::Smart);
        assert_eq!(Location::from_enter(false, true), Location::Smart, "⇧↩ alone is not a location");
        assert_eq!(Location::from_enter(true, false), Location::Right);
        assert_eq!(Location::from_enter(true, true), Location::Below);
        let words: Vec<&str> =
            [Placement::InPlace(1), Placement::NewTab, Placement::Split(Axis::Row), Placement::Split(Axis::Column)].map(Placement::word).into();
        assert_eq!(words, ["当前 pane", "新标签", "右侧", "下方"]);
    }

    #[test]
    fn waits_for_the_prompt_only_with_shell_integration() {
        assert_eq!(prompt_wait(true), Duration::from_secs(3));
        assert_eq!(prompt_wait(false), Duration::from_millis(800));
        assert!(prompt_wait(false) <= MAX_WAIT);
    }

    #[test]
    fn the_first_prompt_start_types_the_command_once() {
        let mut q = Queue::default();
        let seq = q.push("cd /r && claude".into());
        assert_eq!(q.on_prompt(PromptMark::CommandEnd(Some(0))), None, "only a prompt start releases it");
        assert_eq!(q.on_prompt(PromptMark::InputStart), None);
        assert_eq!(q.on_prompt(PromptMark::PromptStart).as_deref(), Some("cd /r && claude\r"));
        assert_eq!(q.on_prompt(PromptMark::PromptStart), None, "the next prompt (after the agent exits) types nothing");
        assert_eq!(q.on_timeout(seq), None, "the timer finds it typed already");
    }

    #[test]
    fn the_timeout_types_it_when_no_prompt_comes() {
        let mut q = Queue::default();
        let seq = q.push("codex resume 'a b'".into());
        assert_eq!(q.on_timeout(seq).as_deref(), Some("codex resume 'a b'\r"));
        assert_eq!(q.on_prompt(PromptMark::PromptStart), None);
        assert_eq!(q.on_timeout(seq), None);
    }

    #[test]
    fn stale_timers_release_nothing_and_batches_keep_their_order() {
        let mut q = Queue::default();
        let old = q.push("a".into());
        assert!(q.on_prompt(PromptMark::PromptStart).is_some());
        let new = q.push("b".into());
        assert_ne!(old, new);
        assert_eq!(q.on_timeout(old), None, "the first batch's timer must not type the second early");
        assert_eq!(q.push("c".into()), new, "joins the waiting batch");
        assert_eq!(q.on_timeout(new).as_deref(), Some("b\rc\r"));
    }

    #[test]
    fn pending_until_released() {
        let mut q = Queue::default();
        assert!(!q.is_pending());
        let seq = q.push("a".into());
        assert!(q.is_pending());
        q.on_timeout(seq);
        assert!(!q.is_pending());
    }

    #[test]
    fn control_characters_other_than_line_breaks_are_typed_as_spaces() {
        assert_eq!(typed("claude 'a\tb'"), "claude 'a b'\r", "a Tab would trigger shell completion");
        assert_eq!(typed("x\u{1b}[A\u{3}\u{15}\ry\u{7f}"), "x [A   y \r");
        assert_eq!(typed("claude '一\n二'"), "claude '一\n二'\r", "line breaks stay");
    }

    #[test]
    fn multi_line_commands_are_typed_verbatim() {
        let mut q = Queue::default();
        let seq = q.push("claude '第一行\n第二行'".into());
        assert_eq!(q.on_timeout(seq).as_deref(), Some("claude '第一行\n第二行'\r"));
    }
}
