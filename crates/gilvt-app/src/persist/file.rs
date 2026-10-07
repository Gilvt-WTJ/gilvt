//! `<state dir>/workspace.json`: atomic writes, and a damaged file is moved aside instead of blocking startup.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use super::snapshot::Snapshot;

pub const FILE_NAME: &str = "workspace.json";

pub enum Loaded {
    Missing,
    Ok(Snapshot),
    /// Unreadable, unparsable, or failed validation; the file is now `workspace.json.bad`.
    Corrupt(String),
}

pub fn load(dir: &Path) -> Loaded {
    let path = dir.join(FILE_NAME);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Loaded::Missing,
        Err(e) => return Loaded::Corrupt(e.to_string()),
    };
    let parsed = serde_json::from_slice::<Snapshot>(&bytes).map_err(|e| e.to_string()).and_then(|s| s.validate().map(|_| s));
    match parsed {
        Ok(s) => Loaded::Ok(s),
        Err(why) => {
            let _ = fs::rename(&path, dir.join("workspace.json.bad"));
            Loaded::Corrupt(why)
        }
    }
}

/// Every write and the removal run under this lock: a background tick write, the quit-time write and a
/// removal never interleave.
static LOCK: Mutex<()> = Mutex::new(());
/// Bumped by whoever supersedes in-flight background writes (the final save, clearing the layout).
static GEN: AtomicU64 = AtomicU64::new(0);
/// Makes every temp file name unique within the process.
static SEQ: AtomicU64 = AtomicU64::new(0);

fn locked() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Supersedes every background write started before this call (see [`save_if_current`]); returns the new generation.
pub fn bump_generation() -> u64 {
    GEN.fetch_add(1, Ordering::SeqCst) + 1
}

pub fn current_generation() -> u64 {
    GEN.load(Ordering::SeqCst)
}

/// A temp file name no other write of this process uses.
fn tmp_name() -> String {
    format!("{FILE_NAME}.tmp-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst))
}

/// Writes through a unique temp file and a rename; the temp file is removed when anything fails. The caller holds the lock.
fn write_locked(dir: &Path, snap: &Snapshot) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(snap).map_err(io::Error::other)?;
    let tmp = dir.join(tmp_name());
    let result = fs::write(&tmp, json).and_then(|_| fs::rename(&tmp, dir.join(FILE_NAME)));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Atomic, serialized write (the quit-time save).
pub fn save(dir: &Path, snap: &Snapshot) -> io::Result<()> {
    let _guard = locked();
    write_locked(dir, snap)
}

/// A background write: only if no [`bump_generation`] happened since `generation` was read. Ok(false): superseded,
/// nothing written.
pub fn save_if_current(dir: &Path, snap: &Snapshot, generation: u64) -> io::Result<bool> {
    save_if(dir, snap, &GEN, generation)
}

fn save_if(dir: &Path, snap: &Snapshot, gen_now: &AtomicU64, generation: u64) -> io::Result<bool> {
    let _guard = locked();
    if gen_now.load(Ordering::SeqCst) != generation {
        return Ok(false);
    }
    write_locked(dir, snap)?;
    Ok(true)
}

/// Deletes the saved layout (no error when there is none) and supersedes in-flight background writes.
pub fn remove(dir: &Path) -> io::Result<()> {
    let _guard = locked();
    bump_generation();
    match fs::remove_file(dir.join(FILE_NAME)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::snapshot::*;

    fn snap() -> Snapshot {
        Snapshot {
            version: VERSION,
            windows: vec![WindowSnap {
                frame: None,
                active_tab: 0,
                monitor: false,
                monitor_active: false,
                tabs: vec![TabSnap { tree: NodeSnap::Leaf { pane: PaneSnap { pane_id: 1, cwd: Some("/w".into()), agent: None } }, focused: 1 }],
            }],
        }
    }

    #[test]
    fn missing_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(load(dir.path()), Loaded::Missing));
    }

    #[test]
    fn save_then_load_round_trips_without_leaving_a_tmp_file() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        save(&state, &snap()).unwrap();
        assert!(matches!(load(&state), Loaded::Ok(s) if s == snap()));
        assert!(tmp_files(&state).is_empty());
    }

    fn tmp_files(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .filter(|n| n.contains(".tmp"))
            .collect()
    }

    #[test]
    fn tmp_names_are_unique() {
        let names: std::collections::HashSet<String> = (0..100).map(|_| tmp_name()).collect();
        assert_eq!(names.len(), 100);
        assert!(names.iter().all(|n| n.starts_with("workspace.json.tmp-")));
    }

    #[test]
    fn sequential_saves_leave_no_tmp_files() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &snap()).unwrap();
        save(dir.path(), &snap()).unwrap();
        assert!(tmp_files(dir.path()).is_empty());
        assert!(matches!(load(dir.path()), Loaded::Ok(_)));
    }

    #[test]
    fn a_failed_write_removes_its_tmp_file() {
        let dir = tempfile::tempdir().unwrap();
        // The target name is a non-empty directory: the rename fails after the temp file was written.
        fs::create_dir_all(dir.path().join(FILE_NAME).join("x")).unwrap();
        assert!(save(dir.path(), &snap()).is_err());
        assert!(tmp_files(dir.path()).is_empty());
    }

    #[test]
    fn a_superseded_background_write_does_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let g = current_generation();
        bump_generation();
        assert!(!save_if_current(dir.path(), &snap(), g).unwrap());
        assert!(!dir.path().join(FILE_NAME).exists());
        // And a current one writes (a private counter: other tests bump the global one).
        let local = AtomicU64::new(7);
        assert!(save_if(dir.path(), &snap(), &local, 7).unwrap());
        assert!(matches!(load(dir.path()), Loaded::Ok(_)));
        local.store(8, Ordering::SeqCst);
        fs::remove_file(dir.path().join(FILE_NAME)).unwrap();
        assert!(!save_if(dir.path(), &snap(), &local, 7).unwrap());
        assert!(!dir.path().join(FILE_NAME).exists());
    }

    #[test]
    fn a_stale_tick_write_cannot_bring_back_a_removed_layout() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &snap()).unwrap();
        let tick_gen = current_generation();
        remove(dir.path()).unwrap(); // the last window closed
        assert!(!dir.path().join(FILE_NAME).exists());
        assert!(!save_if_current(dir.path(), &snap(), tick_gen).unwrap(), "the in-flight tick write lands late");
        assert!(!dir.path().join(FILE_NAME).exists());
        remove(dir.path()).unwrap(); // removing nothing is fine
    }

    #[test]
    fn concurrent_saves_end_with_a_valid_file_and_no_tmp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        save(&path, &snap()).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert!(matches!(load(&path), Loaded::Ok(s) if s == snap()));
        assert!(tmp_files(&path).is_empty());
    }

    #[test]
    fn garbage_is_corrupt_and_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), "{not json").unwrap();
        assert!(matches!(load(dir.path()), Loaded::Corrupt(_)));
        assert!(!dir.path().join(FILE_NAME).exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("workspace.json.bad")).unwrap(), "{not json");
        assert!(matches!(load(dir.path()), Loaded::Missing), "the next start is a clean one");
    }

    #[test]
    fn valid_json_that_fails_validation_is_corrupt_too() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = snap();
        s.windows[0].tabs[0].focused = 99;
        std::fs::write(dir.path().join(FILE_NAME), serde_json::to_vec(&s).unwrap()).unwrap();
        assert!(matches!(load(dir.path()), Loaded::Corrupt(_)));
        assert!(dir.path().join("workspace.json.bad").exists());
    }

    #[test]
    fn an_old_bad_file_is_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("workspace.json.bad"), "old").unwrap();
        std::fs::write(dir.path().join(FILE_NAME), "new junk").unwrap();
        let _ = load(dir.path());
        assert_eq!(std::fs::read_to_string(dir.path().join("workspace.json.bad")).unwrap(), "new junk");
    }
}
