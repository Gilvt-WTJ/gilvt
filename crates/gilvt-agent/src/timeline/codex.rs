//! Codex rollout records → timeline. Tool calls are `function_call` / `custom_tool_call` response items,
//! results the matching `*_output` items (same `call_id`, which hooks send as `tool_use_id`).

use std::time::SystemTime;

use serde_json::{json, Value};

use super::text::{content_text, exit_code};
use super::{ItemStatus, Source, SubagentInfo, Timeline, ToolCall};
use crate::event::{str_field, u64_field};

/// Output of a command the user stopped with Esc.
const ABORTED: &str = "aborted by user";

pub(super) fn apply_record(tl: &mut Timeline, r: &Value, ts: Option<SystemTime>) {
    let p = r.get("payload").unwrap_or(&Value::Null);
    match str_field(r, "type") {
        Some("event_msg") => event_msg(tl, p, ts),
        Some("response_item") => response_item(tl, p, ts),
        _ => {}
    }
}

fn event_msg(tl: &mut Timeline, p: &Value, ts: Option<SystemTime>) {
    match str_field(p, "type") {
        Some("user_message") => tl.open_turn(Source::Transcript, str_field(p, "message").unwrap_or(""), ts),
        Some("task_complete") => {
            let turn = tl.tr_turn(ts);
            tl.end_turn(turn, ts);
        }
        Some("turn_aborted") if str_field(p, "reason") == Some("interrupted") => {
            let turn = tl.tr_turn(ts);
            tl.interrupt_turn(turn, ts);
        }
        Some("turn_aborted") => {
            let turn = tl.tr_turn(ts);
            let reason = str_field(p, "reason").unwrap_or("unknown");
            tl.fail_turn(turn, format!("turn aborted: {reason}"), ts);
        }
        Some("error") => {
            let turn = tl.tr_turn(ts);
            tl.fail_turn(turn, str_field(p, "message").unwrap_or("error").to_string(), ts);
        }
        Some("agent_message") => {
            let turn = tl.tr_turn(ts);
            tl.set_reply(turn, str_field(p, "message").unwrap_or(""));
        }
        Some("token_count") => token_count(tl, p, ts),
        Some("patch_apply_end") if p.get("success").and_then(Value::as_bool) == Some(false) => {
            let out = [str_field(p, "stderr"), str_field(p, "stdout")].into_iter().flatten().collect::<Vec<_>>();
            let id = str_field(p, "call_id").unwrap_or("");
            tl.transcript_result(id, ItemStatus::Failed { exit: None }, &out.join("\n"), ts);
        }
        Some("item_completed") => completed_item(tl, p.get("item").unwrap_or(&Value::Null), ts),
        _ => {}
    }
}

fn completed_item(tl: &mut Timeline, item: &Value, ts: Option<SystemTime>) {
    if str_field(item, "type") != Some("SubAgentActivity") {
        return;
    }
    let kind = str_field(item, "kind").unwrap_or("");
    let thread = str_field(item, "agent_thread_id").unwrap_or("");
    let path = str_field(item, "agent_path").unwrap_or("");
    let event_id = str_field(item, "id").unwrap_or("");
    let call_id = if kind == "started" {
        event_id.to_string()
    } else {
        [thread, path].into_iter().find_map(|key| tl.codex_subagents.get(key).cloned()).unwrap_or_default()
    };
    if call_id.is_empty() {
        return;
    }
    if tl.tool_mut(&call_id).is_none() && kind == "started" {
        let input = json!({"task_name": path.trim_start_matches("/root/")});
        tl.transcript_tool(ToolCall::new(&call_id, "spawn_agent", &input), ts);
    }
    let Some(tool) = tl.tool_mut(&call_id) else { return };
    let agent_type = tool.subagent.as_ref().and_then(|sub| sub.agent_type.clone()).or_else(|| {
        tool.detail.input.iter().find(|(key, _)| key == "task_name" || key == "agent_type")
            .map(|(_, value)| value.clone()).filter(|value| !value.is_empty())
    });
    let agent_id = if !thread.is_empty() { thread } else { path };
    tool.subagent = Some(SubagentInfo {
        agent_id: agent_id.to_string(), agent_type,
        result: tool.subagent.as_ref().and_then(|sub| sub.result.clone()), done: kind == "completed",
    });
    match kind {
        "completed" => {
            tool.status = ItemStatus::Ok;
            tool.ended = ts.or(tool.ended);
        }
        "started" | "interacted" => {
            tool.status = ItemStatus::Running;
            tool.ended = None;
        }
        _ => return,
    }
    let call_id = call_id.clone();
    for key in [thread, path].into_iter().filter(|key| !key.is_empty()) {
        tl.codex_subagents.insert(key.to_string(), call_id.clone());
    }
}

/// Tokens of the turn = growth of the session total (`total_token_usage`); without it, the last request's.
fn token_count(tl: &mut Timeline, p: &Value, ts: Option<SystemTime>) {
    let Some(info) = p.get("info").filter(|i| i.is_object()) else { return };
    let added = match info.get("total_token_usage") {
        Some(total) => {
            let total = u64_field(total, "total_tokens");
            let added = total.saturating_sub(tl.codex_total);
            tl.codex_total = tl.codex_total.max(total);
            added
        }
        None => info.get("last_token_usage").map_or(0, |last| u64_field(last, "total_tokens")),
    };
    let turn = tl.tr_turn(ts);
    tl.add_tokens(turn, added);
}

fn response_item(tl: &mut Timeline, p: &Value, ts: Option<SystemTime>) {
    let id = str_field(p, "call_id").unwrap_or("");
    let name = str_field(p, "name").unwrap_or("");
    match str_field(p, "type") {
        Some("function_call") => {
            let args = str_field(p, "arguments").and_then(|a| serde_json::from_str(a).ok()).unwrap_or(Value::Null);
            if link_background_followup(tl, id, name, &args) {
                return;
            }
            tl.plan.apply_call(id, name, &args);
            tl.transcript_tool(ToolCall::new(id, name, &args), ts);
            mark_subagent_call(tl, id, name, &args);
        }
        Some("custom_tool_call") => {
            let raw = str_field(p, "input").unwrap_or("");
            let (name, input) = nested_exec(name, raw).unwrap_or_else(|| (name.to_string(), json!({"input": raw})));
            if link_background_followup(tl, id, &name, &input) {
                return;
            }
            tl.transcript_tool(ToolCall::new(id, &name, &input), ts);
            mark_subagent_call(tl, id, &name, &input);
        }
        Some("function_call_output" | "custom_tool_call_output") => {
            let (mut status, body, exit) = output(p.get("output").unwrap_or(&Value::Null));
            let target = tl.codex_background_followups.remove(id).unwrap_or_else(|| id.to_string());
            if let Some(key) = background_key(&body) {
                tl.codex_background.insert(key, target.clone());
                if let Some(tool) = tl.tool_mut(&target) {
                    tool.status = ItemStatus::Running;
                    tool.ended = None;
                    tool.detail.output = background_body(&body);
                }
                return;
            }
            if target != id {
                tl.codex_background.retain(|_, call| call != &target);
            }
            if subagent_started(&body) && tl.tool_mut(&target).is_some_and(|tool| tool.subagent.is_some()) {
                if let Some(tool) = tl.tool_mut(&target) {
                    tool.detail.output = super::text::detail_output(&body);
                    tool.status = ItemStatus::Running;
                    tool.ended = None;
                }
                return;
            }
            // A plain-text output (no exit code) says nothing about failure: keep an earlier
            // `patch_apply_end success:false` verdict.
            if status == ItemStatus::Ok && exit.is_none() {
                if let Some(failed @ ItemStatus::Failed { .. }) = tl.tool_mut(&target).map(|t| t.status.clone()) {
                    status = failed;
                }
            }
            tl.transcript_result(&target, status, &body, ts);
        }
        Some("reasoning") => {
            let summary = p.get("summary").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
            let text: Vec<&str> = summary.iter().filter_map(|s| str_field(s, "text")).collect();
            let secs = tl.secs_since_last(ts);
            tl.thinking(secs, &text.join("\n"), ts);
        }
        _ => {}
    }
}

fn mark_subagent_call(tl: &mut Timeline, id: &str, name: &str, input: &Value) {
    if name != "spawn_agent" {
        return;
    }
    let task = str_field(input, "task_name").unwrap_or("");
    let agent_type = (!task.is_empty()).then_some(task).or_else(|| str_field(input, "agent_type").filter(|s| !s.is_empty()));
    if let Some(tool) = tl.tool_mut(id) {
        tool.subagent = Some(SubagentInfo {
            agent_id: task.to_string(), agent_type: agent_type.map(str::to_string), result: None, done: false,
        });
    }
    if !task.is_empty() {
        tl.codex_subagents.insert(task.to_string(), id.to_string());
        tl.codex_subagents.insert(format!("/root/{task}"), id.to_string());
    }
}

fn link_background_followup(tl: &mut Timeline, id: &str, name: &str, input: &Value) -> bool {
    if !matches!(name, "write_stdin" | "wait") {
        return false;
    }
    let key = input.get("session_id").or_else(|| input.get("cell_id")).and_then(|value| {
        value.as_str().map(str::to_string).or_else(|| value.as_u64().map(|n| n.to_string()))
    });
    let Some(root) = key.and_then(|key| tl.codex_background.get(&key).cloned()) else { return false };
    tl.codex_background_followups.insert(id.to_string(), root);
    true
}

/// The current Codex tool bridge wraps terminal calls in a JavaScript `exec` custom tool. Recover the
/// nested call name and the small argument subset needed by the timeline without trying to parse JS.
fn nested_exec(name: &str, input: &str) -> Option<(String, Value)> {
    if name != "exec" {
        return None;
    }
    let after = input.split_once("tools.")?.1;
    let tool = after.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect::<String>();
    if tool.is_empty() {
        return None;
    }
    let mut args = serde_json::Map::new();
    for key in ["cmd", "session_id", "cell_id", "chars"] {
        if let Some(value) = js_property(input, key) {
            args.insert(key.to_string(), value);
        }
    }
    if args.is_empty() {
        args.insert("input".into(), input.into());
    }
    Some((tool, Value::Object(args)))
}

fn js_property(input: &str, key: &str) -> Option<Value> {
    let starts = [format!("{key}:"), format!("\"{key}\":"), format!("'{key}':")];
    let (_, rest) = starts.iter().filter_map(|needle| input.split_once(needle)).min_by_key(|(before, _)| before.len())?;
    let rest = rest.trim_start();
    if rest.starts_with(['\"', '\'']) {
        return js_string(rest).map(Value::String);
    }
    let token = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect::<String>();
    (!token.is_empty()).then(|| token.parse::<i64>().ok().map(Value::from)).flatten()
}

fn js_string(input: &str) -> Option<String> {
    let quote = input.chars().next()?;
    let mut escaped = false;
    for (offset, ch) in input[quote.len_utf8()..].char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            let raw = &input[..offset + quote.len_utf8() + ch.len_utf8()];
            return if quote == '\"' {
                serde_json::from_str(raw).ok()
            } else {
                Some(raw[1..raw.len() - 1].replace("\\'", "'").replace("\\\\", "\\"))
            };
        }
    }
    None
}

fn background_key(output: &str) -> Option<String> {
    const PREFIXES: [&str; 2] = ["Process running with session ID ", "Script running with cell ID "];
    output.lines().find_map(|line| {
        let rest = PREFIXES.iter().find_map(|prefix| line.strip_prefix(prefix))?;
        let key = rest.split_whitespace().next().unwrap_or("");
        (!key.is_empty()).then(|| key.to_string())
    })
}

fn background_body(output: &str) -> Vec<String> {
    let body = output.split_once("\nOutput:\n").map_or("", |(_, body)| body);
    super::text::detail_output(body)
}

fn subagent_started(output: &str) -> bool {
    serde_json::from_str::<Value>(output).ok().is_some_and(|value| str_field(&value, "task_name").is_some())
}

/// Status and body of a tool output: `Process exited with code N` / `Exit code: N` headers before an
/// `Output:` line, the legacy JSON form `{"output", "metadata": {"exit_code"}}`, or `aborted by user …`;
/// plus the exit code when the output names one.
fn output(value: &Value) -> (ItemStatus, String, Option<i32>) {
    let text = content_text(value);
    if text.starts_with(ABORTED) {
        return (ItemStatus::Interrupted, text, None);
    }
    if let Ok(Value::Object(legacy)) = serde_json::from_str::<Value>(&text) {
        if let Some(body) = legacy.get("output").and_then(Value::as_str) {
            let exit = legacy.get("metadata").and_then(|m| m.get("exit_code")).and_then(Value::as_i64).map(|e| e as i32);
            return (status_of(exit), body.to_string(), exit);
        }
    }
    let exit = exit_code(&text);
    let body = match text.split_once("\nOutput:\n") {
        Some((_, body)) if exit.is_some() => body.to_string(),
        _ => text,
    };
    (status_of(exit), body, exit)
}

fn status_of(exit: Option<i32>) -> ItemStatus {
    match exit {
        Some(code) if code != 0 => ItemStatus::Failed { exit: Some(code) },
        _ => ItemStatus::Ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outputs() {
        let exec = json!("Chunk ID: 40c68c\nWall time: 0.0082 seconds\nProcess exited with code 0\nOriginal token count: 1\nOutput:\nhi\n");
        assert_eq!(output(&exec), (ItemStatus::Ok, "hi\n".to_string(), Some(0)));
        let failed = json!("Chunk ID: d8\nWall time: 0.0000 seconds\nProcess exited with code 1\nOriginal token count: 28\nOutput:\nsed: x.md: No such file or directory\n");
        assert_eq!(
            output(&failed),
            (ItemStatus::Failed { exit: Some(1) }, "sed: x.md: No such file or directory\n".into(), Some(1))
        );
        let patch = json!(
            "Exit code: 0\nWall time: 0.1 seconds\nOutput:\nSuccess. Updated the following files:\nA hello.txt\n"
        );
        assert_eq!(output(&patch).0, ItemStatus::Ok);
        assert_eq!(output(&json!("aborted by user after 9.4s")).0, ItemStatus::Interrupted);
        let legacy = json!(r#"{"output":"boom\n","metadata":{"exit_code":2,"duration_seconds":0.1}}"#);
        assert_eq!(output(&legacy), (ItemStatus::Failed { exit: Some(2) }, "boom\n".into(), Some(2)));
        assert_eq!(output(&json!("Plan updated")), (ItemStatus::Ok, "Plan updated".into(), None));
    }
}
