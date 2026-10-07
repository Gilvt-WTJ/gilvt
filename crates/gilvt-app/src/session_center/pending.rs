//! The number of sessions waiting in the 待 Review queue, for the sidebar entry and DebugState. The queue
//! itself is rebuilt by the Session Center when it is open; this counts the same thing (sessions whose
//! priority is not 需要你) for windows that only show a number, and caches it so a frame costs a comparison.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use gilvt_agent::SessionKey;
use gpui::{App, Global};

use super::model;
use crate::agents::Agents;
use crate::launcher::History;
use crate::review::{build_inbox, ReviewPriority, ReviewService, ReviewSort};

/// Everything the count depends on. Two equal keys give equal counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingKey {
    generation: u64,
    revision: u64,
    /// Snoozes expire with the clock, so the count is recomputed every minute.
    minute: u64,
    /// Sessions running in gilvt and whether each needs the user (sorted: order is not a change).
    live: Vec<(SessionKey, bool)>,
}

impl PendingKey {
    pub fn new(
        generation: u64,
        revision: u64,
        minute: u64,
        mut live: Vec<(SessionKey, bool)>,
    ) -> Self {
        live.sort();
        PendingKey {
            generation,
            revision,
            minute,
            live,
        }
    }
}

#[derive(Default)]
pub struct PendingCache(Option<(PendingKey, usize)>);

impl Global for PendingCache {}

impl PendingCache {
    pub fn get(&self, key: &PendingKey) -> Option<usize> {
        self.0
            .as_ref()
            .filter(|(cached, _)| cached == key)
            .map(|(_, count)| *count)
    }

    pub fn put(&mut self, key: PendingKey, count: usize) {
        self.0 = Some((key, count));
    }

    /// The most recently computed count: what the sidebar drew in its latest frame (0 before the first).
    pub fn last(&self) -> usize {
        self.0.as_ref().map_or(0, |(_, count)| *count)
    }
}

/// Sessions in the 待 Review queue right now: the same number the Session Center's tab shows.
pub fn pending_review_count(cx: &mut App) -> usize {
    if !cx.has_global::<ReviewService>() || !cx.has_global::<History>() {
        return 0;
    }
    let snapshot = cx.global::<History>().snapshot();
    let live = cx
        .try_global::<Agents>()
        .map(|agents| model::live_sessions(agents.registry().sessions(), Instant::now()))
        .unwrap_or_default();
    let now = SystemTime::now();
    let key = PendingKey::new(
        snapshot.generation,
        cx.global::<ReviewService>().revision(),
        now.duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs() / 60),
        live.iter()
            .map(|session| (session.key.clone(), session.needs_you))
            .collect(),
    );
    if let Some(count) = cx.default_global::<PendingCache>().get(&key) {
        return count;
    }
    let states = cx
        .global::<ReviewService>()
        .states(snapshot.reviews.iter().map(|index| index.key.clone()));
    let count = build_inbox(
        &snapshot.entries,
        &snapshot.reviews,
        &states,
        &model::runtime_map(&live),
        now,
        ReviewSort::Smart,
    )
    .iter()
    .filter(|item| item.priority != ReviewPriority::NeedsYou)
    .count();
    cx.default_global::<PendingCache>().put(key, count);
    count
}
