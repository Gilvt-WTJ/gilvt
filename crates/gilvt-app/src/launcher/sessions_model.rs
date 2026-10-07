//! The 会话 palette (⌘⇧R, M3c spec §4; mockup m3c-sessions.html, option A) as pure data: which past sessions
//! it lists under the filters and the query, how each row reads, the keys, the row menu, the rename value and
//! the confirm bar's copy. `select` is the cursor and the multi-selection, `time` the relative times and
//! sizes. `sessions_view` draws the list and acts on it.

mod select;
mod time;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gilvt_agent::{AgentKind, HistoryEntry, PaneId, RuntimeRef, SessionKey, TitleSource};
use gpui::Modifiers;

use super::dir_label::{shorten_dir, DIR_MAX_CHARS};
use super::history::{title, title_source};
use super::Location;

pub use select::{Click, Selection};
pub use time::{clock_label, local, size_label, when_label};
#[cfg(test)]
use time::LocalTime;

/// 「≥ 7 天未活动」.
pub const STALE_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);

/// The two exclusive chips: the current project (default) or 全部项目.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Current,
    All,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Filters {
    pub scope: Scope,
    /// 「≥ 7 天未活动」, on top of either scope.
    pub stale: bool,
    /// 「已归档」: list only the archived sessions (they are hidden otherwise).
    pub archived: bool,
}

/// A directory's project, as the sidebar groups sessions: its git root (else the directory itself) and name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Project {
    pub root: PathBuf,
    pub name: String,
}

/// A running session: its pane (None when no window has it) and where that pane is ("左上 pane").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveAt {
    pub pane: Option<PaneId>,
    pub location: String,
    pub runtime: Option<RuntimeRef>,
}

/// Where a pane is, relative to the window showing the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneAt {
    /// Some(n): in window n (1-based, gpui's order), not the palette's.
    pub window: Option<usize>,
    /// 1-based tab number in its window.
    pub tab: usize,
    /// Its tab is the active one of its window.
    pub active_tab: bool,
    /// `workspace::nav::position_word`: 左上 …, "" when alone in its tab.
    pub position: &'static str,
}

/// 「左上 pane」, 「标签 2 · 右 pane」, 「窗口 2 · 标签 1」; 「当前 pane」 for the palette window's focused pane
/// alone in the active tab.
pub fn location_word(at: &PaneAt) -> String {
    let mut parts = Vec::new();
    if let Some(n) = at.window {
        parts.push(format!("窗口 {n}"));
    }
    if at.window.is_some() || !at.active_tab {
        parts.push(format!("标签 {}", at.tab));
    }
    if !at.position.is_empty() {
        parts.push(format!("{} pane", at.position));
    }
    if parts.is_empty() {
        return "当前 pane".into();
    }
    parts.join(" · ")
}

/// Everything a list is built from.
pub struct Input<'a> {
    /// `History::entries`, newest first.
    pub entries: &'a [HistoryEntry],
    /// Running sessions of the registry.
    pub live: &'a HashMap<SessionKey, LiveAt>,
    /// The sidebar rename (M3a store) of any session.
    pub saved_name: &'a dyn Fn(&SessionKey) -> Option<String>,
    /// A session cwd's project.
    pub project: &'a dyn Fn(&Path) -> Project,
    /// The focused pane's project; None → only 全部项目.
    pub current: Option<&'a Project>,
    pub query: &'a str,
    pub filters: Filters,
    pub now: SystemTime,
    /// "10 分钟前" for a last activity (see [`when_label`]).
    pub when: &'a dyn Fn(SystemTime) -> String,
    /// The home directory, shown as `~` in a row's directory.
    pub home: Option<&'a Path>,
    /// Whether a session's directory still exists (never touches the disk here: the history scan counted).
    pub dir_exists: &'a dyn Fn(&Path) -> bool,
    /// The git branch of a directory, when already known.
    pub branch: &'a dyn Fn(&Path) -> Option<String>,
    /// Whether a session is archived (still archived at its current turn count).
    pub archived: &'a dyn Fn(&HistoryEntry) -> bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: SessionKey,
    /// Index into `Input::entries`.
    pub entry: usize,
    /// C = Claude, X = Codex.
    pub letter: char,
    pub title: String,
    /// Where `title` comes from (the user's rename, the agent's title, or a prompt).
    pub title_source: TitleSource,
    /// "3 轮".
    pub turns_label: String,
    /// The session's directory as the list shows it (`~/…/proj/sub`).
    pub dir: String,
    /// The directory no longer exists (resuming would fail): drawn struck through with a tag.
    pub dir_missing: bool,
    pub branch: Option<String>,
    /// Right side: "10 分钟前", plus " · 18 MB" under the stale filter. Live rows show `live` instead.
    pub when: String,
    pub size: u64,
    pub live: Option<LiveAt>,
    pub archived: bool,
}

impl Row {
    /// The grey line under the title: 「<目录> · <分支> · N 轮」 (no branch segment when unknown).
    pub fn subtitle(&self) -> String {
        match &self.branch {
            Some(b) => format!("{} · {b} · {}", self.dir, self.turns_label),
            None => format!("{} · {}", self.dir, self.turns_label),
        }
    }

    /// Running sessions cannot be picked for the Trash.
    pub fn selectable(&self) -> bool {
        self.live.is_none()
    }

    /// Only a past session can be archived (a running one cannot): the same rows as the Trash's.
    pub fn archivable(&self) -> bool {
        self.selectable()
    }

    /// The right side as drawn: 「● 运行中」, 「● 运行中 · 左上 pane」, else `when`.
    pub fn right(&self) -> String {
        match &self.live {
            Some(live) if live.location.is_empty() => "● 运行中".to_string(),
            Some(live) => format!("● 运行中 · {}", live.location),
            None => self.when.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// "gilvt-lab · 当前项目", "其他项目".
    Header(String),
    /// Index into `List::rows`.
    Row(usize),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct List {
    /// What the virtualized list shows, headers included.
    pub lines: Vec<Line>,
    /// The rows only, in display order (the cursor and the selection index these).
    pub rows: Vec<Row>,
}

impl List {
    /// The line showing row `row`.
    pub fn line_of(&self, row: usize) -> Option<usize> {
        self.lines.iter().position(|l| *l == Line::Row(row))
    }
}

/// The scope actually applied: without a current project there is only 全部项目.
pub fn effective_scope(filters: Filters, current: Option<&Project>) -> Scope {
    if current.is_none() { Scope::All } else { filters.scope }
}

pub fn is_stale(e: &HistoryEntry, now: SystemTime) -> bool {
    now.duration_since(e.last_active).is_ok_and(|d| d >= STALE_AFTER)
}

/// Case-insensitive substring of the title shown, the agent's own titles, the prompts the session started
/// with, project name or cwd; prefix of the session id. So a session is found by what it is called now and by
/// the words it began with. `q` is already trimmed and lowercased.
fn matches(q: &str, title: &str, e: &HistoryEntry, project: &Project) -> bool {
    let has = |s: &str| s.to_lowercase().contains(q);
    has(title)
        || e.custom_title.as_deref().is_some_and(has)
        || e.ai_title.as_deref().is_some_and(has)
        || has(&e.topic_prompt)
        || has(&e.first_prompt)
        || has(&project.name)
        || has(&e.cwd.to_string_lossy())
        || e.session_id.to_lowercase().starts_with(q)
}

/// The palette's list. Current scope: one section, the current project's sessions. 全部项目 while searching:
/// the current project's matches first, then 其他项目. 全部项目 without a query: every session, no sections.
pub fn build(i: &Input) -> List {
    let q = i.query.trim().to_lowercase();
    let scope = effective_scope(i.filters, i.current);
    let (mut ours, mut others) = (Vec::new(), Vec::new());
    for (ix, e) in i.entries.iter().enumerate() {
        let key: SessionKey = (e.agent, e.session_id.clone());
        // A running session is never archived (resumed from 已归档 it is back among the others), as the
        // sidebar shows it.
        let archived = !i.live.contains_key(&key) && (i.archived)(e);
        if archived != i.filters.archived {
            continue;
        }
        if i.filters.stale && !is_stale(e, i.now) {
            continue;
        }
        let project = (i.project)(&e.cwd);
        let own = i.current.is_some_and(|c| c.root == project.root);
        if scope == Scope::Current && !own {
            continue;
        }
        let saved = (i.saved_name)(&key);
        let title_source = title_source(e, saved.clone());
        let title = title(e, saved);
        if !q.is_empty() && !matches(&q, &title, e, &project) {
            continue;
        }
        let live = i.live.get(&key).cloned();
        let when = match (i.filters.stale, &live) {
            (_, Some(_)) => String::new(),
            (true, None) => format!("{} · {}", (i.when)(e.last_active), size_label(e.size)),
            (false, None) => (i.when)(e.last_active),
        };
        let letter = match e.agent {
            AgentKind::Claude => 'C',
            AgentKind::Codex => 'X',
        };
        let row = Row {
            key,
            entry: ix,
            letter,
            title,
            title_source,
            turns_label: format!("{} 轮", e.turns),
            dir: shorten_dir(&e.cwd, i.home, DIR_MAX_CHARS),
            dir_missing: !(i.dir_exists)(&e.cwd),
            branch: (i.branch)(&e.cwd),
            when,
            size: e.size,
            live,
            archived,
        };
        if own { ours.push((row, project)) } else { others.push((row, project)) }
    }
    let plain = |(r, _): (Row, Project)| r;
    let mut list = List::default();
    let section = |list: &mut List, header: Option<String>, rows: Vec<Row>| {
        if rows.is_empty() {
            return;
        }
        list.lines.extend(header.map(Line::Header));
        for row in rows {
            list.lines.push(Line::Row(list.rows.len()));
            list.rows.push(row);
        }
    };
    let here = |c: Option<&Project>| c.map(|c| format!("{} · 当前项目", c.name));
    match (scope, q.is_empty()) {
        (Scope::Current, _) => section(&mut list, here(i.current), ours.into_iter().map(plain).collect()),
        (Scope::All, false) if i.current.is_some() => {
            section(&mut list, here(i.current), ours.into_iter().map(plain).collect());
            section(&mut list, Some("其他项目".into()), others.into_iter().map(plain).collect());
        }
        (Scope::All, _) => {
            // Newest first across projects, as the history lists them.
            let mut all: Vec<(Row, Project)> = ours.into_iter().chain(others).collect();
            all.sort_by_key(|(r, _)| r.entry);
            section(&mut list, None, all.into_iter().map(plain).collect());
        }
    }
    list
}

/// Shown instead of an empty list.
pub fn empty_message(total: usize, refreshing: bool, query: &str, filters: Filters, current: Option<&Project>) -> String {
    match () {
        _ if total == 0 && refreshing => "正在读取会话…".into(),
        _ if total == 0 => "还没有 Claude / Codex 会话".into(),
        _ if !query.trim().is_empty() => "无匹配".into(),
        _ if filters.archived => "没有已归档的会话".into(),
        _ if filters.stale => "没有 7 天未活动的会话".into(),
        _ => match (effective_scope(filters, current), current) {
            (Scope::Current, Some(c)) => format!("{} 还没有会话，看看「全部项目」", c.name),
            _ => "没有会话".into(),
        },
    }
}

/// The line above the list while picking (the stale filter, or anything picked).
pub fn picks_text(picked: usize, bytes: u64) -> String {
    match picked {
        0 => "⇧ / ⌘ 点击多选".into(),
        n => format!("⇧ / ⌘ 点击多选 · 已选 {n} 个 · 共 {}", size_label(bytes)),
    }
}

/// The confirm bar: 「N 个会话 · X MB[ + 附属 Y MB] 将移到废纸篓（可从废纸篓还原）」.
pub fn confirm_text(sessions: usize, bytes: u64, companion_bytes: u64) -> String {
    let companion = if companion_bytes == 0 { String::new() } else { format!(" + 附属 {}", size_label(companion_bytes)) };
    format!("{sessions} 个会话 · {}{companion} 将移到废纸篓（可从废纸篓还原）", size_label(bytes))
}

/// The confirm bar while the companion data is still being sized off the UI thread:
/// 「N 个会话 · X MB + 附属 计算中… 将移到废纸篓（可从废纸篓还原）」 (the bar already works: the Trash collects the
/// companions itself).
pub fn confirm_text_sizing(sessions: usize, bytes: u64) -> String {
    format!("{sessions} 个会话 · {} + 附属 计算中… 将移到废纸篓（可从废纸篓还原）", size_label(bytes))
}

/// The rename field's starting text: the rename, else the title it has without one (`auto_title`: the agent's
/// own title or the one derived from its prompts).
pub fn rename_initial(saved: Option<&str>, auto_title: &str) -> String {
    saved.unwrap_or(auto_title).to_string()
}

/// What to store after the field committed `committed` (trimmed; "" = back to `auto_title`). None: nothing
/// changed (⏎ on the untouched automatic title must not freeze it as a rename).
pub fn rename_value(committed: &str, saved: Option<&str>, auto_title: &str) -> Option<String> {
    let value = committed.trim();
    match saved {
        Some(s) if s == value => None,
        None if value.is_empty() || value == auto_title => None,
        _ => Some(value.to_string()),
    }
}

/// A key press in the palette; None = text (or nothing) for the query box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    /// ↩ / ⌘↩ (⌘⇧↩ arrives as the `ResumeBelow` action: ⌘⇧↩ is bound to Zoom Pane).
    Enter(Location),
    /// ⌘R.
    Rename,
    /// ⌘⇧C.
    CopyId,
    /// ⌘E.
    Archive,
    /// ⌘⌫.
    Trash,
    /// ⌘⇧K: the cleanup wizard.
    Cleanup,
    Escape,
    Backspace,
}

pub fn command(key: &str, m: Modifiers) -> Option<Key> {
    let plain = !m.control && !m.alt && !m.platform && !m.shift;
    let cmd_only = m.platform && !m.control && !m.alt;
    match key {
        "up" if plain => Some(Key::Up),
        "down" if plain => Some(Key::Down),
        "p" if m.control && !m.alt && !m.platform => Some(Key::Up),
        "n" if m.control && !m.alt && !m.platform => Some(Key::Down),
        "enter" if !m.control && !m.alt => Some(Key::Enter(Location::from_enter(m.platform, m.shift))),
        "r" if cmd_only && !m.shift => Some(Key::Rename),
        "c" if cmd_only && m.shift => Some(Key::CopyId),
        "e" if cmd_only && !m.shift => Some(Key::Archive),
        "k" if cmd_only && m.shift => Some(Key::Cleanup),
        "backspace" if cmd_only && !m.shift => Some(Key::Trash),
        "backspace" if !m.platform && !m.control => Some(Key::Backspace),
        "escape" => Some(Key::Escape),
        _ => None,
    }
}

/// Whether ↩ (`m` its modifiers, `held` a key repeat) moves the confirm bar's sessions to the Trash: the
/// sidebar bar's rule (`sidebar::ended::confirm_key`), so only a deliberate plain ↩ confirms — ⌘↩ / ⇧↩
/// (resume keys a moment ago) never do.
pub fn confirms_trash(m: &Modifiers, held: bool, menu_open: bool) -> bool {
    crate::sidebar::ended::confirm_key("enter", m, held, menu_open) == Some(true)
}

/// What Esc closes: the row menu, else the confirm bar, else the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Esc {
    Menu,
    Confirm,
    Palette,
}

pub fn escape(menu_open: bool, confirm_open: bool) -> Esc {
    match (menu_open, confirm_open) {
        (true, _) => Esc::Menu,
        (false, true) => Esc::Confirm,
        (false, false) => Esc::Palette,
    }
}

/// The row menu (right click).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Resume,
    ResumeRight,
    Rename,
    CopyId,
    CopyDir,
    Reveal,
    Archive,
    Trash,
}

/// Items, labels and whether each is enabled. `trashable`: the menu's Trash targets are not all running.
/// `archived`: the row is archived, so the action is 取消归档. 移到废纸篓… comes last, after a separator, in red.
pub fn menu_items(trashable: bool, archived: bool) -> [(MenuItem, &'static str, bool); 8] {
    [
        (MenuItem::Resume, "恢复", true),
        (MenuItem::ResumeRight, "在右侧恢复", true),
        (MenuItem::Rename, "重命名…", true),
        (MenuItem::CopyId, "复制会话 ID", true),
        (MenuItem::CopyDir, "复制目录路径", true),
        (MenuItem::Reveal, "在访达中显示", true),
        (MenuItem::Archive, if archived { "取消归档" } else { "归档" }, true),
        (MenuItem::Trash, "移到废纸篓…", trashable),
    ]
}

/// What ↩ on a row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resume {
    /// A running session: go to its pane.
    Focus(PaneId),
    /// A running session owned by another terminal: navigate there, never resume it.
    Navigate(RuntimeRef),
    /// Running, but no window has its pane: nothing to resume or go to.
    Elsewhere,
    /// Type the resume command; a new pane starts in `dir`.
    Run { dir: PathBuf, command: String },
    /// The transcript is gone (the list was stale): refresh it.
    Gone,
    /// The session's directory is gone: no pane opens.
    NoDir(PathBuf),
    /// Nothing says where the session ran (a sidebar session without a cwd, not in the history either).
    NoCwd,
}

impl Resume {
    /// The toast of the outcomes that open nothing (spec §8).
    pub fn toast(&self) -> Option<String> {
        match self {
            Resume::Elsewhere => Some("该会话正在运行，但不在 gilvt 的窗口里".into()),
            Resume::Gone => Some("该会话已不存在".into()),
            Resume::NoDir(dir) => Some(format!("会话目录已不存在：{}", dir.display())),
            Resume::NoCwd => Some("不知道该会话的目录，无法恢复".into()),
            Resume::Focus(_) | Resume::Navigate(_) | Resume::Run { .. } => None,
        }
    }
}

/// ↩ on `e`: `live` from its row; `exists` checks the disk (transcript, cwd). `launch` is the agent's
/// `agent.*_launch` setting.
pub fn resume(e: &HistoryEntry, live: Option<&LiveAt>, launch: &str, exists: impl Fn(&Path) -> bool) -> Resume {
    match live {
        Some(LiveAt { pane: Some(p), .. }) => Resume::Focus(*p),
        Some(LiveAt { runtime: Some(runtime), .. }) => Resume::Navigate(runtime.clone()),
        Some(_) => Resume::Elsewhere,
        None if !exists(&e.transcript) => Resume::Gone,
        None if !exists(&e.cwd) => Resume::NoDir(e.cwd.clone()),
        None => Resume::Run {
            dir: e.cwd.clone(),
            command: gilvt_agent::resume_command(launch, e.agent, &e.session_id, &e.cwd),
        },
    }
}

#[cfg(test)]
#[path = "sessions_model_tests.rs"]
mod tests;
