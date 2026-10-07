//! The 「◎ 监控官」 tab: opening it (⌘⇧O), its per-frame model, the jump-back actions of its cards.

use std::rc::Rc;
use std::time::Duration;

use gpui::{App, AppContext as _, Context, Focusable as _, Window};

use super::{PaneView, Tab, Workspace};
use crate::agents::Agents;
use crate::inspector::InspectorTab;
use crate::monitor::chat_input::{ChatInput, ChatInputEvent};
use crate::monitor::model::{step_selection, MonitorModel};
use crate::monitor::{Filter, MonitorPane, MonitorUi};
use crate::pane_tree::{next_pane_id, PaneId, PaneTree};

/// Redraw interval while cards are drawn (clocks and relative times).
const TICK: Duration = Duration::from_secs(1);

impl Workspace {
    /// This window's monitor pane.
    pub(crate) fn monitor_pane(&self) -> Option<PaneId> {
        self.panes.iter().find_map(|(id, p)| matches!(p, PaneView::Monitor(_)).then_some(*id))
    }

    pub(crate) fn monitor_ui_mut(&mut self, pane: PaneId) -> Option<&mut MonitorUi> {
        match self.panes.get_mut(&pane)? {
            PaneView::Monitor(m) => Some(&mut m.ui),
            _ => None,
        }
    }

    /// This frame's monitor model (None when no monitor pane is drawn).
    pub fn monitor_model(&self) -> Option<&MonitorModel> {
        self.monitor_frame.as_deref()
    }

    /// The 需要你 count of the monitor tab's title: this frame's model when the wall is drawn, else counted from
    /// the registry (a tab in the background has no model and needs the count most).
    pub(super) fn monitor_needs_you(&self, cx: &App) -> usize {
        match self.monitor_model() {
            Some(m) => m.needs_you(),
            None => crate::monitor::model::needs_you_count(cx.global::<Agents>().registry().sessions()),
        }
    }

    /// A tab's title as the tab bar draws it (DebugState exports the same): the monitor's tab counts the
    /// sessions waiting for the user.
    pub(super) fn tab_bar_title(&self, tab: &Tab, cx: &App) -> String {
        let title = self.tab_title(tab, cx);
        if tab.tree.panes().iter().any(|p| matches!(self.panes.get(p), Some(PaneView::Monitor(_)))) {
            crate::monitor::model::tab_title_with_needs_you(title, self.monitor_needs_you(cx))
        } else {
            title
        }
    }

    /// ⌘⇧O: focuses this window's monitor pane, else opens one in a new leftmost tab.
    pub fn open_monitor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.monitor_pane() {
            self.focus_pane(pane, window, cx);
            // `width` only updates while the tab is drawn: the pane area may have changed since (⌘I in another
            // tab). Filling the pane area (alone in its tab, or zoomed), it starts from that width as a new tab
            // does. In a split tab only the measure knows the pane's width: left as it is.
            let fills = self.tabs.iter().find(|t| t.tree.contains(pane)).is_some_and(|t| t.tree.panes().len() == 1 || (t.zoomed && t.focused == pane));
            if let (true, Some(bounds), Some(PaneView::Monitor(m))) = (fills, self.content_bounds.get(), self.panes.get(&pane)) {
                m.width.set(f32::from(bounds.size.width));
            }
            return;
        }
        let id = next_pane_id();
        let chat_input = cx.new(|cx| ChatInput::new(window, cx));
        self.monitor_chat_sub = Some(cx.subscribe_in(&chat_input, window, move |ws, _, ev: &ChatInputEvent, window, cx| match ev {
            ChatInputEvent::Send(out) => {
                let out = out.clone();
                App::defer(cx, move |cx| crate::monitor::chat::send(out, cx));
            }
            ChatInputEvent::Escape => ws.focus_pane(id, window, cx),
        }));
        let pane = MonitorPane {
            focus: cx.focus_handle(),
            ui: MonitorUi::default(),
            chat_input,
            // The new tab is one pane filling the pane area: starting from its width, the first frame already draws
            // the right mode, and a narrow tab is not taken for a wide one that narrowed (which closes the chat
            // overlay that 「在监控官中查看 ↗」 opens and moves the keyboard to the wall).
            width: Rc::new(std::cell::Cell::new(self.content_bounds.get().map_or(0.0, |b| f32::from(b.size.width)))),
            chat_scroll: gpui::ScrollHandle::new(),
            chat_seen: Default::default(),
            chat_questions: Default::default(),
        };
        self.panes.insert(id, PaneView::Monitor(pane));
        self.tabs.insert(0, Tab::new(PaneTree::new(id), id));
        self.active = 0;
        self.focus_active(window, cx);
    }

    /// Computes the monitor model once per frame when a monitor pane is drawn; returns whether it needs the
    /// per-second redraw (`MonitorModel::ticking`).
    pub(super) fn prepare_monitor_frame(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        let shown = self.monitor_pane().filter(|p| self.pane_shown(*p));
        self.monitor_frame = shown.and_then(|p| match self.panes.get(&p) {
            Some(PaneView::Monitor(m)) => Some(Rc::new(crate::monitor::gather::model_for(self, window.window_handle(), &m.ui, cx))),
            _ => None,
        });
        if let (Some(p), Some(model)) = (shown, self.monitor_frame.clone()) {
            if let Some(ui) = self.monitor_ui_mut(p) {
                ui.settle(&model);
            }
            if let Some(PaneView::Monitor(m)) = self.panes.get(&p) {
                let candidates = crate::monitor::chat_view::candidates(&model);
                m.chat_input.update(cx, |input, _| input.set_candidates(candidates));
            }
        }
        self.monitor_frame.as_ref().is_some_and(|m| m.ticking())
    }

    pub(super) fn sync_monitor_ticker(&mut self, tick: bool, cx: &mut Context<Self>) {
        match (tick, self.monitor_tick.is_some()) {
            (true, false) => {
                self.monitor_tick = Some(cx.spawn(async move |ws, cx| loop {
                    cx.background_executor().timer(TICK).await;
                    if ws.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }));
            }
            (false, true) => self.monitor_tick = None,
            _ => {}
        }
    }

    pub(crate) fn monitor_set_filter(&mut self, pane: PaneId, filter: Filter, cx: &mut Context<Self>) {
        if let Some(ui) = self.monitor_ui_mut(pane) {
            ui.filter = if ui.filter == filter { Filter::All } else { filter };
            cx.notify();
        }
    }

    pub(crate) fn monitor_select(&mut self, pane: PaneId, key: String, cx: &mut Context<Self>) {
        if let Some(ui) = self.monitor_ui_mut(pane) {
            ui.selected = Some(key);
            cx.notify();
        }
    }

    pub(crate) fn monitor_toggle_catchup(&mut self, pane: PaneId, key: String, cx: &mut Context<Self>) {
        if let Some(ui) = self.monitor_ui_mut(pane) {
            ui.expanded = if ui.expanded.as_deref() == Some(key.as_str()) { None } else { Some(key.clone()) };
            ui.selected = Some(key);
            cx.notify();
        }
    }

    pub(crate) fn monitor_toggle_ended(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        if let Some(ui) = self.monitor_ui_mut(pane) {
            ui.ended_open = !ui.ended_open;
            cx.notify();
        }
    }

    /// Arrow keys move the selection along the drawn cards, ⏎ jumps to the selected card's pane, Space toggles
    /// its 补课, `s` asks for a new ✦ summary. Returns whether the key was handled.
    pub(crate) fn monitor_key(&mut self, pane: PaneId, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(model) = self.monitor_frame.clone() else { return false };
        let Some(ui) = self.monitor_ui_mut(pane) else { return false };
        match key {
            "down" | "right" | "up" | "left" => match step_selection(&model.visible_keys(), ui.selected.as_deref(), key) {
                Some(next) => ui.selected = Some(next),
                None => return false,
            },
            "space" => match ui.selected.clone() {
                Some(k) => ui.expanded = if ui.expanded.as_ref() == Some(&k) { None } else { Some(k) },
                None => return false,
            },
            "enter" => {
                let selected = ui.selected.as_deref();
                let target = model.groups.iter().flat_map(|g| &g.cards).find(|c| Some(c.key().as_str()) == selected).and_then(|c| c.pane());
                let Some(target) = target else { return false };
                App::defer(cx, move |cx| monitor_jump(target, None, None, cx));
            }
            "s" => {
                let Some(key) = ui.selected.clone() else { return false };
                let has = model.groups.iter().flat_map(|g| &g.cards).any(|c| c.key() == key && c.summary().is_some_and(|s| s.actionable));
                if !has {
                    return false;
                }
                App::defer(cx, move |cx| crate::monitor::summaries::request(key, cx));
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// 「◎ 问它」 / `a`: the session becomes a chip in the chat's input, which takes the keyboard (S2 §6.3).
    pub(crate) fn monitor_ask(&mut self, pane: PaneId, key: String, window: &mut Window, cx: &mut Context<Self>) {
        if !cx.global::<crate::theme::AppSettings>().0.monitor.enabled {
            return;
        }
        let Some(model) = self.monitor_frame.clone() else { return };
        let Some(card) = model.groups.iter().flat_map(|g| &g.cards).find(|c| c.key() == key && !c.excluded()) else { return };
        let label = card.name().to_string();
        let Some(PaneView::Monitor(m)) = self.panes.get_mut(&pane) else { return };
        m.ui.selected = Some(key.clone());
        m.ui.chat_collapsed = false;
        m.ui.chat_open = true;
        let input = m.chat_input.clone();
        input.update(cx, |i, cx| i.add_chip(key, label, cx));
        window.focus(&input.focus_handle(cx));
        cx.notify();
    }

    /// `a` on the wall: ask about the selected card.
    pub(crate) fn monitor_ask_selected(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.monitor_ui_mut(pane).and_then(|ui| ui.selected.clone()) else { return false };
        let askable = self.monitor_frame.as_ref().is_some_and(|m| m.groups.iter().flat_map(|g| &g.cards).any(|c| c.key() == key && !c.excluded()));
        if !askable || !cx.global::<crate::theme::AppSettings>().0.monitor.enabled {
            return false;
        }
        self.monitor_ask(pane, key, window, cx);
        true
    }

    /// A `gilvt://session/<key>` link in an answer: like a click on the sidebar row (focus its pane); a session
    /// without a pane is selected on this wall instead (已结束 opens).
    pub(crate) fn monitor_open_link(&mut self, pane: PaneId, key: String, cx: &mut Context<Self>) {
        let target = self.monitor_frame.as_ref().and_then(|m| crate::monitor::chat_view::link_target(m, &key));
        match target {
            Some(Some(p)) => App::defer(cx, move |cx| crate::workspace::focus_pane_anywhere(p, cx)),
            Some(None) => {
                if let Some(ui) = self.monitor_ui_mut(pane) {
                    ui.selected = Some(key);
                    ui.ended_open = true;
                    ui.filter = Filter::All;
                }
                cx.notify();
            }
            None => {}
        }
    }

    /// A click on the rail: the panel comes back (wide) or opens over the wall (narrow).
    pub(crate) fn monitor_chat_expand(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        if let Some(ui) = self.monitor_ui_mut(pane) {
            ui.chat_collapsed = false;
            ui.chat_open = true;
            cx.notify();
        }
    }

    /// ⇥: back to the rail.
    pub(crate) fn monitor_chat_collapse(&mut self, pane: PaneId, narrow: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ui) = self.monitor_ui_mut(pane) {
            if narrow {
                ui.chat_open = false;
            } else {
                ui.chat_collapsed = true;
            }
            self.monitor_chat_hidden(pane, window, cx);
            cx.notify();
        }
    }

    /// The tab's width crossed `chat_view::NARROW_WIDTH` (measured after the frame was drawn). Narrowing closes
    /// the overlay, so it never opens by itself (the rail is clicked to open it), and the panel is no longer drawn.
    pub(crate) fn monitor_chat_width_crossed(&mut self, pane: PaneId, narrow: bool, window: &mut Window, cx: &mut Context<Self>) {
        if narrow {
            if let Some(ui) = self.monitor_ui_mut(pane) {
                ui.chat_open = false;
            }
            self.monitor_chat_hidden(pane, window, cx);
        }
        cx.notify();
    }

    /// The chat panel stops being drawn: if its input had the keyboard, the wall takes it back (Esc, arrows).
    /// Focus elsewhere (another pane) is left alone.
    fn monitor_chat_hidden(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(PaneView::Monitor(m)) = self.panes.get(&pane) else { return };
        if m.chat_input.focus_handle(cx).contains_focused(window, cx) {
            window.focus(&m.focus);
        }
    }
}

/// Goes to `pane` in whatever window owns it; `turn` also shows 「过程」 with that turn expanded; `line` scrolls
/// the terminal to that absolute line (only the view scrolls; nothing is written to the PTY). Call it outside
/// window updates (e.g. via `cx.defer`).
pub fn monitor_jump(pane: PaneId, turn: Option<u32>, line: Option<u64>, cx: &mut App) {
    crate::workspace::focus_pane_anywhere(pane, cx);
    let Some(w) = crate::workspace::workspaces(cx).into_iter().find(|w| w.read(cx).is_ok_and(|ws| ws.has_pane(pane))) else { return };
    let session = turn.and_then(|_| crate::inspector::model::follow(cx.global::<Agents>().registry().sessions(), pane).map(|s| s.key.clone()));
    let _ = w.update(cx, |ws, window, cx| {
        if let Some(turn) = turn {
            ws.choose_inspector_tab(InspectorTab::Process, window, cx);
            let timeline = &mut ws.inspector.timeline;
            // The inspector follows the focused pane's session from the next frame on: switch the list now
            // so that frame's sync keeps the expansion.
            if let Some(key) = &session {
                timeline.switch_to(key);
            }
            timeline.expand_turn(turn);
        }
        if let (Some(line), Some(t)) = (line, ws.terminal_view(pane).cloned()) {
            t.update(cx, |t, cx| {
                let _ = t.jump_to_line(line, cx);
            });
        }
        cx.notify();
    });
}
