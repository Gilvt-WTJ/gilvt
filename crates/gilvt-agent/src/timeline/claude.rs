//! Claude Code transcript records → timeline.

use std::time::SystemTime;

use serde_json::Value;

use super::plan::created_task_id;
use super::text::{content_text, exit_code};
use super::{ItemStatus, Source, Timeline, ToolCall};
use crate::event::{str_field, u64_field, Event};

/// Model name on locally synthesized messages (API errors, interruptions): no tokens.
const SYNTHETIC_MODEL: &str = "<synthetic>";
/// `toolUseResult` of a rejection by the user: a no in the approval dialog, or Esc while the call ran (a sandbox
/// block has the same `toolDenialKind: user-rejected` but its own message).
const USER_REJECTED: &str = "User rejected tool use";

pub(super) fn apply_record(tl: &mut Timeline, r: &Value, ts: Option<SystemTime>) {
    if r.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return;
    }
    match str_field(r, "type") {
        Some("assistant") => assistant(tl, r, ts),
        Some("user") => user(tl, r, ts),
        Some("system") if str_field(r, "subtype") == Some("turn_duration") => {
            let turn = tl.tr_turn(ts);
            tl.end_turn(turn, ts);
        }
        _ => {}
    }
}

fn assistant(tl: &mut Timeline, r: &Value, ts: Option<SystemTime>) {
    let Some(message) = r.get("message") else { return };
    let content = message.get("content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    if r.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true) {
        let text = content.iter().find_map(|c| str_field(c, "text")).unwrap_or("API error").to_string();
        let turn = tl.tr_turn(ts);
        tl.fail_turn(turn, text, ts);
        return;
    }
    let turn = tl.tr_turn(ts);
    let tokens = reply_tokens(tl, message);
    tl.add_tokens(turn, tokens);
    let mut uses_tool = false;
    for block in content {
        match str_field(block, "type") {
            Some("thinking") => {
                let secs = tl.secs_since_last(ts);
                tl.thinking(secs, str_field(block, "thinking").unwrap_or(""), ts);
            }
            Some("tool_use") => {
                uses_tool = true;
                let input = block.get("input").unwrap_or(&Value::Null);
                let (id, name) = (str_field(block, "id").unwrap_or(""), str_field(block, "name").unwrap_or(""));
                tl.plan.apply_call(id, name, input);
                tl.transcript_tool(ToolCall::new(id, name, input), ts);
            }
            Some("text") => {
                if let Some(body) = str_field(block, "text") {
                    tl.set_reply(turn, body);
                }
            }
            _ => {}
        }
    }
    let stop = str_field(message, "stop_reason");
    if !uses_tool && matches!(stop, Some("end_turn" | "stop_sequence" | "max_tokens")) {
        tl.end_turn(turn, ts);
    }
}

/// New tokens of a reply: input + output + cache, counted once per `message.id` (Claude Code writes one
/// record per content block, each repeating the usage; the later ones can be larger).
pub(super) fn reply_tokens(tl: &mut Timeline, message: &Value) -> u64 {
    let model = str_field(message, "model").unwrap_or("");
    let Some(usage) = message.get("usage").filter(|_| model != SYNTHETIC_MODEL) else { return 0 };
    let total = ["input_tokens", "output_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"]
        .iter()
        .map(|k| u64_field(usage, k))
        .sum::<u64>();
    match str_field(message, "id") {
        Some(id) => {
            let counted = tl.message_tokens.entry(id.to_string()).or_insert(0);
            let new = total.saturating_sub(*counted);
            *counted = (*counted).max(total);
            new
        }
        None => total,
    }
}

fn user(tl: &mut Timeline, r: &Value, ts: Option<SystemTime>) {
    if let Some(origin) = r.get("origin").filter(|o| str_field(o, "kind") == Some("peer")) {
        if let (Some(true), Some(from)) = (origin.get("handback").and_then(Value::as_bool), str_field(origin, "from")) {
            let body = str_field(origin, "body").map(str::to_string);
            let body = body.unwrap_or_else(|| content_text(r.pointer("/message/content").unwrap_or(&Value::Null)));
            tl.hand_back_message(from, &body);
        }
        return;
    }
    if let Some(Value::Array(blocks)) = r.pointer("/message/content") {
        let results: Vec<&Value> = blocks.iter().filter(|b| str_field(b, "type") == Some("tool_result")).collect();
        if !results.is_empty() {
            for result in results {
                tool_result(tl, r, result, ts);
            }
            return;
        }
    }
    let text = content_text(r.pointer("/message/content").unwrap_or(&Value::Null));
    if text.trim_start().starts_with("<task-notification>") {
        tl.task_notification(&text);
        return;
    }
    // A custom slash-command / skill invocation: M3a's `crate::claude::parse_record` drops these (they are
    // not something the user typed as a prompt, for session-naming purposes), but they do run the model and
    // must open their own turn here. `<local-command-…>` records (built-in commands with no model work) are
    // left alone: they do not start with `<command-name>` / `<command-message>`.
    if let Some(prompt) = slash_command_prompt(&text) {
        tl.open_turn(Source::Transcript, &prompt, ts);
        tl.slash_turn = tl.tr_turn;
        return;
    }
    // A built-in command's output (`/exit`, `/model`, …): it ran locally, so the turn its echo opened goes.
    let start = text.trim_start();
    if start.starts_with("<local-command-stdout>") || start.starts_with("<local-command-stderr>") {
        tl.drop_local_command_turn();
        return;
    }
    for event in crate::claude::parse_record(r) {
        match event {
            Event::PromptSubmit { text } => tl.open_turn(Source::Transcript, &text, ts),
            Event::Interrupted => {
                let turn = tl.tr_turn(ts);
                tl.interrupt_turn(turn, ts);
            }
            _ => {}
        }
    }
}

/// The prompt of a custom slash-command / skill record: Claude Code writes the user's turn as
/// `<command-message>…</command-message>\n<command-name>/foo</command-name>\n<command-args>…</command-args>`
/// (message and args are optional / order is not guaranteed). The reconstructed prompt is what the hook's
/// `UserPromptSubmit` sends for the same turn: the raw `/foo args` text.
fn slash_command_prompt(text: &str) -> Option<String> {
    let start = text.trim_start();
    if !(start.starts_with("<command-name>") || start.starts_with("<command-message>")) {
        return None;
    }
    let name = tag_content(text, "command-name")?;
    let args = tag_content(text, "command-args").unwrap_or("");
    Some(if args.is_empty() { name.to_string() } else { format!("{name} {args}") })
}

/// The text between `<tag>` and `</tag>`, trimmed.
fn tag_content<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let start = text.find(&open)? + open.len();
    let end = start + text[start..].find(&close)?;
    Some(text[start..end].trim())
}

/// A `tool_result` block of a user record (the session's or a subagent's).
pub(super) fn tool_result(tl: &mut Timeline, r: &Value, result: &Value, ts: Option<SystemTime>) {
    let id = str_field(result, "tool_use_id").unwrap_or("");
    let output = content_text(result.get("content").unwrap_or(&Value::Null));
    let is_error = result.get("is_error").and_then(Value::as_bool) == Some(true);
    let interrupted = r.pointer("/toolUseResult/interrupted").and_then(Value::as_bool) == Some(true);
    let denial = str_field(r, "toolDenialKind");
    if denial.is_some() && str_field(r, "toolUseResult") == Some(USER_REJECTED) {
        tl.transcript_rejected(id, &output, ts);
        return;
    }
    let status = if denial.is_some() {
        ItemStatus::Denied
    } else if interrupted {
        ItemStatus::Interrupted
    } else if is_error {
        ItemStatus::Failed { exit: exit_code(&output) }
    } else {
        ItemStatus::Ok
    };
    tl.transcript_result(id, status, &output, ts);
    let tool_use_result = r.get("toolUseResult").filter(|v| v.is_object());
    if let Some(task) = created_task_id(tool_use_result, &output) {
        tl.plan.task_created(id, &task);
    }
    tl.spawner_result(id, tool_use_result, &output);
}
