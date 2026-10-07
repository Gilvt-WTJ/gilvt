//! A session's status and the transition function of spec §3.1.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::event::{AgentKind, Event};
use crate::summary::{first_line, tool_label, truncate_chars};

/// Longest automatic session name, in chars (an ellipsis is added when cut).
pub const NAME_MAX: usize = 40;

/// Assumed Claude context window when the transcript does not say (it never does).
const CLAUDE_WINDOW: u64 = 200_000;
/// Assumed instead once a Claude session is past [`CLAUDE_WINDOW`] (a 1M-context model).
const CLAUDE_LARGE_WINDOW: u64 = 1_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// 空闲（等你输入）
    Idle,
    /// 思考中
    Thinking,
    /// 执行 `Bash(go test ./...)`
    Tool {
        label: String,
    },
    NeedsApproval {
        action: String,
    },
    Asking {
        question: String,
    },
    Error {
        message: String,
    },
    Ended,
}

impl Status {
    /// Waiting for the user: an approval or a question.
    pub fn needs_you(&self) -> bool {
        matches!(self, Status::NeedsApproval { .. } | Status::Asking { .. })
    }

    /// Working on a turn (thinking or running a tool).
    pub fn is_running(&self) -> bool {
        matches!(self, Status::Thinking | Status::Tool { .. })
    }
}

pub type PaneId = u64;
/// (agent, session_id)
pub type SessionKey = (AgentKind, String);

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub key: SessionKey,
    pub pane: Option<PaneId>,
    pub cwd: Option<PathBuf>,
    pub transcript: Option<PathBuf>,
    pub status: Status,
    /// The first prompt's first line (≤ [`NAME_MAX`] chars), or the user's rename. Empty until then.
    pub name: String,
    pub renamed: bool,
    pub muted: bool,
    /// 精简模式: no hooks seen, status inferred from the transcript alone.
    pub lite: bool,
    /// > 0 → 「后台任务运行中」
    pub background_tasks: usize,
    /// 完成未看: a turn ended while the pane was not visible.
    pub unseen_done: bool,
    /// When the session entered NeedsApproval / Asking.
    pub waiting_since: Option<Instant>,
    /// Approval requests (named ones: `PermissionRequest`) not answered yet: parallel subagents can queue
    /// several dialogs, and answering one leaves the session waiting for the next. 0 outside NeedsApproval.
    pub pending_approvals: usize,
    /// When the current turn's prompt was submitted.
    pub turn_started: Option<Instant>,
    /// Duration of the turn that just ended.
    pub last_turn: Option<Duration>,
    /// (tokens, window) of the latest reply; see [`Session::context_ratio`].
    pub context: Option<(u64, Option<u64>)>,
    pub model: Option<String>,
    /// Last time anything about the session changed.
    pub updated: Instant,
    pub(crate) auto_name: String,
    pub(crate) last_message_id: Option<String>,
}

impl Session {
    /// A fresh idle session.
    pub fn new(key: SessionKey, now: Instant) -> Session {
        Session {
            key,
            pane: None,
            cwd: None,
            transcript: None,
            status: Status::Idle,
            name: String::new(),
            renamed: false,
            muted: false,
            lite: false,
            background_tasks: 0,
            unseen_done: false,
            waiting_since: None,
            pending_approvals: 0,
            turn_started: None,
            last_turn: None,
            context: None,
            model: None,
            updated: now,
            auto_name: String::new(),
            last_message_id: None,
        }
    }

    pub fn agent(&self) -> AgentKind {
        self.key.0
    }

    pub fn session_id(&self) -> &str {
        &self.key.1
    }

    pub fn needs_you(&self) -> bool {
        self.status.needs_you()
    }

    pub fn is_live(&self) -> bool {
        self.status != Status::Ended
    }

    /// Context occupancy in 0..=1 (can exceed 1 on bad data). Claude transcripts carry no window:
    /// 200k is assumed, or 1M once the session is past 200k.
    pub fn context_ratio(&self) -> Option<f32> {
        let (tokens, window) = self.context?;
        let window = window.or_else(|| {
            (self.agent() == AgentKind::Claude).then_some(if tokens > CLAUDE_WINDOW {
                CLAUDE_LARGE_WINDOW
            } else {
                CLAUDE_WINDOW
            })
        })?;
        (window > 0).then(|| tokens as f32 / window as f32)
    }

    /// Sets (or, with an empty name, clears) the user's rename.
    pub(crate) fn set_rename(&mut self, name: Option<String>) {
        match name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
            Some(name) => {
                self.name = name;
                self.renamed = true;
            }
            None => {
                self.name = self.auto_name.clone();
                self.renamed = false;
            }
        }
    }
}

/// The session name a prompt gives: its first non-blank line, trimmed, at most [`NAME_MAX`] chars.
pub fn session_name(prompt: &str) -> String {
    truncate_chars(first_line(prompt), NAME_MAX)
}

/// Applies one event (spec §3.1). Returns whether anything changed; `updated` is bumped when so.
pub(crate) fn step(s: &mut Session, event: &Event, pane_visible: bool, now: Instant) -> bool {
    let before = s.clone();
    apply(s, event, pane_visible, now);
    let changed = *s != before;
    if changed {
        s.updated = now;
    }
    changed
}

fn apply(s: &mut Session, event: &Event, pane_visible: bool, now: Instant) {
    if s.status == Status::Ended && !matches!(event, Event::SessionStart { .. }) {
        return;
    }
    transition(s, event, pane_visible, now);
    if !matches!(s.status, Status::NeedsApproval { .. }) {
        s.pending_approvals = 0;
    }
}

fn transition(s: &mut Session, event: &Event, pane_visible: bool, now: Instant) {
    match event {
        Event::SessionStart { model } => {
            // A mid-turn SessionStart (after compaction) keeps the turn going.
            if !s.status.is_running() {
                s.status = Status::Idle;
                s.waiting_since = None;
            }
            if model.is_some() {
                s.model.clone_from(model);
            }
        }
        Event::SessionEnd => {
            s.status = Status::Ended;
            s.waiting_since = None;
            s.turn_started = None;
        }
        Event::PromptSubmit { text } => {
            if s.auto_name.is_empty() {
                s.auto_name = session_name(text);
                if !s.renamed {
                    s.name = s.auto_name.clone();
                }
            }
            s.status = Status::Thinking;
            s.turn_started = Some(now);
            s.waiting_since = None;
            s.unseen_done = false;
        }
        Event::ToolStart { tool, summary } => {
            if !s.status.needs_you() {
                s.status = Status::Tool { label: tool_label(tool, summary) };
                s.turn_started.get_or_insert(now);
            }
        }
        Event::ToolEnd { .. } => {
            if s.status.needs_you() || matches!(s.status, Status::Tool { .. }) {
                resume(s);
            }
        }
        // AskUserQuestion is followed by a permission prompt of its own: the question stays.
        Event::PermissionNeeded { .. } if matches!(s.status, Status::Asking { .. }) => {}
        // Claude's permission_prompt notification after its PermissionRequest: already waiting.
        Event::PermissionNeeded { action } if action.is_empty() && s.status.needs_you() => {}
        Event::PermissionNeeded { action } => {
            let action = match &s.status {
                _ if !action.is_empty() => action.clone(),
                Status::Tool { label } => label.clone(),
                _ => String::new(),
            };
            s.status = Status::NeedsApproval { action };
            s.waiting_since.get_or_insert(now);
            s.pending_approvals += 1;
        }
        Event::PermissionDenied => {
            if s.status.needs_you() {
                resume(s);
            }
        }
        // Approved until the transcript says otherwise: the call runs (a no ends the turn right after).
        // Another dialog may be queued behind it (see `pending_approvals`).
        Event::ApprovalAnswered => {
            s.pending_approvals = s.pending_approvals.saturating_sub(1);
            if let Status::NeedsApproval { action } = &s.status {
                if s.pending_approvals == 0 {
                    s.status = if action.is_empty() { Status::Thinking } else { Status::Tool { label: action.clone() } };
                    s.waiting_since = None;
                }
            }
        }
        Event::Question { text } => {
            s.status = Status::Asking { question: text.clone() };
            s.waiting_since.get_or_insert(now);
        }
        Event::TurnEnd { background_tasks } => turn_end(s, *background_tasks, pane_visible, now),
        Event::Interrupted => {
            // The user is at the pane: not 完成未看, and no turn duration to report.
            if s.status != Status::Idle || s.turn_started.is_some() {
                s.status = Status::Idle;
                s.turn_started = None;
                s.last_turn = None;
                s.waiting_since = None;
            }
        }
        Event::Error { message } => {
            s.status = Status::Error { message: message.clone() };
            s.waiting_since = None;
            s.turn_started = None;
            s.unseen_done = false;
        }
        Event::Usage { context_tokens, context_window, message_id } => {
            crate::usage::apply(s, *context_tokens, *context_window, message_id.as_deref());
        }
        Event::Model { name } => {
            if s.model.as_deref() != Some(name) {
                s.model = Some(name.clone());
            }
        }
        Event::SubagentStart => {}
        Event::SubagentStop => s.background_tasks = s.background_tasks.saturating_sub(1),
    }
}

/// Back to thinking after a tool, an approval or a question.
fn resume(s: &mut Session) {
    s.status = Status::Thinking;
    s.waiting_since = None;
}

fn turn_end(s: &mut Session, background_tasks: usize, pane_visible: bool, now: Instant) {
    s.background_tasks = background_tasks;
    s.waiting_since = None;
    if background_tasks > 0 {
        // Still working: the background tasks will wake the session up.
        if !s.status.is_running() {
            s.status = Status::Thinking;
        }
        return;
    }
    let in_turn = s.status != Status::Idle || s.turn_started.is_some();
    if matches!(s.status, Status::Error { .. }) || !in_turn {
        // A duplicate (a reply split over several records) or the Stop after an error.
        return;
    }
    s.status = Status::Idle;
    s.last_turn = s.turn_started.take().map(|t| now.saturating_duration_since(t));
    s.unseen_done = !pane_visible;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_names() {
        assert_eq!(session_name("fix the login bug"), "fix the login bug");
        assert_eq!(session_name("\n\n   trim me   \nsecond line"), "trim me");
        let forty = "a".repeat(40);
        assert_eq!(session_name(&forty), forty);
        assert_eq!(session_name(&format!("{forty}b")), format!("{forty}…"));
        let chinese = "修".repeat(41);
        assert_eq!(session_name(&chinese), format!("{}…", "修".repeat(40)));
        assert_eq!(session_name("   \n  "), "");
    }

    #[test]
    fn status_kinds() {
        assert!(Status::NeedsApproval { action: "x".into() }.needs_you());
        assert!(Status::Asking { question: "q".into() }.needs_you());
        assert!(!Status::Error { message: "e".into() }.needs_you());
        assert!(Status::Thinking.is_running() && Status::Tool { label: "t".into() }.is_running());
        assert!(!Status::Idle.is_running() && !Status::Ended.is_running());
    }

    #[test]
    fn duplicate_turn_ends_are_ignored() {
        let now = Instant::now();
        let mut s = Session::new((AgentKind::Claude, "s".into()), now);
        assert!(step(&mut s, &Event::PromptSubmit { text: "x".into() }, true, now));
        let later = now + Duration::from_secs(5);
        assert!(step(&mut s, &Event::TurnEnd { background_tasks: 0 }, false, later));
        assert_eq!((s.last_turn, s.unseen_done, s.updated), (Some(Duration::from_secs(5)), true, later));
        assert!(!step(&mut s, &Event::TurnEnd { background_tasks: 0 }, true, later + Duration::from_secs(1)));
        assert!(s.unseen_done);
    }
}
