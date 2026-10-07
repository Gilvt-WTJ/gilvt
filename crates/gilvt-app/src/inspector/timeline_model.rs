//! The 「过程」 tab's timeline as pure data (M3b spec §3.4–§3.6, §5; mockup m3b-part3.html): the current
//! turn's rows under a filter, expanded details, subagent nesting, the history turns, and what a click on a
//! row does (`entries`: the list built from them). `timeline_view` draws it; `timeline_list` keeps it in a virtualized gpui list.

use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gilvt_agent::{Anchor, Item, ItemStatus, ToolItem, Turn};

use super::model::elapsed_label;

/// 全部 / Bash / 编辑 / 失败.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Filter {
    #[default]
    All,
    Bash,
    Edit,
    Failed,
}

impl Filter {
    pub const ALL: [Filter; 4] = [Filter::All, Filter::Bash, Filter::Edit, Filter::Failed];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => crate::i18n::text("全部", "All"),
            Filter::Bash => "Bash",
            Filter::Edit => crate::i18n::text("编辑", "Edits"),
            Filter::Failed => crate::i18n::text("失败", "Failed"),
        }
    }

    /// Whether a tool call passes the filter on its own (a subagent row also passes when a child does).
    pub fn matches(self, t: &ToolItem) -> bool {
        match self {
            Filter::All => true,
            Filter::Bash => is_shell(&t.tool),
            Filter::Edit => is_edit(&t.tool),
            Filter::Failed => matches!(t.status, ItemStatus::Failed { .. }),
        }
    }
}

/// Claude Bash, Codex shell / exec_command (and Codex's other shell tool names).
pub fn is_shell(tool: &str) -> bool {
    matches!(tool, "Bash" | "shell" | "exec_command" | "local_shell" | "container.exec" | "unified_exec")
}

/// Edit, Write, MultiEdit, NotebookEdit, apply_patch.
pub fn is_edit(tool: &str) -> bool {
    matches!(tool, "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "apply_patch")
}

fn is_subagent(t: &ToolItem) -> bool {
    t.subagent.is_some() || matches!(t.tool.as_str(), "Task" | "Agent" | "spawn_agent")
}

/// Text / icon colors of a row (the view maps them to the theme).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    Text,
    Running,
    Failed,
    Warn,
    Subagent,
    Muted,
}

/// A row's time: a finished call's duration, or a running one's start (ticks while drawn).
#[derive(Clone, Debug, PartialEq)]
pub enum Timing {
    None,
    Running(Option<SystemTime>),
    Took(String),
}

impl Timing {
    /// "0.1s", "21s", "12s…" (running: whole seconds + "…").
    pub fn label(&self, now: SystemTime) -> String {
        match self {
            Timing::None => String::new(),
            Timing::Running(None) => "…".into(),
            Timing::Running(Some(t)) => format!("{}…", elapsed_label(now.duration_since(*t).unwrap_or_default())),
            Timing::Took(s) => s.clone(),
        }
    }
}

/// A finished call's duration: "0.1s", "3.2s" under 10 s, then "21s", "1m12s".
pub fn duration_label(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 10.0 {
        let text = format!("{:.1}", (s * 10.0).round() / 10.0);
        let text = if text == "10.0" { "10" } else { text.strip_suffix(".0").unwrap_or(&text) };
        format!("{text}s")
    } else {
        elapsed_label(d)
    }
}

/// " · exit 1" / " · 已拒绝" / " · 已中断" / " · 待审批" part of a row (None: running or ok).
pub fn status_suffix(status: &ItemStatus) -> Option<String> {
    match status {
        ItemStatus::Failed { exit: Some(code) } => Some(format!("exit {code}")),
        ItemStatus::Failed { exit: None } => Some(crate::i18n::text("失败", "Failed").into()),
        ItemStatus::Denied => Some(crate::i18n::text("已拒绝", "Denied").into()),
        ItemStatus::Interrupted => Some(crate::i18n::text("已中断", "Interrupted").into()),
        ItemStatus::Pending => Some(crate::i18n::text("待审批", "Awaiting approval").into()),
        ItemStatus::Running | ItemStatus::Ok => None,
    }
}

/// "+18" / "−2" labels of an edit (zero parts omitted).
pub fn lines_labels(lines: Option<(u32, u32)>) -> (Option<String>, Option<String>) {
    match lines {
        Some((a, r)) => ((a > 0).then(|| format!("+{a}")), (r > 0).then(|| format!("−{r}"))),
        None => (None, None),
    }
}

/// The row's icon and its color: status first (▶ running, ✗ failed, ⊘ denied / interrupted, ⏳ pending),
/// else by tool.
pub fn icon(t: &ToolItem) -> (&'static str, Ink) {
    let sub = is_subagent(t);
    match t.status {
        ItemStatus::Failed { .. } => return ("✗", Ink::Failed),
        ItemStatus::Denied | ItemStatus::Interrupted => return ("⊘", Ink::Warn),
        ItemStatus::Pending => return ("⏳", Ink::Warn),
        ItemStatus::Running if !sub => return ("▶", Ink::Running),
        _ => {}
    }
    let glyph = match t.tool.as_str() {
        _ if sub => return ("⎇", Ink::Subagent),
        "Read" | "NotebookRead" | "view_image" => "📖",
        _ if is_edit(&t.tool) => "✏️",
        "Grep" | "Glob" | "LS" => "🔎",
        _ if is_shell(&t.tool) => "$",
        "WebFetch" | "WebSearch" | "web_search" => "🌐",
        "TodoWrite" | "TaskCreate" | "TaskUpdate" | "update_plan" => "☑",
        "AskUserQuestion" => "?",
        _ => "•",
    };
    (glyph, Ink::Text)
}

/// What a row calls the tool: Claude's TUI verb for Edit (「Update」, as in the terminal), else the name.
pub fn verb(t: &ToolItem) -> String {
    let name = match t.tool.as_str() {
        "Edit" | "MultiEdit" => "Update",
        other => other,
    };
    match t.subagent.as_ref().and_then(|s| s.agent_type.as_deref()).filter(|a| !a.is_empty()) {
        Some(kind) => format!("{name} · {kind}"),
        None => name.to_string(),
    }
}

/// The file a row names (the ⌘+click target): Read / Edit / Write / NotebookEdit's path argument, or the
/// first file of an apply_patch.
pub fn file_target(t: &ToolItem) -> Option<String> {
    let arg = |keys: &[&str]| keys.iter().find_map(|k| t.detail.input.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone()));
    let path = match t.tool.as_str() {
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "NotebookRead" | "view_image" => {
            arg(&["file_path", "notebook_path", "path"])
        }
        "apply_patch" => arg(&["input", "command", "patch"]).and_then(|p| {
            p.lines().find_map(|l| {
                ["*** Add File: ", "*** Update File: ", "*** Delete File: "].iter().find_map(|pre| l.strip_prefix(pre)).map(|s| s.trim().to_string())
            })
        }),
        _ => None,
    }?;
    let path = path.trim().trim_end_matches('…');
    (!path.is_empty()).then(|| path.to_string())
}

/// A row's file resolved for Quick Look: absolute as is, `~/…` under `home`, else relative to the
/// session's cwd (None without one).
pub fn resolve_path(raw: &str, cwd: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    let p = Path::new(raw);
    if p.is_absolute() {
        return Some(p.to_path_buf());
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.map(|h| h.join(rest));
    }
    cwd.map(|c| c.join(p))
}

/// One line of an expanded detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailLine {
    /// "key: first line of the value".
    Input { key: String, value: String },
    /// Further lines of a multi-line value.
    More(String),
    Output(String),
}

/// The expanded detail of a call: its arguments (key by key, continuation lines indented) and ≤ 20
/// output lines.
pub fn detail_lines(t: &ToolItem) -> Vec<DetailLine> {
    let mut out = Vec::new();
    for (key, value) in &t.detail.input {
        let mut lines = value.lines();
        out.push(DetailLine::Input { key: key.clone(), value: lines.next().unwrap_or("").to_string() });
        out.extend(lines.map(|l| DetailLine::More(l.to_string())));
    }
    out.extend(t.detail.output.iter().take(20).map(|l| DetailLine::Output(l.clone())));
    out
}

/// The detail as plain text (the copy button).
pub fn detail_text(lines: &[DetailLine]) -> String {
    lines
        .iter()
        .map(|l| match l {
            DetailLine::Input { key, value } => format!("{key}: {value}"),
            DetailLine::More(s) => format!("  {s}"),
            DetailLine::Output(s) => s.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A tool call's row.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolRow {
    pub icon: &'static str,
    pub icon_ink: Ink,
    /// "Bash", "Update", "Task · Explore".
    pub verb: String,
    /// The M3a summary ("go test ./...", "handler.go").
    pub arg: String,
    /// The raw path `arg` names (⌘+click → Quick Look).
    pub file: Option<String>,
    pub added: Option<String>,
    pub removed: Option<String>,
    /// "exit 1", "已拒绝", …
    pub suffix: Option<String>,
    /// The summary's color: failed red, denied / interrupted / pending yellow, subagent purple, no anchor grey.
    pub ink: Ink,
    pub timing: Timing,
    /// Key error lines shown under a failed row.
    pub error: Vec<String>,
    pub anchor: Option<Anchor>,
    /// What the call's line in the terminal shows, alternatives in order (see [`jump_needles`]); empty: jump
    /// to the anchor itself.
    pub needles: Vec<Vec<String>>,
    /// Some while expanded.
    pub detail: Option<Vec<DetailLine>>,
    /// The call's state (`gilvt debug state`).
    pub status: ItemStatus,
    /// A Task / Agent call.
    pub subagent: bool,
    /// An edit's (added, removed) line counts.
    pub lines: Option<(u32, u32)>,
}

/// A tool row's one-line text and where its styled parts are (byte ranges): the file name (underlined,
/// ⌘+click), the added / removed line counts (green / red). The status (「· exit 1」) is apart: it stays
/// visible when the text is cut to the column's width.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub text: String,
    pub file: Option<Range<usize>>,
    pub added: Option<Range<usize>>,
    pub removed: Option<Range<usize>>,
    /// 「· exit 1」, 「· 已拒绝」.
    pub status: Option<String>,
}

/// 「Update handler.go +18 −2」, 「Bash go test ./internal/...」 + 「· exit 1」, 「Bash rm -rf build」 + 「· 已拒绝」.
pub fn summary(r: &ToolRow) -> Summary {
    let mut s = Summary { text: r.verb.clone(), ..Summary::default() };
    let push = |s: &mut Summary, sep: &str, part: &str| -> Range<usize> {
        s.text.push_str(sep);
        let start = s.text.len();
        s.text.push_str(part);
        start..s.text.len()
    };
    if !r.arg.is_empty() {
        let range = push(&mut s, " ", &r.arg);
        s.file = r.file.as_ref().map(|_| range);
    }
    if let Some(a) = &r.added {
        s.added = Some(push(&mut s, " ", a));
    }
    if let Some(m) = &r.removed {
        s.removed = Some(push(&mut s, " ", m));
    }
    s.status = r.suffix.as_ref().map(|x| format!("· {x}"));
    s
}

#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    Tool(ToolRow),
    /// 「✻ 思考 · 4s」; `text` while expanded (empty when the block's text was not recorded).
    Thinking { secs: Option<String>, expandable: bool, text: Option<Vec<String>> },
    /// 「↩ <subagent result>」.
    Returned(String),
    /// 「另有 N 条」.
    Truncated(usize),
    /// A muted note inside an expanded history turn (「该轮的明细已不再保留」).
    Empty(&'static str),
}

/// One line of the timeline (plus what hangs under it: error excerpt, expanded detail).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Expand-state key: the tool-use id, `think:<parent>:<n>` for thinking blocks, `more:<parent>` / `ret:<id>`.
    pub key: String,
    /// Inside a subagent (purple rule).
    pub sub: bool,
    /// Inside an expanded history turn (indented).
    pub history: bool,
    pub kind: RowKind,
}

/// Which details are expanded, by row key, and which history turns, by turn index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expanded {
    pub rows: HashSet<String>,
    pub turns: HashSet<u32>,
}

impl Expanded {
    pub fn toggle_row(&mut self, key: &str) {
        if !self.rows.remove(key) {
            self.rows.insert(key.to_string());
        }
    }

    pub fn toggle_turn(&mut self, index: u32) {
        if !self.turns.remove(&index) {
            self.turns.insert(index);
        }
    }
}

fn tool_row(t: &ToolItem, open: &Expanded) -> ToolRow {
    let (icon, icon_ink) = icon(t);
    let ink = match t.status {
        ItemStatus::Failed { .. } => Ink::Failed,
        ItemStatus::Denied | ItemStatus::Interrupted | ItemStatus::Pending => Ink::Warn,
        _ if is_subagent(t) => Ink::Subagent,
        _ if t.anchor.is_none() => Ink::Muted,
        _ => Ink::Text,
    };
    let timing = match (&t.status, t.started, t.ended) {
        (s, started, _) if s.is_open() => Timing::Running(started),
        (_, Some(a), Some(b)) => b.duration_since(a).map_or(Timing::None, |d| Timing::Took(duration_label(d))),
        _ => Timing::None,
    };
    let file = file_target(t).filter(|_| !t.summary.is_empty());
    let (added, removed) = lines_labels(t.lines);
    ToolRow {
        icon,
        icon_ink,
        verb: verb(t),
        arg: t.summary.clone(),
        file,
        added,
        removed,
        suffix: status_suffix(&t.status),
        ink,
        timing,
        error: if matches!(t.status, ItemStatus::Failed { .. }) { t.error_excerpt.clone() } else { Vec::new() },
        anchor: t.anchor,
        needles: jump_needles(&t.tool, &t.summary),
        detail: open.rows.contains(&t.id).then(|| detail_lines(t)),
        status: t.status.clone(),
        subagent: is_subagent(t),
        lines: t.lines,
    }
}

/// Characters of a call's summary looked for when jumping: enough to tell calls apart, short enough to sit on
/// the first row of the agent's (wrapped) line for the call, e.g. 「● Bash(python3 -m unittest -v)」.
const NEEDLE_CHARS: usize = 24;

/// What a jump looks for above the anchor: alternatives, tried in order, each a set of texts that must all be on
/// one line. For Claude's tools (named in CamelCase) the verb its TUI shows the call under (「Update(」 for an
/// Edit) and the start of the summary (a command, a file name) without a trailing 「…」; a Read Claude folded
/// into 「Read 2 files」 is found by that line next. Codex's lines (「• Ran …」) have no such verb. Empty when the
/// summary is too short to be telling (< 3 chars).
pub fn jump_needles(tool: &str, summary: &str) -> Vec<Vec<String>> {
    let s = summary.trim().trim_end_matches('…').trim_end();
    let text: String = s.chars().take(NEEDLE_CHARS).collect();
    if text.chars().count() < 3 {
        return Vec::new();
    }
    match claude_tui_verb(tool) {
        Some("Read") => vec![vec!["Read(".into(), text], vec!["Read ".into(), " file".into()]],
        Some(verb) => vec![vec![format!("{verb}("), text]],
        None => vec![vec![text]],
    }
}

/// The name Claude Code's TUI shows a tool call under (「● Update(src/lib.rs)」); None for Codex's tools.
fn claude_tui_verb(tool: &str) -> Option<&str> {
    if !tool.starts_with(|c: char| c.is_ascii_uppercase()) {
        return None;
    }
    Some(match tool {
        "Edit" | "MultiEdit" => "Update",
        "Grep" | "Glob" => "Search",
        "Task" => "Agent",
        "WebFetch" => "Fetch",
        "WebSearch" => "Web Search",
        other => other,
    })
}

fn thinking_row(key: String, secs: Option<f32>, text: &[String], open: &Expanded, sub: bool, history: bool) -> Row {
    let secs = secs.filter(|s| s.is_finite() && *s >= 0.0).map(|s| duration_label(Duration::from_secs_f32(s)));
    let expanded = !text.is_empty() && open.rows.contains(&key);
    let lines = expanded.then(|| text.iter().take(20).cloned().collect());
    Row { key, sub, history, kind: RowKind::Thinking { secs, expandable: !text.is_empty(), text: lines } }
}

/// The rows of `items` (a turn's, or a subagent's children when `sub`). Filters other than 全部 drop
/// thinking rows and the 「↩」 line; a subagent row stays when it or one of its children matches, showing
/// only the matching children.
fn push_items(out: &mut Vec<Row>, items: &[Item], parent: &str, filter: Filter, open: &Expanded, sub: bool, history: bool) {
    // Newest first: the latest item is the topmost row, so the user catches up without scrolling.
    for (n, item) in items.iter().enumerate().rev() {
        match item {
            Item::Tool(t) => {
                let mut children = Vec::new();
                if !t.children.is_empty() {
                    push_items(&mut children, &t.children, &t.id, filter, open, true, history);
                }
                let child_tools = children.iter().any(|r| matches!(r.kind, RowKind::Tool(_)));
                if !filter.matches(t) && !child_tools {
                    continue;
                }
                out.push(Row { key: t.id.clone(), sub, history, kind: RowKind::Tool(tool_row(t, open)) });
                // A subagent's reply is its latest event: right under the header, above its children.
                let result = t.subagent.as_ref().and_then(|s| s.result.as_deref()).filter(|r| !r.is_empty());
                if let (Some(result), Filter::All) = (result, filter) {
                    out.push(Row { key: format!("ret:{}", t.id), sub: true, history, kind: RowKind::Returned(result.to_string()) });
                }
                out.extend(children);
            }
            Item::Thinking { secs, text } if filter == Filter::All => {
                out.push(thinking_row(format!("think:{parent}:{n}"), *secs, text, open, sub, history));
            }
            Item::Thinking { .. } => {}
            Item::Truncated { hidden } => {
                out.push(Row { key: format!("more:{parent}"), sub, history, kind: RowKind::Truncated(*hidden) });
            }
        }
    }
}

/// A turn's rows under `filter`, with the expanded details open.
pub fn turn_rows(turn: &Turn, filter: Filter, open: &Expanded, history: bool) -> Vec<Row> {
    let mut out = Vec::new();
    push_items(&mut out, &turn.items, &format!("turn{}", turn.index), filter, open, false, history);
    out
}

mod entries;

pub use entries::*;

#[cfg(test)]
#[path = "timeline_model_tests.rs"]
mod tests;
