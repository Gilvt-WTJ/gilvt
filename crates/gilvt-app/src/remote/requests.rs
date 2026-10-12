//! Requests on a bridge (spec §5): ids and the replies waiting for them. Pure; `bridge` does the IO and timers.

use std::collections::BTreeMap;

#[allow(dead_code)] // used by bridge::request, whose callers arrive in R2a Task 10+
pub struct Pending<R> {
    next: u64,
    waiting: BTreeMap<u64, R>,
}

impl<R> Default for Pending<R> {
    fn default() -> Self {
        Pending { next: 0, waiting: BTreeMap::new() }
    }
}

#[allow(dead_code)]
impl<R> Pending<R> {
    pub fn add(&mut self, r: R) -> u64 {
        self.next += 1;
        self.waiting.insert(self.next, r);
        self.next
    }

    pub fn take(&mut self, id: u64) -> Option<R> {
        self.waiting.remove(&id)
    }

    /// The bridge went away: every reply still waiting.
    pub fn drain(&mut self) -> Vec<R> {
        std::mem::take(&mut self.waiting).into_values().collect()
    }

    pub fn len(&self) -> usize {
        self.waiting.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_answered_once() {
        let mut p = Pending::default();
        let a = p.add("a");
        let b = p.add("b");
        assert_ne!(a, b);
        assert_eq!(p.take(a), Some("a"));
        assert_eq!(p.take(a), None, "a late duplicate answer finds nothing");
        assert_eq!(p.drain(), vec!["b"]);
        assert_eq!(p.len(), 0);
    }
}
