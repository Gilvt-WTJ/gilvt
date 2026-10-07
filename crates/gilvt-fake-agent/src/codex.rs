//! Codex's formats: `~/.codex/sessions/YYYY/MM/DD/rollout-<local time>-<id>.jsonl` records and hook
//! payloads, shaped after `gilvt-agent/tests/fixtures/codex` (recorded from codex-cli 0.145.0).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{json, Map, Value as Json};

use crate::clock::{iso_utc, local_parts, random_bytes, unix_secs, uuid_v7};
use crate::recorder::{EndReason, Outcome, Output, Recorder, ToolRef};
use crate::scenario::TodoItem;
use crate::CODEX_VERSION;

pub const MODEL: &str = "gpt-5.6-sol";
pub const CONTEXT_WINDOW: u64 = 258_400;

/// `<home>/.codex/sessions/YYYY/MM/DD/rollout-YYYY-MM-DDTHH-MM-SS-<id>.jsonl`, in local time like Codex.
pub fn rollout_path(home: &Path, t: SystemTime, session_id: &str) -> PathBuf {
    let (y, mo, d, h, mi, s) = local_parts(t);
    home.join(format!(".codex/sessions/{y:04}/{mo:02}/{d:02}"))
        .join(format!("rollout-{y:04}-{mo:02}-{d:02}T{h:02}-{mi:02}-{s:02}-{session_id}.jsonl"))
}

/// The existing rollout of session `session_id` under `<home>/.codex/sessions`, if any.
pub fn find_rollout(home: &Path, session_id: &str) -> Option<PathBuf> {
    let suffix = format!("-{session_id}.jsonl");
    let dirs = |p: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.path()).collect();
        v.sort();
        v
    };
    let sessions = home.join(".codex/sessions");
    for year in dirs(&sessions).iter().rev() {
        for month in dirs(year).iter().rev() {
            for day in dirs(month).iter().rev() {
                let found = dirs(day).into_iter().find(|f| f.file_name().is_some_and(|n| n.to_string_lossy().ends_with(&suffix)));
                if found.is_some() {
                    return found;
                }
            }
        }
    }
    None
}

pub struct CodexSession {
    session_id: String,
    cwd: PathBuf,
    out: Output,
    /// Whether the rollout already exists (a resumed session): no new `session_meta`.
    existing: bool,
    turn_id: Option<String>,
    turn_started: Option<SystemTime>,
    prompt: String,
    last_reply: Option<String>,
    total_tokens: u64,
}

impl CodexSession {
    pub fn new(session_id: String, cwd: PathBuf, out: Output) -> CodexSession {
        let existing = out.path().is_file();
        CodexSession { session_id, cwd, out, existing, turn_id: None, turn_started: None, prompt: String::new(), last_reply: None, total_tokens: 0 }
    }

    fn line(&mut self, t: SystemTime, kind: &str, payload: Json) {
        self.out.line(&json!({"timestamp": iso_utc(t), "type": kind, "payload": payload}));
    }

    /// Hook payload: identity, then the turn (when in one), then `extra`.
    fn payload(&self, event: &str, extra: Json) -> Json {
        let mut p = Map::new();
        p.insert("session_id".into(), self.session_id.clone().into());
        if let Some(turn) = &self.turn_id {
            p.insert("turn_id".into(), turn.clone().into());
        }
        p.insert("transcript_path".into(), self.out.path().display().to_string().into());
        p.insert("cwd".into(), self.cwd.display().to_string().into());
        p.insert("hook_event_name".into(), event.into());
        if event != "SessionEnd" {
            p.insert("model".into(), MODEL.into());
            p.insert("permission_mode".into(), "default".into());
        }
        if let Json::Object(extra) = extra {
            p.extend(extra);
        }
        Json::Object(p)
    }

    fn hook(&mut self, event: &str, subject: Option<&str>, extra: Json) {
        let payload = self.payload(event, extra);
        self.out.hook(event, subject, payload);
    }

    /// Start, end and duration fields of the current turn's final event.
    fn turn_times(&self, t: SystemTime) -> Json {
        let started = self.turn_started.unwrap_or(t);
        json!({
            "turn_id": self.turn_id, "started_at": unix_secs(started), "completed_at": unix_secs(t),
            "duration_ms": t.duration_since(started).map_or(0, |d| d.as_millis() as u64),
        })
    }

    fn turn_event(&mut self, t: SystemTime, kind: &str, extra: Json) {
        let mut p = json!({"type": kind});
        if let (Json::Object(p), Json::Object(times), Json::Object(extra)) = (&mut p, self.turn_times(t), extra) {
            p.extend(extra);
            p.extend(times);
        }
        self.line(t, "event_msg", p);
        self.turn_id = None;
        self.turn_started = None;
    }
}

/// The rollout's view of a scenario tool: (response item type, name, arguments / input text), and the
/// hook's (tool_name, tool_input). Codex's shell tool is `exec_command {cmd}` in the rollout but
/// `Bash {command}` in hooks; `apply_patch` is a custom tool whose input is the patch text.
fn codex_call(tool: &str, input: &Json) -> (&'static str, String, String) {
    let command = input.get("command").and_then(Json::as_str).unwrap_or("").to_string();
    match tool {
        "Bash" => ("function_call", "exec_command".into(), json!({"cmd": command}).to_string()),
        "apply_patch" => ("custom_tool_call", "apply_patch".into(), command),
        _ => ("function_call", tool.to_string(), input.to_string()),
    }
}

fn call_id() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let id: String = random_bytes(24).iter().map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char).collect();
    format!("call_{id}")
}

fn chunk_id() -> String {
    random_bytes(3).iter().map(|b| format!("{b:02x}")).collect()
}

impl Recorder for CodexSession {
    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn out(&mut self) -> &mut Output {
        &mut self.out
    }

    fn session_start(&mut self, t: SystemTime, resumed: bool) {
        if !self.existing {
            let meta = json!({
                "session_id": self.session_id, "id": self.session_id, "timestamp": iso_utc(t),
                "cwd": self.cwd.display().to_string(), "originator": "codex-tui", "cli_version": CODEX_VERSION,
                "source": "cli", "thread_source": "user", "model_provider": "openai", "base_instructions": "<omitted>",
            });
            self.line(t, "session_meta", meta);
        }
        let source = if resumed { "resume" } else { "startup" };
        self.hook("SessionStart", Some(source), json!({"source": source}));
    }

    fn prompt(&mut self, t: SystemTime, text: &str) {
        let turn = uuid_v7(t);
        self.turn_id = Some(turn.clone());
        self.turn_started = Some(t);
        self.prompt = text.to_string();
        self.last_reply = None;
        self.hook("UserPromptSubmit", None, json!({"prompt": text}));
        let started = json!({
            "type": "task_started", "turn_id": turn, "started_at": unix_secs(t), "model_context_window": CONTEXT_WINDOW,
            "collaboration_mode_kind": "default",
        });
        self.line(t, "event_msg", started);
        let cwd = self.cwd.display().to_string();
        let context = json!({
            "turn_id": turn, "cwd": cwd, "workspace_roots": [cwd], "approval_policy": "on-request",
            "approvals_reviewer": "user", "model": MODEL, "personality": "pragmatic", "summary": "auto",
        });
        self.line(t, "turn_context", context);
        let meta = json!({"turn_id": turn});
        let message = json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}],
            "internal_chat_message_metadata_passthrough": meta});
        self.line(t, "response_item", message);
        let user = json!({"type": "user_message", "message": text, "images": [], "local_images": [], "audio": [],
            "local_audio": [], "text_elements": []});
        self.line(t, "event_msg", user);
    }

    fn tool_start(&mut self, t: SystemTime, tool: &str, input: &Json) -> ToolRef {
        let id = call_id();
        let (kind, name, args) = codex_call(tool, input);
        let meta = json!({"turn_id": self.turn_id});
        let item = if kind == "custom_tool_call" {
            json!({"type": kind, "name": name, "input": args, "call_id": id, "internal_chat_message_metadata_passthrough": meta})
        } else {
            json!({"type": kind, "name": name, "arguments": args, "call_id": id, "internal_chat_message_metadata_passthrough": meta})
        };
        self.line(t, "response_item", item);
        self.hook("PreToolUse", Some(tool), json!({"tool_name": tool, "tool_input": input, "tool_use_id": id}));
        ToolRef { id, tool: tool.to_string(), input: input.clone(), started: t, record_uuid: String::new() }
    }

    fn permission_request(&mut self, _t: SystemTime, call: &ToolRef) {
        self.hook("PermissionRequest", Some(&call.tool), json!({"tool_name": call.tool, "tool_input": call.input}));
    }

    fn question(&mut self, t: SystemTime, question: &str, _header: &str, options: &[String]) -> ToolRef {
        // Codex has no AskUserQuestion (scenarios reject `ask` for codex); a plain tool call if it happens.
        self.tool_start(t, "request_user_input", &json!({"question": question, "options": options}))
    }

    fn tool_end(&mut self, t: SystemTime, call: &ToolRef, outcome: Outcome) {
        let secs = t.duration_since(call.started).map_or(0.0, |d| d.as_secs_f64());
        let patch = call.tool == "apply_patch";
        let (output, response, exit) = match outcome {
            Outcome::Ok(out) => (out.to_string(), Some(out.to_string()), 0),
            Outcome::Failed { exit, output } => (output.to_string(), Some(output.to_string()), exit),
            Outcome::Answered { answer, .. } => (answer.to_string(), Some(answer.to_string()), 0),
            Outcome::Rejected => ("rejected by user".to_string(), None, -1),
            Outcome::Interrupted => (format!("aborted by user after {secs:.1}s"), None, -1),
        };
        if let Some(response) = &response {
            let extra = json!({"tool_name": call.tool, "tool_input": call.input, "tool_response": response, "tool_use_id": call.id});
            self.hook("PostToolUse", Some(&call.tool), extra);
        }
        let meta = json!({"turn_id": self.turn_id});
        let text = match (response.is_some(), call.tool.as_str()) {
            (true, "Bash") => format!("Chunk ID: {}\nWall time: {secs:.4} seconds\nProcess exited with code {exit}\nOutput:\n{output}", chunk_id()),
            (true, "apply_patch") => format!("Exit code: {exit}\nWall time: {secs:.1} seconds\nOutput:\n{output}"),
            _ => output.clone(),
        };
        if patch && response.is_some() {
            let end = json!({"type": "patch_apply_end", "call_id": call.id, "turn_id": self.turn_id, "stdout": output, "stderr": "",
                "success": exit == 0, "changes": {}, "status": "completed"});
            self.line(t, "event_msg", end);
        }
        let kind = if patch { "custom_tool_call_output" } else { "function_call_output" };
        self.line(t, "response_item", json!({"type": kind, "call_id": call.id, "output": text, "internal_chat_message_metadata_passthrough": meta}));
    }

    fn todo(&mut self, t: SystemTime, items: &[TodoItem]) {
        let plan: Vec<Json> = items.iter().map(|i| json!({"step": i.text, "status": i.status})).collect();
        let call = self.tool_start(t, "update_plan", &json!({"plan": plan}));
        self.tool_end(t, &call, Outcome::Ok("Plan updated"));
    }

    fn thinking(&mut self, t: SystemTime, text: &str) {
        self.line(t, "event_msg", json!({"type": "agent_reasoning", "text": text}));
        let hex: String = random_bytes(24).iter().map(|b| format!("{b:02x}")).collect();
        let item = json!({
            "type": "reasoning", "id": format!("rs_{hex}"), "summary": [{"type": "summary_text", "text": text}],
            "encrypted_content": "<fake>", "internal_chat_message_metadata_passthrough": {"turn_id": self.turn_id},
        });
        self.line(t, "response_item", item);
    }

    fn reply(&mut self, t: SystemTime, text: &str, last: bool) {
        self.last_reply = Some(text.to_string());
        let phase = if last { "final_answer" } else { "commentary" };
        self.line(t, "event_msg", json!({"type": "agent_message", "message": text, "phase": phase, "memory_citation": null}));
        let meta = json!({"turn_id": self.turn_id});
        let message = json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}],
            "internal_chat_message_metadata_passthrough": meta});
        self.line(t, "response_item", message);
        let last_usage = json!({"input_tokens": 1_234, "cached_input_tokens": 1_000, "cache_write_input_tokens": 0,
            "output_tokens": 56, "reasoning_output_tokens": 7, "total_tokens": 1_290});
        self.total_tokens += 1_290;
        let mut total = last_usage.clone();
        total["total_tokens"] = self.total_tokens.into();
        let info = json!({"total_token_usage": total, "last_token_usage": last_usage, "model_context_window": CONTEXT_WINDOW});
        self.line(t, "event_msg", json!({"type": "token_count", "info": info, "rate_limits": null}));
    }

    fn api_error(&mut self, t: SystemTime, message: &str) {
        self.line(t, "event_msg", json!({"type": "error", "message": message}));
        self.turn_event(t, "task_complete", json!({"last_agent_message": null, "error": {"message": message}}));
    }

    fn interrupt(&mut self, t: SystemTime, _for_tool: bool) {
        self.turn_event(t, "turn_aborted", json!({"reason": "interrupted"}));
    }

    fn turn_end(&mut self, t: SystemTime) {
        let (turn, last) = (self.turn_id.clone(), self.last_reply.clone());
        self.turn_event(t, "task_complete", json!({"last_agent_message": last}));
        // The Stop payload still names the turn that just ended.
        self.turn_id = turn.clone();
        self.hook("Stop", None, json!({"stop_hook_active": false, "last_assistant_message": last}));
        self.turn_id = None;
        let notify = json!({"type": "agent-turn-complete", "thread-id": self.session_id, "turn-id": turn,
            "cwd": self.cwd.display().to_string(), "input-messages": [self.prompt], "last-assistant-message": last});
        self.out.notify(&notify);
    }

    fn session_end(&mut self, _t: SystemTime, _reason: EndReason) {
        self.hook("SessionEnd", None, json!({"reason": "other"}));
    }

    fn resume_hint(&self) -> String {
        format!("To continue this session, run codex resume {}", self.session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn rollout_paths_are_dated_in_local_time() {
        let t = UNIX_EPOCH + Duration::from_secs(1_790_297_716);
        let (y, mo, d, h, mi, s) = local_parts(t);
        let p = rollout_path(Path::new("/h"), t, "01a0-x");
        let want = format!("/h/.codex/sessions/{y:04}/{mo:02}/{d:02}/rollout-{y:04}-{mo:02}-{d:02}T{h:02}-{mi:02}-{s:02}-01a0-x.jsonl");
        assert_eq!(p, PathBuf::from(want));
    }

    #[test]
    fn finds_an_existing_rollout() {
        let dir = tempfile::tempdir().unwrap();
        let p = rollout_path(dir.path(), SystemTime::now(), "01a0d60f-3618-7973-8440-269629a63e87");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "{}\n").unwrap();
        assert_eq!(find_rollout(dir.path(), "01a0d60f-3618-7973-8440-269629a63e87"), Some(p));
        assert_eq!(find_rollout(dir.path(), "other"), None);
    }

    #[test]
    fn shell_calls_are_exec_command_in_the_rollout() {
        assert_eq!(codex_call("Bash", &json!({"command": "ls"})), ("function_call", "exec_command".into(), "{\"cmd\":\"ls\"}".into()));
        assert_eq!(codex_call("apply_patch", &json!({"command": "*** Begin Patch"})).0, "custom_tool_call");
        assert_eq!(codex_call("update_plan", &json!({"plan": []})).1, "update_plan");
    }
}
