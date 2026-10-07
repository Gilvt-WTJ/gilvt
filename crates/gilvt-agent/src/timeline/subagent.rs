//! Claude subagents (M3b spec §3.4, §8.2): `<session>/subagents/agent-<agent_id>.jsonl` rows nest under the
//! parent Task / Agent call. Explicit links (meta.json `toolUseId`, the parent result's `toolUseResult.agentId`,
//! the hook's `tool_response.agentId`, a task notification) win; until one arrives a subagent is linked by
//! its prompt (== the call's `input.prompt`), else by order (`SubagentStart` → the latest open call).

use std::mem;
use std::time::SystemTime;

use serde_json::Value;

use super::text::{content_text, first_sentence, parse_timestamp, INPUT_VALUE_MAX};
use super::{claude, push_limited, Item, SubagentInfo, Timeline, ToolCall, ToolItem};
use crate::event::str_field;
use crate::summary::truncate_chars;

/// Tools whose calls start a subagent.
const SPAWNERS: [&str; 2] = ["Agent", "Task"];

/// What is known about one subagent.
#[derive(Clone, Debug, Default)]
pub(super) struct Sub {
    /// Tool-use id of the parent call.
    parent: Option<String>,
    /// The link came from the data itself, not from a prompt / order guess.
    explicit: bool,
    agent_type: Option<String>,
    /// Rows waiting for their parent call (not linked yet, or the call not seen yet).
    pub(super) items: Vec<Item>,
    result: Option<String>,
    /// `result` came from the parent's tool result / a task notification.
    official: bool,
    done: bool,
    last_text: Option<String>,
    last_ts: Option<SystemTime>,
    /// Reply tokens seen before the parent call is known / present in any turn; added to that turn's total
    /// (never to the hook or transcript cursor) once [`Timeline::adopt`] finds it.
    pending_tokens: u64,
}

impl Timeline {
    /// One line of `<session>/subagents/agent-<agent_id>.jsonl` (see [`crate::subagent_transcripts`]).
    pub fn apply_subagent_line(&mut self, agent_id: &str, line: &str) {
        let Ok(r) = serde_json::from_str::<Value>(line) else { return };
        let ts = r.get("timestamp").and_then(Value::as_str).and_then(parse_timestamp);
        match str_field(&r, "type") {
            Some("assistant") => self.sub_assistant(agent_id, &r, ts),
            Some("user") => self.sub_user(agent_id, &r, ts),
            _ => {}
        }
        if ts.is_some() {
            self.subs.entry(agent_id.to_string()).or_default().last_ts = ts;
        }
    }

    /// An explicit link, e.g. from `agent-<id>.meta.json` ([`crate::subagent_parent_tool_use`]).
    pub fn link_subagent(&mut self, agent_id: &str, tool_use_id: &str) {
        self.link_sub(agent_id, tool_use_id, true);
    }

    fn sub_assistant(&mut self, agent_id: &str, r: &Value, ts: Option<SystemTime>) {
        let Some(message) = r.get("message") else { return };
        let tokens = claude::reply_tokens(self, message);
        self.count_sub_tokens(agent_id, tokens);
        let content = message.get("content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
        for block in content {
            match str_field(block, "type") {
                Some("thinking") => {
                    let last = self.subs.get(agent_id).and_then(|s| s.last_ts);
                    let secs = ts.zip(last).and_then(|(t, l)| t.duration_since(l).ok()).map(|d| d.as_secs_f32());
                    let text = super::text::first_lines(str_field(block, "thinking").unwrap_or(""));
                    self.push_sub_item(agent_id, Item::Thinking { secs, text });
                }
                Some("tool_use") => {
                    let id = str_field(block, "id").unwrap_or("");
                    let call = ToolCall::new(
                        id,
                        str_field(block, "name").unwrap_or(""),
                        block.get("input").unwrap_or(&Value::Null),
                    );
                    self.sub_tool(agent_id, call, ts, None);
                }
                Some("text") => {
                    let text = str_field(block, "text").unwrap_or("").to_string();
                    self.subs.entry(agent_id.to_string()).or_default().last_text = Some(text);
                }
                _ => {}
            }
        }
        if str_field(message, "stop_reason") == Some("end_turn") {
            let text = self.subs.get(agent_id).and_then(|s| s.last_text.clone()).unwrap_or_default();
            self.sub_result(agent_id, &text, false, true);
        }
    }

    fn sub_user(&mut self, agent_id: &str, r: &Value, ts: Option<SystemTime>) {
        match r.pointer("/message/content") {
            // The first record is the prompt the parent call passed.
            Some(Value::String(prompt)) if r.get("parentUuid").is_none_or(Value::is_null) => {
                self.link_by_prompt(agent_id, prompt);
            }
            Some(Value::Array(blocks)) => {
                for result in blocks.iter().filter(|b| str_field(b, "type") == Some("tool_result")) {
                    claude::tool_result(self, r, result, ts);
                }
            }
            _ => {}
        }
    }

    /// A call inside a subagent (from its transcript, or a hook with `agent_id`): no anchor.
    pub(super) fn sub_tool(
        &mut self,
        agent_id: &str,
        call: ToolCall,
        ts: Option<SystemTime>,
        hook_at: Option<SystemTime>,
    ) {
        if let Some(t) = self.tool_mut(&call.id) {
            if hook_at.is_none() {
                t.tool = call.tool;
                t.summary = call.summary;
                t.detail.input = call.input;
                t.lines = call.lines;
            }
            t.started = hook_at.or(t.started).or(ts);
            return;
        }
        let mut item = super::new_tool(call);
        item.started = hook_at.or(ts);
        self.push_sub_item(agent_id, Item::Tool(item));
    }

    /// The rows of a subagent: under its parent call once both are known, else waiting in `Sub::items`.
    pub(super) fn sub_items_mut(&mut self, agent_id: &str) -> &mut Vec<Item> {
        let parent = self.subs.entry(agent_id.to_string()).or_default().parent.clone();
        if parent.as_deref().is_some_and(|p| self.tool_mut(p).is_some()) {
            return &mut self.tool_mut(parent.as_deref().unwrap_or_default()).expect("checked above").children;
        }
        &mut self.subs.get_mut(agent_id).expect("inserted above").items
    }

    fn push_sub_item(&mut self, agent_id: &str, item: Item) {
        push_limited(self.sub_items_mut(agent_id), item);
    }

    /// Counts a subagent reply's tokens towards its parent call's turn. Never creates or moves the hook /
    /// transcript turn cursor: when the parent isn't linked yet, or its call hasn't appeared in any turn yet,
    /// the tokens are buffered on the `Sub` and added once [`Timeline::adopt`] finds the parent (the session
    /// total is counted right away either way, so it is never short).
    fn count_sub_tokens(&mut self, agent_id: &str, tokens: u64) {
        self.session_tokens += tokens;
        match self.turn_of_linked_parent(agent_id) {
            Some(turn) => self.turns[turn].tokens += tokens,
            None => self.subs.entry(agent_id.to_string()).or_default().pending_tokens += tokens,
        }
    }

    /// The turn holding a subagent's linked parent call, if both are already known. Read-only: never inserts
    /// a turn or touches `tr_turn` / `hk_turn`.
    fn turn_of_linked_parent(&mut self, agent_id: &str) -> Option<usize> {
        let parent = self.subs.get(agent_id)?.parent.clone()?;
        self.turns.iter_mut().rposition(|t| super::find_tool(&mut t.items, &parent).is_some())
    }

    pub(super) fn link_sub(&mut self, agent_id: &str, parent: &str, explicit: bool) {
        if agent_id.is_empty() || parent.is_empty() {
            return;
        }
        let sub = self.subs.entry(agent_id.to_string()).or_default();
        match sub.parent.as_deref() {
            Some(p) if p == parent => {
                sub.explicit |= explicit;
                return;
            }
            Some(_) if sub.explicit || !explicit => return,
            _ => {}
        }
        let old = sub.parent.replace(parent.to_string());
        sub.explicit = explicit;
        if let Some(t) = old.and_then(|old| self.tool_mut(&old)) {
            // A guess was wrong: take the rows back from the other call.
            t.subagent = None;
            let rows = mem::take(&mut t.children);
            self.subs.get_mut(agent_id).expect("inserted above").items.splice(0..0, rows);
        }
        self.adopt(agent_id);
    }

    /// Moves waiting rows and buffered tokens under the parent call (if it exists) and refreshes its
    /// [`SubagentInfo`]. Never touches the hook / transcript turn cursor.
    pub(super) fn adopt(&mut self, agent_id: &str) {
        let Some(sub) = self.subs.get_mut(agent_id) else { return };
        let Some(parent) = sub.parent.clone() else { return };
        let info = SubagentInfo {
            agent_id: agent_id.to_string(),
            agent_type: sub.agent_type.clone(),
            result: sub.result.clone(),
            done: sub.done,
        };
        let rows = mem::take(&mut sub.items);
        let pending = mem::take(&mut sub.pending_tokens);
        if self.tool_mut(&parent).is_none() {
            let sub = self.subs.get_mut(agent_id).expect("checked above");
            sub.items = rows;
            sub.pending_tokens = pending;
            return;
        }
        if pending > 0 {
            if let Some(turn) = self.turns.iter_mut().rposition(|t| super::find_tool(&mut t.items, &parent).is_some()) {
                self.turns[turn].tokens += pending;
            }
        }
        let t = self.tool_mut(&parent).expect("checked above");
        for row in rows {
            push_limited(&mut t.children, row);
        }
        let requested = t.detail.input.iter().find(|(k, _)| k == "subagent_type").map(|(_, v)| v.clone());
        t.subagent = Some(SubagentInfo { agent_type: info.agent_type.or(requested), ..info });
    }

    /// A new call appeared: subagents waiting for it move in.
    pub(super) fn adopt_waiting(&mut self, tool_use_id: &str) {
        let waiting: Vec<String> = self
            .subs
            .iter()
            .filter(|(_, s)| s.parent.as_deref() == Some(tool_use_id))
            .map(|(a, _)| a.clone())
            .collect();
        for agent_id in waiting {
            self.adopt(&agent_id);
        }
    }

    /// The subagent's final reply. Official results (parent tool result, task notification) replace guesses
    /// (its last reply, `SubagentStop.last_assistant_message`).
    pub(super) fn sub_result(&mut self, agent_id: &str, text: &str, official: bool, done: bool) {
        let sub = self.subs.entry(agent_id.to_string()).or_default();
        let sentence = first_sentence(text);
        if !sentence.is_empty() && (official || !sub.official) {
            sub.result = Some(sentence);
            sub.official |= official;
        }
        sub.done |= done;
        self.adopt(agent_id);
    }

    pub(super) fn sub_started(&mut self, agent_id: &str, agent_type: Option<&str>) {
        let sub = self.subs.entry(agent_id.to_string()).or_default();
        sub.agent_type = agent_type.filter(|t| !t.is_empty()).map(str::to_string).or(sub.agent_type.take());
        if sub.parent.is_none() {
            let turn = self.hk_turn.or(self.turns.len().checked_sub(1));
            let open = turn.and_then(|i| {
                self.turns[i].items.iter().rev().find_map(|item| match item {
                    Item::Tool(t) if self.unclaimed_spawner(t) && t.status.is_open() => Some(t.id.clone()),
                    _ => None,
                })
            });
            if let Some(parent) = open {
                self.link_sub(agent_id, &parent, false);
            }
        }
        self.adopt(agent_id);
    }

    fn link_by_prompt(&mut self, agent_id: &str, prompt: &str) {
        if self.subs.get(agent_id).is_some_and(|s| s.parent.is_some()) {
            return;
        }
        let wanted = truncate_chars(prompt, INPUT_VALUE_MAX);
        let parent = self.turns.iter().rev().flat_map(|t| t.items.iter().rev()).find_map(|item| match item {
            Item::Tool(t)
                if self.unclaimed_spawner(t) && t.detail.input.iter().any(|(k, v)| k == "prompt" && *v == wanted) =>
            {
                Some(t.id.clone())
            }
            _ => None,
        });
        if let Some(parent) = parent {
            self.link_sub(agent_id, &parent, false);
        }
    }

    fn unclaimed_spawner(&self, t: &ToolItem) -> bool {
        SPAWNERS.contains(&t.tool.as_str())
            && t.subagent.is_none()
            && !self.subs.values().any(|s| s.parent.as_deref() == Some(t.id.as_str()))
    }

    /// The parent's result for a Task / Agent call: links the subagent it names and, when the subagent
    /// ran synchronously, takes its report as the result.
    pub(super) fn spawner_result(&mut self, tool_use_id: &str, tool_use_result: Option<&Value>, text: &str) {
        let tool = self.tool_mut(tool_use_id).map(|t| t.tool.clone()).unwrap_or_default();
        if !SPAWNERS.contains(&tool.as_str()) {
            return;
        }
        let named = tool_use_result.and_then(|r| str_field(r, "agentId")).map(str::to_string);
        let Some(agent_id) = named.or_else(|| agent_id_in(text)) else { return };
        self.link_sub(&agent_id, tool_use_id, true);
        let completed = tool_use_result.and_then(|r| str_field(r, "status")) == Some("completed");
        let is_async = tool_use_result.is_some_and(|r| r.get("isAsync").and_then(Value::as_bool) == Some(true))
            || text.starts_with("Async agent launched");
        if completed || (tool_use_result.is_none() && !is_async) {
            let report = tool_use_result.and_then(|r| r.get("content")).map(content_text).unwrap_or_default();
            let report = if report.is_empty() { hand_back(text).to_string() } else { report };
            self.sub_result(&agent_id, &report, true, true);
        }
    }

    /// `<task-notification>` user record: an async subagent finished (or stopped). Claude sends the same
    /// record for `run_in_background` shell commands, so only a known Task / Agent call is linked.
    pub(super) fn task_notification(&mut self, text: &str) {
        let (Some(agent_id), Some(tool_use_id)) = (tag(text, "task-id"), tag(text, "tool-use-id")) else { return };
        if !self.tool_mut(tool_use_id).is_some_and(|t| SPAWNERS.contains(&t.tool.as_str())) {
            return;
        }
        self.link_sub(agent_id, tool_use_id, true);
        // Newer Claude Code hands the report back as a message of its own (see `hand_back_message`) and says
        // so here instead of repeating it: keep that report.
        let handed_back = self.subs.get(agent_id).is_some_and(|s| s.official && s.result.is_some());
        let result = if handed_back { "" } else { tag(text, "result").unwrap_or("") };
        self.sub_result(agent_id, result, true, true);
    }

    /// A background subagent's report, handed back as a message from it (`origin.kind: peer`,
    /// `handback: true`; the report follows a frame, each line indented).
    pub(super) fn hand_back_message(&mut self, agent_id: &str, body: &str) {
        self.sub_result(agent_id, hand_back(body), true, true);
    }
}

/// `agentId: <id>` in a Task / Agent result text.
fn agent_id_in(text: &str) -> Option<String> {
    let rest = &text[text.find("agentId: ")? + "agentId: ".len()..];
    let id: String = rest.chars().take_while(char::is_ascii_alphanumeric).collect();
    (!id.is_empty()).then_some(id)
}

/// The subagent's report inside the parent's tool result (after the hand-back frame, when there is one).
fn hand_back(text: &str) -> &str {
    text.split_once("The report follows:\n").map_or(text, |(_, report)| report)
}

fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let start = text.find(&format!("<{name}>"))? + name.len() + 2;
    let end = text[start..].find(&format!("</{name}>"))? + start;
    Some(text[start..end].trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_tags() {
        let n = "<task-notification>\n<task-id>ab4</task-id>\n<tool-use-id>toolu_1</tool-use-id>\n<result>hi\nthere</result>\n";
        assert_eq!(tag(n, "task-id"), Some("ab4"));
        assert_eq!(tag(n, "result"), Some("hi\nthere"));
        assert_eq!(tag(n, "status"), None);
    }

    #[test]
    fn agent_ids_and_reports() {
        let launched =
            "Async agent launched successfully. (…)\nagentId: ab4d80d3c1c0641ac (internal ID - do not mention)";
        assert_eq!(agent_id_in(launched), Some("ab4d80d3c1c0641ac".into()));
        assert_eq!(agent_id_in("no id"), None);
        let framed = "[Subagent hand-back] The text below is … The report follows:\n  **DONE**\n  \n  Commit: x";
        assert_eq!(hand_back(framed), "  **DONE**\n  \n  Commit: x");
        assert_eq!(hand_back("plain"), "plain");
    }
}
