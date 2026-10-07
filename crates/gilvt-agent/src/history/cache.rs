//! [`HistoryIndex`]: the parsed sessions, cached in `<state>/history.json` by transcript path and
//! (size, mtime), so a refresh only reads the files that changed.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::HistoryEntry;
use crate::event::AgentKind;
use crate::review::{fnv1a64, ReviewSessionIndex};

const FILE_NAME: &str = "history.json";
/// Bumped whenever [`HistoryEntry`] or the parsing rules change: an older cache is discarded.
const VERSION: u32 = 5;
const PREFIX_PROBE_BYTES: u64 = 4096;

/// One transcript as last parsed; `entry` is `None` for an excluded session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Cached {
    size: u64,
    mtime: SystemTime,
    #[serde(default)]
    prefix_len: u64,
    #[serde(default)]
    prefix_fingerprint: String,
    entry: Option<HistoryEntry>,
    review: Option<ReviewSessionIndex>,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    files: BTreeMap<PathBuf, Cached>,
}

/// Every session under a home directory, kept up to date by [`HistoryIndex::refresh`]. A missing,
/// unreadable or other-version cache file starts empty; writes go through a temp file + rename.
#[derive(Debug)]
pub struct HistoryIndex {
    path: Option<PathBuf>,
    files: BTreeMap<PathBuf, Cached>,
    /// Codex session id → its `thread_name`, as of the last refresh. Not cached on disk: Codex rewrites that
    /// file on its own schedule.
    codex_titles: HashMap<String, String>,
}

/// `<home>/.codex/session_index.jsonl`: one `{"id","thread_name",…}` per line, a later line for the same id
/// replaces an earlier one. A missing file or an unreadable line gives no titles / skips the line.
fn read_codex_titles(home: &Path) -> HashMap<String, String> {
    let Ok(text) = fs::read_to_string(home.join(".codex/session_index.jsonl")) else {
        return HashMap::new();
    };
    let mut titles = HashMap::new();
    for line in text.lines() {
        let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let id = row.get("id").and_then(serde_json::Value::as_str);
        let name = row
            .get("thread_name")
            .and_then(serde_json::Value::as_str)
            .and_then(crate::title::single_line);
        if let (Some(id), Some(name)) = (id, name) {
            titles.insert(id.to_string(), name);
        }
    }
    titles
}

impl HistoryIndex {
    /// The index cached at `<state_dir>/history.json` (created on the first save); `None` → in memory only.
    pub fn load(state_dir: Option<&Path>) -> HistoryIndex {
        let path = state_dir.map(|dir| dir.join(FILE_NAME));
        let files = path
            .as_ref()
            .and_then(|p| fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<File>(&bytes).ok())
            .filter(|f| f.version == VERSION)
            .map(|f| f.files)
            .unwrap_or_default();
        HistoryIndex {
            path,
            files,
            codex_titles: HashMap::new(),
        }
    }

    /// The sessions as of the last refresh (or load), newest `last_active` first.
    pub fn entries(&self) -> Vec<HistoryEntry> {
        let mut entries: Vec<HistoryEntry> = self
            .files
            .values()
            .filter_map(|c| c.entry.clone())
            .collect();
        for entry in &mut entries {
            if entry.agent == AgentKind::Codex {
                entry.ai_title = self.codex_titles.get(&entry.session_id).cloned();
            }
        }
        entries.sort_by(|a, b| {
            b.last_active
                .cmp(&a.last_active)
                .then_with(|| a.transcript.cmp(&b.transcript))
        });
        entries
    }

    /// Review projections as of the last refresh, latest completed activity first.
    pub fn review_sessions(&self) -> Vec<ReviewSessionIndex> {
        let mut sessions: Vec<ReviewSessionIndex> = self
            .files
            .values()
            .filter_map(|cached| cached.review.clone())
            .collect();
        sessions.sort_by(|a, b| {
            b.last_completed()
                .and_then(|turn| turn.completed_at)
                .cmp(&a.last_completed().and_then(|turn| turn.completed_at))
                .then_with(|| a.transcript.cmp(&b.transcript))
        });
        sessions
    }

    /// Lists `<home>/.claude/projects/*/*.jsonl` and `<home>/.codex/sessions/*/*/*/rollout-*.jsonl`, parses
    /// the files whose size or mtime changed, drops the ones that are gone and saves the cache when anything
    /// changed. A file that cannot be read keeps its previous result (and is retried next time). Returns
    /// [`HistoryIndex::entries`].
    pub fn refresh(&mut self, home: &Path) -> Vec<HistoryEntry> {
        let mut old = std::mem::take(&mut self.files);
        let mut changed = false;
        self.codex_titles = read_codex_titles(home);
        for (agent, path) in transcripts(home) {
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            let (size, mtime) = (meta.len(), meta.modified().unwrap_or(UNIX_EPOCH));
            let cached = old.remove(&path);
            if let Some(c) = cached
                .as_ref()
                .filter(|c| c.size == size && c.mtime == mtime)
            {
                self.files.insert(path, c.clone());
                continue;
            }
            let appended = cached.as_ref().filter(|previous| {
                size > previous.size
                    && previous.entry.is_some()
                    && previous.review.is_some()
                    && prefix_matches(&path, previous)
            });
            let parsed = match (agent, appended) {
                (AgentKind::Claude, Some(previous)) => super::claude::parse_appended(
                    &path,
                    previous.entry.as_ref().unwrap(),
                    previous.review.as_ref().unwrap(),
                )
                .or_else(|_| super::claude::parse(&path)),
                (AgentKind::Codex, Some(previous)) => super::codex::parse_appended(
                    &path,
                    previous.entry.as_ref().unwrap(),
                    previous.review.as_ref().unwrap(),
                )
                .or_else(|_| super::codex::parse(&path)),
                (AgentKind::Claude, None) => super::claude::parse(&path),
                (AgentKind::Codex, None) => super::codex::parse(&path),
            };
            match parsed {
                Ok(parsed) => {
                    changed = true;
                    let (entry, review) = parsed.map_or((None, None), |parsed| {
                        (Some(parsed.history), Some(parsed.review))
                    });
                    let (prefix_len, prefix_fingerprint) =
                        file_prefix(&path, size).unwrap_or_default();
                    self.files.insert(
                        path,
                        Cached {
                            size,
                            mtime,
                            prefix_len,
                            prefix_fingerprint,
                            entry,
                            review,
                        },
                    );
                }
                Err(e) => {
                    eprintln!("gilvt: cannot read session {}: {e}", path.display());
                    if let Some(c) = cached {
                        self.files.insert(path, c);
                    }
                }
            }
        }
        if changed || !old.is_empty() {
            self.save_logged();
        }
        self.entries()
    }

    /// Drops a session whose files were removed (moved to the Trash) and saves.
    pub fn forget(&mut self, transcript: &Path) {
        if self.files.remove(transcript).is_some() {
            self.save_logged();
        }
    }

    fn save_logged(&self) {
        if let Err(e) = self.save() {
            let path = self.path.as_deref().unwrap_or(Path::new(FILE_NAME));
            eprintln!("gilvt: cannot save {}: {e}", path.display());
        }
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = File {
            version: VERSION,
            files: self.files.clone(),
        };
        let json = serde_json::to_vec(&file).map_err(io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)
    }
}

fn prefix_matches(path: &Path, cached: &Cached) -> bool {
    if cached.prefix_len == 0 || cached.prefix_fingerprint.is_empty() {
        return false;
    }
    file_prefix_len(path, cached.prefix_len)
        .is_ok_and(|(_, fingerprint)| fingerprint == cached.prefix_fingerprint)
}

fn file_prefix(path: &Path, size: u64) -> io::Result<(u64, String)> {
    file_prefix_len(path, size.min(PREFIX_PROBE_BYTES))
}

fn file_prefix_len(path: &Path, len: u64) -> io::Result<(u64, String)> {
    let mut bytes = Vec::with_capacity(len as usize);
    fs::File::open(path)?.take(len).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != len {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "transcript prefix became shorter",
        ));
    }
    Ok((len, format!("fnv1a64:{:016x}", fnv1a64(&bytes))))
}

/// Transcript files under `home`, by agent. Paths that are not UTF-8 are skipped (the cache keys are text).
fn transcripts(home: &Path) -> Vec<(AgentKind, PathBuf)> {
    let mut found = Vec::new();
    for project in subdirs(&home.join(".claude/projects")) {
        let jsonl = files(&project)
            .into_iter()
            .filter(|p| name(p).is_some_and(|n| n.ends_with(".jsonl")));
        found.extend(jsonl.map(|p| (AgentKind::Claude, p)));
    }
    for year in subdirs(&home.join(".codex/sessions")) {
        for month in subdirs(&year) {
            for day in subdirs(&month) {
                let rollouts = files(&day).into_iter().filter(|p| {
                    name(p).is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
                });
                found.extend(rollouts.map(|p| (AgentKind::Codex, p)));
            }
        }
    }
    found
}

fn name(path: &Path) -> Option<&str> {
    path.file_name()?.to_str()
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    children(dir, |t| t.is_dir())
}

fn files(dir: &Path) -> Vec<PathBuf> {
    children(dir, |t| t.is_file())
}

fn children(dir: &Path, keep: impl Fn(fs::FileType) -> bool) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .filter(|e| e.file_type().is_ok_and(&keep))
        .map(|e| e.path())
        .filter(|p| p.to_str().is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let entry = HistoryEntry {
            agent: AgentKind::Codex,
            session_id: "s".into(),
            cwd: "/w".into(),
            transcript: "/t.jsonl".into(),
            first_prompt: "修复登录".into(),
            topic_prompt: String::new(),
            custom_title: None,
            ai_title: None,
            started: None,
            last_active: UNIX_EPOCH + std::time::Duration::new(1_790_000_000, 123_456_789),
            turns: 2,
            model: Some("gpt-5.5".into()),
            size: 42,
        };
        let mut index = HistoryIndex::load(Some(dir.path()));
        let mtime = entry.last_active;
        index.files.insert(
            "/t.jsonl".into(),
            Cached {
                size: 42,
                mtime,
                prefix_len: 0,
                prefix_fingerprint: String::new(),
                entry: Some(entry.clone()),
                review: None,
            },
        );
        index.files.insert(
            "/x.jsonl".into(),
            Cached {
                size: 1,
                mtime,
                prefix_len: 0,
                prefix_fingerprint: String::new(),
                entry: None,
                review: None,
            },
        );
        index.save().unwrap();
        let loaded = HistoryIndex::load(Some(dir.path()));
        assert_eq!(loaded.files, index.files);
        assert_eq!(loaded.entries(), vec![entry]);
        assert!(!dir.path().join("history.json.tmp").exists());
    }
}
