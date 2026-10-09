//! Moving a pane out of its split into a tab of its own: `⌘⇧T` on any pane, drag a pinned preview's or an
//! editor's header onto the tab bar, or press `T` in a pinned preview. The new tab sits right of the old one and,
//! like a file tab, returns there when it closes (or on Esc from a preview with no terminal beside it). `⌘⇧T`
//! in such a tab puts the pane back into the split it came from.

use gpui::{div, prelude::*, px, Context, Window};

use super::{Tab, Workspace};
use crate::pane_tree::{Axis, PaneId, PaneTree, Slot};
use crate::theme::{hsla, mix};

/// What a pane header drags: the pane, and its title for the chip under the pointer.
#[derive(Clone)]
pub struct DraggedPane {
    pub pane: PaneId,
    pub title: String,
}

/// The chip that follows the pointer while a pane header is dragged.
impl Render for DraggedPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = crate::terminal_view::TerminalView::palette(window, cx);
        div()
            .px_3()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(hsla(p.ansi[4]))
            .bg(hsla(mix(p.background, p.foreground, 0.1)))
            .text_color(hsla(p.foreground))
            .text_size(px(12.))
            .whitespace_nowrap()
            .shadow_lg()
            .child(format!("{} → 新标签", self.title))
    }
}

/// Where a moved pane goes back to in its origin tab.
#[derive(Clone, Debug)]
pub(super) struct Return {
    /// The origin tab's split before and right after the move: while it is still `after`, going back restores
    /// `before` exactly (position and sizes).
    before: PaneTree,
    after: PaneTree,
    /// Otherwise: beside this neighbour, when it is still in the origin tab.
    slot: Slot,
}

/// Takes pane `id` out of its tab's split into a new tab inserted right after it; returns the new tab's index.
/// None when no tab has the pane, or it is alone in its tab (it already has the whole pane area).
fn detach_to_new_tab(tabs: &mut Vec<Tab>, id: PaneId) -> Option<usize> {
    let ti = tabs.iter().position(|t| t.tree.contains(id))?;
    let tab = &mut tabs[ti];
    let before = tab.tree.clone();
    let slot = before.slot_of(id)?;
    if !tab.tree.remove(id) {
        return None;
    }
    if tab.focused == id {
        if let Some(&first) = tab.tree.panes().first() {
            tab.focused = first;
        }
    }
    tab.zoomed = false;
    let origin = tab.id;
    let mut moved = Tab::new(PaneTree::new(id), id);
    moved.origin_tab = Some(origin);
    moved.return_to = Some(Return { before, after: tab.tree.clone(), slot });
    tabs.insert(ti + 1, moved);
    Some(ti + 1)
}

/// Puts the lone pane of moved tab `ti` back into the tab it was moved from and drops tab `ti`; returns the
/// origin tab's index afterwards. Back in its old place when the origin tab is unchanged, else beside its old
/// neighbour, else right of the origin tab's focused pane. None (nothing changes) when tab `ti` was not moved
/// out of a split, holds more than one pane, or its origin tab is gone.
fn return_to_origin(tabs: &mut Vec<Tab>, ti: usize) -> Option<usize> {
    let tab = tabs.get(ti)?;
    let Some(ret) = tab.return_to.clone() else { return None };
    let [id] = tab.tree.panes()[..] else { return None };
    let oi = tabs.iter().position(|t| Some(t.id) == tab.origin_tab)?;
    let origin = &mut tabs[oi];
    if origin.tree == ret.after {
        origin.tree = ret.before;
    } else if !origin.tree.insert(ret.slot.anchor, id, ret.slot.axis, ret.slot.before) {
        origin.tree.split(origin.focused, id, Axis::Row);
    }
    origin.focused = id;
    origin.zoomed = false;
    tabs.remove(ti);
    Some(if ti < oi { oi - 1 } else { oi })
}

/// `⌘⇧T` on tab `active`: its focused pane goes back where it came from when it was moved out of a split
/// (`return_to_origin`), else out of its split into a new tab (`detach_to_new_tab`). Returns the tab to activate.
fn toggle_pane_tab(tabs: &mut Vec<Tab>, active: usize) -> Option<usize> {
    let focused = tabs.get(active)?.focused;
    return_to_origin(tabs, active).or_else(|| detach_to_new_tab(tabs, focused))
}

impl Workspace {
    /// `⌘⇧T`: moves the focused pane to a tab of its own, or back into the split it was moved out of.
    pub(super) fn toggle_pane_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(at) = toggle_pane_tab(&mut self.tabs, self.active) {
            self.active = at;
            self.focus_active(window, cx);
        }
    }

    /// Moves pane `id` into a new tab right of its own and gives it the keyboard (it is moved there to be read).
    /// A pane alone in its tab, or one this window does not have (a drag from another window), stays put.
    pub(super) fn move_pane_to_new_tab(&mut self, id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if !self.panes.contains_key(&id) {
            return;
        }
        if let Some(at) = detach_to_new_tab(&mut self.tabs, id) {
            self.active = at;
            self.focus_active(window, cx);
        }
    }

    /// The tab a moved pane's tab was opened from, when it is still there; None otherwise.
    pub(super) fn origin_tab_of(&self, id: PaneId) -> Option<usize> {
        let origin = self.tabs.iter().find(|t| t.tree.contains(id))?.origin_tab?;
        self.tabs.iter().position(|t| t.id == origin)
    }
}

#[cfg(test)]
mod tests {
    use super::{detach_to_new_tab, toggle_pane_tab};
    use crate::pane_tree::{Axis, PaneTree, Rect, Slot};
    use crate::workspace::Tab;

    fn split_tab() -> Tab {
        let mut tree = PaneTree::new(1);
        tree.split(1, 2, Axis::Row);
        Tab::new(tree, 1)
    }

    #[test]
    fn a_split_pane_moves_to_a_tab_right_of_its_own() {
        let mut tabs = vec![Tab::new(PaneTree::new(9), 9), split_tab(), Tab::new(PaneTree::new(5), 5)];
        let origin = tabs[1].id;
        assert_eq!(detach_to_new_tab(&mut tabs, 2), Some(2));
        assert_eq!(tabs.len(), 4);
        assert_eq!(tabs[1].tree.panes(), vec![1]);
        assert_eq!(tabs[2].tree.panes(), vec![2]);
        assert_eq!(tabs[2].focused, 2);
        assert_eq!(tabs[2].origin_tab, Some(origin));
        assert_eq!(tabs[3].tree.panes(), vec![5]);
    }

    #[test]
    fn moving_the_focused_pane_refocuses_what_is_left() {
        let mut tab = split_tab();
        tab.focused = 2;
        tab.zoomed = true;
        let mut tabs = vec![tab];
        assert_eq!(detach_to_new_tab(&mut tabs, 2), Some(1));
        assert_eq!(tabs[0].focused, 1);
        assert!(!tabs[0].zoomed);
    }

    #[test]
    fn a_pane_alone_in_its_tab_or_unknown_stays() {
        let mut tabs = vec![Tab::new(PaneTree::new(1), 1)];
        assert_eq!(detach_to_new_tab(&mut tabs, 1), None);
        assert_eq!(detach_to_new_tab(&mut tabs, 7), None);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].tree.panes(), vec![1]);
    }

    /// 1 | (2 / 3), with pane 2 a third of the width.
    fn three_pane_tab() -> Tab {
        let mut tree = PaneTree::new(1);
        tree.split(1, 2, Axis::Row);
        tree.split(2, 3, Axis::Column);
        tree.drag_divider(&[], 0, 66.0, Rect::new(0.0, 0.0, 100.0, 100.0));
        Tab::new(tree, 1)
    }

    #[test]
    fn toggling_twice_puts_the_pane_back_exactly() {
        let mut tabs = vec![Tab::new(PaneTree::new(9), 9), three_pane_tab()];
        let original = tabs[1].tree.clone();
        tabs[1].focused = 2;
        assert_eq!(toggle_pane_tab(&mut tabs, 1), Some(2));
        assert_eq!(tabs[1].tree.panes(), vec![1, 3]);
        assert_eq!(tabs[2].tree.panes(), vec![2]);
        assert_eq!(toggle_pane_tab(&mut tabs, 2), Some(1));
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[1].tree, original);
        assert_eq!(tabs[1].focused, 2);
    }

    #[test]
    fn a_changed_origin_takes_the_pane_beside_its_old_neighbour() {
        let mut tabs = vec![three_pane_tab()];
        tabs[0].focused = 2;
        assert_eq!(toggle_pane_tab(&mut tabs, 0), Some(1));
        // Meanwhile pane 1 closes in the origin tab: pane 2 goes back above 3, its old neighbour.
        assert!(tabs[0].tree.remove(1));
        assert_eq!(toggle_pane_tab(&mut tabs, 1), Some(0));
        assert_eq!(tabs[0].tree.panes(), vec![2, 3]);
        assert_eq!(tabs[0].tree.slot_of(2).map(|s| s.axis), Some(Axis::Column));
        assert_eq!(tabs[0].focused, 2);
    }

    #[test]
    fn without_its_old_neighbour_the_pane_goes_right_of_the_focused_one() {
        let mut tabs = vec![split_tab()];
        tabs[0].focused = 2;
        assert_eq!(toggle_pane_tab(&mut tabs, 0), Some(1));
        // Pane 1 (its neighbour) is split and closed, leaving pane 4.
        tabs[0].tree.split(1, 4, Axis::Column);
        assert!(tabs[0].tree.remove(1));
        tabs[0].focused = 4;
        assert_eq!(toggle_pane_tab(&mut tabs, 1), Some(0));
        assert_eq!(tabs[0].tree.panes(), vec![4, 2]);
        assert_eq!(tabs[0].tree.slot_of(2), Some(Slot { anchor: 4, axis: Axis::Row, before: false }));
    }

    #[test]
    fn a_moved_tab_left_of_its_origin_returns_to_the_shifted_index() {
        let mut tabs = vec![split_tab(), Tab::new(PaneTree::new(9), 9)];
        tabs[0].focused = 2;
        assert_eq!(toggle_pane_tab(&mut tabs, 0), Some(1));
        tabs.swap(0, 1); // the moved tab now sits left of its origin
        assert_eq!(toggle_pane_tab(&mut tabs, 0), Some(0));
        assert_eq!(tabs[0].tree.panes(), vec![1, 2]);
    }

    #[test]
    fn nothing_moves_back_without_an_origin_or_with_a_split_moved_tab() {
        // A lone pane in an ordinary tab.
        let mut tabs = vec![Tab::new(PaneTree::new(1), 1)];
        assert_eq!(toggle_pane_tab(&mut tabs, 0), None);
        assert_eq!(tabs.len(), 1);

        // The origin tab is gone.
        let mut tabs = vec![split_tab()];
        tabs[0].focused = 2;
        toggle_pane_tab(&mut tabs, 0);
        tabs.remove(0);
        assert_eq!(toggle_pane_tab(&mut tabs, 0), None);
        assert_eq!(tabs[0].tree.panes(), vec![2]);

        // The moved tab was split again: ⌘⇧T moves its focused pane out instead.
        let mut tabs = vec![split_tab()];
        tabs[0].focused = 2;
        toggle_pane_tab(&mut tabs, 0);
        tabs[1].tree.split(2, 5, Axis::Row);
        tabs[1].focused = 5;
        assert_eq!(toggle_pane_tab(&mut tabs, 1), Some(2));
        assert_eq!(tabs.iter().map(|t| t.tree.panes()).collect::<Vec<_>>(), vec![vec![1], vec![2], vec![5]]);
    }
}
