//! The turn recorder: a background thread that takes the snapshots around each agent turn and keeps the
//! per-session ledgers (`gilvt_snapshot::ledger`). Windows only read copies of the ledgers; nothing here
//! blocks the main thread.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gilvt_agent::{AgentKind, Event, SessionKey};
use gilvt_snapshot::ledger::{Ledger, Taken, TurnRecord, TurnState, Want};
use gilvt_snapshot::{prune, FileChange, ObjectStore, SnapshotError, TreeId, KEEP};

use super::tails::Batch;

/// A turn boundary that some events report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signal {
    Prompt(String),
    End,
}

/// Where the events came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Hook,
    Transcript,
}

/// The turn boundaries in `events`. Hooks are the live source. A transcript adds what no hook reports: the
/// user interrupting a turn (Esc) and API errors; and, for sessions without hooks (精简模式, `lite`), all of it.
pub fn signals(events: &[Event], source: Source, lite: bool) -> Vec<Signal> {
    let hooks_or_lite = source == Source::Hook || lite;
    events
        .iter()
        .filter_map(|e| match e {
            Event::PromptSubmit { text } if hooks_or_lite => Some(Signal::Prompt(text.clone())),
            Event::TurnEnd { .. } if hooks_or_lite => Some(Signal::End),
            Event::Interrupted | Event::Error { .. } => Some(Signal::End),
            _ => None,
        })
        .collect()
}

pub fn batch_signals(batch: &Batch, lite: bool) -> Vec<Signal> {
    match batch {
        Batch::Live(events) => signals(events, Source::Transcript, lite),
        Batch::Catchup { .. } => Vec::new(),
    }
}


/// A snapshot slower than this is logged.
const SLOW: Duration = Duration::from_secs(2);
/// At most one live snapshot per session this often (more rarely when the last one was slow).
const LIVE_EVERY: Duration = Duration::from_secs(2);

/// Whether a live snapshot may run now: the wait is `LIVE_EVERY` or four times the last live take, whichever
/// is longer, and doubled again when that take was slower than `SLOW`, so a big repository never keeps the
/// recorder busy back to back while real turn-boundary snapshots queue behind it.
fn live_due(last: Option<(Instant, Duration)>, now: Instant) -> bool {
    let Some((at, took)) = last else { return true };
    let mut wait = LIVE_EVERY.max(took * 4);
    if took > SLOW {
        wait *= 2;
    }
    now.saturating_duration_since(at) >= wait
}

fn agent_name(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
    }
}

fn ms(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Cross-turn diffs kept in memory (oldest dropped first).
pub const DIFF_CACHE: usize = 256;

type DiffKey = (TreeId, TreeId);

#[derive(Debug, PartialEq)]
enum Cmd {
    Signal { key: SessionKey, cwd: Option<PathBuf>, at_ms: u64, signal: Signal },
    Live { key: SessionKey, cwd: Option<PathBuf>, base: Option<TreeId> },
    Diff { repo_root: PathBuf, before: TreeId, after: TreeId },
}

/// One round of the worker's work, in order: every turn boundary and live list that came in (as they came), then
/// at most one cross-turn diff; the other diffs wait in `diffs`. A restored session, or the monitor building
/// every live session's artifacts, can queue diffs by the dozen, and a `before` snapshot must not wait behind
/// them.
fn round(diffs: &mut VecDeque<Cmd>, incoming: impl IntoIterator<Item = Cmd>) -> Vec<Cmd> {
    let mut now = Vec::new();
    for cmd in incoming {
        match cmd {
            Cmd::Diff { .. } => diffs.push_back(cmd),
            cmd => now.push(cmd),
        }
    }
    now.extend(diffs.pop_front());
    now
}

/// What a diff against a pruned store reads.
const PRUNED: &str = "快照已清理";

fn pruned() -> String {
    crate::i18n::text(PRUNED, "Snapshot was cleaned up").into()
}

fn no_state_dir() -> String {
    crate::i18n::text("没有状态目录", "No state directory").into()
}

#[derive(Default)]
struct Shared {
    ledgers: HashMap<SessionKey, Ledger>,
    live: HashMap<SessionKey, Vec<FileChange>>,
    diffs: HashMap<DiffKey, Result<Vec<FileChange>, String>>,
    diff_order: VecDeque<DiffKey>,
    diff_pending: HashSet<DiffKey>,
}

/// Handle to the recorder thread. Cheap to call from the main thread: every method either sends a message
/// or copies a small value.
pub struct Recorder {
    tx: Sender<Cmd>,
    shared: Arc<Mutex<Shared>>,
    state_dir: Option<PathBuf>,
}

impl Recorder {
    /// Starts the thread (it first prunes stale snapshot stores). `wake` gets a message whenever a ledger or
    /// live list changed, so the windows redraw.
    pub fn spawn(state_dir: Option<PathBuf>, wake: async_channel::Sender<()>) -> Recorder {
        let (tx, rx) = channel();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let worker = Worker { state_dir: state_dir.clone(), shared: shared.clone(), wake, last_live: HashMap::new() };
        let _ = std::thread::Builder::new().name("gilvt-recorder".into()).spawn(move || worker.run(rx));
        Recorder { tx, shared, state_dir }
    }

    /// Turn boundaries seen for `key` (whose working directory is `cwd`) at `at`.
    pub fn signals(&self, key: &SessionKey, cwd: Option<PathBuf>, signals: Vec<Signal>, at: SystemTime) {
        for signal in signals {
            let _ = self.tx.send(Cmd::Signal { key: key.clone(), cwd: cwd.clone(), at_ms: ms(at), signal });
        }
    }

    /// Asks for a fresh list of the files changed since `base` (the running turn's own `before` when None;
    /// throttled by the thread).
    pub fn request_live(&self, key: &SessionKey, cwd: Option<PathBuf>, base: Option<TreeId>) {
        let _ = self.tx.send(Cmd::Live { key: key.clone(), cwd, base });
    }

    /// The files that differ between two snapshots of `repo_root`, once computed. A miss starts the work on the
    /// recorder thread (once per pair) and returns None; `wake` fires when it is done.
    pub fn diff(&self, repo_root: &Path, before: &TreeId, after: &TreeId) -> Option<Result<Vec<FileChange>, String>> {
        let k = (before.clone(), after.clone());
        let mut shared = self.shared.lock().unwrap();
        if let Some(r) = shared.diffs.get(&k) {
            return Some(r.clone());
        }
        if shared.diff_pending.insert(k) {
            let _ = self.tx.send(Cmd::Diff { repo_root: repo_root.to_path_buf(), before: before.clone(), after: after.clone() });
        }
        None
    }

    /// The turns recorded for `key`, oldest first. A session the thread has not touched yet is read from disk.
    pub fn records(&self, key: &SessionKey) -> Vec<TurnRecord> {
        let mut shared = self.shared.lock().unwrap();
        if !shared.ledgers.contains_key(key) {
            let Some(dir) = &self.state_dir else { return Vec::new() };
            let ledger = Ledger::load(&Ledger::path(dir, agent_name(key.0), &key.1));
            shared.ledgers.insert(key.clone(), ledger);
        }
        shared.ledgers[key].turns.clone()
    }

    pub fn live(&self, key: &SessionKey) -> Option<Vec<FileChange>> {
        self.shared.lock().unwrap().live.get(key).cloned()
    }
}

struct Worker {
    state_dir: Option<PathBuf>,
    shared: Arc<Mutex<Shared>>,
    wake: async_channel::Sender<()>,
    last_live: HashMap<SessionKey, (Instant, Duration)>,
}

impl Worker {
    fn run(mut self, rx: Receiver<Cmd>) {
        if let Some(dir) = &self.state_dir {
            let _ = prune(dir, KEEP);
        }
        // The thread's own copies are authoritative; `shared` holds what the windows read.
        let mut ledgers: HashMap<SessionKey, Ledger> = HashMap::new();
        let mut diffs: VecDeque<Cmd> = VecDeque::new();
        loop {
            // Block only when no diff waits; otherwise take what is there and run one diff after it.
            let first = if diffs.is_empty() {
                match rx.recv() {
                    Ok(cmd) => Some(cmd),
                    Err(_) => return,
                }
            } else {
                None
            };
            for cmd in round(&mut diffs, first.into_iter().chain(rx.try_iter())) {
                self.handle(cmd, &mut ledgers);
            }
        }
    }

    /// Runs one command (`ledgers` are the thread's own copies).
    fn handle(&mut self, cmd: Cmd, ledgers: &mut HashMap<SessionKey, Ledger>) {
        match cmd {
            Cmd::Signal { key, cwd, at_ms, signal } => {
                let ledger = ledgers.entry(key.clone()).or_insert_with(|| self.load(&key));
                let want = match signal {
                    Signal::Prompt(text) => ledger.on_prompt(at_ms, &text),
                    Signal::End => ledger.on_turn_end(at_ms),
                };
                if let Some(want) = want {
                    self.exec(&key, ledger, cwd.as_deref(), want);
                }
                self.publish(&key, ledger);
            }
            Cmd::Live { key, cwd, base } => {
                if !live_due(self.last_live.get(&key).copied(), Instant::now()) {
                    return;
                }
                let Some(ledger) = ledgers.get(&key) else { return };
                let Some(own) = ledger.turns.last().filter(|t| t.state == TurnState::Running).and_then(|t| t.before.clone()) else {
                    return;
                };
                let before = base.unwrap_or(own);
                let started = Instant::now();
                let (store, taken) = self.take(cwd.as_deref());
                self.last_live.insert(key.clone(), (Instant::now(), started.elapsed()));
                if let (Some(store), Taken::Tree { tree, .. }) = (store, taken) {
                    if let Ok(changes) = store.diff_trees(&before, &tree) {
                        self.shared.lock().unwrap().live.insert(key, changes);
                        let _ = self.wake.try_send(());
                    }
                }
            }
            Cmd::Diff { repo_root, before, after } => {
                let got = self.diff(&repo_root, &before, &after);
                let mut shared = self.shared.lock().unwrap();
                let k = (before, after);
                shared.diff_pending.remove(&k);
                if shared.diffs.insert(k.clone(), got).is_none() {
                    shared.diff_order.push_back(k);
                    while shared.diff_order.len() > DIFF_CACHE {
                        if let Some(old) = shared.diff_order.pop_front() {
                            shared.diffs.remove(&old);
                        }
                    }
                }
                drop(shared);
                let _ = self.wake.try_send(());
            }
        }
    }

    /// `before` -> `after` in `repo_root`'s store; a store or tree that is gone reads 「快照已清理」.
    fn diff(&self, repo_root: &Path, before: &TreeId, after: &TreeId) -> Result<Vec<FileChange>, String> {
        let state = self.state_dir.as_deref().ok_or_else(no_state_dir)?;
        let store = ObjectStore::open(repo_root, state).map_err(|_| pruned())?;
        store.diff_trees(before, after).map_err(|e| match e {
            SnapshotError::Missing => pruned(),
            e => e.to_string(),
        })
    }

    fn load(&self, key: &SessionKey) -> Ledger {
        self.state_dir.as_ref().map(|d| Ledger::load(&Ledger::path(d, agent_name(key.0), &key.1))).unwrap_or_default()
    }

    /// Takes the snapshot `want` asks for and records it (after a turn: also the files it changed).
    fn exec(&self, key: &SessionKey, ledger: &mut Ledger, cwd: Option<&Path>, want: Want) {
        let (store, taken) = self.take(cwd);
        match want {
            Want::Before(seq) => {
                ledger.before_taken(seq, taken);
                // A new turn must never show the previous turn's live files.
                self.shared.lock().unwrap().live.remove(key);
            }
            Want::After(seq) => {
                ledger.after_taken(seq, taken);
                self.shared.lock().unwrap().live.remove(key);
                let trees = ledger.trees(seq).map(|(b, a)| (b.clone(), a.clone()));
                if let (Some(store), Some((before, after))) = (store, trees) {
                    ledger.changes_done(seq, store.diff_trees(&before, &after).map_err(|e| e.to_string()));
                }
            }
        }
        if let Some(dir) = &self.state_dir {
            if let Err(e) = ledger.save(&Ledger::path(dir, agent_name(key.0), &key.1)) {
                eprintln!("gilvt: could not save the turn ledger of {} session {}: {e}", agent_name(key.0), key.1);
            }
        }
    }

    /// A snapshot of `cwd`'s repository, and the store it was taken in.
    fn take(&self, cwd: Option<&Path>) -> (Option<ObjectStore>, Taken) {
        let Some(cwd) = cwd else { return (None, Taken::Failed(crate::i18n::text("没有工作目录", "No working directory").into())) };
        let Some(state) = self.state_dir.as_deref() else { return (None, Taken::Failed(no_state_dir())) };
        let store = match ObjectStore::open(cwd, state) {
            Ok(s) => s,
            Err(SnapshotError::NotARepo) => return (None, Taken::NotGit),
            Err(e) => return (None, Taken::Failed(e.to_string())),
        };
        match store.take() {
            Ok(snap) => {
                if snap.took > SLOW {
                    eprintln!("gilvt: the snapshot of {} took {:?}", cwd.display(), snap.took);
                }
                let repo_root = store.repo_root().to_string_lossy().into_owned();
                (Some(store), Taken::Tree { tree: snap.tree, skipped_large: snap.skipped_large, repo_root })
            }
            Err(e) => (Some(store), Taken::Failed(e.to_string())),
        }
    }

    fn publish(&self, key: &SessionKey, ledger: &Ledger) {
        self.shared.lock().unwrap().ledgers.insert(key.clone(), ledger.clone());
        let _ = self.wake.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::tails::catchup;

    fn prompt(t: &str) -> Event {
        Event::PromptSubmit { text: t.into() }
    }

    fn end() -> Event {
        Event::TurnEnd { background_tasks: 0 }
    }

    #[test]
    fn hooks_report_prompts_and_turn_ends() {
        let events = [prompt("a"), Event::ToolStart { tool: "Bash".into(), summary: "ls".into() }, end()];
        assert_eq!(signals(&events, Source::Hook, false), vec![Signal::Prompt("a".into()), Signal::End]);
    }

    #[test]
    fn a_transcript_only_adds_interrupts_and_errors() {
        let events = [prompt("a"), end()];
        assert_eq!(signals(&events, Source::Transcript, false), vec![], "the hook already said both");
        assert_eq!(signals(&[Event::Interrupted], Source::Transcript, false), vec![Signal::End], "Esc fires no hook");
        assert_eq!(signals(&[Event::Error { message: "x".into() }], Source::Transcript, false), vec![Signal::End]);
    }

    #[test]
    fn a_session_without_hooks_is_recorded_from_its_transcript() {
        let events = [prompt("a"), end()];
        assert_eq!(signals(&events, Source::Transcript, true), vec![Signal::Prompt("a".into()), Signal::End]);
    }

    #[test]
    fn catch_up_batches_record_nothing() {
        let history = catchup(vec![prompt("old"), end()]);
        assert_eq!(batch_signals(&history, true), vec![]);
        assert_eq!(batch_signals(&Batch::Live(vec![Event::Interrupted]), false), vec![Signal::End]);
    }
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap().status.success();
        assert!(ok, "git {args:?}");
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "t@example.com"]);
        git(dir.path(), &["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        dir
    }

    /// Polls `f` until it returns Some, for at most 20 s.
    fn eventually<T>(mut f: impl FnMut() -> Option<T>) -> T {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(Instant::now() < until, "timed out");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn key() -> SessionKey {
        (AgentKind::Claude, "sess-1".into())
    }

    #[test]
    fn records_a_turn_end_to_end() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        let (wake, woken) = async_channel::unbounded();
        let rec = Recorder::spawn(Some(state.path().to_path_buf()), wake);
        let cwd = Some(r.path().to_path_buf());
        rec.signals(&key(), cwd.clone(), vec![Signal::Prompt("edit a".into())], SystemTime::now());
        eventually(|| rec.records(&key()).first().and_then(|t| t.before.clone()));
        std::fs::write(r.path().join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(r.path().join("new.txt"), "n\n").unwrap();
        rec.signals(&key(), cwd, vec![Signal::End], SystemTime::now());
        let turn = eventually(|| rec.records(&key()).first().filter(|t| t.changes.is_some()).cloned());
        assert_eq!(turn.state, TurnState::Done);
        let paths: Vec<&str> = turn.changes.as_ref().unwrap().iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, vec!["a.txt", "new.txt"]);
        assert!(woken.try_recv().is_ok(), "the window is told to redraw");
        assert!(Ledger::path(state.path(), "claude", "sess-1").exists(), "the ledger is persisted");
    }

    #[test]
    fn a_non_git_directory_is_recorded_as_such() {
        let (dir, state) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (wake, _woken) = async_channel::unbounded();
        let rec = Recorder::spawn(Some(state.path().to_path_buf()), wake);
        rec.signals(&key(), Some(dir.path().to_path_buf()), vec![Signal::Prompt("x".into())], SystemTime::now());
        let turn = eventually(|| rec.records(&key()).first().filter(|t| t.state != TurnState::Running).cloned());
        assert_eq!(turn.state, TurnState::NotGit);
    }

    #[test]
    fn a_ledger_on_disk_is_readable_before_anything_happens_and_running_turns_are_lost() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        let mut ledger = Ledger::default();
        ledger.on_prompt(1, "left running when gilvt quit");
        ledger.save(&Ledger::path(state.path(), "claude", "sess-1")).unwrap();
        let (wake, _woken) = async_channel::unbounded();
        let rec = Recorder::spawn(Some(state.path().to_path_buf()), wake);
        let _ = r;
        let turns = rec.records(&key());
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].state, TurnState::Lost);
    }

    #[test]
    fn a_running_turn_reports_live_changes() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        let (wake, _woken) = async_channel::unbounded();
        let rec = Recorder::spawn(Some(state.path().to_path_buf()), wake);
        let cwd = Some(r.path().to_path_buf());
        rec.signals(&key(), cwd.clone(), vec![Signal::Prompt("go".into())], SystemTime::now());
        eventually(|| rec.records(&key()).first().and_then(|t| t.before.clone()));
        std::fs::write(r.path().join("a.txt"), "one\nLIVE\n").unwrap();
        rec.request_live(&key(), cwd, None);
        let live = eventually(|| rec.live(&key()));
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].path, "a.txt");
    }

    #[test]
    fn a_new_turn_clears_the_previous_live_list() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        let (wake, _woken) = async_channel::unbounded();
        let rec = Recorder::spawn(Some(state.path().to_path_buf()), wake);
        let cwd = Some(r.path().to_path_buf());
        rec.signals(&key(), cwd.clone(), vec![Signal::Prompt("one".into())], SystemTime::now());
        eventually(|| rec.records(&key()).first().and_then(|t| t.before.clone()));
        std::fs::write(r.path().join("a.txt"), "one\nLIVE\n").unwrap();
        rec.request_live(&key(), cwd.clone(), None);
        eventually(|| rec.live(&key()));
        rec.signals(&key(), cwd, vec![Signal::Prompt("two".into())], SystemTime::now());
        eventually(|| rec.records(&key()).get(1).and_then(|t| t.before.clone()));
        assert_eq!(rec.live(&key()), None, "the second turn starts with no live files");
    }

    #[test]
    fn live_snapshots_back_off_with_their_cost() {
        let now = Instant::now();
        let ago = |s: u64| now - Duration::from_secs(s);
        assert!(live_due(None, now), "the first one is always due");
        assert!(!live_due(Some((ago(1), Duration::from_millis(10))), now), "inside the 2 s window");
        assert!(live_due(Some((ago(2), Duration::from_millis(10))), now));
        assert!(!live_due(Some((ago(3), Duration::from_secs(1))), now), "4 x 1 s = 4 s");
        assert!(live_due(Some((ago(4), Duration::from_secs(1))), now));
        assert!(!live_due(Some((ago(11), Duration::from_secs(3))), now), "slow: 4 x 3 s doubled = 24 s");
        assert!(live_due(Some((ago(24), Duration::from_secs(3))), now));
    }

    fn two_turns(rec: &Recorder, r: &tempfile::TempDir) -> (TreeId, TreeId) {
        let cwd = Some(r.path().to_path_buf());
        rec.signals(&key(), cwd.clone(), vec![Signal::Prompt("one".into())], SystemTime::now());
        eventually(|| rec.records(&key()).first().and_then(|t| t.before.clone()));
        std::fs::write(r.path().join("a.txt"), "one\nTWO\n").unwrap();
        rec.signals(&key(), cwd.clone(), vec![Signal::End], SystemTime::now());
        eventually(|| rec.records(&key()).first().filter(|t| t.changes.is_some()).cloned());
        rec.signals(&key(), cwd.clone(), vec![Signal::Prompt("two".into())], SystemTime::now());
        eventually(|| rec.records(&key()).get(1).and_then(|t| t.before.clone()));
        std::fs::write(r.path().join("a.txt"), "one\nTHREE\n").unwrap();
        rec.signals(&key(), cwd, vec![Signal::End], SystemTime::now());
        let t = rec.records(&key());
        let last = eventually(|| rec.records(&key()).get(1).filter(|t| t.changes.is_some()).cloned());
        (t[0].before.clone().unwrap(), last.after.unwrap())
    }

    #[test]
    fn a_diff_across_turns_is_computed_once_and_cached() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        let (wake, _woken) = async_channel::unbounded();
        let rec = Recorder::spawn(Some(state.path().to_path_buf()), wake);
        let (before, after) = two_turns(&rec, &r);
        let root = r.path().canonicalize().unwrap();
        assert_eq!(rec.diff(&root, &before, &after), None, "the first ask only starts the work");
        let got = eventually(|| rec.diff(&root, &before, &after)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].path.as_str(), got[0].added, got[0].removed), ("a.txt", 1, 1), "net: two -> THREE");
        assert_eq!(rec.shared.lock().unwrap().diff_pending.len(), 0);
    }

    #[test]
    fn a_diff_against_a_pruned_store_fails_cleanly() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        let (wake, _woken) = async_channel::unbounded();
        let rec = Recorder::spawn(Some(state.path().to_path_buf()), wake);
        let gone = TreeId("0".repeat(40));
        let got = { rec.diff(r.path(), &gone, &gone); eventually(|| rec.diff(r.path(), &gone, &gone)) };
        assert_eq!(got, Err("快照已清理".into()));
    }

    fn diff_cmd(n: &str) -> Cmd {
        Cmd::Diff { repo_root: PathBuf::from("/r"), before: TreeId(n.into()), after: TreeId(n.into()) }
    }

    fn signal_cmd(text: &str) -> Cmd {
        Cmd::Signal { key: key(), cwd: None, at_ms: 0, signal: Signal::Prompt(text.into()) }
    }

    #[test]
    fn turn_boundaries_and_live_lists_go_before_queued_diffs() {
        let live = || Cmd::Live { key: key(), cwd: None, base: None };
        let mut diffs = VecDeque::new();
        let got = round(&mut diffs, [diff_cmd("1"), signal_cmd("a"), diff_cmd("2"), live(), signal_cmd("b")]);
        assert_eq!(got, vec![signal_cmd("a"), live(), signal_cmd("b"), diff_cmd("1")], "everything urgent, in order, then one diff");
        assert_eq!(diffs, VecDeque::from([diff_cmd("2")]));
        assert_eq!(round(&mut diffs, [signal_cmd("c")]), vec![signal_cmd("c"), diff_cmd("2")], "a later boundary still jumps the queue");
        assert_eq!(round(&mut diffs, []), vec![]);
    }

    #[test]
    fn a_failed_diff_stays_cached_and_is_not_queued_again() {
        // Without a state directory every diff fails with 「没有状态目录」.
        let (wake, _woken) = async_channel::unbounded();
        let rec = Recorder::spawn(None, wake);
        let t = TreeId("1".repeat(40));
        assert_eq!(rec.diff(Path::new("/r"), &t, &t), None);
        let got = eventually(|| rec.diff(Path::new("/r"), &t, &t));
        assert_eq!(got, Err("没有状态目录".into()));
        assert_eq!(rec.diff(Path::new("/r"), &t, &t), Some(Err("没有状态目录".into())), "asked again: the same answer");
        assert!(rec.shared.lock().unwrap().diff_pending.is_empty(), "and nothing is queued again");
    }

    #[test]
    fn live_uses_the_given_base() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        let (wake, _woken) = async_channel::unbounded();
        let rec = Recorder::spawn(Some(state.path().to_path_buf()), wake);
        let cwd = Some(r.path().to_path_buf());
        rec.signals(&key(), cwd.clone(), vec![Signal::Prompt("one".into())], SystemTime::now());
        let first = eventually(|| rec.records(&key()).first().and_then(|t| t.before.clone()));
        std::fs::write(r.path().join("b.txt"), "b\n").unwrap();
        rec.signals(&key(), cwd.clone(), vec![Signal::End], SystemTime::now());
        eventually(|| rec.records(&key()).first().filter(|t| t.changes.is_some()).cloned());
        rec.signals(&key(), cwd.clone(), vec![Signal::Prompt("continue".into())], SystemTime::now());
        eventually(|| rec.records(&key()).get(1).and_then(|t| t.before.clone()));
        std::fs::write(r.path().join("a.txt"), "one\nLIVE\n").unwrap();
        rec.request_live(&key(), cwd, Some(first));
        let live = eventually(|| rec.live(&key()));
        let paths: Vec<&str> = live.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, vec!["a.txt", "b.txt"], "measured from the task's first turn");
    }
}
