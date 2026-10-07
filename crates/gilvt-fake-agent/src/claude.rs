//! Claude Code's formats: `~/.claude/projects/<encoded cwd>/<session>.jsonl` records and hook payloads,
//! shaped after `gilvt-agent/tests/fixtures/claude` (recorded from Claude Code 2.1.2xx).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{json, Map, Value as Json};

use crate::clock::{anthropic_id, iso_utc, uuid_v4};
use crate::recorder::{EndReason, Outcome, Output, Recorder, ToolRef};
use crate::scenario::TodoItem;
use crate::CLAUDE_VERSION;

/// The model the fake claims to be.
pub const MODEL: &str = "claude-sonnet-4-5-20250929";
/// Model name Claude Code writes on locally made messages (API errors).
const SYNTHETIC_MODEL: &str = "<synthetic>";
/// The tool result Claude Code writes when the user rejects a call (or presses Esc while it runs).
pub const REJECTED: &str = "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";
const TODOS_OK: &str = "Todos have been modified successfully. Ensure that you continue to use the todo list to track your progress. Please proceed with the current tasks if applicable";

/// Claude Code's project directory name for `cwd`: every char that is not an ASCII letter or digit
/// becomes `-` (two for chars outside the BMP: JavaScript counts UTF-16 units).
pub fn project_dir_name(cwd: &Path) -> String {
    cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c.to_string() } else { "-".repeat(c.len_utf16()) }).collect()
}

/// `<home>/.claude/projects/<encoded cwd>/<session>.jsonl`
pub fn transcript_path(home: &Path, cwd: &Path, session_id: &str) -> PathBuf {
    home.join(".claude/projects").join(project_dir_name(cwd)).join(format!("{session_id}.jsonl"))
}

pub struct ClaudeSession {
    session_id: String,
    cwd: PathBuf,
    out: Output,
    /// The last record's uuid (`parentUuid` of the next one).
    parent: Option<String>,
    prompt_id: Option<String>,
    turn_started: Option<SystemTime>,
    turn_messages: usize,
    last_reply: String,
    /// Grows with every reply, like a real session's cached context.
    context: u64,
    todos: Json,
}

impl ClaudeSession {
    /// A session writing to `out` (see [`transcript_path`]). An existing transcript (a resumed session) is
    /// appended to, its last record continuing the `parentUuid` chain.
    pub fn new(session_id: String, cwd: PathBuf, out: Output) -> ClaudeSession {
        let parent = std::fs::read_to_string(out.path()).ok().and_then(|text| {
            text.lines().rev().find_map(|l| serde_json::from_str::<Json>(l).ok()?.get("uuid")?.as_str().map(str::to_string))
        });
        ClaudeSession {
            session_id,
            cwd,
            out,
            parent,
            prompt_id: None,
            turn_started: None,
            turn_messages: 0,
            last_reply: String::new(),
            context: 18_000,
            todos: json!([]),
        }
    }

    fn cwd_str(&self) -> String {
        self.cwd.display().to_string()
    }

    /// A record with the fields every transcript line has; `extra` is merged in.
    fn record(&mut self, t: SystemTime, kind: &str, extra: Json) -> String {
        let uuid = uuid_v4();
        let mut r = Map::new();
        r.insert("parentUuid".into(), self.parent.clone().map_or(Json::Null, Json::String));
        r.insert("isSidechain".into(), false.into());
        r.insert("type".into(), kind.into());
        r.insert("uuid".into(), uuid.clone().into());
        r.insert("timestamp".into(), iso_utc(t).into());
        r.insert("userType".into(), "external".into());
        r.insert("entrypoint".into(), "cli".into());
        r.insert("cwd".into(), self.cwd_str().into());
        r.insert("sessionId".into(), self.session_id.clone().into());
        r.insert("version".into(), CLAUDE_VERSION.into());
        r.insert("gitBranch".into(), "HEAD".into());
        if let Json::Object(extra) = extra {
            r.extend(extra);
        }
        self.out.line(&Json::Object(r));
        self.parent = Some(uuid.clone());
        self.turn_messages += 1;
        uuid
    }

    fn user_record(&mut self, t: SystemTime, extra: Json) -> String {
        let mut extra = extra;
        if let Some(p) = &self.prompt_id {
            extra["promptId"] = p.clone().into();
        }
        self.record(t, "user", extra)
    }

    /// An assistant message with one content block.
    fn assistant(&mut self, t: SystemTime, block: Json, stop_reason: &str) -> String {
        self.context += 1_500;
        let usage = json!({
            "input_tokens": 10, "cache_creation_input_tokens": 1_500, "cache_read_input_tokens": self.context,
            "output_tokens": 120, "service_tier": "standard",
        });
        let message = json!({
            "model": MODEL, "id": anthropic_id("msg_", 22), "type": "message", "role": "assistant",
            "content": [block], "stop_reason": stop_reason, "stop_sequence": null, "usage": usage,
        });
        self.record(t, "assistant", json!({"message": message, "requestId": anthropic_id("req_", 22)}))
    }

    /// Hook payload fields every event has; `in_turn` adds the prompt id and permission mode.
    fn payload(&self, event: &str, in_turn: bool, extra: Json) -> Json {
        let mut p = Map::new();
        p.insert("session_id".into(), self.session_id.clone().into());
        p.insert("transcript_path".into(), self.out.path().display().to_string().into());
        p.insert("cwd".into(), self.cwd_str().into());
        p.insert("scratchpad_dir".into(), scratchpad_dir(&self.cwd, &self.session_id).into());
        if let Some(prompt_id) = &self.prompt_id {
            p.insert("prompt_id".into(), prompt_id.clone().into());
        }
        if in_turn {
            p.insert("permission_mode".into(), "default".into());
        }
        p.insert("hook_event_name".into(), event.into());
        if let Json::Object(extra) = extra {
            p.extend(extra);
        }
        Json::Object(p)
    }

    fn hook(&mut self, event: &str, subject: Option<&str>, in_turn: bool, extra: Json) {
        let payload = self.payload(event, in_turn, extra);
        self.out.hook(event, subject, payload);
    }

    fn result_record(&mut self, t: SystemTime, call: &ToolRef, content: &str, is_error: bool, extra: Json) {
        let mut block = json!({"tool_use_id": call.id, "type": "tool_result", "content": content});
        if is_error {
            block["is_error"] = true.into();
        }
        let mut r = json!({"message": {"role": "user", "content": [block]}, "sourceToolAssistantUUID": call.record_uuid});
        if let (Json::Object(r), Json::Object(extra)) = (&mut r, extra) {
            r.extend(extra);
        }
        self.user_record(t, r);
    }

    fn duration_ms(t: SystemTime, since: SystemTime) -> u64 {
        t.duration_since(since).map_or(0, |d| d.as_millis() as u64)
    }
}

/// `/private/tmp/claude-<uid>/<encoded cwd>/<session>/scratchpad`, as the interactive CLI names it in every
/// payload (only named, never created).
fn scratchpad_dir(cwd: &Path, session_id: &str) -> String {
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    format!("/private/tmp/claude-{uid}/{}/{session_id}/scratchpad", project_dir_name(cwd))
}

/// What Claude Code stores as `toolUseResult` (and sends as the hook's `tool_response`) for a call that
/// returned `output`.
fn tool_use_result(tool: &str, input: &Json, output: &str) -> Json {
    let s = |k: &str| input.get(k).and_then(Json::as_str).unwrap_or("");
    match tool {
        "Bash" => json!({"stdout": output, "stderr": "", "interrupted": false, "isImage": false}),
        "Read" => {
            let lines = output.lines().count();
            json!({"type": "text", "file": {"filePath": s("file_path"), "content": output, "numLines": lines, "startLine": 1, "totalLines": lines}})
        }
        "Write" => json!({"type": "create", "filePath": s("file_path"), "content": s("content"), "structuredPatch": [], "originalFile": null}),
        "Edit" => json!({
            "filePath": s("file_path"), "oldString": s("old_string"), "newString": s("new_string"), "originalFile": "",
            "structuredPatch": [], "userModified": false, "replaceAll": input.get("replace_all").and_then(Json::as_bool).unwrap_or(false),
        }),
        _ => Json::String(output.to_string()),
    }
}

impl Recorder for ClaudeSession {
    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn out(&mut self) -> &mut Output {
        &mut self.out
    }

    fn session_start(&mut self, _t: SystemTime, resumed: bool) {
        let source = if resumed { "resume" } else { "startup" };
        self.hook("SessionStart", Some(source), false, json!({"source": source, "model": MODEL}));
    }

    fn prompt(&mut self, t: SystemTime, text: &str) {
        self.prompt_id = Some(uuid_v4());
        self.turn_started = Some(t);
        self.turn_messages = 0;
        self.last_reply.clear();
        self.hook("UserPromptSubmit", None, true, json!({"prompt": text}));
        let record = json!({
            "message": {"role": "user", "content": text}, "permissionMode": "default",
            "origin": {"kind": "human"}, "promptSource": "typed",
        });
        self.user_record(t, record);
    }

    fn tool_start(&mut self, t: SystemTime, tool: &str, input: &Json) -> ToolRef {
        let id = anthropic_id("toolu_", 22);
        let block = json!({"type": "tool_use", "id": id, "name": tool, "input": input, "caller": {"type": "direct"}});
        let record_uuid = self.assistant(t, block, "tool_use");
        self.hook("PreToolUse", Some(tool), true, json!({"tool_name": tool, "tool_input": input, "tool_use_id": id}));
        ToolRef { id, tool: tool.to_string(), input: input.clone(), started: t, record_uuid }
    }

    fn permission_request(&mut self, _t: SystemTime, call: &ToolRef) {
        let suggestions = json!([{"type": "setMode", "mode": "acceptEdits", "destination": "session"}]);
        let extra = json!({"tool_name": call.tool, "tool_input": call.input, "permission_suggestions": suggestions});
        self.hook("PermissionRequest", Some(&call.tool), true, extra);
        let notification = json!({"message": "Claude needs your permission", "notification_type": "permission_prompt"});
        self.hook("Notification", Some("permission_prompt"), false, notification);
    }

    fn question(&mut self, t: SystemTime, question: &str, header: &str, options: &[String]) -> ToolRef {
        let options: Vec<Json> = options.iter().map(|o| json!({"label": o, "description": ""})).collect();
        let input = json!({"questions": [{"question": question, "header": header, "options": options, "multiSelect": false}]});
        let call = self.tool_start(t, "AskUserQuestion", &input);
        self.permission_request(t, &call);
        call
    }

    fn tool_end(&mut self, t: SystemTime, call: &ToolRef, outcome: Outcome) {
        let duration_ms = Self::duration_ms(t, call.started);
        let base = json!({"tool_name": call.tool, "tool_input": call.input});
        let with = |extra: Json| {
            let mut v = base.clone();
            if let (Json::Object(v), Json::Object(extra)) = (&mut v, extra) {
                v.extend(extra);
            }
            v
        };
        match outcome {
            Outcome::Ok(output) => {
                let result = if call.tool == "TodoWrite" {
                    let new = call.input.get("todos").cloned().unwrap_or(json!([]));
                    json!({"oldTodos": std::mem::replace(&mut self.todos, new.clone()), "newTodos": new})
                } else {
                    tool_use_result(&call.tool, &call.input, output)
                };
                let extra = with(json!({"tool_response": result, "tool_use_id": call.id, "duration_ms": duration_ms}));
                self.hook("PostToolUse", Some(&call.tool), true, extra);
                self.result_record(t, call, output, false, json!({"toolUseResult": result}));
            }
            Outcome::Failed { exit, output } => {
                let text = format!("Exit code {exit}\n{output}");
                let extra = with(json!({"tool_use_id": call.id, "error": text, "is_interrupt": false}));
                self.hook("PostToolUseFailure", Some(&call.tool), true, extra);
                self.result_record(t, call, &text, true, json!({"toolUseResult": format!("Error: {text}")}));
            }
            // No hook: Claude Code sends none for a rejection (gilvt learns it from the transcript).
            Outcome::Rejected | Outcome::Interrupted => {
                let extra = json!({"toolUseResult": "User rejected tool use", "toolDenialKind": "user-rejected"});
                self.result_record(t, call, REJECTED, true, extra);
            }
            Outcome::Answered { question, answer } => {
                let questions = call.input.get("questions").cloned().unwrap_or(json!([]));
                let mut answers = Map::new();
                answers.insert(question.to_string(), answer.into());
                let result = json!({"questions": questions, "answers": answers});
                let text = format!(
                    "User has answered your questions: \"{question}\"=\"{answer}\". You can now continue with the user's answers in mind."
                );
                let extra = with(json!({"tool_response": result, "tool_use_id": call.id, "duration_ms": duration_ms}));
                self.hook("PostToolUse", Some(&call.tool), true, extra);
                self.result_record(t, call, &text, false, json!({"toolUseResult": result}));
            }
        }
    }

    fn todo(&mut self, t: SystemTime, items: &[TodoItem]) {
        let todos: Vec<Json> = items.iter().map(|i| json!({"content": i.text, "status": i.status, "activeForm": i.text})).collect();
        let call = self.tool_start(t, "TodoWrite", &json!({"todos": todos}));
        self.tool_end(t, &call, Outcome::Ok(TODOS_OK));
    }

    fn thinking(&mut self, t: SystemTime, text: &str) {
        // A turn's thinking is followed by more of it (a tool call or the reply), like the records Claude
        // Code writes before a tool use: `end_turn` here would end gilvt's turn early.
        let block = json!({"type": "thinking", "thinking": text, "signature": anthropic_id("", 64)});
        self.assistant(t, block, "tool_use");
    }

    fn reply(&mut self, t: SystemTime, text: &str, last: bool) {
        self.last_reply = text.to_string();
        self.assistant(t, json!({"type": "text", "text": text}), if last { "end_turn" } else { "tool_use" });
    }

    fn api_error(&mut self, t: SystemTime, message: &str) {
        let usage = json!({"input_tokens": 0, "output_tokens": 0, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0});
        let msg = json!({
            "id": uuid_v4(), "model": SYNTHETIC_MODEL, "role": "assistant", "stop_reason": "stop_sequence", "stop_sequence": "",
            "type": "message", "usage": usage, "content": [{"type": "text", "text": message}],
        });
        self.record(t, "assistant", json!({"message": msg, "error": "unknown", "isApiErrorMessage": true}));
        let extra = json!({"error": "unknown", "error_details": message, "last_assistant_message": message});
        self.hook("StopFailure", None, true, extra);
        self.turn_started = None;
    }

    fn interrupt(&mut self, t: SystemTime, for_tool: bool) {
        let text = if for_tool { "[Request interrupted by user for tool use]" } else { "[Request interrupted by user]" };
        self.user_record(t, json!({"message": {"role": "user", "content": [{"type": "text", "text": text}]}}));
        self.turn_duration(t);
    }

    fn turn_end(&mut self, t: SystemTime) {
        let extra = json!({
            "stop_hook_active": false, "last_assistant_message": self.last_reply,
            "background_tasks": [], "session_crons": [],
        });
        self.hook("Stop", None, true, extra);
        self.turn_duration(t);
    }

    fn session_end(&mut self, _t: SystemTime, reason: EndReason) {
        let reason = match reason {
            EndReason::PromptInputExit => "prompt_input_exit",
            EndReason::Other => "other",
        };
        self.hook("SessionEnd", None, false, json!({"reason": reason}));
    }

    fn resume_hint(&self) -> String {
        format!("Resume this session with:\nclaude --resume {}", self.session_id)
    }
}

impl ClaudeSession {
    /// `system` / `turn_duration`: Claude Code's end-of-turn record.
    fn turn_duration(&mut self, t: SystemTime) {
        let Some(started) = self.turn_started.take() else { return };
        let duration = Self::duration_ms(t, started);
        let messages = self.turn_messages + 1;
        self.record(t, "system", json!({"subtype": "turn_duration", "durationMs": duration, "messageCount": messages, "isMeta": false}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_dir_names() {
        assert_eq!(project_dir_name(Path::new("/Users/u/proj")), "-Users-u-proj");
        assert_eq!(project_dir_name(Path::new("/tmp/a b_c.d")), "-tmp-a-b-c-d");
        assert_eq!(project_dir_name(Path::new("/x/项目")), "-x---");
        assert_eq!(project_dir_name(Path::new("/x/🐱")), "-x---");
        assert_eq!(
            transcript_path(Path::new("/h"), Path::new("/w/p"), "s1"),
            PathBuf::from("/h/.claude/projects/-w-p/s1.jsonl")
        );
    }

    #[test]
    fn a_resumed_transcript_continues_the_uuid_chain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "{\"type\":\"user\",\"uuid\":\"u-1\"}\n{\"type\":\"last-prompt\"}\n").unwrap();
        let mut s = ClaudeSession::new("s".into(), "/w".into(), Output::new(path.clone(), crate::hooks::Hooks::none(), false));
        s.prompt(SystemTime::now(), "again");
        let text = std::fs::read_to_string(&path).unwrap();
        let last: Json = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(last["parentUuid"], "u-1");
        assert_eq!(last["message"]["content"], "again");
        assert_eq!(text.lines().count(), 3, "appended");
    }
}
