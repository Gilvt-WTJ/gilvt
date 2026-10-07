use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use serde_json::Value;

use super::{
    fnv1a64, fnv1a64_extend, ReviewOutcome, ReviewSessionIndex, ReviewTurnIndex, TurnCursor,
};
use crate::event::{str_field, u64_field, AgentKind, Event};
use crate::summary::{first_line, truncate_chars};
use crate::timeline::text::{content_text, edit_lines, exit_code, parse_timestamp};
use crate::SessionKey;

const PROMPT_PREVIEW_MAX: usize = 160;
const REPLY_PREVIEW_MAX: usize = 240;
const SYNTHETIC_MODEL: &str = "<synthetic>";

#[derive(Debug)]
struct BuildingTurn {
    cursor: TurnCursor,
    ordinal: u32,
    start_offset: u64,
    prompt_preview: String,
    reply_preview: String,
    started_at: Option<SystemTime>,
    tool_ids: HashSet<String>,
    failed_tool_ids: HashSet<String>,
    lines_added: u32,
    lines_removed: u32,
    tokens: u64,
    claude_message_tokens: HashMap<String, u64>,
    fingerprint: u64,
}

impl BuildingTurn {
    fn new(
        cursor: TurnCursor,
        ordinal: u32,
        start_offset: u64,
        prompt: &str,
        started_at: Option<SystemTime>,
    ) -> Self {
        BuildingTurn {
            cursor,
            ordinal,
            start_offset,
            prompt_preview: preview(prompt, PROMPT_PREVIEW_MAX),
            reply_preview: String::new(),
            started_at,
            tool_ids: HashSet::new(),
            failed_tool_ids: HashSet::new(),
            lines_added: 0,
            lines_removed: 0,
            tokens: 0,
            claude_message_tokens: HashMap::new(),
            fingerprint: 0xcbf29ce484222325,
        }
    }

    fn record(&mut self, raw: &[u8]) {
        self.fingerprint = fnv1a64_extend(self.fingerprint, raw);
    }

    fn index(
        self,
        end_offset: u64,
        completed_at: Option<SystemTime>,
        outcome: ReviewOutcome,
    ) -> ReviewTurnIndex {
        ReviewTurnIndex {
            cursor: self.cursor,
            ordinal: self.ordinal,
            start_offset: self.start_offset,
            end_offset,
            fingerprint: format!("fnv1a64:{:016x}", self.fingerprint),
            prompt_preview: self.prompt_preview,
            reply_preview: self.reply_preview,
            started_at: self.started_at,
            completed_at,
            outcome,
            tool_count: self.tool_ids.len() as u32,
            failed_tool_count: self.failed_tool_ids.len() as u32,
            lines_added: self.lines_added,
            lines_removed: self.lines_removed,
            tokens: self.tokens,
        }
    }
}

/// Builds the review projection while the history parser reads the transcript once.
pub(crate) struct ReviewScanner {
    agent: AgentKind,
    turns: Vec<ReviewTurnIndex>,
    current: Option<BuildingTurn>,
    next_ordinal: u32,
    last_timestamp: Option<SystemTime>,
    last_end_offset: u64,
    codex_turn_id: Option<String>,
    codex_total_tokens: u64,
}

impl ReviewScanner {
    pub(crate) fn new(agent: AgentKind) -> Self {
        ReviewScanner {
            agent,
            turns: Vec::new(),
            current: None,
            next_ordinal: 1,
            last_timestamp: None,
            last_end_offset: 0,
            codex_turn_id: None,
            codex_total_tokens: 0,
        }
    }

    pub(crate) fn resume(agent: AgentKind, turns: Vec<ReviewTurnIndex>) -> Self {
        let codex_total_tokens = turns.iter().map(|turn| turn.tokens).sum();
        ReviewScanner {
            agent,
            next_ordinal: turns.len() as u32 + 1,
            turns,
            current: None,
            last_timestamp: None,
            last_end_offset: 0,
            codex_turn_id: None,
            codex_total_tokens,
        }
    }

    pub(crate) fn apply(&mut self, record: &Value, start_offset: u64, end_offset: u64, raw: &[u8]) {
        let timestamp = record
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_timestamp);
        match self.agent {
            AgentKind::Claude => self.claude(record, start_offset, end_offset, raw, timestamp),
            AgentKind::Codex => self.codex(record, start_offset, end_offset, raw, timestamp),
        }
        self.last_timestamp = timestamp.or(self.last_timestamp);
        self.last_end_offset = end_offset;
    }

    pub(crate) fn finish(
        self,
        key: SessionKey,
        transcript: PathBuf,
        transcript_size: u64,
        scanned_through: u64,
        incomplete_tail: bool,
    ) -> ReviewSessionIndex {
        ReviewSessionIndex {
            key,
            transcript,
            transcript_size,
            scanned_through,
            incomplete_tail,
            turns: self.turns,
        }
    }

    fn open(
        &mut self,
        cursor: TurnCursor,
        prompt: &str,
        start_offset: u64,
        timestamp: Option<SystemTime>,
    ) {
        if self.current.is_some() {
            self.complete(
                ReviewOutcome::Done,
                self.last_timestamp,
                self.last_end_offset,
            );
        }
        let ordinal = self.next_ordinal;
        self.next_ordinal += 1;
        self.current = Some(BuildingTurn::new(
            cursor,
            ordinal,
            start_offset,
            prompt,
            timestamp,
        ));
    }

    fn complete(&mut self, outcome: ReviewOutcome, timestamp: Option<SystemTime>, end_offset: u64) {
        if let Some(turn) = self.current.take() {
            self.turns.push(turn.index(end_offset, timestamp, outcome));
        }
    }

    fn fallback(raw: &[u8], end_offset: u64) -> TurnCursor {
        let hash = fnv1a64(raw);
        TurnCursor::Fallback {
            end_offset,
            fingerprint: format!("fnv1a64:{hash:016x}"),
        }
    }

    fn claude(&mut self, r: &Value, start: u64, end: u64, raw: &[u8], ts: Option<SystemTime>) {
        if r.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            if let Some(turn) = self.current.as_mut() {
                turn.record(raw);
            }
            return;
        }
        let events = crate::claude::parse_record(r);
        if let Some(prompt) = events.iter().find_map(|event| match event {
            Event::PromptSubmit { text } => Some(text.as_str()),
            _ => None,
        }) {
            let cursor = str_field(r, "uuid")
                .filter(|id| !id.is_empty())
                .map(|prompt_uuid| TurnCursor::Claude {
                    prompt_uuid: prompt_uuid.to_string(),
                })
                .unwrap_or_else(|| Self::fallback(raw, end));
            self.open(cursor, prompt, start, ts);
            if let Some(turn) = self.current.as_mut() {
                turn.record(raw);
            }
        } else if let Some(turn) = self.current.as_mut() {
            turn.record(raw);
        }

        match str_field(r, "type") {
            Some("assistant") => self.claude_assistant(r),
            Some("user") => self.claude_tool_results(r, start),
            _ => {}
        }
        for event in events {
            match event {
                Event::Interrupted => self.complete(ReviewOutcome::Interrupted, ts, end),
                Event::Error { message } => self.complete(
                    ReviewOutcome::Failed {
                        message: preview(&message, REPLY_PREVIEW_MAX),
                    },
                    ts,
                    end,
                ),
                Event::TurnEnd { .. } => self.complete(ReviewOutcome::Done, ts, end),
                _ => {}
            }
        }
    }

    fn claude_assistant(&mut self, r: &Value) {
        let Some(turn) = self.current.as_mut() else {
            return;
        };
        let Some(message) = r.get("message") else {
            return;
        };
        let content = message
            .get("content")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let text = content
            .iter()
            .filter(|block| str_field(block, "type") == Some("text"))
            .filter_map(|block| str_field(block, "text"))
            .collect::<Vec<_>>()
            .join("\n");
        if !text.trim().is_empty() {
            turn.reply_preview = preview(&text, REPLY_PREVIEW_MAX);
        }
        for (index, block) in content
            .iter()
            .enumerate()
            .filter(|(_, block)| str_field(block, "type") == Some("tool_use"))
        {
            let id = str_field(block, "id")
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("anonymous:{}:{index}", turn.start_offset));
            if turn.tool_ids.insert(id) {
                let tool = str_field(block, "name").unwrap_or("");
                if let Some((added, removed)) =
                    edit_lines(tool, block.get("input").unwrap_or(&Value::Null))
                {
                    turn.lines_added += added;
                    turn.lines_removed += removed;
                }
            }
        }
        if str_field(message, "model") != Some(SYNTHETIC_MODEL) {
            if let Some(usage) = message.get("usage") {
                let total = [
                    "input_tokens",
                    "output_tokens",
                    "cache_creation_input_tokens",
                    "cache_read_input_tokens",
                ]
                .iter()
                .map(|key| u64_field(usage, key))
                .sum::<u64>();
                let id = str_field(message, "id").unwrap_or("");
                let counted = turn
                    .claude_message_tokens
                    .entry(id.to_string())
                    .or_insert(0);
                turn.tokens += total.saturating_sub(*counted);
                *counted = (*counted).max(total);
            }
        }
    }

    fn claude_tool_results(&mut self, r: &Value, start: u64) {
        let Some(turn) = self.current.as_mut() else {
            return;
        };
        let Some(blocks) = r.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        for (index, block) in blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| str_field(block, "type") == Some("tool_result"))
        {
            let failed = block.get("is_error").and_then(Value::as_bool) == Some(true)
                || r.get("toolDenialKind").is_some()
                || r.pointer("/toolUseResult/interrupted")
                    .and_then(Value::as_bool)
                    == Some(true);
            if failed {
                let id = str_field(block, "tool_use_id")
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("result:{start}:{index}"));
                turn.failed_tool_ids.insert(id);
            }
        }
    }

    fn codex(&mut self, r: &Value, start: u64, end: u64, raw: &[u8], ts: Option<SystemTime>) {
        let payload = r.get("payload").unwrap_or(&Value::Null);
        if (str_field(r, "type"), str_field(payload, "type"))
            == (Some("event_msg"), Some("user_message"))
        {
            let cursor = self
                .codex_turn_id
                .clone()
                .map(|turn_id| TurnCursor::Codex { turn_id })
                .unwrap_or_else(|| Self::fallback(raw, end));
            self.open(
                cursor,
                str_field(payload, "message").unwrap_or(""),
                start,
                ts,
            );
            if let Some(turn) = self.current.as_mut() {
                turn.record(raw);
            }
            return;
        }
        if let Some(turn) = self.current.as_mut() {
            turn.record(raw);
        }
        match (str_field(r, "type"), str_field(payload, "type")) {
            (Some("event_msg"), Some("task_started")) => {
                self.codex_turn_id = str_field(payload, "turn_id")
                    .filter(|id| !id.is_empty())
                    .map(str::to_string);
            }
            (Some("response_item"), Some("message"))
                if str_field(payload, "role") == Some("assistant") =>
            {
                if let Some(turn_id) = codex_item_turn_id(payload) {
                    self.adopt_codex_id(turn_id);
                }
                if let Some(turn) = self.current.as_mut() {
                    let text = content_text(payload.get("content").unwrap_or(&Value::Null));
                    if !text.trim().is_empty() {
                        turn.reply_preview = preview(&text, REPLY_PREVIEW_MAX);
                    }
                }
            }
            (Some("response_item"), Some("function_call" | "custom_tool_call")) => {
                if let Some(turn_id) = codex_item_turn_id(payload) {
                    self.adopt_codex_id(turn_id);
                }
                self.codex_tool(payload, start);
            }
            (Some("response_item"), Some("function_call_output" | "custom_tool_call_output")) => {
                self.codex_tool_output(payload, start);
            }
            (Some("event_msg"), Some("patch_apply_end"))
                if payload.get("success").and_then(Value::as_bool) == Some(false) =>
            {
                if let Some(turn) = self.current.as_mut() {
                    turn.failed_tool_ids
                        .insert(str_field(payload, "call_id").unwrap_or("patch").to_string());
                }
            }
            (Some("event_msg"), Some("token_count")) => self.codex_tokens(payload),
            (Some("event_msg"), Some("task_complete")) => {
                if let Some(turn_id) = str_field(payload, "turn_id").filter(|id| !id.is_empty()) {
                    self.adopt_codex_id(turn_id);
                }
                if let Some(turn) = self.current.as_mut() {
                    if let Some(reply) = str_field(payload, "last_agent_message")
                        .filter(|text| !text.trim().is_empty())
                    {
                        turn.reply_preview = preview(reply, REPLY_PREVIEW_MAX);
                    }
                }
                self.complete(ReviewOutcome::Done, ts, end);
            }
            (Some("event_msg"), Some("turn_aborted")) => {
                let outcome = if str_field(payload, "reason") == Some("interrupted") {
                    ReviewOutcome::Interrupted
                } else {
                    ReviewOutcome::Failed {
                        message: format!(
                            "turn aborted: {}",
                            str_field(payload, "reason").unwrap_or("unknown")
                        ),
                    }
                };
                self.complete(outcome, ts, end);
            }
            (Some("event_msg"), Some("error")) => {
                let message = preview(
                    str_field(payload, "message").unwrap_or("error"),
                    REPLY_PREVIEW_MAX,
                );
                self.complete(ReviewOutcome::Failed { message }, ts, end);
            }
            _ => {}
        }
    }

    fn adopt_codex_id(&mut self, turn_id: &str) {
        if let Some(turn) = self.current.as_mut() {
            if matches!(turn.cursor, TurnCursor::Fallback { .. }) {
                turn.cursor = TurnCursor::Codex {
                    turn_id: turn_id.to_string(),
                };
            }
        }
    }

    fn codex_tool(&mut self, payload: &Value, start: u64) {
        let Some(turn) = self.current.as_mut() else {
            return;
        };
        let id = str_field(payload, "call_id")
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("call:{start}"));
        if !turn.tool_ids.insert(id) {
            return;
        }
        let tool = str_field(payload, "name").unwrap_or("");
        let input = match str_field(payload, "type") {
            Some("function_call") => str_field(payload, "arguments")
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or(Value::Null),
            _ => serde_json::json!({ "input": str_field(payload, "input").unwrap_or("") }),
        };
        if let Some((added, removed)) = edit_lines(tool, &input) {
            turn.lines_added += added;
            turn.lines_removed += removed;
        }
    }

    fn codex_tool_output(&mut self, payload: &Value, start: u64) {
        let Some(turn) = self.current.as_mut() else {
            return;
        };
        let output = content_text(payload.get("output").unwrap_or(&Value::Null));
        if exit_code(&output).is_some_and(|code| code != 0) || output.starts_with("aborted by user")
        {
            let id = str_field(payload, "call_id")
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("output:{start}"));
            turn.failed_tool_ids.insert(id);
        }
    }

    fn codex_tokens(&mut self, payload: &Value) {
        let Some(turn) = self.current.as_mut() else {
            return;
        };
        let Some(info) = payload.get("info").filter(|value| value.is_object()) else {
            return;
        };
        let added = match info.get("total_token_usage") {
            Some(total) => {
                let total = u64_field(total, "total_tokens");
                let added = total.saturating_sub(self.codex_total_tokens);
                self.codex_total_tokens = self.codex_total_tokens.max(total);
                added
            }
            None => info
                .get("last_token_usage")
                .map_or(0, |last| u64_field(last, "total_tokens")),
        };
        turn.tokens += added;
    }
}

fn codex_item_turn_id(payload: &Value) -> Option<&str> {
    payload
        .pointer("/internal_chat_message_metadata_passthrough/turn_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}

fn preview(text: &str, max: usize) -> String {
    truncate_chars(first_line(text), max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_is_repeatable() {
        assert_eq!(
            ReviewScanner::fallback(b"same", 10),
            ReviewScanner::fallback(b"same", 10)
        );
        assert_ne!(
            ReviewScanner::fallback(b"same", 10),
            ReviewScanner::fallback(b"other", 10)
        );
    }

    #[test]
    fn opening_a_new_prompt_closes_a_missingly_terminated_turn() {
        let mut scan = ReviewScanner::new(AgentKind::Claude);
        let first = serde_json::json!({"type":"user","uuid":"p1","message":{"content":"one"}});
        let second = serde_json::json!({"type":"user","uuid":"p2","message":{"content":"two"}});
        scan.apply(&first, 0, 10, b"first");
        scan.apply(&second, 10, 20, b"second");
        assert_eq!(scan.turns.len(), 1);
        assert_eq!(
            scan.turns[0].cursor,
            TurnCursor::Claude {
                prompt_uuid: "p1".into()
            }
        );
        assert_eq!(scan.turns[0].end_offset, 10);
    }
}
