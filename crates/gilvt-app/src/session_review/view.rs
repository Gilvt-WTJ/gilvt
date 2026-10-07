//! Review actions of the Session Center. Nothing here writes to a PTY; the only persisted changes are the
//! review store's cursor, snooze and pin, and each of them reports a failed save instead of pretending.

use std::time::{SystemTime, UNIX_EPOCH};

use gilvt_agent::{interrupt_runtime, runtime_diagnostics, terminate_runtime, BindingConfidence, RuntimeRef, SessionKey};
use gpui::{point, px, ClipboardItem, Context, Window};

use super::model::{
    next_key, open_page_for, save_notice, snooze_until, turn_page, Action, Mode, Paging,
    SnoozeChoice, Surface,
};
use crate::launcher::archive_sessions;
use crate::launcher::sessions_view::SessionsEvent;
use crate::launcher::Location;
use crate::review::ReviewService;
use crate::session_center::model::Row;
use crate::session_center::view::SessionCenterView;

/// Seconds the local time zone is ahead of UTC at `now`.
fn utc_offset_secs(now: SystemTime) -> i64 {
    let secs = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs()) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return 0;
    }
    tm.tm_gmtoff as i64
}

impl SessionCenterView {
    pub(crate) fn dispatch(&mut self, action: Action, _: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::Open => self.open_review(cx),
            Action::ReviewNext => self.review_next(false, cx),
            Action::ReviewNextAndArchive => self.review_next(true, cx),
            Action::Skip => self.skip(cx),
            Action::OpenSnoozeMenu => self.set_snooze_menu(true),
            Action::CloseSnoozeMenu => self.set_snooze_menu(false),
            Action::Snooze(choice) => self.snooze(choice, cx),
            Action::Pin => self.toggle_pin(cx),
            Action::ToggleFullHistory => self.toggle_full_history(cx),
            Action::BaselineHere => self.baseline_here(cx),
            Action::ReviewAllVisible => self.review_all_visible(cx),
            Action::Page(direction) => self.page(direction, cx),
            Action::Scroll(direction) => self.scroll_detail(direction),
            Action::BackToAgent => self.back_to_agent(cx),
            Action::Interrupt => self.interrupt(cx),
            Action::Terminate => self.terminate(cx),
            Action::CopyDiagnostics => self.copy_diagnostics(cx),
            Action::Back => self.close_review(),
            Action::Close => cx.emit(SessionsEvent::Close),
        }
        cx.notify();
    }

    pub(crate) fn close_review(&mut self) {
        self.surface = None;
        self.notice = None;
        self.terminate_confirm = None;
    }

    fn set_snooze_menu(&mut self, open: bool) {
        if let Some(surface) = &mut self.surface {
            surface.snooze_menu = open;
        }
    }

    /// Opens (or reloads) the read-only review of the selected row. This never moves the review cursor.
    pub(crate) fn open_review(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self
            .selection
            .selected(&self.list.rows)
            .map(|row| row.key.clone())
        else {
            return;
        };
        // What "reviewed, next" will save: the last turn the queue showed for this session right now.
        let through = self
            .row_for(&key)
            .and_then(|row| row.snapshot_through.clone());
        match &mut self.surface {
            Some(surface) if surface.key == key => surface.snooze_menu = false,
            Some(surface) => surface.reopen(key),
            None => self.surface = Some(Surface::new(key)),
        }
        if let Some(surface) = &mut self.surface {
            surface.opened_through = through;
        }
        self.notice = None;
        self.terminate_confirm = None;
        self.reload(cx);
    }

    fn row_for(&self, key: &SessionKey) -> Option<&Row> {
        self.list.rows.iter().find(|row| &row.key == key)
    }

    /// The open session's saved review position is not in its transcript any more.
    pub(crate) fn surface_stale(&self) -> bool {
        self.surface
            .as_ref()
            .and_then(|surface| self.row_for(&surface.key))
            .is_some_and(|row| row.cursor_stale)
    }

    /// "Reviewed, next" would act right now: the shown document is this session's and fully loaded, no turn
    /// follows the shown page, and the saved position is valid. The button, DebugState and the key all use
    /// this one answer.
    pub(crate) fn can_review_next(&self, cx: &gpui::App) -> bool {
        let Some(surface) = &self.surface else {
            return false;
        };
        let service = cx.global::<ReviewService>();
        !service.detail_loading()
            && service.detail().is_some_and(|document| {
                surface
                    .review_target(
                        &document.key,
                        &document.snapshot_through,
                        document.has_later,
                        self.surface_stale(),
                    )
                    .is_some()
            })
    }

    /// 「标记已 Review 并归档」 would act right now: "reviewed, next" would, and the session is not running.
    pub(crate) fn can_review_and_archive(&self, cx: &gpui::App) -> bool {
        let running = self
            .surface
            .as_ref()
            .and_then(|surface| self.row_for(&surface.key))
            .is_some_and(|row| row.running);
        super::model::can_review_and_archive(self.can_review_next(cx), running)
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let stale = self.surface_stale();
        let Some(surface) = &mut self.surface else {
            return;
        };
        if stale {
            // Only the newest turns can be shown; the recovery actions decide what counts as seen.
            surface.set_mode(Mode::Full);
        }
        let reviewed_through = cx
            .global::<ReviewService>()
            .state(&surface.key)
            .reviewed_through;
        let page = open_page_for(surface.mode, reviewed_through.as_ref(), stale);
        self.request(page, cx);
    }

    fn request(&mut self, page: gilvt_agent::ReviewPage, cx: &mut Context<Self>) {
        let Some(surface) = &self.surface else {
            return;
        };
        let Some(index) = self
            .snapshot
            .reviews
            .iter()
            .find(|index| index.key == surface.key)
            .cloned()
        else {
            self.notice = Some("找不到这个会话的记录文件".into());
            return;
        };
        self.detail_scroll.set_offset(point(px(0.), px(0.)));
        ReviewService::request_detail(index, page, cx);
    }

    fn toggle_full_history(&mut self, cx: &mut Context<Self>) {
        let Some(surface) = &mut self.surface else {
            return;
        };
        let mode = match surface.mode {
            Mode::Incremental => Mode::Full,
            Mode::Full => Mode::Incremental,
        };
        surface.set_mode(mode);
        self.reload(cx);
    }

    fn page(&mut self, direction: Paging, cx: &mut Context<Self>) {
        let Some(surface) = &self.surface else {
            return;
        };
        let Some(document) = cx.global::<ReviewService>().detail() else {
            return;
        };
        let available = match direction {
            Paging::Earlier => document.has_earlier,
            Paging::Later => document.has_later,
        };
        let (Some(first), Some(last)) = (document.turns.first(), document.turns.last()) else {
            return;
        };
        if document.key != surface.key || !available {
            return;
        }
        let page = turn_page(direction, &first.cursor, &last.cursor);
        self.request(page, cx);
    }

    fn scroll_detail(&mut self, direction: i8) {
        let offset = self.detail_scroll.offset();
        let reach = self.detail_scroll.max_offset().height;
        let y = (offset.y - px(48.) * f32::from(direction))
            .min(px(0.))
            .max(-reach);
        self.detail_scroll.set_offset(point(offset.x, y));
    }

    fn queue_keys(&self) -> Vec<SessionKey> {
        self.list.rows.iter().map(|row| row.key.clone()).collect()
    }

    /// Moves the queue selection to `key`; false when it is no longer in the queue.
    fn select_key(&mut self, key: &SessionKey) -> bool {
        let Some(index) = self.list.rows.iter().position(|row| &row.key == key) else {
            return false;
        };
        self.selection.set(&self.list.rows, index);
        self.scroll_to_cursor();
        true
    }

    /// Shows `next` after the current item was resolved, or closes the review when nothing is left.
    fn go_to(&mut self, next: Option<SessionKey>, cx: &mut Context<Self>) {
        self.invalidate();
        self.sync(cx);
        match next {
            Some(key) if self.select_key(&key) => self.open_review(cx),
            _ => self.close_review(),
        }
    }

    fn skip(&mut self, cx: &mut Context<Self>) {
        let Some(current) = self.surface.as_ref().map(|surface| surface.key.clone()) else {
            return;
        };
        // Skipping persists nothing: the queue is unchanged and only the selection moves.
        match next_key(&self.queue_keys(), &current) {
            Some(key) if self.select_key(&key) => self.open_review(cx),
            _ => {}
        }
    }

    /// "Reviewed, next": advances the cursor to the last turn the queue showed when this review was opened,
    /// so a turn that finished afterwards stays in the queue. Only when the shown page reaches the end (no
    /// turn is marked that was not shown) and the saved position is valid. The queue only moves on once the
    /// save succeeded.
    fn review_next(&mut self, archive: bool, cx: &mut Context<Self>) {
        let stale = self.surface_stale();
        let Some(surface) = &self.surface else {
            return;
        };
        let Some(document) = cx.global::<ReviewService>().detail() else {
            return;
        };
        let Some(target) = surface.review_target(
            &document.key,
            &document.snapshot_through,
            document.has_later,
            stale,
        ) else {
            return;
        };
        let key = surface.key.clone();
        let Some(index) = self
            .snapshot
            .reviews
            .iter()
            .find(|index| index.key == key)
            .cloned()
        else {
            self.notice = Some("找不到这个会话的记录文件，没有标记为已 Review".into());
            return;
        };
        // The archive needs the history's entry (its turn count) and a session that is not running.
        let entry = if archive {
            if !self.can_review_and_archive(cx) {
                return;
            }
            match self
                .snapshot
                .entries
                .iter()
                .find(|e| (e.agent, e.session_id.clone()) == key)
                .cloned()
            {
                Some(entry) => Some(entry),
                None => {
                    self.notice = Some("找不到这个会话的历史记录，没有标记为已 Review".into());
                    return;
                }
            }
        } else {
            None
        };
        let next = next_key(&self.queue_keys(), &key);
        let saved =
            cx.global_mut::<ReviewService>()
                .advance_reviewed(&index, target, SystemTime::now());
        match saved {
            Ok(_) => {
                self.notice = None;
                let report = entry.map(|entry| archive_sessions(&[entry], cx));
                self.go_to(next, cx);
                if let Some(report) = report.filter(|r| r.archived == 0) {
                    // Reviewed, but the archive did not happen: say so on the next item.
                    self.notice = Some(report.toast().map_or_else(
                        || "已标记为已 Review，但没有归档".to_string(),
                        |t| format!("已标记为已 Review，但没有归档（{t}）"),
                    ));
                }
            }
            Err(error) => {
                self.notice = Some(save_notice(
                    error.kind(),
                    "标记为已 Review",
                    &error.to_string(),
                ));
            }
        }
    }

    /// Stale position, option 1: treat everything up to the session's newest completed turn as seen.
    fn baseline_here(&mut self, cx: &mut Context<Self>) {
        if !self.surface_stale() {
            return;
        }
        let Some(key) = self.surface.as_ref().map(|surface| surface.key.clone()) else {
            return;
        };
        let Some(last) = self
            .snapshot
            .reviews
            .iter()
            .find(|index| index.key == key)
            .and_then(|index| index.last_completed())
            .map(|turn| turn.cursor.clone())
        else {
            self.notice = Some("这个会话现在没有已完成的 turn，不能从当前开始".into());
            return;
        };
        let next = next_key(&self.queue_keys(), &key);
        let saved =
            cx.global_mut::<ReviewService>()
                .reset_reviewed(&key, Some(last), SystemTime::now());
        match saved {
            Ok(()) => {
                self.notice = None;
                self.go_to(next, cx);
            }
            Err(error) => {
                self.notice = Some(save_notice(
                    error.kind(),
                    "重置 Review 位置",
                    &error.to_string(),
                ));
            }
        }
    }

    /// Stale position, option 2: review every turn that is still in the transcript.
    fn review_all_visible(&mut self, cx: &mut Context<Self>) {
        if !self.surface_stale() {
            return;
        }
        let Some(key) = self.surface.as_ref().map(|surface| surface.key.clone()) else {
            return;
        };
        let saved = cx
            .global_mut::<ReviewService>()
            .reset_reviewed(&key, None, SystemTime::now());
        match saved {
            Ok(()) => {
                self.notice = None;
                self.invalidate();
                self.sync(cx);
                // The stale page was the full history; now every turn is pending, so go back to those.
                if let Some(surface) = &mut self.surface {
                    surface.set_mode(Mode::Incremental);
                }
                if self.select_key(&key) {
                    self.open_review(cx);
                } else {
                    self.close_review();
                }
            }
            Err(error) => {
                self.notice = Some(save_notice(
                    error.kind(),
                    "重置 Review 位置",
                    &error.to_string(),
                ));
            }
        }
    }

    fn snooze(&mut self, choice: SnoozeChoice, cx: &mut Context<Self>) {
        let Some(key) = self.surface.as_ref().map(|surface| surface.key.clone()) else {
            return;
        };
        let now = SystemTime::now();
        let until = snooze_until(choice, now, utc_offset_secs(now));
        let next = next_key(&self.queue_keys(), &key);
        let saved = cx
            .global_mut::<ReviewService>()
            .set_snoozed_until(&key, Some(until));
        match saved {
            Ok(()) => {
                self.notice = None;
                self.go_to(next, cx);
            }
            Err(error) => {
                self.set_snooze_menu(false);
                self.notice = Some(save_notice(
                    error.kind(),
                    "设置稍后提醒",
                    &error.to_string(),
                ));
            }
        }
    }

    fn toggle_pin(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.surface.as_ref().map(|surface| surface.key.clone()) else {
            return;
        };
        let pinned = cx.global::<ReviewService>().state(&key).pinned;
        match cx.global_mut::<ReviewService>().set_pinned(&key, !pinned) {
            Ok(()) => {
                self.notice = None;
                self.invalidate();
            }
            Err(error) => {
                let what = if pinned { "取消置顶" } else { "置顶" };
                self.notice = Some(save_notice(error.kind(), what, &error.to_string()));
            }
        }
    }

    fn back_to_agent(&mut self, cx: &mut Context<Self>) {
        if let Some(key) = self.surface.as_ref().map(|surface| surface.key.clone()) {
            self.select_key(&key);
        }
        self.activate(Location::Smart, cx);
    }

    pub(crate) fn surface_runtime(&self) -> Option<(&SessionKey, &RuntimeRef)> {
        let key = &self.surface.as_ref()?.key;
        Some((key, self.row_for(key)?.runtime.as_ref()?))
    }

    pub(crate) fn can_control_runtime(&self) -> bool {
        self.surface_runtime().is_some_and(|(_, runtime)| runtime.confidence == BindingConfidence::Exact)
    }

    fn interrupt(&mut self, cx: &mut Context<Self>) {
        let Some((key, runtime)) = self.surface_runtime().map(|(key, runtime)| (key.clone(), runtime.clone())) else { return };
        self.terminate_confirm = None;
        self.notice = Some(match interrupt_runtime(key.0, &runtime) {
            Ok(()) => "已向 Agent 发送中断信号".into(),
            Err(error) => error.to_string(),
        });
        cx.notify();
    }

    fn terminate(&mut self, cx: &mut Context<Self>) {
        let Some((key, runtime)) = self.surface_runtime().map(|(key, runtime)| (key.clone(), runtime.clone())) else { return };
        if runtime.confidence != BindingConfidence::Exact { return }
        if self.terminate_confirm.as_ref() != Some(&key) {
            self.terminate_confirm = Some(key);
            self.notice = Some("再次点击“确认终止”才会向 Agent 进程组发送 SIGTERM".into());
            cx.notify();
            return;
        }
        self.terminate_confirm = None;
        self.notice = Some(match terminate_runtime(key.0, &runtime) {
            Ok(()) => "已向 Agent 进程组发送终止信号".into(),
            Err(error) => error.to_string(),
        });
        cx.notify();
    }

    fn copy_diagnostics(&mut self, cx: &mut Context<Self>) {
        let Some((key, runtime)) = self.surface_runtime().map(|(key, runtime)| (key.clone(), runtime.clone())) else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(runtime_diagnostics(key.0, &key.1, &runtime)));
        self.notice = Some("已复制会话诊断信息".into());
        cx.notify();
    }
}
