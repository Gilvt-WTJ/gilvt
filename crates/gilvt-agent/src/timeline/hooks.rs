//! Hooks → timeline (both agents): exact start / end times, anchors, approvals, denials.

use std::time::SystemTime;

use serde_json::Value;

use super::text::{content_text, exit_code};
use super::{complete, new_tool, Anchor, Approval, Item, ItemStatus, Source, Timeline, ToolCall, ToolItem};
use crate::event::{parse_hook, str_field, Event, HookInput};

impl Timeline {
    /// One hook payload (`event` as on the `gilvt hook` command line, "" → payload `hook_event_name`),
    /// received at `at`. `anchor` is the pane's absolute cursor line when the hook arrived; only a session
    /// `PreToolUse` keeps it. Hooks fired inside a subagent (`agent_id`) go to that subagent's rows.
    pub fn apply_hook(&mut self, event: &str, payload: &Value, anchor: Option<Anchor>, at: SystemTime) {
        let event = match event {
            "" => str_field(payload, "hook_event_name").unwrap_or(""),
            event => event,
        };
        let sub = str_field(payload, "agent_id").filter(|a| !a.is_empty());
        let id = str_field(payload, "tool_use_id").unwrap_or("");
        let tool = str_field(payload, "tool_name").unwrap_or("");
        match (event, sub) {
            ("SubagentStart", Some(agent)) => self.sub_started(agent, str_field(payload, "agent_type")),
            ("SubagentStop", Some(agent)) => {
                self.sub_started(agent, str_field(payload, "agent_type"));
                self.sub_result(agent, str_field(payload, "last_assistant_message").unwrap_or(""), false, true);
            }
            ("PreToolUse", Some(agent)) => {
                let call = ToolCall::new(id, tool, payload.get("tool_input").unwrap_or(&Value::Null));
                self.sub_tool(agent, call, None, Some(at));
                if let Some(t) = self.tool_mut(id).filter(|t| t.approval == Approval::Unknown) {
                    t.approval = Approval::NotAsked;
                }
            }
            ("PreToolUse", None) => {
                let call = ToolCall::new(id, tool, payload.get("tool_input").unwrap_or(&Value::Null));
                self.hook_tool_start(call, anchor, at);
            }
            ("PostToolUse", _) => {
                let response = payload.get("tool_response").unwrap_or(&Value::Null);
                let output = hook_output(response);
                self.hook_tool_end(sub, id, tool, at, |t| {
                    if t.status.is_open() {
                        complete(t, ItemStatus::Ok, &output);
                    }
                });
                self.spawner_result(id, Some(response).filter(|r| r.is_object()), &output);
            }
            ("PostToolUseFailure", _) => {
                let error = str_field(payload, "error").unwrap_or("").to_string();
                let interrupted = payload.get("is_interrupt").and_then(Value::as_bool) == Some(true);
                let status =
                    if interrupted { ItemStatus::Interrupted } else { ItemStatus::Failed { exit: exit_code(&error) } };
                self.hook_tool_end(sub, id, tool, at, |t| complete(t, status, &error));
            }
            ("PermissionDenied", _) => {
                let reason = str_field(payload, "reason").unwrap_or("").to_string();
                self.hook_tool_end(sub, id, tool, at, |t| complete(t, ItemStatus::Denied, &reason));
            }
            ("PermissionRequest", Some(agent)) => mark_pending(self.sub_items_mut(agent), tool, at),
            ("PermissionRequest", None) => {
                let turn = self.hook_turn(Some(at));
                mark_pending(&mut self.turns[turn].items, tool, at);
            }
            (_, Some(_)) => {}
            ("UserPromptSubmit", None) => {
                for e in self.adapter_events(event, payload) {
                    if let Event::PromptSubmit { text } = e {
                        self.open_turn(Source::Hook, &text, Some(at));
                    }
                }
            }
            ("Stop", None) => {
                let turn = self.hook_turn(Some(at));
                self.end_turn(turn, Some(at));
            }
            ("StopFailure", None) => {
                for e in self.adapter_events(event, payload) {
                    if let Event::Error { message } = e {
                        let turn = self.hook_turn(Some(at));
                        self.fail_turn(turn, message, Some(at));
                    }
                }
            }
            _ => {}
        }
    }

    fn adapter_events(&self, event: &str, payload: &Value) -> Vec<Event> {
        parse_hook(&HookInput { agent: self.agent, event, payload })
    }

    /// `PreToolUse`: a Running item with the anchor, or the start time / anchor of the one the transcript
    /// already wrote.
    fn hook_tool_start(&mut self, call: ToolCall, anchor: Option<Anchor>, at: SystemTime) {
        if let Some(t) = self.tool_mut(&call.id) {
            t.anchor = t.anchor.or(anchor);
            t.started = Some(at);
            if t.approval == Approval::Unknown {
                t.approval = Approval::NotAsked;
            }
            return;
        }
        let turn = self.hook_turn(Some(at));
        let mut item = new_tool(call);
        item.anchor = anchor;
        item.started = Some(at);
        item.approval = Approval::NotAsked;
        self.push_item(turn, Item::Tool(item));
    }

    /// A hook that ends a call: exact end time, then `update`. A call no `PreToolUse` announced (hooks
    /// installed mid-turn) is added without an anchor (to the subagent's rows for a subagent's hook).
    fn hook_tool_end(
        &mut self,
        sub: Option<&str>,
        id: &str,
        tool: &str,
        at: SystemTime,
        update: impl FnOnce(&mut ToolItem),
    ) {
        if id.is_empty() {
            return;
        }
        if self.tool_mut(id).is_none() {
            let call = ToolCall::new(id, tool, &Value::Null);
            match sub {
                Some(agent) => self.sub_tool(agent, call, None, None),
                None => {
                    let turn = self.hook_turn(Some(at));
                    self.push_item(turn, Item::Tool(new_tool(call)));
                }
            }
        }
        let Some(t) = self.tool_mut(id) else { return };
        t.ended = Some(at);
        update(t);
    }
}

/// `PermissionRequest` names no tool-use id: the latest running call of that tool (else the latest
/// running call) waits for approval.
fn mark_pending(items: &mut [Item], tool: &str, at: SystemTime) {
    let running = |i: &Item, named: bool| match i {
        Item::Tool(t) => t.status == ItemStatus::Running && (!named || t.tool == tool),
        _ => false,
    };
    let pos = items.iter().rposition(|i| running(i, true)).or_else(|| items.iter().rposition(|i| running(i, false)));
    if let Some(Item::Tool(t)) = pos.map(|i| &mut items[i]) {
        t.status = ItemStatus::Pending;
        t.approval = Approval::Asked(at);
    }
}

/// Claude sends structured `tool_response`s (Bash: `{stdout, stderr}`; others vary), Codex a string.
fn hook_output(response: &Value) -> String {
    match response {
        Value::String(s) => s.clone(),
        Value::Object(_) => {
            let out = [str_field(response, "stdout"), str_field(response, "stderr")];
            let joined: Vec<&str> = out.into_iter().flatten().filter(|s| !s.is_empty()).collect();
            if joined.is_empty() {
                content_text(response.get("content").unwrap_or(&Value::Null))
            } else {
                joined.join("\n")
            }
        }
        other => content_text(other),
    }
}
