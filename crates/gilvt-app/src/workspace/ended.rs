//! The sidebar's 已结束 rows in a window (M3c spec §6): resuming one (double click, 恢复 / 在右侧恢复) through
//! `run_command`, and 移到废纸篓… behind the 会话 palette's confirm bar, drawn at the bottom of the sidebar.
//! The decisions are `sidebar::ended`; messages go to the window's banner.

use std::path::Path;
use std::time::SystemTime;

use gilvt_agent::SessionKey;
use gpui::{App, Context, Window};

use super::{focus_pane_anywhere, Workspace};
use crate::agents::Agents;
use crate::launcher::archive_sessions;
use crate::launcher::cleanup::trash_sessions;
use crate::launcher::sessions_model::Resume;
use crate::launcher::sessions_view::Confirm;
use crate::launcher::{History, Location};
use crate::sidebar::ended;
use crate::theme::AppSettings;

impl Workspace {
    /// The sidebar's confirm bar, while it shows.
    pub fn ended_confirm(&self) -> Option<&Confirm> {
        self.ended_confirm.as_ref()
    }

    /// Resumes ended session `key` at `location`, from the registry's cwd and id (spec §2.1 locations).
    pub fn resume_ended(&mut self, key: SessionKey, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        self.ended_confirm = None;
        let Some(session) = cx.global::<Agents>().registry().get(&key) else { return };
        let indexed_cwd = cx.try_global::<History>().and_then(|h| h.find(key.0, &key.1)).map(|e| e.cwd.clone());
        let launch = cx.global::<AppSettings>().0.agent.launch(key.0);
        let outcome = ended::resume(session, indexed_cwd.as_deref(), launch, Path::exists);
        match outcome {
            Resume::Run { dir, command } => self.run_command(location, dir, command, window, cx),
            Resume::Focus(pane) if self.has_pane(pane) => self.focus_pane(pane, window, cx),
            Resume::Focus(pane) => App::defer(cx, move |cx| focus_pane_anywhere(pane, cx)),
            other => {
                if other == Resume::Gone {
                    // Moved away outside gilvt: out of 已结束 and the history too.
                    Agents::forget(std::slice::from_ref(&key), cx);
                    History::refresh(cx);
                }
                self.error = other.toast();
                self.focus_active(window, cx);
                cx.notify();
            }
        }
    }

    /// 归档 on ended session `key`, as of the turns the history lists (a later turn brings it back). The
    /// session leaves 已结束 on the next frame; the result shows in the banner.
    pub(super) fn archive_ended(&mut self, key: SessionKey, window: &mut Window, cx: &mut Context<Self>) {
        let entry = cx.try_global::<History>().and_then(|h| h.find(key.0, &key.1)).cloned();
        self.error = match entry {
            Some(entry) => archive_sessions(&[entry], cx).toast(),
            None => Some(crate::i18n::text("找不到该会话的记录文件", "Could not find this session's record file").into()),
        };
        self.focus_active(window, cx);
        cx.notify();
    }

    /// 移到废纸篓… on ended session `key`: shows the confirm bar (the root takes the keyboard for ↩ / Esc).
    pub(super) fn ask_trash_ended(&mut self, key: SessionKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = cx.global::<Agents>().registry().get(&key) else { return };
        if session.is_live() {
            // It runs again: nothing to move (the final check in `trash_sessions` stays).
            self.error = Some(crate::i18n::text("运行中的会话不能移到废纸篓", "A running session cannot be moved to Trash").into());
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let indexed = cx.try_global::<History>().and_then(|h| h.find(key.0, &key.1));
        let Some(entry) = ended::trash_entry(session, indexed, SystemTime::now()) else {
            self.error = Some(crate::i18n::text("找不到该会话的记录文件", "Could not find this session's record file").into());
            self.focus_active(window, cx);
            cx.notify();
            return;
        };
        self.sidebar.hidden = false;
        let confirm = Confirm::new(vec![entry]);
        confirm.size_in_background(cx, |ws: &mut Self, id, bytes, cx| {
            if ws
                .ended_confirm
                .as_mut()
                .is_some_and(|c| c.companion_sized(id, bytes))
            {
                cx.notify();
            }
        });
        self.ended_confirm = Some(confirm);
        self.ended_confirm_focus = ended::ConfirmFocus::Requested;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// 移到废纸篓 on the confirm bar (or ↩): the palette's trash path; a failure shows in the banner.
    pub fn trash_ended_confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(confirm) = self.ended_confirm.take() else { return };
        let report = trash_sessions(confirm.entries, cx);
        if let Some(toast) = report.toast() {
            self.error = Some(toast);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    pub fn cancel_trash_ended(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ended_confirm.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }
}
