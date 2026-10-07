use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gilvt_agent::{AgentKind, ReviewOutcome, RuntimeRef, Session, SessionKey, TurnCursor};

use crate::review::{ReviewInboxItem, ReviewPriority, ReviewRuntime};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    NeedsYou,
    Review,
    Running,
    #[default]
    All,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::NeedsYou, Tab::Review, Tab::Running, Tab::All];

    pub fn label(self) -> &'static str {
        match self {
            Tab::NeedsYou => "需要你",
            Tab::Review => "待 Review",
            Tab::Running => "运行中",
            Tab::All => "全部会话",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub needs_you: usize,
    pub review: usize,
    pub running: usize,
    pub all: usize,
}

impl Counts {
    pub fn for_tab(&self, tab: Tab) -> usize {
        match tab {
            Tab::NeedsYou => self.needs_you,
            Tab::Review => self.review,
            Tab::Running => self.running,
            Tab::All => self.all,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveSession {
    pub key: SessionKey,
    pub pane: Option<gilvt_agent::PaneId>,
    pub runtime: Option<RuntimeRef>,
    pub title: String,
    pub cwd: PathBuf,
    pub needs_you: bool,
    pub waiting_for: Option<Duration>,
}

pub fn live_sessions<'a>(
    sessions: impl IntoIterator<Item = &'a Session>,
    now: Instant,
) -> Vec<LiveSession> {
    live_sessions_with_runtime(sessions, now, |_| None)
}

pub fn live_sessions_with_runtime<'a>(
    sessions: impl IntoIterator<Item = &'a Session>,
    now: Instant,
    runtime: impl Fn(&SessionKey) -> Option<RuntimeRef>,
) -> Vec<LiveSession> {
    sessions
        .into_iter()
        .filter(|session| session.is_live())
        .map(|session| LiveSession {
            key: session.key.clone(),
            pane: session.pane,
            runtime: runtime(&session.key),
            title: if session.name.trim().is_empty() {
                "新会话".into()
            } else {
                session.name.clone()
            },
            cwd: session.cwd.clone().unwrap_or_default(),
            needs_you: session.needs_you(),
            waiting_for: session
                .waiting_since
                .map(|since| now.saturating_duration_since(since)),
        })
        .collect()
}

/// What the inbox needs to know about sessions running in gilvt.
pub fn runtime_map(live: &[LiveSession]) -> HashMap<SessionKey, ReviewRuntime> {
    live.iter()
        .map(|item| {
            (
                item.key.clone(),
                ReviewRuntime {
                    running_in_gilvt: true,
                    needs_you: item.needs_you,
                    waiting_for: item.waiting_for,
                },
            )
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub key: SessionKey,
    pub title: String,
    pub cwd: PathBuf,
    pub priority: Option<ReviewPriority>,
    pub unreviewed_count: u32,
    pub latest_outcome: Option<ReviewOutcome>,
    pub tool_count: u32,
    pub failed_tool_count: u32,
    pub lines_added: u32,
    pub lines_removed: u32,
    pub pinned: bool,
    /// The saved review position is no longer in the transcript (it was truncated or replaced).
    pub cursor_stale: bool,
    /// What else a search matches besides `title` (the agent's titles, the prompts it started with).
    pub search_terms: Vec<String>,
    /// The session's last completed turn as the queue showed it; None for a session with nothing pending.
    pub snapshot_through: Option<TurnCursor>,
    pub running: bool,
    pub runtime: Option<RuntimeRef>,
    pub needs_you: bool,
    pub waiting_for: Option<Duration>,
}

impl Row {
    pub fn agent_label(&self) -> &'static str {
        match self.key.0 {
            AgentKind::Claude => "Claude",
            AgentKind::Codex => "Codex",
        }
    }
}

/// Where a queue row's session ran, as the palette shows it (spec §5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowDir {
    /// The directory as the palette shortens it (`~/…/proj/sub`); "" when the cwd is unknown.
    pub dir: String,
    /// The git branch of the directory, when the git cache already knows it.
    pub branch: Option<String>,
    /// The directory no longer exists: drawn struck through with 「目录已不存在」.
    pub missing: bool,
}

/// `cwd`'s [`RowDir`]: `home` as the palette's (`dir_label::label_home`), `exists` the history scan's answer
/// (unknown directories exist), `branch` the git cache. An empty `cwd` (no history entry) shows nothing.
pub fn row_dir(
    cwd: &Path,
    home: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
    branch: impl Fn(&Path) -> Option<String>,
) -> RowDir {
    if cwd.as_os_str().is_empty() {
        return RowDir::default();
    }
    RowDir {
        dir: crate::launcher::shorten_dir(cwd, home, crate::launcher::DIR_MAX_CHARS),
        branch: branch(cwd),
        missing: !exists(cwd),
    }
}

/// The review header's grey line: 「<目录>[ · <分支>] · <agent>」.
pub fn header_subtitle(row: &Row, dir: &RowDir) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !dir.dir.is_empty() {
        parts.push(dir.dir.clone());
    }
    parts.extend(dir.branch.iter().cloned());
    if let Some(runtime) = &row.runtime {
        parts.push(runtime.location_label());
    }
    parts.push(row.agent_label().into());
    parts.join(" · ")
}

/// A queue row's grey line: the header's, then 「 · N tools[ · 有失败]」 and 「 · +a -r」 when there are any.
pub fn queue_subtitle(row: &Row, dir: &RowDir) -> String {
    let mut text = header_subtitle(row, dir);
    if row.tool_count > 0 {
        text.push_str(&format!(" · {} tools", row.tool_count));
        if row.failed_tool_count > 0 {
            text.push_str(" · 有失败");
        }
    }
    if row.lines_added + row.lines_removed > 0 {
        text.push_str(&format!(" · +{} -{}", row.lines_added, row.lines_removed));
    }
    text
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct List {
    pub counts: Counts,
    pub rows: Vec<Row>,
}

pub fn build(
    tab: Tab,
    query: &str,
    history_total: usize,
    inbox: &[ReviewInboxItem],
    live: &[LiveSession],
) -> List {
    let inbox_by_key: HashMap<&SessionKey, &ReviewInboxItem> =
        inbox.iter().map(|item| (&item.key, item)).collect();
    let live_by_key: HashMap<&SessionKey, &LiveSession> =
        live.iter().map(|item| (&item.key, item)).collect();
    let counts = Counts {
        needs_you: live.iter().filter(|item| item.needs_you).count(),
        review: inbox
            .iter()
            .filter(|item| item.priority != ReviewPriority::NeedsYou)
            .count(),
        running: live.len(),
        all: history_total,
    };
    let mut rows = match tab {
        Tab::NeedsYou => live
            .iter()
            .filter(|item| item.needs_you)
            .map(|item| row(Some(item), inbox_by_key.get(&item.key).copied()))
            .collect(),
        Tab::Review => inbox
            .iter()
            .filter(|item| item.priority != ReviewPriority::NeedsYou)
            .map(|item| row(live_by_key.get(&item.key).copied(), Some(item)))
            .collect(),
        Tab::Running => live
            .iter()
            .map(|item| row(Some(item), inbox_by_key.get(&item.key).copied()))
            .collect(),
        Tab::All => Vec::new(),
    };
    let query = query.trim().to_lowercase();
    if !query.is_empty() {
        rows.retain(|item| matches_query(item, &query));
    }
    if tab == Tab::NeedsYou {
        rows.sort_by(|a, b| {
            b.waiting_for
                .cmp(&a.waiting_for)
                .then_with(|| a.key.cmp(&b.key))
        });
    } else if tab == Tab::Running {
        rows.sort_by(|a, b| {
            b.needs_you
                .cmp(&a.needs_you)
                .then_with(|| a.key.cmp(&b.key))
        });
    }
    List { counts, rows }
}

fn row(live: Option<&LiveSession>, inbox: Option<&ReviewInboxItem>) -> Row {
    let key = inbox
        .map(|item| item.key.clone())
        .or_else(|| live.map(|item| item.key.clone()))
        .expect("row source");
    Row {
        key,
        title: live
            .map(|item| item.title.clone())
            .or_else(|| inbox.map(|item| item.title.clone()))
            .unwrap_or_default(),
        cwd: live
            .map(|item| item.cwd.clone())
            .or_else(|| inbox.map(|item| item.cwd.clone()))
            .unwrap_or_default(),
        priority: inbox.map(|item| item.priority),
        unreviewed_count: inbox.map_or(0, |item| item.unreviewed_count),
        latest_outcome: inbox.map(|item| item.latest_outcome.clone()),
        tool_count: inbox.map_or(0, |item| item.tool_count),
        failed_tool_count: inbox.map_or(0, |item| item.failed_tool_count),
        lines_added: inbox.map_or(0, |item| item.lines_added),
        lines_removed: inbox.map_or(0, |item| item.lines_removed),
        pinned: inbox.is_some_and(|item| item.pinned),
        cursor_stale: inbox.is_some_and(|item| item.cursor_stale),
        search_terms: inbox.map(|item| item.search_terms.clone()).unwrap_or_default(),
        snapshot_through: inbox.map(|item| item.snapshot_through.clone()),
        running: live.is_some(),
        runtime: live.and_then(|item| item.runtime.clone()),
        needs_you: live.is_some_and(|item| item.needs_you),
        waiting_for: live.and_then(|item| item.waiting_for),
    }
}

fn matches_query(row: &Row, query: &str) -> bool {
    row.title.to_lowercase().contains(query)
        || row
            .search_terms
            .iter()
            .any(|term| term.to_lowercase().contains(query))
        || row.cwd.to_string_lossy().to_lowercase().contains(query)
        || row.key.1.to_lowercase().starts_with(query)
        || row.agent_label().to_lowercase().contains(query)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    cursor: usize,
    key: Option<SessionKey>,
}

impl Selection {
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn selected<'a>(&self, rows: &'a [Row]) -> Option<&'a Row> {
        rows.get(self.cursor)
    }

    pub fn sync(&mut self, rows: &[Row], preserve: bool) {
        if rows.is_empty() {
            self.cursor = 0;
            self.key = None;
            return;
        }
        self.cursor = if preserve {
            self.key
                .as_ref()
                .and_then(|key| rows.iter().position(|row| &row.key == key))
                .unwrap_or(self.cursor.min(rows.len() - 1))
        } else {
            0
        };
        self.key = Some(rows[self.cursor].key.clone());
    }

    pub fn step(&mut self, rows: &[Row], down: bool) {
        if rows.is_empty() {
            return;
        }
        self.cursor = if down {
            (self.cursor + 1).min(rows.len() - 1)
        } else {
            self.cursor.saturating_sub(1)
        };
        self.key = Some(rows[self.cursor].key.clone());
    }

    pub fn set(&mut self, rows: &[Row], index: usize) {
        if let Some(row) = rows.get(index) {
            self.cursor = index;
            self.key = Some(row.key.clone());
        }
    }
}
