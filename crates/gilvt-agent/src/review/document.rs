use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::time::SystemTime;

use serde_json::{json, Value};

use super::{
    fnv1a64, ReviewCompatibility, ReviewItem, ReviewRecordLocator, ReviewSessionIndex, ReviewTool,
    ReviewToolDetail, ReviewToolStatus, ReviewTruncation, ReviewTurn, ReviewTurnIndex, TurnCursor,
};
use crate::event::{str_field, AgentKind, Event};
use crate::timeline::text::{
    content_text, detail_input, detail_output, edit_lines, exit_code, first_lines, parse_timestamp,
};
use crate::{error_excerpt, tool_summary, SessionKey};

pub const REVIEW_PAGE_TURNS: usize = 20;
pub const REVIEW_PAGE_BYTES: usize = 8 * 1024 * 1024;

/// A cursor-stable page request. `After { cursor: None }` is the first page of a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewPage {
    After { cursor: Option<TurnCursor> },
    Before { cursor: TurnCursor },
    Latest,
}

/// A read-only page reconstructed from one immutable review-index snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewDocument {
    pub key: SessionKey,
    /// The last completed turn in the index, not merely the last turn on this page.
    pub snapshot_through: TurnCursor,
    pub snapshot_size: u64,
    pub turns: Vec<ReviewTurn>,
    pub has_earlier: bool,
    pub has_later: bool,
    pub compatibility: ReviewCompatibility,
    pub bytes_read: usize,
}

impl ReviewDocument {
    /// Loads one page without reading beyond `index.scanned_through`. Appends after the index was captured
    /// are deliberately invisible; truncation or a rewritten selected turn returns an error.
    pub fn load(index: &ReviewSessionIndex, page: ReviewPage) -> io::Result<Self> {
        let snapshot_through = index
            .last_completed()
            .map(|turn| turn.cursor.clone())
            .ok_or_else(|| invalid("review session has no completed turns"))?;
        let (start, end) = page_range(&index.turns, &page)?;
        let metadata = fs::metadata(&index.transcript)?;
        if metadata.len() < index.scanned_through {
            return Err(invalid(
                "transcript was truncated after the review index was built",
            ));
        }

        let mut file = fs::File::open(&index.transcript)?;
        let mut turns = Vec::with_capacity(end - start);
        let mut compatibility = ReviewCompatibility::default();
        let mut bytes_read = 0usize;
        for indexed in &index.turns[start..end] {
            let span = checked_span(indexed, index.scanned_through)?;
            if span > REVIEW_PAGE_BYTES as u64 {
                turns.push(truncated_turn(indexed, span));
                continue;
            }
            let mut bytes = vec![0; span as usize];
            file.seek(SeekFrom::Start(indexed.start_offset))?;
            file.read_exact(&mut bytes)?;
            bytes_read += bytes.len();
            if bytes.last() != Some(&b'\n') {
                return Err(invalid(
                    "indexed turn does not end at a complete JSONL line",
                ));
            }
            let fingerprint = format!("fnv1a64:{:016x}", fnv1a64(&bytes));
            if !indexed.fingerprint.is_empty() && fingerprint != indexed.fingerprint {
                return Err(invalid(
                    "transcript changed after the review index was built",
                ));
            }
            let (turn, found_cursor) =
                parse_turn(index.key.0, indexed, &bytes, &mut compatibility)?;
            if found_cursor.as_ref() != Some(&indexed.cursor) {
                return Err(invalid(
                    "transcript changed after the review index was built",
                ));
            }
            turns.push(turn);
        }

        Ok(ReviewDocument {
            key: index.key.clone(),
            snapshot_through,
            snapshot_size: index.scanned_through,
            turns,
            has_earlier: start > 0,
            has_later: end < index.turns.len(),
            compatibility,
            bytes_read,
        })
    }
}

fn page_range(turns: &[ReviewTurnIndex], page: &ReviewPage) -> io::Result<(usize, usize)> {
    let anchor = |cursor: &TurnCursor| {
        turns
            .iter()
            .position(|turn| &turn.cursor == cursor)
            .ok_or_else(|| invalid("review page cursor is stale"))
    };
    match page {
        ReviewPage::After { cursor } => {
            let start = cursor
                .as_ref()
                .map(&anchor)
                .transpose()?
                .map_or(0, |at| at + 1);
            Ok(forward_range(turns, start))
        }
        ReviewPage::Before { cursor } => Ok(backward_range(turns, anchor(cursor)?)),
        ReviewPage::Latest => Ok(backward_range(turns, turns.len())),
    }
}

fn forward_range(turns: &[ReviewTurnIndex], start: usize) -> (usize, usize) {
    let mut end = start;
    let mut bytes = 0u64;
    while end < turns.len() && end - start < REVIEW_PAGE_TURNS {
        let span = turns[end]
            .end_offset
            .saturating_sub(turns[end].start_offset);
        if end > start && bytes.saturating_add(span) > REVIEW_PAGE_BYTES as u64 {
            break;
        }
        bytes = bytes.saturating_add(span.min(REVIEW_PAGE_BYTES as u64));
        end += 1;
        if span > REVIEW_PAGE_BYTES as u64 {
            break;
        }
    }
    (start, end)
}

fn backward_range(turns: &[ReviewTurnIndex], end: usize) -> (usize, usize) {
    let mut start = end;
    let mut bytes = 0u64;
    while start > 0 && end - start < REVIEW_PAGE_TURNS {
        let candidate = start - 1;
        let span = turns[candidate]
            .end_offset
            .saturating_sub(turns[candidate].start_offset);
        if start < end && bytes.saturating_add(span) > REVIEW_PAGE_BYTES as u64 {
            break;
        }
        bytes = bytes.saturating_add(span.min(REVIEW_PAGE_BYTES as u64));
        start = candidate;
        if span > REVIEW_PAGE_BYTES as u64 {
            break;
        }
    }
    (start, end)
}

fn checked_span(turn: &ReviewTurnIndex, snapshot: u64) -> io::Result<u64> {
    if turn.start_offset >= turn.end_offset || turn.end_offset > snapshot {
        return Err(invalid(
            "review turn offsets are outside the indexed snapshot",
        ));
    }
    Ok(turn.end_offset - turn.start_offset)
}

fn truncated_turn(index: &ReviewTurnIndex, bytes: u64) -> ReviewTurn {
    ReviewTurn {
        cursor: index.cursor.clone(),
        ordinal: index.ordinal,
        prompt: index.prompt_preview.clone(),
        final_reply: index.reply_preview.clone(),
        started_at: index.started_at,
        completed_at: index.completed_at,
        outcome: index.outcome.clone(),
        tokens: index.tokens,
        lines_added: index.lines_added,
        lines_removed: index.lines_removed,
        items: Vec::new(),
        truncation: Some(ReviewTruncation {
            bytes,
            limit: REVIEW_PAGE_BYTES as u64,
        }),
    }
}

fn parse_turn(
    agent: AgentKind,
    index: &ReviewTurnIndex,
    bytes: &[u8],
    compatibility: &mut ReviewCompatibility,
) -> io::Result<(ReviewTurn, Option<TurnCursor>)> {
    let mut builder = TurnBuilder::new(index);
    let mut relative = 0usize;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.last() != Some(&b'\n') {
            return Err(invalid("review turn contains an incomplete JSONL line"));
        }
        let start = index.start_offset + relative as u64;
        relative += line.len();
        let end = index.start_offset + relative as u64;
        let record = match serde_json::from_slice::<Value>(line) {
            Ok(record) => record,
            Err(_) => {
                compatibility.unknown_records += 1;
                continue;
            }
        };
        match agent {
            AgentKind::Claude => builder.claude(&record, line, start, end, compatibility),
            AgentKind::Codex => builder.codex(&record, line, start, end, compatibility),
        }
    }
    Ok(builder.finish())
}

struct TurnBuilder {
    index: ReviewTurnIndex,
    cursor: Option<TurnCursor>,
    prompt: String,
    final_reply: String,
    reply_message: Option<String>,
    reply_blocks: HashSet<String>,
    item_blocks: HashSet<String>,
    items: Vec<ReviewItem>,
    tools: HashMap<String, usize>,
    last_timestamp: Option<SystemTime>,
}

impl TurnBuilder {
    fn new(index: &ReviewTurnIndex) -> Self {
        TurnBuilder {
            index: index.clone(),
            cursor: None,
            prompt: String::new(),
            final_reply: String::new(),
            reply_message: None,
            reply_blocks: HashSet::new(),
            item_blocks: HashSet::new(),
            items: Vec::new(),
            tools: HashMap::new(),
            last_timestamp: None,
        }
    }

    fn finish(self) -> (ReviewTurn, Option<TurnCursor>) {
        (
            ReviewTurn {
                cursor: self.index.cursor,
                ordinal: self.index.ordinal,
                prompt: self.prompt,
                final_reply: self.final_reply,
                started_at: self.index.started_at,
                completed_at: self.index.completed_at,
                outcome: self.index.outcome,
                tokens: self.index.tokens,
                lines_added: self.index.lines_added,
                lines_removed: self.index.lines_removed,
                items: self.items,
                truncation: None,
            },
            self.cursor,
        )
    }

    fn claude(
        &mut self,
        record: &Value,
        raw: &[u8],
        start: u64,
        end: u64,
        compatibility: &mut ReviewCompatibility,
    ) {
        if record.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        let timestamp = timestamp(record);
        if let Some(prompt) = crate::claude::parse_record(record)
            .iter()
            .find_map(|event| match event {
                Event::PromptSubmit { text } => Some(text.as_str()),
                _ => None,
            })
        {
            self.prompt = prompt.to_string();
            self.cursor = Some(
                str_field(record, "uuid")
                    .filter(|id| !id.is_empty())
                    .map(|prompt_uuid| TurnCursor::Claude {
                        prompt_uuid: prompt_uuid.to_string(),
                    })
                    .unwrap_or_else(|| fallback(raw, end)),
            );
        }
        match str_field(record, "type") {
            Some("assistant") => self.claude_assistant(record, start, timestamp, compatibility),
            Some("user") => self.claude_user(record, start, end, timestamp, compatibility),
            Some(
                "system" | "progress" | "file-history-snapshot" | "queue-operation" | "last-prompt",
            ) => {}
            Some(_) | None => compatibility.unknown_records += 1,
        }
        self.last_timestamp = timestamp.or(self.last_timestamp);
    }

    fn claude_assistant(
        &mut self,
        record: &Value,
        start: u64,
        timestamp: Option<SystemTime>,
        compatibility: &mut ReviewCompatibility,
    ) {
        let Some(message) = record.get("message") else {
            compatibility.unknown_records += 1;
            return;
        };
        let Some(blocks) = message.get("content").and_then(Value::as_array) else {
            compatibility.unknown_records += 1;
            return;
        };
        for (position, block) in blocks.iter().enumerate() {
            let block_key = format!(
                "{}:{}:{}",
                str_field(message, "id").unwrap_or(""),
                record
                    .get("apiBlockIndex")
                    .and_then(Value::as_u64)
                    .unwrap_or(start),
                position
            );
            match str_field(block, "type") {
                Some("text") => {
                    let message_key = str_field(message, "id")
                        .filter(|id| !id.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("record:{start}"));
                    self.reply_text(
                        &message_key,
                        &block_key,
                        str_field(block, "text").unwrap_or(""),
                    );
                }
                Some("thinking") => {
                    if self.item_blocks.insert(block_key) {
                        let secs = seconds_since(self.last_timestamp, timestamp);
                        self.items.push(ReviewItem::Thinking {
                            secs,
                            text: first_lines(str_field(block, "thinking").unwrap_or("")),
                        });
                    }
                }
                Some("tool_use") => self.add_tool(block, start, timestamp),
                Some(_) | None => compatibility.unknown_blocks += 1,
            }
        }
    }

    fn claude_user(
        &mut self,
        record: &Value,
        start: u64,
        end: u64,
        timestamp: Option<SystemTime>,
        compatibility: &mut ReviewCompatibility,
    ) {
        let Some(blocks) = record.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        for block in blocks {
            match str_field(block, "type") {
                Some("text") => {}
                Some("tool_result") => {
                    let id = str_field(block, "tool_use_id").unwrap_or("");
                    let output = content_text(block.get("content").unwrap_or(&Value::Null));
                    let status = if record.get("toolDenialKind").is_some() {
                        ReviewToolStatus::Denied
                    } else if record
                        .pointer("/toolUseResult/interrupted")
                        .and_then(Value::as_bool)
                        == Some(true)
                    {
                        ReviewToolStatus::Interrupted
                    } else if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                        ReviewToolStatus::Failed {
                            exit: exit_code(&output),
                        }
                    } else {
                        ReviewToolStatus::Ok
                    };
                    if !self.complete_tool(id, status, &output, start, end, timestamp) {
                        compatibility.unknown_blocks += 1;
                    }
                }
                Some(_) | None => compatibility.unknown_blocks += 1,
            }
        }
    }

    fn codex(
        &mut self,
        record: &Value,
        raw: &[u8],
        start: u64,
        end: u64,
        compatibility: &mut ReviewCompatibility,
    ) {
        let timestamp = timestamp(record);
        let payload = record.get("payload").unwrap_or(&Value::Null);
        match (str_field(record, "type"), str_field(payload, "type")) {
            (Some("event_msg"), Some("user_message")) => {
                self.prompt = str_field(payload, "message").unwrap_or("").to_string();
                self.cursor = Some(
                    codex_turn_id(payload)
                        .map(|turn_id| TurnCursor::Codex {
                            turn_id: turn_id.to_string(),
                        })
                        .unwrap_or_else(|| match &self.index.cursor {
                            TurnCursor::Codex { .. } => self.index.cursor.clone(),
                            TurnCursor::Fallback { .. } | TurnCursor::Claude { .. } => {
                                fallback(raw, end)
                            }
                        }),
                );
            }
            (Some("response_item"), Some("message"))
                if str_field(payload, "role") == Some("assistant") =>
            {
                if let Some(turn_id) = codex_turn_id(payload) {
                    self.adopt_codex_cursor(turn_id);
                }
                let text = content_text(payload.get("content").unwrap_or(&Value::Null));
                let message_key = str_field(payload, "id")
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("record:{start}"));
                self.reply_text(&message_key, &format!("{message_key}:{start}"), &text);
            }
            (Some("response_item"), Some("reasoning")) => {
                let summary = payload
                    .get("summary")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                for (position, block) in summary.iter().enumerate() {
                    match str_field(block, "type") {
                        Some("summary_text") | None if str_field(block, "text").is_some() => {
                            let key = format!("reasoning:{start}:{position}");
                            if self.item_blocks.insert(key) {
                                self.items.push(ReviewItem::Thinking {
                                    secs: seconds_since(self.last_timestamp, timestamp),
                                    text: first_lines(str_field(block, "text").unwrap_or("")),
                                });
                            }
                        }
                        _ => compatibility.unknown_blocks += 1,
                    }
                }
            }
            (Some("response_item"), Some("function_call" | "custom_tool_call")) => {
                if let Some(turn_id) = codex_turn_id(payload) {
                    self.adopt_codex_cursor(turn_id);
                }
                let input = codex_tool_input(payload);
                self.add_tool_value(
                    str_field(payload, "call_id").unwrap_or(""),
                    str_field(payload, "name").unwrap_or(""),
                    &input,
                    start,
                    timestamp,
                );
            }
            (Some("response_item"), Some("function_call_output" | "custom_tool_call_output")) => {
                let output = content_text(payload.get("output").unwrap_or(&Value::Null));
                let (status, body) = codex_output(&output);
                if !self.complete_tool(
                    str_field(payload, "call_id").unwrap_or(""),
                    status,
                    &body,
                    start,
                    end,
                    timestamp,
                ) {
                    compatibility.unknown_blocks += 1;
                }
            }
            (Some("event_msg"), Some("task_complete")) => {
                if let Some(turn_id) = str_field(payload, "turn_id").filter(|id| !id.is_empty()) {
                    self.adopt_codex_cursor(turn_id);
                }
                if let Some(reply) = str_field(payload, "last_agent_message") {
                    self.final_reply = reply.to_string();
                }
            }
            (Some("event_msg"), Some("patch_apply_end"))
                if payload.get("success").and_then(Value::as_bool) == Some(false) =>
            {
                let id = str_field(payload, "call_id").unwrap_or("patch");
                let output = [str_field(payload, "stderr"), str_field(payload, "stdout")]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("\n");
                self.complete_tool(
                    id,
                    ReviewToolStatus::Failed { exit: None },
                    &output,
                    start,
                    end,
                    timestamp,
                );
            }
            (
                Some("event_msg"),
                Some(
                    "task_started" | "token_count" | "agent_message" | "turn_aborted" | "error"
                    | "agent_reasoning" | "item_started" | "item_completed" | "context_compacted",
                ),
            ) => {}
            (
                Some(
                    "session_meta" | "turn_context" | "world_state" | "ghost_snapshot"
                    | "compacted",
                ),
                _,
            ) => {}
            (Some("response_item"), Some("message")) => {}
            (Some(_), Some(_)) | (Some(_), None) | (None, _) => compatibility.unknown_records += 1,
        }
        self.last_timestamp = timestamp.or(self.last_timestamp);
    }

    fn reply_text(&mut self, message: &str, block: &str, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if self.reply_message.as_deref() != Some(message) {
            self.reply_message = Some(message.to_string());
            self.reply_blocks.clear();
            self.final_reply.clear();
        }
        if self.reply_blocks.insert(block.to_string()) {
            if !self.final_reply.is_empty() {
                self.final_reply.push('\n');
            }
            self.final_reply.push_str(text);
        }
    }

    fn add_tool(&mut self, block: &Value, start: u64, timestamp: Option<SystemTime>) {
        self.add_tool_value(
            str_field(block, "id").unwrap_or(""),
            str_field(block, "name").unwrap_or(""),
            block.get("input").unwrap_or(&Value::Null),
            start,
            timestamp,
        );
    }

    fn add_tool_value(
        &mut self,
        id: &str,
        tool: &str,
        input: &Value,
        start: u64,
        timestamp: Option<SystemTime>,
    ) {
        let id = if id.is_empty() {
            format!("anonymous:{start}")
        } else {
            id.to_string()
        };
        if let Some(position) = self.tools.get(&id).copied() {
            if let ReviewItem::Tool(existing) = &mut self.items[position] {
                existing.tool = tool.to_string();
                existing.summary = tool_summary(tool, input);
                existing.detail.input = detail_input(input);
                existing.lines = edit_lines(tool, input);
            }
            return;
        }
        let position = self.items.len();
        self.items.push(ReviewItem::Tool(ReviewTool {
            id: id.clone(),
            tool: tool.to_string(),
            summary: tool_summary(tool, input),
            status: ReviewToolStatus::Running,
            started_at: timestamp,
            completed_at: None,
            error_excerpt: Vec::new(),
            lines: edit_lines(tool, input),
            detail: ReviewToolDetail {
                input: detail_input(input),
                output: Vec::new(),
            },
            output_locator: None,
        }));
        self.tools.insert(id, position);
    }

    fn complete_tool(
        &mut self,
        id: &str,
        status: ReviewToolStatus,
        output: &str,
        start: u64,
        end: u64,
        timestamp: Option<SystemTime>,
    ) -> bool {
        let Some(position) = self.tools.get(id).copied() else {
            return false;
        };
        let ReviewItem::Tool(tool) = &mut self.items[position] else {
            return false;
        };
        tool.error_excerpt = if matches!(status, ReviewToolStatus::Failed { .. }) {
            error_excerpt(output)
        } else {
            Vec::new()
        };
        tool.status = status;
        tool.completed_at = timestamp;
        tool.detail.output = detail_output(output);
        tool.output_locator = Some(ReviewRecordLocator {
            start_offset: start,
            end_offset: end,
        });
        true
    }

    fn adopt_codex_cursor(&mut self, turn_id: &str) {
        if self.cursor.is_none() || matches!(self.cursor, Some(TurnCursor::Fallback { .. })) {
            self.cursor = Some(TurnCursor::Codex {
                turn_id: turn_id.to_string(),
            });
        }
    }
}

fn codex_turn_id(payload: &Value) -> Option<&str> {
    str_field(payload, "turn_id")
        .or_else(|| {
            payload
                .pointer("/internal_chat_message_metadata_passthrough/turn_id")
                .and_then(Value::as_str)
        })
        .filter(|id| !id.is_empty())
}

fn codex_tool_input(payload: &Value) -> Value {
    match str_field(payload, "type") {
        Some("function_call") => str_field(payload, "arguments")
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or(Value::Null),
        _ => json!({ "input": str_field(payload, "input").unwrap_or("") }),
    }
}

fn codex_output(output: &str) -> (ReviewToolStatus, String) {
    if output.starts_with("aborted by user") {
        return (ReviewToolStatus::Interrupted, output.to_string());
    }
    if let Ok(Value::Object(legacy)) = serde_json::from_str::<Value>(output) {
        if let Some(body) = legacy.get("output").and_then(Value::as_str) {
            let exit = legacy
                .get("metadata")
                .and_then(|metadata| metadata.get("exit_code"))
                .and_then(Value::as_i64)
                .map(|exit| exit as i32);
            return (tool_status(exit), body.to_string());
        }
    }
    let exit = exit_code(output);
    let body = match output.split_once("\nOutput:\n") {
        Some((_, body)) if exit.is_some() => body.to_string(),
        _ => output.to_string(),
    };
    (tool_status(exit), body)
}

fn tool_status(exit: Option<i32>) -> ReviewToolStatus {
    match exit {
        Some(code) if code != 0 => ReviewToolStatus::Failed { exit: Some(code) },
        _ => ReviewToolStatus::Ok,
    }
}

fn timestamp(record: &Value) -> Option<SystemTime> {
    str_field(record, "timestamp").and_then(parse_timestamp)
}

fn seconds_since(previous: Option<SystemTime>, current: Option<SystemTime>) -> Option<f32> {
    current?
        .duration_since(previous?)
        .ok()
        .map(|duration| duration.as_secs_f32())
}

fn fallback(raw: &[u8], end_offset: u64) -> TurnCursor {
    let hash = fnv1a64(raw);
    TurnCursor::Fallback {
        end_offset,
        fingerprint: format!("fnv1a64:{hash:016x}"),
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
