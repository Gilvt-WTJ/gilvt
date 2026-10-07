//! The palette's logic without gpui views: keys, selection, listing cache, recent files, and how
//! paths are shown and inserted.

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gilvt_finder::{insertion, relative_to, Hit, Listing};
use gpui::Modifiers;

/// A palette key press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Up,
    Down,
    /// ⏎: Quick Look.
    Open,
    /// ⌘⏎: a pinned pane.
    Pin,
    /// ⌥⏎: the path on the command line.
    Insert,
    Close,
    Backspace,
}

/// The command for a key, or `None` when it is text (or nothing) for the query box.
pub fn command(key: &str, m: Modifiers) -> Option<Command> {
    let plain = !m.control && !m.alt && !m.platform && !m.shift;
    match key {
        "up" if plain => Some(Command::Up),
        "down" if plain => Some(Command::Down),
        "p" if m.control && !m.alt && !m.platform => Some(Command::Up),
        "n" if m.control && !m.alt && !m.platform => Some(Command::Down),
        "enter" if plain => Some(Command::Open),
        "enter" if m.platform && !m.alt && !m.control => Some(Command::Pin),
        "enter" if m.alt && !m.platform && !m.control => Some(Command::Insert),
        "escape" => Some(Command::Close),
        "backspace" if !m.platform && !m.control => Some(Command::Backspace),
        _ => None,
    }
}

/// Selection after moving one row (wrapping around) in a list of `len`.
pub fn step(selected: usize, len: usize, down: bool) -> usize {
    match (len, down) {
        (0, _) => 0,
        (_, true) => (selected + 1) % len,
        (_, false) => (selected + len - 1) % len,
    }
}

/// Selection once a refreshed listing produced `hits`: the same file when it is still there.
pub fn reselect(previous: Option<&str>, hits: &[Hit]) -> usize {
    previous.and_then(|p| hits.iter().position(|h| h.path == p)).unwrap_or(0)
}

/// `path` relative to `root`, '/'-separated; `None` outside it.
pub fn rel_to_root(path: &Path, root: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
    Some(parts?.join("/"))
}

/// A hit's path split into the directory part (trailing '/' included, as `Hit::dir_matches`
/// indexes it) and the file name.
pub fn split_path(path: &str) -> (&str, &str) {
    let name = path.rfind('/').map_or(0, |i| i + 1);
    path.split_at(name)
}

/// Byte ranges of the chars at `indices` in `text`, adjacent chars merged. `indices` must be
/// strictly ascending, as `Hit`'s are.
pub fn byte_ranges(text: &str, indices: &[usize]) -> Vec<Range<usize>> {
    debug_assert!(indices.windows(2).all(|w| w[0] < w[1]), "indices not ascending: {indices:?}");
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut wanted = indices.iter().copied().peekable();
    for (ci, (bi, c)) in text.char_indices().enumerate() {
        if wanted.peek() != Some(&ci) {
            continue;
        }
        wanted.next();
        let end = bi + c.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == bi => last.end = end,
            _ => ranges.push(bi..end),
        }
    }
    ranges
}

/// Listings up to this many files are ranked on the main thread, so results follow each key at
/// once (a few ms in debug builds); larger ones are ranked on the background executor.
pub const SYNC_RANK_MAX: usize = 20_000;

pub fn ranks_in_background(files: usize) -> bool {
    files > SYNC_RANK_MAX
}

/// The file to keep selected once the results of the latest search arrive: `requested` by the new
/// search (`None` after a query change: the first hit). While an earlier search still runs the hits
/// shown have not changed, so a pending request for the first hit wins.
pub fn keep_selected(running: bool, pending: Option<String>, requested: Option<String>) -> Option<String> {
    if running {
        pending.and(requested)
    } else {
        requested
    }
}

/// Whether ⏎ / ⌘⏎ / ⌥⏎ must wait: the hits shown are from search `shown`, older than the latest
/// search `latest` (still running in the background), so they may not match the query typed.
pub fn accept_waits(shown: u64, latest: u64) -> bool {
    shown < latest
}

/// Whether no hits for the current listing have been shown yet: its first search (`listed`) is
/// still running, so an empty list means "not ranked yet", not "no match".
pub fn awaiting_first_hits(shown: u64, listed: u64) -> bool {
    shown < listed
}

/// Banner for a selected file that cannot be acted on: gone since it was listed, or not a regular
/// file (e.g. a listed symlink to a directory).
pub fn unusable_banner(rel: &str, path: &Path) -> Option<String> {
    if !path.exists() {
        Some(format!("找不到 {rel}"))
    } else if !path.is_file() {
        Some(format!("{rel} 不是文件"))
    } else {
        None
    }
}

/// Text ⌥⏎ inserts for `file`: its path relative to the pane's `cwd`, as a shell word.
pub fn insert_text(file: &Path, cwd: &Path) -> String {
    insertion(&[relative_to(file, cwd).to_string_lossy().into_owned()])
}

/// Files recently opened in Quick Look, most recent first.
#[derive(Clone, Debug, Default)]
pub struct Recent(VecDeque<PathBuf>);

impl Recent {
    pub const CAP: usize = 50;

    pub fn push(&mut self, path: PathBuf) {
        self.0.retain(|p| p != &path);
        self.0.push_front(path);
        self.0.truncate(Self::CAP);
    }

    /// The ones under `root`, relative to it (the ranking's `Context::recent`).
    pub fn under(&self, root: &Path) -> Vec<String> {
        self.0.iter().filter_map(|p| rel_to_root(p, root)).collect()
    }
}

/// Listings by search root, least recently used dropped first.
#[derive(Debug, Default)]
pub struct ListingCache(VecDeque<(PathBuf, Arc<Listing>)>);

impl ListingCache {
    /// A listing of a large repository is a few MB.
    pub const CAP: usize = 8;

    pub fn get(&mut self, root: &Path) -> Option<Arc<Listing>> {
        let i = self.0.iter().position(|(r, _)| r == root)?;
        let entry = self.0.remove(i)?;
        let listing = entry.1.clone();
        self.0.push_front(entry);
        Some(listing)
    }

    pub fn insert(&mut self, root: PathBuf, listing: Arc<Listing>) {
        self.0.retain(|(r, _)| r != &root);
        self.0.push_front((root, listing));
        self.0.truncate(Self::CAP);
    }
}

/// Listings running now, by search root, with the palettes waiting for each. At most one listing
/// runs per root: a palette opened while one is running waits for it instead of starting another.
#[derive(Debug)]
pub struct InFlight<W>(HashMap<PathBuf, Vec<W>>);

impl<W> Default for InFlight<W> {
    fn default() -> Self {
        InFlight(HashMap::new())
    }
}

impl<W> InFlight<W> {
    /// Adds `waiter` to the listing of `root`; `true` when none was running, so the caller starts it.
    pub fn join(&mut self, root: &Path, waiter: W) -> bool {
        match self.0.get_mut(root) {
            Some(waiters) => {
                waiters.push(waiter);
                false
            }
            None => {
                self.0.insert(root.to_path_buf(), vec![waiter]);
                true
            }
        }
    }

    /// The listing of `root` finished: who was waiting for it (the next `join` starts a new one).
    pub fn finish(&mut self, root: &Path) -> Vec<W> {
        self.0.remove(root).unwrap_or_default()
    }
}

/// Whether the palette opened from terminal `origin` stays open: that pane still exists
/// (`alive`) and is among the panes the active tab `shown` (only the focused one when zoomed).
/// Otherwise ⌥⏎ would paste into a terminal the user cannot see.
pub fn palette_stays<P: PartialEq>(origin: P, alive: bool, shown: &[P]) -> bool {
    alive && shown.contains(&origin)
}

/// A refreshed listing, unless it matches the cached one.
pub fn changed(cached: Option<&Listing>, fresh: Listing) -> Option<Listing> {
    (cached != Some(&fresh)).then_some(fresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str) -> Hit {
        Hit { path: path.into(), score: 0, name_matches: Vec::new(), dir_matches: Vec::new(), changed: false }
    }

    #[test]
    fn keys() {
        let none = Modifiers::default();
        let ctrl = Modifiers { control: true, ..none };
        let cmd = Modifiers { platform: true, ..none };
        let alt = Modifiers { alt: true, ..none };
        let shift = Modifiers { shift: true, ..none };
        assert_eq!(command("up", none), Some(Command::Up));
        assert_eq!(command("down", none), Some(Command::Down));
        assert_eq!(command("p", ctrl), Some(Command::Up));
        assert_eq!(command("n", ctrl), Some(Command::Down));
        assert_eq!(command("enter", none), Some(Command::Open));
        assert_eq!(command("enter", cmd), Some(Command::Pin));
        assert_eq!(command("enter", alt), Some(Command::Insert));
        assert_eq!(command("escape", none), Some(Command::Close));
        assert_eq!(command("backspace", none), Some(Command::Backspace));
        // Text goes to the query box: plain letters, ⌥ letters (e.g. å), shifted letters.
        assert_eq!(command("p", none), None);
        assert_eq!(command("n", alt), None);
        assert_eq!(command("a", shift), None);
        // Other chords are left alone.
        assert_eq!(command("enter", shift), None);
        assert_eq!(command("enter", Modifiers { alt: true, ..cmd }), None);
        assert_eq!(command("backspace", cmd), None);
    }

    #[test]
    fn selection_wraps() {
        assert_eq!(step(0, 3, true), 1);
        assert_eq!(step(2, 3, true), 0);
        assert_eq!(step(0, 3, false), 2);
        assert_eq!(step(0, 0, true), 0);
        assert_eq!(step(0, 0, false), 0);
    }

    #[test]
    fn refresh_keeps_the_selected_file() {
        let hits = [hit("a.rs"), hit("b.rs"), hit("c.rs")];
        assert_eq!(reselect(Some("c.rs"), &hits), 2);
        assert_eq!(reselect(Some("gone.rs"), &hits), 0);
        assert_eq!(reselect(None, &hits), 0);
    }

    #[test]
    fn large_listings_rank_in_the_background() {
        assert!(!ranks_in_background(0));
        assert!(!ranks_in_background(SYNC_RANK_MAX));
        assert!(ranks_in_background(SYNC_RANK_MAX + 1));
    }

    #[test]
    fn selection_kept_across_background_searches() {
        let s = |p: &str| Some(p.to_string());
        // Nothing running: the new search decides.
        assert_eq!(keep_selected(false, s("old.rs"), s("a.rs")), s("a.rs"));
        assert_eq!(keep_selected(false, s("old.rs"), None), None);
        // A listing refresh while one runs keeps what was selected.
        assert_eq!(keep_selected(true, s("a.rs"), s("a.rs")), s("a.rs"));
        // A query change while a refresh runs, or a refresh while a query change runs: first hit.
        assert_eq!(keep_selected(true, s("a.rs"), None), None);
        assert_eq!(keep_selected(true, None, s("a.rs")), None);
    }

    #[test]
    fn confirming_waits_for_the_latest_hits() {
        assert!(!accept_waits(0, 0));
        assert!(!accept_waits(3, 3));
        assert!(accept_waits(2, 3));
        // The first search of a listing, before any hits.
        assert!(awaiting_first_hits(0, 1));
        assert!(!awaiting_first_hits(1, 1));
        // Later keystrokes on the same listing keep the shown (e.g. empty) hits meaningful.
        assert!(!awaiting_first_hits(4, 1));
    }

    #[test]
    fn banners_for_unusable_selections() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        std::fs::write(d.join("a.rs"), "").unwrap();
        std::fs::create_dir(d.join("dir")).unwrap();
        std::os::unix::fs::symlink(d.join("a.rs"), d.join("to-file")).unwrap();
        std::os::unix::fs::symlink(d.join("dir"), d.join("to-dir")).unwrap();
        std::os::unix::fs::symlink(d.join("nowhere"), d.join("broken")).unwrap();
        assert_eq!(unusable_banner("a.rs", &d.join("a.rs")), None);
        assert_eq!(unusable_banner("to-file", &d.join("to-file")), None);
        assert_eq!(unusable_banner("to-dir", &d.join("to-dir")).as_deref(), Some("to-dir 不是文件"));
        assert_eq!(unusable_banner("broken", &d.join("broken")).as_deref(), Some("找不到 broken"));
        assert_eq!(unusable_banner("gone.rs", &d.join("gone.rs")).as_deref(), Some("找不到 gone.rs"));
    }

    #[test]
    fn paths_relative_to_the_root() {
        assert_eq!(rel_to_root(Path::new("/r/a/b.rs"), Path::new("/r")).as_deref(), Some("a/b.rs"));
        assert_eq!(rel_to_root(Path::new("/r"), Path::new("/r")).as_deref(), Some(""));
        assert_eq!(rel_to_root(Path::new("/other/b.rs"), Path::new("/r")), None);
    }

    #[test]
    fn splits_hit_paths() {
        assert_eq!(split_path("src/finder/mod.rs"), ("src/finder/", "mod.rs"));
        assert_eq!(split_path("README.md"), ("", "README.md"));
    }

    #[test]
    fn highlight_ranges_are_char_aware() {
        assert_eq!(byte_ranges("main.rs", &[0, 1, 5]), vec![0..2, 5..6]);
        // 中 and 文 are 3 bytes each.
        assert_eq!(byte_ranges("中文.md", &[1, 2]), vec![3..7]);
        assert_eq!(byte_ranges("ab", &[]), Vec::<Range<usize>>::new());
        assert_eq!(byte_ranges("ab", &[5]), Vec::<Range<usize>>::new());
    }

    #[test]
    fn inserts_paths_relative_to_the_pane() {
        assert_eq!(insert_text(Path::new("/r/src/a b.rs"), Path::new("/r")), r"src/a\ b.rs ");
        assert_eq!(insert_text(Path::new("/r/x.md"), Path::new("/r/sub")), "../x.md ");
    }

    #[test]
    fn recent_files_most_recent_first() {
        let mut recent = Recent::default();
        recent.push("/r/a.rs".into());
        recent.push("/r/b.rs".into());
        recent.push("/elsewhere/c.rs".into());
        recent.push("/r/a.rs".into());
        assert_eq!(recent.under(Path::new("/r")), ["a.rs", "b.rs"]);
        for i in 0..Recent::CAP + 5 {
            recent.push(format!("/r/{i}").into());
        }
        let under = recent.under(Path::new("/r"));
        assert_eq!(under.len(), Recent::CAP);
        assert_eq!(under[0], format!("{}", Recent::CAP + 4));
    }

    #[test]
    fn listing_cache_drops_the_least_recently_used() {
        let listing = |f: &str| Arc::new(Listing { files: vec![f.into()], ..Listing::default() });
        let mut cache = ListingCache::default();
        for i in 0..ListingCache::CAP {
            cache.insert(format!("/r{i}").into(), listing("x"));
        }
        // Using /r0 makes /r1 the oldest.
        assert!(cache.get(Path::new("/r0")).is_some());
        cache.insert("/new".into(), listing("y"));
        assert!(cache.get(Path::new("/r1")).is_none());
        assert!(cache.get(Path::new("/r0")).is_some());
        // Inserting an existing root replaces it.
        cache.insert("/new".into(), listing("z"));
        assert_eq!(cache.get(Path::new("/new")).unwrap().files, ["z"]);
    }

    #[test]
    fn one_listing_per_root() {
        let mut in_flight = InFlight::default();
        assert!(in_flight.join(Path::new("/r"), 1));
        // A second palette on the same root waits; another root starts its own.
        assert!(!in_flight.join(Path::new("/r"), 2));
        assert!(in_flight.join(Path::new("/s"), 3));
        assert_eq!(in_flight.finish(Path::new("/r")), [1, 2]);
        assert_eq!(in_flight.finish(Path::new("/r")), Vec::<i32>::new());
        // Once finished, the next open lists again.
        assert!(in_flight.join(Path::new("/r"), 4));
        assert_eq!(in_flight.finish(Path::new("/s")), [3]);
    }

    #[test]
    fn palette_closes_with_its_pane_or_tab() {
        // Origin pane 1 on the active tab.
        assert!(palette_stays(1, true, &[1, 2]));
        // Another tab became active (⌘2, a tab click, the origin's tab closed).
        assert!(!palette_stays(1, true, &[3]));
        // The origin pane closed.
        assert!(!palette_stays(1, false, &[2]));
        // The active tab is zoomed on another pane.
        assert!(!palette_stays(1, true, &[2]));
        // No tab at all.
        assert!(!palette_stays(1, true, &[]));
    }

    #[test]
    fn refresh_reports_only_changes() {
        let a = Listing { files: vec!["a".into()], ..Listing::default() };
        let b = Listing { files: vec!["a".into(), "b".into()], ..Listing::default() };
        assert_eq!(changed(Some(&a), a.clone()), None);
        assert_eq!(changed(Some(&a), b.clone()), Some(b.clone()));
        assert_eq!(changed(None, a.clone()), Some(a));
    }
}
