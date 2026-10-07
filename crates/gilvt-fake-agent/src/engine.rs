//! The scenario engine: runs the steps against a [`Recorder`] (files + hooks) and an [`Io`] (keys, clock,
//! screen). Pure apart from what the recorder does, so tests drive it with [`ScriptedIo`].
//!
//! Turns: a `prompt` step, the command-line prompt, or a line the user types starts one; `idle` ends it
//! (Stop) and shows a fresh prompt; the next step that needs a turn waits for the user's next line. After
//! the last step the session stays idle: every line gets the default reply, until `/exit`, Ctrl-D or
//! Ctrl-C twice. An interruption (Esc, a rejected approval with `on_no = "interrupt"`, a dismissed
//! question) or an API error ends the turn early: the steps up to the next `idle` are skipped.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde_json::Value as Json;

use crate::keys::Key;
use crate::recorder::{EndReason, Outcome, Recorder, ToolRef};
use crate::scenario::{OnNo, Step, TodoItem, ToolCall};
use crate::tui;
use crate::Agent;

/// The reply to lines typed after the scenario's last step.
pub const DEFAULT_REPLY: &str = "OK: {prompt}";
/// A second Ctrl-C within this exits.
const CTRL_C_WINDOW: Duration = Duration::from_secs(2);

/// Keys, time and the screen.
pub trait Io {
    fn now(&self) -> SystemTime;
    /// The next key within `timeout` (`None`: wait for one). `None` when the timeout passed.
    fn key(&mut self, timeout: Option<Duration>) -> Option<Key>;
    fn write(&mut self, text: &str);
}

/// A scripted key for [`ScriptedIo`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scripted {
    /// Delivered to the next read that waits for a key (a prompt, a dialog).
    Key(Key),
    /// Delivered to the next read of any kind, including a timed wait while a tool runs (Esc mid-call).
    During(Key),
}

/// Headless [`Io`]: keys from a script (then `Eof`), a clock that only moves when the engine waits (and
/// 1 ms per key), and the screen collected in a string.
pub struct ScriptedIo {
    now: SystemTime,
    keys: VecDeque<Scripted>,
    pub screen: String,
}

impl ScriptedIo {
    pub fn new(start: SystemTime, keys: Vec<Scripted>) -> ScriptedIo {
        ScriptedIo { now: start, keys: keys.into(), screen: String::new() }
    }

    /// Every key as a [`Scripted::Key`].
    pub fn typing(start: SystemTime, keys: impl IntoIterator<Item = Key>) -> ScriptedIo {
        ScriptedIo::new(start, keys.into_iter().map(Scripted::Key).collect())
    }
}

/// Keys typing `text`, then Enter.
pub fn line(text: &str) -> Vec<Key> {
    text.chars().map(Key::Char).chain([Key::Enter]).collect()
}

impl Io for ScriptedIo {
    fn now(&self) -> SystemTime {
        self.now
    }

    fn key(&mut self, timeout: Option<Duration>) -> Option<Key> {
        let key = match (timeout, self.keys.front()) {
            (Some(_), Some(Scripted::During(_))) | (None, Some(_)) => self.keys.pop_front(),
            (Some(t), _) => {
                self.now += t;
                return None;
            }
            (None, None) => return Some(Key::Eof),
        };
        self.now += Duration::from_millis(1);
        key.map(|(Scripted::Key(k) | Scripted::During(k))| k)
    }

    fn write(&mut self, text: &str) {
        self.screen.push_str(text);
    }
}

/// How the session starts.
#[derive(Clone, Debug)]
pub struct Settings {
    pub agent: Agent,
    pub cwd: PathBuf,
    /// The command-line prompt: the first turn's (instead of the first `prompt` step's text).
    pub initial_prompt: Option<String>,
    /// `--resume` / `resume`.
    pub resumed: bool,
    /// Set the terminal title (Claude only).
    pub cwd_title: bool,
}

enum Flow {
    Go,
    /// The turn ended early: skip to the next `idle`.
    Skip,
    Exit(i32, EndReason),
}

pub struct Engine<R: Recorder, I: Io> {
    pub rec: R,
    pub io: I,
    steps: Vec<Step>,
    s: Settings,
    in_turn: bool,
    /// `❯ ` is on screen, waiting for input.
    prompt_shown: bool,
    turn_prompt: String,
    answer: String,
    ctrl_c: Option<SystemTime>,
    /// Keys typed while the engine waited (type-ahead), for the next prompt or dialog.
    typed_ahead: VecDeque<Key>,
}

impl<R: Recorder, I: Io> Engine<R, I> {
    pub fn new(rec: R, io: I, steps: Vec<Step>, settings: Settings) -> Self {
        Engine { rec, io, steps, s: settings, in_turn: false, prompt_shown: false, turn_prompt: String::new(), answer: String::new(), ctrl_c: None, typed_ahead: VecDeque::new() }
    }

    /// Runs the session to its end; returns the exit code.
    pub fn run(&mut self) -> i32 {
        self.io.write(&tui::welcome(self.s.agent, &self.s.cwd));
        self.set_title(None);
        let now = self.io.now();
        self.rec.session_start(now, self.s.resumed);
        let steps = std::mem::take(&mut self.steps);
        let mut skipping = false;
        for (i, step) in steps.iter().enumerate() {
            if skipping {
                match step {
                    Step::Idle => skipping = false,
                    Step::Exit(code) => return self.finish(*code, EndReason::Other),
                    _ => {}
                }
                continue;
            }
            match self.step(step, last_in_turn(&steps[i + 1..])) {
                Flow::Go => {}
                Flow::Skip => skipping = true,
                Flow::Exit(code, reason) => return self.finish(code, reason),
            }
        }
        if self.in_turn {
            self.end_turn();
        }
        loop {
            match self.read_line() {
                Ok(text) => {
                    self.begin_turn(&text);
                    self.reply(DEFAULT_REPLY, true);
                    self.end_turn();
                }
                Err(Flow::Exit(code, reason)) => return self.finish(code, reason),
                Err(_) => unreachable!("read_line only fails with Exit"),
            }
        }
    }

    fn step(&mut self, step: &Step, last: bool) -> Flow {
        if step.in_turn() {
            if let Err(flow) = self.ensure_turn() {
                return flow;
            }
        }
        match step {
            Step::Prompt(text) => {
                if self.in_turn {
                    self.end_turn();
                }
                let text = self.s.initial_prompt.take().unwrap_or_else(|| text.clone());
                self.echo_prompt(&text);
                self.begin_turn(&text);
                Flow::Go
            }
            Step::Sleep(d) => match self.wait(*d) {
                true => self.interrupt(false),
                false => Flow::Go,
            },
            Step::Idle => {
                if self.in_turn {
                    self.end_turn();
                }
                self.show_prompt();
                Flow::Go
            }
            Step::Exit(code) => Flow::Exit(*code, EndReason::Other),
            Step::Tool(call) => self.tool(call),
            Step::Approve { call, on_no } => self.approve(call, *on_no),
            Step::Ask { question, header, options } => self.ask(question, header, options),
            Step::Todo(items) => {
                self.todo(items);
                Flow::Go
            }
            Step::Reply(text) => {
                self.reply(text, last);
                Flow::Go
            }
            Step::Thinking { text, ms } => self.thinking(text, *ms),
            Step::ApiError(message) => {
                let now = self.io.now();
                self.rec.api_error(now, message);
                self.io.write(&tui::api_error(message));
                self.in_turn = false;
                self.show_prompt();
                Flow::Skip
            }
        }
    }

    fn tool(&mut self, call: &ToolCall) -> Flow {
        let input = self.fill_json(&call.input);
        let now = self.io.now();
        let r = self.rec.tool_start(now, &call.tool, &input);
        self.io.write(&tui::tool_line(&call.tool, &input));
        self.apply_writes(call);
        self.run_tool(&r, call)
    }

    /// The call's file effects, applied when it starts (`{cwd}` in paths and content is replaced).
    fn apply_writes(&self, call: &ToolCall) {
        for (path, content) in &call.writes {
            let path = std::path::PathBuf::from(self.fill(path));
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&path, self.fill(content));
        }
    }

    /// The call runs for `ms` (Esc interrupts it) and returns its result.
    fn run_tool(&mut self, r: &ToolRef, call: &ToolCall) -> Flow {
        if self.wait(Duration::from_millis(call.ms)) {
            let now = self.io.now();
            self.rec.tool_end(now, r, Outcome::Interrupted);
            return self.interrupt(true);
        }
        let result = self.fill(&call.result);
        let now = self.io.now();
        match call.exit_code.filter(|c| *c != 0) {
            Some(exit) => {
                self.rec.tool_end(now, r, Outcome::Failed { exit, output: &result });
                self.io.write(&tui::result_line(&format!("Error: Exit code {exit}\n{result}")));
            }
            None => {
                self.rec.tool_end(now, r, Outcome::Ok(&result));
                self.io.write(&tui::result_line(&result));
            }
        }
        Flow::Go
    }

    fn approve(&mut self, call: &ToolCall, on_no: OnNo) -> Flow {
        let input = self.fill_json(&call.input);
        let now = self.io.now();
        let r = self.rec.tool_start(now, &call.tool, &input);
        self.io.write(&tui::tool_line(&call.tool, &input));
        self.apply_writes(call);
        let now = self.io.now();
        self.rec.permission_request(now, &r);
        self.io.write(&tui::approval(self.s.agent, &call.tool, &input));
        let options = tui::approval_options(self.s.agent);
        let choice = match self.choose(&options, true) {
            Ok(c) => c,
            Err(flow) => return flow,
        };
        self.rec.out().approval_answered();
        if choice == Some(0) {
            return self.run_tool(&r, call);
        }
        let now = self.io.now();
        self.rec.tool_end(now, &r, Outcome::Rejected);
        self.io.write(&tui::result_line("User rejected the tool use"));
        match on_no {
            OnNo::SkipTool => Flow::Go,
            OnNo::Interrupt => self.interrupt(true),
        }
    }

    fn ask(&mut self, question: &str, header: &str, options: &[String]) -> Flow {
        let now = self.io.now();
        let r = self.rec.question(now, question, header, options);
        self.io.write(&tui::question(header, question));
        let choice = match self.choose(options, false) {
            Ok(c) => c,
            Err(flow) => return flow,
        };
        self.rec.out().approval_answered();
        let now = self.io.now();
        match choice {
            Some(i) => {
                self.answer = options[i].clone();
                self.rec.tool_end(now, &r, Outcome::Answered { question, answer: &options[i] });
                self.io.write(&format!("⏺ User answered Claude's questions:\n  ⎿  · {question} → {}\n", options[i]));
                Flow::Go
            }
            None => {
                self.rec.tool_end(now, &r, Outcome::Rejected);
                self.io.write("  ⎿  User declined to answer questions\n");
                self.interrupt(true)
            }
        }
    }

    /// A choice among `options` with the `❯` cursor: ↑↓ move, Enter picks. Approvals: `1` / `y` yes,
    /// `2` / `n` no at once. Questions: a digit moves the cursor there. Esc / Ctrl-C: `None`.
    fn choose(&mut self, options: &[String], approval: bool) -> Result<Option<usize>, Flow> {
        let mut cursor = 0;
        self.io.write(&tui::options(options, cursor, false));
        loop {
            let moved = match self.read_key() {
                Key::Up => cursor.checked_sub(1),
                Key::Down => Some(cursor + 1).filter(|c| *c < options.len()),
                Key::Enter => return Ok(Some(cursor)),
                Key::Esc | Key::CtrlC => return Ok(None),
                Key::Char(c) if approval => match c {
                    'y' | 'Y' | '1' => return Ok(Some(0)),
                    'n' | 'N' | '2' => return Ok(Some(1)),
                    _ => None,
                },
                Key::Char(c) => c.to_digit(10).and_then(|d| (d as usize).checked_sub(1)).filter(|d| *d < options.len()),
                Key::Eof => return Err(Flow::Exit(0, EndReason::Other)),
                _ => None,
            };
            if let Some(c) = moved {
                cursor = c;
                self.io.write(&tui::options(options, cursor, true));
            }
        }
    }

    fn todo(&mut self, items: &[TodoItem]) {
        let now = self.io.now();
        self.rec.todo(now, items);
        let head = match self.s.agent {
            Agent::Claude => "⏺ Update Todos\n",
            Agent::Codex => "• Updated Plan\n",
        };
        self.io.write(head);
        for (i, item) in items.iter().enumerate() {
            let mark = match item.status.as_str() {
                "completed" => "☒",
                "in_progress" => "◼",
                _ => "☐",
            };
            self.io.write(&format!("  {}  {mark} {}\n", if i == 0 { "⎿" } else { " " }, item.text));
        }
    }

    /// Thinks for `ms` (Esc interrupts it), then records the block.
    fn thinking(&mut self, text: &str, ms: u64) -> Flow {
        if self.wait(Duration::from_millis(ms)) {
            return self.interrupt(false);
        }
        let text = self.fill(text);
        let now = self.io.now();
        self.rec.thinking(now, &text);
        self.io.write(&tui::thinking(self.s.agent, ms));
        Flow::Go
    }

    fn reply(&mut self, text: &str, last: bool) {
        let text = self.fill(text);
        let now = self.io.now();
        self.rec.reply(now, &text, last);
        self.io.write(&tui::reply(&text));
    }

    /// The turn ends as interrupted; the rest of it is skipped.
    fn interrupt(&mut self, for_tool: bool) -> Flow {
        let now = self.io.now();
        self.rec.interrupt(now, for_tool);
        self.io.write(&tui::interrupted(self.s.agent));
        self.in_turn = false;
        self.show_prompt();
        Flow::Skip
    }

    /// Waits `d`; `true` when the user interrupted the turn (Esc / Ctrl-C) meanwhile.
    fn wait(&mut self, d: Duration) -> bool {
        let deadline = self.io.now() + d;
        loop {
            let left = match deadline.duration_since(self.io.now()) {
                Ok(left) if !left.is_zero() => left,
                _ => return false,
            };
            match self.io.key(Some(left)) {
                Some(Key::Esc | Key::CtrlC) if self.in_turn => return true,
                Some(Key::Esc | Key::CtrlC) | None => {}
                Some(Key::Eof) => {
                    // No more input: keep it for the next read, and wait out the rest.
                    if self.typed_ahead.back() != Some(&Key::Eof) {
                        self.typed_ahead.push_back(Key::Eof);
                    }
                }
                Some(key) => self.typed_ahead.push_back(key),
            }
        }
    }

    fn ensure_turn(&mut self) -> Result<(), Flow> {
        if self.in_turn {
            return Ok(());
        }
        let text = match self.s.initial_prompt.take() {
            Some(text) => {
                self.echo_prompt(&text);
                text
            }
            None => self.read_line()?,
        };
        self.begin_turn(&text);
        Ok(())
    }

    fn begin_turn(&mut self, text: &str) {
        self.turn_prompt = text.to_string();
        self.answer.clear();
        let now = self.io.now();
        self.rec.prompt(now, text);
        self.in_turn = true;
        self.set_title(Some(text));
    }

    fn end_turn(&mut self) {
        let now = self.io.now();
        self.rec.turn_end(now);
        self.in_turn = false;
        self.set_title(None);
    }

    fn echo_prompt(&mut self, text: &str) {
        if !self.prompt_shown {
            self.io.write(tui::PROMPT);
        }
        self.io.write(&format!("{text}\n"));
        self.prompt_shown = false;
    }

    fn show_prompt(&mut self) {
        self.io.write(&format!("\n{}", tui::PROMPT));
        self.prompt_shown = true;
    }

    fn set_title(&mut self, working: Option<&str>) {
        if self.s.cwd_title && self.s.agent == Agent::Claude {
            self.io.write(&tui::title(working));
        }
    }

    /// A line typed at the prompt (echoed; Backspace works, Ctrl-U clears it, a bracketed paste is inserted).
    /// `/exit`, Ctrl-D on an empty line, Ctrl-C twice and the end of input exit.
    fn read_line(&mut self) -> Result<String, Flow> {
        if !self.prompt_shown {
            self.io.write(tui::PROMPT);
            self.prompt_shown = true;
        }
        let mut buf = String::new();
        loop {
            let key = self.read_key();
            if key != Key::CtrlC {
                self.ctrl_c = None;
            }
            match key {
                Key::Char(c) => {
                    buf.push(c);
                    self.io.write(&c.to_string());
                }
                Key::Paste(text) => {
                    // Inserted as typed; a line break in it does not submit the line.
                    let text: String = text.chars().filter(|c| !c.is_control() || *c == '\n').collect();
                    buf.push_str(&text);
                    self.io.write(&text.replace('\n', "\r\n"));
                }
                Key::Backspace => {
                    if let Some(c) = buf.pop() {
                        self.io.write(&tui::erase(c));
                    }
                }
                Key::Enter if buf.trim().is_empty() => {}
                Key::Enter => {
                    self.io.write("\n");
                    self.prompt_shown = false;
                    if buf.trim() == "/exit" {
                        return Err(Flow::Exit(0, EndReason::PromptInputExit));
                    }
                    return Ok(buf);
                }
                Key::CtrlD if buf.is_empty() => return Err(Flow::Exit(0, EndReason::PromptInputExit)),
                Key::CtrlU => {
                    buf.clear();
                    self.io.write(&format!("\r\x1b[K{}", tui::PROMPT));
                }
                Key::CtrlC if !buf.is_empty() => {
                    buf.clear();
                    self.io.write(&format!("\r\x1b[K{}", tui::PROMPT));
                }
                Key::CtrlC => {
                    let now = self.io.now();
                    if self.ctrl_c.is_some_and(|t| now.duration_since(t).is_ok_and(|d| d < CTRL_C_WINDOW)) {
                        self.io.write("\n");
                        return Err(Flow::Exit(0, EndReason::PromptInputExit));
                    }
                    self.ctrl_c = Some(now);
                    self.io.write(&format!("\n  {}\n{}", tui::CTRL_C_HINT, tui::PROMPT));
                }
                Key::Eof => return Err(Flow::Exit(0, EndReason::Other)),
                _ => {}
            }
        }
    }

    /// The next key for a prompt or dialog: type-ahead first.
    fn read_key(&mut self) -> Key {
        match self.typed_ahead.pop_front() {
            Some(k) => k,
            None => self.io.key(None).unwrap_or(Key::Eof),
        }
    }

    fn finish(&mut self, code: i32, reason: EndReason) -> i32 {
        self.io.write(&format!("\n{}\n", self.rec.resume_hint()));
        let now = self.io.now();
        self.rec.session_end(now, reason);
        code
    }

    /// `{prompt}`, `{answer}`, `{cwd}` replaced.
    fn fill(&self, s: &str) -> String {
        s.replace("{prompt}", &self.turn_prompt).replace("{answer}", &self.answer).replace("{cwd}", &self.s.cwd.display().to_string())
    }

    fn fill_json(&self, v: &Json) -> Json {
        match v {
            Json::String(s) => Json::String(self.fill(s)),
            Json::Array(a) => Json::Array(a.iter().map(|x| self.fill_json(x)).collect()),
            Json::Object(o) => Json::Object(o.iter().map(|(k, x)| (k.clone(), self.fill_json(x))).collect()),
            other => other.clone(),
        }
    }
}

/// Whether nothing but the turn's end follows (for a reply: the turn's final message).
fn last_in_turn(rest: &[Step]) -> bool {
    matches!(rest.iter().find(|s| !matches!(s, Step::Sleep(_))), None | Some(Step::Idle | Step::Exit(_) | Step::Prompt(_)))
}
