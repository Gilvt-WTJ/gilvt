//! The pure half of the window snapshot: the app's own models (registry sessions, the sidebar model, the
//! pane marks, menu items) turned into the `DebugState` structs. The gpui side (`workspace/debug.rs`)
//! gathers the models and the recorded rects and calls these.

use std::time::Instant;

use gilvt_agent::{AgentKind, GitInfo, Session, SessionKey, SessionSummary, Status, TitleSource};

use super::rects::{Rect4, RectId};
use super::{Button, CloseConfirm, CloseItem, CompareState, Git, GroupButton, Menu, MenuItem, Pending, Row, Section, SessionsRow, Sidebar, Tooltip, TrashConfirm};
use crate::editor::compare::{Compare, CompareRows};
use crate::editor::popup::{MenuKind, OpenMenu};
use crate::pane_tree::PaneId;
use crate::launcher::sessions_model;
use crate::sidebar::model::{self, Grouping, Model, SectionKind};
use crate::workspace::marks::{Edge, EdgeColor, Mark};

/// "claude:<session id>".
pub fn session_id(key: &SessionKey) -> String {
    format!("{}:{}", key.0.name(), key.1)
}

/// The editor pane's bar as `editors[].bar` names it.
pub fn editor_bar(bar: &crate::editor::view::Bar) -> &'static str {
    use crate::editor::view::Bar;
    match bar {
        Bar::None => "none",
        Bar::Close => "close",
        Bar::Modified { .. } => "modified",
        Bar::SaveError { .. } => "save_error",
        Bar::Deleted => "deleted",
        Bar::Confirm(_) => "confirm",
    }
}

/// The notice bar's buttons as drawn (labels from `chrome::bar_buttons`), with pane `pane`'s rects.
pub fn editor_bar_buttons(bar: &crate::editor::view::Bar, pane: PaneId, rect: &dyn Fn(RectId) -> Option<Rect4>) -> Vec<Button> {
    crate::editor::chrome::bar_buttons(bar)
        .into_iter()
        .enumerate()
        .map(|(n, (label, _, _))| Button { label: label.to_string(), rect: rect(RectId::EditorBarButton(pane, n)) })
        .collect()
}

/// The status bar's flash at `now`: the same rule the status bar draws by (an expired one is null).
pub fn editor_status_flash(flash: Option<&(String, Instant)>, now: Instant) -> Option<String> {
    crate::editor::view::live_flash(flash, now).map(str::to_string)
}

/// The status-bar menu as drawn (None while closed) and its kind ("encoding" | "line_ending").
pub fn editor_menu(menu: Option<&OpenMenu>, pane: PaneId, rect: &dyn Fn(RectId) -> Option<Rect4>) -> (Option<Menu>, Option<&'static str>) {
    let Some(m) = menu else { return (None, None) };
    let items = m
        .items
        .iter()
        .enumerate()
        .map(|(n, i)| MenuItem { label: i.label.clone(), checked: i.checked, enabled: i.enabled, rect: rect(RectId::EditorMenuItem(pane, n)) })
        .collect();
    let kind = match m.kind {
        MenuKind::Encoding => "encoding",
        MenuKind::LineEnding => "line_ending",
    };
    (Some(Menu { items }), Some(kind))
}

/// The 「对比」 overlay (None while closed): its display rows (after folding), layout and buttons.
pub fn editor_compare(compare: Option<&Compare>, pane: PaneId, rect: &dyn Fn(RectId) -> Option<Rect4>) -> Option<CompareState> {
    let c = compare?;
    Some(CompareState {
        rows: c.row_count(),
        split: matches!(c.rows, CompareRows::Split(_)),
        disk_missing: c.disk_missing,
        same: !c.disk_missing && c.rows == CompareRows::Same,
        rect: rect(RectId::EditorCompare(pane)),
        buttons: crate::editor::compare::buttons(c.disk_missing)
            .into_iter()
            .enumerate()
            .map(|(n, (label, _, _))| Button { label: label.to_string(), rect: rect(RectId::EditorCompareButton(pane, n)) })
            .collect(),
    })
}

/// The status names of the snapshot (one per `Status` variant).
pub fn status_name(status: &Status) -> &'static str {
    match status {
        Status::Thinking => "thinking",
        Status::Tool { .. } => "running_tool",
        Status::NeedsApproval { .. } => "awaiting_approval",
        Status::Asking { .. } => "awaiting_answer",
        Status::Idle => "idle",
        Status::Error { .. } => "errored",
        Status::Ended => "ended",
    }
}

/// The question / action / tool / error a status carries, first line trimmed as the sidebar shows it.
pub fn status_detail(status: &Status) -> String {
    let text = match status {
        Status::Tool { label } => label,
        Status::NeedsApproval { action } => action,
        Status::Asking { question } => question,
        Status::Error { message } => message,
        Status::Thinking | Status::Idle | Status::Ended => return String::new(),
    };
    text.lines().next().unwrap_or("").trim().to_string()
}

/// A status mark's color name.
pub fn mark_color(mark: Mark) -> &'static str {
    match mark {
        Mark::NeedsYou => "amber",
        Mark::Error => "red",
        Mark::Running => "blue",
        Mark::Done => "green",
    }
}

/// The outline a pane is drawn with: its status ring, else the focus border of a split tab.
pub fn border(edge: Edge) -> Option<&'static str> {
    match (edge.ring, edge.border) {
        (Some((mark, _)), _) | (None, Some(EdgeColor::Status(mark))) => Some(mark_color(mark)),
        (None, Some(EdgeColor::Focus)) => Some("focus"),
        (None, None) => None,
    }
}

/// A terminal's foreground program: `name` its process name (None when the lookup failed), `agent` what
/// the foreground poll's rules make of it.
pub fn foreground(name: Option<&str>, agent: Option<AgentKind>) -> String {
    match (name, agent) {
        (None, _) => "unknown".into(),
        (Some(name), _) if crate::agents::is_shell(name) => "shell".into(),
        (Some(_), Some(kind)) => format!("agent:{}", kind.name()),
        (Some(name), None) => format!("other:{name}"),
    }
}

/// "needs_you", "project:<name>", "status:<group>" (等你处理 …) or "ended".
pub fn section_name(section: &model::Section) -> String {
    match section.kind {
        SectionKind::NeedsYou => "needs_you".into(),
        SectionKind::Pending => "pending".into(),
        SectionKind::Ended => "ended".into(),
        SectionKind::Terminals => "terminals".into(),
        SectionKind::Group if section.id.starts_with("status:") => format!("status:{}", section.title),
        SectionKind::Group => section.id.clone(),
    }
}

/// The pending resumes as listed.
pub fn pending(list: &[crate::agents::PendingResume]) -> Vec<Pending> {
    list.iter()
        .map(|p| Pending {
            session: session_id(&p.key),
            pane: p.pane,
            cwd: p.cwd.as_ref().map(|c| c.display().to_string()),
            name: p.name.clone(),
            last_status: p.last_status.clone(),
        })
        .collect()
}

/// A row's git facts: `line` as drawn, the rest as `g` has it.
pub fn git_state(line: &str, g: &GitInfo) -> Git {
    Git {
        line: line.to_string(),
        branch: g.branch.clone(),
        detached: g.detached_short.clone(),
        dirty: g.dirty_count,
        ahead: g.ahead,
        behind: g.behind,
        linked_worktree: g.is_linked_worktree,
        repo_root: g.repo_root.display().to_string(),
        repo: g.main_repo_name(),
    }
}

/// The close confirm bar: `action` is "pane" | "tab" | "window" | "quit"; `dirty` the unsaved editor panes it lists
/// as (pane, file name).
pub fn close_confirm(action: &'static str, items: &[SessionSummary], dirty: &[(crate::pane_tree::PaneId, String)]) -> CloseConfirm {
    CloseConfirm {
        action,
        items: items
            .iter()
            .map(|s| CloseItem { session: session_id(&s.key), pane: s.pane, name: s.name.clone(), status: s.status.clone() })
            .collect(),
        dirty: dirty.iter().map(|(_, name)| name.clone()).collect(),
    }
}

/// The sidebar part of a window: `model` is None while the sidebar is hidden; `renaming` the session whose
/// row shows the rename field; `session` looks up a row's registry session; `rect` the rects recorded in
/// the latest frame; `trash` the sessions of the confirm bar, when it shows; `git` a session's git facts.
pub fn sidebar<'a>(
    model: Option<&Model>,
    grouping: Grouping,
    renaming: Option<&SessionKey>,
    session: &dyn Fn(&SessionKey) -> Option<&'a Session>,
    rect: &dyn Fn(RectId) -> Option<Rect4>,
    trash: Option<&[SessionKey]>,
    git: &dyn Fn(&SessionKey) -> Option<GitInfo>,
) -> Sidebar {
    let group_by = match grouping {
        Grouping::Project => "project",
        Grouping::Status => "status",
    };
    let trash_confirm = trash.map(|keys| TrashConfirm {
        sessions: keys.iter().map(session_id).collect(),
        buttons: buttons(&confirm_buttons(), RectId::SidebarTrashButton, rect),
    });
    let Some(m) = model else {
        return Sidebar {
            visible: false,
            group_by,
            sections: Vec::new(),
            rows: Vec::new(),
            trash_confirm,
            terminals: 0,
            header: String::new(),
            tooltip: None,
            group_buttons: Vec::new(),
            review_entry: None,
        };
    };
    let sections = m
        .sections
        .iter()
        .enumerate()
        .map(|(si, s)| Section {
            name: section_name(s),
            title: s.title.clone(),
            collapsed: s.collapsed,
            count: s.rows.len(),
            rect: rect(RectId::SidebarSection(si)),
        })
        .collect::<Vec<_>>();
    // Rows of collapsed sections are listed too (not drawn, no rect); SidebarRow(n) counts drawn rows.
    let mut drawn = 0;
    let rows = model::all_rows(m, renaming)
        .into_iter()
        .map(|(si, r, visible)| {
            let rect = visible.then(|| {
                drawn += 1;
                rect(RectId::SidebarRow(drawn - 1))
            });
            let key = r.key.as_ref();
            let status = key.and_then(|k| session(k)).map(|s| &s.status);
            let terminal = r.kind == model::RowKind::Terminal;
            Row {
                kind: if terminal { "terminal" } else { "agent" },
                cwd: r.cwd.clone(),
                session: key.map(session_id).unwrap_or_default(),
                agent: key.map_or("", |k| k.0.name()),
                name: r.name.clone(),
                status: status.map_or(if terminal { "terminal" } else if r.ended { "ended" } else { "idle" }, status_name),
                detail: status.map(status_detail).unwrap_or_default(),
                line: r.status.clone(),
                muted: r.muted,
                lite: r.lite,
                pane: r.pane,
                focused: r.current,
                section: sections[si].name.clone(),
                visible,
                rect: rect.flatten(),
                git: if r.git.is_empty() { None } else { key.and_then(|k| git(k)).map(|g| git_state(&r.git, &g)) },
                summary: r.summary.clone(),
            }
        })
        .collect();
    // The review entry's count and rect belong to the sidebar view; `workspace::debug` fills them in.
    Sidebar {
        visible: true,
        group_by,
        sections,
        rows,
        trash_confirm,
        terminals: m.terminals,
        header: model::header_text(m),
        tooltip: crate::sidebar::tooltip::shown().map(|text| Tooltip { text }),
        group_buttons: [("project", Grouping::Project), ("status", Grouping::Status)]
            .into_iter()
            .enumerate()
            .map(|(n, (name, g))| GroupButton { name, active: grouping == g, rect: rect(RectId::SidebarGrouping(n)) })
            .collect(),
        review_entry: None,
    }
}

/// "claude:<prompt uuid>", "codex:<turn id>" or "fallback:<end offset>": a review cursor as a debug id.
pub fn cursor_id(cursor: &gilvt_agent::TurnCursor) -> String {
    match cursor {
        gilvt_agent::TurnCursor::Claude { prompt_uuid } => format!("claude:{prompt_uuid}"),
        gilvt_agent::TurnCursor::Codex { turn_id } => format!("codex:{turn_id}"),
        gilvt_agent::TurnCursor::Fallback { end_offset, .. } => format!("fallback:{end_offset}"),
    }
}
/// A menu from its items (label, checked, enabled) in drawing order; `id` names item n's rect.
pub fn menu<'a>(
    items: impl IntoIterator<Item = (&'a str, bool, bool)>,
    id: fn(usize) -> RectId,
    rect: &dyn Fn(RectId) -> Option<Rect4>,
) -> Menu {
    let items = items
        .into_iter()
        .enumerate()
        .map(|(n, (label, checked, enabled))| MenuItem { label: label.to_string(), checked, enabled, rect: rect(id(n)) })
        .collect();
    Menu { items }
}

/// The buttons of both trash confirm bars (the sidebar's and the 会话 palette's), in drawing order.
pub const CONFIRM_BUTTONS: [&str; 2] = ["取消", "移到废纸篓"];

/// [`CONFIRM_BUTTONS`] in the interface language, as the bars draw them.
pub fn confirm_buttons() -> [&'static str; 2] {
    [crate::i18n::text(CONFIRM_BUTTONS[0], "Cancel"), crate::i18n::text(CONFIRM_BUTTONS[1], "Move to Trash")]
}

/// Buttons from their labels in drawing order; `id` names button n's rect.
pub fn buttons(labels: &[&str], id: fn(usize) -> RectId, rect: &dyn Fn(RectId) -> Option<Rect4>) -> Vec<Button> {
    labels.iter().enumerate().map(|(n, label)| Button { label: label.to_string(), rect: rect(id(n)) }).collect()
}

/// The 会话 palette's rows: `cursor` the cursor row, `picked` whether a session is picked for the Trash.
pub fn palette_rows(
    rows: &[sessions_model::Row],
    cursor: usize,
    picked: &dyn Fn(&SessionKey) -> bool,
    rect: &dyn Fn(RectId) -> Option<Rect4>,
) -> Vec<SessionsRow> {
    rows.iter()
        .enumerate()
        .map(|(i, r)| SessionsRow {
            session: session_id(&r.key),
            title: r.title.clone(),
            title_source: match r.title_source {
                TitleSource::Saved => "saved",
                TitleSource::Custom => "custom",
                TitleSource::Ai => "ai",
                TitleSource::Prompt => "prompt",
            },
            agent: r.key.0.name(),
            meta: r.subtitle(),
            dir: r.dir.clone(),
            dir_missing: r.dir_missing,
            right: r.right(),
            live: r.live.is_some(),
            binding: match r.live.as_ref().and_then(|live| live.runtime.as_ref()).map(|runtime| runtime.confidence) {
                Some(gilvt_agent::BindingConfidence::Exact) => "exact",
                Some(gilvt_agent::BindingConfidence::Inferred) => "inferred",
                Some(gilvt_agent::BindingConfidence::Unresolved) => "unresolved",
                None => "none",
            },
            pid: r.live.as_ref().and_then(|live| live.runtime.as_ref()).map(|runtime| runtime.pid),
            tty: r.live.as_ref().and_then(|live| live.runtime.as_ref()).and_then(|runtime| runtime.tty.as_ref()).map(|tty| tty.display().to_string()),
            selected: i == cursor,
            marked: picked(&r.key),
            rect: rect(RectId::PaletteRow(i)),
        })
        .collect()
}

/// The inspector banner's title without its leading ⏳.
pub fn banner_text(title: &str) -> String {
    title.trim_start_matches('⏳').trim_start().to_string()
}

#[cfg(test)]
#[path = "map_tests.rs"]
mod tests;
