//! Persisted per-session user choices (renames and mutes): `<dir>/sessions.json`.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::status::SessionKey;

const FILE_NAME: &str = "sessions.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub muted: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    sessions: BTreeMap<String, Entry>,
}

/// Renames and mutes keyed by `<agent>:<session_id>`. A missing or unreadable file starts empty; writes
/// go through a temp file + rename.
#[derive(Debug)]
pub struct Store {
    path: Option<PathBuf>,
    entries: BTreeMap<String, Entry>,
}

impl Store {
    /// The store at `<dir>/sessions.json` (dir = `~/Library/Application Support/gilvt/state`); created on
    /// first write.
    pub fn open(dir: &Path) -> Store {
        let path = dir.join(FILE_NAME);
        let entries = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<File>(&bytes).ok())
            .map(|f| f.sessions)
            .unwrap_or_default();
        Store { path: Some(path), entries }
    }

    /// A store that never touches the disk (tests, or no state directory).
    pub fn in_memory() -> Store {
        Store { path: None, entries: BTreeMap::new() }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub(crate) fn get(&self, key: &SessionKey) -> Entry {
        self.entries.get(&id(key)).cloned().unwrap_or_default()
    }

    pub(crate) fn update(&mut self, key: &SessionKey, change: impl FnOnce(&mut Entry)) -> io::Result<()> {
        let id = id(key);
        let mut entry = self.entries.remove(&id).unwrap_or_default();
        change(&mut entry);
        if entry != Entry::default() {
            self.entries.insert(id, entry);
        }
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = File { sessions: self.entries.clone() };
        let json = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)
    }
}

fn id((agent, session_id): &SessionKey) -> String {
    format!("{}:{session_id}", agent.name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentKind;

    fn key(id: &str) -> SessionKey {
        (AgentKind::Codex, id.to_string())
    }

    #[test]
    fn survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let mut store = Store::open(&state);
        store.update(&key("a"), |e| e.name = Some("修复登录".into())).unwrap();
        store.update(&key("b"), |e| e.muted = true).unwrap();
        let text = fs::read_to_string(state.join("sessions.json")).unwrap();
        assert!(text.contains("\"codex:a\""), "{text}");
        let store = Store::open(&state);
        assert_eq!(store.get(&key("a")), Entry { name: Some("修复登录".into()), muted: false });
        assert_eq!(store.get(&key("b")), Entry { name: None, muted: true });
        assert_eq!(store.get(&key("c")), Entry::default());
        assert!(!state.join("sessions.json.tmp").exists());
    }

    #[test]
    fn cleared_entries_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path());
        store.update(&key("a"), |e| e.muted = true).unwrap();
        store.update(&key("a"), |e| e.muted = false).unwrap();
        let text = fs::read_to_string(dir.path().join("sessions.json")).unwrap();
        assert!(!text.contains("codex:a"), "{text}");
    }

    #[test]
    fn unreadable_file_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("sessions.json"), "{broken").unwrap();
        assert_eq!(Store::open(dir.path()).get(&key("a")), Entry::default());
        let mut memory = Store::in_memory();
        memory.update(&key("a"), |e| e.muted = true).unwrap();
        assert!(memory.get(&key("a")).muted);
        assert_eq!(memory.path(), None);
    }
}
