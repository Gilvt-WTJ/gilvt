//! Watched remote files (R2a §8.1 `Watch`): the app keeps who wants which (host, path); the daemon is asked to watch a
//! path when its first watcher comes and to stop when the last goes, and again for every path after a bridge reconnects
//! (the daemon's watches belong to one bridge connection). `FileChanged` pushes go to every watcher of that path.
//!
//! A `FileChanged` can arrive before the `Watch`'s `Ok`, so a watcher is registered here before the request is sent.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gilvt_ipc::remote::{Fingerprint, RemoteRequest, RemoteResponse};
use gpui::{App, Global};

#[derive(Default)]
pub struct WatchTable {
    next: u64,
    subs: BTreeMap<u64, (String, PathBuf)>,
    /// (host, path) the daemon answered `Watch` Ok for on its current bridge connection (plan ruling 25).
    acked: BTreeSet<(String, PathBuf)>,
    /// Paths with an `Unwatch` sent and not yet answered. The daemon does not order requests for one path, so a new
    /// `Watch` for such a path waits (`deferred`) for the answer.
    unwatching: BTreeSet<(String, PathBuf)>,
    deferred: BTreeSet<(String, PathBuf)>,
    /// Bumped at every `Watch` sent and every `Unwatch` begun: an `Ok` is credited only if it answers the latest.
    generation: BTreeMap<(String, PathBuf), u64>,
    next_generation: u64,
}

impl WatchTable {
    /// A new watcher id, and whether it is the first for (host, path).
    pub fn add(&mut self, host: &str, path: &Path) -> (u64, bool) {
        let first = self.ids_for(host, path).is_empty();
        self.next += 1;
        self.subs.insert(self.next, (host.to_string(), path.to_path_buf()));
        (self.next, first)
    }

    /// (host, path, it was the last watcher of it).
    pub fn remove(&mut self, id: u64) -> Option<(String, PathBuf, bool)> {
        let (host, path) = self.subs.remove(&id)?;
        let last = self.ids_for(&host, &path).is_empty();
        if last {
            self.acked.remove(&(host.clone(), path.clone()));
        }
        Some((host, path, last))
    }

    /// The daemon accepted the `Watch`. Ignored when nobody watches the path any more (the answer came late).
    pub fn ack(&mut self, host: &str, path: &Path) {
        if !self.ids_for(host, path).is_empty() {
            self.acked.insert((host.to_string(), path.to_path_buf()));
        }
    }

    /// The bridge to `host` ended or came back: its daemon-side watches are gone (they belong to one connection).
    pub fn clear_acks(&mut self, host: &str) {
        self.acked.retain(|(h, _)| h != host);
    }

    fn bump(&mut self, host: &str, path: &Path) -> u64 {
        self.next_generation += 1;
        self.generation.insert((host.to_string(), path.to_path_buf()), self.next_generation);
        self.next_generation
    }

    /// A `Watch` for the path is wanted: Some(generation to send it under) when it may go out now, None when an
    /// `Unwatch` of it is still unanswered (the Watch is queued and comes out of [`Self::unwatch_answered`]).
    pub fn watch_now(&mut self, host: &str, path: &Path) -> Option<u64> {
        let key = (host.to_string(), path.to_path_buf());
        if self.unwatching.contains(&key) {
            self.deferred.insert(key);
            return None;
        }
        Some(self.bump(host, path))
    }

    /// The last watcher of the path left: true when an `Unwatch` should be sent (false: one is already in flight, and
    /// any queued `Watch` is dropped). Acknowledgements of earlier sends no longer count.
    pub fn begin_unwatch(&mut self, host: &str, path: &Path) -> bool {
        let key = (host.to_string(), path.to_path_buf());
        self.deferred.remove(&key);
        self.bump(host, path);
        self.unwatching.insert(key)
    }

    /// The `Unwatch` was answered (or failed): the generation to send a queued `Watch` under, if one is still wanted.
    pub fn unwatch_answered(&mut self, host: &str, path: &Path) -> Option<u64> {
        let key = (host.to_string(), path.to_path_buf());
        self.unwatching.remove(&key);
        if self.deferred.remove(&key) && !self.ids_for(host, path).is_empty() {
            return Some(self.bump(host, path));
        }
        None
    }

    /// [`Self::ack`] for the answer to the `Watch` sent under `generation`; an older one is ignored.
    pub fn ack_if_current(&mut self, host: &str, path: &Path, generation: u64) {
        if self.generation.get(&(host.to_string(), path.to_path_buf())) == Some(&generation) {
            self.ack(host, path);
        }
    }

    pub fn ids_for(&self, host: &str, path: &Path) -> Vec<u64> {
        self.subs.iter().filter(|(_, (h, p))| h == host && p == path).map(|(id, _)| *id).collect()
    }

    pub fn paths_of(&self, host: &str) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.subs.values().filter(|(h, _)| h == host).map(|(_, p)| p.clone()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Watches the daemon has acknowledged: what DebugState's `hosts[].watches` reports (a test waiting for it knows
    /// the daemon has taken its baseline).
    pub fn count(&self, host: &str) -> usize {
        self.acked.iter().filter(|(h, _)| h == host).count()
    }
}

type OnChange = Rc<RefCell<dyn FnMut(Option<Fingerprint>, &mut App)>>;

#[derive(Default)]
pub struct RemoteWatches {
    table: WatchTable,
    callbacks: HashMap<u64, OnChange>,
}
impl Global for RemoteWatches {}

/// Asks the daemon to watch `path`, unless an `Unwatch` of it is unanswered (then it goes out when that is answered).
fn send_watch(host: &str, path: PathBuf, cx: &mut App) {
    let Some(generation) = cx.global_mut::<RemoteWatches>().table.watch_now(host, &path) else { return };
    // A refused Watch (64 already, bridge down) only costs live updates: the view still shows what it loaded, and the
    // path is not counted as acknowledged.
    let (h, p) = (host.to_string(), path.clone());
    super::bridge::request(
        host,
        RemoteRequest::Watch { path },
        super::fs::WATCH_TIMEOUT,
        Box::new(move |r, cx| {
            if matches!(r, Ok(RemoteResponse::Ok)) && cx.has_global::<RemoteWatches>() {
                cx.global_mut::<RemoteWatches>().table.ack_if_current(&h, &p, generation);
            }
        }),
        cx,
    );
}

fn send_unwatch(host: &str, path: PathBuf, cx: &mut App) {
    if !cx.global_mut::<RemoteWatches>().table.begin_unwatch(host, &path) {
        return;
    }
    let (h, p) = (host.to_string(), path.clone());
    super::bridge::request(
        host,
        RemoteRequest::Unwatch { path },
        super::fs::WATCH_TIMEOUT,
        Box::new(move |_, cx| {
            if !cx.has_global::<RemoteWatches>() {
                return;
            }
            // Answered or failed, the Unwatch is over: a Watch that was waiting for it may go.
            let queued = cx.global_mut::<RemoteWatches>().table.unwatch_answered(&h, &p);
            if queued.is_some() {
                send_watch(&h, p, cx);
            }
        }),
        cx,
    );
}

#[allow(dead_code)] // Markdown / editor tasks.
pub fn watch(host: &str, path: PathBuf, on_change: impl FnMut(Option<Fingerprint>, &mut App) + 'static, cx: &mut App) -> u64 {
    if !cx.has_global::<RemoteWatches>() {
        return 0;
    }
    let (id, first) = cx.global_mut::<RemoteWatches>().table.add(host, &path);
    cx.global_mut::<RemoteWatches>().callbacks.insert(id, Rc::new(RefCell::new(on_change)));
    if first {
        send_watch(host, path, cx);
    }
    id
}

#[allow(dead_code)] // Markdown / editor tasks.
pub fn unwatch(id: u64, cx: &mut App) {
    if !cx.has_global::<RemoteWatches>() {
        return;
    }
    let w = cx.global_mut::<RemoteWatches>();
    w.callbacks.remove(&id);
    if let Some((host, path, true)) = w.table.remove(id) {
        send_unwatch(&host, path, cx);
    }
}

pub fn file_changed(host: &str, path: &Path, fingerprint: Option<Fingerprint>, cx: &mut App) {
    let Some(w) = cx.try_global::<RemoteWatches>() else { return };
    // Collected first: a callback may unwatch (its view closed) while the others still run.
    let callbacks: Vec<(u64, OnChange)> = w.table.ids_for(host, path).iter().filter_map(|id| w.callbacks.get(id).map(|cb| (*id, cb.clone()))).collect();
    for (id, cb) in callbacks {
        // An earlier callback may have closed this watcher's view.
        if !cx.try_global::<RemoteWatches>().is_some_and(|w| w.callbacks.contains_key(&id)) {
            continue;
        }
        (cb.borrow_mut())(fingerprint.clone(), cx);
    }
}

/// A new bridge connection is up: ask the daemon to watch everything the app still wants (acknowledgements start again).
pub fn rewatch(host: &str, cx: &mut App) {
    if !cx.has_global::<RemoteWatches>() {
        return;
    }
    cx.global_mut::<RemoteWatches>().table.clear_acks(host);
    for path in cx.global::<RemoteWatches>().table.paths_of(host) {
        send_watch(host, path, cx);
    }
}

/// The bridge to `host` ended: the daemon's watches are gone, so none of them is acknowledged any more.
pub fn bridge_down(host: &str, cx: &mut App) {
    if cx.has_global::<RemoteWatches>() {
        cx.global_mut::<RemoteWatches>().table.clear_acks(host);
    }
}

#[allow(dead_code)] // DebugState hosts[].watches (Task 13).
pub fn count(host: &str, cx: &App) -> usize {
    cx.try_global::<RemoteWatches>().map_or(0, |w| w.table.count(host))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_watcher_watches_and_the_last_unwatches() {
        let mut t = WatchTable::default();
        let (a, first) = t.add("h", Path::new("/f"));
        assert!(first);
        let (b, first) = t.add("h", Path::new("/f"));
        assert!(!first, "already watched on that host");
        let (c, first) = t.add("other", Path::new("/f"));
        assert!(first, "the same path on another host is another file");
        assert_eq!(t.ids_for("h", Path::new("/f")), vec![a, b]);
        assert_eq!((t.count("h"), t.paths_of("h")), (0, vec![PathBuf::from("/f")]), "wanted, not yet acknowledged by the daemon");
        t.ack("h", Path::new("/f"));
        t.ack("h", Path::new("/never-watched"));
        assert_eq!(t.count("h"), 1, "only an acknowledged watch counts; an ack for an unwatched path is ignored");
        t.clear_acks("h");
        assert_eq!((t.count("h"), t.paths_of("h").len()), (0, 1), "the bridge went down: the daemon forgot, the app still wants it");
        t.ack("h", Path::new("/f"));
        assert_eq!(t.remove(a), Some(("h".into(), PathBuf::from("/f"), false)));
        assert_eq!(t.count("h"), 1);
        assert_eq!(t.remove(b), Some(("h".into(), PathBuf::from("/f"), true)));
        assert_eq!(t.remove(b), None);
        assert_eq!(t.count("h"), 0);
        t.ack("h", Path::new("/f"));
        assert_eq!(t.count("h"), 0, "nobody watches it any more");
        assert_eq!(t.ids_for("other", Path::new("/f")), vec![c]);
    }

    #[test]
    fn a_watch_waits_for_the_unwatch_of_the_same_path_and_stale_oks_are_ignored() {
        let (h, p) = ("h", Path::new("/f"));
        let mut t = WatchTable::default();
        let (a, _) = t.add(h, p);
        let g1 = t.watch_now(h, p).expect("nothing pending: send now");
        t.ack_if_current(h, p, g1);
        assert_eq!(t.count(h), 1);
        assert_eq!(t.remove(a).map(|r| r.2), Some(true));
        assert!(t.begin_unwatch(h, p), "first unwatch in flight: send it");
        assert_eq!(t.count(h), 0);
        let (_b, first) = t.add(h, p);
        assert!(first);
        assert_eq!(t.watch_now(h, p), None, "the Unwatch has not been answered: the Watch is queued");
        t.ack_if_current(h, p, g1);
        assert_eq!(t.count(h), 0, "an Ok from before the Unwatch is not credited");
        let g2 = t.unwatch_answered(h, p).expect("queued Watch goes out now");
        assert_ne!(g1, g2);
        t.ack_if_current(h, p, g1);
        assert_eq!(t.count(h), 0, "still stale");
        t.ack_if_current(h, p, g2);
        assert_eq!(t.count(h), 1);
    }

    #[test]
    fn a_queued_watch_is_dropped_when_its_watcher_leaves_first() {
        let (h, p) = ("h", Path::new("/f"));
        let mut t = WatchTable::default();
        let (a, _) = t.add(h, p);
        t.remove(a);
        assert!(t.begin_unwatch(h, p));
        let (b, _) = t.add(h, p);
        assert_eq!(t.watch_now(h, p), None);
        t.remove(b);
        assert!(!t.begin_unwatch(h, p), "one Unwatch is already in flight");
        assert_eq!(t.unwatch_answered(h, p), None, "nobody wants it any more");
    }
}
