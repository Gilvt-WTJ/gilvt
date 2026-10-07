//! The inspector list built from a timeline: the entries (head, title, current turn, history turns),
//! the per-turn row cache, list splices, autoscroll and what a row click does.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use gilvt_agent::{Anchor, Turn, TurnOutcome};
use gilvt_term::ScrollOutcome;

use super::{turn_rows, Expanded, Filter, Row, RowKind, Timing};
use crate::inspector::model::elapsed_label;
use crate::launcher::sessions_model::{clock_label, local};

/// When a turn started, in local time seen now: 「14:30:12」 (`seconds`) / 「14:30」 today, 「昨天 14:30」 before.
pub fn local_clock(t: SystemTime, seconds: bool) -> String {
    clock_label(local(t), local(SystemTime::now()), seconds)
}

/// A history turn's line: 「第 2 轮 · 重构路由注册 · 9 步 · 14:10 · 2m03s ✓」 (the view adds ▸ / ▾); `started`
/// is when the turn started (left out when unknown).
pub fn history_label(turn: &Turn, started: Option<&str>) -> String {
    let prompt = turn
        .prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(crate::i18n::text("（无提示词）", "(no prompt)"));
    let prompt = gilvt_agent::truncate_chars(prompt, 40);
    let english = crate::i18n::current() == crate::i18n::Language::English;
    let mut parts = vec![
        if english {
            format!("Turn {}", turn.index)
        } else {
            format!("第 {} 轮", turn.index)
        },
        prompt,
        if english {
            format!("{} steps", turn.steps)
        } else {
            format!("{} 步", turn.steps)
        },
    ];
    parts.extend(started.map(str::to_string));
    let took = match (turn.started, turn.ended) {
        (Some(a), Some(b)) => b.duration_since(a).ok().map(elapsed_label),
        _ => None,
    };
    let mark = match turn.outcome {
        TurnOutcome::Done => "✓",
        TurnOutcome::Failed { .. } | TurnOutcome::Interrupted => "✗",
        TurnOutcome::Running => "…",
    };
    parts.push(match took {
        Some(t) => format!("{t} {mark}"),
        None => mark.to_string(),
    });
    parts.join(" · ")
}

/// One item of the inspector's list.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// Banner, status card, TODO (drawn from the view's own data).
    Head,
    /// 「时间线 · 第 N 轮 · 14:30:12」 and the filters; `started`: when the turn started (None: unknown).
    Title { turn: u32, filter: Filter, started: Option<String> },
    Row(Rc<Row>),
    /// 「本轮暂无事件」 / 「本轮没有符合的事件」.
    Note(&'static str),
    /// A history turn's line; `started` is the clock in it (None: unknown).
    History { index: u32, label: String, open: bool, started: Option<String> },
}

/// Built rows of each turn, reused while the turn is the same `Arc` and neither filter nor expansion
/// changed (`gen`). Only turns asked for in the last pass are kept.
pub struct RowCache {
    turns: HashMap<(u32, bool), (Arc<Turn>, Filter, u64, Rc<Vec<Rc<Row>>>)>,
    /// Turns built (not reused) so far (tests).
    pub builds: usize,
    /// When a turn started, as text ([`local_clock`]; tests fix it).
    clock: fn(SystemTime, bool) -> String,
}

impl Default for RowCache {
    fn default() -> Self {
        RowCache::with_clock(local_clock)
    }
}

impl RowCache {
    pub fn with_clock(clock: fn(SystemTime, bool) -> String) -> Self {
        RowCache { turns: HashMap::new(), builds: 0, clock }
    }

    fn rows(&mut self, next: &mut HashMap<(u32, bool), (Arc<Turn>, Filter, u64, Rc<Vec<Rc<Row>>>)>, turn: &Arc<Turn>, filter: Filter, open: &Expanded, gen: u64, history: bool) -> Rc<Vec<Rc<Row>>> {
        let key = (turn.index, history);
        let rows = match self.turns.remove(&key) {
            Some((t, f, g, rows)) if Arc::ptr_eq(&t, turn) && f == filter && g == gen => rows,
            _ => {
                self.builds += 1;
                Rc::new(turn_rows(turn, filter, open, history).into_iter().map(Rc::new).collect())
            }
        };
        next.insert(key, (turn.clone(), filter, gen, rows.clone()));
        rows
    }

    /// The inspector's list below the head: title + filters, the current turn's rows newest first (or a note), then the
    /// history turns newest first, each followed by its rows when expanded. `gen` changes whenever
    /// `filter` / `open` do.
    pub fn entries(&mut self, turns: &[Arc<Turn>], filter: Filter, open: &Expanded, gen: u64) -> Vec<Entry> {
        let mut next = HashMap::new();
        let mut out = vec![Entry::Head];
        if let Some((current, older)) = turns.split_last() {
            let started = current.started.map(|t| (self.clock)(t, true));
            out.push(Entry::Title { turn: current.index, filter, started });
            let rows = self.rows(&mut next, current, filter, open, gen, false);
            if rows.is_empty() {
                out.push(Entry::Note(if current.items.is_empty() { "本轮暂无事件" } else { "本轮没有符合的事件" }));
            }
            out.extend(rows.iter().cloned().map(Entry::Row));
            for turn in older.iter().rev() {
                let expanded = open.turns.contains(&turn.index);
                let started = turn.started.map(|t| (self.clock)(t, false));
                let label = history_label(turn, started.as_deref());
                out.push(Entry::History { index: turn.index, label, open: expanded, started });
                if expanded {
                    let rows = self.rows(&mut next, turn, filter, open, gen, true);
                    if rows.is_empty() {
                        let note = if turn.items.is_empty() { "该轮的明细已不再保留" } else { "该轮没有符合的事件" };
                        out.push(Entry::Row(Rc::new(Row { key: format!("none:{}", turn.index), sub: false, history: true, kind: RowKind::Empty(note) })));
                    }
                    out.extend(rows.iter().cloned().map(Entry::Row));
                }
            }
        }
        self.turns = next;
        out
    }
}

/// The smallest change turning `old` into `new`: the old range to replace and how many new items replace
/// it. None when equal.
pub fn splice_range<T: PartialEq>(old: &[T], new: &[T]) -> Option<(Range<usize>, usize)> {
    if old == new {
        return None;
    }
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old.iter().rev().zip(new.iter().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();
    Some((prefix..old.len() - suffix, new.len() - prefix - suffix))
}

/// What a click on a row does after trying its anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jump {
    /// Scrolled; the terminal highlights 3 rows for 1 s.
    Highlight,
    Toast(&'static str),
    /// Nothing to scroll to: the click opens / closes the row's detail instead.
    ToggleDetail,
}

pub const EVICTED: &str = "已超出回滚范围";
pub const PANE_CLOSED: &str = "该 pane 已关闭";

/// The click's result: no anchor → detail; `scroll` None (the anchor's pane is gone) → 「该 pane 已关闭」;
/// evicted → 「已超出回滚范围」; a full-screen program on the alternate screen → detail.
pub fn jump(anchor: Option<Anchor>, scroll: impl FnOnce(Anchor) -> Option<ScrollOutcome>) -> Jump {
    let Some(anchor) = anchor else { return Jump::ToggleDetail };
    match scroll(anchor) {
        None => Jump::Toast(PANE_CLOSED),
        Some(ScrollOutcome::Shown { .. }) => Jump::Highlight,
        Some(ScrollOutcome::Evicted) => Jump::Toast(EVICTED),
        Some(ScrollOutcome::AltScreen) => Jump::ToggleDetail,
    }
}

/// Whether running rows need the 1 s ticker: only while the followed session is live (an ended session's
/// open rows never finish, so their elapsed time must not keep redrawing the inspector).
pub fn rows_need_tick(session_live: bool, entries: &[Entry]) -> bool {
    session_live && has_running(entries)
}

/// Whether any drawn row runs (its time must be redrawn every second).
pub fn has_running(entries: &[Entry]) -> bool {
    entries.iter().any(|e| matches!(e, Entry::Row(r) if matches!(&r.kind, RowKind::Tool(t) if matches!(t.timing, Timing::Running(_)))))
}
