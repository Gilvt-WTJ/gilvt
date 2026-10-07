//! The cleanup wizard's state and actions. Opening it reads the history, the review states and the registry;
//! the companion sizes (a walk of the disk) and the four presets are computed on the background executor,
//! and until they arrive every preset reads 「计算中…」 and both buttons are off. 归档 archives the checked
//! sessions through the shared helper (which re-checks each for running); 移到废纸篓 opens the palette's
//! confirm bar and only its confirmation moves anything.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gilvt_agent::{HistoryEntry, ReviewSessionIndex, ReviewState, Session, SessionKey};
use gpui::{
    prelude::*, App, Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, ScrollStrategy,
    Subscription, Task, UniformListScrollHandle, Window,
};

use super::model::{
    self, command, Action, Column, Effect, HistoryReload, Outcome, Preset, WizardModel,
};
use crate::agents::Agents;
use crate::debug_state::rects::{Rect4, RectId};
use crate::debug_state::{map, CleanupButton, CleanupPreset, CleanupRow};
use crate::launcher::archive_sessions;
use crate::launcher::cleanup::trash_sessions;
use crate::launcher::history::title;
use crate::launcher::sessions_model::{confirms_trash, local, when_label};
use crate::launcher::sessions_view::{Confirm, SessionsEvent};
use crate::launcher::{shorten_dir, History, DIR_MAX_CHARS};
use crate::review::ReviewService;

/// How [`CleanupWizard::reload`] treats what the user chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reload {
    Reset,
    KeepChoices,
}

const BANNER: Duration = Duration::from_secs(4);

/// How a session reads in the preview (worked out when the wizard loads; nothing here touches the disk).
pub(super) struct RowText {
    pub(super) key: SessionKey,
    pub(super) title: String,
    pub(super) dir: String,
    pub(super) dir_missing: bool,
    pub(super) turns: u32,
    pub(super) when: String,
}

pub struct CleanupWizard {
    pub(super) focus_handle: FocusHandle,
    pub(super) model: WizardModel,
    /// The history as loaded; `Hit::ix` indexes it.
    pub(super) entries: Arc<Vec<HistoryEntry>>,
    pub(super) rows: Vec<RowText>,
    /// The four presets' outcomes, in `Preset::ALL`'s order; None while computing.
    pub(super) outcomes: Option<Vec<Outcome>>,
    pub(super) confirm: Option<Confirm>,
    pub(super) banner: Option<String>,
    pub(super) scroll: UniformListScrollHandle,
    /// Bumped by every load, so a late result of an older one is dropped.
    load: u64,
    /// The history publication the last load read: opening the wizard starts a rescan (the palette's), and
    /// its result must reach the presets too.
    history_generation: Option<u64>,
    /// A rescan landed while the confirm bar showed: reload (keeping the choices) once it closes.
    pending_reload: bool,
    _compute: Option<Task<()>>,
    _banner: Option<Task<()>>,
    _history: Option<Subscription>,
}

impl EventEmitter<SessionsEvent> for CleanupWizard {}

impl Focusable for CleanupWizard {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CleanupWizard {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut wizard = CleanupWizard {
            focus_handle: cx.focus_handle(),
            model: WizardModel::new(),
            entries: Arc::default(),
            rows: Vec::new(),
            outcomes: None,
            confirm: None,
            banner: None,
            scroll: UniformListScrollHandle::new(),
            load: 0,
            history_generation: None,
            pending_reload: false,
            _compute: None,
            _banner: None,
            _history: None,
        };
        wizard._history = Some(cx.observe_global::<History>(|w, cx| {
            let now = cx.try_global::<History>().map(|h| h.snapshot().generation);
            match model::reload_for_history(w.history_generation, now, w.confirm.is_some()) {
                HistoryReload::None => {}
                HistoryReload::Now => w.reload(Reload::KeepChoices, cx),
                HistoryReload::AfterConfirm => w.pending_reload = true,
            }
        }));
        wizard.reload(Reload::Reset, cx);
        wizard
    }

    /// Reads what the presets run over and computes them in the background. `Reset` (on open and after the
    /// user's own 归档 / 移到废纸篓): the preset's default picks, the first row. `KeepChoices` (a rescan the user
    /// did not ask for): the old rows stay until the new result swaps in with the same preset, cursor, scroll
    /// and checks by session ([`WizardModel::carried_over`]).
    pub(super) fn reload(&mut self, mode: Reload, cx: &mut Context<Self>) {
        let snapshot = cx.try_global::<History>().map(History::snapshot);
        self.history_generation = snapshot.as_ref().map(|s| s.generation);
        let (entries, reviews) = snapshot.map(|s| (s.entries, s.reviews)).unwrap_or_default();
        let keys: Vec<SessionKey> = entries
            .iter()
            .map(|e| (e.agent, e.session_id.clone()))
            .collect();
        let mut states_by_key = cx
            .try_global::<ReviewService>()
            .map(|s| s.states(keys.iter().cloned()))
            .unwrap_or_default();
        let states: Vec<ReviewState> = keys
            .iter()
            .map(|k| states_by_key.remove(k).unwrap_or_default())
            .collect();
        let registry = cx.try_global::<Agents>().map(Agents::registry);
        let live: Vec<bool> = keys
            .iter()
            .map(|k| {
                registry
                    .and_then(|r| r.get(k))
                    .is_some_and(Session::is_live)
            })
            .collect();
        let home = crate::launcher::dir_label::label_home();
        let history = cx.try_global::<History>();
        let now = SystemTime::now();
        let now_local = local(now);
        let rows: Vec<RowText> = entries
            .iter()
            .zip(&keys)
            .map(|(e, key)| RowText {
                key: key.clone(),
                title: title(e, registry.and_then(|r| r.saved_name(key))),
                dir: shorten_dir(&e.cwd, home, DIR_MAX_CHARS),
                dir_missing: history.is_some_and(|h| !h.dir_exists(&e.cwd)),
                turns: e.turns,
                when: when_label(
                    local(e.last_active),
                    now_local,
                    now.duration_since(e.last_active).unwrap_or_default(),
                ),
            })
            .collect();
        self.pending_reload = false;
        let mut fresh = Some((entries.clone(), rows));
        if mode == Reload::Reset {
            if let Some((entries, rows)) = fresh.take() {
                self.entries = entries;
                self.rows = rows;
            }
            self.outcomes = None;
            self.confirm = None;
        }
        self.load += 1;
        let load = self.load;
        let work = cx.background_executor().spawn(async move {
            let companion: Vec<u64> = entries.iter().map(gilvt_agent::companion_size).collect();
            let reviews: HashMap<SessionKey, ReviewSessionIndex> =
                reviews.iter().map(|r| (r.key.clone(), r.clone())).collect();
            let candidates = model::candidates(&entries, &states, &reviews, &companion, &live);
            model::outcomes(&candidates, now)
        });
        self._compute = Some(cx.spawn(async move |this, cx| {
            let outcomes = work.await;
            let _ = this.update(cx, |w, cx| {
                if w.load != load {
                    return;
                }
                match fresh {
                    // A rescan: the old rows were still showing.
                    Some((entries, rows)) => {
                        if w.confirm.is_some() {
                            w.pending_reload = true;
                            return;
                        }
                        let old_keys: Vec<SessionKey> =
                            w.rows.iter().map(|r| r.key.clone()).collect();
                        let new_keys: Vec<SessionKey> =
                            rows.iter().map(|r| r.key.clone()).collect();
                        if let Some(new) = outcomes.get(w.model.preset) {
                            w.model = w.model.carried_over(&old_keys, w.current(), &new_keys, new);
                        }
                        w.entries = entries;
                        w.rows = rows;
                        w.outcomes = Some(outcomes);
                    }
                    None => {
                        w.model
                            .open_preset(w.model.preset, outcomes.get(w.model.preset));
                        w.outcomes = Some(outcomes);
                        w.scroll.scroll_to_item(0, ScrollStrategy::Top);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The open preset's outcome, once computed.
    pub(super) fn current(&self) -> Option<&Outcome> {
        self.outcomes
            .as_ref()
            .and_then(|o| o.get(self.model.preset))
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = command(&event.keystroke.key, event.keystroke.modifiers) else {
            return;
        };
        let confirms = confirms_trash(&event.keystroke.modifiers, event.is_held, false);
        let effect = self.model.key(
            key,
            self.outcomes.as_deref(),
            self.confirm.is_some(),
            confirms,
        );
        match effect {
            Effect::None => {}
            Effect::Back => cx.emit(SessionsEvent::CloseCleanup),
            Effect::CancelConfirm => self.cancel_confirm(cx),
            Effect::ConfirmTrash => self.trash_confirmed(cx),
        }
        if self.model.column == Column::Preview {
            self.scroll
                .scroll_to_item(self.model.cursor, ScrollStrategy::Center);
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// A click on preset `ix`.
    pub(super) fn click_preset(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.confirm.is_some() {
            return;
        }
        let outcome = self.outcomes.as_ref().and_then(|o| o.get(ix));
        self.model.open_preset(ix, outcome);
        self.model.column = Column::Presets;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// A click on preview row `row`: the cursor goes there and its box flips.
    pub(super) fn click_row(&mut self, row: usize, cx: &mut Context<Self>) {
        if self.confirm.is_some() {
            return;
        }
        let Some(ix) = self.current().and_then(|o| o.hits.get(row)).map(|h| h.ix) else {
            return;
        };
        self.model.cursor = row;
        self.model.column = Column::Preview;
        self.model.toggle(ix);
        cx.notify();
    }

    /// The checked hits' entries and their companion bytes.
    fn picked_entries(&self) -> (Vec<HistoryEntry>, u64) {
        let Some(outcome) = self.current() else {
            return (Vec::new(), 0);
        };
        let picked = self.model.picked(outcome);
        let companion = outcome
            .hits
            .iter()
            .filter(|h| self.model.is_picked(h.ix))
            .map(|h| h.companion)
            .sum();
        (
            picked
                .iter()
                .filter_map(|&ix| self.entries.get(ix).cloned())
                .collect(),
            companion,
        )
    }

    /// Whether the buttons do anything: the sizes are known, nothing is being confirmed, something is checked.
    pub(super) fn buttons(&self) -> [(Action, String, bool); 2] {
        let outcome = self.current().cloned().unwrap_or_default();
        let ready = self.outcomes.is_some() && self.confirm.is_none();
        self.model
            .buttons(&outcome)
            .map(|(action, label, enabled)| (action, label, enabled && ready))
    }

    /// 归档 / 移到废纸篓.
    pub(super) fn act(&mut self, action: Action, cx: &mut Context<Self>) {
        let index = match action {
            Action::Archive => 0,
            Action::Trash => 1,
        };
        if !self.buttons()[index].2 {
            return;
        }
        let (entries, companion) = self.picked_entries();
        if entries.is_empty() {
            return;
        }
        match action {
            Action::Archive => {
                // Only the sessions not archived yet: archiving again would just restart their clock.
                let entries: Vec<HistoryEntry> = self
                    .current()
                    .map(|o| self.model.to_archive(o))
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|ix| self.entries.get(ix).cloned())
                    .collect();
                let report = archive_sessions(&entries, cx);
                if let Some(toast) = report.toast() {
                    self.show_banner(toast, cx);
                }
                self.reload(Reload::Reset, cx);
            }
            Action::Trash => {
                self.confirm = Some(Confirm::with_companion(entries, companion));
                cx.notify();
            }
        }
    }

    /// 移到废纸篓 on the confirm bar (or a plain ↩ while it shows).
    pub(super) fn trash_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm.take() else {
            return;
        };
        let report = trash_sessions(confirm.entries, cx);
        let toast = report
            .toast()
            .unwrap_or_else(|| format!("已移到废纸篓 {} 个会话", report.moved));
        self.show_banner(toast, cx);
        self.reload(Reload::Reset, cx);
    }

    pub(super) fn cancel_confirm(&mut self, cx: &mut Context<Self>) {
        self.confirm = None;
        if self.pending_reload {
            self.reload(Reload::KeepChoices, cx);
        }
        cx.notify();
    }

    fn show_banner(&mut self, text: String, cx: &mut Context<Self>) {
        self.banner = Some(text);
        self._banner = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BANNER).await;
            let _ = this.update(cx, |w, cx| {
                w.banner = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn listeners(
        &self,
        root: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        root.on_key_down(cx.listener(Self::on_key_down))
    }

    /// `gilvt debug state`.
    pub fn debug_overlay(
        &self,
        rect: &dyn Fn(RectId) -> Option<Rect4>,
    ) -> crate::debug_state::Overlay {
        let presets = Preset::ALL
            .iter()
            .enumerate()
            .map(|(n, p)| {
                let outcome = self.outcomes.as_ref().and_then(|o| o.get(n));
                CleanupPreset {
                    label: p.label().into(),
                    hint: p.hint().into(),
                    count: outcome.map(|o| o.hits.len()),
                    bytes: outcome.map(|o| o.bytes),
                    active: n == self.model.preset,
                    rect: rect(RectId::CleanupPreset(n)),
                }
            })
            .collect();
        let rows = self
            .current()
            .map(|o| {
                o.hits
                    .iter()
                    .enumerate()
                    .filter_map(|(n, h)| {
                        let r = self.rows.get(h.ix)?;
                        Some(CleanupRow {
                            session: map::session_id(&r.key),
                            title: r.title.clone(),
                            dir: r.dir.clone(),
                            dir_missing: r.dir_missing,
                            picked: self.model.is_picked(h.ix),
                            pinned: h.pinned,
                            unreviewed: h.unreviewed,
                            selected: n == self.model.cursor,
                            rect: rect(RectId::CleanupRow(n)),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let outcome = self.current().cloned().unwrap_or_default();
        let buttons = self
            .buttons()
            .into_iter()
            .enumerate()
            .map(|(n, (action, label, enabled))| CleanupButton {
                label,
                action: match action {
                    Action::Archive => "archive",
                    Action::Trash => "trash",
                },
                default: action == self.model.action,
                enabled,
                rect: rect(RectId::CleanupButton(n)),
            })
            .collect();
        let confirm = self.confirm.as_ref().map(|c| crate::debug_state::Confirm {
            text: c.text.clone(),
            buttons: map::buttons(&map::CONFIRM_BUTTONS, RectId::CleanupConfirmButton, rect),
        });
        crate::debug_state::Overlay::Cleanup {
            preset: self.model.preset,
            column: match self.model.column {
                Column::Presets => "presets",
                Column::Preview => "preview",
            },
            selected: self.model.cursor,
            computing: self.outcomes.is_none(),
            presets,
            rows,
            summary: self.model.summary(&outcome),
            buttons,
            confirm,
            banner: self.banner.clone(),
        }
    }
}
