use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use gilvt_agent::{
    HistoryEntry, ReviewOutcome, ReviewSessionIndex, ReviewState, ReviewTurnIndex, SessionKey,
    TurnCursor,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewPriority {
    NeedsYou,
    Failed,
    Completed,
    RunningWithResults,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReviewSort {
    #[default]
    Smart,
    Recent,
    Project,
    Oldest,
}

/// Runtime facts that are known only for sessions observed by gilvt in this process.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewRuntime {
    pub running_in_gilvt: bool,
    pub needs_you: bool,
    /// Duration already spent waiting. Larger values sort first in the NeedsYou bucket.
    pub waiting_for: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewInboxItem {
    pub key: SessionKey,
    pub title: String,
    pub cwd: PathBuf,
    pub first_unreviewed: TurnCursor,
    pub snapshot_through: TurnCursor,
    pub unreviewed_count: u32,
    pub priority: ReviewPriority,
    pub pinned: bool,
    pub running_in_gilvt: bool,
    pub cursor_stale: bool,
    /// More words a search matches besides the title: the agent's own titles and the prompts the session
    /// started with.
    pub search_terms: Vec<String>,
    pub first_completed_at: Option<SystemTime>,
    pub latest_completed_at: Option<SystemTime>,
    pub latest_outcome: ReviewOutcome,
    pub tool_count: u32,
    pub failed_tool_count: u32,
    pub lines_added: u32,
    pub lines_removed: u32,
}

pub fn build_inbox(
    history: &[HistoryEntry],
    reviews: &[ReviewSessionIndex],
    states: &HashMap<SessionKey, ReviewState>,
    runtime: &HashMap<SessionKey, ReviewRuntime>,
    now: SystemTime,
    sort: ReviewSort,
) -> Vec<ReviewInboxItem> {
    let history: HashMap<SessionKey, &HistoryEntry> = history
        .iter()
        .map(|entry| ((entry.agent, entry.session_id.clone()), entry))
        .collect();
    let mut items = reviews
        .iter()
        .filter_map(|review| {
            let state = states.get(&review.key).cloned().unwrap_or_default();
            if state.snoozed_until.is_some_and(|until| until > now) {
                return None;
            }
            if history
                .get(&review.key)
                .is_some_and(|entry| state.archived_for(entry.turns))
            {
                return None;
            }
            let (start, cursor_stale) =
                unreviewed_start(&review.turns, state.reviewed_through.as_ref());
            let pending = review.turns.get(start..)?;
            let first = pending.first()?;
            let latest = pending.last()?;
            let live = runtime.get(&review.key).cloned().unwrap_or_default();
            let priority = if live.needs_you {
                ReviewPriority::NeedsYou
            } else if cursor_stale || matches!(first.outcome, ReviewOutcome::Failed { .. }) {
                ReviewPriority::Failed
            } else if live.running_in_gilvt {
                ReviewPriority::RunningWithResults
            } else {
                ReviewPriority::Completed
            };
            let entry = history.get(&review.key);
            Some(ReviewInboxItem {
                key: review.key.clone(),
                title: entry
                    .map(|entry| crate::launcher::history::title(entry, None))
                    .unwrap_or_else(|| first.prompt_preview.clone()),
                cwd: entry.map(|entry| entry.cwd.clone()).unwrap_or_default(),
                first_unreviewed: first.cursor.clone(),
                snapshot_through: latest.cursor.clone(),
                unreviewed_count: pending.len() as u32,
                priority,
                pinned: state.pinned,
                running_in_gilvt: live.running_in_gilvt,
                cursor_stale,
                search_terms: entry
                    .map(|entry| {
                        [
                            Some(entry.first_prompt.as_str()),
                            Some(entry.topic_prompt.as_str()),
                            entry.ai_title.as_deref(),
                            entry.custom_title.as_deref(),
                        ]
                        .into_iter()
                        .flatten()
                        .filter(|term| !term.is_empty())
                        .map(str::to_string)
                        .collect()
                    })
                    .unwrap_or_default(),
                first_completed_at: first.completed_at,
                latest_completed_at: latest.completed_at,
                latest_outcome: latest.outcome.clone(),
                tool_count: pending.iter().map(|turn| turn.tool_count).sum(),
                failed_tool_count: pending.iter().map(|turn| turn.failed_tool_count).sum(),
                lines_added: pending.iter().map(|turn| turn.lines_added).sum(),
                lines_removed: pending.iter().map(|turn| turn.lines_removed).sum(),
            })
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| compare(a, b, runtime, sort));
    items
}

fn unreviewed_start(turns: &[ReviewTurnIndex], cursor: Option<&TurnCursor>) -> (usize, bool) {
    match cursor {
        None => (0, false),
        Some(cursor) => match turns.iter().position(|turn| &turn.cursor == cursor) {
            Some(position) => (position + 1, false),
            None => (0, true),
        },
    }
}

fn compare(
    a: &ReviewInboxItem,
    b: &ReviewInboxItem,
    runtime: &HashMap<SessionKey, ReviewRuntime>,
    sort: ReviewSort,
) -> Ordering {
    let pinned = b.pinned.cmp(&a.pinned);
    let order = match sort {
        ReviewSort::Smart => priority_rank(a.priority)
            .cmp(&priority_rank(b.priority))
            .then(pinned)
            .then_with(|| match a.priority {
                ReviewPriority::NeedsYou => runtime
                    .get(&b.key)
                    .and_then(|live| live.waiting_for)
                    .cmp(&runtime.get(&a.key).and_then(|live| live.waiting_for)),
                _ => a.first_completed_at.cmp(&b.first_completed_at),
            }),
        ReviewSort::Recent => {
            pinned.then_with(|| b.latest_completed_at.cmp(&a.latest_completed_at))
        }
        ReviewSort::Project => pinned
            .then_with(|| a.cwd.cmp(&b.cwd))
            .then_with(|| a.first_completed_at.cmp(&b.first_completed_at)),
        ReviewSort::Oldest => pinned.then_with(|| a.first_completed_at.cmp(&b.first_completed_at)),
    };
    order.then_with(|| a.key.cmp(&b.key))
}

fn priority_rank(priority: ReviewPriority) -> u8 {
    match priority {
        ReviewPriority::NeedsYou => 0,
        ReviewPriority::Failed => 1,
        ReviewPriority::Completed => 2,
        ReviewPriority::RunningWithResults => 3,
    }
}
