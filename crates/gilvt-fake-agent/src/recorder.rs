//! What the engine asks of an agent format ([`crate::claude`] / [`crate::codex`]): records in the
//! transcript / rollout file and hook payloads, in the order the real CLI produces them.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value as Json;

use crate::hooks::Hooks;
use crate::scenario::TodoItem;

/// One thing the session produced, in order (tests replay it into gilvt's parsers).
#[derive(Clone, Debug, PartialEq)]
pub enum Trace {
    /// A transcript / rollout line.
    Line(String),
    /// A hook fired (whether or not a command was configured for it).
    Hook { event: String, payload: Json },
    /// The user answered an approval dialog in the pane (gilvt sees the key press).
    ApprovalAnswered,
}

/// The session's files and hooks.
pub struct Output {
    path: PathBuf,
    file: Option<File>,
    hooks: Hooks,
    /// Lite mode: no hooks at all.
    lite: bool,
    trace: Option<Vec<Trace>>,
}

impl Output {
    pub fn new(path: PathBuf, hooks: Hooks, lite: bool) -> Output {
        Output { path, file: None, hooks, lite, trace: None }
    }

    /// Keeps a [`Trace`] of everything from now on.
    pub fn with_trace(mut self) -> Output {
        self.trace = Some(Vec::new());
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn trace(&self) -> &[Trace] {
        self.trace.as_deref().unwrap_or_default()
    }

    /// Appends one record (the file and its directory are created at the first one, like the real CLIs).
    pub fn line(&mut self, record: &Json) {
        let text = serde_json::to_string(record).expect("json");
        if self.file.is_none() {
            if let Some(dir) = self.path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            self.file = OpenOptions::new().create(true).append(true).open(&self.path).ok();
        }
        if let Some(f) = self.file.as_mut() {
            let _ = f.write_all(format!("{text}\n").as_bytes());
            let _ = f.flush();
        }
        if let Some(t) = self.trace.as_mut() {
            t.push(Trace::Line(text));
        }
    }

    /// Fires `event` (nothing in lite mode). `subject` is what hook matchers test.
    pub fn hook(&mut self, event: &str, subject: Option<&str>, payload: Json) {
        if self.lite {
            return;
        }
        self.hooks.run(event, subject, &payload);
        if let Some(t) = self.trace.as_mut() {
            t.push(Trace::Hook { event: event.to_string(), payload });
        }
    }

    /// Codex `notify` (nothing in lite mode).
    pub fn notify(&mut self, payload: &Json) {
        if !self.lite {
            self.hooks.notify(payload);
        }
    }

    pub fn approval_answered(&mut self) {
        if let Some(t) = self.trace.as_mut() {
            t.push(Trace::ApprovalAnswered);
        }
    }
}

/// A tool call in flight.
#[derive(Clone, Debug)]
pub struct ToolRef {
    pub id: String,
    pub tool: String,
    pub input: Json,
    pub started: SystemTime,
    /// Claude: the uuid of the assistant record with the `tool_use` block.
    pub record_uuid: String,
}

/// How a tool call ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome<'a> {
    Ok(&'a str),
    Failed { exit: i32, output: &'a str },
    /// The user said no in the approval dialog (or dismissed a question).
    Rejected,
    /// The user pressed Esc while the call ran.
    Interrupted,
    /// An AskUserQuestion answer.
    Answered { question: &'a str, answer: &'a str },
}

/// Why the session ends (Claude's SessionEnd `reason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// `/exit`, Ctrl-D, Ctrl-C twice.
    PromptInputExit,
    Other,
}

pub trait Recorder {
    fn session_id(&self) -> &str;
    fn out(&mut self) -> &mut Output;
    /// SessionStart (`resumed`: `--resume` / `resume`).
    fn session_start(&mut self, t: SystemTime, resumed: bool);
    /// A new turn.
    fn prompt(&mut self, t: SystemTime, text: &str);
    /// The model calls a tool (PreToolUse).
    fn tool_start(&mut self, t: SystemTime, tool: &str, input: &Json) -> ToolRef;
    /// The call needs approval: the dialog is showing.
    fn permission_request(&mut self, t: SystemTime, call: &ToolRef);
    /// AskUserQuestion (Claude): the question is showing.
    fn question(&mut self, t: SystemTime, question: &str, header: &str, options: &[String]) -> ToolRef;
    fn tool_end(&mut self, t: SystemTime, call: &ToolRef, outcome: Outcome);
    fn todo(&mut self, t: SystemTime, items: &[TodoItem]);
    /// A thinking block (Claude) / reasoning summary (Codex).
    fn thinking(&mut self, t: SystemTime, text: &str);
    /// An assistant message; `last`: the turn's final one (nothing but the turn end follows).
    fn reply(&mut self, t: SystemTime, text: &str, last: bool);
    /// The turn fails (and ends).
    fn api_error(&mut self, t: SystemTime, message: &str);
    /// The user interrupted the turn (Esc, or a rejected approval); the turn ends. `for_tool`: while a
    /// tool call was pending.
    fn interrupt(&mut self, t: SystemTime, for_tool: bool);
    /// The turn ends normally (Stop).
    fn turn_end(&mut self, t: SystemTime);
    fn session_end(&mut self, t: SystemTime, reason: EndReason);
    /// What the CLI prints on exit to say how to resume.
    fn resume_hint(&self) -> String;
}
