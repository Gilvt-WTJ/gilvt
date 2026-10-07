//! The cleanup wizard as pure data: which preset is open, which of its hits are checked, the keys, the copy
//! of the preset lines, the summary and the two buttons, and the candidates the presets run over.

use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

use gilvt_agent::{HistoryEntry, ReviewSessionIndex, ReviewState, SessionKey};
use gpui::Modifiers;

use crate::launcher::cleanup_presets::run;
pub use crate::launcher::cleanup_presets::{Action, Candidate, Hit, Outcome, Preset};
use crate::launcher::sessions_model::size_label;

/// Which column the arrows move in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Column {
    #[default]
    Presets,
    Preview,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WizardModel {
    /// Index into `Preset::ALL`.
    pub preset: usize,
    /// The emphasised button (the preset's default action).
    pub action: Action,
    /// Candidate indexes (`Hit::ix`) the user unchecked.
    pub unpicked: HashSet<usize>,
    /// Row in the preview.
    pub cursor: usize,
    pub column: Column,
}

impl Default for WizardModel {
    fn default() -> Self {
        Self::new()
    }
}

impl WizardModel {
    /// The first preset, its default action, everything checked, the cursor on the first row.
    pub fn new() -> Self {
        WizardModel {
            preset: 0,
            action: Preset::ALL[0].default_action(),
            unpicked: HashSet::new(),
            cursor: 0,
            column: Column::Presets,
        }
    }

    /// Switches to preset `ix` (clamped) with `candidates_default` emphasised: every hit checked again.
    pub fn select_preset(&mut self, ix: usize, candidates_default: Action) {
        self.preset = ix.min(Preset::ALL.len() - 1);
        self.action = candidates_default;
        self.unpicked.clear();
        self.cursor = 0;
    }

    /// Switches to preset `ix` with its own default action and default picks (`outcome`: its hits, None
    /// while they are being computed — call again once they are known).
    pub fn open_preset(&mut self, ix: usize, outcome: Option<&Outcome>) {
        let ix = ix.min(Preset::ALL.len() - 1);
        self.select_preset(ix, Preset::ALL[ix].default_action());
        if let Some(outcome) = outcome {
            self.unpicked = Self::default_picks(outcome);
        }
    }

    /// The hits that start unchecked: the pinned ones (spec §4: pinned sessions are never picked by default).
    pub fn default_picks(outcome: &Outcome) -> HashSet<usize> {
        outcome
            .hits
            .iter()
            .filter(|h| h.pinned)
            .map(|h| h.ix)
            .collect()
    }

    /// The checked hits' candidate indexes, in the preview's order.
    pub fn picked(&self, outcome: &Outcome) -> Vec<usize> {
        outcome
            .hits
            .iter()
            .map(|h| h.ix)
            .filter(|ix| !self.unpicked.contains(ix))
            .collect()
    }

    pub fn is_picked(&self, ix: usize) -> bool {
        !self.unpicked.contains(&ix)
    }

    /// The same choices over a refreshed history (a rescan the user did not ask for): the preset, column and
    /// emphasised action stay; checks follow the sessions by key (`old_keys` / `new_keys` index the old and the
    /// new candidates); a session new to the open preset gets the default rule (pinned unchecked), one that
    /// left it is dropped; the cursor stays on its session while it is still listed, else keeps its row.
    /// `old`: the open preset's hits before (None while they were still being computed); `new`: after.
    pub fn carried_over(
        &self,
        old_keys: &[SessionKey],
        old: Option<&Outcome>,
        new_keys: &[SessionKey],
        new: &Outcome,
    ) -> WizardModel {
        let was_picked: HashMap<&SessionKey, bool> = old
            .map(|o| {
                o.hits
                    .iter()
                    .filter_map(|h| old_keys.get(h.ix).map(|k| (k, self.is_picked(h.ix))))
                    .collect()
            })
            .unwrap_or_default();
        let unpicked = new
            .hits
            .iter()
            .filter(|h| {
                let picked = new_keys
                    .get(h.ix)
                    .and_then(|k| was_picked.get(k).copied())
                    .unwrap_or(!h.pinned);
                !picked
            })
            .map(|h| h.ix)
            .collect();
        let cursor_key = old
            .and_then(|o| o.hits.get(self.cursor))
            .and_then(|h| old_keys.get(h.ix));
        let cursor = cursor_key
            .and_then(|key| {
                new.hits
                    .iter()
                    .position(|h| new_keys.get(h.ix) == Some(key))
            })
            .unwrap_or_else(|| self.cursor.min(new.hits.len().saturating_sub(1)));
        WizardModel {
            preset: self.preset,
            action: self.action,
            unpicked,
            cursor,
            column: self.column,
        }
    }

    /// Checks or unchecks candidate `ix`.
    pub fn toggle(&mut self, ix: usize) {
        if !self.unpicked.remove(&ix) {
            self.unpicked.insert(ix);
        }
    }

    fn picked_hits<'a>(&'a self, outcome: &'a Outcome) -> impl Iterator<Item = &'a Hit> + 'a {
        outcome.hits.iter().filter(|h| self.is_picked(h.ix))
    }

    /// The checked hits 归档 would act on: the ones not archived already (archiving those again would only
    /// restart their clock).
    pub fn to_archive(&self, outcome: &Outcome) -> Vec<usize> {
        self.picked_hits(outcome)
            .filter(|h| !h.archived)
            .map(|h| h.ix)
            .collect()
    }

    /// 「已选 36 / 37 · 183 MB」: the checked hits and their bytes (companion data included).
    pub fn summary(&self, outcome: &Outcome) -> String {
        let (n, bytes) = self
            .picked_hits(outcome)
            .fold((0, 0), |(n, b), h| (n + 1, b + h.bytes));
        if crate::i18n::current() == crate::i18n::Language::English {
            format!(
                "Selected {n} / {} · {}",
                outcome.hits.len(),
                size_label(bytes)
            )
        } else {
            format!("已选 {n} / {} · {}", outcome.hits.len(), size_label(bytes))
        }
    }

    /// 「归档 N 个」 (N: the checked hits not archived already) and 「移到废纸篓 N 个（X MB + 附属 Y MB）」 (X the
    /// transcripts, Y their companion data; no 附属 part without any), each enabled when its N > 0.
    pub fn buttons(&self, outcome: &Outcome) -> [(Action, String, bool); 2] {
        let archive = self.to_archive(outcome).len();
        let (mut n, mut bytes, mut companion) = (0usize, 0u64, 0u64);
        for h in self.picked_hits(outcome) {
            n += 1;
            bytes += h.bytes.saturating_sub(h.companion);
            companion += h.companion;
        }
        let english = crate::i18n::current() == crate::i18n::Language::English;
        let extra = if companion == 0 {
            String::new()
        } else if english {
            format!(" + {} companion data", size_label(companion))
        } else {
            format!(" + 附属 {}", size_label(companion))
        };
        [
            (
                Action::Archive,
                if english {
                    format!("Archive {archive}")
                } else {
                    format!("归档 {archive} 个")
                },
                archive > 0,
            ),
            (
                Action::Trash,
                if english {
                    format!("Move {n} to Trash ({}{extra})", size_label(bytes))
                } else {
                    format!("移到废纸篓 {n} 个（{}{extra}）", size_label(bytes))
                },
                n > 0,
            ),
        ]
    }

    /// A key. `outcomes`: the four presets' hits (None while computing); `confirm_open`: the Trash confirm bar
    /// shows; `enter_confirms`: this ↩ is a deliberate plain one (`sessions_model::confirms_trash`).
    pub fn key(
        &mut self,
        key: WizardKey,
        outcomes: Option<&[Outcome]>,
        confirm_open: bool,
        enter_confirms: bool,
    ) -> Effect {
        if confirm_open {
            return match key {
                WizardKey::Escape => Effect::CancelConfirm,
                WizardKey::Enter if enter_confirms => Effect::ConfirmTrash,
                _ => Effect::None,
            };
        }
        let current = outcomes.and_then(|o| o.get(self.preset));
        let rows = current.map_or(0, |o| o.hits.len());
        match (key, self.column) {
            (WizardKey::Escape, _) => return Effect::Back,
            (WizardKey::Up, Column::Presets) if self.preset > 0 => self.open_preset(
                self.preset - 1,
                outcomes.and_then(|o| o.get(self.preset - 1)),
            ),
            (WizardKey::Down, Column::Presets) if self.preset + 1 < Preset::ALL.len() => self
                .open_preset(
                    self.preset + 1,
                    outcomes.and_then(|o| o.get(self.preset + 1)),
                ),
            (WizardKey::Up, Column::Preview) => self.cursor = self.cursor.saturating_sub(1),
            (WizardKey::Down, Column::Preview) if self.cursor + 1 < rows => self.cursor += 1,
            (WizardKey::Right | WizardKey::Tab, _) => self.column = Column::Preview,
            (WizardKey::Left, _) => self.column = Column::Presets,
            (WizardKey::Space, Column::Preview) => {
                if let Some(h) = current.and_then(|o| o.hits.get(self.cursor)) {
                    self.toggle(h.ix);
                }
            }
            _ => {}
        }
        Effect::None
    }
}

/// A key press in the wizard; None for anything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WizardKey {
    Up,
    Down,
    Right,
    /// ← and ⇧Tab.
    Left,
    Tab,
    Space,
    /// Any ↩ (whether it confirms is the confirm bar's rule).
    Enter,
    Escape,
}

/// What a key asks the view to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Back to the 会话 palette.
    Back,
    CancelConfirm,
    ConfirmTrash,
}

pub fn command(key: &str, m: Modifiers) -> Option<WizardKey> {
    let plain = !m.control && !m.alt && !m.platform && !m.shift;
    match key {
        "up" if plain => Some(WizardKey::Up),
        "down" if plain => Some(WizardKey::Down),
        "right" if plain => Some(WizardKey::Right),
        "left" if plain => Some(WizardKey::Left),
        "tab" if plain => Some(WizardKey::Tab),
        "tab" if m.shift && !m.control && !m.alt && !m.platform => Some(WizardKey::Left),
        "space" if plain => Some(WizardKey::Space),
        "enter" => Some(WizardKey::Enter),
        "escape" => Some(WizardKey::Escape),
        _ => None,
    }
}

/// What a new history publication does to the open wizard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryReload {
    /// Already loaded (or no history).
    None,
    /// Reload now, keeping the user's choices ([`WizardModel::carried_over`]).
    Now,
    /// The Trash confirm bar shows (it names the sessions it will move): reload once it closes.
    AfterConfirm,
}

/// Whether a new history publication (`now`; `seen`: the one the wizard last loaded) reloads the presets.
pub fn reload_for_history(
    seen: Option<u64>,
    now: Option<u64>,
    confirm_open: bool,
) -> HistoryReload {
    match (now.is_some() && now != seen, confirm_open) {
        (false, _) => HistoryReload::None,
        (true, false) => HistoryReload::Now,
        (true, true) => HistoryReload::AfterConfirm,
    }
}

/// A preset line's right side: 「14 个 · 0.3 MB」, or 「计算中…」 until the sizes are known.
pub fn preset_count(outcome: Option<&Outcome>) -> String {
    match outcome {
        Some(o) if crate::i18n::current() == crate::i18n::Language::English => {
            format!("{} · {}", o.hits.len(), size_label(o.bytes))
        }
        Some(o) => format!("{} 个 · {}", o.hits.len(), size_label(o.bytes)),
        None => crate::i18n::text("计算中…", "Calculating…").into(),
    }
}

/// (tool calls, fully reviewed) of a session from its review index. An unknown index counts as `u32::MAX`
/// tool calls (never "no tool calls") and as not reviewed; a session without turns has nothing to review.
pub fn review_facts(index: Option<&ReviewSessionIndex>, state: &ReviewState) -> (u32, bool) {
    let Some(index) = index else {
        return (u32::MAX, false);
    };
    let tools = index
        .turns
        .iter()
        .fold(0u32, |sum, t| sum.saturating_add(t.tool_count));
    let reviewed = index
        .turns
        .last()
        .is_none_or(|t| state.reviewed_through.as_ref() == Some(&t.cursor));
    (tools, reviewed)
}

/// The presets' candidates: `states`, `companion` and `live` line up with `entries`.
pub fn candidates<'a>(
    entries: &'a [HistoryEntry],
    states: &'a [ReviewState],
    reviews: &HashMap<SessionKey, ReviewSessionIndex>,
    companion: &[u64],
    live: &[bool],
) -> Vec<Candidate<'a>> {
    entries
        .iter()
        .zip(states)
        .enumerate()
        .map(|(i, (entry, state))| {
            let index = reviews.get(&(entry.agent, entry.session_id.clone()));
            let (tools, fully_reviewed) = review_facts(index, state);
            Candidate {
                entry,
                state,
                tools,
                fully_reviewed,
                companion_bytes: companion.get(i).copied().unwrap_or(0),
                live: live.get(i).copied().unwrap_or(false),
            }
        })
        .collect()
}

/// The four presets' outcomes, in `Preset::ALL`'s order.
pub fn outcomes(candidates: &[Candidate], now: SystemTime) -> Vec<Outcome> {
    Preset::ALL
        .iter()
        .map(|&p| run(p, candidates, now))
        .collect()
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
