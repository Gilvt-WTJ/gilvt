//! The review's part of `gilvt debug state` (`session_center.session_review`).

use gilvt_agent::ReviewOutcome;
use gpui::App;

use super::model::{Layout, Mode};
use crate::debug_state::map::{cursor_id, session_id};
use crate::debug_state::rects::{Rect4, RectId, ReviewButton};
use crate::debug_state::{ReviewAction, ReviewActions, ReviewTurnState, SessionReviewState};
use crate::review::ReviewService;
use crate::session_center::view::SessionCenterView;

impl SessionCenterView {
    /// The open review as drawn in the latest frame; None while no review is open.
    pub(crate) fn debug_review(
        &self,
        rect: &dyn Fn(RectId) -> Option<Rect4>,
        cx: &App,
    ) -> Option<SessionReviewState> {
        let surface = self.surface.as_ref()?;
        let service = cx.global::<ReviewService>();
        let loading = service.detail_loading();
        let document = service
            .detail()
            .filter(|document| document.key == surface.key);
        // As drawn in the latest frame (the view records it while rendering).
        let narrow = self.layout == Layout::Narrow;
        let ready = self.can_review_next(cx);
        let stale = self.surface_stale();
        let action = |button: ReviewButton, enabled: bool| ReviewAction {
            enabled,
            rect: rect(RectId::ReviewButton(button)),
        };
        let menu = surface.snooze_menu;
        let has_runtime = self.surface_runtime().is_some();
        let can_control = self.can_control_runtime();
        Some(SessionReviewState {
            session_key: session_id(&surface.key),
            mode: match surface.mode {
                Mode::Incremental => "incremental",
                Mode::Full => "full",
            },
            layout: if narrow { "narrow" } else { "wide" },
            loading,
            error: service.detail_error().map(str::to_string),
            notice: self.notice.clone(),
            snapshot_through: document
                .as_ref()
                .map(|document| cursor_id(&document.snapshot_through)),
            has_earlier: document.as_ref().is_some_and(|d| d.has_earlier),
            has_later: document.as_ref().is_some_and(|d| d.has_later),
            snooze_menu: menu,
            stale,
            pinned: self
                .list
                .rows
                .iter()
                .any(|row| row.key == surface.key && row.pinned),
            turns: document
                .as_ref()
                .map(|document| {
                    document
                        .turns
                        .iter()
                        .enumerate()
                        .map(|(index, turn)| ReviewTurnState {
                            cursor: cursor_id(&turn.cursor),
                            ordinal: turn.ordinal,
                            outcome: match turn.outcome {
                                ReviewOutcome::Done => "done",
                                ReviewOutcome::Interrupted => "interrupted",
                                ReviewOutcome::Failed { .. } => "failed",
                            },
                            has_reply: !turn.final_reply.trim().is_empty(),
                            expanded: surface.is_expanded(&turn.cursor),
                            rect: rect(RectId::ReviewTurn(index)),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            actions: ReviewActions {
                review_next: action(ReviewButton::Next, ready),
                review_next_archive: action(
                    ReviewButton::NextArchive,
                    self.can_review_and_archive(cx),
                ),
                skip: action(ReviewButton::Skip, true),
                snooze: action(ReviewButton::Snooze, true),
                pin: action(ReviewButton::Pin, true),
                back_to_agent: action(ReviewButton::BackToAgent, true),
                interrupt: action(ReviewButton::Interrupt, has_runtime && can_control),
                terminate: action(ReviewButton::Terminate, has_runtime && can_control),
                copy_diagnostics: action(ReviewButton::CopyDiagnostics, has_runtime),
                terminate_confirming: self.terminate_confirm.as_ref().is_some_and(|key| key == &surface.key),
                full_history: action(ReviewButton::FullHistory, true),
                back: action(ReviewButton::Back, narrow),
                earlier: action(
                    ReviewButton::Earlier,
                    document.as_ref().is_some_and(|d| d.has_earlier),
                ),
                later: action(
                    ReviewButton::Later,
                    document.as_ref().is_some_and(|d| d.has_later),
                ),
                snooze_hour: action(ReviewButton::SnoozeHour, menu),
                snooze_later: action(ReviewButton::SnoozeLater, menu),
                snooze_tomorrow: action(ReviewButton::SnoozeTomorrow, menu),
                snooze_cancel: action(ReviewButton::SnoozeCancel, menu),
                baseline_here: action(ReviewButton::BaselineHere, stale),
                review_all: action(ReviewButton::ReviewAll, stale),
            },
        })
    }
}
