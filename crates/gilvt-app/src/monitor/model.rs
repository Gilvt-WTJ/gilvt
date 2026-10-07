//! The monitor tab's cards as pure data: sessions and terminal panes in, grouped cards out.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gilvt_agent::{AgentKind, PaneId, PlanItem, PlanState, Session, SessionKey, Turn, TurnOutcome};
use gilvt_term::CommandBlock;

use super::summaries::{SumState, SummaryView};
use super::{Filter, MonitorUi};
use crate::inspector::artifacts_model::ArtCard;
use crate::sidebar::model::{duration_label, status_group, status_line, Tone};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    NeedsYou,
    Error,
    Running,
    Done,
    Idle,
    Terminals,
    Ended,
}

impl Group {
    pub const ALL: [Group; 7] = [Group::NeedsYou, Group::Error, Group::Running, Group::Done, Group::Idle, Group::Terminals, Group::Ended];

    pub fn title(self) -> &'static str {
        match self {
            Group::NeedsYou => crate::i18n::text("需要你", "Needs you"),
            Group::Error => crate::i18n::text("出错", "Errors"),
            Group::Running => crate::i18n::text("运行中", "Running"),
            Group::Done => crate::i18n::text("完成未看", "Done, unseen"),
            Group::Idle => crate::i18n::text("空闲", "Idle"),
            Group::Terminals => crate::i18n::text("终端", "Terminals"),
            Group::Ended => crate::i18n::text("已结束", "Ended"),
        }
    }

    /// DebugState / filter id.
    pub fn id(self) -> &'static str {
        match self {
            Group::NeedsYou => "needs_you",
            Group::Error => "error",
            Group::Running => "running",
            Group::Done => "done",
            Group::Idle => "idle",
            Group::Terminals => "terminals",
            Group::Ended => "ended",
        }
    }

    /// The sidebar's status group (`status_group`) of a live session.
    fn of_live(s: &Session) -> Group {
        match status_group(s) {
            0 => Group::NeedsYou,
            1 => Group::Error,
            2 => Group::Running,
            3 => Group::Done,
            _ => Group::Idle,
        }
    }
}

pub struct AgentIn<'a> {
    pub session: &'a Session,
    /// The sidebar's row name (agent title / rename / first prompt).
    pub name: String,
    pub location: String,
    pub git: Option<String>,
    pub archived: bool,
    pub plan: &'a [PlanItem],
    /// The timeline's turns, oldest first.
    pub turns: &'a [Arc<Turn>],
    /// The 「产物」 cards, newest first (per-turn file changes).
    pub cards: &'a [ArtCard],
    pub summary: SummarySlot,
    /// The directory is in `[monitor] exclude_paths`: no ✦ block, no 「◎ 问它」, not in the chat's `@` list or tools.
    pub excluded: bool,
}

pub struct TerminalIn {
    pub pane: PaneId,
    pub name: String,
    pub cwd: Option<PathBuf>,
    pub location: String,
    /// The foreground program when it is not the shell.
    pub foreground: Option<String>,
    /// Newest first, at most [`CATCHUP_BLOCKS`].
    pub blocks: Vec<CommandBlock>,
    pub summary: SummarySlot,
    /// The directory is in `[monitor] exclude_paths`: no ✦ block, no 「◎ 问它」, not in the chat's `@` list or tools.
    pub excluded: bool,
}

/// What the summaries service has for a card (S2 §4.7).
#[derive(Clone, Debug, PartialEq)]
pub enum SummarySlot {
    /// The monitor is off or the directory is excluded: no ✦ block at all.
    Hidden,
    /// On, but nothing yet: a 「✦ 生成总结」 button (only when a request can act; else `Hidden`).
    Empty,
    /// `stale`: the session moved on since the summary was made. `actionable`: a request for it can act
    /// (`summaries::agent_requestable` / a terminal with a finished command); else the text shows without
    /// buttons (e.g. an ended session's saved summary).
    View { view: Arc<SummaryView>, stale: bool, actionable: bool },
}

/// A card's ✦ block as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct SummaryLine {
    /// none | pending | ready | stale | failed | paused
    pub state: &'static str,
    pub header: String,
    pub goal: Option<String>,
    pub recent: Option<String>,
    /// The header is a button (生成总结 / 重试).
    pub clickable: bool,
    /// A request can act on it: 「✦ 重新总结」, the header buttons and `s` are offered.
    pub actionable: bool,
}

/// The slot of a card the monitor shows (on, not excluded): `view` from the summaries service, `current` the
/// subject's activity now, `actionable` whether a request for it can act. Nothing to show and nothing to ask
/// for draws no block.
pub fn summary_slot(view: Option<Arc<SummaryView>>, current: Option<u64>, actionable: bool) -> SummarySlot {
    match view {
        Some(view) => {
            let stale = current.is_some_and(|c| c > view.activity) && view.state == SumState::Ready;
            SummarySlot::View { view, stale, actionable }
        }
        None if actionable => SummarySlot::Empty,
        None => SummarySlot::Hidden,
    }
}

/// 「刚刚」 under a minute (or when unknown), else 「3 分钟前」.
fn age(at: Option<SystemTime>, wall: SystemTime) -> String {
    match at.and_then(|t| wall.duration_since(t).ok()) {
        Some(d) if d < Duration::from_secs(60) => crate::i18n::text("刚刚", "just now").into(),
        Some(d) if crate::i18n::current() == crate::i18n::Language::English => {
            format!("{} ago", duration_label(d))
        }
        Some(d) => format!("{}前", duration_label(d)),
        None => crate::i18n::text("刚刚", "just now").into(),
    }
}

/// The ✦ block of a card (`terminal`: no 目标 line); None draws no block.
pub fn summary_line(slot: &SummarySlot, terminal: bool, wall: SystemTime) -> Option<SummaryLine> {
    let (v, stale, actionable) = match slot {
        SummarySlot::Hidden => return None,
        SummarySlot::Empty => {
            return Some(SummaryLine {
                state: "none",
                header: crate::i18n::text("✦ 生成总结", "✦ Generate Summary").into(),
                goal: None,
                recent: None,
                clickable: true,
                actionable: true,
            })
        }
        SummarySlot::View {
            view,
            stale,
            actionable,
        } => (view, *stale, *actionable),
    };
    let covers = v.covers.map(|c| c.label());
    let goal = v.summary.as_ref().and_then(|s| s.goal.clone()).filter(|_| !terminal);
    let recent = v.summary.as_ref().map(|s| s.recent.clone()).filter(|r| !r.is_empty());
    let join = |parts: Vec<Option<String>>| parts.into_iter().flatten().collect::<Vec<_>>().join(" · ");
    let (state, header, clickable) = match &v.state {
        SumState::Pending if v.summary.is_none() => (
            "pending",
            crate::i18n::text("✦ AI 总结 · 生成中…", "✦ AI Summary · Generating…").to_string(),
            false,
        ),
        SumState::Pending => {
            let previous = if crate::i18n::current() == crate::i18n::Language::English {
                format!("previous {}", age(v.generated_at, wall))
            } else {
                format!("上次 {}", age(v.generated_at, wall))
            };
            (
                "pending",
                join(vec![
                    Some(
                        crate::i18n::text("✦ AI 总结 · 更新中…", "✦ AI Summary · Updating…").into(),
                    ),
                    Some(previous),
                    covers,
                ]),
                false,
            )
        }
        SumState::Ready => {
            let mut h = join(vec![
                Some(crate::i18n::text("✦ AI 总结", "✦ AI Summary").into()),
                Some(age(v.generated_at, wall)),
                covers,
            ]);
            if stale {
                h.push_str(crate::i18n::text(" · 有新进展", " · new activity"));
            }
            (if stale { "stale" } else { "ready" }, h, false)
        }
        SumState::Failed(m) if crate::i18n::current() == crate::i18n::Language::English => {
            ("failed", format!("✦ Summary failed: {m} · Retry"), true)
        }
        SumState::Failed(m) => ("failed", format!("✦ 总结失败：{m} · 重试"), true),
        SumState::Paused(m) if crate::i18n::current() == crate::i18n::Language::English => (
            "paused",
            format!("✦ Automatic summaries paused: {m} · Retry"),
            true,
        ),
        SumState::Paused(m) => ("paused", format!("✦ 已暂停自动总结：{m} · 重试"), true),
    };
    Some(SummaryLine { state, header, goal, recent, clickable: clickable && actionable, actionable })
}

/// Commands listed in a terminal card's 补课.
pub const CATCHUP_BLOCKS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Changes {
    pub files: usize,
    /// None while the task's rows are not its net diff (still computing, or a union of its turns' files):
    /// only the file count is shown then.
    pub added: Option<u32>,
    pub removed: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Ok,
    Failed,
    Running,
    Interrupted,
    /// A command whose end was not seen.
    Unknown,
}

impl Mark {
    pub fn glyph(self) -> &'static str {
        match self {
            Mark::Ok => "✓",
            Mark::Failed => "✗",
            Mark::Running => "●",
            Mark::Interrupted => "■",
            Mark::Unknown => "?",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TurnLine {
    pub turn: u32,
    pub prompt: String,
    pub mark: Mark,
    pub took: Option<String>,
    pub changes: Option<Changes>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockLine {
    pub id: u64,
    pub command: String,
    pub mark: Mark,
    pub exit: Option<i32>,
    pub took: Option<String>,
    /// 「3 分钟前」 (when it ended; when it started while running).
    pub ago: String,
    /// Absolute line of its start, to jump back to.
    pub line: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentCard {
    pub key: SessionKey,
    pub pane: Option<PaneId>,
    pub letter: char,
    pub name: String,
    pub location: String,
    pub tone: Tone,
    pub status: String,
    pub git: String,
    pub turn: Option<u32>,
    /// 本轮耗时 while a turn runs, else 「用时 …」 of the last one.
    pub elapsed: Option<String>,
    pub changes: Option<Changes>,
    /// (done, total)
    pub todo: Option<(usize, usize)>,
    pub context: Option<f32>,
    /// The last reply's first sentence, shown while 完成未看.
    pub quote: String,
    pub catchup: Vec<TurnLine>,
    pub summary: Option<SummaryLine>,
    /// The directory is in `[monitor] exclude_paths`: no ✦ block, no 「◎ 问它」, not in the chat's `@` list or tools.
    pub excluded: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerminalCard {
    pub pane: PaneId,
    pub name: String,
    /// `~`-abbreviated.
    pub cwd: String,
    pub location: String,
    /// (command, elapsed) of the command running now.
    pub running: Option<(String, String)>,
    pub last: Option<BlockLine>,
    /// The last non-empty output line of the last command when it failed (output captured from the PTY, S2 §3.3).
    pub error_line: Option<String>,
    pub foreground: Option<String>,
    pub catchup: Vec<BlockLine>,
    pub summary: Option<SummaryLine>,
    /// The directory is in `[monitor] exclude_paths`: no ✦ block, no 「◎ 问它」, not in the chat's `@` list or tools.
    pub excluded: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Card {
    Agent(AgentCard),
    Terminal(TerminalCard),
}

impl Card {
    pub fn key(&self) -> String {
        match self {
            Card::Agent(a) => super::summaries::agent_key(&a.key),
            Card::Terminal(t) => super::summaries::pane_key(t.pane),
        }
    }

    pub fn summary(&self) -> Option<&SummaryLine> {
        match self {
            Card::Agent(a) => a.summary.as_ref(),
            Card::Terminal(t) => t.summary.as_ref(),
        }
    }

    pub fn pane(&self) -> Option<PaneId> {
        match self {
            Card::Agent(a) => a.pane,
            Card::Terminal(t) => Some(t.pane),
        }
    }

    pub fn excluded(&self) -> bool {
        match self {
            Card::Agent(a) => a.excluded,
            Card::Terminal(t) => t.excluded,
        }
    }

    /// The name as the card's title shows it (chips, tool rows).
    pub fn name(&self) -> &str {
        match self {
            Card::Agent(a) => &a.name,
            Card::Terminal(t) => &t.name,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GroupView {
    pub group: Group,
    pub collapsed: bool,
    pub cards: Vec<Card>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct MonitorModel {
    /// Non-zero groups, in display order, whatever the filter.
    pub counts: Vec<(Group, usize)>,
    /// The groups shown (after the filter), non-empty only.
    pub groups: Vec<GroupView>,
    /// The filter applied: the stored one, or 全部 when its group has no cards (the active chip).
    pub filter: Filter,
    /// Every card of the wall in display order, whatever the filter and 已结束's fold, minus excluded ones: what
    /// the chat may name (`@` candidates, session links).
    pub askable: Vec<WallEntry>,
}

/// A card the chat may name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallEntry {
    pub key: String,
    /// None: an ended session without a pane.
    pub pane: Option<PaneId>,
    /// 「name · location」.
    pub label: String,
}

impl MonitorModel {
    pub fn needs_you(&self) -> usize {
        self.counts.iter().find(|(g, _)| *g == Group::NeedsYou).map_or(0, |(_, n)| *n)
    }

    /// Some cards are drawn: their clocks and relative times (「3 分钟前」) follow the 1 s redraw (spec §3.4).
    pub fn ticking(&self) -> bool {
        self.groups.iter().any(|g| !g.collapsed && !g.cards.is_empty())
    }

    /// Keys of the cards drawn (collapsed groups excluded), in order: keyboard selection moves along it.
    pub fn visible_keys(&self) -> Vec<String> {
        self.groups.iter().filter(|g| !g.collapsed).flat_map(|g| g.cards.iter().map(Card::key)).collect()
    }
}

/// The 需要你 group's size without building the model (same rule as [`build`]: live sessions only).
pub fn needs_you_count<'a>(sessions: impl IntoIterator<Item = &'a Session>) -> usize {
    sessions.into_iter().filter(|s| s.is_live() && Group::of_live(s) == Group::NeedsYou).count()
}

/// The monitor tab's title in the tab bar (and DebugState): 「 · N 需要你」 appended when N > 0.
pub fn tab_title_with_needs_you(title: String, needs_you: usize) -> String {
    match needs_you {
        0 => title,
        n => format!("{title} · {n} 需要你"),
    }
}

/// The card selected after arrow `key` ("down"/"right" next, "up"/"left" previous) on `keys` (the drawn cards
/// in order): the first card when nothing (or a card no longer drawn) is selected; clamps at both ends.
/// None for an empty wall or another key.
pub fn step_selection(keys: &[String], current: Option<&str>, key: &str) -> Option<String> {
    if keys.is_empty() {
        return None;
    }
    let at = current.and_then(|k| keys.iter().position(|x| x == k));
    let next = match (key, at) {
        ("down" | "right", Some(i)) => (i + 1).min(keys.len() - 1),
        ("up" | "left", Some(i)) => i.saturating_sub(1),
        ("down" | "right" | "up" | "left", None) => 0,
        _ => return None,
    };
    Some(keys[next].clone())
}

pub fn build(agents: &[AgentIn], terminals: &[TerminalIn], ui: &MonitorUi, now: Instant, wall: SystemTime, home: Option<&Path>) -> MonitorModel {
    let mut buckets: Vec<(Group, Vec<Card>)> = Group::ALL.iter().map(|g| (*g, Vec::new())).collect();
    let mut push = |g: Group, c: Card| buckets.iter_mut().find(|(b, _)| *b == g).unwrap().1.push(c);

    let mut live: Vec<&AgentIn> = agents.iter().filter(|a| a.session.is_live()).collect();
    // 需要你: longest wait first (as the sidebar); the rest keep registry order (stable sort).
    live.sort_by_key(|a| match (Group::of_live(a.session), a.session.waiting_since) {
        (Group::NeedsYou, Some(t)) => (0, Some(t)),
        _ => (1, None),
    });
    for a in live {
        push(Group::of_live(a.session), Card::Agent(agent_card(a, now, wall)));
    }
    for t in terminals {
        push(Group::Terminals, Card::Terminal(terminal_card(t, wall, home)));
    }
    let mut ended: Vec<&AgentIn> = agents.iter().filter(|a| !a.session.is_live() && !a.archived).collect();
    ended.sort_by_key(|a| std::cmp::Reverse(a.session.updated));
    for a in ended {
        push(Group::Ended, Card::Agent(agent_card(a, now, wall)));
    }

    let counts: Vec<(Group, usize)> = buckets.iter().filter(|(_, c)| !c.is_empty()).map(|(g, c)| (*g, c.len())).collect();
    // A filter on a group that has emptied (its chip is gone) shows 全部 rather than an empty wall.
    let filter = match ui.filter {
        Filter::Only(g) if !counts.iter().any(|(c, _)| *c == g) => Filter::All,
        f => f,
    };
    let askable = buckets
        .iter()
        .flat_map(|(_, cards)| cards)
        .filter(|c| !c.excluded())
        .map(|c| {
            let location = match c {
                Card::Agent(a) => a.location.as_str(),
                Card::Terminal(t) => t.location.as_str(),
            };
            WallEntry { key: c.key(), pane: c.pane(), label: format!("{} · {location}", c.name()) }
        })
        .collect();
    let groups = buckets
        .into_iter()
        .filter(|(g, c)| !c.is_empty() && (filter == Filter::All || filter == Filter::Only(*g)))
        .map(|(group, cards)| GroupView { group, collapsed: group == Group::Ended && !ui.ended_open, cards })
        .collect();
    MonitorModel { counts, groups, filter, askable }
}

fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

fn took(from: Option<SystemTime>, to: Option<SystemTime>) -> Option<String> {
    Some(duration_label(to?.duration_since(from?).ok()?))
}

fn agent_card(a: &AgentIn, now: Instant, wall: SystemTime) -> AgentCard {
    let s = a.session;
    let (tone, status) = status_line(s);
    // In a turn (working, waiting for you, or background tasks still running) → 本轮 so far; else the last
    // turn's length.
    let in_turn = s.is_live() && (s.status.is_running() || s.needs_you() || s.background_tasks > 0);
    let elapsed = match s.turn_started {
        Some(t) if in_turn => Some(duration_label(now.saturating_duration_since(t))),
        _ => s.last_turn.map(|d| format!("用时 {}", duration_label(d))),
    };
    let changes_of = |card: &ArtCard| {
        let counted = !(card.computing || card.counts_hidden);
        (!card.files.is_empty()).then(|| Changes {
            files: card.files.len(),
            added: counted.then(|| card.files.iter().map(|f| f.added).sum()),
            removed: counted.then(|| card.files.iter().map(|f| f.removed).sum()),
        })
    };
    let done = a.plan.iter().filter(|p| p.state == PlanState::Done).count();
    let current = a.turns.last();
    AgentCard {
        key: s.key.clone(),
        pane: s.pane.filter(|_| s.is_live()),
        letter: match s.agent() {
            AgentKind::Claude => 'C',
            AgentKind::Codex => 'X',
        },
        name: a.name.clone(),
        location: if s.is_live() { a.location.clone() } else { String::new() },
        tone,
        status,
        git: if s.is_live() { a.git.clone().unwrap_or_default() } else { String::new() },
        turn: current.map(|t| t.index),
        elapsed,
        // The card of the task the current turn belongs to: while a new turn has no card yet, the previous
        // task's changes are not this turn's.
        changes: current.and_then(|t| a.cards.iter().find(|c| c.turns.contains(&t.index))).and_then(changes_of),
        todo: (!a.plan.is_empty()).then_some((done, a.plan.len())),
        context: s.context_ratio().filter(|_| s.is_live()).map(|r| r.clamp(0.0, 1.0)),
        quote: if s.unseen_done { current.map(|t| t.reply.clone()).unwrap_or_default() } else { String::new() },
        catchup: a
            .turns
            .iter()
            .rev()
            .map(|t| TurnLine {
                turn: t.index,
                prompt: first_line(&t.prompt),
                mark: match t.outcome {
                    TurnOutcome::Running => Mark::Running,
                    TurnOutcome::Done => Mark::Ok,
                    TurnOutcome::Interrupted => Mark::Interrupted,
                    TurnOutcome::Failed { .. } => Mark::Failed,
                },
                took: took(t.started, t.ended),
                // A task's changes go on the line of its last turn only.
                changes: a.cards.iter().find(|c| c.turns.last() == Some(&t.index)).and_then(changes_of),
            })
            .collect(),
        summary: summary_line(&a.summary, false, wall),
        excluded: a.excluded,
    }
}

/// A command as one line: a multi-line one shows its first non-empty line and `…`.
fn command_head(cmd: &str) -> String {
    let mut lines = cmd.lines().map(str::trim_end).filter(|l| !l.trim().is_empty());
    match (lines.next(), lines.next()) {
        (Some(first), Some(_)) => format!("{first}…"),
        (Some(first), None) => first.to_string(),
        (None, _) => String::new(),
    }
}

fn block_line(b: &CommandBlock, wall: SystemTime) -> BlockLine {
    let mark = match (b.running(), b.exit) {
        (true, _) => Mark::Running,
        (false, Some(0)) => Mark::Ok,
        (false, Some(_)) => Mark::Failed,
        (false, None) => Mark::Unknown,
    };
    let at = b.ended.unwrap_or(b.started);
    BlockLine {
        id: b.id,
        command: b.command.as_deref().map(command_head).filter(|c| !c.is_empty()).unwrap_or_else(|| "（命令未知）".into()),
        mark,
        exit: b.exit,
        took: took(Some(b.started), b.ended),
        ago: format!("{}前", duration_label(wall.duration_since(at).unwrap_or(Duration::ZERO))),
        line: b.start_line,
    }
}

fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// The last non-empty line of a failed command's output.
pub fn error_line_of(b: &CommandBlock) -> Option<String> {
    if b.running() || !b.failed() {
        return None;
    }
    b.output_tail.as_deref()?.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

fn terminal_card(t: &TerminalIn, wall: SystemTime, home: Option<&Path>) -> TerminalCard {
    let running = t.blocks.first().filter(|b| b.running()).map(|b| {
        let line = block_line(b, wall);
        (line.command, duration_label(wall.duration_since(b.started).unwrap_or(Duration::ZERO)))
    });
    let last_block = t.blocks.iter().find(|b| !b.running());
    // The newest block only: while a command runs, the previous failure is not news.
    let error_line = t.blocks.first().and_then(error_line_of);
    TerminalCard {
        pane: t.pane,
        name: t.name.clone(),
        cwd: t.cwd.as_deref().map(|c| tilde(c, home)).unwrap_or_default(),
        location: t.location.clone(),
        running,
        last: last_block.map(|b| block_line(b, wall)),
        error_line,
        foreground: t.foreground.clone().filter(|_| t.blocks.is_empty()),
        catchup: t.blocks.iter().take(CATCHUP_BLOCKS).map(|b| block_line(b, wall)).collect(),
        summary: summary_line(&t.summary, true, wall),
        excluded: t.excluded,
    }
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
