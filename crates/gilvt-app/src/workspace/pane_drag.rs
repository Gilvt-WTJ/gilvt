//! Moving a pinned preview or editor pane out of its split into a tab of its own: drag its header onto the tab
//! bar, or press `T` in a pinned preview. The new tab sits right of the old one and, like a file tab, returns
//! there when it closes (or on Esc from a preview with no terminal beside it).

use gpui::{div, prelude::*, px, Context, Window};

use super::{Tab, Workspace};
use crate::pane_tree::{PaneId, PaneTree};
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

/// Takes pane `id` out of its tab's split into a new tab inserted right after it; returns the new tab's index.
/// None when no tab has the pane, or it is alone in its tab (it already has the whole pane area).
fn detach_to_new_tab(tabs: &mut Vec<Tab>, id: PaneId) -> Option<usize> {
    let ti = tabs.iter().position(|t| t.tree.contains(id))?;
    let tab = &mut tabs[ti];
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
    tabs.insert(ti + 1, moved);
    Some(ti + 1)
}

impl Workspace {
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
    use super::detach_to_new_tab;
    use crate::pane_tree::{Axis, PaneTree};
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
}
