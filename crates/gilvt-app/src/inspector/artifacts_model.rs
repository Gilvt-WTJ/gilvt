//! The 「产物」 tab's model (M4a spec §4, tasks spec §3–4): one card per task — a prompt and the reply-only
//! follow-ups after it — from the timeline's turns and the snapshot ledger, with the task's net diff. Pure:
//! windows and files are not touched here.

mod net;
mod tasks;

use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gilvt_agent::{Item, ItemStatus, Turn};
use gilvt_snapshot::ledger::{prompt_head, TurnRecord, TurnState};
use gilvt_snapshot::{ChangeStatus, FileChange, TreeId};
use gilvt_viewer::diff::{diff_texts, LineKind};

use super::model::elapsed_label;
use super::timeline_model::is_shell;

/// Files a card lists before 「按目录分组查看全部」.
pub const SHOWN_FILES: usize = 8;
const TITLE_MAX: usize = 48;
/// A timeline turn and a ledger record are the same turn when they started this close together.
const SAME_TURN_SLACK_MS: u64 = 30_000;
const TEST_COMMANDS: [&str; 6] = ["go test", "npm test", "pnpm test", "pytest", "cargo test", "make test"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestResult {
    pub command: String,
    pub ok: bool,
    pub exit: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    NotGit,
    NoSnapshot,
    Failed(String),
    Lost,
    LargeSkipped(usize),
    /// The task's net diff could not be computed (the card falls back to the union of its turns).
    DiffFailed(String),
    /// A notice about one turn of a multi-turn task.
    InTurn(u32, Box<Notice>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CardState {
    Done,
    Running,
    Quiet,
    Degraded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRow {
    pub path: String,
    pub abs: PathBuf,
    pub status: char,
    pub added: u32,
    pub removed: u32,
    pub binary: bool,
    pub old_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardRange {
    pub repo_root: PathBuf,
    pub before: TreeId,
    pub after: TreeId,
    /// Quick Look's label: 本轮（第 3 轮前 → 后）/ 本任务（第 5 轮前 → 第 7 轮后）/ 本会话（…）.
    pub label: String,
    /// 这一轮 / 这个任务 / 本会话.
    pub scope: &'static str,
}

/// One task: a prompt and the reply-only follow-ups after it.
#[derive(Clone, Debug, PartialEq)]
pub struct ArtCard {
    /// The lead's key: its record's seq, or 1_000_000 + its turn index.
    pub key: u32,
    /// The lead's timeline number.
    pub turn: Option<u32>,
    /// The members' timeline numbers, oldest first.
    pub turns: Vec<u32>,
    pub title: String,
    /// Prompt heads of the follow-ups.
    pub follow_ups: Vec<String>,
    /// Clock of the lead's start ("" when unknown).
    pub started: String,
    pub took: Option<String>,
    pub state: CardState,
    pub files: Vec<FileRow>,
    /// A multi-turn net diff is still being computed.
    pub computing: bool,
    /// The files are the members' union, not a net diff: draw no +/− numbers.
    pub counts_hidden: bool,
    pub test: Option<TestResult>,
    pub quote: String,
    pub touched_later: bool,
    /// The first later turn that changed one of these files again.
    pub touched_later_turn: Option<u32>,
    pub notices: Vec<Notice>,
    pub range: Option<CardRange>,
    /// Running only: the lead's `before` (the live list is measured from it).
    pub live_base: Option<TreeId>,
    pub quiet_group: Option<u32>,
}

/// The summary line: the session net's files and line counts, or — until the net is in — the Done cards'
/// files without counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub files: usize,
    pub added: Option<u32>,
    pub removed: Option<u32>,
    pub computing: bool,
}

/// The session's net diff (tasks spec §4.4): the newest repository's first `before` to its last Done
/// `after`, limited to the files some task touched.
#[derive(Clone, Debug, PartialEq)]
pub struct Net {
    /// Label 本会话（第 a 轮前 → 第 b 轮后）, scope 本会话.
    pub range: CardRange,
    /// 「14:02 第 2 轮前 → 14:31 第 7 轮后」.
    pub range_label: String,
    /// Files some turn touched; empty while computing.
    pub files: Vec<FileRow>,
    /// Files in the diff that no turn touched (changed by hand or by another program).
    pub excluded: usize,
    /// The repository's name when the session's turns span several (only the newest one counts).
    pub only_repo: Option<String>,
    pub computing: bool,
    pub failed: Option<String>,
}

/// A run of Quiet cards folded into one row, keyed by its newest card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuietGroup {
    pub key: u32,
    /// Card keys, newest first.
    pub cards: Vec<u32>,
    pub titles: Vec<String>,
}

/// One drawn block: an index into `cards` or `quiet_groups`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    Card(usize),
    Quiet(usize),
}

/// `cards` are newest first; `blocks` are the drawing order.
#[derive(Clone, Debug, PartialEq)]
pub struct Artifacts {
    pub summary: Option<Summary>,
    pub net: Option<Net>,
    pub cards: Vec<ArtCard>,
    pub quiet_groups: Vec<QuietGroup>,
    pub blocks: Vec<Block>,
}

/// A cached cross-turn diff (None: not computed yet; asking starts it).
pub type DiffFn<'a> = &'a dyn Fn(&CardRange) -> Option<Result<Vec<FileChange>, String>>;

pub struct Inputs<'a> {
    /// The timeline's turns as `TimelineView` holds them (shared, so a redraw copies no items).
    pub turns: &'a [Arc<Turn>],
    pub records: &'a [TurnRecord],
    /// The files the running turn has changed so far.
    pub live: Option<&'a [FileChange]>,
    /// Where files are when a record does not say (the session's working directory).
    pub fallback_root: Option<&'a Path>,
    /// `clock_label` of a start / end time (injected so tests are independent of the local zone).
    pub clock: &'a dyn Fn(SystemTime) -> String,
    /// A cached cross-turn diff (None: not computed yet; asking starts it).
    pub diff: DiffFn<'a>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sel {
    Net,
    NetFile(usize),
    Card(u32),
    File(u32, usize),
    Quiet(u32),
}

/// What is unfolded: the session net, each card, a card's 「全部文件」, each quiet group.
pub struct Open<'a> {
    pub net: bool,
    pub card: &'a dyn Fn(&ArtCard) -> bool,
    pub all: &'a dyn Fn(u32) -> bool,
    pub quiet: &'a dyn Fn(u32) -> bool,
}

impl Notice {
    pub fn text(&self) -> String {
        match self {
            Notice::NotGit => crate::i18n::text(
                "非 git 目录，暂不记录文件改动",
                "Not a Git directory; file changes are not recorded",
            )
            .into(),
            Notice::NoSnapshot => crate::i18n::text(
                "无快照：这一轮发生时 gilvt 没在记录",
                "No snapshot: gilvt was not recording during this turn",
            )
            .into(),
            Notice::Failed(why) if crate::i18n::current() == crate::i18n::Language::English => {
                format!("Snapshot failed: {why}")
            }
            Notice::Failed(why) => format!("快照失败：{why}"),
            Notice::Lost => crate::i18n::text(
                "中断：没有看到这一轮的结束",
                "Interrupted: the end of this turn was not observed",
            )
            .into(),
            Notice::LargeSkipped(n) if crate::i18n::current() == crate::i18n::Language::English => {
                format!("Skipped {n} large files")
            }
            Notice::LargeSkipped(n) => format!("{n} 个大文件已跳过"),
            Notice::DiffFailed(why) if crate::i18n::current() == crate::i18n::Language::English => {
                format!("Failed to calculate net changes: {why}")
            }
            Notice::DiffFailed(why) => format!("净改动计算失败：{why}"),
            Notice::InTurn(n, inner)
                if crate::i18n::current() == crate::i18n::Language::English =>
            {
                format!("Turn {n}: {}", inner.text())
            }
            Notice::InTurn(n, inner) => format!("第 {n} 轮：{}", inner.text()),
        }
    }
}

impl CardState {
    pub fn name(self) -> &'static str {
        match self {
            CardState::Done => "done",
            CardState::Running => "running",
            CardState::Quiet => "quiet",
            CardState::Degraded => "degraded",
        }
    }
}

/// 「刚刚」 / 「N 分钟前」 / 「N 小时前」 / 「N 天前」 (the session review's header still uses it).
pub fn relative(now: SystemTime, then: SystemTime) -> String {
    let secs = now.duration_since(then).map_or(0, |d| d.as_secs());
    match secs {
        0..60 => crate::i18n::text("刚刚", "Just now").into(),
        60..3600 if crate::i18n::current() == crate::i18n::Language::English => {
            format!("{}m ago", secs / 60)
        }
        60..3600 => format!("{} 分钟前", secs / 60),
        3600..86_400 if crate::i18n::current() == crate::i18n::Language::English => {
            format!("{}h ago", secs / 3600)
        }
        3600..86_400 => format!("{} 小时前", secs / 3600),
        _ if crate::i18n::current() == crate::i18n::Language::English => {
            format!("{}d ago", secs / 86_400)
        }
        _ => format!("{} 天前", secs / 86_400),
    }
}

fn millis(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
}

/// Pairs each timeline turn with the ledger record of the same turn (in order): same prompt head, and when
/// both start times are known, started within 30 s of each other.
pub fn match_turns<T: Borrow<Turn>>(turns: &[T], records: &[TurnRecord]) -> Vec<Option<usize>> {
    let mut used = vec![false; records.len()];
    turns
        .iter()
        .map(|t| {
            let t: &Turn = t.borrow();
            let head = prompt_head(&t.prompt);
            let started = t.started.and_then(millis);
            let mut best: Option<(usize, u64)> = None;
            for (i, r) in records.iter().enumerate().filter(|(i, _)| !used[*i]) {
                if r.prompt != head {
                    continue;
                }
                let gap = match (started, r.started_ms) {
                    (Some(a), b) if b != 0 => a.abs_diff(b),
                    _ => 0,
                };
                if gap <= SAME_TURN_SLACK_MS && best.is_none_or(|(_, g)| gap < g) {
                    best = Some((i, gap));
                }
            }
            best.map(|(i, _)| {
                used[i] = true;
                i
            })
        })
        .collect()
}

/// The last finished test / build command of the turn, with its outcome.
pub fn test_result(turn: &Turn) -> Option<TestResult> {
    turn.items.iter().rev().find_map(|item| match item {
        Item::Tool(t) if is_shell(&t.tool) && TEST_COMMANDS.iter().any(|c| t.summary.contains(c)) => match &t.status {
            ItemStatus::Ok => Some(TestResult { command: t.summary.clone(), ok: true, exit: None }),
            ItemStatus::Failed { exit } => Some(TestResult { command: t.summary.clone(), ok: false, exit: *exit }),
            _ => None,
        },
        _ => None,
    })
}

fn file_row(c: &FileChange, root: Option<&Path>) -> FileRow {
    FileRow {
        path: c.path.clone(),
        abs: root.map_or_else(|| PathBuf::from(&c.path), |r| r.join(&c.path)),
        status: match c.status {
            ChangeStatus::M => 'M',
            ChangeStatus::A => 'A',
            ChangeStatus::D => 'D',
            ChangeStatus::R => 'R',
        },
        added: c.added,
        removed: c.removed,
        binary: c.binary,
        old_path: c.old_path.clone(),
    }
}

/// Every turn, paired with its ledger record, plus the records whose turn left the timeline: oldest first.
fn members<'a>(i: &Inputs<'a>) -> Vec<tasks::Member<'a>> {
    let matched = match_turns(i.turns, i.records);
    let mut used = vec![false; i.records.len()];
    let mut entries: Vec<(u64, Option<&'a Turn>, Option<&'a TurnRecord>)> = Vec::new();
    for (t, m) in i.turns.iter().zip(&matched) {
        let rec = m.map(|ix| {
            used[ix] = true;
            &i.records[ix]
        });
        let when = t.started.and_then(millis).or_else(|| rec.map(|r| r.started_ms)).unwrap_or(0);
        entries.push((when, Some(&**t), rec));
    }
    for (_, r) in i.records.iter().enumerate().filter(|(ix, _)| !used[*ix]) {
        entries.push((r.started_ms, None, Some(r)));
    }
    entries.sort_by_key(|e| e.0);
    let last = i.records.last();
    entries.into_iter().map(|(_, turn, rec)| tasks::Member { turn, rec, last: rec.is_some_and(|r| last.is_some_and(|l| std::ptr::eq(r, l))) }).collect()
}

pub fn build(i: &Inputs) -> Artifacts {
    let members = members(i);
    let mut cards: Vec<ArtCard> = tasks::group(&members).iter().map(|t| tasks::card(t, i)).collect();
    // Newest first: each card learns which of its files a later task changed again, and the first such turn.
    let mut later: HashMap<String, Option<u32>> = HashMap::new();
    for c in cards.iter_mut().rev() {
        let hit: Vec<Option<u32>> = c.files.iter().filter_map(|f| later.get(&f.path).copied()).collect();
        c.touched_later = !hit.is_empty();
        c.touched_later_turn = hit.into_iter().flatten().min();
        for f in &c.files {
            later.insert(f.path.clone(), c.turns.first().copied());
        }
    }
    cards.reverse(); // newest first
    let (quiet_groups, blocks) = fold_quiet(&mut cards);
    let net = net::session(&members, i);
    let summary = net::summary(&cards, net.as_ref());
    Artifacts { summary, net, cards, quiet_groups, blocks }
}

/// Runs of Quiet cards (newest first) become groups keyed by their newest card.
fn fold_quiet(cards: &mut [ArtCard]) -> (Vec<QuietGroup>, Vec<Block>) {
    let (mut groups, mut blocks): (Vec<QuietGroup>, Vec<Block>) = (Vec::new(), Vec::new());
    for (ix, card) in cards.iter_mut().enumerate() {
        if card.state != CardState::Quiet {
            blocks.push(Block::Card(ix));
            continue;
        }
        // The last block is a group exactly when the card before this one was Quiet.
        if !matches!(blocks.last(), Some(Block::Quiet(_))) {
            groups.push(QuietGroup { key: card.key, cards: Vec::new(), titles: Vec::new() });
            blocks.push(Block::Quiet(groups.len() - 1));
        }
        let g = groups.last_mut().expect("pushed above");
        g.cards.push(card.key);
        g.titles.push(card.title.clone());
        card.quiet_group = Some(g.key);
    }
    (groups, blocks)
}

/// The newest card that is not in a quiet group (open by default).
pub fn newest_open_by_default(a: &Artifacts) -> Option<u32> {
    a.blocks.iter().find_map(|b| match b {
        Block::Card(i) => Some(a.cards[*i].key),
        Block::Quiet(_) => None,
    })
}

/// The rows an arrow key walks over, in drawing order: the session net (and its files when open), then the
/// blocks — a card followed, when open, by the files it lists (at most [`SHOWN_FILES`] unless `all` says
/// to list them all), and a quiet group followed, when open, by its cards.
pub fn nav_rows(a: &Artifacts, open: &Open) -> Vec<Sel> {
    let mut rows = Vec::new();
    if let Some(n) = &a.net {
        rows.push(Sel::Net);
        if open.net {
            rows.extend((0..n.files.len()).map(Sel::NetFile));
        }
    }
    let card = |rows: &mut Vec<Sel>, c: &ArtCard| {
        rows.push(Sel::Card(c.key));
        if (open.card)(c) && c.state != CardState::Quiet {
            let shown = if (open.all)(c.key) { c.files.len() } else { c.files.len().min(SHOWN_FILES) };
            rows.extend((0..shown).map(|i| Sel::File(c.key, i)));
        }
    };
    for b in &a.blocks {
        match b {
            Block::Card(i) => card(&mut rows, &a.cards[*i]),
            Block::Quiet(g) => {
                let g = &a.quiet_groups[*g];
                rows.push(Sel::Quiet(g.key));
                if (open.quiet)(g.key) {
                    for k in &g.cards {
                        if let Some(c) = a.cards.iter().find(|c| c.key == *k) {
                            card(&mut rows, c);
                        }
                    }
                }
            }
        }
    }
    rows
}

/// The row `delta` steps from `cur` (clamped to the list); with nothing selected, or a row that is gone,
/// the first row.
pub fn step(rows: &[Sel], cur: Option<Sel>, delta: i32) -> Option<Sel> {
    let first = *rows.first()?;
    let Some(at) = cur.and_then(|c| rows.iter().position(|r| *r == c)) else { return Some(first) };
    let next = (at as i64 + i64::from(delta)).clamp(0, rows.len() as i64 - 1) as usize;
    Some(rows[next])
}

/// The 1-based line of the first change between `old` and `new` (1 when they are the same).
pub fn first_changed_line(old: &str, new: &str) -> u32 {
    diff_texts(old, new).lines.iter().find(|l| l.kind != LineKind::Context).and_then(|l| l.new_no.or(l.old_no)).unwrap_or(1)
}

#[cfg(test)]
#[path = "artifacts_model_tests.rs"]
mod tests;

#[cfg(test)]
pub mod tests_support {
    use super::*;

    /// A Done card for turn `key` with no files and every other field empty.
    pub fn card(key: u32) -> ArtCard {
        ArtCard {
            key,
            turn: Some(key),
            turns: vec![key],
            title: String::new(),
            follow_ups: Vec::new(),
            started: String::new(),
            took: None,
            state: CardState::Done,
            files: Vec::new(),
            computing: false,
            counts_hidden: false,
            test: None,
            quote: String::new(),
            touched_later: false,
            touched_later_turn: None,
            notices: Vec::new(),
            range: None,
            live_base: None,
            quiet_group: None,
        }
    }
}
