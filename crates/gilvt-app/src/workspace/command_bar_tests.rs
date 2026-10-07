//! The command bar's focus and tab moves on a window without terminals (a monitor tab only: no PTY, no model calls).
//! The window's root view is a stub, so nothing is drawn: blur (which needs a drawn focus path) is covered by R9.

use gpui::{div, prelude::*, px, Bounds, Entity, Focusable, TestAppContext, Window, WindowHandle};

use super::super::{PaneView, Workspace};
use crate::monitor::command_bar::BarEvent;
use crate::monitor::model::{MonitorModel, WallEntry};
use crate::pane_tree::PaneId;
use crate::settings::Settings;
use crate::theme::AppSettings;

struct Host {
    ws: Entity<Workspace>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
    }
}

fn set_enabled(cx: &mut TestAppContext, on: bool) {
    cx.update(|cx| {
        let mut s = Settings::default();
        s.monitor.enabled = on;
        cx.set_global(AppSettings(s));
        crate::theme::init_for_tests(cx);
    });
}

/// A window whose only tab is the 监控官 (its wall has the keyboard).
fn window(cx: &mut TestAppContext, width: Option<f32>) -> WindowHandle<Host> {
    set_enabled(cx, true);
    cx.add_window(|window, cx| {
        let ws = cx.new(|cx| {
            let mut ws = Workspace::blank(None, window, cx);
            if let Some(w) = width {
                ws.content_bounds.set(Some(Bounds::new(Default::default(), gpui::size(px(w), px(500.)))));
            }
            ws.open_monitor(window, cx);
            ws
        });
        Host { ws }
    })
}

fn with<R>(cx: &mut TestAppContext, w: WindowHandle<Host>, f: impl FnOnce(&mut Workspace, &mut Window, &mut gpui::Context<Workspace>) -> R) -> R {
    w.update(cx, |host, window, cx| host.ws.clone().update(cx, |ws, cx| f(ws, window, cx))).unwrap()
}

fn monitor(ws: &Workspace) -> (PaneId, &crate::monitor::MonitorPane) {
    let id = ws.monitor_pane().unwrap();
    match ws.panes.get(&id) {
        Some(PaneView::Monitor(m)) => (id, m),
        _ => unreachable!(),
    }
}

fn wall_focused(ws: &Workspace, window: &Window) -> bool {
    monitor(ws).1.focus.is_focused(window)
}

#[gpui::test]
fn toggle_opens_and_escape_gives_the_keyboard_back(cx: &mut TestAppContext) {
    let w = window(cx, None);
    with(cx, w, |ws, window, cx| {
        assert!(wall_focused(ws, window));
        ws.command_bar_event(BarEvent::Toggle, window, cx);
        assert!(ws.command_bar.expanded && ws.command_bar_input_focused(window, cx));
        ws.command_bar_event(BarEvent::Escape, window, cx);
        assert!(!ws.command_bar.expanded);
        assert!(wall_focused(ws, window), "the keyboard is back on the wall");
        ws.command_bar_event(BarEvent::Toggle, window, cx);
        ws.command_bar_event(BarEvent::Toggle, window, cx);
        assert!(!ws.command_bar.expanded && wall_focused(ws, window), "⌘⇧M closes it too");
    });
}

#[gpui::test]
fn the_chat_panel_input_gets_the_keyboard_back(cx: &mut TestAppContext) {
    let w = window(cx, None);
    with(cx, w, |ws, window, cx| {
        let input = monitor(ws).1.chat_input.focus_handle(cx);
        window.focus(&input);
        ws.command_bar_event(BarEvent::Toggle, window, cx);
        assert!(ws.command_bar_input_focused(window, cx));
        ws.command_bar_event(BarEvent::Escape, window, cx);
        assert!(input.is_focused(window), "not the wall: where it was");
    });
}

#[gpui::test]
fn off_does_nothing_and_turning_off_closes(cx: &mut TestAppContext) {
    let w = window(cx, None);
    set_enabled(cx, false);
    with(cx, w, |ws, window, cx| {
        ws.command_bar_event(BarEvent::Toggle, window, cx);
        assert!(!ws.command_bar.expanded && wall_focused(ws, window));
    });
    set_enabled(cx, true);
    with(cx, w, |ws, window, cx| ws.command_bar_event(BarEvent::Toggle, window, cx));
    set_enabled(cx, false);
    with(cx, w, |ws, window, cx| {
        ws.command_bar_event(BarEvent::Frame, window, cx);
        assert!(!ws.command_bar.expanded && wall_focused(ws, window), "had the keyboard: the pane gets it");
    });
}

#[gpui::test]
fn cmd_w_closes_the_bar_not_the_pane(cx: &mut TestAppContext) {
    let w = window(cx, None);
    with(cx, w, |ws, window, cx| {
        ws.command_bar_event(BarEvent::Toggle, window, cx);
        ws.close_pane_action(window, cx);
        assert!(!ws.command_bar.expanded);
        assert!(ws.monitor_pane().is_some() && wall_focused(ws, window), "the pane under it stays");
    });
}

#[gpui::test]
fn only_live_links_go_somewhere(cx: &mut TestAppContext) {
    let w = window(cx, None);
    with(cx, w, |ws, window, cx| {
        ws.command_bar_event(BarEvent::Toggle, window, cx);
        let entry = |key: &str| WallEntry { key: key.into(), pane: None, label: key.into() };
        // `excluded` is drawn as text (not askable); a click on it, were one to arrive, changes nothing.
        ws.command_bar.model = Some(std::rc::Rc::new(MonitorModel { askable: vec![entry("agent:claude:ended")], ..Default::default() }));
        ws.command_bar_open_link("agent:claude:excluded".into(), window, cx);
        assert!(ws.command_bar.expanded && ws.command_bar_input_focused(window, cx));
        ws.command_bar_open_link("agent:claude:ended".into(), window, cx);
        assert!(!ws.command_bar.expanded);
        let ui = &monitor(ws).1.ui;
        assert_eq!((ui.selected.as_deref(), ui.ended_open), (Some("agent:claude:ended"), true), "a pane-less session: its card");
        assert!(wall_focused(ws, window));
    });
}

#[gpui::test]
fn open_in_monitor_opens_the_chat_and_focuses_its_input(cx: &mut TestAppContext) {
    let w = window(cx, None);
    with(cx, w, |ws, window, cx| {
        let (pane, _) = monitor(ws);
        if let Some(ui) = ws.monitor_ui_mut(pane) {
            ui.chat_collapsed = true;
        }
        ws.command_bar_event(BarEvent::Toggle, window, cx);
        ws.command_bar_open_monitor(window, cx);
        let (_, m) = monitor(ws);
        assert!(!ws.command_bar.expanded && !m.ui.chat_collapsed && m.ui.chat_open);
        assert!(m.chat_input.focus_handle(cx).is_focused(window));
    });
}

#[gpui::test]
fn a_new_monitor_tab_starts_at_the_pane_area_width(cx: &mut TestAppContext) {
    let w = window(cx, Some(700.));
    let width: f32 = with(cx, w, |ws, _, _| monitor(ws).1.width.get());
    assert_eq!(width, 700.0, "narrow from the first frame: no wide→narrow crossing closes the chat");
    let unmeasured = window(cx, None);
    assert_eq!(with(cx, unmeasured, |ws, _, _| monitor(ws).1.width.get()), 0.0);
}

#[gpui::test]
fn an_existing_monitor_tab_is_reseeded_to_the_pane_area_width(cx: &mut TestAppContext) {
    use crate::monitor::chat_view::{is_narrow, panel_mode, PanelMode};
    // Opened wide (inspector hidden); then, with a terminal tab active, ⌘I narrows the pane area.
    let w = window(cx, Some(1000.));
    with(cx, w, |ws, window, cx| {
        let other = plain_pane(ws, None, window, cx);
        ws.activate(1, window, cx);
        assert_eq!(ws.focused_pane(), Some(other));
        ws.content_bounds.set(Some(Bounds::new(Default::default(), gpui::size(px(700.), px(500.)))));
        ws.command_bar_event(BarEvent::Toggle, window, cx);
        ws.command_bar_open_monitor(window, cx);
        let (_, m) = monitor(ws);
        assert_eq!(m.width.get(), 700.0, "alone in its tab: the stale stored width is replaced");
        // The first frame's measure sees 700 as well: no wide→narrow crossing closes the overlay.
        assert_eq!(is_narrow(m.width.get()), is_narrow(700.));
        assert!(m.ui.chat_open);
        assert_eq!(panel_mode(m.width.get(), m.ui.chat_collapsed, m.ui.chat_open), PanelMode::Overlay);
        assert!(m.chat_input.focus_handle(cx).is_focused(window), "the keyboard is in the chat input");
    });
}

#[gpui::test]
fn the_bar_opens_over_any_other_pane(cx: &mut TestAppContext) {
    // Neither a terminal nor the 监控官 has the keyboard: the OS composition is dropped unconditionally (no-op
    // without an input context) and the bar takes the keyboard; Esc gives it back.
    let w = window(cx, None);
    with(cx, w, |ws, window, cx| {
        let other = plain_pane(ws, None, window, cx);
        ws.activate(1, window, cx);
        ws.command_bar_event(BarEvent::Toggle, window, cx);
        assert!(ws.command_bar.expanded && ws.command_bar_input_focused(window, cx));
        ws.command_bar_event(BarEvent::Escape, window, cx);
        assert_eq!(ws.focused_pane(), Some(other));
        assert!(!ws.command_bar_input_focused(window, cx));
    });
}

#[gpui::test]
fn a_split_monitor_tab_keeps_its_measured_width_unless_zoomed(cx: &mut TestAppContext) {
    let w = window(cx, Some(1000.));
    with(cx, w, |ws, window, cx| {
        plain_pane(ws, Some(0), window, cx);
        monitor(ws).1.width.set(480.);
        ws.content_bounds.set(Some(Bounds::new(Default::default(), gpui::size(px(1200.), px(500.)))));
        ws.open_monitor(window, cx);
        assert_eq!(monitor(ws).1.width.get(), 480.0, "split: only the measure knows the pane's width");
        ws.tabs[0].zoomed = true;
        ws.open_monitor(window, cx);
        assert_eq!(monitor(ws).1.width.get(), 1200.0, "zoomed: it fills the pane area");
    });
}

/// A pane without a process (another wall view): split into tab `split_in`, or alone in a new last tab.
fn extra_pane(ws: &mut Workspace, split_in: Option<usize>, window: &mut Window, cx: &mut gpui::Context<Workspace>) -> PaneId {
    let id = crate::pane_tree::next_pane_id();
    let pane = crate::monitor::MonitorPane {
        focus: cx.focus_handle(),
        ui: Default::default(),
        chat_input: cx.new(|cx| crate::monitor::chat_input::ChatInput::new(window, cx)),
        width: Default::default(),
        chat_scroll: gpui::ScrollHandle::new(),
        chat_seen: Default::default(),
        chat_questions: Default::default(),
    };
    ws.panes.insert(id, PaneView::Monitor(pane));
    match split_in {
        Some(ti) => {
            let target = ws.tabs[ti].focused;
            ws.tabs[ti].tree.split(target, id, crate::pane_tree::Axis::Row);
        }
        None => ws.tabs.push(super::super::Tab::new(crate::pane_tree::PaneTree::new(id), id)),
    }
    id
}

/// A pane that is not the 监控官 (an empty preview: no file, no process), placed as `extra_pane` does: keeps
/// `monitor_pane()` unambiguous.
fn plain_pane(ws: &mut Workspace, split_in: Option<usize>, window: &mut Window, cx: &mut gpui::Context<Workspace>) -> PaneId {
    let id = crate::pane_tree::next_pane_id();
    let view = cx.new(|cx| crate::preview_view::PreviewView::new(crate::preview_view::OpenRequest::files(Vec::new()), false, window, cx));
    ws.panes.insert(id, PaneView::Preview(view));
    match split_in {
        Some(ti) => {
            let target = ws.tabs[ti].focused;
            ws.tabs[ti].tree.split(target, id, crate::pane_tree::Axis::Row);
        }
        None => ws.tabs.push(super::super::Tab::new(crate::pane_tree::PaneTree::new(id), id)),
    }
    id
}

fn pane_focused(ws: &Workspace, id: PaneId, window: &Window) -> bool {
    matches!(ws.panes.get(&id), Some(PaneView::Monitor(m)) if m.focus.is_focused(window))
}

#[gpui::test]
fn a_pane_closing_by_itself_leaves_the_keyboard_in_the_bar(cx: &mut TestAppContext) {
    let w = window(cx, None);
    with(cx, w, |ws, window, cx| {
        let beside = extra_pane(ws, Some(0), window, cx);
        let background = extra_pane(ws, None, window, cx);
        ws.command_bar_event(BarEvent::Toggle, window, cx);
        // The tab's focused pane goes (an exited shell, an editor asking to close): Tab.focused moves on, the bar
        // keeps the keyboard.
        let focused = ws.tabs[0].focused;
        ws.close_pane(focused, window, cx);
        assert_eq!(ws.tabs[0].focused, beside);
        assert!(ws.command_bar.expanded && ws.command_bar_input_focused(window, cx), "typing must not go into a pane");
        // A shell in a background tab exits (its tab goes).
        ws.close_pane(background, window, cx);
        assert_eq!(ws.tabs.len(), 1);
        assert!(ws.command_bar.expanded && ws.command_bar_input_focused(window, cx));
        // Esc: the pane that took the tab's focus gets the keyboard.
        ws.command_bar_event(BarEvent::Escape, window, cx);
        assert!(pane_focused(ws, beside, window));
    });
}

#[gpui::test]
fn a_tab_switch_still_takes_the_keyboard(cx: &mut TestAppContext) {
    let w = window(cx, None);
    with(cx, w, |ws, window, cx| {
        let other = extra_pane(ws, None, window, cx);
        ws.command_bar_event(BarEvent::Toggle, window, cx);
        ws.activate(1, window, cx);
        assert!(pane_focused(ws, other, window), "⌘2 moves the keyboard (the bar's blur then collapses it)");
    });
}

#[gpui::test]
fn prepare_keeps_the_wall_while_open(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler;
    let w = window(cx, None);
    with(cx, w, |ws, window, cx| {
        let wall = std::rc::Rc::new(MonitorModel { askable: vec![WallEntry { key: "pane:9".into(), pane: None, label: "zsh".into() }], ..Default::default() });
        // This window draws its wall (set by `prepare_monitor_frame` each frame): the bar uses that model.
        ws.monitor_frame = Some(wall.clone());
        ws.prepare_command_bar(window, cx);
        assert!(ws.command_bar.model.is_none(), "closed: no model");
        ws.command_bar_event(BarEvent::Toggle, window, cx);
        ws.prepare_command_bar(window, cx);
        assert!(ws.command_bar.model.as_ref().is_some_and(|m| std::rc::Rc::ptr_eq(m, &wall)));
        let input = ws.command_bar.input.clone();
        input.update(cx, |i, cx| i.replace_text_in_range(None, "@", window, cx));
        assert!(input.read(cx).draft().picker().is_some());
        assert!(input.read(cx).matches().is_empty(), "no candidates before a frame with the picker open");
        ws.prepare_command_bar(window, cx);
        assert_eq!(input.read(cx).matches().iter().map(|c| c.key.as_str()).collect::<Vec<_>>(), ["pane:9"]);
        ws.command_bar_event(BarEvent::Escape, window, cx);
        ws.prepare_command_bar(window, cx);
        assert!(ws.command_bar.model.is_none(), "dropped once closed");
    });
}

#[gpui::test]
fn debug_state_is_null_while_the_monitor_is_off(cx: &mut TestAppContext) {
    let w = window(cx, None);
    set_enabled(cx, false);
    with(cx, w, |ws, _, cx| assert!(ws.debug_command_bar(false, &|_| None, cx).is_none(), "off: null"));
}
