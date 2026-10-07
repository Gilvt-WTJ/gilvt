use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use gilvt_agent::{ReviewState, SessionKey, TurnCursor};
use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "reviews.json";
const VERSION: u32 = 1;
static SAVE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct File {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    initialized_at: Option<SystemTime>,
    #[serde(default)]
    baseline_complete: bool,
    #[serde(default)]
    sessions: BTreeMap<String, ReviewState>,
}

impl Default for File {
    fn default() -> Self {
        File {
            version: VERSION,
            initialized_at: None,
            baseline_complete: false,
            sessions: BTreeMap::new(),
        }
    }
}

/// Persistent review cursors and queue preferences. Mutations are transactional: memory changes only after
/// the replacement file has been written successfully.
#[derive(Debug)]
pub struct ReviewStore {
    path: Option<PathBuf>,
    file: File,
    load_error: Option<String>,
    preserve_invalid_file: bool,
}

impl ReviewStore {
    /// Opens `<state_dir>/reviews.json`. Missing files start empty. Invalid or unsupported files are kept
    /// until the first successful save, when they are moved aside as `reviews.json.corrupt[-N]`.
    pub fn open(state_dir: &Path) -> ReviewStore {
        let path = state_dir.join(FILE_NAME);
        match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<File>(&bytes) {
                Ok(file) if file.version == VERSION => ReviewStore {
                    path: Some(path),
                    file,
                    load_error: None,
                    preserve_invalid_file: false,
                },
                Ok(file) => ReviewStore {
                    path: Some(path),
                    file: File::default(),
                    load_error: Some(format!("unsupported reviews.json version {}", file.version)),
                    preserve_invalid_file: true,
                },
                Err(error) => ReviewStore {
                    path: Some(path),
                    file: File::default(),
                    load_error: Some(format!("cannot parse reviews.json: {error}")),
                    preserve_invalid_file: true,
                },
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => ReviewStore {
                path: Some(path),
                file: File::default(),
                load_error: None,
                preserve_invalid_file: false,
            },
            Err(error) => ReviewStore {
                path: Some(path),
                file: File::default(),
                load_error: Some(format!("cannot read reviews.json: {error}")),
                preserve_invalid_file: false,
            },
        }
    }

    /// A store that never touches disk.
    pub fn in_memory() -> ReviewStore {
        ReviewStore {
            path: None,
            file: File::default(),
            load_error: None,
            preserve_invalid_file: false,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn initialized_at(&self) -> Option<SystemTime> {
        self.file.initialized_at
    }

    pub fn baseline_complete(&self) -> bool {
        self.file.baseline_complete
    }

    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    pub fn state(&self, key: &SessionKey) -> ReviewState {
        self.file
            .sessions
            .get(&id(key))
            .cloned()
            .unwrap_or_default()
    }

    /// Baselines every session known at the end of the first complete history refresh. Repeated calls are
    /// no-ops, so a cached partial list cannot overwrite the completed baseline.
    pub fn initialize_baseline(
        &mut self,
        at: SystemTime,
        sessions: impl IntoIterator<Item = (SessionKey, Option<TurnCursor>)>,
    ) -> io::Result<bool> {
        if self.file.baseline_complete {
            return Ok(false);
        }
        let mut next = self.file.clone();
        next.initialized_at = Some(at);
        next.baseline_complete = true;
        for (key, cursor) in sessions {
            let state = next.sessions.entry(id(&key)).or_default();
            if !state.baseline_reconciled {
                state.baseline_reconciled = true;
                if let Some(cursor) = cursor {
                    state.reviewed_through = Some(cursor);
                    state.reviewed_at = Some(at);
                }
            }
        }
        self.commit(next)?;
        Ok(true)
    }

    /// Baselines sessions first discovered after initialization. The caller supplies the last completed turn
    /// at or before `initialized_at`; `None` means the session is new and should enter the inbox normally.
    pub fn reconcile_baseline(
        &mut self,
        sessions: impl IntoIterator<Item = (SessionKey, Option<TurnCursor>)>,
    ) -> io::Result<bool> {
        let Some(initialized_at) = self
            .file
            .initialized_at
            .filter(|_| self.file.baseline_complete)
        else {
            return Ok(false);
        };
        let mut next = self.file.clone();
        let mut changed = false;
        for (key, cursor) in sessions {
            let key = id(&key);
            let state = next.sessions.entry(key).or_default();
            if !state.baseline_reconciled {
                state.baseline_reconciled = true;
                if let Some(cursor) = cursor {
                    state.reviewed_through = Some(cursor);
                    state.reviewed_at = Some(initialized_at);
                }
                changed = true;
            }
        }
        if changed {
            self.commit(next)?;
        }
        Ok(changed)
    }

    /// Advances the review cursor according to the current transcript order. A stale current cursor is an
    /// error rather than an implicit reset; the stale-cursor recovery UI will use a separate explicit action.
    pub fn advance_reviewed(
        &mut self,
        key: &SessionKey,
        cursor: TurnCursor,
        ordered_cursors: &[TurnCursor],
        at: SystemTime,
    ) -> io::Result<bool> {
        let requested = ordered_cursors
            .iter()
            .position(|candidate| candidate == &cursor)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "review cursor is not in the current transcript",
                )
            })?;
        let current = self.state(key).reviewed_through;
        if let Some(current) = current {
            let Some(position) = ordered_cursors
                .iter()
                .position(|candidate| candidate == &current)
            else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "saved review cursor is stale",
                ));
            };
            if requested <= position {
                return Ok(false);
            }
        }
        self.update(key, |state| {
            state.baseline_reconciled = true;
            state.reviewed_through = Some(cursor);
            state.reviewed_at = Some(at);
            state.snoozed_until = None;
        })?;
        Ok(true)
    }

    /// Explicit recovery for a stale cursor. Unlike [`Self::advance_reviewed`], this may move backward or
    /// clear the cursor, so callers must expose it as a deliberate user action.
    pub fn reset_reviewed(
        &mut self,
        key: &SessionKey,
        cursor: Option<TurnCursor>,
        at: SystemTime,
    ) -> io::Result<()> {
        self.update(key, |state| {
            state.baseline_reconciled = true;
            state.reviewed_at = cursor.as_ref().map(|_| at);
            state.reviewed_through = cursor;
            state.snoozed_until = None;
        })
    }

    pub fn set_snoozed_until(
        &mut self,
        key: &SessionKey,
        until: Option<SystemTime>,
    ) -> io::Result<()> {
        self.update(key, |state| state.snoozed_until = until)
    }

    pub fn set_pinned(&mut self, key: &SessionKey, pinned: bool) -> io::Result<()> {
        self.update(key, |state| state.pinned = pinned)
    }

    pub fn set_archived(&mut self, key: &SessionKey, turns: u32, at: SystemTime) -> io::Result<()> {
        self.update(key, |state| {
            state.archived_at = Some(at);
            state.archived_turns = turns;
        })
    }

    pub fn clear_archived(&mut self, key: &SessionKey) -> io::Result<()> {
        self.update(key, |state| {
            state.archived_at = None;
            state.archived_turns = 0;
        })
    }

    fn update(
        &mut self,
        key: &SessionKey,
        change: impl FnOnce(&mut ReviewState),
    ) -> io::Result<()> {
        let mut next = self.file.clone();
        let id = id(key);
        let mut state = next.sessions.remove(&id).unwrap_or_default();
        change(&mut state);
        if state != ReviewState::default() {
            next.sessions.insert(id, state);
        }
        self.commit(next)
    }

    fn commit(&mut self, next: File) -> io::Result<()> {
        if let Some(path) = &self.path {
            save(path, &next, self.preserve_invalid_file)?;
        }
        self.file = next;
        self.load_error = None;
        self.preserve_invalid_file = false;
        Ok(())
    }
}

fn save(path: &Path, file: &File, preserve_invalid_file: bool) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let sequence = SAVE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("json.tmp-{}-{sequence}", std::process::id()));
    fs::write(
        &tmp,
        serde_json::to_vec_pretty(file).map_err(io::Error::other)?,
    )?;
    if preserve_invalid_file && path.exists() {
        fs::rename(path, available_corrupt_path(path))?;
    }
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&tmp);
            Err(error)
        }
    }
}

fn available_corrupt_path(path: &Path) -> PathBuf {
    let first = path.with_extension("json.corrupt");
    if !first.exists() {
        return first;
    }
    for suffix in 1.. {
        let candidate = path.with_extension(format!("json.corrupt-{suffix}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn id((agent, session_id): &SessionKey) -> String {
    format!("{}:{session_id}", agent.name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_round_trips_through_the_file_and_clears_to_an_empty_entry() {
        let dir = tempfile::tempdir().unwrap();
        let key: SessionKey = (gilvt_agent::AgentKind::Claude, "c1".into());
        let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10);
        {
            let mut store = ReviewStore::open(dir.path());
            store.set_archived(&key, 5, at).unwrap();
        }
        let mut store = ReviewStore::open(dir.path());
        let state = store.state(&key);
        assert_eq!((state.archived_at, state.archived_turns), (Some(at), 5));
        store.clear_archived(&key).unwrap();
        assert_eq!(store.state(&key), gilvt_agent::ReviewState::default());
        let text = std::fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
        assert!(
            !text.contains("c1"),
            "an empty state leaves no entry: {text}"
        );
    }
}
