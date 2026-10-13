//! One bridge's watched files (R2a §8.1 `Watch`): looked at every `WATCH_POLL`; a changed stat (mtime, size, gone,
//! back) is reported once. `poll` only stats: the daemon computes the fingerprints of the reported files after
//! releasing the set's lock, so a slow disk never holds up `Watch` / `Unwatch` on the read loop (plan ruling 26).

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use gilvt_ipc::remote::MAX_WATCHES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Seen {
    exists: bool,
    mtime_ns: u64,
    size: u64,
    ino: u64,
}

fn look(path: &Path) -> Seen {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => Seen { exists: true, mtime_ns: crate::files::mtime_ns(&m), size: m.len(), ino: m.ino() },
        _ => Seen { exists: false, mtime_ns: 0, size: 0, ino: 0 },
    }
}

#[derive(Default)]
pub struct WatchSet {
    /// What each file looked like at the last poll (or when it was added).
    files: BTreeMap<PathBuf, Seen>,
}

impl WatchSet {
    pub fn add(&mut self, path: PathBuf) -> Result<(), String> {
        if self.files.contains_key(&path) {
            return Ok(());
        }
        if self.files.len() >= MAX_WATCHES {
            return Err(format!("already watching {MAX_WATCHES} files"));
        }
        // The baseline is taken now, so a change right after `Watch` is answered is not missed.
        let seen = look(&path);
        self.files.insert(path, seen);
        Ok(())
    }

    pub fn remove(&mut self, path: &Path) {
        self.files.remove(path);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn poll(&mut self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for (path, last) in self.files.iter_mut() {
            let now = look(path);
            if *last != now {
                *last = now;
                out.push(path.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_changes_deletions_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "one").unwrap();
        let mut w = WatchSet::default();
        w.add(f.clone()).unwrap();
        assert!(w.poll().is_empty(), "the first look is the baseline");
        std::fs::write(&f, "three").unwrap();
        assert_eq!(w.poll(), vec![f.clone()]);
        assert_eq!(crate::files::fingerprint_of(&f).unwrap().map(|fp| fp.size), Some(5));
        assert!(w.poll().is_empty(), "reported once");
        std::fs::remove_file(&f).unwrap();
        assert_eq!(w.poll(), vec![f.clone()]);
        assert_eq!(crate::files::fingerprint_of(&f).unwrap(), None, "gone");
        assert!(w.poll().is_empty());
        std::fs::write(&f, "back").unwrap();
        assert_eq!(w.poll(), vec![f.clone()], "reappearing is a change");
        assert_eq!(crate::files::fingerprint_of(&f).unwrap().map(|fp| fp.size), Some(4));
    }

    #[test]
    fn at_most_sixty_four_and_adding_twice_is_one() {
        let mut w = WatchSet::default();
        for i in 0..MAX_WATCHES {
            w.add(PathBuf::from(format!("/nonexistent/{i}"))).unwrap();
        }
        w.add(PathBuf::from("/nonexistent/0")).unwrap();
        assert_eq!(w.len(), MAX_WATCHES);
        assert!(w.add(PathBuf::from("/nonexistent/x")).is_err());
        w.remove(Path::new("/nonexistent/0"));
        assert!(w.add(PathBuf::from("/nonexistent/x")).is_ok());
    }
}
