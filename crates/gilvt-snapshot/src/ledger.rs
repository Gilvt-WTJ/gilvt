//! The turn ledger of one session: which turns gilvt saw, the snapshot trees around each, and the files each
//! changed. A pure state machine plus a small JSON file; the snapshotting itself runs elsewhere.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{FileChange, TreeId};

/// Longest prompt head kept, in chars.
pub const PROMPT_MAX: usize = 200;
/// Two reports of the same prompt this close together are one turn.
pub const SAME_TURN_MS: u64 = 5_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnState {
    /// Started, not ended.
    Running,
    /// Ended; `changes` follow once computed.
    Done,
    /// gilvt quit while the turn ran: there is no `after`.
    Lost,
    /// The directory is not a git repository.
    NotGit,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TurnRecord {
    /// 1-based among the turns gilvt recorded for the session.
    pub seq: u32,
    /// First line of the prompt, ≤ [`PROMPT_MAX`] chars.
    pub prompt: String,
    /// Unix milliseconds; 0 when unknown.
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
    pub repo_root: Option<String>,
    pub before: Option<TreeId>,
    pub after: Option<TreeId>,
    pub changes: Option<Vec<FileChange>>,
    pub skipped_large: Vec<String>,
    pub state: TurnState,
}

/// A snapshot that has to be taken for the record `seq`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    Before(u32),
    After(u32),
}

/// What taking a snapshot came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Taken {
    Tree { tree: TreeId, skipped_large: Vec<String>, repo_root: String },
    NotGit,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    pub turns: Vec<TurnRecord>,
}

/// The first non-blank line of `prompt`, trimmed, ≤ [`PROMPT_MAX`] chars (what a record stores and what the
/// cards match timeline turns by).
pub fn prompt_head(prompt: &str) -> String {
    let line = prompt.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    line.chars().take(PROMPT_MAX).collect()
}

impl Ledger {
    /// `<dir>/artifacts/<agent>-<session>.json`; anything but `[A-Za-z0-9_-]` in the names becomes `_`.
    pub fn path(dir: &Path, agent: &str, session: &str) -> PathBuf {
        let clean = |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect::<String>();
        dir.join("artifacts").join(format!("{}-{}.json", clean(agent), clean(session)))
    }

    /// The ledger at `path`; a missing or unreadable file is an empty ledger. Turns left `Running` by a
    /// previous run can no longer end: they become `Lost`.
    pub fn load(path: &Path) -> Ledger {
        let mut ledger: Ledger = fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        for t in ledger.turns.iter_mut().filter(|t| t.state == TurnState::Running) {
            t.state = TurnState::Lost;
        }
        ledger
    }

    /// Writes through a temp file + rename.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec(self).map_err(io::Error::other)?)?;
        fs::rename(&tmp, path)
    }

    fn running(&self) -> Option<&TurnRecord> {
        self.turns.last().filter(|t| t.ended_ms.is_none() && t.state != TurnState::Lost)
    }

    fn record_mut(&mut self, seq: u32) -> Option<&mut TurnRecord> {
        self.turns.iter_mut().find(|t| t.seq == seq)
    }

    /// A prompt was submitted. The same prompt within [`SAME_TURN_MS`] of a running turn's start is that turn
    /// seen twice (the hook and the transcript both report it). Any other prompt means the running turn's
    /// end was never seen: it is closed as `Lost` and a new turn starts.
    pub fn on_prompt(&mut self, at_ms: u64, prompt: &str) -> Option<Want> {
        let head = prompt_head(prompt);
        if let Some(running) = self.running() {
            let near = at_ms.abs_diff(running.started_ms) <= SAME_TURN_MS || running.started_ms == 0 || at_ms == 0;
            if running.prompt == head && near {
                return None;
            }
            let seq = running.seq;
            if let Some(t) = self.record_mut(seq) {
                if t.state == TurnState::Running {
                    t.state = TurnState::Lost;
                }
            }
        }
        let seq = self.turns.last().map_or(1, |t| t.seq + 1);
        self.turns.push(TurnRecord {
            seq,
            prompt: head,
            started_ms: at_ms,
            ended_ms: None,
            repo_root: None,
            before: None,
            after: None,
            changes: None,
            skipped_large: Vec::new(),
            state: TurnState::Running,
        });
        Some(Want::Before(seq))
    }

    /// The turn ended (Stop, or the user interrupted it). Nothing to end: nothing to do.
    pub fn on_turn_end(&mut self, at_ms: u64) -> Option<Want> {
        let seq = self.running()?.seq;
        self.record_mut(seq)?.ended_ms = Some(at_ms);
        Some(Want::After(seq))
    }

    pub fn before_taken(&mut self, seq: u32, taken: Taken) {
        let Some(t) = self.record_mut(seq) else { return };
        match taken {
            Taken::Tree { tree, skipped_large, repo_root } => {
                t.before = Some(tree);
                t.repo_root = Some(repo_root);
                t.skipped_large = skipped_large;
            }
            Taken::NotGit => t.state = TurnState::NotGit,
            Taken::Failed(m) => t.state = TurnState::Failed(m),
        }
    }

    pub fn after_taken(&mut self, seq: u32, taken: Taken) {
        let Some(t) = self.record_mut(seq) else { return };
        match taken {
            Taken::Tree { tree, skipped_large, repo_root } => {
                t.after = Some(tree);
                t.repo_root.get_or_insert(repo_root);
                for f in skipped_large {
                    if !t.skipped_large.contains(&f) {
                        t.skipped_large.push(f);
                    }
                }
                if t.state == TurnState::Running {
                    t.state = TurnState::Done;
                }
            }
            Taken::NotGit => t.state = TurnState::NotGit,
            Taken::Failed(m) => {
                if t.state == TurnState::Running {
                    t.state = TurnState::Failed(m);
                }
            }
        }
    }

    pub fn changes_done(&mut self, seq: u32, changes: Result<Vec<FileChange>, String>) {
        let Some(t) = self.record_mut(seq) else { return };
        match changes {
            Ok(c) => t.changes = Some(c),
            Err(m) => t.state = TurnState::Failed(m),
        }
    }

    /// Both trees of the record `seq`, when it has them.
    pub fn trees(&self, seq: u32) -> Option<(&TreeId, &TreeId)> {
        let t = self.turns.iter().find(|t| t.seq == seq)?;
        Some((t.before.as_ref()?, t.after.as_ref()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChangeStatus;

    fn tree(s: &str) -> Taken {
        Taken::Tree { tree: TreeId(s.into()), skipped_large: Vec::new(), repo_root: "/r".into() }
    }

    fn change(path: &str) -> FileChange {
        FileChange { path: path.into(), old_path: None, status: ChangeStatus::M, added: 1, removed: 0, binary: false }
    }

    #[test]
    fn a_turn_goes_through_its_states() {
        let mut l = Ledger::default();
        assert_eq!(l.on_prompt(1000, "  fix the bug\nplease"), Some(Want::Before(1)));
        assert_eq!(l.turns[0].prompt, "fix the bug");
        l.before_taken(1, tree("aaa"));
        assert_eq!(l.on_turn_end(2000), Some(Want::After(1)));
        l.after_taken(1, tree("bbb"));
        l.changes_done(1, Ok(vec![change("a.rs")]));
        let t = &l.turns[0];
        assert_eq!((t.state.clone(), t.ended_ms, t.repo_root.as_deref()), (TurnState::Done, Some(2000), Some("/r")));
        assert_eq!(l.trees(1), Some((&TreeId("aaa".into()), &TreeId("bbb".into()))));
        assert_eq!(t.changes.as_ref().unwrap()[0].path, "a.rs");
        assert_eq!(l.on_prompt(3000, "next"), Some(Want::Before(2)));
    }

    #[test]
    fn a_second_prompt_or_end_is_the_same_turn_seen_twice() {
        let mut l = Ledger::default();
        assert_eq!(l.on_turn_end(1), None, "no turn to end");
        l.on_prompt(10, "a");
        assert_eq!(l.on_prompt(11, "a"), None, "the prompt hook and the transcript both report it");
        assert_eq!(l.turns.len(), 1);
        assert_eq!(l.on_prompt(10 + SAME_TURN_MS + 1, "a"), Some(Want::Before(2)), "the same words much later are a new turn");
        assert_eq!(l.turns[0].state, TurnState::Lost, "the turn whose end was never seen is closed");
        l.turns.truncate(1);
        l.turns[0].state = TurnState::Running;
        l.on_turn_end(20);
        assert_eq!(l.on_turn_end(21), None);
        assert_eq!(l.turns[0].ended_ms, Some(20));
    }

    #[test]
    fn a_different_prompt_closes_a_turn_whose_end_was_never_seen() {
        let mut l = Ledger::default();
        l.on_prompt(10, "first");
        l.before_taken(1, tree("a"));
        assert_eq!(l.on_prompt(20, "second"), Some(Want::Before(2)), "the user interrupted: no hook says so");
        assert_eq!((l.turns[0].state.clone(), l.turns[0].ended_ms), (TurnState::Lost, None));
        assert_eq!(l.turns[1].state, TurnState::Running);
        assert_eq!(l.on_turn_end(30), Some(Want::After(2)));
    }

    #[test]
    fn failures_are_recorded_not_hidden() {
        let mut l = Ledger::default();
        l.on_prompt(1, "a");
        l.before_taken(1, Taken::NotGit);
        assert_eq!(l.turns[0].state, TurnState::NotGit);
        l.on_turn_end(2);
        l.after_taken(1, Taken::Failed("boom".into()));
        assert_eq!(l.turns[0].state, TurnState::NotGit, "the first verdict stays");
        l.on_prompt(3, "b");
        l.before_taken(2, Taken::Failed("git: x".into()));
        assert_eq!(l.turns[1].state, TurnState::Failed("git: x".into()));
        l.on_prompt(4, "c");
        l.before_taken(3, tree("t1"));
        l.on_turn_end(5);
        l.after_taken(3, tree("t2"));
        l.changes_done(3, Err("diff failed".into()));
        assert_eq!(l.turns[2].state, TurnState::Failed("diff failed".into()));
    }

    #[test]
    fn skipped_large_files_are_merged() {
        let mut l = Ledger::default();
        l.on_prompt(1, "a");
        l.before_taken(1, Taken::Tree { tree: TreeId("a".into()), skipped_large: vec!["big".into()], repo_root: "/r".into() });
        l.on_turn_end(2);
        l.after_taken(1, Taken::Tree { tree: TreeId("b".into()), skipped_large: vec!["big".into(), "huge".into()], repo_root: "/r".into() });
        assert_eq!(l.turns[0].skipped_large, vec!["big".to_string(), "huge".to_string()]);
    }

    #[test]
    fn save_and_load_round_trip_and_running_turns_are_lost() {
        let dir = tempfile::tempdir().unwrap();
        let path = Ledger::path(dir.path(), "claude", "s/1:x");
        assert!(path.ends_with("artifacts/claude-s_1_x.json"));
        let mut l = Ledger::default();
        l.on_prompt(1, "done one");
        l.before_taken(1, tree("a"));
        l.on_turn_end(2);
        l.after_taken(1, tree("b"));
        l.on_prompt(3, "interrupted by quitting");
        l.before_taken(2, tree("c"));
        l.save(&path).unwrap();
        assert!(!path.with_extension("json.tmp").exists());
        let back = Ledger::load(&path);
        assert_eq!(back.turns[0], l.turns[0]);
        assert_eq!(back.turns[1].state, TurnState::Lost);
        assert_eq!(back.turns[1].before, Some(TreeId("c".into())));
    }

    #[test]
    fn a_missing_or_corrupt_file_is_an_empty_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.json");
        assert_eq!(Ledger::load(&path), Ledger::default());
        fs::write(&path, b"{ not json").unwrap();
        assert_eq!(Ledger::load(&path), Ledger::default());
    }

    #[test]
    fn a_duplicate_prompt_is_ignored_even_after_the_snapshot_failed() {
        let mut l = Ledger::default();
        l.on_prompt(10, "a");
        l.before_taken(1, Taken::NotGit);
        assert_eq!(l.on_prompt(11, "a"), None, "same prompt within SAME_TURN_MS is ignored even after NotGit");
        assert_eq!(l.turns.len(), 1);
    }

    #[test]
    fn a_turn_whose_snapshot_failed_still_gets_its_end() {
        let mut l = Ledger::default();
        l.on_prompt(10, "a");
        l.before_taken(1, Taken::Failed("x".into()));
        assert_eq!(l.on_turn_end(20), Some(Want::After(1)), "can still end a failed turn");
        assert_eq!(l.turns[0].ended_ms, Some(20));
        assert_eq!(l.turns[0].state, TurnState::Failed("x".into()), "state stays Failed");
    }

    #[test]
    fn a_new_prompt_keeps_a_degraded_verdict() {
        let mut l = Ledger::default();
        l.on_prompt(10, "a");
        l.before_taken(1, Taken::NotGit);
        assert_eq!(l.on_prompt(20, "b"), Some(Want::Before(2)), "new prompt starts a new turn");
        assert_eq!(l.turns[0].state, TurnState::NotGit, "first turn keeps NotGit, not Lost");
    }

    #[test]
    fn the_dedupe_window_includes_its_edge_and_unknown_times() {
        let mut l = Ledger::default();
        l.on_prompt(1000, "a");
        assert_eq!(l.on_prompt(1000 + SAME_TURN_MS, "a"), None, "edge of window is included");

        let mut l2 = Ledger::default();
        l2.on_prompt(0, "a");
        assert_eq!(l2.on_prompt(999_999, "a"), None, "unknown times (0) mean same turn");
    }
}
