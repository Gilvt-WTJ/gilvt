//! Session switching (⌘⇧J, ⌘⇧↑ / ⌘⇧↓), the 「会话」 menu's actions, and the sidebar row menu with
//! inline rename (spec §4.1, §4.3). Only focus moves: nothing is sent to any TUI (the 已结束 rows' resume
//! types a command, see `ended.rs`).

use gilvt_agent::SessionKey;
use gpui::{prelude::*, App, ClipboardItem, Context, Div, Entity, Focusable, KeyDownEvent, Pixels, Point, Subscription, Window};

use super::{focus_pane_anywhere, PaneView, Workspace};
use crate::launch::{marks_expected, ShellEnv};
use crate::launcher::place::prompt_wait;
use crate::theme::AppSettings;
use crate::actions::*;
use crate::agents::{resume_allowed, Agents};
use crate::sidebar::ended::confirm_key;
use crate::sidebar::menu::{MenuCommand, SessionMenu};
use crate::sidebar::model::{self, Grouping, Step, Target};
use crate::sidebar::rename::{RenameEvent, RenameField};

/// A row being renamed in this window's sidebar.
pub struct Renaming {
    pub key: SessionKey,
    pub field: Entity<RenameField>,
    _events: Subscription,
}

impl Workspace {
    pub fn session_menu(&self) -> Option<&SessionMenu> {
        self.session_menu.as_ref()
    }

    /// The session whose sidebar row is being renamed.
    pub fn renaming_key(&self) -> Option<&SessionKey> {
        self.renaming.as_ref().map(|r| &r.key)
    }

    /// The inline rename field of `key`'s row, while it is being renamed.
    pub fn rename_field(&self, key: &SessionKey) -> Option<Entity<RenameField>> {
        self.renaming.as_ref().filter(|r| &r.key == key).map(|r| r.field.clone())
    }

    /// The session in this window's focused pane.
    fn current_session(&self, cx: &App) -> Option<SessionKey> {
        let pane = self.focused_pane()?;
        cx.global::<Agents>().registry().by_pane(pane).map(|s| s.key.clone())
    }

    /// Moves the keyboard to `target`'s pane, in whichever window has it.
    fn go_to(&mut self, target: Option<Target>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, pane)) = target else { return };
        if self.has_pane(pane) {
            self.focus_pane(pane, window, cx);
        } else {
            App::defer(cx, move |cx| focus_pane_anywhere(pane, cx));
        }
    }

    fn switch(&mut self, pick: impl FnOnce(&model::Model, Option<&SessionKey>) -> Option<Target>, window: &mut Window, cx: &mut Context<Self>) {
        let m = crate::sidebar::view::model_for(self, window.window_handle(), cx);
        let current = self.current_session(cx);
        let target = pick(&m, current.as_ref());
        self.go_to(target, window, cx);
    }

    pub fn open_session_menu(&mut self, key: SessionKey, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        // No stale rename field left behind; its own blur handler must not steal focus back.
        self.renaming = None;
        self.ended_confirm = None;
        self.session_menu = Some(SessionMenu { key, at });
        // Esc closes the menu here instead of reaching (and interrupting) an agent's TUI.
        window.focus(&self.focus_handle);
        cx.notify();
    }

    pub fn close_session_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_menu.take().is_some() {
            self.focus_active(window, cx);
        }
    }

    pub fn menu_command(&mut self, cmd: MenuCommand, key: SessionKey, window: &mut Window, cx: &mut Context<Self>) {
        self.session_menu = None;
        match cmd {
            MenuCommand::Resume(location) => return self.resume_ended(key, location, window, cx),
            MenuCommand::Archive => return self.archive_ended(key, window, cx),
            MenuCommand::Trash => return self.ask_trash_ended(key, window, cx),
            MenuCommand::Rename => return self.start_rename(key, window, cx),
            MenuCommand::ToggleMute => toggle_mute(&key, cx),
            MenuCommand::CopyId => cx.write_to_clipboard(ClipboardItem::new_string(key.1.clone())),
        }
        self.focus_active(window, cx);
    }

    /// Shows the rename field in `key`'s row (showing the sidebar in this window if it is hidden).
    pub fn start_rename(&mut self, key: SessionKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = cx.global::<Agents>().registry().get(&key) else { return };
        let current = session.name.clone();
        // Not remembered: this window only, for as long as the user leaves it open.
        self.sidebar.hidden = false;
        self.ended_confirm = None;
        let field = cx.new(|cx| RenameField::new(&current, window, cx));
        let id = field.entity_id();
        let events = cx.subscribe_in(&field, window, move |ws, _, event, window, cx| {
            if ws.renaming.as_ref().is_none_or(|r| r.field.entity_id() != id) {
                return;
            }
            let Some(r) = ws.renaming.take() else { return };
            if let RenameEvent::Commit(name) = event {
                Agents::rename(&r.key, name.clone(), cx);
            }
            if event.refocus_terminal() {
                ws.focus_active(window, cx);
            } else {
                // Blurred by something else taking the keyboard on purpose (e.g. the row menu);
                // just drop the field and let the redraw happen, without moving focus ourselves.
                cx.notify();
            }
        });
        window.focus(&field.focus_handle(cx));
        self.renaming = Some(Renaming { key, field, _events: events });
        cx.notify();
    }

    /// Esc closes the row menu, then the confirm bar; ↩ is 全部保存并关闭 on a bar
    /// listing unsaved files and 取消 on an agents-only bar. The bar only answers while the root
    /// itself has the keyboard (a key bubbling up from a pane or an overlay is not for it).
    fn on_root_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = e.keystroke.key.as_str();
        if self.close_confirm.is_some() && self.focus_handle.is_focused(window) {
            use super::close::CloseKey;
            let has_files = self.close_confirm.as_ref().is_some_and(|c| !c.dirty.is_empty());
            match super::close::close_key(key, &e.keystroke.modifiers, has_files) {
                Some(CloseKey::Close) if !e.is_held => self.confirm_close(window, cx),
                Some(CloseKey::SaveAll) if !e.is_held => self.save_all_and_close(window, cx),
                Some(CloseKey::Cancel) => self.cancel_close(window, cx),
                _ => return,
            }
            cx.stop_propagation();
        } else if self.session_menu.is_some() && key == "escape" {
            self.close_session_menu(window, cx);
            cx.stop_propagation();
        } else if self.ended_confirm.is_some() && self.focus_handle.is_focused(window) {
            match confirm_key(key, &e.keystroke.modifiers, e.is_held, self.session_menu.is_some()) {
                Some(true) => self.trash_ended_confirmed(window, cx),
                Some(false) => self.cancel_trash_ended(window, cx),
                None => return,
            }
            cx.stop_propagation();
        }
    }

    /// Types the resume command for a pending session into its old pane's shell (waits for the first prompt,
    /// like a new pane, since the shell just started). A session that has no pane any more is dropped.
    /// Only ever called for an explicit user action (row click, 全部恢复, the menu item).
    pub fn resume_pending(&mut self, key: &SessionKey, window: &mut Window, cx: &mut Context<Self>) {
        // Look before taking: an entry whose pane another window owns must stay (`resume_pending_anywhere`).
        let Some(p) = cx.global::<Agents>().pending().iter().find(|p| &p.key == key).cloned() else { return };
        let Some(PaneView::Terminal(t)) = self.panes.get(&p.pane).cloned() else { return };
        // Only into an idle shell: an agent or program started by hand in this pane would take the command as
        // input. The entry stays, so the user can click again (also right after a restore, before the first
        // foreground poll has classified the pane).
        let idle = cx.global::<Agents>().pane_is_idle_shell(p.pane);
        if !resume_allowed(idle, t.read(cx).launch_pending()) {
            let name = if p.name.is_empty() { p.key.1.clone() } else { p.name.clone() };
            self.error = Some(format!("该 pane 正在运行其他程序，无法恢复：{name}"));
            cx.notify();
            return;
        }
        Agents::take_pending(key, cx);
        let launch = cx.global::<AppSettings>().0.agent.launch(p.key.0).to_string();
        let cwd = p.cwd.clone().unwrap_or_else(|| std::path::PathBuf::from("."));
        let command = gilvt_agent::resume_command(&launch, p.key.0, &p.key.1, &cwd);
        let wait = prompt_wait(marks_expected(&cx.global::<AppSettings>().0, cx.global::<ShellEnv>()));
        t.update(cx, |t, cx| t.type_command(command, Some(wait), cx));
        Agents::typed(p.pane, cx);
        self.focus_pane(p.pane, window, cx);
        cx.notify();
    }

    /// 全部恢复: one pending session of any window every 500 ms, each in the window that owns its pane.
    pub fn resume_all_pending(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let keys: Vec<SessionKey> = cx.global::<Agents>().pending().iter().map(|p| p.key.clone()).collect();
        cx.spawn(async move |_, cx| {
            for key in keys {
                let _ = cx.update(|cx| super::resume_pending_anywhere(&key, cx));
                cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
            }
        })
        .detach();
    }

    /// The 「会话」 menu's actions on the window root.
    pub(super) fn session_actions(root: Div, cx: &mut Context<Self>) -> Div {
        root.on_key_down(cx.listener(Self::on_root_key))
            .on_action(cx.listener(|ws, _: &ToggleSidebar, _, cx| ws.update_sidebar(|s| s.hidden = !s.hidden, cx)))
            .on_action(cx.listener(|ws, _: &NextNeedsYou, window, cx| ws.switch(model::next_needs_you, window, cx)))
            .on_action(cx.listener(|ws, _: &PrevSession, window, cx| {
                ws.switch(|m, c| model::neighbor(&model::session_order(m), c, Step::Prev), window, cx)
            }))
            .on_action(cx.listener(|ws, _: &NextSession, window, cx| {
                ws.switch(|m, c| model::neighbor(&model::session_order(m), c, Step::Next), window, cx)
            }))
            .on_action(cx.listener(|ws, _: &GroupByProject, _, cx| ws.update_sidebar(|s| s.grouping = Grouping::Project, cx)))
            .on_action(cx.listener(|ws, _: &GroupByStatus, _, cx| ws.update_sidebar(|s| s.grouping = Grouping::Status, cx)))
            .on_action(cx.listener(|ws, _: &RenameSession, window, cx| {
                if let Some(key) = ws.current_session(cx) {
                    ws.start_rename(key, window, cx);
                }
            }))
            .on_action(cx.listener(|ws, _: &ResumeAll, window, cx| ws.resume_all_pending(window, cx)))
            .on_action(cx.listener(|ws, _: &MuteSession, _, cx| {
                if let Some(key) = ws.current_session(cx) {
                    toggle_mute(&key, cx);
                }
            }))
    }
}

fn toggle_mute(key: &SessionKey, cx: &mut App) {
    let Some(muted) = cx.global::<Agents>().registry().get(key).map(|s| s.muted) else { return };
    Agents::set_muted(key, !muted, cx);
}
