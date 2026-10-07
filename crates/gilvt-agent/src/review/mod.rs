//! Stable identities and persisted user state for turn-by-turn session review.
//!
//! The live [`crate::Timeline`] uses positional turn indexes because it can merge hooks and transcript
//! events. Review cursors must survive process restarts, so they use identities written by the agent and a
//! byte-offset fallback for older or unknown transcript formats.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

mod detail;
mod document;
mod scan;

pub use detail::{
    ReviewCompatibility, ReviewItem, ReviewRecordLocator, ReviewTool, ReviewToolDetail,
    ReviewToolStatus, ReviewTruncation, ReviewTurn,
};
pub use document::{ReviewDocument, ReviewPage, REVIEW_PAGE_BYTES, REVIEW_PAGE_TURNS};
pub(crate) use scan::ReviewScanner;

/// A completed transcript turn that can be used as a durable review cursor.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TurnCursor {
    /// UUID of the main-thread user prompt record.
    Claude { prompt_uuid: String },
    /// Codex's per-turn identifier, carried by response items and task completion records.
    Codex { turn_id: String },
    /// Compatibility fallback when an agent format does not expose a stable turn identifier.
    Fallback {
        end_offset: u64,
        fingerprint: String,
    },
}

/// Terminal state of a reviewable turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewOutcome {
    Done,
    Interrupted,
    Failed { message: String },
}

/// User-owned review state for one session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewState {
    /// Distinguishes a never-seen session from an explicit "review from the beginning" reset.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub baseline_reconciled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewed_through: Option<TurnCursor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewed_at: Option<SystemTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<SystemTime>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// When the user archived the session (hidden from the lists, files untouched).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<SystemTime>,
    /// The session's real-turn count when it was archived; a larger count later un-archives it.
    #[serde(skip_serializing_if = "is_zero")]
    pub archived_turns: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl ReviewState {
    /// Archived and no turn was added since (`turns` = the session's current real-turn count).
    pub fn archived_for(&self, turns: u32) -> bool {
        self.archived_at.is_some() && turns <= self.archived_turns
    }
}

/// The bounded, cacheable facts needed to build the Review Inbox and locate a turn on disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewTurnIndex {
    pub cursor: TurnCursor,
    /// 1-based position among real user turns in this session.
    pub ordinal: u32,
    pub start_offset: u64,
    pub end_offset: u64,
    /// FNV-1a of every complete JSONL line in this turn, including trailing newlines.
    #[serde(default)]
    pub fingerprint: String,
    pub prompt_preview: String,
    pub reply_preview: String,
    pub started_at: Option<SystemTime>,
    pub completed_at: Option<SystemTime>,
    pub outcome: ReviewOutcome,
    pub tool_count: u32,
    pub failed_tool_count: u32,
    pub lines_added: u32,
    pub lines_removed: u32,
    pub tokens: u64,
}

pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    fnv1a64_extend(0xcbf29ce484222325, bytes)
}

pub(crate) fn fnv1a64_extend(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Review projection of one interactive session. Full conversation text is loaded lazily from the offsets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSessionIndex {
    pub key: crate::SessionKey,
    pub transcript: std::path::PathBuf,
    pub transcript_size: u64,
    /// End of the last complete line that was scanned.
    pub scanned_through: u64,
    pub incomplete_tail: bool,
    pub turns: Vec<ReviewTurnIndex>,
}

impl ReviewSessionIndex {
    pub fn last_completed(&self) -> Option<&ReviewTurnIndex> {
        self.turns.last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_json_is_tagged_and_stable() {
        let claude = TurnCursor::Claude {
            prompt_uuid: "prompt-1".into(),
        };
        assert_eq!(
            serde_json::to_string(&claude).unwrap(),
            r#"{"kind":"claude","prompt_uuid":"prompt-1"}"#
        );
        let fallback = TurnCursor::Fallback {
            end_offset: 42,
            fingerprint: "fnv1a64:abcd".into(),
        };
        assert_eq!(
            serde_json::from_str::<TurnCursor>(&serde_json::to_string(&fallback).unwrap()).unwrap(),
            fallback
        );
    }

    #[test]
    fn old_review_state_can_omit_new_fields() {
        let state: ReviewState = serde_json::from_str(r#"{"pinned":true}"#).unwrap();
        assert_eq!(
            state,
            ReviewState {
                pinned: true,
                ..ReviewState::default()
            }
        );
    }

    #[test]
    fn archived_until_a_new_turn_arrives() {
        let state = ReviewState {
            archived_at: Some(SystemTime::UNIX_EPOCH),
            archived_turns: 3,
            ..ReviewState::default()
        };
        assert!(state.archived_for(3), "same turn count stays archived");
        assert!(
            state.archived_for(2),
            "a shrunk count (rewritten file) stays archived"
        );
        assert!(!state.archived_for(4), "a new turn un-archives");
        assert!(!ReviewState::default().archived_for(0), "never archived");
    }

    #[test]
    fn old_review_state_without_archive_fields_loads_and_stays_empty() {
        let state: ReviewState = serde_json::from_str(r#"{"pinned":true}"#).unwrap();
        assert_eq!(state.archived_at, None);
        assert_eq!(state.archived_turns, 0);
        assert_eq!(
            serde_json::to_string(&ReviewState::default()).unwrap(),
            "{}",
            "no archive fields are written for an untouched session"
        );
    }
}
