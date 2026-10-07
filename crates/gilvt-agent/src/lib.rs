//! What the Claude Code / Codex sessions in gilvt's panes are doing: hook payloads and transcript lines
//! normalized into [`Event`]s by one adapter per agent. The state machine and registry are pure (time is
//! injected); `Tail` and the discovery functions do blocking file IO for a background thread. gilvt-app owns
//! the IPC, the panes and the UI, so this crate must not depend on gpui. [`Timeline`] (M3b) keeps what each
//! session did, turn by turn, for the inspector.
//! [`HistoryIndex`] (M3c) lists the past sessions on this machine for resuming;
//! [`new_agent_command`] / [`resume_command`] build the command lines that start or resume one.

mod claude;
mod close_guard;
mod codex;
mod discover;
mod event;
mod history;
mod git;
mod launch;
mod registry;
mod review;
mod runtime;
mod status;
mod store;
mod summary;
mod tail;
mod title;
#[cfg(test)]
mod title_tests;
mod timeline;
mod usage;

pub use close_guard::{blocks_close, needs_confirm, status_label, SessionSummary};
pub use discover::{
    agent_of_process, claude_project_dir_name, newest_claude_transcript, newest_codex_rollout, process_basename,
    subagent_parent_tool_use, subagent_transcripts,
};
pub use event::{hook_meta, parse_hook, parse_transcript_line, AgentKind, Event, HookInput, HookMeta};
pub use history::{
    companion_files, companion_size, disk_size, is_session_uuid, parse_claude_review, parse_claude_session, parse_codex_review, parse_codex_rollout, path_size, session_files,
    HistoryEntry, HistoryIndex, FIRST_PROMPT_MAX,
};
pub use git::{
    branch_name, create_worktree, display_line, main_repo_of, parse_status_v2, query as git_query, query_outcome, QueryOutcome, short_id, slug, worktree_path, worktree_start_dir, GitInfo,
    StatusSummary,
};
pub use launch::{
    new_agent_command, resume_command, shell_quote, ClaudePermission, CodexPermission, NewAgent, Permission,
};
pub use registry::{Change, Registry};
pub use review::{
    ReviewCompatibility, ReviewDocument, ReviewItem, ReviewOutcome, ReviewPage, ReviewRecordLocator,
    ReviewSessionIndex, ReviewState, ReviewTool, ReviewToolDetail, ReviewToolStatus, ReviewTruncation, ReviewTurn,
    ReviewTurnIndex, TurnCursor, REVIEW_PAGE_BYTES, REVIEW_PAGE_TURNS,
};
pub use runtime::{
    default_state_dir, interrupt_runtime, runtime_diagnostics, scan_agent_processes, terminate_runtime,
    BindingConfidence, Lease, LeaseStore, ProcessObservation, RuntimeRef, SignalError, TerminalOwner,
};
pub use status::{session_name, PaneId, Session, SessionKey, Status, NAME_MAX};
pub use store::Store;
pub use summary::{tool_label, tool_summary, truncate_chars, SUMMARY_MAX};
pub use tail::Tail;
pub use title::{
    choose as choose_title, is_informative, is_reply, session_title, tidy_prompt, Title, TitleSource,
    NO_PROMPT,
};
pub use timeline::{
    error_excerpt, Anchor, Approval, Detail, Item, ItemStatus, PlanItem, PlanState, SubagentInfo, Timeline, ToolItem, Turn,
    TurnOutcome, ANSWER_TO_REJECTION, MAX_ITEMS, MAX_TURNS,
};
