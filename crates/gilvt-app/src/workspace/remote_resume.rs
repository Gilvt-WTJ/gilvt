//! Resuming a remote history entry (spec R2a §5.6): in an idle pane of its host, else in a new tab that runs
//! `ssh <display>` first and types the command at the first remote prompt.

use std::time::{Duration, Instant};

use gpui::{Context, Window};

use super::{PaneView, Workspace};
use crate::agents::Agents;
use crate::launch::{marks_expected, ShellEnv};
use crate::launcher::place::prompt_wait;
use crate::remote::resume::{gave_up_text, resume_target, wait_step, PaneState, ResumeTarget, Wait};
use crate::theme::AppSettings;

impl Workspace {
    fn remote_pane_state(&self, pane: u64, cx: &gpui::App) -> Option<PaneState> {
        let Some(PaneView::Terminal(t)) = self.panes.get(&pane) else { return None };
        let t = t.read(cx);
        let remote = t.remote();
        let free = cx.global::<Agents>().registry().by_pane(pane).is_none();
        Some(PaneState { pane, host: remote.map(|r| r.host.clone()), enhanced: remote.is_some_and(|r| r.enhanced), idle: free && t.remote_idle() })
    }

    pub fn resume_remote(&mut self, host: String, command: String, window: &mut Window, cx: &mut Context<Self>) {
        let panes: Vec<PaneState> = self.panes.keys().copied().filter_map(|p| self.remote_pane_state(p, cx)).collect();
        match resume_target(&host, self.focused_pane(), &panes) {
            ResumeTarget::Pane(pane) => {
                if let Some(PaneView::Terminal(t)) = self.panes.get(&pane) {
                    t.update(cx, |t, cx| t.type_command(command, None, cx));
                }
                Agents::typed(pane, cx);
                self.focus_pane(pane, window, cx);
            }
            ResumeTarget::NewTab => {
                let display = crate::remote::display_of(&host, cx);
                let Some(pane) = self.open_tab(None, window, cx) else { return };
                let wait = prompt_wait(marks_expected(&cx.global::<AppSettings>().0, cx.global::<ShellEnv>()));
                if let Some(PaneView::Terminal(t)) = self.panes.get(&pane) {
                    t.update(cx, |t, cx| t.type_command(format!("ssh {}", gilvt_agent::shell_quote(&display)), Some(wait), cx));
                }
                Agents::typed(pane, cx);
                let started = Instant::now();
                // Ends when the command is typed, the wait runs out, the pane is closed or the window is gone.
                cx.spawn(async move |ws, cx| loop {
                    cx.background_executor().timer(Duration::from_millis(250)).await;
                    let done = ws.update(cx, |ws, cx| {
                        let state = ws.remote_pane_state(pane, cx);
                        match wait_step(&host, state.as_ref(), started, Instant::now()) {
                            Wait::Waiting => false,
                            Wait::Ready => {
                                if let Some(PaneView::Terminal(t)) = ws.panes.get(&pane) {
                                    t.update(cx, |t, cx| t.type_command(command.clone(), None, cx));
                                }
                                gpui::App::defer(cx, move |cx| Agents::typed(pane, cx));
                                true
                            }
                            // A closed tab is the user's own doing: nothing to report.
                            Wait::GaveUp => {
                                if state.is_some() {
                                    ws.error = Some(gave_up_text(&display));
                                    cx.notify();
                                }
                                true
                            }
                        }
                    });
                    if done.unwrap_or(true) {
                        break;
                    }
                })
                .detach();
            }
        }
        cx.notify();
    }
}
