//! Watched remote files (R2a §8.1 `Watch`): the app keeps who wants which (host, path); the daemon is asked to watch a
//! path when its first watcher comes and to stop when the last goes, and again for every path after a bridge reconnects
//! (the daemon's watches belong to one bridge connection). `FileChanged` pushes go to every watcher of that path.
//!
//! A `FileChanged` can arrive before the `Watch`'s `Ok`, so a watcher is registered here before the request is sent.
//!
//! The daemon takes its baseline when it handles `Watch`, so a change made while the bridge was down, or between a
//! view's first read and its `Watch`, is never pushed (R2a §9.4). Every watcher therefore keeps what it last knew of
//! the file (`Known`: from its read's fingerprint and from `FileChanged`), and once the daemon has acknowledged the
//! watch (after a reconnect: all of them, in one batched `Stat`) the app compares: a watcher whose size or mtime no
//! longer matches is told as if `FileChanged` had come, with a fingerprint that has no hash.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gilvt_ipc::remote::{Fingerprint, RemoteRequest, RemoteResponse, StatEntry};
use gpui::{App, Global};

/// What a watcher last knew of its file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Known {
    /// Nothing read yet (a preview whose load has not answered): nothing to compare.
    Unknown,
    Missing,
    File { size: u64, mtime_ns: u64 },
}

impl Known {
    pub fn of(fp: Option<&Fingerprint>) -> Known {
        fp.map_or(Known::Missing, |f| Known::File { size: f.size, mtime_ns: f.mtime_ns })
    }

    fn of_stat(e: &StatEntry) -> Known {
        if e.exists && e.is_file { Known::File { size: e.size, mtime_ns: e.mtime_ns } } else { Known::Missing }
    }
}

/// Whether a watcher that knew `known` missed a change, given what `Stat` says now: Some(what to tell it) — a
/// fingerprint with size and mtime only (no hash), or None for a file that is gone.
pub fn missed(known: Known, now: &StatEntry) -> Option<Option<Fingerprint>> {
    if known == Known::Unknown || now.error.is_some() {
        return None;
    }
    match Known::of_stat(now) {
        k if k == known => None,
        Known::File { size, mtime_ns } => Some(Some(Fingerprint { mtime_ns, size, sha256: String::new() })),
        _ => Some(None),
    }
}

struct Sub {
    host: String,
    path: PathBuf,
    known: Known,
    /// Bumped whenever `known` changes: a `Stat` answer is used only if nothing newer came meanwhile.
    seq: u64,
    /// Compared with the host since the daemon's latest acknowledgement of the path.
    checked: bool,
}

/// One watcher whose file is being compared by a `Stat`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub id: u64,
    pub path: PathBuf,
    seq: u64,
}

#[derive(Default)]
pub struct WatchTable {
    next: u64,
    subs: BTreeMap<u64, Sub>,
    /// (host, path) the daemon answered `Watch` Ok for on its current bridge connection (plan ruling 25).
    acked: BTreeSet<(String, PathBuf)>,
    /// Paths with an `Unwatch` sent and not yet answered; a new `Watch` for such a path waits (`deferred`) for the
    /// answer. Defensive: the daemon handles `Watch`/`Unwatch` inline, in the order they arrive, and the bridge writer
    /// is FIFO, so the two cannot actually be reordered today; this keeps it correct if either ever changes.
    unwatching: BTreeSet<(String, PathBuf)>,
    deferred: BTreeSet<(String, PathBuf)>,
    /// Bumped at every `Watch` sent and every `Unwatch` begun: an `Ok` is credited only if it answers the latest.
    generation: BTreeMap<(String, PathBuf), u64>,
    next_generation: u64,
}

impl WatchTable {
    /// A new watcher id, and whether it is the first for (host, path). `known`: what its read found, if any yet.
    pub fn add(&mut self, host: &str, path: &Path, known: Known) -> (u64, bool) {
        let first = self.ids_for(host, path).is_empty();
        self.next += 1;
        self.subs.insert(self.next, Sub { host: host.to_string(), path: path.to_path_buf(), known, seq: 0, checked: false });
        (self.next, first)
    }

    /// (host, path, it was the last watcher of it).
    pub fn remove(&mut self, id: u64) -> Option<(String, PathBuf, bool)> {
        let Sub { host, path, .. } = self.subs.remove(&id)?;
        let last = self.ids_for(&host, &path).is_empty();
        if last {
            self.acked.remove(&(host.clone(), path.clone()));
        }
        Some((host, path, last))
    }

    /// The watcher read the file (or saved it): this is what the host has now.
    pub fn note(&mut self, id: u64, known: Known) -> Option<(String, PathBuf)> {
        let sub = self.subs.get_mut(&id)?;
        sub.known = known;
        sub.seq += 1;
        Some((sub.host.clone(), sub.path.clone()))
    }

    /// `FileChanged` was delivered to every watcher of the path: they all know it now.
    pub fn changed(&mut self, host: &str, path: &Path, fingerprint: Option<&Fingerprint>) {
        for sub in self.subs.values_mut().filter(|s| s.host == host && s.path == path) {
            sub.known = Known::of(fingerprint);
            sub.seq += 1;
        }
    }

    /// The daemon accepted the `Watch`. Ignored when nobody watches the path any more (the answer came late). Its
    /// baseline is new: every watcher of the path must be compared with the host again.
    pub fn ack(&mut self, host: &str, path: &Path) {
        if !self.ids_for(host, path).is_empty() {
            self.acked.insert((host.to_string(), path.to_path_buf()));
            for sub in self.subs.values_mut().filter(|s| s.host == host && s.path == path) {
                sub.checked = false;
            }
        }
    }

    /// Watchers of `paths` (on `host`) to compare with the host now: acknowledged, something known, not yet compared
    /// since the acknowledgement. They count as compared from here on.
    pub fn due_checks(&mut self, host: &str, paths: &[PathBuf]) -> Vec<Check> {
        let acked = &self.acked;
        self.subs
            .iter_mut()
            .filter(|(_, s)| s.host == host && paths.contains(&s.path) && !s.checked && s.known != Known::Unknown && acked.contains(&(s.host.clone(), s.path.clone())))
            .map(|(id, s)| {
                s.checked = true;
                Check { id: *id, path: s.path.clone(), seq: s.seq }
            })
            .collect()
    }

    /// The `Stat` for `checks` answered (`now`: one entry per distinct path, in [`distinct_paths`] order): the
    /// watchers that missed a change, with what to tell them (they know it from here on). A watcher that heard
    /// something newer since the `Stat` was sent, or has gone, is left alone.
    pub fn stat_answered(&mut self, checks: &[Check], paths: &[PathBuf], now: &[StatEntry]) -> Vec<(u64, Option<Fingerprint>)> {
        let mut out = Vec::new();
        for c in checks {
            let Some(entry) = paths.iter().position(|p| p == &c.path).and_then(|i| now.get(i)) else { continue };
            let Some(sub) = self.subs.get_mut(&c.id).filter(|s| s.seq == c.seq) else { continue };
            if let Some(fp) = missed(sub.known, entry) {
                sub.known = Known::of(fp.as_ref());
                sub.seq += 1;
                out.push((c.id, fp));
            }
        }
        out
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
        self.subs.iter().filter(|(_, s)| s.host == host && s.path == path).map(|(id, _)| *id).collect()
    }

    pub fn paths_of(&self, host: &str) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.subs.values().filter(|s| s.host == host).map(|s| s.path.clone()).collect();
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

/// The paths of `checks`, each once, in order: what one `Stat` asks about.
pub fn distinct_paths(checks: &[Check]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    for c in checks {
        if !v.contains(&c.path) {
            v.push(c.path.clone());
        }
    }
    v
}

/// The `Watch`es sent for one reconnect: when the last has answered, one `Stat` compares all their watchers.
struct Rewatch {
    host: String,
    remaining: usize,
    paths: Vec<PathBuf>,
}

/// Asks the daemon to watch `path`, unless an `Unwatch` of it is unanswered (then it goes out when that is answered;
/// false is returned). Once acknowledged, the path's watchers are compared with the host — at once, or with the rest
/// of `batch` when every `Watch` of that reconnect has answered.
fn send_watch(host: &str, path: PathBuf, batch: Option<Rc<RefCell<Rewatch>>>, cx: &mut App) -> bool {
    let Some(generation) = cx.global_mut::<RemoteWatches>().table.watch_now(host, &path) else { return false };
    // A refused Watch (64 already, bridge down) only costs live updates: the view still shows what it loaded, and the
    // path is not counted as acknowledged.
    let (h, p) = (host.to_string(), path.clone());
    super::bridge::request(
        host,
        RemoteRequest::Watch { path },
        super::fs::WATCH_TIMEOUT,
        Box::new(move |r, cx| {
            if !cx.has_global::<RemoteWatches>() {
                return;
            }
            if matches!(r, Ok(RemoteResponse::Ok)) {
                cx.global_mut::<RemoteWatches>().table.ack_if_current(&h, &p, generation);
            }
            match batch {
                Some(b) => {
                    let done = {
                        let mut b = b.borrow_mut();
                        b.remaining -= 1;
                        b.paths.push(p);
                        (b.remaining == 0).then(|| std::mem::take(&mut b.paths))
                    };
                    if let Some(paths) = done {
                        check(&h, paths, cx);
                    }
                }
                None => check(&h, vec![p], cx),
            }
        }),
        cx,
    );
    true
}

/// One `Stat` for the watchers of `paths` that are due a comparison; those that missed a change are told.
fn check(host: &str, paths: Vec<PathBuf>, cx: &mut App) {
    let checks = cx.global_mut::<RemoteWatches>().table.due_checks(host, &paths);
    if checks.is_empty() {
        return;
    }
    let paths = distinct_paths(&checks);
    let stat = super::fs::RemoteFs::new(host, cx).stat(paths.clone(), cx);
    cx.spawn(async move |cx| {
        // A failed Stat (the bridge went again) is retried after the next reconnect's acknowledgements.
        let Ok(now) = stat.await else { return };
        let _ = cx.update(|cx| {
            if !cx.has_global::<RemoteWatches>() {
                return;
            }
            let missed = cx.global_mut::<RemoteWatches>().table.stat_answered(&checks, &paths, &now);
            for (id, fp) in missed {
                deliver(id, fp, cx);
            }
        });
    })
    .detach();
}

/// Calls watcher `id`'s callback, unless it has gone (an earlier callback may have closed its view).
fn deliver(id: u64, fingerprint: Option<Fingerprint>, cx: &mut App) {
    let Some(cb) = cx.try_global::<RemoteWatches>().and_then(|w| w.callbacks.get(&id).cloned()) else { return };
    (cb.borrow_mut())(fingerprint, cx);
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
                send_watch(&h, p, None, cx);
            }
        }),
        cx,
    );
}

/// Watches `path` on `host`. `read`: the fingerprint the caller's read found (None when it has not read yet; tell
/// [`note`] when it does), so a change the daemon's baseline missed is still reported.
pub fn watch(host: &str, path: PathBuf, read: Option<&Fingerprint>, on_change: impl FnMut(Option<Fingerprint>, &mut App) + 'static, cx: &mut App) -> u64 {
    if !cx.has_global::<RemoteWatches>() {
        return 0;
    }
    let known = read.map_or(Known::Unknown, |f| Known::of(Some(f)));
    let (id, first) = cx.global_mut::<RemoteWatches>().table.add(host, &path, known);
    cx.global_mut::<RemoteWatches>().callbacks.insert(id, Rc::new(RefCell::new(on_change)));
    if first {
        send_watch(host, path, None, cx);
    } else {
        // The path is watched already: compared now if the daemon has acknowledged it, else when it does.
        check(host, vec![path], cx);
    }
    id
}

/// Watcher `id`'s view read (or saved) the file: `fingerprint` is what the host has now. The first time, and the
/// watch is already acknowledged, the file is compared with the host.
pub fn note(id: u64, fingerprint: Option<&Fingerprint>, cx: &mut App) {
    if !cx.has_global::<RemoteWatches>() {
        return;
    }
    if let Some((host, path)) = cx.global_mut::<RemoteWatches>().table.note(id, Known::of(fingerprint)) {
        check(&host, vec![path], cx);
    }
}

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
    if !cx.has_global::<RemoteWatches>() {
        return;
    }
    cx.global_mut::<RemoteWatches>().table.changed(host, path, fingerprint.as_ref());
    // Collected first: a callback may unwatch (its view closed) while the others still run.
    let ids = cx.global::<RemoteWatches>().table.ids_for(host, path);
    for id in ids {
        deliver(id, fingerprint.clone(), cx);
    }
}

/// A new bridge connection is up: ask the daemon to watch everything the app still wants (acknowledgements start
/// again), then compare every watcher with the host in one `Stat`: changes made while the bridge was down are not
/// pushed (the new watches take their baseline now).
pub fn rewatch(host: &str, cx: &mut App) {
    if !cx.has_global::<RemoteWatches>() {
        return;
    }
    cx.global_mut::<RemoteWatches>().table.clear_acks(host);
    let paths = cx.global::<RemoteWatches>().table.paths_of(host);
    let batch = Rc::new(RefCell::new(Rewatch { host: host.to_string(), remaining: paths.len(), paths: Vec::new() }));
    for path in paths {
        if !send_watch(host, path, Some(batch.clone()), cx) {
            // Waiting for an Unwatch: it is compared when its own Watch is answered.
            let done = {
                let mut b = batch.borrow_mut();
                b.remaining -= 1;
                (b.remaining == 0).then(|| (b.host.clone(), std::mem::take(&mut b.paths)))
            };
            if let Some((h, paths)) = done {
                check(&h, paths, cx);
            }
        }
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
        let (a, first) = t.add("h", Path::new("/f"), Known::Unknown);
        assert!(first);
        let (b, first) = t.add("h", Path::new("/f"), Known::Unknown);
        assert!(!first, "already watched on that host");
        let (c, first) = t.add("other", Path::new("/f"), Known::Unknown);
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
        let (a, _) = t.add(h, p, Known::Unknown);
        let g1 = t.watch_now(h, p).expect("nothing pending: send now");
        t.ack_if_current(h, p, g1);
        assert_eq!(t.count(h), 1);
        assert_eq!(t.remove(a).map(|r| r.2), Some(true));
        assert!(t.begin_unwatch(h, p), "first unwatch in flight: send it");
        assert_eq!(t.count(h), 0);
        let (_b, first) = t.add(h, p, Known::Unknown);
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

    fn st(size: u64, mtime_ns: u64) -> StatEntry {
        StatEntry { exists: true, is_file: true, size, mtime_ns, error: None }
    }

    fn file(size: u64, mtime_ns: u64) -> Known {
        Known::File { size, mtime_ns }
    }

    #[test]
    fn a_stat_finds_what_the_baseline_missed() {
        let unhashed = |size, mtime_ns| Some(Some(Fingerprint { mtime_ns, size, sha256: String::new() }));
        assert_eq!(missed(file(3, 5), &st(3, 5)), None, "unchanged");
        assert_eq!(missed(file(3, 5), &st(3, 6)), unhashed(3, 6), "a new mtime");
        assert_eq!(missed(file(3, 5), &st(4, 5)), unhashed(4, 5), "a new size");
        assert_eq!(missed(file(3, 5), &StatEntry::default()), Some(None), "gone");
        assert_eq!(missed(file(3, 5), &StatEntry { exists: true, is_file: false, ..StatEntry::default() }), Some(None), "now a directory");
        assert_eq!(missed(Known::Missing, &StatEntry::default()), None, "still gone");
        assert_eq!(missed(Known::Missing, &st(1, 1)), unhashed(1, 1), "back");
        assert_eq!(missed(Known::Unknown, &st(1, 1)), None, "nothing read yet: nothing to compare");
        assert_eq!(missed(file(3, 5), &StatEntry { error: Some("denied".into()), ..StatEntry::default() }), None, "a failed look is not a change");
    }

    #[test]
    fn after_a_reconnect_every_acknowledged_watcher_is_compared_once() {
        let (h, f, g) = ("h", PathBuf::from("/f"), PathBuf::from("/g"));
        let mut t = WatchTable::default();
        let (a, _) = t.add(h, &f, file(3, 5));
        let (b, _) = t.add(h, &f, file(3, 5));
        let (c, _) = t.add(h, &g, Known::Unknown); // a preview still loading
        let (_other, _) = t.add("other", &f, file(3, 5));
        assert!(t.due_checks(h, &[f.clone(), g.clone()]).is_empty(), "not acknowledged yet");
        t.ack(h, &f);
        t.ack(h, &g);
        let checks = t.due_checks(h, &[f.clone(), g.clone()]);
        assert_eq!(checks.iter().map(|c| c.id).collect::<Vec<_>>(), vec![a, b], "the loading preview has nothing to compare");
        assert_eq!(distinct_paths(&checks), vec![f.clone()], "one Stat entry per path");
        assert!(t.due_checks(h, &[f.clone()]).is_empty(), "compared once per acknowledgement");
        // the preview's load answers: compared now, the watch being acknowledged already
        t.note(c, file(1, 1));
        let late = t.due_checks(h, &[g.clone()]);
        assert_eq!(late.iter().map(|c| c.id).collect::<Vec<_>>(), vec![c]);
        // a FileChanged came while the Stat was on its way: the Stat's answer is older, nobody is told
        t.changed(h, &f, Some(&Fingerprint { mtime_ns: 9, size: 4, sha256: "ab".into() }));
        assert_eq!(t.stat_answered(&checks, &[f.clone()], &[st(3, 7)]), Vec::<(u64, Option<Fingerprint>)>::new());
        // a reconnect: compared again; the file changed meanwhile
        t.clear_acks(h);
        t.ack(h, &f);
        let checks = t.due_checks(h, &[f.clone()]);
        let fired = t.stat_answered(&checks, &[f.clone()], &[st(4, 12)]);
        assert_eq!(fired.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![a, b]);
        assert_eq!(fired[0].1, Some(Fingerprint { mtime_ns: 12, size: 4, sha256: String::new() }));
        // what they were told is what they know: the same Stat again finds nothing
        t.clear_acks(h);
        t.ack(h, &f);
        let checks = t.due_checks(h, &[f.clone()]);
        assert!(t.stat_answered(&checks, &[f.clone()], &[st(4, 12)]).is_empty());
        // a watcher that left is not told
        t.clear_acks(h);
        t.ack(h, &f);
        let checks = t.due_checks(h, &[f.clone()]);
        t.remove(a);
        assert_eq!(t.stat_answered(&checks, &[f.clone()], &[StatEntry::default()]).iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![b]);
    }

    #[test]
    fn a_queued_watch_is_dropped_when_its_watcher_leaves_first() {
        let (h, p) = ("h", Path::new("/f"));
        let mut t = WatchTable::default();
        let (a, _) = t.add(h, p, Known::Unknown);
        t.remove(a);
        assert!(t.begin_unwatch(h, p));
        let (b, _) = t.add(h, p, Known::Unknown);
        assert_eq!(t.watch_now(h, p), None);
        t.remove(b);
        assert!(!t.begin_unwatch(h, p), "one Unwatch is already in flight");
        assert_eq!(t.unwatch_answered(h, p), None, "nobody wants it any more");
    }
}
