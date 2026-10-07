use std::cell::Cell;

use gilvt_agent::AgentKind;

use super::pending::{PendingCache, PendingKey};

fn key(generation: u64, revision: u64, minute: u64, live: &[(&str, bool)]) -> PendingKey {
    PendingKey::new(
        generation,
        revision,
        minute,
        live.iter()
            .map(|(id, needs_you)| ((AgentKind::Claude, id.to_string()), *needs_you))
            .collect(),
    )
}

/// What `pending_review_count` does: the cached count when the inputs are unchanged, else compute and store.
fn count(cache: &mut PendingCache, key: PendingKey, calls: &Cell<u32>, value: usize) -> usize {
    if let Some(cached) = cache.get(&key) {
        return cached;
    }
    calls.set(calls.get() + 1);
    cache.put(key, value);
    value
}

#[test]
fn the_same_inputs_are_computed_once() {
    let (mut cache, calls) = (PendingCache::default(), Cell::new(0));
    assert_eq!(
        count(&mut cache, key(1, 1, 5, &[("a", false)]), &calls, 3),
        3
    );
    // A different value from `compute` proves the cached one was returned.
    assert_eq!(
        count(&mut cache, key(1, 1, 5, &[("a", false)]), &calls, 9),
        3
    );
    assert_eq!(calls.get(), 1);
}

#[test]
fn every_input_invalidates_the_count() {
    let calls = Cell::new(0);
    let base = || key(1, 1, 5, &[("a", false)]);
    for changed in [
        key(2, 1, 5, &[("a", false)]),
        key(1, 2, 5, &[("a", false)]),
        key(1, 1, 6, &[("a", false)]),
        key(1, 1, 5, &[("a", true)]),
        key(1, 1, 5, &[("a", false), ("b", false)]),
    ] {
        let mut cache = PendingCache::default();
        count(&mut cache, base(), &calls, 1);
        let before = calls.get();
        count(&mut cache, changed, &calls, 2);
        assert_eq!(calls.get(), before + 1);
    }
}

#[test]
fn the_last_count_is_what_was_drawn_most_recently() {
    let (mut cache, calls) = (PendingCache::default(), Cell::new(0));
    assert_eq!(cache.last(), 0, "nothing drawn yet");
    count(&mut cache, key(1, 1, 5, &[]), &calls, 4);
    assert_eq!(cache.last(), 4);
    count(&mut cache, key(2, 1, 5, &[]), &calls, 1);
    assert_eq!(cache.last(), 1);
}

#[test]
fn the_live_set_is_compared_without_regard_to_order() {
    let (mut cache, calls) = (PendingCache::default(), Cell::new(0));
    count(
        &mut cache,
        key(1, 1, 5, &[("a", false), ("b", true)]),
        &calls,
        1,
    );
    count(
        &mut cache,
        key(1, 1, 5, &[("b", true), ("a", false)]),
        &calls,
        1,
    );
    assert_eq!(calls.get(), 1);
}
