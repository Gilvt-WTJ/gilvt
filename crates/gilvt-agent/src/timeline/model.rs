//! The timeline's public data: turns, rows, statuses.

use std::time::SystemTime;

use crate::status::PaneId;

/// A tool call's state. `Pending` = waiting for the user's approval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemStatus {
    Running,
    Ok,
    Failed { exit: Option<i32> },
    Denied,
    Interrupted,
    Pending,
}

impl ItemStatus {
    /// Still running or waiting for approval.
    pub fn is_open(&self) -> bool {
        matches!(self, ItemStatus::Running | ItemStatus::Pending)
    }
}

/// Whether a call waited for the user's approval.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Approval {
    /// No hook announced the call (transcript only: history, 精简模式): whether it waited is not known.
    #[default]
    Unknown,
    /// A `PreToolUse` hook announced the call and no approval dialog followed (so far).
    NotAsked,
    /// The approval dialog was requested at this time (`PermissionRequest`); no answer was seen in the pane.
    Asked(SystemTime),
    /// The user answered in the pane at this time. Yes or no is not known then: a no shows up later as the
    /// transcript's rejection, right after the answer.
    Answered(SystemTime),
}

/// Where a tool call started in its pane: an absolute terminal line (lines scrolled off since the pane
/// started + cursor row), captured when the `PreToolUse` hook arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Anchor {
    pub pane: PaneId,
    pub line: u64,
}

/// The expandable part of a row: arguments as key / value pairs (sorted by key, long values cut) and the
/// first lines of the output (≤ 20 lines, ≤ 4 KB).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Detail {
    pub input: Vec<(String, String)>,
    pub output: Vec<String>,
}

/// The subagent a Claude Task / Agent or Codex `spawn_agent` call started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentInfo {
    pub agent_id: String,
    pub agent_type: Option<String>,
    /// First sentence of the subagent's final reply (the 「↩ …」 line).
    pub result: Option<String>,
    pub done: bool,
}

/// One tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolItem {
    /// Tool-use id (Claude `tool_use.id`, Codex `call_id`).
    pub id: String,
    pub tool: String,
    /// M3a's tool summary (`go test ./...`, `lib.rs`).
    pub summary: String,
    pub status: ItemStatus,
    pub started: Option<SystemTime>,
    pub ended: Option<SystemTime>,
    /// Key error lines of a failed call (see [`error_excerpt`]); empty otherwise.
    pub error_excerpt: Vec<String>,
    /// `(+added, −removed)` lines of an edit (Edit / Write / MultiEdit / NotebookEdit / apply_patch).
    pub lines: Option<(u32, u32)>,
    pub detail: Detail,
    /// None for calls no `PreToolUse` hook announced (transcript-only, subagent-internal, lite mode).
    pub anchor: Option<Anchor>,
    pub approval: Approval,
    /// Only for subagents: the subagent's own items.
    pub children: Vec<Item>,
    pub subagent: Option<SubagentInfo>,
}

impl ToolItem {
    pub fn new(id: &str, tool: &str) -> ToolItem {
        ToolItem {
            id: id.to_string(),
            tool: tool.to_string(),
            summary: String::new(),
            status: ItemStatus::Running,
            started: None,
            ended: None,
            error_excerpt: Vec::new(),
            lines: None,
            detail: Detail::default(),
            anchor: None,
            approval: Approval::Unknown,
            children: Vec::new(),
            subagent: None,
        }
    }
}

/// A timeline row.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Tool(ToolItem),
    /// A thinking block: seconds since the previous record (when the records carry timestamps), first lines.
    Thinking {
        secs: Option<f32>,
        text: Vec<String>,
    },
    /// 「另有 N 条」: older items of an overlong turn folded away.
    Truncated {
        hidden: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnOutcome {
    Running,
    Done,
    Interrupted,
    Failed { message: String },
}

/// One user prompt and what the agent did for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    /// 1-based position in the session (第 N 轮).
    pub index: u32,
    /// The prompt as typed; empty for activity seen before any prompt.
    pub prompt: String,
    pub started: Option<SystemTime>,
    pub ended: Option<SystemTime>,
    pub outcome: TurnOutcome,
    pub items: Vec<Item>,
    /// Claude: input + output + cache tokens of the turn's replies (deduplicated by message id);
    /// Codex: growth of `token_count` totals.
    pub tokens: u64,
    /// Tool calls in the turn (「K 步」); kept when older turns drop their rows (see [`crate::MAX_TURNS`]).
    pub steps: usize,
    /// First sentence of the latest assistant text of the turn (the artifacts card's grey quote); empty
    /// before the agent said anything.
    pub reply: String,
}

impl Turn {
    pub(crate) fn new(prompt: &str, started: Option<SystemTime>) -> Turn {
        Turn {
            index: 0,
            prompt: prompt.to_string(),
            started,
            ended: None,
            outcome: TurnOutcome::Running,
            items: Vec::new(),
            tokens: 0,
            steps: 0,
            reply: String::new(),
        }
    }
}
impl Turn {
    /// A running turn with `prompt`, for other crates' tests.
    #[doc(hidden)]
    pub fn new_for_test(prompt: &str) -> Turn {
        Turn::new(prompt, None)
    }
}
