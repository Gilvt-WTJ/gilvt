//! The tail thread's state: per session, the transcript tail, the subagent transcript tails (Claude) and the
//! [`Timeline`]. Transcript lines go both to the registry (as event batches) and, all of them, history
//! included, to the timeline; hooks arrive from the main thread. Sessions that stop being watched (ended)
//! keep their timeline for this run, and their tail position in case they come back.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gilvt_agent::{subagent_parent_tool_use, subagent_transcripts, AgentKind, SessionKey, Tail, Timeline};

use super::tails::{catchup, parse, Batch};
use super::timelines::{snapshot, HookFeed, TimelineView};

/// Upper bound of reads (`Tail` reads ≤ 16 MB a call) when a transcript starts being watched, and per
/// subagent transcript per pass.
const CATCHUP_READS: usize = 8;

/// A timeline and whether it can still be fed.
struct Fed {
    timeline: Timeline,
    /// Panicked once: no longer fed (spec §6 「无法读取该会话的过程」).
    failed: bool,
    /// Changed since the last snapshot.
    dirty: bool,
}

impl Fed {
    fn new(agent: AgentKind) -> Fed {
        Fed { timeline: Timeline::new(agent), failed: false, dirty: false }
    }

    /// A parser bug must not take the transcript thread (and the status of every session) down with it.
    fn apply(&mut self, key: &SessionKey, f: impl FnOnce(&mut Timeline)) {
        if self.failed {
            return;
        }
        self.dirty = true;
        let timeline = &mut self.timeline;
        if catch_unwind(AssertUnwindSafe(|| f(timeline))).is_err() {
            eprintln!("gilvt: the timeline of {:?} session {} failed; it is no longer updated", key.0, key.1);
            self.failed = true;
        }
    }
}

struct SubTail {
    tail: Tail,
    /// The parent Task / Agent call is known (`agent-<id>.meta.json`).
    linked: bool,
}

struct Entry {
    fed: Fed,
    /// The session transcript / rollout.
    tail: Option<Tail>,
    /// Being watched (a live session with a transcript).
    live: bool,
    /// The next read is history: the registry takes it as [`Batch::Catchup`].
    fresh: bool,
    /// Claude subagent transcripts by agent id.
    subs: BTreeMap<String, SubTail>,
    /// The last published snapshot.
    view: Option<Arc<TimelineView>>,
}

impl Entry {
    fn new(agent: AgentKind) -> Entry {
        Entry { fed: Fed::new(agent), tail: None, live: false, fresh: false, subs: BTreeMap::new(), view: None }
    }
}

/// `<dir>/<session>/subagents` beside `<dir>/<session>.jsonl`.
fn subagents_dir(transcript: &Path) -> PathBuf {
    transcript.with_extension("").join("subagents")
}

#[derive(Default)]
pub struct Worker {
    entries: HashMap<SessionKey, Entry>,
}

impl Worker {
    /// Replaces the set of watched transcripts. A new transcript is read from its start (history); a
    /// session watched again with the same transcript continues where it stopped.
    pub fn watch(&mut self, set: Vec<(SessionKey, PathBuf)>) {
        let wanted: HashMap<SessionKey, PathBuf> = set.into_iter().collect();
        for (key, path) in &wanted {
            let e = self.entries.entry(key.clone()).or_insert_with(|| Entry::new(key.0));
            match &e.tail {
                Some(t) if t.path() == path => {}
                Some(_) => {
                    // Another transcript for the same session: start over rather than replay it into the old
                    // rows twice.
                    e.fed = Fed::new(key.0);
                    e.fed.dirty = true;
                    e.subs.clear();
                    e.tail = Some(Tail::new(path.clone()));
                    e.fresh = true;
                }
                None => {
                    e.tail = Some(Tail::new(path.clone()));
                    e.fresh = true;
                }
            }
            // Watched again: what was written meanwhile is history for the registry too.
            e.fresh |= !e.live;
        }
        for (key, e) in self.entries.iter_mut() {
            e.live = wanted.contains_key(key);
        }
    }

    /// A hook request of session `key` (the registry knows the session).
    pub fn hook(&mut self, key: SessionKey, h: HookFeed) {
        let e = self.entries.entry(key.clone()).or_insert_with(|| Entry::new(key.0));
        e.fed.apply(&key, |t| t.apply_hook(&h.event, &h.payload, h.anchor, h.at));
    }

    /// The user answered an approval dialog of session `key` (a session without a timeline yet has nothing
    /// waiting).
    pub fn approval_answered(&mut self, key: &SessionKey, at: std::time::SystemTime) {
        if let Some(e) = self.entries.get_mut(key) {
            e.fed.apply(key, |t| t.approval_answered(at));
        }
    }

    /// Reads what the watched transcripts gained: every line to the timeline, the parsed events to `emit`
    /// for the registry. Then the subagent transcripts of Claude sessions.
    pub fn read(&mut self, emit: &mut dyn FnMut(SessionKey, Batch)) {
        for (key, e) in self.entries.iter_mut().filter(|(_, e)| e.live) {
            let Some(tail) = e.tail.as_mut() else { continue };
            let fresh = std::mem::take(&mut e.fresh);
            let mut lines = Vec::new();
            for _ in 0..if fresh { CATCHUP_READS } else { 1 } {
                match tail.read_new() {
                    Ok(new) if !new.is_empty() => lines.extend(new),
                    _ => break,
                }
            }
            for line in &lines {
                e.fed.apply(key, |t| t.apply_transcript_line(line));
            }
            let events = parse(key, &lines);
            let batch = if fresh { catchup(events) } else { Batch::Live(events) };
            if !batch.is_empty() {
                emit(key.clone(), batch);
            }
            if key.0 == AgentKind::Claude {
                let transcript = tail.path().to_path_buf();
                read_subagents(key, e, &transcript);
            }
        }
    }

    /// Snapshots of the timelines that changed since the last call.
    pub fn publish(&mut self) -> Vec<(SessionKey, Arc<TimelineView>)> {
        let mut out = Vec::new();
        for (key, e) in self.entries.iter_mut().filter(|(_, e)| e.fed.dirty) {
            e.fed.dirty = false;
            if let Some(view) = snapshot(&e.fed.timeline, e.view.as_deref(), e.fed.failed) {
                let view = Arc::new(view);
                e.view = Some(Arc::clone(&view));
                out.push((key.clone(), view));
            }
        }
        out
    }

    /// Directories to watch: those of the watched transcripts, and the existing subagent directories of the
    /// watched Claude sessions.
    pub fn dirs(&self) -> HashSet<PathBuf> {
        let mut dirs = HashSet::new();
        for (key, e) in self.entries.iter().filter(|(_, e)| e.live) {
            let Some(tail) = &e.tail else { continue };
            dirs.extend(tail.path().parent().map(Path::to_path_buf));
            let subs = subagents_dir(tail.path());
            if key.0 == AgentKind::Claude && subs.is_dir() {
                dirs.insert(subs);
            }
        }
        dirs
    }
}

/// Finds new subagent transcripts, links them to their parent call once `meta.json` names it, and feeds
/// their new lines.
fn read_subagents(key: &SessionKey, e: &mut Entry, transcript: &Path) {
    for (id, path) in subagent_transcripts(transcript) {
        e.subs.entry(id).or_insert_with(|| SubTail { tail: Tail::new(path), linked: false });
    }
    for (id, sub) in e.subs.iter_mut() {
        if !sub.linked {
            if let Some(parent) = subagent_parent_tool_use(sub.tail.path()) {
                e.fed.apply(key, |t| t.link_subagent(id, &parent));
                sub.linked = true;
            }
        }
        for _ in 0..CATCHUP_READS {
            match sub.tail.read_new() {
                Ok(lines) if !lines.is_empty() => {
                    for line in &lines {
                        e.fed.apply(key, |t| t.apply_subagent_line(id, line));
                    }
                }
                _ => break,
            }
        }
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
