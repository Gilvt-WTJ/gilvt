//! What gilvt tells the model about a session or a terminal (S2 §4.2): structured, clipped text.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gilvt_agent::{Item, ItemStatus, ToolItem, Turn, TurnOutcome};
use serde::{Deserialize, Serialize};

use crate::output::clip_chars;
use crate::provider::OneShot;

pub const MAX_INPUT: usize = 24 * 1024;
pub const TOOL_BYTES: usize = 4096;
pub const FIRST_TURNS: usize = 2;
pub const ROLLING_TURNS: usize = 3;
pub const TERMINAL_COMMANDS: usize = 10;
const PROMPT_CHARS: usize = 600;

pub const AGENT_INSTRUCTIONS: &str = "你是 gilvt 终端里的监控官，给一个编码 Agent 会话写进展摘要，让用户不用切过去就知道它在干什么。\
只根据给出的记录，不要编造，不要提建议。只输出两行，不要 Markdown：\n\
目标：<这个会话要完成什么，一句话>\n\
近期：<最近做了什么、结果如何、现在卡在哪，一到两句>\n\
每行不超过 120 个字，用中文。给出了上一份总结时，目标没有明显变化就原样保留。";

pub const TERMINAL_INSTRUCTIONS: &str = "你是 gilvt 终端里的监控官，给一个普通终端写进展摘要。\
只根据给出的命令记录，不要编造，不要提建议。只输出一行，不要 Markdown：\n\
近期：<最近在做什么、结果如何，一到两句，失败时说出失败的地方>\n\
不超过 120 个字，用中文。";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Covers {
    /// Turns a..=b.
    Turns(u32, u32),
    /// The newest n commands.
    Commands(usize),
}

impl Covers {
    /// For the card (not the model): follows the interface language.
    pub fn label(&self) -> String {
        if gilvt_i18n::english() {
            return match *self {
                Covers::Turns(a, b) if a == b => format!("Covers turn {a}"),
                Covers::Turns(a, b) => format!("Covers turns {a}–{b}"),
                Covers::Commands(1) => "Covers the last command".into(),
                Covers::Commands(n) => format!("Covers the last {n} commands"),
            };
        }
        match *self {
            Covers::Turns(a, b) if a == b => format!("覆盖第 {a} 轮"),
            Covers::Turns(a, b) => format!("覆盖第 {a}–{b} 轮"),
            Covers::Commands(n) => format!("覆盖最近 {n} 条命令"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AgentDigest<'a> {
    pub name: &'a str,
    pub agent: &'a str,
    pub cwd: Option<&'a Path>,
    pub status: &'a str,
    pub todo: Option<(usize, usize)>,
    pub turns: &'a [Arc<Turn>],
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommandDigest {
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub exit: Option<i32>,
    pub running: bool,
    pub took: Option<Duration>,
    pub output_tail: Option<String>,
}

/// `commands` are newest first.
#[derive(Clone, Debug)]
pub struct TerminalDigest<'a> {
    pub name: &'a str,
    pub cwd: Option<&'a Path>,
    pub commands: &'a [CommandDigest],
}

#[derive(Clone, Debug)]
pub struct Previous<'a> {
    pub goal: Option<&'a str>,
    pub recent: &'a str,
    pub through_turn: u32,
}

/// Lines of a prompt. Droppable ones carry a priority; when the whole is too long the lowest priority goes
/// first (ties: earlier line first), and one note says how many went.
struct Lines(Vec<(String, Option<u32>)>);

impl Lines {
    fn push(&mut self, s: impl Into<String>) {
        self.0.push((s.into(), None));
    }
    /// A droppable line (already clipped by the caller); `priority` low = dropped first.
    fn droppable(&mut self, s: String, priority: u32) {
        self.0.push((s, Some(priority)));
    }
    fn finish(self, note: fn(usize) -> String) -> String {
        self.finish_within(note, MAX_INPUT)
    }
    fn finish_within(mut self, note: fn(usize) -> String, cap: usize) -> String {
        let total = |l: &[(String, Option<u32>)]| l.iter().map(|(s, _)| s.len() + 1).sum::<usize>();
        // The note is at most as long as for "every droppable line dropped".
        let reserve = note(self.0.iter().filter(|(_, d)| d.is_some()).count()).len() + 1;
        let mut dropped = 0;
        let mut at = None;
        while total(&self.0) > cap && total(&self.0) + reserve > cap {
            let Some(i) = self.0.iter().enumerate().filter_map(|(i, (_, d))| d.map(|d| (d, i))).min().map(|(_, i)| i) else { break };
            self.0.remove(i);
            at = Some(at.map_or(i, |a: usize| a.min(i)));
            dropped += 1;
        }
        if let Some(i) = at {
            self.0.insert(i.min(self.0.len()), (note(dropped), None));
        }
        let text: String = self.0.into_iter().map(|(s, _)| s + "\n").collect();
        // Safety net only: the clips above keep everything else small.
        clip_bytes(&text, cap)
    }
}

/// The first `max` bytes of `s` on a char boundary.
fn clip_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// The last `max` bytes of `s` on a char boundary.
pub(crate) fn clip_bytes_tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

fn agent_note(n: usize) -> String {
    format!("（已省略 {n} 条更早的动作）")
}

fn terminal_note(n: usize) -> String {
    format!("（已省略 {n} 条较早命令的输出）")
}

fn outcome_text(o: &TurnOutcome) -> String {
    match o {
        TurnOutcome::Running => "进行中".into(),
        TurnOutcome::Done => "完成".into(),
        TurnOutcome::Interrupted => "被中断".into(),
        TurnOutcome::Failed { message } => format!("失败：{}", clip_chars(message, PROMPT_CHARS)),
    }
}

fn tool_line(t: &ToolItem) -> String {
    let (mark, tail) = match &t.status {
        ItemStatus::Ok => ("✓", String::new()),
        ItemStatus::Failed { exit: Some(c) } => ("✗", format!("（exit {c}）")),
        ItemStatus::Failed { exit: None } => ("✗", String::new()),
        ItemStatus::Denied => ("⊘", "（被拒绝）".into()),
        ItemStatus::Interrupted => ("■", "（被中断）".into()),
        ItemStatus::Running | ItemStatus::Pending => ("●", String::new()),
    };
    let lines = t.lines.map(|(a, r)| format!(" (+{a} −{r})")).unwrap_or_default();
    let mut s = format!("- {mark} {} {}{lines}{tail}", t.tool, t.summary);
    if !t.children.is_empty() {
        s.push_str(&format!("（子 Agent：{} 个动作）", t.children.len()));
    }
    for e in &t.error_excerpt {
        s.push_str("\n    ");
        s.push_str(e);
    }
    s
}

pub fn agent_request(d: &AgentDigest, prev: Option<&Previous>) -> (OneShot, Covers) {
    let start = match prev {
        Some(p) => d.turns.iter().position(|t| t.index >= p.through_turn).unwrap_or(d.turns.len().saturating_sub(1)).max(d.turns.len().saturating_sub(ROLLING_TURNS)),
        None => d.turns.len().saturating_sub(FIRST_TURNS),
    };
    let shown = &d.turns[start..];
    let covers = Covers::Turns(shown.first().map_or(0, |t| t.index), shown.last().map_or(0, |t| t.index));
    let mut l = Lines(Vec::new());
    l.push(format!("会话：{}（{}）", d.name, d.agent));
    if let Some(cwd) = d.cwd {
        l.push(format!("目录：{}", cwd.display()));
    }
    l.push(format!("状态：{}", d.status));
    if let Some((done, total)) = d.todo {
        l.push(format!("TODO：{done}/{total}"));
    }
    if let Some(p) = prev {
        l.push("<上一份总结>");
        if let Some(g) = p.goal {
            l.push(format!("目标：{}", clip_chars(g, PROMPT_CHARS)));
        }
        l.push(format!("近期：{}", clip_chars(p.recent, PROMPT_CHARS)));
        l.push("</上一份总结>");
    }
    let mut seq = 0;
    for t in shown {
        push_turn(&mut l, t, &mut seq);
    }
    (OneShot { instructions: AGENT_INSTRUCTIONS.into(), prompt: l.finish(agent_note) }, covers)
}

/// One turn's lines: heading, prompt, one droppable line per tool call (`seq` numbers them oldest first, so the
/// oldest go first), the reply.
fn push_turn(l: &mut Lines, t: &Turn, seq: &mut u32) {
    l.push(format!("## 第 {} 轮（{}）", t.index, outcome_text(&t.outcome)));
    l.push(format!("用户：{}", clip_chars(t.prompt.trim(), PROMPT_CHARS)));
    for item in &t.items {
        match item {
            Item::Tool(tool) => {
                *seq += 1;
                l.droppable(clip_bytes(&tool_line(tool), TOOL_BYTES), *seq);
            }
            Item::Truncated { hidden } => l.push(format!("（另有 {hidden} 条更早的动作）")),
            Item::Thinking { .. } => {}
        }
    }
    if !t.reply.is_empty() {
        l.push(format!("回复：{}", clip_chars(&t.reply, PROMPT_CHARS)));
    }
}

/// A session's turns as the chat's `get_timeline` returns them (S2 §6.1): the same lines and clips as a summary's
/// input, at most [`MAX_INPUT`] bytes (the oldest tool calls dropped first, with a note).
pub fn timeline_text(name: &str, turns: &[Arc<Turn>]) -> String {
    timeline_text_within(name, turns, MAX_INPUT)
}

/// [`timeline_text`] clipped to `cap` bytes instead.
pub fn timeline_text_within(name: &str, turns: &[Arc<Turn>], cap: usize) -> String {
    let mut l = Lines(Vec::new());
    l.push(format!("会话：{name}"));
    let mut seq = 0;
    for t in turns {
        push_turn(&mut l, t, &mut seq);
    }
    l.finish_within(agent_note, cap)
}

/// A terminal's newest `max` commands (newest first) as text: what a terminal summary reads, and the chat's
/// `get_commands`.
pub fn commands_text(d: &TerminalDigest, max: usize) -> String {
    commands_text_within(d, max, MAX_INPUT)
}

/// [`commands_text`] clipped to `cap` bytes instead (the newest commands and the ends of outputs stay).
pub fn commands_text_within(d: &TerminalDigest, max: usize, cap: usize) -> String {
    let shown = &d.commands[..d.commands.len().min(max)];
    let mut l = Lines(Vec::new());
    l.push(format!("终端：{}", d.name));
    if let Some(cwd) = d.cwd {
        l.push(format!("目录：{}", cwd.display()));
    }
    l.push("最近的命令（新的在前）：");
    for (i, c) in shown.iter().enumerate() {
        let state = match (c.running, c.exit) {
            (true, _) => "运行中".to_string(),
            (false, Some(code)) => format!("exit {code}"),
            (false, None) => "结束情况未知".to_string(),
        };
        let took = c.took.map(|t| format!("，用时 {} 秒", t.as_secs())).unwrap_or_default();
        l.push(format!("## $ {}（{state}{took}）", clip_chars(c.command.trim(), PROMPT_CHARS)));
        if let Some(cwd) = &c.cwd {
            l.push(format!("目录：{}", cwd.display()));
        }
        if let Some(tail) = c.output_tail.as_deref().filter(|t| !t.is_empty()) {
            // Newest command first, so the oldest output (highest index) is dropped first.
            l.droppable(format!("输出末尾：\n{}", clip_bytes_tail(tail, TOOL_BYTES)), (shown.len() - i) as u32);
        }
    }
    l.finish_within(terminal_note, cap)
}

pub fn terminal_request(d: &TerminalDigest) -> (OneShot, Covers) {
    let shown = d.commands.len().min(TERMINAL_COMMANDS);
    (OneShot { instructions: TERMINAL_INSTRUCTIONS.into(), prompt: commands_text(d, TERMINAL_COMMANDS) }, Covers::Commands(shown))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(index: u32, prompt: &str, outcome: TurnOutcome, tools: Vec<ToolItem>, reply: &str) -> Arc<Turn> {
        let mut t = Turn::new_for_test(prompt);
        t.index = index;
        t.outcome = outcome;
        t.items = tools.into_iter().map(Item::Tool).collect();
        t.steps = t.items.len();
        t.reply = reply.into();
        Arc::new(t)
    }

    fn tool(name: &str, summary: &str, status: ItemStatus) -> ToolItem {
        let mut t = ToolItem::new("id", name);
        t.summary = summary.into();
        t.status = status;
        t
    }

    fn digest<'a>(turns: &'a [Arc<Turn>]) -> AgentDigest<'a> {
        AgentDigest { name: "web-login", agent: "Codex", cwd: Some(Path::new("/w/web")), status: "● Bash(pnpm test)", todo: Some((2, 5)), turns }
    }

    #[test]
    fn first_summary_takes_the_last_two_turns() {
        let turns = vec![
            turn(1, "先读一下", TurnOutcome::Done, vec![], ""),
            turn(2, "迁到 JWT", TurnOutcome::Done, vec![tool("Edit", "src/jwt.ts", ItemStatus::Ok)], "改完了。"),
            turn(3, "修测试", TurnOutcome::Running, vec![tool("Bash", "pnpm test", ItemStatus::Failed { exit: Some(1) })], ""),
        ];
        let (req, covers) = agent_request(&digest(&turns), None);
        assert_eq!(covers, Covers::Turns(2, 3));
        assert!(!req.prompt.contains("先读一下"));
        assert!(req.prompt.contains("用户：迁到 JWT"));
        assert!(req.prompt.contains("- ✗ Bash pnpm test（exit 1）"));
        assert!(req.prompt.contains("- ✓ Edit src/jwt.ts"));
        assert!(req.prompt.contains("回复：改完了。"));
        assert!(req.prompt.contains("TODO：2/5"));
        assert!(req.prompt.contains("状态：● Bash(pnpm test)"));
        assert_eq!(req.instructions, AGENT_INSTRUCTIONS);
    }

    #[test]
    fn rolling_update_sends_the_previous_summary_and_newer_turns() {
        let turns: Vec<_> = (1..=6).map(|i| turn(i, &format!("第{i}轮"), TurnOutcome::Done, vec![], "")).collect();
        let prev = Previous { goal: Some("旧目标"), recent: "旧近期", through_turn: 2 };
        let (req, covers) = agent_request(&digest(&turns), Some(&prev));
        assert_eq!(covers, Covers::Turns(4, 6), "at most three turns");
        assert!(req.prompt.contains("<上一份总结>\n目标：旧目标\n近期：旧近期\n</上一份总结>"));
        assert!(!req.prompt.contains("第3轮"));
        let prev = Previous { goal: None, recent: "r", through_turn: 6 };
        assert_eq!(agent_request(&digest(&turns), Some(&prev)).1, Covers::Turns(6, 6), "the covered turn may have grown");
    }

    #[test]
    fn a_huge_tool_is_clipped_and_the_whole_stays_under_the_cap() {
        let mut big = tool("Bash", &"x".repeat(10_000), ItemStatus::Failed { exit: Some(2) });
        big.error_excerpt = vec!["e".repeat(10_000)];
        let many: Vec<ToolItem> = (0..400).map(|i| tool("Read", &format!("file{i}.rs {}", "y".repeat(100)), ItemStatus::Ok)).collect();
        let mut tools = vec![big];
        tools.extend(many);
        let turns = vec![turn(1, "p", TurnOutcome::Running, tools, "")];
        let (req, _) = agent_request(&digest(&turns), None);
        assert!(req.prompt.len() <= MAX_INPUT, "{}", req.prompt.len());
        assert!(req.prompt.contains("（已省略 "), "older lines are dropped with a note");
        assert!(req.prompt.lines().all(|l| l.len() <= TOOL_BYTES + 16));
    }

    #[test]
    fn terminal_commands_newest_first() {
        let cmds = vec![
            CommandDigest { command: "make test".into(), cwd: Some("/r".into()), exit: Some(2), running: false, took: Some(Duration::from_secs(12)), output_tail: Some("FAIL pkg/cache".into()) },
            CommandDigest { command: "git pull".into(), cwd: Some("/r".into()), exit: Some(0), running: false, took: None, output_tail: None },
        ];
        let (req, covers) = terminal_request(&TerminalDigest { name: "zsh", cwd: Some(Path::new("/r")), commands: &cmds });
        assert_eq!(covers, Covers::Commands(2));
        let make = req.prompt.find("$ make test").unwrap();
        assert!(make < req.prompt.find("$ git pull").unwrap());
        assert!(req.prompt.contains("exit 2"));
        assert!(req.prompt.contains("FAIL pkg/cache"));
        assert_eq!(req.instructions, TERMINAL_INSTRUCTIONS);
    }

    #[test]
    fn terminal_drops_the_oldest_output_first() {
        let cmds: Vec<CommandDigest> = (0..10)
            .map(|i| CommandDigest { command: format!("cmd{i}"), cwd: None, exit: Some(0), running: false, took: None, output_tail: Some(format!("{}END{i}", "o".repeat(4000))) })
            .collect();
        let (req, _) = terminal_request(&TerminalDigest { name: "zsh", cwd: None, commands: &cmds });
        assert!(req.prompt.len() <= MAX_INPUT, "{}", req.prompt.len());
        assert!(req.prompt.contains("END0"), "the newest command keeps its output");
        assert!(!req.prompt.contains("END9"), "the oldest goes first");
        assert!(req.prompt.contains("较早命令"));
        assert!(!req.prompt.contains("更早的动作"));
    }

    #[test]
    fn a_huge_reply_is_clipped_not_cut_by_the_safety_net() {
        let turns = vec![turn(1, "p", TurnOutcome::Done, vec![], &"好".repeat(40 * 1024))];
        let (req, _) = agent_request(&digest(&turns), None);
        assert!(req.prompt.len() <= MAX_INPUT, "{}", req.prompt.len());
        let last = req.prompt.lines().last().unwrap();
        assert!(last.starts_with("回复："), "{last}");
        assert!(last.chars().count() <= PROMPT_CHARS + 8);
    }

    #[test]
    fn terminal_output_keeps_its_end() {
        let tail = format!("{}\nUNIQUE-LAST-LINE", "a".repeat(6000));
        let cmds = vec![CommandDigest { command: "make".into(), cwd: None, exit: Some(1), running: false, took: None, output_tail: Some(tail) }];
        let (req, _) = terminal_request(&TerminalDigest { name: "zsh", cwd: None, commands: &cmds });
        assert!(req.prompt.contains("UNIQUE-LAST-LINE"));
        assert!(req.prompt.len() < TOOL_BYTES + 400);
    }

    #[test]
    fn covers_labels() {
        assert_eq!(Covers::Turns(2, 3).label(), "覆盖第 2–3 轮");
        assert_eq!(Covers::Turns(4, 4).label(), "覆盖第 4 轮");
        assert_eq!(Covers::Commands(4).label(), "覆盖最近 4 条命令");
    }

    #[test]
    fn timeline_text_lists_the_turns_asked_for() {
        let turns = vec![
            turn(2, "改登录", TurnOutcome::Done, vec![tool("Edit", "src/a.ts", ItemStatus::Ok)], "改好了。"),
            turn(3, "跑测试", TurnOutcome::Running, vec![tool("Bash", "pnpm test", ItemStatus::Failed { exit: Some(1) })], ""),
        ];
        let text = timeline_text("web-login", &turns);
        assert!(text.starts_with("会话：web-login\n"), "{text}");
        assert!(text.contains("## 第 2 轮（完成）") && text.contains("## 第 3 轮（进行中）"), "{text}");
        assert!(text.contains("- ✓ Edit src/a.ts") && text.contains("- ✗ Bash pnpm test（exit 1）"), "{text}");
        assert!(text.contains("回复：改好了。"));
    }

    #[test]
    fn timeline_text_is_clipped_like_summaries() {
        let tools: Vec<ToolItem> = (0..2000).map(|i| tool("Bash", &format!("cmd {i} {}", "x".repeat(100)), ItemStatus::Ok)).collect();
        let turns = vec![turn(1, "大轮", TurnOutcome::Done, tools, "")];
        let text = timeline_text("big", &turns);
        assert!(text.len() <= MAX_INPUT, "{}", text.len());
        assert!(text.contains("已省略"), "says how many went");
        assert!(text.contains("cmd 1999 "), "the newest calls stay");
        assert!(!text.contains("cmd 0 "), "the oldest go first");
    }

    #[test]
    fn commands_text_takes_up_to_max() {
        let commands: Vec<CommandDigest> = (0..25)
            .map(|i| CommandDigest { command: format!("make t{i}"), cwd: None, exit: Some(0), running: false, took: None, output_tail: None })
            .collect();
        let d = TerminalDigest { name: "zsh", cwd: None, commands: &commands };
        let text = commands_text(&d, 20);
        assert!(text.contains("make t19") && !text.contains("make t20"), "{text}");
        assert_eq!(terminal_request(&d).0.prompt, commands_text(&d, TERMINAL_COMMANDS), "summaries are unchanged");
    }
}
