//! The editor's live preview (spec 2026-10-05 §3): a read-only `PreviewView` pane to the right of an editor
//! pane that follows its buffer. Opening, closing, switching provider, the 200 ms debounce and the scroll
//! sync live here; what a provider builds is `crate::live_preview`.

use std::time::Duration;

use gpui::{AppContext as _, Context, Window};

use super::{PaneView, Workspace};
use crate::live_preview;
use crate::pane_tree::{next_pane_id, Axis, PaneId};
use crate::preview_view::{PreviewEvent, PreviewView};
use crate::sidebar::UiPrefs;

impl Workspace {
    /// `⌘⇧V` / the header button: open the preview of editor pane `editor`, or close it when it is open.
    pub(super) fn toggle_live_preview(&mut self, editor: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(preview) = self.live_pairs.preview_of(editor) {
            self.close_pane(preview, window, cx);
        } else {
            self.open_live_preview(editor, window, cx);
        }
    }

    fn open_live_preview(&mut self, editor: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(PaneView::Editor(ev)) = self.panes.get(&editor).cloned() else { return };
        let Some(ti) = self.tabs.iter().position(|t| t.tree.contains(editor)) else { return };
        let name = ev.read(cx).title();
        let prefs = cx.try_global::<UiPrefs>().map(|p| p.live_preview.clone()).unwrap_or_default();
        let provider = live_preview::default_provider(&name, &prefs);
        let input = ev.read(cx).live_input(provider, editor);
        let too_large = ev.read(cx).live_too_large();
        let view = cx.new(|cx| PreviewView::new_live(input, too_large, window, cx));
        let id = next_pane_id();
        view.update(cx, |v, _| v.pane = Some(id));
        let sub = cx.subscribe_in(&view, window, move |ws, _, event, window, cx| match event {
            // Esc in the preview gives the keyboard back to the editor it follows.
            PreviewEvent::Leave => ws.focus_pane(editor, window, cx),
            PreviewEvent::NewTab => ws.move_pane_to_new_tab(id, window, cx),
            _ => {}
        });
        self.subscriptions.insert(id, sub);
        self.panes.insert(id, PaneView::Preview(view.clone()));
        let tab = &mut self.tabs[ti];
        tab.tree.split(editor, id, Axis::Row);
        tab.focused = editor;
        tab.zoomed = false;
        self.active = ti;
        self.live_pairs.insert(editor, id);
        ev.update(cx, |v, cx| {
            v.live = Some(provider);
            cx.notify();
        });
        // `new_live` already builds the whole buffer once (so `refreshes` is 1 after opening); no second build is
        // scheduled. A large buffer only gets its banner, which `PreviewView` keeps across that first build.
        if too_large {
            view.update(cx, |v, cx| v.set_banner_text(live_preview::too_large_banner().to_string(), cx));
        }
        self.focus_active(window, cx);
    }

    /// The header button with the preview open and more than one provider: the next one, remembered per file type.
    pub(super) fn cycle_live_preview(&mut self, editor: PaneId, cx: &mut Context<Self>) {
        let Some(PaneView::Editor(ev)) = self.panes.get(&editor).cloned() else { return };
        let (name, current) = {
            let v = ev.read(cx);
            (v.title(), v.live)
        };
        let Some(current) = current else { return };
        let next = live_preview::next_provider(&live_preview::providers_for(&name), current);
        ev.update(cx, |v, cx| {
            v.live = Some(next);
            cx.notify();
        });
        let key = live_preview::pref_key(&name);
        UiPrefs::remember(
            move |p| {
                p.live_preview.insert(key, next.id().to_string());
            },
            cx,
        );
        self.live_debounce.remove(&editor);
        // An explicit action, never fired by typing: builds at once even for a large buffer.
        self.push_live_text(editor, true, cx);
    }

    /// Rebuilds the preview of `editor` after the debounce (at once when `force`, for a save).
    pub(super) fn schedule_live_refresh(&mut self, editor: PaneId, force: bool, cx: &mut Context<Self>) {
        if self.live_pairs.preview_of(editor).is_none() {
            return;
        }
        let delay = if force { Duration::ZERO } else { Duration::from_millis(live_preview::DEBOUNCE_MS) };
        let task = cx.spawn(async move |ws, cx| {
            cx.background_executor().timer(delay).await;
            let _ = ws.update(cx, |ws, cx| ws.push_live_text(editor, force, cx));
        });
        // Replacing the entry drops (cancels) the previous pending refresh.
        self.live_debounce.insert(editor, task);
    }

    /// Hands the editor's current buffer to its preview. A pair whose pane is gone is forgotten. A large buffer is
    /// only built when `force` (a save or an explicit provider switch); typing just keeps the banner.
    pub(super) fn push_live_text(&mut self, editor: PaneId, force: bool, cx: &mut Context<Self>) {
        let Some(preview) = self.live_pairs.preview_of(editor) else { return };
        let (Some(PaneView::Editor(ev)), Some(PaneView::Preview(pv))) = (self.panes.get(&editor).cloned(), self.panes.get(&preview).cloned()) else {
            self.live_pairs.remove_editor(editor);
            self.live_debounce.remove(&editor);
            if let Some(PaneView::Editor(v)) = self.panes.get(&editor) {
                v.update(cx, |v, cx| {
                    v.live = None;
                    cx.notify();
                });
            }
            return;
        };
        let Some(provider) = ev.read(cx).live else { return };
        let too_large = ev.read(cx).live_too_large();
        if too_large && !force {
            pv.update(cx, |v, cx| v.set_banner_text(live_preview::too_large_banner().to_string(), cx));
            return;
        }
        let input = ev.read(cx).live_input(provider, editor);
        pv.update(cx, |v, cx| v.set_live(input, too_large, cx));
    }

    /// The editor scrolled: the preview follows (one way). `line` is 1-based.
    pub(super) fn sync_live_scroll(&mut self, editor: PaneId, line: usize, cx: &mut Context<Self>) {
        let Some(preview) = self.live_pairs.preview_of(editor) else { return };
        if let Some(PaneView::Preview(pv)) = self.panes.get(&preview).cloned() {
            pv.update(cx, |v, cx| v.scroll_to_source_line(line as u32, cx));
        }
    }

    /// Pane `id` is being closed: closes the other half of its pair, or clears the editor's preview state.
    pub(super) fn live_pair_closing(&mut self, id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        self.live_debounce.remove(&id);
        if let Some(preview) = self.live_pairs.remove_editor(id) {
            // The editor closes: its preview goes with it (the mapping is gone, so this does not come back).
            self.close_pane(preview, window, cx);
        }
        if let Some(editor) = self.live_pairs.remove_preview(id) {
            // The preview closes alone: the editor goes back to 「预览 ⌘⇧V」.
            self.live_debounce.remove(&editor);
            if let Some(PaneView::Editor(v)) = self.panes.get(&editor) {
                v.update(cx, |v, cx| {
                    v.live = None;
                    cx.notify();
                });
            }
        }
    }
}
