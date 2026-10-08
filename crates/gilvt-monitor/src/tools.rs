//! The 监控官's read-only tools (S2 §6.1): what `gilvt mcp` lists, how a call's arguments are checked, and what
//! each call answers. Pure: gilvt-app supplies the data through [`Source`] on its main thread, and a source never
//! lists a session in an excluded directory — such a session does not exist here.

use std::path::PathBuf;
use std::sync::Arc;

use gilvt_agent::{Item, Turn};
use gilvt_i18n::english;
use serde::Serialize;
use serde_json::{json, Value};

use crate::input::{self, CommandDigest, TerminalDigest};

pub const MAX_TIMELINE_TURNS: usize = 3;
pub const MAX_COMMANDS: usize = 20;
pub const DEFAULT_COMMANDS: usize = 10;
pub const MAX_SCREEN_LINES: usize = 200;
pub const DEFAULT_SCREEN_LINES: usize = 60;
/// Claude names MCP tools `mcp__<server>__<tool>`.
pub const CLAUDE_PREFIX: &str = "mcp__gilvt__";
pub const NOT_FOUND: &str = "没有这个会话";

/// `tools/list`'s entries (MCP `Tool`): name, description, JSON schema of the arguments, read-only hint.
pub fn specs() -> Vec<Value> {
    let key = json!({"type": "string", "description": "会话的 key：agent:<claude|codex>:<会话 id> 或 pane:<pane id>（来自 list_sessions）"});
    let ro = json!({"readOnlyHint": true, "openWorldHint": false});
    let obj = |props: Value, required: &[&str]| json!({"type": "object", "properties": props, "required": required, "additionalProperties": false});
    vec![
        json!({"name": "list_sessions", "annotations": ro,
               "description": "列出 gilvt 所有窗口里的 Agent 会话与终端：key、种类、名称、分组（needs_you/error/running/done/idle/terminals/ended）、状态行、位置、cwd、git、✦ 总结。",
               "inputSchema": obj(json!({}), &[])}),
        json!({"name": "get_session", "annotations": ro,
               "description": "一个会话的概况：list_sessions 的字段，加 TODO、上下文占用、各轮概要（轮号、prompt 首行、结果、耗时、+/−）。",
               "inputSchema": obj(json!({"key": key}), &["key"])}),
        json!({"name": "get_timeline", "annotations": ro,
               "description": "一个 Agent 会话某几轮的工具调用与结果（最多 3 轮，太长时较早的工具调用会被省略）。",
               "inputSchema": obj(json!({"key": key, "turns": {"type": "string", "description": "轮号，如 \"3\" 或 \"2,3\"；或 \"last:2\" 表示最近 2 轮；最多 3 轮"}}), &["key", "turns"])}),
        json!({"name": "get_commands", "annotations": ro,
               "description": "一个终端（key 为 pane:<id>）最近的命令块：命令、目录、退出码、耗时、输出末尾。",
               "inputSchema": obj(json!({"key": key, "limit": {"type": "integer", "minimum": 1, "maximum": MAX_COMMANDS, "default": DEFAULT_COMMANDS}}), &["key"])}),
        json!({"name": "read_screen", "annotations": ro,
               "description": "一个 pane 的可见区与回滚区的最后 N 行文本。用户会在对话里看到你读了屏幕。",
               "inputSchema": obj(json!({"key": key, "lines": {"type": "integer", "minimum": 1, "maximum": MAX_SCREEN_LINES, "default": DEFAULT_SCREEN_LINES}}), &["key"])}),
    ]
}

/// `agent:<claude|codex>:<id>` or `pane:<digits>`, trimmed.
pub fn check_key(raw: &str) -> Result<String, String> {
    let k = raw.trim();
    let id_ok = |id: &str| !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    let ok = match k.split_once(':') {
        Some(("pane", id)) => !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()),
        Some(("agent", rest)) => matches!(rest.split_once(':'), Some(("claude" | "codex", id)) if id_ok(id)),
        _ => false,
    };
    if ok {
        Ok(k.to_string())
    } else {
        Err(format!("key 格式不对：{raw}（应为 agent:<claude|codex>:<id> 或 pane:<id>）"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnSel {
    /// These turn numbers (sorted, distinct).
    Turns(Vec<u32>),
    /// The newest n.
    Last(usize),
}

impl TurnSel {
    pub fn pick(&self, turns: &[Arc<Turn>]) -> Vec<Arc<Turn>> {
        match self {
            TurnSel::Last(n) => turns[turns.len().saturating_sub(*n)..].to_vec(),
            TurnSel::Turns(list) => turns.iter().filter(|t| list.contains(&t.index)).cloned().collect(),
        }
    }
}

fn parse_turns(v: &Value) -> Result<TurnSel, String> {
    let positive = |x: &Value| x.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0).ok_or_else(|| "turns 必须是正整数".to_string());
    let mut list: Vec<u32> = match v {
        Value::String(s) if s.trim().starts_with("last:") => {
            let n: usize = s.trim()["last:".len()..].trim().parse().map_err(|_| format!("turns 格式不对：{s}"))?;
            return if (1..=MAX_TIMELINE_TURNS).contains(&n) { Ok(TurnSel::Last(n)) } else { Err(format!("turns 最多 {MAX_TIMELINE_TURNS} 轮")) };
        }
        Value::String(s) => s.split(',').map(|p| p.trim().parse::<u32>().ok().filter(|n| *n > 0).ok_or_else(|| format!("turns 格式不对：{s}"))).collect::<Result<_, _>>()?,
        Value::Number(_) => vec![positive(v)?],
        Value::Array(a) => a.iter().map(positive).collect::<Result<_, _>>()?,
        _ => return Err("turns 必须是轮号（如 \"3\"、\"2,3\"）或 \"last:N\"".into()),
    };
    list.sort_unstable();
    list.dedup();
    match list.len() {
        0 => Err("turns 不能为空".into()),
        n if n > MAX_TIMELINE_TURNS => Err(format!("turns 最多 {MAX_TIMELINE_TURNS} 轮")),
        _ => Ok(TurnSel::Turns(list)),
    }
}

/// An optional integer argument in `1..=max`.
fn bounded(args: &Value, name: &str, default: usize, max: usize) -> Result<usize, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => match v.as_u64().map(|n| n as usize) {
            Some(n) if (1..=max).contains(&n) => Ok(n),
            _ => Err(format!("{name} 必须是 1 到 {max} 之间的整数")),
        },
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolCall {
    ListSessions,
    GetSession { key: String },
    GetTimeline { key: String, turns: TurnSel },
    GetCommands { key: String, limit: usize },
    ReadScreen { key: String, lines: usize },
}

/// A `tools/call`'s name (with or without Claude's prefix) and arguments, checked.
pub fn parse_call(tool: &str, args: &Value) -> Result<ToolCall, String> {
    let tool = tool.strip_prefix(CLAUDE_PREFIX).unwrap_or(tool);
    let empty = json!({});
    let args = if args.is_null() { &empty } else { args };
    if !args.is_object() {
        return Err("参数必须是一个对象".into());
    }
    let key = || args.get("key").and_then(Value::as_str).ok_or_else(|| "缺少 key".to_string()).and_then(check_key);
    match tool {
        "list_sessions" => Ok(ToolCall::ListSessions),
        "get_session" => Ok(ToolCall::GetSession { key: key()? }),
        "get_timeline" => {
            let key = key()?;
            let turns = parse_turns(args.get("turns").ok_or("缺少 turns")?)?;
            Ok(ToolCall::GetTimeline { key, turns })
        }
        "get_commands" => {
            let key = key()?;
            if !key.starts_with("pane:") {
                return Err("get_commands 只接受终端：pane:<id>".into());
            }
            Ok(ToolCall::GetCommands { key, limit: bounded(args, "limit", DEFAULT_COMMANDS, MAX_COMMANDS)? })
        }
        "read_screen" => Ok(ToolCall::ReadScreen { key: key()?, lines: bounded(args, "lines", DEFAULT_SCREEN_LINES, MAX_SCREEN_LINES)? }),
        other => Err(format!("没有这个工具：{other}")),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SummaryInfo {
    pub goal: Option<String>,
    pub recent: String,
    /// 「刚刚」 / 「3 分钟前」.
    pub age: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionInfo {
    pub key: String,
    /// "agent" | "terminal"
    pub kind: &'static str,
    /// "claude" | "codex"; None for terminals.
    pub agent: Option<&'static str>,
    pub name: String,
    /// The card wall's group: needs_you | error | running | done | idle | terminals | ended.
    pub group: &'static str,
    /// The status line as the card draws it.
    pub status: String,
    /// Window / tab / side, as the card draws it.
    pub location: String,
    pub cwd: Option<String>,
    pub git: Option<String>,
    pub summary: Option<SummaryInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TurnBrief {
    pub turn: u32,
    /// The prompt's first line.
    pub prompt: String,
    /// 完成 | 进行中 | 被中断 | 失败 …
    pub result: String,
    pub took: Option<String>,
    pub added: u32,
    pub removed: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionDetail {
    #[serde(flatten)]
    pub info: SessionInfo,
    /// 「2/5」.
    pub todo: Option<String>,
    /// 「42%」 of the context window.
    pub context: Option<String>,
    pub turns: Vec<TurnBrief>,
}

/// Where the answers come from (gilvt-app's main thread). [`Source::sessions`] leaves out excluded directories;
/// the other methods are only asked about keys it listed.
pub trait Source {
    fn sessions(&self) -> Vec<SessionInfo>;
    fn detail(&self, key: &str) -> Option<SessionDetail>;
    /// An agent session's turns, oldest first.
    fn turns(&self, key: &str) -> Option<Vec<Arc<Turn>>>;
    /// A terminal's newest `limit` commands, newest first (commands run in excluded directories left out).
    fn commands(&self, key: &str, limit: usize) -> Option<Vec<CommandDigest>>;
    /// The last `lines` lines of the pane showing `key`; None when no pane shows it.
    fn screen(&self, key: &str, lines: usize) -> Option<String>;
}

/// A successful call: the text the model reads (JSON with `gilvt_label`) and the label the chat shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolText {
    pub text: String,
    pub label: String,
}

fn answer(label: String, mut body: Value) -> ToolText {
    body["gilvt_label"] = json!(label);
    ToolText { text: body.to_string(), label }
}

/// An answer whose serialized text — the bytes the model reads, escapes included — is at most [`input::MAX_INPUT`].
/// `make(cap)` builds the label and body with its free text clipped to `cap` bytes; `cap` shrinks in proportion to
/// the overshoot until the whole fits.
fn fitted(make: impl Fn(usize) -> (String, Value)) -> ToolText {
    let mut cap = input::MAX_INPUT;
    loop {
        let (label, body) = make(cap);
        let out = answer(label, body);
        if out.text.len() <= input::MAX_INPUT || cap <= 256 {
            return out;
        }
        let scaled = (cap as f64 * input::MAX_INPUT as f64 / out.text.len() as f64 * 0.95) as usize;
        cap = scaled.min(cap - 1);
    }
}

/// A list answer: elements are removed (from the front when `from_front`, else the back) until the serialized
/// answer fits; the body then carries `"omitted": n`. `label(kept, omitted)` names the result.
fn fitted_list(label: impl Fn(usize, usize) -> String, build: impl Fn(&[Value]) -> Value, mut items: Vec<Value>, from_front: bool) -> ToolText {
    let mut omitted = 0;
    loop {
        let mut body = build(&items);
        if omitted > 0 {
            body["omitted"] = json!(omitted);
        }
        let out = answer(label(items.len(), omitted), body);
        if out.text.len() <= input::MAX_INPUT || items.is_empty() {
            return out;
        }
        // Drop in proportion to the overshoot (at least one), so a long list needs few rounds.
        let drop = (items.len() - (items.len() as f64 * input::MAX_INPUT as f64 / out.text.len() as f64 * 0.95) as usize).max(1).min(items.len());
        if from_front {
            items.drain(..drop);
        } else {
            items.truncate(items.len() - drop);
        }
        omitted += drop;
    }
}

/// Answers `call` from `src`. `Err` is the one-line reason the model gets with `isError: true`.
pub fn respond(call: &ToolCall, src: &dyn Source) -> Result<ToolText, String> {
    let sessions = src.sessions();
    let find = |key: &str| sessions.iter().find(|s| s.key == key).cloned().ok_or_else(|| format!("{NOT_FOUND}：{key}"));
    match call {
        ToolCall::ListSessions => {
            let items: Vec<Value> = sessions.iter().map(|s| json!(s)).collect();
            let label = |kept: usize, omitted: usize| match (english(), omitted) {
                (true, 0) => format!("Listed {}", sessions_count(kept)),
                (true, _) => format!("Listed {} ({omitted} more did not fit)", sessions_count(kept)),
                (false, 0) => format!("已列出 {kept} 个会话"),
                (false, _) => format!("已列出 {kept} 个会话（另有 {omitted} 个放不下）"),
            };
            Ok(fitted_list(label, |items| json!({"sessions": items}), items, false))
        }
        ToolCall::GetSession { key } => {
            let s = find(key)?;
            let d = src.detail(key).ok_or_else(|| format!("{NOT_FOUND}：{key}"))?;
            let session = json!(d);
            let items = session["turns"].as_array().cloned().unwrap_or_default();
            let build = |items: &[Value]| {
                let mut v = session.clone();
                v["turns"] = Value::Array(items.to_vec());
                json!({"session": v})
            };
            // The oldest turns go first.
            let label = |_, _| if english() { format!("Read the overview of {}", s.name) } else { format!("已读取 {} 的概况", s.name) };
            Ok(fitted_list(label, build, items, true))
        }
        ToolCall::GetTimeline { key, turns } => {
            let s = find(key)?;
            if s.kind != "agent" {
                return Err("get_timeline 只接受 Agent 会话（终端用 get_commands）".into());
            }
            let picked = turns.pick(&src.turns(key).unwrap_or_default());
            let (Some(first), Some(last)) = (picked.first(), picked.last()) else { return Err(format!("{} 没有这些轮", s.name)) };
            let tools: usize = picked.iter().map(|t| t.items.iter().filter(|i| matches!(i, Item::Tool(_))).count()).sum();
            let label = if english() {
                let range = if first.index == last.index { format!("turn {}", first.index) } else { format!("turns {}–{}", first.index, last.index) };
                let calls = if tools == 1 { "1 tool call".to_string() } else { format!("{tools} tool calls") };
                format!("Read the timeline of {}, {range} ({calls})", s.name)
            } else {
                let range = if first.index == last.index { format!("第 {} 轮", first.index) } else { format!("第 {}–{} 轮", first.index, last.index) };
                format!("已读取 {} {range}时间线（{tools} 个工具调用）", s.name)
            };
            Ok(fitted(|cap| (label.clone(), json!({"key": key, "timeline": input::timeline_text_within(&s.name, &picked, cap)}))))
        }
        ToolCall::GetCommands { key, limit } => {
            let s = find(key)?;
            let commands = src.commands(key, *limit).ok_or_else(|| format!("{NOT_FOUND}：{key}"))?;
            let cwd = s.cwd.as_ref().map(PathBuf::from);
            let digest = TerminalDigest { name: &s.name, cwd: cwd.as_deref(), commands: &commands };
            let n = commands.len().min(*limit);
            let label = if english() {
                let commands = if n == 1 { "command".to_string() } else { format!("{n} commands") };
                format!("Read the last {commands} of {}", s.name)
            } else {
                format!("已读取 {} 最近 {n} 条命令", s.name)
            };
            Ok(fitted(|cap| (label.clone(), json!({"key": key, "commands": input::commands_text_within(&digest, *limit, cap)}))))
        }
        ToolCall::ReadScreen { key, lines } => {
            let s = find(key)?;
            let screen = src.screen(key, *lines).ok_or_else(|| format!("{} 现在没有显示在任何 pane 里", s.name))?;
            Ok(fitted(|cap| {
                let screen = input::clip_bytes_tail(&screen, cap);
                let n = screen.lines().count();
                let label = if english() {
                    format!("Read the screen of {} (last {})", s.name, lines_count(n))
                } else {
                    format!("已读取 {} 的屏幕（最后 {n} 行）", s.name)
                };
                (label, json!({"key": key, "lines": n, "screen": screen}))
            }))
        }
    }
}

/// `1 session` / `N sessions`.
fn sessions_count(n: usize) -> String {
    if n == 1 { "1 session".into() } else { format!("{n} sessions") }
}

/// `1 line` / `N lines`.
fn lines_count(n: usize) -> String {
    if n == 1 { "1 line".into() } else { format!("{n} lines") }
}

/// The 「✓ …」 text of a finished call: the result's `gilvt_label`.
pub fn done_label(result: &str) -> Option<String> {
    serde_json::from_str::<Value>(result).ok()?.get("gilvt_label")?.as_str().map(str::to_string)
}

/// The 「… 正在 …」 text of a call in progress; `name_of` turns a key into the session's name.
pub fn pending_label(tool: &str, args: &Value, name_of: &dyn Fn(&str) -> Option<String>) -> String {
    let tool = tool.strip_prefix(CLAUDE_PREFIX).unwrap_or(tool);
    let name = |key: &str| name_of(key).unwrap_or_else(|| key.to_string());
    if english() {
        return match parse_call(tool, args) {
            Ok(ToolCall::ListSessions) => "Listing sessions…".into(),
            Ok(ToolCall::GetSession { key }) => format!("Reading the overview of {}…", name(&key)),
            Ok(ToolCall::GetTimeline { key, turns: TurnSel::Last(1) }) => {
                format!("Reading the timeline of {}, last turn…", name(&key))
            }
            Ok(ToolCall::GetTimeline { key, turns: TurnSel::Last(n) }) => {
                format!("Reading the timeline of {}, last {n} turns…", name(&key))
            }
            Ok(ToolCall::GetTimeline { key, turns: TurnSel::Turns(t) }) => {
                let list: Vec<String> = t.iter().map(u32::to_string).collect();
                let turns = if t.len() == 1 { "turn" } else { "turns" };
                format!("Reading the timeline of {}, {turns} {}…", name(&key), list.join(", "))
            }
            Ok(ToolCall::GetCommands { key, .. }) => format!("Reading the commands of {}…", name(&key)),
            Ok(ToolCall::ReadScreen { key, lines }) => {
                format!("Reading the screen of {} (last {})…", name(&key), lines_count(lines))
            }
            Err(_) => format!("Calling {tool}…"),
        };
    }
    match parse_call(tool, args) {
        Ok(ToolCall::ListSessions) => "正在列出会话…".into(),
        Ok(ToolCall::GetSession { key }) => format!("正在读取 {} 的概况…", name(&key)),
        Ok(ToolCall::GetTimeline { key, turns: TurnSel::Last(n) }) => format!("正在读取 {} 最近 {n} 轮时间线…", name(&key)),
        Ok(ToolCall::GetTimeline { key, turns: TurnSel::Turns(t) }) => {
            let list: Vec<String> = t.iter().map(u32::to_string).collect();
            format!("正在读取 {} 第 {} 轮时间线…", name(&key), list.join("、"))
        }
        Ok(ToolCall::GetCommands { key, .. }) => format!("正在读取 {} 的命令…", name(&key)),
        Ok(ToolCall::ReadScreen { key, lines }) => format!("正在读取 {} 的屏幕（最后 {lines} 行）…", name(&key)),
        Err(_) => format!("正在调用 {tool}…"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_agent::{ItemStatus, ToolItem, TurnOutcome};
    use serde_json::json;

    fn turn(index: u32, tools: usize) -> Arc<Turn> {
        let mut t = Turn::new_for_test(&format!("第 {index} 轮"));
        t.index = index;
        t.outcome = TurnOutcome::Done;
        t.items = (0..tools)
            .map(|i| {
                let mut x = ToolItem::new(&format!("t{i}"), "Bash");
                x.summary = format!("cmd {i}");
                x.status = ItemStatus::Ok;
                Item::Tool(x)
            })
            .collect();
        Arc::new(t)
    }

    fn info(key: &str, name: &str, kind: &'static str) -> SessionInfo {
        SessionInfo {
            key: key.into(),
            kind,
            agent: (kind == "agent").then_some("claude"),
            name: name.into(),
            group: if kind == "agent" { "needs_you" } else { "terminals" },
            status: "⏳ 等待审批".into(),
            location: "web · 右".into(),
            cwd: Some("/w/web".into()),
            git: None,
            summary: None,
        }
    }

    /// `pane:9` is a terminal; `agent:claude:a1` has 5 turns; the excluded session is simply not listed.
    struct Fake {
        screen: String,
    }

    impl Source for Fake {
        fn sessions(&self) -> Vec<SessionInfo> {
            vec![info("agent:claude:a1", "web-login", "agent"), info("pane:9", "zsh", "terminal")]
        }
        fn detail(&self, key: &str) -> Option<SessionDetail> {
            let info = self.sessions().into_iter().find(|s| s.key == key)?;
            Some(SessionDetail { info, todo: Some("2/5".into()), context: None, turns: vec![] })
        }
        fn turns(&self, key: &str) -> Option<Vec<Arc<Turn>>> {
            (key == "agent:claude:a1").then(|| (1..=5).map(|i| turn(i, i as usize)).collect())
        }
        fn commands(&self, key: &str, limit: usize) -> Option<Vec<CommandDigest>> {
            (key == "pane:9").then(|| {
                (0..30).take(limit).map(|i| CommandDigest { command: format!("make t{i}"), cwd: None, exit: Some(0), running: false, took: None, output_tail: None }).collect()
            })
        }
        fn screen(&self, key: &str, _lines: usize) -> Option<String> {
            (key == "pane:9").then(|| self.screen.clone())
        }
    }

    fn fake() -> Fake {
        Fake { screen: "sandbox$ make test\nok".into() }
    }

    #[test]
    fn specs_are_the_five_read_only_tools() {
        let specs = specs();
        let names: Vec<&str> = specs.iter().map(|s| s["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["list_sessions", "get_session", "get_timeline", "get_commands", "read_screen"]);
        for s in &specs {
            assert_eq!(s["annotations"]["readOnlyHint"], true);
            assert_eq!(s["inputSchema"]["type"], "object");
        }
        assert_eq!(specs[2]["inputSchema"]["properties"]["turns"]["type"], "string");
    }

    #[test]
    fn keys_are_checked() {
        assert_eq!(check_key(" pane:12 ").unwrap(), "pane:12");
        assert!(check_key("agent:codex:019a-bc_d").is_ok());
        for bad in ["", "pane:", "pane:x", "agent:claude:", "agent:gemini:1", "agent:claude:a b", "../etc"] {
            assert!(check_key(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn turn_selections() {
        let t = |v: Value| parse_call("get_timeline", &json!({"key": "agent:claude:a1", "turns": v}));
        let sel = |v: Value| match t(v) {
            Ok(ToolCall::GetTimeline { turns, .. }) => Ok(turns),
            Ok(other) => panic!("{other:?}"),
            Err(e) => Err(e),
        };
        assert_eq!(sel(json!("last:2")), Ok(TurnSel::Last(2)));
        assert_eq!(sel(json!("3")), Ok(TurnSel::Turns(vec![3])));
        assert_eq!(sel(json!("3, 2")), Ok(TurnSel::Turns(vec![2, 3])));
        assert_eq!(sel(json!([4])), Ok(TurnSel::Turns(vec![4])));
        assert_eq!(sel(json!(5)), Ok(TurnSel::Turns(vec![5])));
        for bad in [json!("last:4"), json!("1,2,3,4"), json!([0]), json!("x"), json!(null), json!({})] {
            assert!(sel(bad.clone()).is_err(), "{bad}");
        }
        let turns: Vec<Arc<Turn>> = (1..=5).map(|i| turn(i, 0)).collect();
        assert_eq!(TurnSel::Last(2).pick(&turns).iter().map(|t| t.index).collect::<Vec<_>>(), [4, 5]);
        assert_eq!(TurnSel::Turns(vec![1, 9]).pick(&turns).iter().map(|t| t.index).collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn argument_limits() {
        let c = |tool: &str, args: Value| parse_call(tool, &args);
        assert_eq!(c("get_commands", json!({"key": "pane:9"})), Ok(ToolCall::GetCommands { key: "pane:9".into(), limit: DEFAULT_COMMANDS }));
        assert!(c("get_commands", json!({"key": "pane:9", "limit": 21})).is_err());
        assert!(c("get_commands", json!({"key": "pane:9", "limit": 0})).is_err());
        assert!(c("get_commands", json!({"key": "agent:claude:a1"})).is_err(), "terminals only");
        assert_eq!(c("read_screen", json!({"key": "pane:9"})), Ok(ToolCall::ReadScreen { key: "pane:9".into(), lines: DEFAULT_SCREEN_LINES }));
        assert!(c("read_screen", json!({"key": "pane:9", "lines": 201})).is_err());
        assert!(c("read_screen", json!({"key": "pane:9", "lines": "60"})).is_err());
        assert!(c("get_session", json!({})).is_err(), "key is required");
        assert!(c("get_session", json!([1])).is_err(), "arguments are an object");
        assert_eq!(c("mcp__gilvt__list_sessions", Value::Null), Ok(ToolCall::ListSessions), "Claude's prefix; no arguments");
        assert!(c("run_in_pane", json!({})).is_err(), "no write tools");
    }

    #[test]
    fn list_and_session() {
        let out = respond(&ToolCall::ListSessions, &fake()).unwrap();
        assert_eq!(out.label, "已列出 2 个会话");
        let v: Value = serde_json::from_str(&out.text).unwrap();
        assert_eq!(v["gilvt_label"], "已列出 2 个会话");
        assert_eq!(v["sessions"][0]["key"], "agent:claude:a1");
        assert_eq!(v["sessions"][1]["group"], "terminals");
        let out = respond(&ToolCall::GetSession { key: "agent:claude:a1".into() }, &fake()).unwrap();
        assert_eq!(out.label, "已读取 web-login 的概况");
        let v: Value = serde_json::from_str(&out.text).unwrap();
        assert_eq!(v["session"]["name"], "web-login", "flattened");
        assert_eq!(v["session"]["todo"], "2/5");
        assert_eq!(done_label(&out.text).as_deref(), Some("已读取 web-login 的概况"));
        assert_eq!(done_label("not json"), None);
    }

    #[test]
    fn excluded_and_missing_sessions_look_the_same() {
        let missing = respond(&ToolCall::GetSession { key: "agent:claude:zz".into() }, &fake()).unwrap_err();
        assert_eq!(missing, "没有这个会话：agent:claude:zz");
        // An excluded session is one the source does not list, even if it could read it.
        let excluded = respond(&ToolCall::ReadScreen { key: "pane:99".into(), lines: 10 }, &fake()).unwrap_err();
        assert_eq!(excluded, "没有这个会话：pane:99");
    }

    #[test]
    fn labels_in_english() {
        use gilvt_i18n::{has_chinese, with_language, Language};
        let name = |k: &str| (k == "agent:claude:a1").then(|| "web-login".to_string());
        let (done, pending) = with_language(Language::English, || {
            let calls = [
                ToolCall::ListSessions,
                ToolCall::GetSession { key: "agent:claude:a1".into() },
                ToolCall::GetTimeline { key: "agent:claude:a1".into(), turns: TurnSel::Last(2) },
                ToolCall::GetTimeline { key: "agent:claude:a1".into(), turns: TurnSel::Turns(vec![3]) },
                ToolCall::GetCommands { key: "pane:9".into(), limit: 20 },
                ToolCall::ReadScreen { key: "pane:9".into(), lines: 2 },
            ];
            let done: Vec<String> = calls.iter().map(|c| done_label(&respond(c, &fake()).unwrap().text).unwrap()).collect();
            let pending: Vec<String> = [
                ("mcp__gilvt__list_sessions", json!({})),
                ("get_session", json!({"key": "agent:claude:a1"})),
                ("get_timeline", json!({"key": "agent:claude:a1", "turns": "last:2"})),
                ("get_timeline", json!({"key": "agent:claude:a1", "turns": "2,3"})),
                ("get_commands", json!({"key": "pane:9"})),
                ("read_screen", json!({"key": "pane:9"})),
                ("frob", json!({})),
            ]
            .iter()
            .map(|(tool, args)| pending_label(tool, args, &name))
            .collect();
            (done, pending)
        });
        assert_eq!(done[0], "Listed 2 sessions");
        assert_eq!(done[2], "Read the timeline of web-login, turns 4–5 (9 tool calls)");
        assert_eq!(done[3], "Read the timeline of web-login, turn 3 (3 tool calls)");
        assert_eq!(pending[0], "Listing sessions…");
        assert_eq!(pending[3], "Reading the timeline of web-login, turns 2, 3…");
        assert_eq!(pending[5], "Reading the screen of pane:9 (last 60 lines)…");
        for label in done.iter().chain(&pending) {
            assert!(!has_chinese(label), "{label}");
        }
        assert_eq!(pending_label("mcp__gilvt__list_sessions", &json!({}), &name), "正在列出会话…");
        assert_eq!(with_language(Language::English, || crate::input::Covers::Turns(2, 3).label()), "Covers turns 2–3");
    }

    #[test]
    fn timeline_label_counts_tool_calls() {
        let out = respond(&ToolCall::GetTimeline { key: "agent:claude:a1".into(), turns: TurnSel::Last(2) }, &fake()).unwrap();
        assert_eq!(out.label, "已读取 web-login 第 4–5 轮时间线（9 个工具调用）");
        let one = respond(&ToolCall::GetTimeline { key: "agent:claude:a1".into(), turns: TurnSel::Turns(vec![3]) }, &fake()).unwrap();
        assert_eq!(one.label, "已读取 web-login 第 3 轮时间线（3 个工具调用）");
        assert!(respond(&ToolCall::GetTimeline { key: "agent:claude:a1".into(), turns: TurnSel::Turns(vec![9]) }, &fake()).is_err());
        assert!(respond(&ToolCall::GetTimeline { key: "pane:9".into(), turns: TurnSel::Last(1) }, &fake()).is_err(), "terminals have no timeline");
    }

    #[test]
    fn commands_and_screen() {
        let out = respond(&ToolCall::GetCommands { key: "pane:9".into(), limit: 20 }, &fake()).unwrap();
        assert_eq!(out.label, "已读取 zsh 最近 20 条命令");
        assert!(out.text.contains("make t19") && !out.text.contains("make t20"));
        let out = respond(&ToolCall::ReadScreen { key: "pane:9".into(), lines: 60 }, &fake()).unwrap();
        assert_eq!(out.label, "已读取 zsh 的屏幕（最后 2 行）");
        assert!(respond(&ToolCall::ReadScreen { key: "agent:claude:a1".into(), lines: 60 }, &fake()).unwrap_err().contains("没有显示"));
    }

    #[test]
    fn huge_screens_are_clipped() {
        let src = Fake { screen: "宽".repeat(100_000) };
        let out = respond(&ToolCall::ReadScreen { key: "pane:9".into(), lines: 200 }, &src).unwrap();
        assert!(out.text.len() <= input::MAX_INPUT, "{}", out.text.len());
    }

    #[test]
    fn pending_labels_name_the_session() {
        let name = |k: &str| (k == "agent:claude:a1").then(|| "web-login".to_string());
        assert_eq!(pending_label("mcp__gilvt__list_sessions", &json!({}), &name), "正在列出会话…");
        assert_eq!(pending_label("get_timeline", &json!({"key": "agent:claude:a1", "turns": "last:2"}), &name), "正在读取 web-login 最近 2 轮时间线…");
        assert_eq!(pending_label("get_timeline", &json!({"key": "agent:claude:a1", "turns": "2,3"}), &name), "正在读取 web-login 第 2、3 轮时间线…");
        assert_eq!(pending_label("read_screen", &json!({"key": "pane:9"}), &name), "正在读取 pane:9 的屏幕（最后 60 行）…");
        assert_eq!(pending_label("frob", &json!({}), &name), "正在调用 frob…");
    }

    /// A source whose answers are as big and as escape-heavy as they come.
    struct Big;

    impl Source for Big {
        fn sessions(&self) -> Vec<SessionInfo> {
            (0..200)
                .map(|i| {
                    let mut s = if i == 0 { info("agent:claude:a1", "web-login", "agent") } else { info(&format!("pane:{i}"), &format!("zsh-{i}"), "terminal") };
                    s.summary = Some(SummaryInfo { goal: None, recent: "近".repeat(300), age: "刚刚".into() });
                    s
                })
                .collect()
        }
        fn detail(&self, _key: &str) -> Option<SessionDetail> {
            let turns = (1..=500)
                .map(|i| TurnBrief { turn: i, prompt: "改一下登录，让它支持 SSO".into(), result: "完成".into(), took: Some("2 分钟".into()), added: 3, removed: 1 })
                .collect();
            Some(SessionDetail { info: info("agent:claude:a1", "web-login", "agent"), todo: None, context: None, turns })
        }
        fn turns(&self, _key: &str) -> Option<Vec<Arc<Turn>>> {
            let mut t = Turn::new_for_test("say \"hi\"\nthen \"bye\"");
            t.index = 1;
            t.items = (0..300)
                .map(|i| {
                    let mut x = ToolItem::new(&format!("t{i}"), "Bash");
                    x.summary = format!("echo \"q{i}\" \\ \"{}\"", "\"\\".repeat(40));
                    x.status = ItemStatus::Ok;
                    x.error_excerpt = vec!["\"line\" one".into(), "line \"two\"".into()];
                    Item::Tool(x)
                })
                .collect();
            Some(vec![Arc::new(t)])
        }
        fn commands(&self, _key: &str, limit: usize) -> Option<Vec<CommandDigest>> {
            Some(
                (0..limit)
                    .map(|i| {
                        let tail = format!("{}\nEND-MARKER-{i}", "\"\\\n".repeat(2000));
                        CommandDigest { command: format!("echo \"{i}\""), cwd: None, exit: Some(0), running: false, took: None, output_tail: Some(tail) }
                    })
                    .collect(),
            )
        }
        fn screen(&self, _key: &str, _lines: usize) -> Option<String> {
            Some(format!("{}LAST-LINE", "\x1b[31mred \"q\"\x1b[0m\n".repeat(5000)))
        }
    }

    fn within(out: &ToolText) -> Value {
        assert!(out.text.len() <= input::MAX_INPUT, "{} bytes", out.text.len());
        serde_json::from_str(&out.text).expect("still JSON")
    }

    #[test]
    fn a_long_session_list_drops_its_tail_and_says_so() {
        let out = respond(&ToolCall::ListSessions, &Big).unwrap();
        let v = within(&out);
        let kept = v["sessions"].as_array().unwrap().len();
        assert!(kept > 0 && kept < 200, "{kept}");
        assert_eq!(v["omitted"], 200 - kept);
        assert_eq!(v["sessions"][0]["key"], "agent:claude:a1", "the head stays");
        assert!(out.label.contains(&format!("已列出 {kept} 个会话")), "{}", out.label);
    }

    #[test]
    fn a_session_with_500_turns_drops_the_oldest() {
        let out = respond(&ToolCall::GetSession { key: "agent:claude:a1".into() }, &Big).unwrap();
        let v = within(&out);
        let turns = v["session"]["turns"].as_array().unwrap();
        assert!(!turns.is_empty() && turns.len() < 500, "{}", turns.len());
        assert_eq!(v["omitted"], 500 - turns.len());
        assert_eq!(turns.last().unwrap()["turn"], 500, "the newest stays");
        assert_eq!(v["session"]["name"], "web-login");
    }

    #[test]
    fn quote_and_newline_heavy_timelines_fit_once_escaped() {
        let out = respond(&ToolCall::GetTimeline { key: "agent:claude:a1".into(), turns: TurnSel::Last(1) }, &Big).unwrap();
        let v = within(&out);
        let text = v["timeline"].as_str().unwrap();
        assert!(text.contains("已省略"), "older calls were dropped");
        assert!(text.contains("q299"), "the newest calls stay");
    }

    #[test]
    fn command_output_full_of_quotes_fits_and_keeps_its_end() {
        let out = respond(&ToolCall::GetCommands { key: "pane:1".into(), limit: 20 }, &Big).unwrap();
        let v = within(&out);
        let text = v["commands"].as_str().unwrap();
        assert!(text.contains("END-MARKER-0"), "the newest output keeps its end");
    }

    #[test]
    fn screens_with_control_characters_fit_and_keep_the_last_line() {
        let out = respond(&ToolCall::ReadScreen { key: "pane:1".into(), lines: 200 }, &Big).unwrap();
        let v = within(&out);
        assert!(v["screen"].as_str().unwrap().ends_with("LAST-LINE"));
        assert_eq!(v["lines"].as_u64().unwrap() as usize, v["screen"].as_str().unwrap().lines().count());
    }
}
