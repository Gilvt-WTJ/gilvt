//! Finding the transcript of an agent that runs without hooks (lite binding), a Claude session's subagent
//! transcripts, and recognizing agents by their process.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::event::AgentKind;

/// Claude Code cuts longer project directory names and appends a hash of the path.
const CLAUDE_DIR_MAX: usize = 200;
/// Day directories of `~/.codex/sessions` searched (today and yesterday, whatever the time zone).
const CODEX_DAYS: usize = 2;
/// Upper bound for a rollout's first line (`session_meta` carries the base instructions).
const META_LINE_MAX: u64 = 4 << 20;

/// Claude Code's project directory name for `cwd`: every char that is not an ASCII letter or digit
/// becomes `-` (two for chars outside the BMP, which JavaScript sees as two UTF-16 units).
pub fn claude_project_dir_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_string() } else { "-".repeat(c.len_utf16()) })
        .collect()
}

/// Newest Claude transcript for `cwd` modified at or after `since`:
/// `<home>/.claude/projects/<encoded cwd>/*.jsonl` → (session_id, path).
pub fn newest_claude_transcript(home: &Path, cwd: &Path, since: SystemTime) -> Option<(String, PathBuf)> {
    let projects = home.join(".claude/projects");
    let mut dirs = Vec::new();
    for cwd in spellings(cwd) {
        let name = claude_project_dir_name(&cwd);
        let dir = projects.join(&name);
        if dir.is_dir() {
            dirs.push(dir);
        } else if name.len() > CLAUDE_DIR_MAX {
            let prefix = format!("{}-", &name[..CLAUDE_DIR_MAX]);
            dirs.extend(entries(&projects).into_iter().filter(|p| file_name(p).starts_with(&prefix) && p.is_dir()));
        }
    }
    let newest = dirs
        .iter()
        .flat_map(|d| entries(d))
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl") && p.is_file())
        .filter_map(|p| Some((modified(&p).filter(|m| *m >= since)?, p)))
        .max_by_key(|(m, _)| *m)?
        .1;
    let id = newest.file_stem()?.to_string_lossy().into_owned();
    Some((id, newest))
}

/// Newest Codex rollout modified at or after `since` whose `session_meta.cwd` is `cwd`, among
/// `<home>/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` of the two newest days → (session_id, path).
pub fn newest_codex_rollout(home: &Path, cwd: &Path, since: SystemTime) -> Option<(String, PathBuf)> {
    let sessions = home.join(".codex/sessions");
    let mut days = Vec::new();
    'years: for year in numbered(&sessions) {
        for month in numbered(&year) {
            for day in numbered(&month) {
                days.push(day);
                if days.len() == CODEX_DAYS {
                    break 'years;
                }
            }
        }
    }
    let mut rollouts: Vec<(SystemTime, PathBuf)> = days
        .iter()
        .flat_map(|d| entries(d))
        .filter(|p| {
            let name = file_name(p);
            name.starts_with("rollout-") && name.ends_with(".jsonl")
        })
        .filter_map(|p| Some((modified(&p).filter(|m| *m >= since)?, p)))
        .collect();
    rollouts.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    let wanted = spellings(cwd);
    rollouts.into_iter().find_map(|(_, path)| {
        let (id, meta_cwd) = session_meta(&path)?;
        spellings(&meta_cwd).iter().any(|c| wanted.contains(c)).then_some((id, path))
    })
}

/// A process name as a command name: the basename, without a trailing `.exe` (any case). The native
/// Claude Code build installs `bin/claude.exe` behind the `claude` link, so the kernel calls it `claude.exe`.
pub fn process_basename(name: &str) -> &str {
    let base = name.rsplit('/').next().unwrap_or(name);
    match base.len().checked_sub(4) {
        Some(stem) if stem > 0 && base.is_char_boundary(stem) && base[stem..].eq_ignore_ascii_case(".exe") => &base[..stem],
        _ => base,
    }
}

/// The agent a foreground program is: `claude` → Claude; `codex*`, or `node` running codex → Codex;
/// `node` running `@anthropic-ai/claude-code` → Claude. `argv` is the command line (or argv[0..]).
pub fn agent_of_process(name: &str, argv: Option<&str>) -> Option<AgentKind> {
    let name = process_basename(name).to_ascii_lowercase();
    let argv = argv.unwrap_or("").to_ascii_lowercase();
    match name.as_str() {
        "claude" => Some(AgentKind::Claude),
        n if n.starts_with("codex") => Some(AgentKind::Codex),
        "node" | "bun" if argv.contains("codex") => Some(AgentKind::Codex),
        "node" | "bun" if argv.contains("claude-code") || argv.contains("/claude ") || argv.ends_with("/claude") => {
            Some(AgentKind::Claude)
        }
        _ => None,
    }
}

/// A Claude session's subagent transcripts: `<dir>/<session>/subagents/agent-<agent_id>.jsonl` next to
/// `<dir>/<session>.jsonl` → (agent_id, path), sorted by agent id. Missing directory → [].
pub fn subagent_transcripts(transcript: &Path) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = entries(&transcript.with_extension("").join("subagents"))
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl") && p.is_file())
        .filter_map(|p| {
            let id = file_name(&p).strip_suffix(".jsonl")?.strip_prefix("agent-")?.to_string();
            (!id.is_empty()).then_some((id, p))
        })
        .collect();
    found.sort();
    found
}

/// The parent Task / Agent tool-use id of a subagent transcript, from `agent-<id>.meta.json` beside it
/// (`{"agentType", "description", "toolUseId", "spawnDepth"}`); written when the subagent starts.
pub fn subagent_parent_tool_use(subagent_transcript: &Path) -> Option<String> {
    let text = fs::read_to_string(subagent_transcript.with_extension("meta.json")).ok()?;
    let meta: Value = serde_json::from_str(&text).ok()?;
    meta.get("toolUseId")?.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// `(session id, cwd)` from a rollout's first line.
fn session_meta(path: &Path) -> Option<(String, PathBuf)> {
    let mut line = String::new();
    BufReader::new(fs::File::open(path).ok()?.take(META_LINE_MAX)).read_line(&mut line).ok()?;
    let record: Value = serde_json::from_str(&line).ok()?;
    if record.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    let payload = record.get("payload")?;
    let id = payload.get("id").or_else(|| payload.get("session_id"))?.as_str()?;
    Some((id.to_string(), PathBuf::from(payload.get("cwd")?.as_str()?)))
}

/// `path` as given (without a trailing slash) and canonicalized, so `/tmp/x` matches `/private/tmp/x`.
fn spellings(path: &Path) -> Vec<PathBuf> {
    let given: PathBuf = path.components().collect();
    let mut all = vec![given.clone()];
    if let Ok(canonical) = given.canonicalize() {
        if canonical != given {
            all.push(canonical);
        }
    }
    all
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default()
}

/// All-digit subdirectories, newest (largest) first.
fn numbered(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = entries(dir)
        .into_iter()
        .filter(|p| {
            let name = file_name(p);
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) && p.is_dir()
        })
        .collect();
    dirs.sort_by_key(|p| std::cmp::Reverse(file_name(p).parse::<u32>().unwrap_or(0)));
    dirs
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}
