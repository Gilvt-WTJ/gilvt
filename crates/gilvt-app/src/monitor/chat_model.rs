//! The 监控官's conversation as plain data (S2 §6.2–6.4): messages with their tool rows, the status pill, what is
//! written to the CLI (scope, prelude), and the timers that end a silent turn or an idle process. No gpui and no
//! processes: `monitor::chat` drives it with `ChatEvent`s. History is never written to disk.

use std::time::{Duration, Instant};

use gilvt_monitor::chat::{ChatEvent, TurnEnd};
use gilvt_monitor::provider::ProviderError;
use gilvt_monitor::tools;

pub const IDLE_LIMIT: Duration = Duration::from_secs(30 * 60);
pub const TURN_SILENCE: Duration = Duration::from_secs(5 * 60);
pub const INTERRUPT_GRACE: Duration = Duration::from_secs(5);
pub const PRELUDE_TURNS: usize = 6;
pub const PRELUDE_BYTES: usize = 1024;
/// The quick questions: (button, message sent).
pub const QUICK: [(&str, &str); 3] = [("✦ 生成站会简报", "生成站会简报"), ("哪些需要我？", "哪些需要我？"), ("有什么出错了？", "有什么出错了？")];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Notice,
    Error,
}

impl Role {
    pub fn id(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Notice => "notice",
            Role::Error => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolRow {
    pub id: String,
    pub name: String,
    /// 「… 正在读取 …」 / 「✓ 已读取 …」 / 「✗ …」.
    pub label: String,
    pub done: bool,
    pub ok: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorCard {
    pub title: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub role: Role,
    /// User: as typed (without scope); assistant: Markdown so far; notice / error: one line.
    pub text: String,
    /// The chips a user message was sent with: (key, label).
    pub chips: Vec<(String, String)>,
    pub tools: Vec<ToolRow>,
    /// An assistant message still being written.
    pub open: bool,
    /// The red card of a chat that could not start.
    pub card: Option<ErrorCard>,
    /// An answer's assistant messages (message id, text), joined into `text`.
    parts: Vec<(String, String)>,
}

impl Message {
    fn new(role: Role, text: impl Into<String>) -> Message {
        Message { role, text: text.into(), chips: Vec::new(), tools: Vec::new(), open: false, card: None, parts: Vec::new() }
    }

    fn rebuild(&mut self) {
        self.text = self.parts.iter().map(|(_, t)| t.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n");
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    Starting,
    Answering,
    /// An interrupt was sent; waiting for the turn to end.
    Stopping,
    Error,
}

impl Status {
    pub fn id(self) -> &'static str {
        match self {
            Status::Idle => "idle",
            Status::Starting => "starting",
            Status::Answering => "answering",
            Status::Stopping => "stopping",
            Status::Error => "error",
        }
    }

    pub fn pill(self) -> Option<&'static str> {
        match self {
            Status::Idle => None,
            Status::Starting => Some("启动中"),
            Status::Answering => Some("回答中"),
            Status::Stopping => Some("停止中"),
            Status::Error => Some("出错"),
        }
    }
}

/// A message to send: what the user typed and the chips (key, label) that scope it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub text: String,
    pub chips: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Conversation {
    pub messages: Vec<Message>,
    pub status: Status,
    /// +1 per change.
    pub revision: u64,
    /// Turns ended (all processes).
    pub turns: u64,
}

fn first_line(s: &str, max_chars: usize) -> String {
    gilvt_monitor::output::clip_chars(s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or(""), max_chars)
}

pub fn exit_text(code: Option<i32>, stderr_tail: &str) -> String {
    match (code, stderr_tail.trim()) {
        (Some(c), "") => format!("监控官进程已退出（退出码 {c}）"),
        (Some(c), tail) => format!("监控官进程已退出（退出码 {c}：{}）", gilvt_monitor::output::clip_chars(tail, 200)),
        (None, _) => "监控官进程已退出（被信号结束）".to_string(),
    }
}

impl Conversation {
    pub fn push_user(&mut self, out: &Outgoing) {
        let mut m = Message::new(Role::User, out.text.clone());
        m.chips = out.chips.clone();
        self.messages.push(m);
        self.revision += 1;
    }

    pub fn notice(&mut self, text: &str) {
        self.messages.push(Message::new(Role::Notice, text));
        self.revision += 1;
    }

    pub fn error_card(&mut self, card: ErrorCard) {
        let mut m = Message::new(Role::Error, card.text.clone());
        m.card = Some(card);
        self.messages.push(m);
        self.status = Status::Error;
        self.revision += 1;
    }

    fn push_error(&mut self, text: String) {
        self.messages.push(Message::new(Role::Error, text));
        self.status = Status::Error;
    }

    /// The open answer — wherever it is: a message sent while answering sits below it — or a new one.
    fn assistant(&mut self) -> &mut Message {
        let i = match self.messages.iter().rposition(|m| m.role == Role::Assistant && m.open) {
            Some(i) => i,
            None => {
                let mut m = Message::new(Role::Assistant, "");
                m.open = true;
                self.messages.push(m);
                self.messages.len() - 1
            }
        };
        &mut self.messages[i]
    }

    /// Ends the open answer (dropping it if nothing came); unfinished tool rows fail.
    pub fn abandon(&mut self) {
        let Some(i) = self.messages.iter().rposition(|m| m.role == Role::Assistant && m.open) else { return };
        let m = &mut self.messages[i];
        m.open = false;
        for t in m.tools.iter_mut().filter(|t| !t.done) {
            t.done = true;
            t.label = format!("✗ {}：没有完成", t.name);
        }
        if m.text.is_empty() && m.tools.is_empty() {
            self.messages.remove(i);
        }
        self.revision += 1;
    }

    pub fn apply(&mut self, ev: &ChatEvent, name_of: &dyn Fn(&str) -> Option<String>) {
        match ev {
            ChatEvent::Ready { note: Some(n), .. } | ChatEvent::Notice(n) => self.messages.push(Message::new(Role::Notice, n.clone())),
            ChatEvent::Ready { .. } | ChatEvent::BadLine(_) => {}
            ChatEvent::TurnStarted => self.status = Status::Answering,
            ChatEvent::Text { message, delta } => {
                let m = self.assistant();
                match m.parts.iter_mut().find(|(id, _)| id == message) {
                    Some((_, t)) => t.push_str(delta),
                    None => m.parts.push((message.clone(), delta.clone())),
                }
                m.rebuild();
            }
            ChatEvent::TextDone { message, text } => {
                let m = self.assistant();
                match m.parts.iter_mut().find(|(id, _)| id == message) {
                    Some((_, t)) => *t = text.clone(),
                    None => m.parts.push((message.clone(), text.clone())),
                }
                m.rebuild();
            }
            ChatEvent::ToolStarted { id, tool, args } => {
                let label = format!("… {}", tools::pending_label(tool, args, name_of));
                self.assistant().tools.push(ToolRow { id: id.clone(), name: tool.clone(), label, done: false, ok: false });
            }
            ChatEvent::ToolDone { id, ok, text } => {
                let row = self.messages.iter_mut().rev().filter(|m| m.role == Role::Assistant).flat_map(|m| m.tools.iter_mut()).find(|t| &t.id == id);
                if let Some(row) = row {
                    row.done = true;
                    row.ok = *ok;
                    row.label = match (*ok, tools::done_label(text)) {
                        (true, Some(l)) => format!("✓ {l}"),
                        (true, None) => format!("✓ 已调用 {}", row.name),
                        (false, _) => format!("✗ {}：{}", row.name, first_line(text, 80)),
                    };
                }
            }
            ChatEvent::TurnEnded(end) => {
                self.abandon();
                self.turns += 1;
                self.status = Status::Idle;
                match end {
                    TurnEnd::Done => {}
                    TurnEnd::Interrupted => self.messages.push(Message::new(Role::Notice, "（这一轮已中断）")),
                    TurnEnd::Failed(e) => self.push_error(e.message()),
                }
            }
            ChatEvent::Failed(e) => {
                self.abandon();
                self.push_error(e.message());
            }
            ChatEvent::Exited { code, stderr_tail } => {
                self.abandon();
                if self.messages.last().is_some_and(|m| m.role == Role::Error) {
                    self.status = Status::Error;
                } else {
                    self.push_error(exit_text(*code, stderr_tail));
                }
            }
        }
        self.revision += 1;
    }
}

/// `s` on one line, at most [`PRELUDE_BYTES`] bytes (on a char boundary).
fn prelude_line(s: &str) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut end = flat.len().min(PRELUDE_BYTES);
    while !flat.is_char_boundary(end) {
        end -= 1;
    }
    flat[..end].to_string()
}

/// What a new process is told about the conversation so far: the last [`PRELUDE_TURNS`] questions and answers.
pub fn prelude(messages: &[Message]) -> Option<String> {
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        if m.role != Role::User {
            continue;
        }
        let answer = messages[i + 1..].iter().take_while(|n| n.role != Role::User).find(|n| n.role == Role::Assistant && !n.text.is_empty()).map_or("（没有回答）", |n| n.text.as_str());
        pairs.push((m.text.as_str(), answer));
    }
    if pairs.is_empty() {
        return None;
    }
    let start = pairs.len().saturating_sub(PRELUDE_TURNS);
    let body: String = pairs[start..].iter().map(|(q, a)| format!("你：{}\n监控官：{}\n", prelude_line(q), prelude_line(a))).collect();
    Some(format!("<前情>\n{body}</前情>\n"))
}

/// The line written to the CLI: scope first, then the prelude (first message to a new process), then the text.
pub fn wire_text(out: &Outgoing, prelude: Option<&str>) -> String {
    let mut s = String::new();
    if !out.chips.is_empty() {
        let keys: Vec<&str> = out.chips.iter().map(|(k, _)| k.as_str()).collect();
        s.push_str(&format!("<scope keys=\"{}\"/>\n", keys.join(" ")));
    }
    if let Some(p) = prelude {
        s.push_str(p);
    }
    s.push_str(&out.text);
    s
}

/// Messages queued behind a turn, sent as one: texts with a blank line between, chips in order of first use.
pub fn merge(queue: &[Outgoing]) -> Option<Outgoing> {
    let first = queue.first()?;
    let mut chips = first.chips.clone();
    for c in queue[1..].iter().flat_map(|o| &o.chips) {
        if !chips.iter().any(|(k, _)| k == &c.0) {
            chips.push(c.clone());
        }
    }
    Some(Outgoing { text: queue.iter().map(|o| o.text.as_str()).collect::<Vec<_>>().join("\n\n"), chips })
}

/// The red card when the chat cannot start (the settings page's wording for a missing CLI / a failed login).
pub fn error_card(provider: &str, program: &str, e: &ProviderError) -> ErrorCard {
    let text = match e {
        ProviderError::Unsupported(m) => m.clone(),
        other => crate::settings_window::form::advice(program, other),
    };
    ErrorCard { title: format!("无法启动 {provider} 对话"), text }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timeout {
    /// [`TURN_SILENCE`] without output in a turn: interrupt it.
    TurnSilent,
    /// [`IDLE_LIMIT`] without a message: end the process.
    Idle,
    /// No turn end [`INTERRUPT_GRACE`] after an interrupt: kill the process.
    InterruptIgnored,
}

#[derive(Clone, Copy, Debug)]
pub struct Timers {
    pub last_output: Instant,
    pub last_use: Instant,
    pub interrupted_at: Option<Instant>,
}

impl Timers {
    pub fn new(now: Instant) -> Timers {
        Timers { last_output: now, last_use: now, interrupted_at: None }
    }

    pub fn output(&mut self, now: Instant) {
        self.last_output = now;
    }

    pub fn used(&mut self, now: Instant) {
        self.last_use = now;
        self.last_output = now;
    }

    pub fn check(&self, now: Instant, answering: bool) -> Option<Timeout> {
        if let Some(t) = self.interrupted_at {
            return (now.saturating_duration_since(t) >= INTERRUPT_GRACE).then_some(Timeout::InterruptIgnored);
        }
        if answering {
            return (now.saturating_duration_since(self.last_output) >= TURN_SILENCE).then_some(Timeout::TurnSilent);
        }
        (now.saturating_duration_since(self.last_use) >= IDLE_LIMIT).then_some(Timeout::Idle)
    }
}

#[cfg(test)]
#[path = "chat_model_tests.rs"]
mod tests;
