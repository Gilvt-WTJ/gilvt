//! The 会话 palette (⌘⇧R) in a window: opening it over the pane area, where running sessions' panes are, and
//! acting on its events (resume via `run_command`, go to a pane). The palette itself is `launcher::sessions_view`.

use std::collections::HashMap;

use gilvt_agent::SessionKey;
use gpui::{AnyWindowHandle, App, AppContext, Context, Window};

use super::{focus_pane_anywhere, workspaces, Workspace};
use crate::agents::Agents;
use crate::launcher::sessions_model::{location_word, LiveAt, PaneAt};
use crate::launcher::sessions_view::SessionsEvent;
use crate::launcher::History;
use crate::pane_tree::PaneId;
use crate::session_center::view::SessionCenterView;

impl Workspace {
    /// ⌘⇧R: opens the palette (closing the ⌘P palette, the ⌘⇧N panel and Quick Look) and rescans the history; closes it when open.
    pub(super) fn toggle_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sessions.take().is_some() {
            self.focus_active(window, cx);
            return;
        }
        self.finder = None;
        self.quicklook = None;
        self.new_agent = None;
        self.ended_confirm = None;
        self.session_menu = None;
        self.renaming = None;
        History::refresh(cx);
        let cwd = self.focused_cwd(cx);
        let me = cx.entity().downgrade();
        let view = cx.new(|cx| SessionCenterView::new(cwd, me, window, cx));
        let sub = cx.subscribe_in(&view, window, |ws, _, event, window, cx| {
            ws.sessions = None;
            match event {
                SessionsEvent::Run { location, dir, command } => ws.run_command(*location, dir.clone(), command.clone(), window, cx),
                SessionsEvent::Focus(pane) if ws.has_pane(*pane) => ws.focus_pane(*pane, window, cx),
                SessionsEvent::Focus(pane) => {
                    ws.focus_active(window, cx);
                    let pane = *pane;
                    App::defer(cx, move |cx| focus_pane_anywhere(pane, cx));
                }
                SessionsEvent::Navigate(runtime) => {
                    match crate::external_navigation::navigate(runtime) {
                        Ok(()) => ws.error = None,
                        Err(error) => {
                            let tty = runtime.tty.as_deref().map_or("unknown".into(), |tty| tty.display().to_string());
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(format!("pid={}\ntty={tty}", runtime.pid)));
                            ws.error = Some(format!("{error}；TTY/PID 已复制，请手动切换"));
                        }
                    }
                    ws.focus_active(window, cx);
                    cx.notify();
                }
                SessionsEvent::Close => ws.focus_active(window, cx),
                // Handled inside the Session Center (it never forwards them).
                SessionsEvent::SelectCenterTab(_) | SessionsEvent::OpenCleanup | SessionsEvent::CloseCleanup => {}
            }
        });
        self.sessions = Some((view, sub));
        self.focus_active(window, cx);
    }

    /// ⌘⇧K / 会话 menu 「清理…」: the cleanup wizard in the 会话 overlay (opening the overlay first).
    pub(super) fn open_cleanup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sessions.is_none() {
            self.toggle_sessions(window, cx);
        }
        if let Some((view, _)) = &self.sessions {
            view.update(cx, |view, cx| view.open_cleanup(window, cx));
        }
    }

    /// The sidebar's 待 Review entry: opens the Session Center on `tab` (or switches to it when already open).
    pub fn open_sessions_on(&mut self, tab: crate::session_center::model::Tab, window: &mut Window, cx: &mut Context<Self>) {
        if self.sessions.is_none() {
            self.toggle_sessions(window, cx);
        }
        if let Some((view, _)) = &self.sessions {
            view.update(cx, |view, cx| view.set_tab(tab, window, cx));
        }
    }

    /// Where `pane` is for the palette of the window showing it; `window`: its number when that is another one.
    fn pane_at(&self, pane: PaneId, window: Option<usize>, cx: &App) -> Option<PaneAt> {
        let loc = self.pane_location(pane, cx)?;
        Some(PaneAt { window, tab: loc.tab + 1, active_tab: loc.tab == self.active, position: loc.position })
    }
}

/// Running sessions and where their panes are, seen from window `me` (whose workspace is `ws`). Safe inside
/// `ws`'s own update or render: window `me` is never read by handle.
pub fn live_sessions(ws: &Workspace, me: AnyWindowHandle, cx: &App) -> HashMap<SessionKey, LiveAt> {
    let Some(agents) = cx.try_global::<Agents>() else { return HashMap::new() };
    let others: Vec<(usize, &Workspace)> = workspaces(cx)
        .into_iter()
        .enumerate()
        .filter(|(_, w)| AnyWindowHandle::from(*w) != me)
        .filter_map(|(i, w)| Some((i, w.read(cx).ok()?)))
        .collect();
    agents
        .registry()
        .sessions()
        .filter(|s| s.is_live())
        .map(|s| {
            let found = s.pane.and_then(|p| {
                let at = if ws.has_pane(p) {
                    ws.pane_at(p, None, cx)
                } else {
                    others.iter().find(|(_, o)| o.has_pane(p)).and_then(|(i, o)| o.pane_at(p, Some(i + 1), cx))
                };
                Some((p, at?))
            });
            let live = match found {
                Some((pane, at)) => LiveAt { pane: Some(pane), location: location_word(&at), runtime: None },
                None => {
                    let runtime = agents.runtime(&s.key).cloned();
                    let location = runtime.as_ref().map(gilvt_agent::RuntimeRef::location_label).unwrap_or_default();
                    LiveAt { pane: None, location, runtime }
                }
            };
            (s.key.clone(), live)
        })
        .collect()
}
