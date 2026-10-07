//! When to summarize again (S2 §4.4), and the queue of pending summaries.

use std::time::{Duration, Instant};

pub const MAX_FAILURES: u32 = 3;

/// Why a summary is wanted; higher runs first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// A running session's interval elapsed.
    Periodic,
    /// A turn ended (agent) / a command ended (terminal).
    Ended,
    /// The session started needing you or failed.
    Attention,
    /// The user asked.
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolicyConfig {
    pub auto: bool,
    pub interval: Duration,
}

/// What happened to a subject's summaries so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    /// The activity the latest successful summary covered.
    pub covered: Option<u64>,
    /// When the latest summary (any) was started.
    pub last_run: Option<Instant>,
    /// Failures in a row.
    pub failures: u32,
    /// A trigger held back by the interval.
    pub deferred: Option<Priority>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    /// Grows with the subject's activity (see `Summaries::activity`); 0 = nothing yet.
    pub activity: u64,
    /// An agent working right now (periodic refresh applies).
    pub running: bool,
    pub trigger: Option<Priority>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Now(Priority),
    /// Wanted, but the interval since the last run has not passed: keep it as `deferred`.
    Later(Priority),
    Skip,
}

pub fn decide(cfg: &PolicyConfig, track: &Track, seen: &Seen, now: Instant) -> Decision {
    if seen.trigger == Some(Priority::Manual) {
        return Decision::Now(Priority::Manual);
    }
    if !cfg.auto || track.failures >= MAX_FAILURES || seen.activity == 0 || track.covered.is_some_and(|c| seen.activity <= c) {
        return Decision::Skip;
    }
    let since = track.last_run.map(|t| now.saturating_duration_since(t));
    let due = since.map_or(true, |d| d >= cfg.interval);
    let periodic = (seen.running && due).then_some(Priority::Periodic);
    let Some(p) = [seen.trigger, track.deferred, periodic].into_iter().flatten().max() else { return Decision::Skip };
    if due {
        Decision::Now(p)
    } else {
        Decision::Later(p)
    }
}

/// [`decide`], kept in `track`: a summary to queue now, or None. A trigger held back by the interval is kept
/// as `deferred`; it is dropped once it ran, and when the activity is already covered (a later, unrelated
/// change must not fire it).
pub fn apply(cfg: &PolicyConfig, track: &mut Track, seen: &Seen, now: Instant) -> Option<Priority> {
    match decide(cfg, track, seen, now) {
        Decision::Now(p) => {
            track.deferred = None;
            Some(p)
        }
        Decision::Later(p) => {
            track.deferred = track.deferred.max(Some(p));
            None
        }
        Decision::Skip => {
            if track.covered.is_some_and(|c| seen.activity <= c) {
                track.deferred = None;
            }
            None
        }
    }
}

/// Pending summaries by key: one entry per key (a new push raises its priority and replaces its payload),
/// popped by priority, then by first push.
#[derive(Debug)]
pub struct Queue<T> {
    items: Vec<(String, Priority, T, u64)>,
    next: u64,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Queue { items: Vec::new(), next: 0 }
    }
}

impl<T> Queue<T> {
    pub fn push(&mut self, key: String, p: Priority, payload: T) {
        if let Some(item) = self.items.iter_mut().find(|i| i.0 == key) {
            item.1 = item.1.max(p);
            item.2 = payload;
            return;
        }
        self.next += 1;
        self.items.push((key, p, payload, self.next));
    }

    pub fn pop(&mut self) -> Option<(String, Priority, T)> {
        let best = self.items.iter().enumerate().max_by(|(_, a), (_, b)| a.1.cmp(&b.1).then(b.3.cmp(&a.3)))?.0;
        let (key, p, payload, _) = self.items.remove(best);
        Some((key, p, payload))
    }

    pub fn remove(&mut self, key: &str) {
        self.items.retain(|i| i.0 != key);
    }

    /// Keeps only the entries whose key passes `keep`.
    pub fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.items.retain(|i| keep(&i.0));
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.items.iter().any(|i| i.0 == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    fn cfg(auto: bool) -> PolicyConfig {
        PolicyConfig { auto, interval: 2 * MIN }
    }

    fn seen(activity: u64, running: bool, trigger: Option<Priority>) -> Seen {
        Seen { activity, running, trigger }
    }

    #[test]
    fn manual_always_runs() {
        let t0 = Instant::now();
        let track = Track { covered: Some(9), last_run: Some(t0), failures: 5, deferred: None };
        assert_eq!(decide(&cfg(false), &track, &seen(9, false, Some(Priority::Manual)), t0), Decision::Now(Priority::Manual));
    }

    #[test]
    fn auto_off_only_manual() {
        let t0 = Instant::now();
        assert_eq!(decide(&cfg(false), &Track::default(), &seen(5, true, Some(Priority::Attention)), t0), Decision::Skip);
    }

    #[test]
    fn nothing_new_nothing_to_do() {
        let t0 = Instant::now();
        let track = Track { covered: Some(5), ..Track::default() };
        assert_eq!(decide(&cfg(true), &track, &seen(5, true, Some(Priority::Ended)), t0), Decision::Skip);
        assert_eq!(decide(&cfg(true), &Track::default(), &seen(0, true, None), t0), Decision::Skip, "no activity yet");
    }

    #[test]
    fn a_running_session_is_summarized_then_every_interval() {
        let t0 = Instant::now();
        assert_eq!(decide(&cfg(true), &Track::default(), &seen(3, true, None), t0), Decision::Now(Priority::Periodic));
        let track = Track { covered: Some(3), last_run: Some(t0), ..Track::default() };
        assert_eq!(decide(&cfg(true), &track, &seen(4, true, None), t0 + MIN), Decision::Skip, "periodic waits");
        assert_eq!(decide(&cfg(true), &track, &seen(4, true, None), t0 + 2 * MIN), Decision::Now(Priority::Periodic));
        assert_eq!(decide(&cfg(true), &track, &seen(4, false, None), t0 + 10 * MIN), Decision::Skip, "idle: only on events");
    }

    #[test]
    fn events_inside_the_interval_wait() {
        let t0 = Instant::now();
        let track = Track { covered: Some(3), last_run: Some(t0), ..Track::default() };
        assert_eq!(decide(&cfg(true), &track, &seen(4, false, Some(Priority::Ended)), t0 + MIN), Decision::Later(Priority::Ended));
        let track = Track { deferred: Some(Priority::Ended), ..track };
        assert_eq!(decide(&cfg(true), &track, &seen(4, false, None), t0 + 2 * MIN), Decision::Now(Priority::Ended), "the deferred trigger fires");
        assert_eq!(decide(&cfg(true), &track, &seen(4, true, Some(Priority::Attention)), t0 + 3 * MIN), Decision::Now(Priority::Attention), "the highest wins");
    }

    #[test]
    fn apply_keeps_and_clears_the_deferred_trigger() {
        let t0 = Instant::now();
        let mut track = Track { covered: Some(3), last_run: Some(t0), ..Track::default() };
        assert_eq!(apply(&cfg(true), &mut track, &seen(4, false, Some(Priority::Ended)), t0 + MIN), None);
        assert_eq!(track.deferred, Some(Priority::Ended), "held back by the interval");
        assert_eq!(apply(&cfg(true), &mut track, &seen(4, false, None), t0 + 2 * MIN), Some(Priority::Ended));
        assert_eq!(track.deferred, None, "it ran");
    }

    #[test]
    fn a_deferred_trigger_for_covered_activity_is_dropped() {
        let t0 = Instant::now();
        // An Ended trigger was held back; then a manual summary covered activity 4.
        let mut track = Track { covered: Some(4), last_run: Some(t0), deferred: Some(Priority::Ended), ..Track::default() };
        assert_eq!(apply(&cfg(true), &mut track, &seen(4, false, None), t0 + MIN), None);
        assert_eq!(track.deferred, None, "nothing new: the old trigger must not fire on later, unrelated activity");
        // Later activity alone (no trigger, idle) does not run.
        assert_eq!(apply(&cfg(true), &mut track, &seen(5, false, None), t0 + 3 * MIN), None);
    }

    #[test]
    fn three_failures_pause_auto() {
        let t0 = Instant::now();
        let track = Track { failures: MAX_FAILURES, ..Track::default() };
        assert_eq!(decide(&cfg(true), &track, &seen(4, true, Some(Priority::Attention)), t0), Decision::Skip);
    }

    #[test]
    fn queue_dedupes_and_pops_by_priority_then_age() {
        let mut q = Queue::default();
        q.push("a".into(), Priority::Periodic, 1);
        q.push("b".into(), Priority::Ended, 2);
        q.push("c".into(), Priority::Ended, 3);
        q.push("a".into(), Priority::Attention, 4);
        assert_eq!(q.len(), 3);
        assert_eq!(q.pop(), Some(("a".into(), Priority::Attention, 4)), "raised and its payload replaced");
        q.push("c".into(), Priority::Periodic, 5);
        assert_eq!(q.pop(), Some(("b".into(), Priority::Ended, 2)), "b was queued before c; c keeps Ended");
        assert_eq!(q.pop(), Some(("c".into(), Priority::Ended, 5)));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn retain_drops_the_keys_not_kept() {
        let mut q = Queue::default();
        q.push("a".into(), Priority::Ended, 1);
        q.push("b".into(), Priority::Manual, 2);
        q.push("c".into(), Priority::Periodic, 3);
        q.retain(|k| k != "b");
        assert!(!q.contains("b"));
        assert_eq!(q.len(), 2);
        assert_eq!(q.pop(), Some(("a".into(), Priority::Ended, 1)));
        assert_eq!(q.pop(), Some(("c".into(), Priority::Periodic, 3)));
    }
}
