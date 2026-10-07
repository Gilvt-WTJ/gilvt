//! The Dock badge and bounce (M3c §7). Pure: which sessions count, when the label changes, when to bounce;
//! `dock_tile` does the AppKit calls.

use std::collections::HashSet;

use gilvt_agent::{Session, SessionKey};

/// The sessions the badge counts: needing you (an approval or a question) and not muted.
pub fn waiting<'a>(sessions: impl IntoIterator<Item = &'a Session>) -> HashSet<SessionKey> {
    sessions.into_iter().filter(|s| s.needs_you() && !s.muted).map(|s| s.key.clone()).collect()
}

/// The badge for `count` waiting sessions; None clears it.
pub fn badge_label(count: usize) -> Option<String> {
    (count > 0).then(|| count.to_string())
}

/// Bounce once when a session starts waiting (it is in `now` but was not in `prev`) while gilvt is in the
/// background. A session that keeps waiting never bounces again; one that leaves and comes back does.
pub fn should_bounce(prev: &HashSet<SessionKey>, now: &HashSet<SessionKey>, app_active: bool, enabled: bool) -> bool {
    enabled && !app_active && now.difference(prev).next().is_some()
}

/// What to do to the Dock after an observation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DockUpdate {
    /// `Some(label)`: set the badge to `label` (None = clear). `None`: leave the badge as it is.
    pub badge: Option<Option<String>>,
    pub bounce: bool,
}

/// The waiting set and badge as last shown, plus what the caller was told to do so far (for
/// `gilvt debug state`: every update returned here is carried out by `update_dock`).
#[derive(Debug, Default)]
pub struct Dock {
    waiting: HashSet<SessionKey>,
    shown: usize,
    label: Option<String>,
    bounces: u64,
}

impl Dock {
    /// The waiting sessions are now `now`: the badge only when its count changed, a bounce per [`should_bounce`].
    pub fn observe(&mut self, now: HashSet<SessionKey>, app_active: bool, bounce_enabled: bool) -> DockUpdate {
        let bounce = should_bounce(&self.waiting, &now, app_active, bounce_enabled);
        let badge = (now.len() != self.shown).then(|| badge_label(now.len()));
        self.shown = now.len();
        self.waiting = now;
        if let Some(label) = &badge {
            self.label = label.clone();
        }
        self.bounces += u64::from(bounce);
        DockUpdate { badge, bounce }
    }

    /// The badge label last set; None when there is none.
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// How many bounces (`requestUserAttention`) this process has asked for.
    pub fn bounces(&self) -> u64 {
        self.bounces
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_agent::{AgentKind, Status};
    use std::time::Instant;

    fn key(id: &str) -> SessionKey {
        (AgentKind::Claude, id.to_string())
    }

    fn set(ids: &[&str]) -> HashSet<SessionKey> {
        ids.iter().map(|id| key(id)).collect()
    }

    fn session(id: &str, status: Status, muted: bool) -> Session {
        let mut s = Session::new(key(id), Instant::now());
        s.status = status;
        s.muted = muted;
        s
    }

    #[test]
    fn only_unmuted_sessions_that_need_you_count() {
        let approval = Status::NeedsApproval { action: "Bash(ls)".into() };
        let sessions = [
            session("a", approval.clone(), false),
            session("b", Status::Asking { question: "?".into() }, false),
            session("muted", approval, true),
            session("busy", Status::Thinking, false),
            session("idle", Status::Idle, false),
            session("gone", Status::Ended, false),
        ];
        assert_eq!(waiting(&sessions), set(&["a", "b"]));
        assert_eq!(badge_label(0), None);
        assert_eq!(badge_label(2).as_deref(), Some("2"));
    }

    #[test]
    fn bounce_only_for_a_new_waiter_in_the_background() {
        let (none, a, ab) = (set(&[]), set(&["a"]), set(&["a", "b"]));
        assert!(should_bounce(&none, &a, false, true));
        assert!(should_bounce(&a, &ab, false, true), "b is new");
        assert!(!should_bounce(&a, &a, false, true), "a keeps waiting");
        assert!(!should_bounce(&ab, &a, false, true), "b left");
        assert!(!should_bounce(&none, &a, true, true), "gilvt is in front");
        assert!(!should_bounce(&none, &a, false, false), "notify.dock_bounce = false");
        // One leaves and another arrives in the same batch: the count is the same, yet it is new.
        assert!(should_bounce(&a, &set(&["b"]), false, true));
    }

    #[test]
    fn the_badge_changes_only_with_the_count() {
        let mut dock = Dock::default();
        assert_eq!(dock.observe(set(&[]), false, true), DockUpdate::default(), "nothing shown at launch");
        assert_eq!(dock.observe(set(&["a"]), false, true), DockUpdate { badge: Some(Some("1".into())), bounce: true });
        assert_eq!(dock.observe(set(&["a"]), false, true), DockUpdate::default());
        // Swapped waiter: same count (no badge call) but a bounce.
        assert_eq!(dock.observe(set(&["b"]), false, true), DockUpdate { badge: None, bounce: true });
        assert_eq!(dock.observe(set(&["a", "b"]), true, true), DockUpdate { badge: Some(Some("2".into())), bounce: false });
        assert_eq!(dock.observe(set(&[]), false, true), DockUpdate { badge: Some(None), bounce: false });
        // b waits again after leaving: new again.
        assert_eq!(dock.observe(set(&["b"]), false, true), DockUpdate { badge: Some(Some("1".into())), bounce: true });
    }

    #[test]
    fn records_the_label_shown_and_the_bounces() {
        let mut dock = Dock::default();
        assert_eq!((dock.label(), dock.bounces()), (None, 0));
        dock.observe(set(&["a"]), false, true);
        assert_eq!((dock.label(), dock.bounces()), (Some("1"), 1));
        dock.observe(set(&["a", "b"]), true, true); // in front: badge, no bounce
        assert_eq!((dock.label(), dock.bounces()), (Some("2"), 1));
        dock.observe(set(&["c"]), false, false); // bounces disabled
        assert_eq!((dock.label(), dock.bounces()), (Some("1"), 1));
        dock.observe(set(&["d"]), false, true); // same count, new waiter
        assert_eq!((dock.label(), dock.bounces()), (Some("1"), 2));
        dock.observe(set(&[]), false, true);
        assert_eq!((dock.label(), dock.bounces()), (None, 2));
    }
}
