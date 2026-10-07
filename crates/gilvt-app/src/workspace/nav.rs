//! Where a pane is (tab, position in the tab) and moving the keyboard to it from anywhere: the sidebar,
//! and later the switching shortcuts and notifications. Focus changes never write to the PTY.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use gilvt_agent::SessionKey;
use gpui::{App, Context, WindowHandle};

use super::{PaneView, Workspace};
use crate::agents::Agents;
use crate::pane_tree::{PaneId, Rect};

/// How long a pane flashes after `focus_pane`.
pub const FLASH: Duration = Duration::from_millis(400);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneLocation {
    pub tab: usize,
    /// What the tab bar shows for the tab.
    pub tab_title: String,
    /// 左 / 右 / 上 / 下 (左上 … in a grid, 中 in the middle); "" when the pane is alone in its tab.
    pub position: &'static str,
}

const EPS: f32 = 1e-3;

fn side(start: f32, len: f32, low: &'static str, high: &'static str) -> Option<&'static str> {
    if len >= 1.0 - EPS {
        None
    } else if start < EPS {
        Some(low)
    } else if start + len > 1.0 - EPS {
        Some(high)
    } else {
        Some("中")
    }
}

/// Position word of a pane laid out at `r` inside the unit square.
pub fn position_word(r: Rect) -> &'static str {
    match (side(r.x, r.w, "左", "右"), side(r.y, r.h, "上", "下")) {
        (None, None) => "",
        (Some(h), None) => h,
        (None, Some(v)) => v,
        (Some("中"), Some(v)) => v,
        (Some(h), Some("中")) => h,
        (Some(h), Some(v)) => match (h, v) {
            ("左", "上") => "左上",
            ("左", _) => "左下",
            (_, "上") => "右上",
            _ => "右下",
        },
    }
}

impl Workspace {
    pub fn pane_location(&self, pane: PaneId, cx: &App) -> Option<PaneLocation> {
        let tab = self.tabs.iter().position(|t| t.tree.contains(pane))?;
        let t = &self.tabs[tab];
        let tab_title = self.tab_title(t, cx);
        let rect = t.tree.layout(Rect::new(0.0, 0.0, 1.0, 1.0)).into_iter().find(|(id, _)| *id == pane)?.1;
        Some(PaneLocation { tab, tab_title, position: position_word(rect) })
    }

    /// Activates the pane's tab, gives it the keyboard and flashes it once. Closes the palettes and
    /// Quick Look, which would otherwise keep the keyboard.
    pub fn focus_pane(&mut self, pane: PaneId, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let Some(ti) = self.tabs.iter().position(|t| t.tree.contains(pane)) else { return };
        self.ended_confirm = None;
        self.finder = None;
        self.sessions = None;
        self.new_agent = None;
        self.quicklook = None;
        self.session_menu = None;
        // No stale rename field left behind; its own blur handler must not steal focus back.
        self.renaming = None;
        self.active = ti;
        let tab = &mut self.tabs[ti];
        if tab.zoomed && tab.focused != pane {
            tab.zoomed = false;
        }
        tab.focused = pane;
        self.flash_seq += 1;
        let seq = self.flash_seq;
        self.flash = Some((pane, seq));
        cx.spawn(async move |ws, cx| {
            cx.background_executor().timer(FLASH).await;
            let _ = ws.update(cx, |ws, cx| {
                if ws.flash.is_some_and(|(_, s)| s == seq) {
                    ws.flash = None;
                    cx.notify();
                }
            });
        })
        .detach();
        self.focus_active(window, cx);
    }

    /// The pane is drawn in this window now (its tab is active and it is not hidden by a zoom).
    pub fn pane_shown(&self, pane: PaneId) -> bool {
        self.tabs.get(self.active).is_some_and(|t| if t.zoomed { t.focused == pane } else { t.tree.contains(pane) })
    }

    /// The active tab's focused pane.
    pub fn focused_pane(&self) -> Option<PaneId> {
        self.tabs.get(self.active).map(|t| t.focused)
    }

    /// The terminal view of pane `pane` in this window.
    pub fn terminal_view(&self, pane: PaneId) -> Option<&gpui::Entity<crate::terminal_view::TerminalView>> {
        match self.panes.get(&pane)? {
            PaneView::Terminal(t) => Some(t),
            _ => None,
        }
    }

    fn foreground_pids(&self, cx: &App) -> Vec<(PaneId, Option<u32>)> {
        self.panes
            .iter()
            .filter_map(|(id, p)| match p {
                PaneView::Terminal(t) => Some((*id, t.read(cx).session.foreground_pid())),
                PaneView::Preview(_) | PaneView::Editor(_) | PaneView::Monitor(_) => None,
            })
            .collect()
    }

    /// Tells the registry the focused terminal was seen (clears 完成未看), if this window has the keyboard.
    pub(super) fn report_focus(&self, window: &gpui::Window, cx: &mut App) {
        if !window.is_window_active() {
            return;
        }
        if let Some(id) = self.focused_pane().filter(|id| matches!(self.panes.get(id), Some(PaneView::Terminal(_)))) {
            Agents::pane_focused(id, cx);
        }
    }
}

/// Every workspace window, in the order gpui lists them.
pub fn workspaces(cx: &App) -> Vec<WindowHandle<Workspace>> {
    cx.windows().into_iter().filter_map(|w| w.downcast::<Workspace>()).collect()
}

/// Panes the user can see now: shown in the active (key) window.
pub fn visible_panes(cx: &mut App) -> HashSet<PaneId> {
    let mut out = HashSet::new();
    for w in workspaces(cx) {
        if w.is_active(cx) != Some(true) {
            continue;
        }
        if let Ok(ws) = w.read(cx) {
            out.extend(ws.panes.keys().copied().filter(|p| ws.pane_shown(*p)));
        }
    }
    out
}

/// Foreground process group of every terminal pane (None when unknown).
pub fn terminal_pids(cx: &App) -> Vec<(PaneId, Option<u32>)> {
    workspaces(cx).into_iter().filter_map(|w| w.read(cx).ok().map(|ws| ws.foreground_pids(cx))).flatten().collect()
}

/// A terminal pane as the sidebar lists it.
pub struct TerminalPane {
    pub pane: PaneId,
    /// The tab-bar title of the pane (its custom title, else the foreground program).
    pub title: String,
    pub cwd: Option<PathBuf>,
}

impl Workspace {
    /// The terminal panes of this window, in tab order and, within a tab, in layout order.
    pub fn terminal_panes(&self, cx: &App) -> Vec<TerminalPane> {
        let mut out = Vec::new();
        for tab in &self.tabs {
            for id in tab.tree.panes() {
                if let Some(PaneView::Terminal(t)) = self.panes.get(&id) {
                    let t = t.read(cx);
                    out.push(TerminalPane { pane: id, title: t.title(), cwd: t.cwd() });
                }
            }
        }
        out
    }
}

/// The directories of every plain terminal pane of every window (the sidebar resolves their projects and git
/// facts). Panes an agent occupies are skipped, as in the sidebar list: their sessions have their own rule.
pub fn terminal_dirs(cx: &App) -> Vec<PathBuf> {
    let occupied = if cx.has_global::<Agents>() { cx.global::<Agents>().occupied_panes() } else { Default::default() };
    workspaces(cx)
        .into_iter()
        .filter_map(|w| w.read(cx).ok().map(|ws| ws.terminal_panes(cx)))
        .flatten()
        .filter(|t| !occupied.contains(&t.pane))
        .filter_map(|t| t.cwd)
        .collect()
}

/// Absolute cursor line of terminal pane `pane` (a hook's terminal anchor). None: no such terminal, or a
/// program is on the alternate screen.
pub fn absolute_cursor_line(pane: PaneId, cx: &App) -> Option<u64> {
    workspaces(cx).into_iter().find_map(|w| match w.read(cx).ok()?.panes.get(&pane)? {
        PaneView::Terminal(t) => Some(t.read(cx).session.absolute_cursor_line()),
        PaneView::Preview(_) | PaneView::Editor(_) | PaneView::Monitor(_) => Some(None),
    })?
}

/// Repaints the windows whose inspector is shown and follows one of these sessions (their timeline or
/// permission mode changed).
pub fn notify_sessions(keys: &[SessionKey], cx: &mut App) {
    for w in workspaces(cx) {
        let shows = w
            .read(cx)
            .ok()
            .filter(|ws| !ws.inspector.hidden)
            .and_then(|ws| ws.focused_pane())
            .and_then(|p| crate::inspector::model::follow(cx.global::<Agents>().registry().sessions(), p));
        if shows.is_some_and(|s| keys.contains(&s.key)) {
            let _ = w.update(cx, |_, _, cx| cx.notify());
        }
    }
}

/// Repaints the workspace windows that have a monitor pane (any of them may list a pane of another window). A
/// pane's command blocks are drawn nowhere else: DebugState reads them when queried, not from a frame.
pub fn notify_monitors(cx: &mut App) {
    for w in workspaces(cx) {
        if w.read(cx).is_ok_and(|ws| ws.monitor_pane().is_some()) {
            let _ = w.update(cx, |_, _, cx| cx.notify());
        }
    }
}

/// Repaints every workspace window.
pub fn notify_all(cx: &mut App) {
    for w in workspaces(cx) {
        let _ = w.update(cx, |_, _, cx| cx.notify());
    }
}

/// Brings the window owning `pane` to the front and focuses the pane there. Call it outside window
/// updates (e.g. via `cx.defer`).
pub fn focus_pane_anywhere(pane: PaneId, cx: &mut App) {
    let Some(w) = workspaces(cx).into_iter().find(|w| w.read(cx).is_ok_and(|ws| ws.has_pane(pane))) else { return };
    cx.activate(true);
    let _ = w.update(cx, |ws, window, cx| {
        window.activate_window();
        ws.focus_pane(pane, window, cx);
    });
}

/// Index of the window whose panes include `pane`.
fn owner_of(pane: PaneId, window_panes: &[Vec<PaneId>]) -> Option<usize> {
    window_panes.iter().position(|panes| panes.contains(&pane))
}

/// Resumes pending session `key` in the window that owns its pane (brought to the front). An entry no
/// window owns any more is stale and dropped. Call it outside window updates (e.g. via `cx.defer`).
pub fn resume_pending_anywhere(key: &SessionKey, cx: &mut App) {
    let Some(pane) = cx.global::<Agents>().pending().iter().find(|p| &p.key == key).map(|p| p.pane) else { return };
    let windows = workspaces(cx);
    let owned: Vec<Vec<PaneId>> =
        windows.iter().map(|w| w.read(cx).map(|ws| if ws.has_pane(pane) { vec![pane] } else { Vec::new() }).unwrap_or_default()).collect();
    let Some(i) = owner_of(pane, &owned) else {
        Agents::take_pending(key, cx);
        return;
    };
    cx.activate(true);
    let key = key.clone();
    let _ = windows[i].update(cx, |ws, window, cx| {
        window.activate_window();
        ws.resume_pending(&key, window, cx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane_tree::{Axis, PaneTree};

    fn words(tree: &PaneTree) -> Vec<(PaneId, &'static str)> {
        tree.layout(Rect::new(0.0, 0.0, 1.0, 1.0)).into_iter().map(|(id, r)| (id, position_word(r))).collect()
    }

    #[test]
    fn owner_of_finds_the_window_with_the_pane() {
        let windows = vec![vec![1, 2], vec![], vec![7, 9]];
        assert_eq!(owner_of(9, &windows), Some(2));
        assert_eq!(owner_of(1, &windows), Some(0));
        assert_eq!(owner_of(5, &windows), None);
        assert_eq!(owner_of(5, &[]), None);
    }

    #[test]
    fn position_words_follow_the_layout() {
        let mut t = PaneTree::new(1);
        assert_eq!(words(&t), [(1, "")]);
        t.split(1, 2, Axis::Row);
        assert_eq!(words(&t), [(1, "左"), (2, "右")]);
        t.split(2, 3, Axis::Column);
        assert_eq!(words(&t), [(1, "左"), (2, "右上"), (3, "右下")]);
        let mut t = PaneTree::new(1);
        t.split(1, 2, Axis::Column);
        t.split(2, 3, Axis::Column);
        assert_eq!(words(&t), [(1, "上"), (2, "中"), (3, "下")]);
        let mut t = PaneTree::new(1);
        t.split(1, 2, Axis::Row);
        t.split(2, 3, Axis::Row);
        t.split(2, 4, Axis::Column);
        assert_eq!(words(&t), [(1, "左"), (2, "上"), (4, "下"), (3, "右")]);
    }
}
