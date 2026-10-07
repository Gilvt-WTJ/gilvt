//! The window's side of the inspector timeline (M3b spec §3.6): filters, expanding rows / history turns,
//! jumping to a row's terminal anchor (scroll + 1 s highlight, focusing that pane's tab), ⌘+click Quick
//! Look, copying a detail. Only scroll positions, focus and the inspector change: nothing is written to a
//! PTY.

use std::path::PathBuf;

use gilvt_agent::Anchor;
use gilvt_term::ScrollOutcome;
use gpui::{AnyWindowHandle, ClipboardItem, Context, Window};

use super::{focus_pane_anywhere, workspaces, PaneView, Workspace};
use crate::inspector::timeline_model::{jump, Filter, Jump};
use crate::inspector::InspectorState;
use crate::preview_view::OpenRequest;

impl Workspace {
    pub fn inspector_mut(&mut self) -> &mut InspectorState {
        &mut self.inspector
    }

    pub fn set_timeline_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.inspector.timeline.set_filter(filter);
        cx.notify();
    }

    /// ▸: opens / closes a row's detail (or a thinking block).
    pub fn toggle_timeline_row(&mut self, key: &str, cx: &mut Context<Self>) {
        self.inspector.timeline.toggle_row(key);
        cx.notify();
    }

    /// A history turn's line: expands / collapses it in place.
    pub fn toggle_timeline_turn(&mut self, index: u32, cx: &mut Context<Self>) {
        self.inspector.timeline.toggle_turn(index);
        cx.notify();
    }

    /// A click on a row: jump to its anchor (to the nearest line above it showing all of `needles`, when
    /// given: the agent's line for the call); without one (or with a full-screen program on the alternate
    /// screen) the click opens / closes the detail; evicted / closed → a toast.
    pub fn timeline_click(&mut self, key: &str, anchor: Option<Anchor>, needles: &[Vec<String>], window: &mut Window, cx: &mut Context<Self>) {
        match jump(anchor, |a| self.scroll_to_anchor(a, needles, window, cx)) {
            Jump::Highlight => {}
            Jump::Toast(note) => self.inspector_note(note, cx),
            Jump::ToggleDetail => self.toggle_timeline_row(key, cx),
        }
    }

    /// Scrolls the anchor's terminal (this window, else another one) and, when the line is shown, brings
    /// its pane forward (tab activated, keyboard moved) unless it already has the keyboard here. None: the
    /// pane is gone.
    fn scroll_to_anchor(&mut self, a: Anchor, needles: &[Vec<String>], window: &mut Window, cx: &mut Context<Self>) -> Option<ScrollOutcome> {
        if let Some(PaneView::Terminal(t)) = self.panes.get(&a.pane).cloned() {
            let outcome = t.update(cx, |t, cx| t.jump_to_call(a.line, needles, cx));
            let covered = self.quicklook.is_some() || self.finder.is_some();
            if matches!(outcome, ScrollOutcome::Shown { .. }) && (self.focused_pane() != Some(a.pane) || covered) {
                self.focus_pane(a.pane, window, cx);
            }
            return Some(outcome);
        }
        // This window is being updated: never read it through its handle.
        let me = window.window_handle();
        let t = workspaces(cx).into_iter().filter(|w| AnyWindowHandle::from(*w) != me).find_map(|w| match w.read(cx).ok()?.panes.get(&a.pane)? {
            PaneView::Terminal(t) => Some(t.clone()),
            PaneView::Preview(_) | PaneView::Editor(_) | PaneView::Monitor(_) => None,
        })?;
        let outcome = t.update(cx, |t, cx| t.jump_to_call(a.line, needles, cx));
        if matches!(outcome, ScrollOutcome::Shown { .. }) {
            let pane = a.pane;
            cx.defer(move |cx| focus_pane_anywhere(pane, cx));
        }
        Some(outcome)
    }

    /// ⌘+click on a row's file name: Quick Look over the focused pane.
    pub fn timeline_quick_look(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let origin = self.focused_pane();
        self.open_preview(OpenRequest::file(path, None, None), false, origin, window, cx);
    }

    /// The copy button of an expanded detail.
    pub fn copy_timeline_detail(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.inspector_note("已复制", cx);
    }
}
