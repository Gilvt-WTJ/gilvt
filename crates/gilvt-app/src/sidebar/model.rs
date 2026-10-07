//! What the session sidebar shows, as pure data: sections (需要你, the groups of 全部会话, 已结束),
//! rows with their status and location lines, and the next / previous session for keyboard switching.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gilvt_agent::{AgentKind, PaneId, Session, SessionKey, Status};
use serde::{Deserialize, Serialize};

/// How 全部会话 is grouped (persisted in ui.json).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grouping {
    #[default]
    Project,
    Status,
}

/// Color of a status line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// 等待审批 / 在问你 (yellow).
    Waiting,
    /// 思考中 / 执行中 (blue).
    Running,
    Error,
    /// 完成未看 (green).
    Done,
    Muted,
}

/// Context bar color: below 80 %, from 80 %, from 90 %.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Meter {
    Normal,
    Warn,
    Full,
}

/// The context bar's color for an occupancy `ratio` (0..=1).
pub fn meter(ratio: f32) -> Meter {
    if ratio >= 0.9 {
        Meter::Full
    } else if ratio >= 0.8 {
        Meter::Warn
    } else {
        Meter::Normal
    }
}

/// A session with what the model cannot know itself.
pub struct Item<'a> {
    pub session: &'a Session,
    /// Git root name, else the cwd's last component.
    pub project: String,
    /// "标签 · 左" (plus " · 窗口 2" with several windows); "" when the pane is gone.
    pub location: String,
    /// The session in this window's focused pane.
    pub current: bool,
    /// Its pane is open in some window (click and switching can go there).
    pub reachable: bool,
    /// The git line ("⎇ main ●2 ↑1"), when the session's directory is in a repository.
    pub git: Option<String>,
    /// The session's directory ("" when unknown); shown in the row's tooltip.
    pub cwd: String,
    /// The title the agent keeps for the session (its own `custom-title`, else the generated one), when the
    /// history index has it. Names the row unless the user renamed the session in gilvt.
    pub agent_title: Option<String>,
    /// The session is archived (and no newer turn arrived): an ended one is left out of 已结束.
    pub archived: bool,
}

/// What a sidebar row stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Agent,
    /// A terminal pane without a live agent session: a quiet, read-only row.
    Terminal,
}

/// A terminal pane the sidebar lists next to the agent sessions.
pub struct TerminalItem {
    pub pane: PaneId,
    /// [`terminal_name`] of the pane's title.
    pub name: String,
    /// [`terminal_project`] of the pane's directory.
    pub project: String,
    /// "标签 · 左" (plus the window), as for sessions.
    pub location: String,
    /// The git line, when the pane's directory is in a repository.
    pub git: Option<String>,
    /// The pane's directory ("" when unknown); shown in the row's tooltip.
    pub cwd: String,
    /// The pane is this window's focused pane.
    pub current: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The session; None for a terminal row.
    pub key: Option<SessionKey>,
    pub kind: RowKind,
    /// The pane to go to: live sessions whose pane is open in some window.
    pub pane: Option<PaneId>,
    /// C = Claude, X = Codex.
    pub letter: char,
    pub name: String,
    pub muted: bool,
    /// 精简模式.
    pub lite: bool,
    pub time: String,
    pub tone: Tone,
    pub status: String,
    pub location: String,
    /// Branch / dirty / ahead-behind line; "" without git (and for ended and pending rows).
    pub git: String,
    pub context: Option<(f32, Meter)>,
    pub current: bool,
    /// Drawn on the 需要你 background.
    pub needs_you: bool,
    pub ended: bool,
    /// The directory ("" when unknown or for pending rows); the tooltip shows it.
    pub cwd: String,
    /// An ended row's directory, last component only ("" otherwise): grey, after the name.
    pub dir: String,
    /// The ✦ summary's first sentence of 近期 (S2 §4.7); None when off, excluded or not summarized yet.
    pub summary: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    NeedsYou,
    Group,
    /// 待恢复: sessions of the saved layout nobody resumed yet.
    Pending,
    Ended,
    /// 终端: the terminal panes, by status grouping only.
    Terminals,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub kind: SectionKind,
    /// Key for collapse overrides ("project:<name>", "status:<n>").
    pub id: String,
    pub title: String,
    /// Shown after the title when collapsed ("1 个空闲，已折叠").
    pub note: String,
    pub collapsed: bool,
    /// Collapsed unless toggled (a group whose sessions are all idle; 已结束).
    pub default_collapsed: bool,
    /// Also filled when collapsed (switching visits them).
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// Live sessions (「会话 · N」).
    pub live: usize,
    /// Terminal rows (「终端 M」).
    pub terminals: usize,
    pub sections: Vec<Section>,
}

/// A window's sidebar settings. Groups whose sessions are all idle start collapsed; the user's toggle
/// holds until that default changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarState {
    pub hidden: bool,
    pub grouping: Grouping,
    pub ended_open: bool,
    overrides: HashMap<String, (bool, bool)>,
}

impl SidebarState {
    pub fn new(hidden: bool, grouping: Grouping) -> Self {
        SidebarState { hidden, grouping, ..Default::default() }
    }

    pub fn collapsed(&self, id: &str, default: bool) -> bool {
        match self.overrides.get(id) {
            Some(&(at, collapsed)) if at == default => collapsed,
            _ => default,
        }
    }

    pub fn toggle(&mut self, id: &str, default: bool) {
        let collapsed = self.collapsed(id, default);
        self.overrides.insert(id.to_string(), (default, !collapsed));
    }
}

/// Status groups, in display order.
pub const STATUS_GROUPS: [&str; 5] = ["等你处理", "出错", "执行中", "完成未看", "空闲"];

pub fn status_group(s: &Session) -> usize {
    match &s.status {
        _ if s.needs_you() => 0,
        Status::Error { .. } => 1,
        st if st.is_running() || s.background_tasks > 0 => 2,
        _ if s.unseen_done => 3,
        _ => 4,
    }
}

/// "40 秒", "2 分钟", "3 小时".
pub fn duration_label(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s} 秒"),
        60..3600 => format!("{} 分钟", s / 60),
        _ => format!("{} 小时", s / 3600),
    }
}

/// The location line: tab title, position in the tab, and the window when there are several.
pub fn location_text(tab_title: &str, position: &str, window: Option<&str>) -> String {
    let mut out = tab_title.to_string();
    for part in [Some(position), window].into_iter().flatten().filter(|p| !p.is_empty()) {
        out.push_str(" · ");
        out.push_str(part);
    }
    out
}

/// 状态 + 当前动作.
pub fn status_line(s: &Session) -> (Tone, String) {
    match &s.status {
        Status::NeedsApproval { action } => (Tone::Waiting, join("⏳ 等待审批", action)),
        Status::Asking { question } => (Tone::Waiting, join("? 在问你", question)),
        Status::Error { message } => (Tone::Error, join("✕ 出错", message)),
        Status::Ended => (Tone::Muted, "已结束".into()),
        _ if s.background_tasks > 0 => (Tone::Running, format!("● 后台任务运行中 · {} 个后台任务", s.background_tasks)),
        Status::Thinking => (Tone::Running, "● 思考中".into()),
        Status::Tool { label } => (Tone::Running, join("● 执行中", label)),
        Status::Idle if s.unseen_done => {
            let took = s.last_turn.map(|d| format!("用时 {}", duration_label(d))).unwrap_or_default();
            (Tone::Done, join("✓ 完成未看", &took))
        }
        Status::Idle => (Tone::Muted, "空闲 · 等你输入".into()),
    }
}

fn join(head: &str, detail: &str) -> String {
    let detail = detail.lines().next().unwrap_or("").trim();
    if detail.is_empty() { head.to_string() } else { format!("{head} · {detail}") }
}

/// Wait time while it needs you, 执行中 while it runs, else the clock time of the last change.
fn time_label(s: &Session, now: Instant, clock: &dyn Fn(Instant) -> String) -> String {
    match (&s.status, s.waiting_since) {
        (st, Some(since)) if st.needs_you() => duration_label(now.saturating_duration_since(since)),
        (st, _) if st.is_running() || (s.is_live() && s.background_tasks > 0) => "执行中".into(),
        _ => clock(s.updated),
    }
}

/// A session row's name: the agent's own title unless the user renamed the session in gilvt, else its
/// name, else 「新会话」 (also the monitor tab's card name).
pub fn row_name(item: &Item) -> String {
    let s = item.session;
    match &item.agent_title {
        Some(title) if !s.renamed => title.clone(),
        _ if s.name.is_empty() => "新会话".into(),
        _ => s.name.clone(),
    }
}

fn row(item: &Item, needs_you: bool, now: Instant, clock: &dyn Fn(Instant) -> String) -> Row {
    let s = item.session;
    let (tone, status) = status_line(s);
    let context = s.context_ratio().filter(|_| s.is_live()).map(|r| {
        (r.clamp(0.0, 1.0), meter(r))
    });
    Row {
        key: Some(s.key.clone()),
        kind: RowKind::Agent,
        pane: s.pane.filter(|_| item.reachable && s.is_live()),
        letter: match s.agent() {
            AgentKind::Claude => 'C',
            AgentKind::Codex => 'X',
        },
        name: row_name(item),
        muted: s.muted,
        lite: s.lite && s.is_live(),
        time: time_label(s, now, clock),
        tone,
        status,
        location: if s.is_live() { item.location.clone() } else { String::new() },
        git: if s.is_live() { item.git.clone().unwrap_or_default() } else { String::new() },
        context,
        current: item.current && s.is_live(),
        needs_you,
        ended: !s.is_live(),
        cwd: item.cwd.clone(),
        dir: if s.is_live() || item.cwd.is_empty() { String::new() } else { crate::launcher::last_component(std::path::Path::new(&item.cwd)) },
        summary: None,
    }
}

/// A terminal's row: quiet — no status line, no time, no context bar.
fn terminal_row(t: &TerminalItem) -> Row {
    Row {
        key: None,
        kind: RowKind::Terminal,
        pane: Some(t.pane),
        letter: '>',
        name: t.name.clone(),
        muted: false,
        lite: false,
        time: String::new(),
        tone: Tone::Muted,
        status: String::new(),
        location: t.location.clone(),
        git: t.git.clone().unwrap_or_default(),
        context: None,
        current: t.current,
        needs_you: false,
        ended: false,
        cwd: t.cwd.clone(),
        dir: String::new(),
        summary: None,
    }
}

/// Up to and including the first sentence end (。！？ and . ! ? followed by a space), else the whole text.
pub fn first_sentence(s: &str) -> String {
    let s = s.trim();
    for (i, c) in s.char_indices() {
        let end = i + c.len_utf8();
        if matches!(c, '。' | '！' | '？') || (matches!(c, '.' | '!' | '?') && s[end..].starts_with(' ')) {
            return s[..end].to_string();
        }
    }
    s.to_string()
}

/// `model` with each row's `summary` from `f`.
pub fn with_summaries(mut model: Model, f: impl Fn(&Row) -> Option<String>) -> Model {
    for row in model.sections.iter_mut().flat_map(|s| s.rows.iter_mut()) {
        row.summary = f(row);
    }
    model
}

/// A terminal row's name: its pane title on one line, else 终端.
pub fn terminal_name(title: &str) -> String {
    let one_line = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.is_empty() { "终端".into() } else { one_line }
}

/// A terminal's project: the one resolved from its directory, else 终端 (no directory known).
pub fn terminal_project(project: Option<String>) -> String {
    project.unwrap_or_else(|| "终端".into())
}

/// The header: 「会话 · N」, plus 「 · 终端 M」 when terminals are listed.
pub fn header_text(m: &Model) -> String {
    if m.terminals == 0 {
        format!("会话 · {}", m.live)
    } else {
        format!("会话 · {} · 终端 {}", m.live, m.terminals)
    }
}

/// The hover tooltip: everything the row shows in full, plus its directory; empty fields are skipped.
pub fn tooltip_lines(r: &Row) -> Vec<String> {
    [r.name.as_str(), r.status.as_str(), r.location.as_str(), r.cwd.as_str(), r.git.as_str()]
        .into_iter()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Sections for `items` (registry order) and the terminal panes without an agent.
pub fn build(items: &[Item], terminals: &[TerminalItem], view: &SidebarState, now: Instant, clock: &dyn Fn(Instant) -> String) -> Model {
    let live: Vec<&Item> = items.iter().filter(|i| i.session.is_live()).collect();
    let mut sections = Vec::new();

    let mut waiting: Vec<&&Item> = live.iter().filter(|i| i.session.needs_you()).collect();
    waiting.sort_by_key(|i| i.session.waiting_since.map_or((1, now), |t| (0, t)));
    if !waiting.is_empty() {
        sections.push(Section {
            kind: SectionKind::NeedsYou,
            id: "needs_you".into(),
            title: format!("需要你 · {}", waiting.len()),
            note: String::new(),
            collapsed: false,
            default_collapsed: false,
            rows: waiting.iter().map(|i| row(i, true, now, clock)).collect(),
        });
    }

    // (id, title, agent members, terminal members)
    let mut groups: Vec<(String, String, Vec<&Item>, Vec<&TerminalItem>)> = Vec::new();
    match view.grouping {
        Grouping::Project => {
            for item in &live {
                match groups.iter_mut().find(|g| g.1 == item.project) {
                    Some(g) => g.2.push(item),
                    None => groups.push((format!("project:{}", item.project), item.project.clone(), vec![item], Vec::new())),
                }
            }
            for t in terminals {
                match groups.iter_mut().find(|g| g.1 == t.project) {
                    Some(g) => g.3.push(t),
                    None => groups.push((format!("project:{}", t.project), t.project.clone(), Vec::new(), vec![t])),
                }
            }
        }
        Grouping::Status => {
            for (n, title) in STATUS_GROUPS.iter().enumerate() {
                let members: Vec<&Item> = live.iter().copied().filter(|i| status_group(i.session) == n).collect();
                if !members.is_empty() {
                    groups.push((format!("status:{n}"), title.to_string(), members, Vec::new()));
                }
            }
        }
    }
    for (id, title, members, terms) in groups {
        // Only the agents decide whether a group is idle; a group of terminals alone starts expanded.
        let idle = !members.is_empty() && members.iter().all(|i| status_group(i.session) == 4);
        let collapsed = view.collapsed(&id, idle);
        let note = match (collapsed, idle) {
            (false, _) => String::new(),
            (true, true) => format!("{} 个空闲，已折叠", members.len()),
            (true, false) => format!("{} 个会话，已折叠", members.len() + terms.len()),
        };
        let mut rows: Vec<Row> = members.iter().map(|i| row(i, false, now, clock)).collect();
        rows.extend(terms.iter().map(|t| terminal_row(t)));
        sections.push(Section { kind: SectionKind::Group, id, title, note, collapsed, default_collapsed: idle, rows });
    }

    if view.grouping == Grouping::Status && !terminals.is_empty() {
        let collapsed = view.collapsed("terminals", true);
        sections.push(Section {
            kind: SectionKind::Terminals,
            id: "terminals".into(),
            title: format!("终端 · {}", terminals.len()),
            note: if collapsed { format!("{} 个，已折叠", terminals.len()) } else { String::new() },
            collapsed,
            default_collapsed: true,
            rows: terminals.iter().map(terminal_row).collect(),
        });
    }

    let mut ended: Vec<&Item> = items.iter().filter(|i| !i.session.is_live() && !i.archived).collect();
    ended.sort_by_key(|i| std::cmp::Reverse(i.session.updated));
    if !ended.is_empty() {
        sections.push(Section {
            kind: SectionKind::Ended,
            id: "ended".into(),
            title: format!("已结束 · {}", ended.len()),
            note: String::new(),
            collapsed: !view.ended_open,
            default_collapsed: true,
            rows: ended.iter().map(|i| row(i, false, now, clock)).collect(),
        });
    }
    Model { live: live.len(), terminals: terminals.len(), sections }
}

/// 待恢复 rows: sessions from the saved layout that nobody resumed yet.
pub fn pending_rows(pending: &[crate::agents::PendingResume]) -> Vec<Row> {
    pending
        .iter()
        .map(|p| Row {
            key: Some(p.key.clone()),
            kind: RowKind::Agent,
            pane: Some(p.pane),
            letter: if p.key.0 == AgentKind::Claude { 'C' } else { 'X' },
            name: if p.name.is_empty() { "（未命名）".into() } else { p.name.clone() },
            muted: false,
            lite: false,
            time: String::new(),
            tone: Tone::Muted,
            status: "待恢复".into(),
            location: String::new(),
            git: String::new(),
            context: None,
            current: false,
            needs_you: false,
            ended: false,
            cwd: String::new(),
            dir: String::new(),
            summary: None,
        })
        .collect()
}

/// `model` with the 待恢复 section (right after 需要你, before the groups) when anything is pending.
/// Not live, so it is neither counted in `live` nor part of the keyboard switching order.
pub fn with_pending(mut model: Model, pending: &[crate::agents::PendingResume]) -> Model {
    if pending.is_empty() {
        return model;
    }
    let at = model.sections.iter().take_while(|s| s.kind == SectionKind::NeedsYou).count();
    model.sections.insert(
        at,
        Section {
            kind: SectionKind::Pending,
            id: "pending".into(),
            title: format!("待恢复 · {}", pending.len()),
            note: String::new(),
            collapsed: false,
            default_collapsed: false,
            rows: pending_rows(pending),
        },
    );
    model
}

/// Every row in sidebar order, with its section's index and whether it is drawn: a collapsed section
/// draws none of its rows, except the first row of the session being renamed (`renaming`), whose field
/// must stay visible.
pub fn all_rows<'a>(model: &'a Model, renaming: Option<&SessionKey>) -> Vec<(usize, &'a Row, bool)> {
    let mut field = renaming;
    let mut out = Vec::new();
    for (si, section) in model.sections.iter().enumerate() {
        for row in &section.rows {
            let has_field = field.is_some_and(|k| row.key.as_ref() == Some(k));
            if has_field {
                field = None;
            }
            out.push((si, row, !section.collapsed || has_field));
        }
    }
    out
}

/// The rows the sidebar draws, in order, with their section's index (the drawn part of [`all_rows`]).
pub fn visible_rows<'a>(model: &'a Model, renaming: Option<&SessionKey>) -> Vec<(usize, &'a Row)> {
    all_rows(model, renaming).into_iter().filter(|r| r.2).map(|(si, row, _)| (si, row)).collect()
}

/// A session to switch to, and its pane.
pub type Target = (SessionKey, PaneId);

fn targets<'a>(sections: impl Iterator<Item = &'a Section>) -> Vec<Target> {
    sections.flat_map(|s| s.rows.iter().filter_map(|r| Some((r.key.clone()?, r.pane?)))).collect()
}

/// Sessions in sidebar order for ⌘⇧↑ / ⌘⇧↓: the rows of 全部会话 of all windows (collapsed groups
/// included; the 需要你 duplicates, 已结束 and sessions whose pane is gone not).
pub fn session_order(model: &Model) -> Vec<Target> {
    targets(model.sections.iter().filter(|s| s.kind == SectionKind::Group))
}

/// ⌘⇧J: the session after `current` in 需要你 (longest wait first, wrapping); the longest waiting one
/// when `current` is not waiting.
pub fn next_needs_you(model: &Model, current: Option<&SessionKey>) -> Option<Target> {
    let waiting = targets(model.sections.iter().filter(|s| s.kind == SectionKind::NeedsYou));
    neighbor(&waiting, current, Step::Next)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Prev,
    Next,
}

/// The row before / after `current` in `rows` (wrapping); from outside the list, the first (Next) or
/// last (Prev) row.
pub fn neighbor(rows: &[Target], current: Option<&SessionKey>, step: Step) -> Option<Target> {
    let n = rows.len();
    if n == 0 {
        return None;
    }
    let i = match (current.and_then(|c| rows.iter().position(|r| &r.0 == c)), step) {
        (Some(i), Step::Next) => (i + 1) % n,
        (Some(i), Step::Prev) => (i + n - 1) % n,
        (None, Step::Next) => 0,
        (None, Step::Prev) => n - 1,
    };
    Some(rows[i].clone())
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
