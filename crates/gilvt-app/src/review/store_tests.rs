use std::fs;
use std::time::{Duration, SystemTime};

use gilvt_agent::{AgentKind, ReviewState, SessionKey, TurnCursor};

use super::ReviewStore;

fn key(agent: AgentKind, id: &str) -> SessionKey {
    (agent, id.to_string())
}

fn claude(id: &str) -> TurnCursor {
    TurnCursor::Claude {
        prompt_uuid: id.to_string(),
    }
}

fn codex(id: &str) -> TurnCursor {
    TurnCursor::Codex {
        turn_id: id.to_string(),
    }
}

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

#[test]
fn first_complete_refresh_baselines_existing_turns_once() {
    let mut store = ReviewStore::in_memory();
    let a = key(AgentKind::Claude, "a");
    let b = key(AgentKind::Codex, "b");
    assert!(store
        .initialize_baseline(
            at(100),
            [(a.clone(), Some(claude("p2"))), (b.clone(), None)]
        )
        .unwrap());
    assert!(store.baseline_complete());
    assert_eq!(store.initialized_at(), Some(at(100)));
    assert_eq!(store.state(&a).reviewed_through, Some(claude("p2")));
    assert_eq!(
        store.state(&b),
        ReviewState {
            baseline_reconciled: true,
            ..ReviewState::default()
        }
    );

    assert!(!store
        .initialize_baseline(at(200), [(a.clone(), Some(claude("p3")))])
        .unwrap());
    assert_eq!(store.initialized_at(), Some(at(100)));
    assert_eq!(store.state(&a).reviewed_through, Some(claude("p2")));
}

#[test]
fn late_old_sessions_are_baselined_but_new_sessions_are_not() {
    let mut store = ReviewStore::in_memory();
    store.initialize_baseline(at(100), []).unwrap();
    let old = key(AgentKind::Claude, "old");
    let new = key(AgentKind::Codex, "new");
    store.set_pinned(&old, true).unwrap();
    assert!(store
        .reconcile_baseline([
            (old.clone(), Some(claude("before-init"))),
            (new.clone(), None)
        ])
        .unwrap());
    assert_eq!(
        store.state(&old).reviewed_through,
        Some(claude("before-init"))
    );
    assert_eq!(store.state(&old).reviewed_at, Some(at(100)));
    assert!(store.state(&old).pinned);
    assert_eq!(
        store.state(&new),
        ReviewState {
            baseline_reconciled: true,
            ..ReviewState::default()
        }
    );
    assert!(!store
        .reconcile_baseline([(old.clone(), Some(claude("later")))])
        .unwrap());
}

#[test]
fn review_cursor_only_moves_forward_and_clears_snooze() {
    let mut store = ReviewStore::in_memory();
    let key = key(AgentKind::Codex, "s");
    let order = [codex("t1"), codex("t2"), codex("t3")];
    store.set_snoozed_until(&key, Some(at(500))).unwrap();
    assert!(store
        .advance_reviewed(&key, codex("t2"), &order, at(200))
        .unwrap());
    assert_eq!(store.state(&key).reviewed_through, Some(codex("t2")));
    assert_eq!(store.state(&key).snoozed_until, None);
    assert!(!store
        .advance_reviewed(&key, codex("t1"), &order, at(300))
        .unwrap());
    assert_eq!(store.state(&key).reviewed_at, Some(at(200)));
    assert!(store
        .advance_reviewed(&key, codex("missing"), &order, at(300))
        .is_err());
}

#[test]
fn stale_saved_cursor_requires_explicit_recovery() {
    let mut store = ReviewStore::in_memory();
    let key = key(AgentKind::Claude, "s");
    store
        .initialize_baseline(at(100), [(key.clone(), Some(claude("old")))])
        .unwrap();
    let error = store
        .advance_reviewed(&key, claude("new"), &[claude("new")], at(200))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(store.state(&key).reviewed_through, Some(claude("old")));
    store
        .reset_reviewed(&key, Some(claude("new")), at(200))
        .unwrap();
    assert_eq!(store.state(&key).reviewed_through, Some(claude("new")));
}

#[test]
fn explicit_reset_to_the_beginning_is_not_baselined_again() {
    let mut store = ReviewStore::in_memory();
    let key = key(AgentKind::Claude, "reset");
    store
        .initialize_baseline(at(100), [(key.clone(), Some(claude("old")))])
        .unwrap();
    store.reset_reviewed(&key, None, at(200)).unwrap();
    assert!(store.state(&key).baseline_reconciled);
    assert_eq!(store.state(&key).reviewed_through, None);

    assert!(!store
        .reconcile_baseline([(key.clone(), Some(claude("old")))])
        .unwrap());
    assert_eq!(store.state(&key).reviewed_through, None);
}

#[test]
fn preferences_and_cursors_survive_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let key = key(AgentKind::Claude, "s");
    let mut store = ReviewStore::open(dir.path());
    assert_eq!(
        store.path(),
        Some(dir.path().join("reviews.json").as_path())
    );
    store.initialize_baseline(at(100), []).unwrap();
    store.set_pinned(&key, true).unwrap();
    store.set_snoozed_until(&key, Some(at(300))).unwrap();
    store
        .advance_reviewed(&key, claude("p1"), &[claude("p1")], at(200))
        .unwrap();

    let reopened = ReviewStore::open(dir.path());
    assert!(reopened.baseline_complete());
    assert_eq!(reopened.state(&key).reviewed_through, Some(claude("p1")));
    assert!(reopened.state(&key).pinned);
    assert_eq!(reopened.state(&key).snoozed_until, None);
    assert!(!dir.path().join("reviews.json.tmp").exists());
}

#[test]
fn corrupt_file_is_preserved_before_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reviews.json");
    fs::write(&path, "{broken").unwrap();
    let mut store = ReviewStore::open(dir.path());
    assert!(store.load_error().is_some());
    store.set_pinned(&key(AgentKind::Codex, "s"), true).unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("reviews.json.corrupt")).unwrap(),
        "{broken"
    );
    assert!(ReviewStore::open(dir.path()).load_error().is_none());
}

#[test]
fn failed_save_does_not_change_memory() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("not-a-directory");
    fs::write(&state_path, "file").unwrap();
    let mut store = ReviewStore::open(&state_path);
    let key = key(AgentKind::Claude, "s");
    assert!(store.set_pinned(&key, true).is_err());
    assert_eq!(store.state(&key), ReviewState::default());
}
