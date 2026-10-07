//! The 新建 Agent panel (⌘⇧N) in a window: opening it over the pane area and running its command. The panel
//! itself is `launcher::new_agent_view`.

use gpui::{AppContext, Context, Window};

use super::Workspace;
use crate::launcher::new_agent_view::{NewAgentEvent, NewAgentView};

impl Workspace {
    /// ⌘⇧N: opens the panel in the focused pane's directory (closing the palettes and Quick Look); closes it
    /// when open.
    pub(super) fn toggle_new_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.new_agent.take().is_some() {
            self.focus_active(window, cx);
            return;
        }
        self.finder = None;
        self.sessions = None;
        self.quicklook = None;
        self.ended_confirm = None;
        self.session_menu = None;
        self.renaming = None;
        let cwd = self.focused_cwd(cx);
        let me = cx.entity().downgrade();
        let view = cx.new(|cx| NewAgentView::new(cwd, me, cx));
        let sub = cx.subscribe_in(&view, window, |ws, _, event, window, cx| {
            ws.new_agent = None;
            match event {
                NewAgentEvent::Run { location, dir, command } => ws.run_command(*location, dir.clone(), command.clone(), window, cx),
                NewAgentEvent::Close => ws.focus_active(window, cx),
            }
        });
        self.new_agent = Some((view, sub));
        self.focus_active(window, cx);
    }
}
