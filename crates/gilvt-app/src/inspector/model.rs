//! What the inspector's 「过程」 tab shows above the timeline, as pure data (M3b spec §2, §3.1–§3.3; mockup
//! m3b-layout.html): which session it follows, the status card, the waiting banner, the TODO block, and the
//! column's width limits.

use std::time::{Duration, Instant, SystemTime};

use gilvt_agent::{AgentKind, PaneId, PlanItem, PlanState, Session, Status, Turn};

use crate::agents::TimelineView;
use crate::sidebar::model::{duration_label, meter, status_line, Meter, Tone};

pub const MIN_WIDTH: f32 = 240.;
pub const MAX_WIDTH: f32 = 560.;
pub const DEFAULT_WIDTH: f32 = 320.;

/// A width within 240–560 px (non-finite → the default).
pub fn clamp_width(w: f32) -> f32 {
    if w.is_finite() { w.clamp(MIN_WIDTH, MAX_WIDTH) } else { DEFAULT_WIDTH }
}

/// The width while dragging its left boundary to `x` in a window `window_width` wide: the inspector is the
/// rightmost column, so it spans from the pointer to the window's right edge.
pub fn drag_width(window_width: f32, x: f32) -> f32 {
    clamp_width(window_width - x)
}

/// The session the inspector follows for the focused pane: its live session, else the one that ended there
/// most recently (spec §2 「已结束」: shown until a new session starts in the pane).
pub fn follow<'a>(sessions: impl IntoIterator<Item = &'a Session>, pane: PaneId) -> Option<&'a Session> {
    let mut live: Option<&Session> = None;
    let mut ended: Option<&Session> = None;
    for s in sessions.into_iter().filter(|s| s.pane == Some(pane)) {
        let slot = if s.is_live() { &mut live } else { &mut ended };
        if slot.is_none_or(|o| s.updated >= o.updated) {
            *slot = Some(s);
        }
    }
    live.or(ended)
}

/// What the focused pane is when it has no session: the empty state's first line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plain {
    Shell,
    Preview,
    Editor,
    Monitor,
}

impl Plain {
    pub fn title(self) -> &'static str {
        match self {
            Plain::Shell => "当前 pane 是普通 shell",
            Plain::Preview => "当前 pane 是文件预览",
            Plain::Editor => "当前 pane 是文件编辑器",
            Plain::Monitor => "监控官：所有会话的总览",
        }
    }
}

/// "850", "12.4k", "183k", "1.2M", "150M".
pub fn token_label(n: u64) -> String {
    fn scaled(v: f64, unit: &str) -> String {
        let text = if v < 100.0 { format!("{v:.1}") } else { format!("{v:.0}") };
        format!("{}{unit}", text.strip_suffix(".0").unwrap_or(&text))
    }
    match n {
        0..1_000 => n.to_string(),
        // 999_950 would round to "1000k".
        1_000..999_500 => scaled(n as f64 / 1e3, "k"),
        _ => scaled(n as f64 / 1e6, "M"),
    }
}

/// 本轮耗时: "48s", "1m12s", "2m03s", "1h02m".
pub fn elapsed_label(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        _ => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    }
}

/// The banner's wait: seconds for the first minute ("38s"), then minutes ("2m", "1h05m"); it is only
/// redrawn every second during the first minute.
pub fn wait_label(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        _ => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    }
}

/// Now, on both clocks: sessions use `Instant`, timeline turns `SystemTime`.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    pub now: Instant,
    pub wall: SystemTime,
}

impl Clock {
    pub fn now() -> Clock {
        Clock { now: Instant::now(), wall: SystemTime::now() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextBar {
    /// 0..=1.
    pub ratio: f32,
    pub meter: Meter,
    /// "上下文 62k / 200k · 31%".
    pub label: String,
}

/// The status card (spec §3.2). No buttons.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    /// Yellow card while waiting, red on error; the status text takes the tone's color.
    pub tone: Tone,
    /// "● 执行工具 · Bash", "⏳ 等待审批 · Bash(rm -rf build)", "空闲 · 等你输入", "已结束", …
    pub status: String,
    /// "第 3 轮 · 1m12s" (right side of the first line).
    pub turn: Option<String>,
    pub context: Option<ContextBar>,
    /// "claude-opus-5-5 · default" (permission mode omitted when unknown).
    pub model: Option<String>,
    /// "本轮 12.4k · 会话 183k tokens".
    pub tokens: Option<String>,
    /// 精简模式 / 后台任务运行中 / 该版本暂未完全适配.
    pub tags: Vec<&'static str>,
    /// The turn is running: the elapsed time must be redrawn every second.
    pub running: bool,
}

fn with_detail(head: &str, detail: &str) -> String {
    let detail = detail.lines().next().unwrap_or("").trim();
    if detail.is_empty() { head.to_string() } else { format!("{head} · {detail}") }
}

/// The card's first line. Unlike the sidebar, a running tool is named by its tool only (the command is in
/// the timeline), and background tasks are a tag rather than the status.
pub fn card_status(s: &Session) -> (Tone, String) {
    match &s.status {
        Status::NeedsApproval { .. } | Status::Asking { .. } | Status::Error { .. } | Status::Ended => status_line(s),
        Status::Thinking => (Tone::Running, "● 思考中".into()),
        Status::Tool { label } => {
            let tool = label.split('(').next().unwrap_or("").trim();
            (Tone::Running, with_detail("● 执行工具", tool))
        }
        Status::Idle if s.unseen_done => {
            let took = s.last_turn.map(|d| format!("用时 {}", duration_label(d))).unwrap_or_default();
            (Tone::Done, with_detail("✓ 完成未看", &took))
        }
        Status::Idle => (Tone::Muted, "空闲 · 等你输入".into()),
    }
}

/// A live session in a turn: working or waiting for you.
fn busy(s: &Session) -> bool {
    s.is_live() && (s.status.is_running() || s.needs_you())
}

/// 本轮耗时: running → since the prompt (the session's clock, else the turn's start); finished → the turn's
/// length (timeline, else the session's last turn).
fn turn_elapsed(s: &Session, turn: Option<&Turn>, clock: Clock) -> Option<Duration> {
    if busy(s) {
        return s
            .turn_started
            .map(|t| clock.now.saturating_duration_since(t))
            .or_else(|| turn?.started.and_then(|t| clock.wall.duration_since(t).ok()));
    }
    turn.and_then(|t| t.ended?.duration_since(t.started?).ok()).or(s.last_turn)
}

fn context_bar(s: &Session) -> Option<ContextBar> {
    let (tokens, _) = s.context?;
    let ratio = s.context_ratio().filter(|r| r.is_finite() && *r > 0.0)?;
    // The window the ratio was computed with (Claude transcripts carry none; the session assumes 200k / 1M).
    let window = (tokens as f64 / ratio as f64).round() as u64;
    let pct = (ratio * 100.0).round() as u32;
    Some(ContextBar {
        ratio: ratio.clamp(0.0, 1.0),
        meter: meter(ratio),
        label: format!("上下文 {} / {} · {pct}%", token_label(tokens), token_label(window)),
    })
}

/// The status card of `s`, with its timeline (if any) and the permission mode its hooks reported.
pub fn card(s: &Session, view: Option<&TimelineView>, permission_mode: Option<&str>, clock: Clock) -> Card {
    let (tone, status) = card_status(s);
    let turn = view.and_then(TimelineView::current);
    let elapsed = turn_elapsed(s, turn, clock).map(elapsed_label);
    let running = busy(s) && elapsed.is_some();
    let turn_label = match (turn.map(|t| t.index), elapsed) {
        (Some(n), Some(e)) => Some(format!("第 {n} 轮 · {e}")),
        (Some(n), None) => Some(format!("第 {n} 轮")),
        (None, Some(e)) => Some(format!("本轮 · {e}")),
        (None, None) => None,
    };
    let model = [s.model.as_deref(), permission_mode].into_iter().flatten().filter(|p| !p.is_empty()).collect::<Vec<_>>();
    let tokens = view.map(|v| match turn {
        Some(t) => format!("本轮 {} · 会话 {} tokens", token_label(t.tokens), token_label(v.session_tokens)),
        None => format!("会话 {} tokens", token_label(v.session_tokens)),
    });
    let mut tags = Vec::new();
    if s.lite && s.is_live() {
        tags.push("精简模式");
    }
    if s.background_tasks > 0 && s.is_live() {
        tags.push("后台任务运行中");
    }
    if view.is_some_and(TimelineView::not_adapted) {
        tags.push("该版本暂未完全适配");
    }
    Card {
        tone,
        status,
        turn: turn_label,
        context: context_bar(s),
        model: (!model.is_empty()).then(|| model.join(" · ")),
        tokens,
        tags,
        running,
    }
}

/// The waiting banner (spec §3.1): another session waits for you.
#[derive(Clone, Debug, PartialEq)]
pub struct Banner {
    /// "⏳ codex · web 在等审批" / "⏳ claude · api 在问你".
    pub title: String,
    /// "npm test -- --watch=false · 38s", plus " · 另有 N 个" when others wait too.
    pub detail: String,
    pub waited: Duration,
}

/// The longest-waiting session other than `current` that ⌘⇧J can reach (`reachable`: its pane is open in
/// some window). None when `current` itself waits (its card is yellow then) or nobody else does.
pub fn banner<'a>(
    sessions: impl IntoIterator<Item = &'a Session>,
    current: Option<&Session>,
    reachable: impl Fn(PaneId) -> bool,
    project: impl Fn(&Session) -> String,
    now: Instant,
) -> Option<Banner> {
    if current.is_some_and(|c| c.is_live() && c.needs_you()) {
        return None;
    }
    let mut waiting: Vec<&Session> = sessions
        .into_iter()
        .filter(|s| s.is_live() && s.needs_you())
        .filter(|s| current.is_none_or(|c| c.key != s.key))
        .filter(|s| s.pane.is_some_and(&reachable))
        .collect();
    waiting.sort_by_key(|s| s.waiting_since.map_or((1, now), |t| (0, t)));
    let first = *waiting.first()?;
    let agent = match first.agent() {
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
    };
    let (verb, what) = match &first.status {
        Status::Asking { question } => ("在问你", question.as_str()),
        Status::NeedsApproval { action } => ("在等审批", action.as_str()),
        _ => ("在等你", ""),
    };
    let waited = first.waiting_since.map_or(Duration::ZERO, |t| now.saturating_duration_since(t));
    let others = waiting.len() - 1;
    let what = what.lines().next().unwrap_or("").trim();
    let more = (others > 0).then(|| format!("另有 {others} 个"));
    let parts = [Some(what.to_string()).filter(|w| !w.is_empty()), Some(wait_label(waited)), more];
    let detail = parts.into_iter().flatten().collect::<Vec<_>>().join(" · ");
    Some(Banner { title: format!("⏳ {agent} · {} {verb}", project(first)), detail, waited })
}

/// Whether the inspector must be redrawn every second: a running turn's elapsed time, or a banner wait
/// still counted in seconds.
pub fn needs_tick(card: Option<&Card>, banner: Option<&Banner>) -> bool {
    card.is_some_and(|c| c.running) || banner.is_some_and(|b| b.waited < Duration::from_secs(60))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanRow {
    /// ☑ / ◐ / ☐.
    pub glyph: &'static str,
    pub text: String,
    /// Done → struck through, Active → bold.
    pub state: PlanState,
}

/// The TODO block (spec §3.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// "TODO · 3/5".
    pub title: String,
    pub rows: Vec<PlanRow>,
}

/// None when there is no TODO (the block is hidden).
pub fn plan(items: &[PlanItem]) -> Option<Plan> {
    if items.is_empty() {
        return None;
    }
    let done = items.iter().filter(|i| i.state == PlanState::Done).count();
    let rows = items
        .iter()
        .map(|i| PlanRow {
            glyph: match i.state {
                PlanState::Done => "☑",
                PlanState::Active => "◐",
                PlanState::Todo => "☐",
            },
            text: i.text.clone(),
            state: i.state,
        })
        .collect();
    Some(Plan { title: format!("TODO · {done}/{}", items.len()), rows })
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
