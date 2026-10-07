//! The bottom command bar's state changes (S2 §6.4): ⌘⇧M, Esc, ⌘W, losing the keyboard, 「在监控官中查看 ↗」 and
//! session links. What happens is decided by the pure `monitor::command_bar::step` and `chat_view::link_target`;
//! this file moves the keyboard and the tabs accordingly.

use std::rc::Rc;

use gpui::{App, Context, Focusable, Window};

use super::{PaneView, Workspace};
use crate::monitor::chat_input::discard_os_composition;
use crate::monitor::chat_view::{self, PanelMode};
use crate::monitor::command_bar::{step, BarEvent, Focus};
use crate::monitor::{Filter, MonitorUi};
use crate::theme::AppSettings;

impl Workspace {
    /// The bar's input has the keyboard (DebugState `command_bar.focused`).
    pub(crate) fn command_bar_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.command_bar.input.focus_handle(cx).is_focused(window)
    }

    pub(crate) fn command_bar_event(&mut self, ev: BarEvent, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = cx.global::<AppSettings>().0.monitor.enabled;
        let focused = self.command_bar_input_focused(window, cx);
        let was = self.command_bar.expanded;
        let s = step(was, enabled, focused, ev);
        if s.expanded == was && s.focus == Focus::Keep {
            return;
        }
        if !was && s.expanded {
            self.command_bar.return_to = window.focused(cx);
            self.drop_focused_composition(cx);
            self.command_bar.scroll.scroll_to_bottom();
        }
        if was && !s.expanded {
            // The draft stays for the next ⌘⇧M; the picker and an IME composition do not. The composition goes
            // while the bar still has the keyboard (the OS input context is the key window's).
            self.command_bar.input.update(cx, |input, cx| {
                input.cancel_composition(cx);
                input.close_picker(cx);
            });
        }
        self.command_bar.expanded = s.expanded;
        match s.focus {
            Focus::Input => window.focus(&self.command_bar.input.focus_handle(cx)),
            Focus::Back => self.command_bar_focus_back(window, cx),
            Focus::Keep => {}
        }
        if !s.expanded {
            self.command_bar.return_to = None;
        }
        cx.notify();
    }

    /// ⌘⇧M while a terminal (or the chat panel's input) is composing: the composition would stay underlined there
    /// and the IME would commit it into the bar. Dropped before the bar takes the keyboard.
    fn drop_focused_composition(&mut self, cx: &mut Context<Self>) {
        match self.focused_pane().and_then(|p| self.panes.get(&p)) {
            Some(PaneView::Terminal(t)) => t.update(cx, |t, cx| {
                if t.marked_text.take().is_some() {
                    discard_os_composition();
                    cx.notify();
                }
            }),
            Some(PaneView::Monitor(m)) => m.chat_input.update(cx, |input, cx| input.cancel_composition(cx)),
            // An editor pane, the sidebar's rename field, a palette: whatever is composing, the OS drops it.
            _ => discard_os_composition(),
        }
    }

    /// The keyboard goes back where it was: an open palette or the active tab's focused pane (the bar never changes
    /// `Tab.focused`), or the 监控官 tab's chat input when that had it and is still drawn.
    fn command_bar_focus_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let back = self.command_bar.return_to.take();
        self.focus_active(window, cx);
        let overlay = self.finder.is_some()
            || self.sessions.is_some()
            || self.new_agent.is_some()
            || self.inspector.config.disclosure.dialog.is_some()
            || self.quicklook.is_some();
        let (Some(back), Some(pane)) = (back, self.focused_pane()) else { return };
        if overlay || !cx.global::<AppSettings>().0.monitor.enabled {
            return;
        }
        if let Some(PaneView::Monitor(m)) = self.panes.get(&pane) {
            let input = m.chat_input.focus_handle(cx);
            let drawn = chat_view::panel_mode(m.width.get(), m.ui.chat_collapsed, m.ui.chat_open) != PanelMode::Rail;
            if input == back && drawn {
                window.focus(&input);
            }
        }
    }

    /// `focus_active` for changes the user did not ask for (a pane closing by itself): when the open bar has the
    /// keyboard it keeps it (the tab's focus and the window title still move on). Moves the user asked for (⌘1–9, a
    /// click in a pane, the sidebar) use `focus_active` / `focus_pane`, and the bar's blur closes it.
    pub(super) fn focus_active_keeping_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keep = self.command_bar.expanded && self.command_bar_input_focused(window, cx);
        self.focus_active(window, cx);
        if keep {
            // Back before the next frame: gpui compares focus paths per frame, so the bar never sees a blur.
            window.focus(&self.command_bar.input.focus_handle(cx));
        }
    }

    /// 「在监控官中查看 ↗」: this window's monitor tab (made when missing), its chat open and taking the keyboard.
    pub(crate) fn command_bar_open_monitor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command_bar_event(BarEvent::OpenMonitor, window, cx);
        self.open_monitor(window, cx);
        let Some(pane) = self.monitor_pane() else {
            // No monitor pane: the keyboard must not stay in the hidden bar's input.
            self.focus_active(window, cx);
            return;
        };
        self.monitor_chat_expand(pane, cx);
        if let Some(PaneView::Monitor(m)) = self.panes.get(&pane) {
            window.focus(&m.chat_input.focus_handle(cx));
        }
    }

    /// A `gilvt://session/<key>` link in the popup (Decision 12): like the panel's, its card's pane (in whatever
    /// window), else the card on this window's wall. Only cards of the wall that are not excluded are live
    /// (`chat_view::link_target`); others are plain text and never get here.
    pub(crate) fn command_bar_open_link(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.command_bar.model.as_ref().and_then(|m| chat_view::link_target(m, &key));
        let Some(target) = target else { return };
        self.command_bar_event(BarEvent::Link, window, cx);
        match target {
            Some(p) => App::defer(cx, move |cx| crate::workspace::focus_pane_anywhere(p, cx)),
            None => self.command_bar_show_card(key, window, cx),
        }
    }

    /// Every frame: an open bar closes when the 监控官 was turned off; while open it keeps this frame's wall (the
    /// `@` picker's candidates, live links).
    pub(super) fn prepare_command_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.command_bar.expanded {
            self.command_bar.model = None;
            return;
        }
        if !cx.global::<AppSettings>().0.monitor.enabled {
            // Moving the keyboard during render is not allowed: after this frame.
            cx.defer_in(window, |ws, window, cx| ws.command_bar_event(BarEvent::Frame, window, cx));
            return;
        }
        let model = match self.monitor_frame.clone() {
            Some(m) => m,
            None => Rc::new(crate::monitor::gather::model_for(self, window.window_handle(), &MonitorUi::default(), cx)),
        };
        if self.command_bar.input.read(cx).draft().picker().is_some() {
            let candidates = chat_view::candidates(&model);
            self.command_bar.input.update(cx, |input, _| input.set_candidates(candidates));
        }
        self.command_bar.model = Some(model);
    }

    /// A link to a session without a pane (已结束): its card on this window's wall, with 已结束 open.
    fn command_bar_show_card(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_monitor(window, cx);
        let Some(pane) = self.monitor_pane() else { return };
        if let Some(ui) = self.monitor_ui_mut(pane) {
            ui.selected = Some(key);
            ui.ended_open = true;
            ui.filter = Filter::All;
        }
        cx.notify();
    }

    /// ⌘W: an open bar collapses first (the pane under it stays); then the palettes, dialogs and Quick Look; then
    /// the focused pane.
    pub(super) fn close_pane_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command_bar.expanded {
            self.command_bar_event(BarEvent::Escape, window, cx);
        } else if self.close_confirm.is_some() {
            self.cancel_close(window, cx);
        } else if self.finder.is_some() || self.sessions.is_some() || self.new_agent.is_some() {
            self.finder = None;
            self.sessions = None;
            self.new_agent = None;
            self.focus_active(window, cx);
        } else if self.inspector.config.disclosure.dialog.is_some() {
            self.close_config_dialog(window, cx);
        } else if self.quicklook.is_some() {
            self.close_quicklook(window, cx);
        } else if let Some(id) = self.tabs.get(self.active).map(|t| t.focused) {
            self.close_focused_pane(id, window, cx);
        }
    }
}

#[cfg(test)]
#[path = "command_bar_tests.rs"]
mod tests;
