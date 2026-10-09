//! Close confirmation (P0 spec §5): ⌘W / ⌘⇧W / the red button / ⌘Q ask first when an agent in scope is working or
//! waiting for the user; Idle / Ended / Error agents and plain shells close without a prompt.
//! E2a §6: closing a tab or a window, or quitting, also asks when an editor pane in scope has unsaved changes
//! (closing a single editor pane asks in the pane itself, see `workspace/editor.rs`).

use gilvt_agent::{needs_confirm, AgentKind, SessionKey, SessionSummary};
use gpui::{App, Context, Modifiers, Window};

use super::{PaneView, Workspace};
use crate::agents::Agents;
use crate::editor::view::{EditorView, SaveOutcome};
use crate::pane_tree::{PaneId, PaneTree};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseAction {
    Pane(PaneId),
    Tab(usize),
    Window,
    /// ⌘Q: every window.
    Quit,
}

impl CloseAction {
    /// The name `gilvt debug state` reports.
    pub fn name(&self) -> &'static str {
        match self {
            CloseAction::Pane(_) => "pane",
            CloseAction::Tab(_) => "tab",
            CloseAction::Window => "window",
            CloseAction::Quit => "quit",
        }
    }
}

/// The panes a close would end: the pane, the tab's panes, or every pane of the window's tabs.
pub fn panes_in_scope(tabs: &[&PaneTree], action: &CloseAction) -> Vec<PaneId> {
    match action {
        CloseAction::Pane(id) => vec![*id],
        CloseAction::Tab(i) => tabs.get(*i).map(|t| t.panes()).unwrap_or_default(),
        CloseAction::Window | CloseAction::Quit => tabs.iter().flat_map(|t| t.panes()).collect(),
    }
}

/// One list from several windows' lists, in order, each session once (by key).
pub fn merge_at_stake(per_window: Vec<Vec<SessionSummary>>) -> Vec<SessionSummary> {
    let mut seen: Vec<SessionKey> = Vec::new();
    let mut out = Vec::new();
    for item in per_window.into_iter().flatten() {
        if !seen.contains(&item.key) {
            seen.push(item.key.clone());
            out.push(item);
        }
    }
    out
}

/// `「C · 修复登录 · 思考中」`: the agent's letter, its name, its status.
pub fn item_line(s: &SessionSummary) -> String {
    let letter = match s.key.0 {
        AgentKind::Claude => 'C',
        AgentKind::Codex => 'X',
    };
    if crate::i18n::english() {
        format!("\"{letter} · {} · {}\"", s.name, s.status)
    } else {
        format!("「{letter} · {} · {}」", s.name, s.status)
    }
}

/// The unsaved editor panes (pane, file name) that are in `scope`, in `dirty`'s order.
pub fn dirty_in_scope(dirty: &[(PaneId, String)], scope: &[PaneId]) -> Vec<(PaneId, String)> {
    dirty.iter().filter(|(id, _)| scope.contains(id)).cloned().collect()
}

/// The bar's file list: `● SKILL.md`, one line per unsaved file.
pub fn dirty_lines(dirty: &[(PaneId, String)]) -> Vec<String> {
    dirty.iter().map(|(_, name)| format!("● {name}")).collect()
}

/// What a close would lose: agents that are working or waiting, and editor files with unsaved changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AtStake {
    pub items: Vec<SessionSummary>,
    /// (pane, file name); names only, no directories.
    pub dirty: Vec<(PaneId, String)>,
}

impl AtStake {
    /// The confirm bar shows only when something would be lost; otherwise the close runs as it always did.
    pub fn needs_confirm(&self) -> bool {
        !self.items.is_empty() || !self.dirty.is_empty()
    }
}

/// ⌘Q: several windows' [`AtStake`] as one, in order: each session once (by key), each editor pane once.
pub fn merge_all(per_window: Vec<AtStake>) -> AtStake {
    let mut dirty: Vec<(PaneId, String)> = Vec::new();
    let mut items = Vec::new();
    for w in per_window {
        items.push(w.items);
        for d in w.dirty {
            if !dirty.iter().any(|(id, _)| *id == d.0) {
                dirty.push(d);
            }
        }
    }
    AtStake { items: merge_at_stake(items), dirty }
}

/// A key the confirm bar answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseKey {
    /// 取消.
    Cancel,
    /// ⌘↩: 仍然关闭 / 不保存并关闭.
    Close,
    /// 全部保存并关闭 (bar listing unsaved files only).
    SaveAll,
}

/// A key on the window root while the confirm bar shows; None = not the bar's. `files`: the bar lists unsaved
/// files. Agents-only bar: Esc / ↩ = 取消 (the default), ⌘↩ = 仍然关闭. Bar with files (E2a §6, default 保存):
/// ↩ / ⌘S = 全部保存并关闭, Esc = 取消, ⌘↩ = 不保存并关闭.
pub fn close_key(key: &str, mods: &Modifiers, files: bool) -> Option<CloseKey> {
    let only_cmd = mods.platform && !mods.shift && !mods.alt && !mods.control;
    match key {
        "escape" if !mods.modified() => Some(CloseKey::Cancel),
        "enter" if !mods.modified() => Some(if files { CloseKey::SaveAll } else { CloseKey::Cancel }),
        "enter" if only_cmd => Some(CloseKey::Close),
        "s" if only_cmd && files => Some(CloseKey::SaveAll),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseConfirm {
    pub action: CloseAction,
    pub items: Vec<SessionSummary>,
    /// Unsaved editor panes in scope (pane, file name); for ⌘Q every window's.
    pub dirty: Vec<(PaneId, String)>,
    /// The panes in scope when the bar opened. A tab is found again by these, not by its position (tabs shift
    /// while the bar is open).
    pub scope_panes: Vec<PaneId>,
}

/// What 「仍然关闭」 / 「不保存并关闭」 / 「全部保存并关闭」 does after re-checking what is at stake now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmDecision {
    Proceed,
    /// Something not listed became at stake meanwhile: show the fresh list and ask again.
    Reprompt(AtStake),
}

/// Proceeds unless `fresh` holds a session or an unsaved editor pane the user was not shown in `listed`.
pub fn confirm_decision(listed: &AtStake, fresh: AtStake) -> ConfirmDecision {
    let agents_seen = fresh.items.iter().all(|f| listed.items.iter().any(|l| l.key == f.key));
    let files_seen = fresh.dirty.iter().all(|(f, _)| listed.dirty.iter().any(|(l, _)| l == f));
    if agents_seen && files_seen {
        ConfirmDecision::Proceed
    } else {
        ConfirmDecision::Reprompt(fresh)
    }
}

/// After 「全部保存并关闭」: saving files never ends agents by itself, so agents still at stake are asked about again
/// (on the agents-only bar); otherwise as [`confirm_decision`].
pub fn after_save_decision(listed: &AtStake, fresh: AtStake) -> ConfirmDecision {
    if fresh.items.is_empty() {
        confirm_decision(listed, fresh)
    } else {
        ConfirmDecision::Reprompt(fresh)
    }
}

/// ⌘Q re-check: every window's [`AtStake`] merged, or None (do not quit) when a window could not be read, as
/// [`quit_requested`] does.
pub fn quit_fresh(per_window: Vec<AtStake>, unreadable: usize) -> Option<AtStake> {
    (unreadable == 0).then(|| merge_all(per_window))
}

/// Looking a listed pane up for 「全部保存并关闭」.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup<T> {
    Found(T),
    /// No window has the pane any more: nothing to save.
    Gone,
    /// A window that might hold it could not be read: its file's state is unknown.
    Unreadable,
}

/// Why 「全部保存并关闭」 stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveStop {
    /// This pane did not save (changed on disk, write failed): bring it forward.
    NotSaved(PaneId),
    /// This pane's window could not be read: abort, show nothing.
    Unreadable(PaneId),
}

/// 「全部保存并关闭」: saves `panes` in order through `save` (a gone pane is skipped) and stops at the first one that
/// did not save or could not be reached.
pub fn save_all(panes: &[PaneId], mut save: impl FnMut(PaneId) -> Lookup<SaveOutcome>) -> Result<(), SaveStop> {
    for &id in panes {
        match save(id) {
            Lookup::Gone | Lookup::Found(SaveOutcome::Saved) => {}
            Lookup::Found(SaveOutcome::Modified | SaveOutcome::Failed) => return Err(SaveStop::NotSaved(id)),
            Lookup::Unreadable => return Err(SaveStop::Unreadable(id)),
        }
    }
    Ok(())
}

/// The tab whose panes include all of `scope` (tabs given as their pane sets); None when it is gone.
pub fn tab_index_for(tabs: &[Vec<PaneId>], scope: &[PaneId]) -> Option<usize> {
    if scope.is_empty() {
        return None;
    }
    tabs.iter().position(|t| scope.iter().all(|p| t.contains(p)))
}

impl Workspace {
    fn blocking_in(&self, action: &CloseAction, cx: &App) -> AtStake {
        let trees: Vec<&PaneTree> = self.tabs.iter().map(|t| &t.tree).collect();
        let panes = panes_in_scope(&trees, action);
        let items = match cx.try_global::<Agents>() {
            Some(a) => needs_confirm(&panes, a.registry().sessions()),
            None => Vec::new(),
        };
        let dirty: Vec<(PaneId, String)> = self
            .dirty_editors(cx)
            .into_iter()
            .map(|(id, path)| {
                (
                    id,
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| crate::i18n::text("未命名", "Untitled").into()),
                )
            })
            .collect();
        AtStake { items, dirty: dirty_in_scope(&dirty, &panes) }
    }

    fn confirm_for(&self, action: CloseAction, at: AtStake) -> CloseConfirm {
        let scope_panes = self.scope_of(&action);
        CloseConfirm { action, items: at.items, dirty: at.dirty, scope_panes }
    }

    /// Shows the confirm bar; the root takes the keyboard for ⌘↩ / ↩ / Esc.
    fn open_close_confirm(&mut self, confirm: CloseConfirm, window: &mut Window, cx: &mut Context<Self>) {
        self.close_confirm = Some(confirm);
        self.close_confirm_focus = crate::sidebar::ended::ConfirmFocus::Requested;
        self.ended_confirm = None;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// Closes now when nothing would be lost; otherwise opens the confirm bar listing what would be.
    pub fn request_close(&mut self, action: CloseAction, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.blocking_in(&action, cx);
        if !at.needs_confirm() {
            self.run_close(&action, window, cx);
            return;
        }
        let confirm = self.confirm_for(action, at);
        self.open_close_confirm(confirm, window, cx);
    }

    fn scope_of(&self, action: &CloseAction) -> Vec<PaneId> {
        let trees: Vec<&PaneTree> = self.tabs.iter().map(|t| &t.tree).collect();
        panes_in_scope(&trees, action)
    }

    fn run_close(&mut self, action: &CloseAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            CloseAction::Pane(id) => self.close_pane(*id, window, cx),
            CloseAction::Tab(i) => self.close_tab_at(*i, window, cx),
            // No snapshot here: this runs inside the window's own update, so reading it would fail. The
            // periodic save and `on_window_closed` take care of the layout.
            CloseAction::Window => window.remove_window(),
            CloseAction::Quit => crate::persist::save_then(cx, |cx| cx.quit()),
        }
    }

    /// 「仍然关闭」 / 「不保存并关闭」 (⌘↩).
    pub fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_close(false, window, cx);
    }

    /// 「全部保存并关闭」 (⌘S on the bar).
    pub fn save_all_and_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_close(true, window, cx);
    }

    /// Re-checks first. The target is found again by its panes (a tab may have moved or gone), and an agent or an
    /// unsaved file that became at stake since the bar opened is shown (the bar stays) instead of closed unasked.
    /// With `save` the listed files are saved first; one that does not save (changed on disk, write failed) stops
    /// the close: the bar goes and that pane comes to the front, where its own bar says why (E2a §6).
    fn finish_close(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.close_confirm.take() else { return };
        let action = match &c.action {
            CloseAction::Tab(_) => {
                let sets: Vec<Vec<PaneId>> = self.tabs.iter().map(|t| t.tree.panes()).collect();
                let Some(i) = tab_index_for(&sets, &c.scope_panes) else {
                    self.focus_active(window, cx);
                    cx.notify();
                    return;
                };
                CloseAction::Tab(i)
            }
            CloseAction::Pane(id) => {
                if !self.has_pane(*id) {
                    self.focus_active(window, cx);
                    cx.notify();
                    return;
                }
                c.action.clone()
            }
            CloseAction::Window | CloseAction::Quit => c.action.clone(),
        };
        if save && !c.dirty.is_empty() {
            let panes: Vec<PaneId> = c.dirty.iter().map(|(id, _)| *id).collect();
            let saved = save_all(&panes, |id| match self.editor_anywhere(id, window, cx) {
                Lookup::Found(v) => Lookup::Found(v.update(cx, |v, cx| v.save(cx))),
                Lookup::Gone => Lookup::Gone,
                Lookup::Unreadable => Lookup::Unreadable,
            });
            match saved {
                Ok(()) => {}
                Err(SaveStop::NotSaved(id)) => {
                    if self.has_pane(id) {
                        self.focus_pane(id, window, cx);
                    } else {
                        // Another window's (⌘Q): bring that window and pane forward once this update is over.
                        self.focus_active(window, cx);
                        cx.defer(move |cx| super::focus_pane_anywhere(id, cx));
                    }
                    cx.notify();
                    return;
                }
                Err(SaveStop::Unreadable(id)) => {
                    eprintln!("gilvt: could not reach the window of pane {id} to save it; not closing (try again)");
                    self.focus_active(window, cx);
                    cx.notify();
                    return;
                }
            }
        }
        let fresh = match &action {
            CloseAction::Quit => {
                let own = self.blocking_in(&CloseAction::Window, cx);
                let (others, unreadable) = collect_at_stake(cx, Some(window.window_handle().window_id()));
                let mut all = vec![own];
                all.extend(others.into_iter().map(|(_, at)| at));
                let Some(fresh) = quit_fresh(all, unreadable) else {
                    eprintln!("gilvt: {unreadable} window(s) could not be checked; not quitting (try again)");
                    self.focus_active(window, cx);
                    cx.notify();
                    return;
                };
                fresh
            }
            _ => self.blocking_in(&action, cx),
        };
        let listed = AtStake { items: c.items, dirty: c.dirty };
        let decision = if save { after_save_decision(&listed, fresh) } else { confirm_decision(&listed, fresh) };
        match decision {
            ConfirmDecision::Proceed => self.run_close(&action, window, cx),
            ConfirmDecision::Reprompt(at) => {
                let confirm = self.confirm_for(action, at);
                self.open_close_confirm(confirm, window, cx);
            }
        }
    }

    /// The editor view of `pane`, in this window or (⌘Q) another one. Unreadable when it is in no readable window
    /// and some other window could not be read (it may be there).
    fn editor_anywhere(&self, pane: PaneId, window: &Window, cx: &mut App) -> Lookup<gpui::Entity<EditorView>> {
        let editor = |ws: &Workspace| match ws.panes.get(&pane) {
            Some(PaneView::Editor(v)) => Some(v.clone()),
            _ => None,
        };
        if self.has_pane(pane) {
            return editor(self).map_or(Lookup::Gone, Lookup::Found);
        }
        let own = window.window_handle().window_id();
        let mut unreadable = false;
        for w in super::workspaces(cx).into_iter().filter(|w| w.window_id() != own) {
            match w.update(cx, |ws, _, _| editor(ws)) {
                Ok(Some(v)) => return Lookup::Found(v),
                Ok(None) => {}
                Err(_) => unreadable = true,
            }
        }
        if unreadable {
            Lookup::Unreadable
        } else {
            Lookup::Gone
        }
    }

    /// 「取消」: Esc, and ↩ on an agents-only bar (on a bar listing unsaved files ↩ is 全部保存并关闭).
    pub fn cancel_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.close_confirm.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// The red window button: closes when nothing is at stake, else asks (and keeps the window).
    pub(super) fn should_close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.close_confirm.is_some() {
            return false;
        }
        let at = self.blocking_in(&CloseAction::Window, cx);
        if !at.needs_confirm() {
            return true;
        }
        let confirm = self.confirm_for(CloseAction::Window, at);
        self.open_close_confirm(confirm, window, cx);
        false
    }
}

/// ⌘Q: one confirm bar, in the first window that has something at stake, listing every window's agents and unsaved
/// files (「全部保存并关闭」 saves them all, whichever window holds them); saves the layout and quits when nothing is
/// at stake. Must run outside action dispatch (the active window is leased then): the Quit
/// handler defers into it.
/// Never quits when a window could not be checked or the bar could not be shown.
pub fn quit_requested(cx: &mut App) {
    let (found, unreadable) = collect_at_stake(cx, None);
    let first = found.iter().find(|(_, at)| at.needs_confirm()).map(|(w, _)| *w);
    let at = merge_all(found.into_iter().map(|(_, at)| at).collect());
    if let (Some(w), true) = (first, at.needs_confirm()) {
        let shown = w.update(cx, |ws, window, cx| {
            window.activate_window();
            let confirm = ws.confirm_for(CloseAction::Quit, at);
            ws.open_close_confirm(confirm, window, cx);
        });
        if shown.is_err() {
            eprintln!("gilvt: could not show the quit confirmation; not quitting");
        }
        return;
    }
    if unreadable > 0 {
        eprintln!("gilvt: {unreadable} window(s) could not be checked; not quitting (try again)");
        return;
    }
    crate::persist::save_then(cx, |cx| cx.quit());
}

type WindowItems = (gpui::WindowHandle<Workspace>, AtStake);

/// Every window's at-stake sessions and unsaved files (except window `skip`), and how many windows could not be read.
fn collect_at_stake(cx: &mut App, skip: Option<gpui::WindowId>) -> (Vec<WindowItems>, usize) {
    let mut found = Vec::new();
    let mut unreadable = 0;
    for w in super::workspaces(cx) {
        if Some(w.window_id()) == skip {
            continue;
        }
        match w.update(cx, |ws, _, cx| ws.blocking_in(&CloseAction::Window, cx)) {
            Ok(items) => found.push((w, items)),
            Err(e) => {
                unreadable += 1;
                eprintln!("gilvt: could not check window {:?} before quitting: {e}", w.window_id());
            }
        }
    }
    (found, unreadable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane_tree::Axis;

    #[test]
    fn scopes() {
        let mut a = PaneTree::new(1);
        a.split(1, 2, Axis::Row);
        let b = PaneTree::new(3);
        let tabs = [&a, &b];
        assert_eq!(panes_in_scope(&tabs, &CloseAction::Pane(2)), vec![2]);
        assert_eq!(panes_in_scope(&tabs, &CloseAction::Tab(0)), vec![1, 2]);
        assert_eq!(panes_in_scope(&tabs, &CloseAction::Tab(9)), Vec::<u64>::new());
        assert_eq!(panes_in_scope(&tabs, &CloseAction::Window), vec![1, 2, 3]);
        assert_eq!(panes_in_scope(&tabs, &CloseAction::Quit), vec![1, 2, 3]);
    }

    fn summary(agent: AgentKind, id: &str, name: &str, pane: PaneId) -> SessionSummary {
        SessionSummary { key: (agent, id.into()), pane: Some(pane), name: name.into(), status: "思考中".into() }
    }

    #[test]
    fn windows_merge_in_order_without_duplicates() {
        let a = summary(AgentKind::Claude, "a", "A", 1);
        let b = summary(AgentKind::Codex, "b", "B", 2);
        let c = summary(AgentKind::Claude, "c", "C", 3);
        let merged = merge_at_stake(vec![vec![a.clone(), b.clone()], vec![], vec![b.clone(), c.clone()]]);
        assert_eq!(merged, vec![a, b, c]);
        assert!(merge_at_stake(vec![vec![], vec![]]).is_empty());
    }

    #[test]
    fn confirm_re_checks_what_is_at_stake() {
        let a = summary(AgentKind::Claude, "a", "A", 1);
        let b = summary(AgentKind::Codex, "b", "B", 2);
        let c = summary(AgentKind::Claude, "c", "C", 3);
        let agents = |items: Vec<SessionSummary>| AtStake { items, dirty: Vec::new() };
        let listed = agents(vec![a.clone(), b.clone()]);
        assert_eq!(confirm_decision(&listed, listed.clone()), ConfirmDecision::Proceed);
        assert_eq!(confirm_decision(&listed, AtStake::default()), ConfirmDecision::Proceed);
        assert_eq!(confirm_decision(&listed, agents(vec![b.clone()])), ConfirmDecision::Proceed);
        let fresh = agents(vec![a, c]);
        assert_eq!(confirm_decision(&listed, fresh.clone()), ConfirmDecision::Reprompt(fresh));
    }

    #[test]
    fn confirm_re_checks_unsaved_files_too() {
        let listed = AtStake { items: Vec::new(), dirty: vec![(7, "SKILL.md".into())] };
        // Still dirty after 不保存, or saved (gone from the fresh list): proceed.
        assert_eq!(confirm_decision(&listed, listed.clone()), ConfirmDecision::Proceed);
        assert_eq!(confirm_decision(&listed, AtStake::default()), ConfirmDecision::Proceed);
        // A file that became dirty while the bar was up is shown, not dropped unasked.
        let fresh = AtStake { items: Vec::new(), dirty: vec![(7, "SKILL.md".into()), (8, "a.md".into())] };
        assert_eq!(confirm_decision(&listed, fresh.clone()), ConfirmDecision::Reprompt(fresh));
        // An agent that became at stake on a files-only bar is shown too.
        let a = summary(AgentKind::Claude, "a", "A", 1);
        let fresh = AtStake { items: vec![a], dirty: vec![(7, "SKILL.md".into())] };
        assert_eq!(confirm_decision(&listed, fresh.clone()), ConfirmDecision::Reprompt(fresh));
    }

    #[test]
    fn dirty_editors_in_scope_block_closing() {
        // scope = panes of tab 0; pane 7 is a dirty editor in it, pane 9 a dirty editor elsewhere
        let dirty = vec![(7, "SKILL.md".to_string()), (9, "other.md".to_string())];
        assert_eq!(dirty_in_scope(&dirty, &[3, 7]), vec![(7, "SKILL.md".to_string())]);
        assert!(dirty_in_scope(&dirty, &[3]).is_empty());
    }

    #[test]
    fn dirty_lines_list_every_file() {
        let lines = dirty_lines(&[(7, "SKILL.md".into()), (9, "a.md".into())]);
        assert_eq!(lines, ["● SKILL.md", "● a.md"]);
        assert!(dirty_lines(&[]).is_empty());
    }

    #[test]
    fn an_editor_in_a_tab_is_in_that_tab_and_the_window_scope() {
        // Tab 0: terminal 1 | editor 7; tab 1: editor 9.
        let mut a = PaneTree::new(1);
        a.split(1, 7, Axis::Row);
        let b = PaneTree::new(9);
        let tabs = [&a, &b];
        let dirty = vec![(7, "SKILL.md".to_string()), (9, "b.md".to_string())];
        assert_eq!(dirty_in_scope(&dirty, &panes_in_scope(&tabs, &CloseAction::Tab(0))), vec![(7, "SKILL.md".to_string())]);
        assert_eq!(dirty_in_scope(&dirty, &panes_in_scope(&tabs, &CloseAction::Tab(1))), vec![(9, "b.md".to_string())]);
        assert_eq!(dirty_in_scope(&dirty, &panes_in_scope(&tabs, &CloseAction::Window)), dirty);
        assert_eq!(dirty_in_scope(&dirty, &panes_in_scope(&tabs, &CloseAction::Quit)), dirty);
        assert!(dirty_in_scope(&dirty, &panes_in_scope(&tabs, &CloseAction::Pane(1))).is_empty());
    }

    #[test]
    fn the_bar_shows_only_when_agents_or_files_are_at_stake() {
        assert!(!AtStake::default().needs_confirm(), "nothing at stake: close as before, no bar");
        let a = summary(AgentKind::Claude, "a", "A", 1);
        assert!(AtStake { items: vec![a.clone()], dirty: Vec::new() }.needs_confirm());
        assert!(AtStake { items: Vec::new(), dirty: vec![(7, "x.md".into())] }.needs_confirm());
        assert!(AtStake { items: vec![a], dirty: vec![(7, "x.md".into())] }.needs_confirm());
    }

    #[test]
    fn quit_merges_every_windows_agents_and_files() {
        let a = summary(AgentKind::Claude, "a", "A", 1);
        let b = summary(AgentKind::Codex, "b", "B", 2);
        let w1 = AtStake { items: vec![a.clone()], dirty: vec![(7, "SKILL.md".into())] };
        let w2 = AtStake { items: vec![a.clone(), b.clone()], dirty: vec![(9, "a.md".into()), (7, "SKILL.md".into())] };
        let merged = merge_all(vec![w1, AtStake::default(), w2]);
        assert_eq!(merged.items, vec![a, b]);
        assert_eq!(merged.dirty, vec![(7, "SKILL.md".to_string()), (9, "a.md".to_string())]);
        assert!(!merge_all(vec![AtStake::default(), AtStake::default()]).needs_confirm());
    }

    #[test]
    fn save_all_stops_at_the_first_file_that_did_not_save() {
        let mut tried = Vec::new();
        let r = save_all(&[1, 2, 3], |id| {
            tried.push(id);
            Lookup::Found(if id == 2 { SaveOutcome::Modified } else { SaveOutcome::Saved })
        });
        assert_eq!((r, tried), (Err(SaveStop::NotSaved(2)), vec![1, 2]));
        assert_eq!(save_all(&[4], |_| Lookup::Found(SaveOutcome::Failed)), Err(SaveStop::NotSaved(4)));
        // A pane that is gone is skipped; all saved = Ok.
        assert_eq!(save_all(&[1, 2], |id| if id == 2 { Lookup::Found(SaveOutcome::Saved) } else { Lookup::Gone }), Ok(()));
        assert_eq!(save_all(&[], |_| Lookup::Found(SaveOutcome::Failed)), Ok(()));
    }

    #[test]
    fn save_all_aborts_when_a_panes_window_cannot_be_read() {
        let mut tried = Vec::new();
        let r = save_all(&[1, 2, 3], |id| {
            tried.push(id);
            if id == 2 { Lookup::Unreadable } else { Lookup::Found(SaveOutcome::Saved) }
        });
        assert_eq!((r, tried), (Err(SaveStop::Unreadable(2)), vec![1, 2]), "an unreachable file is not treated as gone");
    }

    #[test]
    fn quit_re_check_does_not_proceed_with_an_unreadable_window() {
        let w = AtStake { items: Vec::new(), dirty: vec![(7, "SKILL.md".into())] };
        assert_eq!(quit_fresh(vec![w.clone(), AtStake::default()], 1), None);
        assert_eq!(quit_fresh(vec![AtStake::default()], 2), None, "even with nothing visible at stake");
        assert_eq!(quit_fresh(vec![w.clone(), AtStake::default()], 0), Some(w));
    }

    #[test]
    fn saving_files_never_ends_agents_unasked() {
        let a = summary(AgentKind::Claude, "a", "A", 1);
        let listed = AtStake { items: vec![a.clone()], dirty: vec![(7, "SKILL.md".into())] };
        // Files saved, the listed agent still at work: ask about it again on its own bar.
        let fresh = AtStake { items: vec![a], dirty: Vec::new() };
        assert_eq!(after_save_decision(&listed, fresh.clone()), ConfirmDecision::Reprompt(fresh));
        // Only files, all saved: close.
        let files = AtStake { items: Vec::new(), dirty: vec![(7, "SKILL.md".into())] };
        assert_eq!(after_save_decision(&files, AtStake::default()), ConfirmDecision::Proceed);
        // A file that became dirty meanwhile is still asked about.
        let fresh = AtStake { items: Vec::new(), dirty: vec![(8, "a.md".into())] };
        assert_eq!(after_save_decision(&files, fresh.clone()), ConfirmDecision::Reprompt(fresh));
    }

    #[test]
    fn a_tab_is_found_by_its_panes_after_shifts() {
        let tabs = vec![vec![1, 2], vec![3], vec![4, 5]];
        assert_eq!(tab_index_for(&tabs, &[4, 5]), Some(2));
        let shifted = vec![vec![3], vec![4, 5]];
        assert_eq!(tab_index_for(&shifted, &[4, 5]), Some(1));
        assert_eq!(tab_index_for(&shifted, &[1, 2]), None);
        assert_eq!(tab_index_for(&shifted, &[]), None);
    }

    #[test]
    fn a_line_has_letter_name_and_status() {
        assert_eq!(item_line(&summary(AgentKind::Claude, "a", "修复登录", 1)), "「C · 修复登录 · 思考中」");
        assert_eq!(item_line(&summary(AgentKind::Codex, "b", "写迁移", 1)), "「X · 写迁移 · 思考中」");
        let english = crate::i18n::with_language(crate::i18n::Language::English, || {
            item_line(&SessionSummary { status: "Thinking".into(), ..summary(AgentKind::Claude, "a", "Fix login", 1) })
        });
        assert_eq!(english, "\"C · Fix login · Thinking\"");
    }

    #[test]
    fn close_bar_keys_follow_whether_files_are_unsaved() {
        let none = Modifiers::default();
        let cmd = Modifiers { platform: true, ..Default::default() };
        let shift = Modifiers { shift: true, ..Default::default() };
        let cmd_shift = Modifiers { platform: true, shift: true, ..Default::default() };
        // Agents-only bar: exactly as before.
        assert_eq!(close_key("escape", &none, false), Some(CloseKey::Cancel));
        assert_eq!(close_key("enter", &none, false), Some(CloseKey::Cancel));
        assert_eq!(close_key("enter", &cmd, false), Some(CloseKey::Close));
        assert_eq!(close_key("enter", &shift, false), None);
        assert_eq!(close_key("escape", &cmd, false), None);
        assert_eq!(close_key("a", &none, false), None);
        assert_eq!(close_key("s", &cmd, false), None, "⌘S is not the agents-only bar's");
        // Bar with unsaved files (E2a §6, default 保存): ↩ / ⌘S save all, Esc cancels, ⌘↩ = 不保存并关闭.
        assert_eq!(close_key("enter", &none, true), Some(CloseKey::SaveAll));
        assert_eq!(close_key("s", &cmd, true), Some(CloseKey::SaveAll));
        assert_eq!(close_key("escape", &none, true), Some(CloseKey::Cancel));
        assert_eq!(close_key("enter", &cmd, true), Some(CloseKey::Close));
        assert_eq!(close_key("enter", &shift, true), None);
        assert_eq!(close_key("escape", &cmd, true), None);
        assert_eq!(close_key("s", &none, true), None);
        assert_eq!(close_key("s", &cmd_shift, true), None);
    }
}
