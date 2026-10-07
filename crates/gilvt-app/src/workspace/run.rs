//! Running a launcher command (M3c §2.1): typed into the focused pane when it is an idle shell, else into a
//! new tab or split whose shell starts in the command's directory. The decision is `launcher::place`.

use std::path::PathBuf;

use gpui::{App, Context, Window};

use super::{PaneView, Workspace};
use crate::agents::Agents;
use crate::launch::{marks_expected, ShellEnv};
use crate::launcher::place::{place, prompt_wait};
use crate::launcher::{Location, Placement};
use crate::theme::AppSettings;

impl Workspace {
    /// Where `location` would run a command now (the ⌘⇧N preview names it with [`Placement::word`]).
    pub fn placement(&self, location: Location, cx: &App) -> Placement {
        let agents = cx.try_global::<Agents>();
        let idle = |p| match self.panes.get(&p) {
            Some(PaneView::Terminal(t)) => !t.read(cx).launch_pending() && agents.is_some_and(|a| a.pane_is_idle_shell(p)),
            _ => false,
        };
        place(location, self.focused_pane(), idle)
    }

    /// Types `command` (already `cd <dir> && …`) plus Return at `location`. A new pane's shell starts in
    /// `dir` and gets the command at its first prompt (or after the wait, see `launcher::place`). Only for an
    /// explicit user action; the caller checks that `dir` exists.
    pub fn run_command(&mut self, location: Location, dir: PathBuf, command: String, window: &mut Window, cx: &mut Context<Self>) {
        let pane = match self.placement(location, cx) {
            Placement::InPlace(pane) => {
                if let Some(PaneView::Terminal(t)) = self.panes.get(&pane) {
                    t.update(cx, |t, cx| t.type_command(command, None, cx));
                }
                Agents::typed(pane, cx);
                self.focus_active(window, cx);
                return;
            }
            Placement::NewTab => self.open_tab(Some(dir), window, cx),
            Placement::Split(axis) => self.split_in(axis, Some(dir), window, cx),
        };
        // Spawning failed: the error banner says why.
        let Some(pane) = pane else { return };
        let wait = prompt_wait(marks_expected(&cx.global::<AppSettings>().0, cx.global::<ShellEnv>()));
        if let Some(PaneView::Terminal(t)) = self.panes.get(&pane) {
            t.update(cx, |t, cx| t.type_command(command, Some(wait), cx));
        }
        // Busy from queue time: now, and again when the wait ends (the command is typed by then, so the hold
        // covers the agent's start-up too).
        Agents::typed(pane, cx);
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(wait).await;
            let _ = cx.update(|cx| Agents::typed(pane, cx));
        })
        .detach();
    }
}
