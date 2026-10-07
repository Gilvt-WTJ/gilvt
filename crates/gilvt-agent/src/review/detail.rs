use std::time::SystemTime;

use super::{ReviewOutcome, TurnCursor};

/// Parser compatibility facts for the records that were read into this page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewCompatibility {
    pub unknown_records: u32,
    pub unknown_blocks: u32,
}

/// Byte range of a transcript record. The UI can use it for a future on-demand expansion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewRecordLocator {
    pub start_offset: u64,
    pub end_offset: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewToolStatus {
    Running,
    Ok,
    Failed { exit: Option<i32> },
    Denied,
    Interrupted,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewToolDetail {
    pub input: Vec<(String, String)>,
    pub output: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewTool {
    pub id: String,
    pub tool: String,
    pub summary: String,
    pub status: ReviewToolStatus,
    pub started_at: Option<SystemTime>,
    pub completed_at: Option<SystemTime>,
    pub error_excerpt: Vec<String>,
    pub lines: Option<(u32, u32)>,
    pub detail: ReviewToolDetail,
    pub output_locator: Option<ReviewRecordLocator>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReviewItem {
    Thinking {
        secs: Option<f32>,
        text: Vec<String>,
    },
    Tool(ReviewTool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewTruncation {
    pub bytes: u64,
    pub limit: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReviewTurn {
    pub cursor: TurnCursor,
    pub ordinal: u32,
    pub prompt: String,
    pub final_reply: String,
    pub started_at: Option<SystemTime>,
    pub completed_at: Option<SystemTime>,
    pub outcome: ReviewOutcome,
    pub tokens: u64,
    pub lines_added: u32,
    pub lines_removed: u32,
    pub items: Vec<ReviewItem>,
    pub truncation: Option<ReviewTruncation>,
}
