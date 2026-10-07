//! Saving this window's layout and rebuilding it from a snapshot (P0 spec §3).

use std::path::PathBuf;

use gpui::{App, Context, Window};

use super::{to_rect, PaneView, Tab, Workspace};
use crate::agents::Agents;
use crate::pane_tree::PaneTree;
use crate::persist::restore_cwd;
use crate::persist::snapshot::{FrameSnap, NodeSnap, PaneSnap, TabSnap, WindowSnap};

impl Workspace {
    /// This window's layout. Preview and editor panes are left out (see `NodeSnap::from_node`); tabs left with no
    /// terminal are left out too (so a file tab never comes back).
    pub fn snapshot(&self, window: &Window, cx: &App) -> WindowSnap {
        let agents = cx.try_global::<Agents>();
        let mut active = 0;
        let mut tabs = Vec::new();
        let (mut monitor, mut monitor_active) = (false, false);
        for (i, tab) in self.tabs.iter().enumerate() {
            let only_monitor = tab.tree.panes().iter().all(|p| matches!(self.panes.get(p), Some(PaneView::Monitor(_))));
            if only_monitor {
                monitor = true;
                monitor_active |= i == self.active;
                continue;
            }
            let tree = NodeSnap::from_node(tab.tree.root(), &mut |id| {
                let PaneView::Terminal(t) = self.panes.get(&id)? else { return None };
                Some(PaneSnap { pane_id: id, cwd: t.read(cx).cwd(), agent: agents.and_then(|a| a.agent_snap(id)) })
            });
            let Some(tree) = tree else { continue };
            let focused = if tree.panes().iter().any(|p| p.pane_id == tab.focused) { tab.focused } else { tree.panes()[0].pane_id };
            if i == self.active {
                active = tabs.len();
            }
            tabs.push(TabSnap { tree, focused });
        }
        let r = to_rect(window.bounds());
        WindowSnap { frame: Some(FrameSnap { x: r.x, y: r.y, w: r.w, h: r.h }), active_tab: active, tabs, monitor, monitor_active }
    }

    /// A window rebuilt from `snap`: every terminal restarts in its saved cwd under its saved pane id (a missing
    /// directory falls back to `$HOME` with a notice). Agents are not started: their sessions are registered as
    /// pending resumes. A tab whose shell cannot start is skipped; with no tab left the window opens one in `$HOME`.
    pub fn restore(error: Option<String>, snap: &WindowSnap, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut ws = Self::blank(error, window, cx);
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut fell_back = Vec::new();
        for tab in &snap.tabs {
            let mut spawned = Vec::new();
            let mut failed = false;
            for pane in tab.tree.panes() {
                let (cwd, fallback) = restore_cwd(pane.cwd.as_deref(), home.clone());
                if fallback {
                    fell_back.push(pane.cwd.clone().unwrap_or_default());
                }
                match ws.spawn_pane_with_id(pane.pane_id, cwd, window, cx) {
                    Some(id) => spawned.push(id),
                    None => {
                        failed = true;
                        break;
                    }
                }
                if let Some(agent) = &pane.agent {
                    Agents::add_pending(pane.pane_id, pane.cwd.clone().or_else(|| home.clone()), agent, cx);
                }
            }
            if failed {
                for id in spawned {
                    ws.panes.remove(&id);
                    ws.subscriptions.remove(&id);
                    Agents::pane_closed(id, cx);
                }
                continue;
            }
            ws.tabs.push(Tab::new(PaneTree::from_root(tab.tree.to_node()), tab.focused));
        }
        if ws.tabs.is_empty() {
            ws.open_tab(home, window, cx);
        }
        if snap.monitor {
            ws.open_monitor(window, cx); // inserts at 0 and activates it
        }
        ws.active = restored_active(snap, ws.tabs.len());
        ws.focus_active(window, cx);
        if let Some(first) = fell_back.first() {
            ws.error = Some(if crate::i18n::current() == crate::i18n::Language::English {
                format!("Directory {} no longer exists; the pane was started in the home directory", first.display())
            } else {
                format!("目录 {} 已不存在，该 pane 改在主目录启动", first.display())
            });
        }
        ws
    }
}

/// Which tab is active after a restore: `snap.active_tab` indexes the saved terminal tabs, and a restored monitor
/// tab sits in front of them (index 0), shifting them by one.
fn restored_active(snap: &WindowSnap, tabs_len: usize) -> usize {
    if snap.monitor && snap.monitor_active {
        return 0;
    }
    (snap.active_tab + usize::from(snap.monitor)).min(tabs_len.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(active_tab: usize, monitor: bool, monitor_active: bool) -> WindowSnap {
        WindowSnap { frame: None, active_tab, tabs: vec![], monitor, monitor_active }
    }

    #[test]
    fn the_monitor_tab_shifts_the_saved_terminal_index() {
        assert_eq!(restored_active(&snap(1, false, false), 3), 1, "no monitor: as saved");
        assert_eq!(restored_active(&snap(1, true, false), 4), 2, "monitor in front: +1");
        assert_eq!(restored_active(&snap(0, true, false), 2), 1, "first terminal tab");
        assert_eq!(restored_active(&snap(2, true, true), 4), 0, "monitor was active");
        assert_eq!(restored_active(&snap(9, true, false), 3), 2, "clamped");
        assert_eq!(restored_active(&snap(9, false, false), 3), 2, "clamped");
    }
}
