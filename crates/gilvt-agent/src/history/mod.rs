//! The sessions on this machine (M3c spec §3): one [`HistoryEntry`] per interactive Claude session
//! (`~/.claude/projects/<dir>/<id>.jsonl`) and Codex rollout (`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`),
//! for the 「会话」 palette. Files are read line by line; [`HistoryIndex`] caches the results by
//! (size, mtime) in `state/history.json`. Blocking file IO: run it on a background thread.

mod cache;
mod claude;
mod codex;
mod companion;

use std::fs;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::AgentKind;
use crate::review::ReviewSessionIndex;
use crate::summary::{first_line, truncate_chars};
use crate::timeline::parse_timestamp;

pub use cache::HistoryIndex;
pub use companion::{companion_files, companion_size, is_session_uuid, path_size};

/// Longest [`HistoryEntry::first_prompt`], in chars, including the `…` added when cut.
pub const FIRST_PROMPT_MAX: usize = 80;

/// One past session, as the 「会话」 palette lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub agent: AgentKind,
    pub session_id: String,
    pub cwd: PathBuf,
    pub transcript: PathBuf,
    /// First real prompt, first non-blank line, trimmed, ≤ [`FIRST_PROMPT_MAX`] chars.
    pub first_prompt: String,
    /// The first prompt that says what the session is about (not 「继续」, a bare path, …), same shape as
    /// `first_prompt`; empty when none does. Used for the title when the agent gave none.
    #[serde(default)]
    pub topic_prompt: String,
    /// The title the user gave the session in the agent (Claude's last `custom-title`).
    #[serde(default)]
    pub custom_title: Option<String>,
    /// The title the agent generated (Claude's last `ai-title`; Codex's `thread_name`, filled in by
    /// [`HistoryIndex::entries`](cache::HistoryIndex::entries)).
    #[serde(default)]
    pub ai_title: Option<String>,
    /// First record timestamp.
    pub started: Option<SystemTime>,
    /// Latest record timestamp (falls back to the file's mtime).
    pub last_active: SystemTime,
    /// Real prompts, counted like the timeline's turns.
    pub turns: u32,
    /// Latest model named in the file (Claude `message.model`, Codex `turn_context.model`).
    pub model: Option<String>,
    /// Bytes on disk: the transcript plus its companions (see [`session_files`]).
    pub size: u64,
}

/// Parses one Claude transcript. `None` when the session is excluded (SDK / `--print` run, subagent-only
/// or no real prompt) or the file cannot be read.
pub fn parse_claude_session(path: &Path) -> Option<HistoryEntry> {
    claude::parse(path)
        .ok()
        .flatten()
        .map(|parsed| parsed.history)
}

/// Parses one Codex rollout. `None` when the session is excluded (subagent thread, `codex exec`, no user
/// prompt) or the file cannot be read.
pub fn parse_codex_rollout(path: &Path) -> Option<HistoryEntry> {
    codex::parse(path)
        .ok()
        .flatten()
        .map(|parsed| parsed.history)
}

/// Parses one Claude transcript into its bounded review index.
pub fn parse_claude_review(path: &Path) -> Option<ReviewSessionIndex> {
    claude::parse(path)
        .ok()
        .flatten()
        .map(|parsed| parsed.review)
}

/// Parses one Codex rollout into its bounded review index.
pub fn parse_codex_review(path: &Path) -> Option<ReviewSessionIndex> {
    codex::parse(path)
        .ok()
        .flatten()
        .map(|parsed| parsed.review)
}

/// The files that make up a session, the transcript first: Claude `<id>.jsonl` plus the `<id>/` directory
/// (subagents etc.) when it exists; Codex the rollout plus its `<rollout>.*` siblings (`.langsmith`).
pub fn session_files(e: &HistoryEntry) -> Vec<PathBuf> {
    let mut files = vec![e.transcript.clone()];
    match e.agent {
        AgentKind::Claude => {
            let dir = e.transcript.with_extension("");
            if dir.is_dir() {
                files.push(dir);
            }
        }
        AgentKind::Codex => files.extend(codex_siblings(&e.transcript)),
    }
    files
}

/// The facts both parsers collect from the records.
#[derive(Default)]
struct Scan {
    first_prompt: Option<String>,
    topic_prompt: Option<String>,
    custom_title: Option<String>,
    ai_title: Option<String>,
    turns: u32,
    started: Option<SystemTime>,
    last: Option<SystemTime>,
    model: Option<String>,
}

pub(super) struct ParsedSession {
    pub history: HistoryEntry,
    pub review: ReviewSessionIndex,
}

pub(super) struct ReadSummary {
    pub scanned_through: u64,
    pub incomplete_tail: bool,
}

impl Scan {
    fn timestamp(&mut self, r: &Value) {
        let Some(ts) = r
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_timestamp)
        else {
            return;
        };
        self.started.get_or_insert(ts);
        self.last = Some(self.last.map_or(ts, |last| last.max(ts)));
    }

    fn prompt(&mut self, text: &str) {
        self.turns += 1;
        let line = first_line(text);
        if self.first_prompt.is_none() && !line.is_empty() {
            self.first_prompt = Some(truncate_chars(line, FIRST_PROMPT_MAX - 1));
        }
        if self.topic_prompt.is_none() && crate::title::is_informative(line) {
            self.topic_prompt = Some(truncate_chars(line, FIRST_PROMPT_MAX - 1));
        }
    }

    /// Claude's own titles. A session carries several; the last non-blank one of each kind wins.
    fn title_record(&mut self, r: &Value) {
        let (slot, key) = match r.get("type").and_then(Value::as_str) {
            Some("ai-title") => (&mut self.ai_title, "aiTitle"),
            Some("custom-title") => (&mut self.custom_title, "customTitle"),
            _ => return,
        };
        if let Some(title) = r
            .get(key)
            .and_then(Value::as_str)
            .and_then(crate::title::single_line)
        {
            *slot = Some(title);
        }
    }

    /// The entry, or `None` without a single prompt.
    fn entry(
        self,
        agent: AgentKind,
        session_id: String,
        cwd: PathBuf,
        path: &Path,
        size: u64,
    ) -> io::Result<Option<HistoryEntry>> {
        if self.turns == 0 {
            return Ok(None);
        }
        let last_active = match self.last {
            Some(last) => last,
            None => fs::metadata(path)?.modified()?,
        };
        Ok(Some(HistoryEntry {
            agent,
            session_id,
            cwd,
            transcript: path.to_path_buf(),
            first_prompt: self.first_prompt.unwrap_or_default(),
            topic_prompt: self.topic_prompt.unwrap_or_default(),
            custom_title: self.custom_title,
            ai_title: self.ai_title,
            started: self.started,
            last_active,
            turns: self.turns,
            model: self.model,
            size,
        }))
    }
}

/// Calls `f` with each complete JSON line of `path` until it returns `false`. Lines that are not JSON are
/// skipped, and so is a last line without its newline (still being written).
fn for_each_record(
    path: &Path,
    f: impl FnMut(&Value, u64, u64, &[u8]) -> bool,
) -> io::Result<ReadSummary> {
    for_each_record_from(path, 0, f)
}

/// Like [`for_each_record`], starting at a known complete-line boundary.
fn for_each_record_from(
    path: &Path,
    start_offset: u64,
    mut f: impl FnMut(&Value, u64, u64, &[u8]) -> bool,
) -> io::Result<ReadSummary> {
    let mut reader = BufReader::new(fs::File::open(path)?);
    reader.seek(SeekFrom::Start(start_offset))?;
    let mut line = Vec::new();
    let mut offset = start_offset;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(ReadSummary {
                scanned_through: offset,
                incomplete_tail: false,
            });
        }
        if line.last() != Some(&b'\n') {
            return Ok(ReadSummary {
                scanned_through: offset,
                incomplete_tail: true,
            });
        }
        let start = offset;
        offset += read as u64;
        let Ok(record) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if !f(&record, start, offset, &line) {
            return Ok(ReadSummary {
                scanned_through: offset,
                incomplete_tail: false,
            });
        }
    }
}

/// `<rollout>.*` next to a rollout, sorted.
fn codex_siblings(rollout: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (rollout.parent(), rollout.file_name()) else {
        return Vec::new();
    };
    let prefix = format!("{}.", name.to_string_lossy());
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(&prefix))
        })
        .collect();
    found.sort();
    found
}

/// Bytes of a file, or of every file under a directory (symlinks are not followed); 0 when missing.
pub fn disk_size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    fs::read_dir(path)
        .map(|rd| rd.flatten().map(|e| disk_size(&e.path())).sum())
        .unwrap_or(0)
}
