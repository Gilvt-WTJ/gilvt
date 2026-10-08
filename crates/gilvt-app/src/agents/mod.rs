//! The app side of agent status: one `Agents` global (the session registry of all windows) fed by
//! `gilvt hook` requests, transcript tails and a foreground poll over the terminal panes. Parsing and
//! file IO run off the main thread; changes are applied on it and repaint the windows. The same inputs
//! feed one timeline per session on the tail thread (M3b); the global holds their latest snapshots.

mod foreground;
mod pending;
mod process;
mod project;
mod recorder;
mod state;
mod tails;
mod timelines;
mod worker;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gilvt_agent::{newest_claude_transcript, GitInfo, newest_codex_rollout, scan_agent_processes, AgentKind, BindingConfidence, Change, Lease, LeaseStore, PaneId, ProcessObservation, Registry, RuntimeRef, Session, SessionKey, Store, TerminalOwner};
use gilvt_ipc::Request;
use gpui::{App, AsyncApp, Global};

use self::recorder::{batch_signals, signals as hook_signals, Recorder, Source};
use self::state::{parse_hook_request, Core, Discovery};
pub use self::foreground::is_shell;
pub use self::pending::{resume_allowed, PendingResume};
pub use self::process::describe as describe_process;
pub use self::project::{fallback_name, project_root};
pub use self::state::PaneKey;
use self::foreground::Foreground;
use self::tails::{Out, Tails};
use self::timelines::{anchor_for, HookFeed};
pub use self::timelines::TimelineView;
use crate::theme::AppSettings;
use crate::workspace;

/// How often the foreground of every terminal pane is checked.
const POLL: Duration = Duration::from_secs(1);
/// Relative times in the sidebar ("2 分钟") are redrawn this often while sessions are live.
const CLOCK_POLLS: u32 = 20;
/// Polls between git refreshes of the sessions' repositories (the poll runs once a second).
const GIT_POLLS: u32 = 10;
/// Session id prefix for processes that cannot be associated with a durable Agent session.
const UNBOUND_PREFIX: &str = "__gilvt_unbound_";

/// `~/Library/Application Support/gilvt/state` (renames, mutes, ui.json, the codex trust cache).
pub fn state_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join("Library/Application Support/gilvt/state"))
}

pub struct Agents {
    core: Core,
    tails: Option<Tails>,
    watching: Vec<(SessionKey, PathBuf)>,
    /// Project names of session directories, resolved in the background.
    projects: HashMap<PathBuf, String>,
    resolving: HashSet<PathBuf>,
    /// Git facts per session directory; None = looked up, not a repository. Refreshed in the background.
    git: HashMap<PathBuf, Option<GitInfo>>,
    git_resolving: HashSet<PathBuf>,
    /// Directories of the terminal panes the sidebar lists: resolved like a session's (project, git).
    term_dirs: HashSet<PathBuf>,
    /// Latest timeline snapshot per session (ended sessions keep theirs for this run).
    timelines: HashMap<SessionKey, Arc<TimelineView>>,
    recorder: Recorder,
    /// Sessions from the saved layout, not resumed yet.
    pending: pending::PendingSet,
    /// Processes discovered outside gilvt, keyed by the session row they currently back.
    runtimes: HashMap<SessionKey, RuntimeRef>,
}

impl Global for Agents {}

impl Agents {
    /// The snapshot recorder (the 「产物」 tab reads its ledgers).
    pub fn recorder(&self) -> &Recorder {
        &self.recorder
    }

    pub fn registry(&self) -> &Registry {
        &self.core.registry
    }

    pub fn runtime(&self, key: &SessionKey) -> Option<&RuntimeRef> {
        self.runtimes.get(key)
    }

    /// The session's timeline as of its latest change (None until something reached it). A new `Arc` per
    /// change, with [`TimelineView::revision`] + 1.
    pub fn timeline(&self, key: &SessionKey) -> Option<Arc<TimelineView>> {
        self.timelines.get(key).cloned()
    }

    /// The session's latest hook `permission_mode` (None: no hook carried one yet, e.g. 精简模式).
    pub fn permission_mode(&self, key: &SessionKey) -> Option<&str> {
        self.core.permission_mode(key)
    }

    /// M3c §2.1: `pane` is a terminal sitting at an idle shell, per the latest foreground poll (a pane not
    /// polled yet, or whose lookup failed, is not).
    pub fn pane_is_idle_shell(&self, pane: PaneId) -> bool {
        self.core.pane_is_idle_shell(pane)
    }

    /// The launcher typed a command into `pane`: not idle again until a poll sees the shell in front.
    pub fn typed(pane: PaneId, cx: &mut App) {
        if cx.has_global::<Agents>() {
            cx.global_mut::<Agents>().core.typed(pane);
        }
    }

    /// Renames a session (trimmed; "" brings the automatic name back). Persisted by the store.
    pub fn rename(key: &SessionKey, name: String, cx: &mut App) {
        cx.global_mut::<Agents>().core.registry.rename(key, name);
        cx.defer(workspace::notify_all);
    }

    /// Mutes / unmutes a session's notifications (🔕). Persisted by the store.
    pub fn set_muted(key: &SessionKey, muted: bool, cx: &mut App) {
        cx.global_mut::<Agents>().core.registry.set_muted(key, muted);
        // A muted session leaves the Dock badge's count (no registry change reaches `after_changes`).
        crate::notify::dock_changed(cx);
        cx.defer(workspace::notify_all);
    }

    /// Sessions whose files went to the Trash (M3c §4.3): ended ones leave the sidebar's 已结束 group and
    /// their timelines are dropped; live ones stay.
    pub fn forget(keys: &[SessionKey], cx: &mut App) {
        if !cx.has_global::<Agents>() {
            return;
        }
        let mut dropped = false;
        {
            let agents = cx.global_mut::<Agents>();
            for key in keys {
                if agents.core.forget(key) {
                    agents.timelines.remove(key);
                    dropped = true;
                }
            }
        }
        crate::monitor::summaries::forget(keys, cx);
        if dropped {
            cx.defer(workspace::notify_all);
        }
    }

    /// The session's project: git root name, else the cwd's last component.
    pub fn project(&self, s: &Session) -> String {
        match &s.cwd {
            Some(cwd) => self.project_of_dir(cwd),
            None => crate::i18n::text("其他", "Other").into(),
        }
    }

    /// A directory's project: its repository's name once looked up, else the resolved git-root name, else the last component.
    pub fn project_of_dir(&self, cwd: &Path) -> String {
        if let Some(g) = self.git_of_dir(cwd) {
            return g.main_repo_name();
        }
        self.projects.get(cwd).cloned().unwrap_or_else(|| project::fallback_name(cwd))
    }

    /// The session's repository facts, if its directory is in a git repository and has been looked up.
    pub fn git(&self, s: &Session) -> Option<&GitInfo> {
        self.git_of_dir(s.cwd.as_ref()?)
    }

    /// Repository facts of a directory (None: not looked up yet, or not a repository).
    pub fn git_of_dir(&self, cwd: &Path) -> Option<&GitInfo> {
        self.git.get(cwd)?.as_ref()
    }

    /// Looks up (or refreshes) git facts of the live sessions' directories off the main thread. One query
    /// per directory; sessions that are thinking or running a tool are skipped when they already have a
    /// value, to keep big repositories quiet while the agent works.
    fn refresh_git(cx: &mut App) {
        let want: Vec<PathBuf> = {
            let a = cx.global::<Agents>();
            let mut v: Vec<PathBuf> = a
                .core
                .registry
                .sessions()
                .filter(|s| s.is_live())
                .filter_map(|s| {
                    let cwd = s.cwd.clone()?;
                    let known = a.git.contains_key(&cwd);
                    (!a.git_resolving.contains(&cwd) && (!known || !s.status.is_running())).then_some(cwd)
                })
                .collect();
            v.extend(a.term_dirs.iter().filter(|d| !a.git_resolving.contains(*d)).cloned());
            v.sort();
            v.dedup();
            v
        };
        for cwd in want {
            cx.global_mut::<Agents>().git_resolving.insert(cwd.clone());
            let bg = cwd.clone();
            cx.spawn(async move |cx| {
                let outcome = cx.background_executor().spawn(async move { gilvt_agent::query_outcome(&bg) }).await;
                let _ = cx.update(|cx| {
                    let a = cx.global_mut::<Agents>();
                    a.git_resolving.remove(&cwd);
                    if let Some(info) = git_cache_update(a.git.get(&cwd), outcome) {
                        a.git.insert(cwd, info);
                        cx.defer(workspace::notify_all);
                    }
                });
            })
            .detach();
        }
    }

    /// The directories of the terminal panes the sidebar lists (called every poll). A change resolves the new
    /// directories' projects at once and looks their git facts up without waiting for the 10 s refresh.
    pub fn track_dirs(dirs: Vec<PathBuf>, cx: &mut App) {
        let set: HashSet<PathBuf> = dirs.into_iter().collect();
        let agents = cx.global_mut::<Agents>();
        if agents.term_dirs == set {
            return;
        }
        let fresh = set.iter().any(|d| !agents.git.contains_key(d));
        agents.term_dirs = set;
        resolve_projects(cx);
        if fresh {
            Agents::refresh_git(cx);
        }
    }

    /// The agent bound to `pane` (live, else waiting to be resumed), for the layout snapshot.
    pub fn agent_snap(&self, pane: PaneId) -> Option<crate::persist::snapshot::AgentSnap> {
        if let Some(s) = self.core.registry.by_pane(pane) {
            return Some(crate::persist::snapshot::AgentSnap {
                kind: s.agent().name().into(),
                session_id: s.session_id().to_string(),
                name: s.name.clone(),
                last_status: gilvt_agent::status_label(&s.status),
            });
        }
        self.pending.for_pane(pane).map(PendingResume::to_snap)
    }

    /// Registers a session from a saved layout as waiting to be resumed in `pane`.
    pub fn add_pending(pane: PaneId, cwd: Option<PathBuf>, agent: &crate::persist::snapshot::AgentSnap, cx: &mut App) {
        let Some(p) = PendingResume::from_snap(pane, cwd, agent) else {
            eprintln!("gilvt: 恢复 pane {pane}：未知的 agent 类型「{}」，已忽略", agent.kind);
            return;
        };
        if cx.has_global::<Agents>() {
            cx.global_mut::<Agents>().pending.add(p);
        }
    }

    pub fn pending(&self) -> &[PendingResume] {
        self.pending.all()
    }

    /// The panes an agent occupies: those of live sessions and of sessions waiting to be resumed. Every other
    /// terminal pane is a plain terminal (the sidebar lists it quietly; its directory gets git facts).
    pub fn occupied_panes(&self) -> HashSet<PaneId> {
        occupied_panes(self.core.registry.sessions(), self.pending.all())
    }

    pub fn take_pending(key: &SessionKey, cx: &mut App) -> Option<PendingResume> {
        cx.try_global::<Agents>()?;
        cx.global_mut::<Agents>().pending.take(key)
    }

    /// A pane closed: its sessions end now rather than at the next poll.
    pub fn pane_closed(pane: PaneId, cx: &mut App) {
        if cx.has_global::<Agents>() {
            cx.global_mut::<Agents>().pending.drop_pane(pane);
        }
        crate::notify::pane_closed(pane, cx);
        let Some(agents) = cx.try_global::<Agents>() else { return };
        if agents.core.registry.by_pane(pane).is_some() {
            let changes = cx.global_mut::<Agents>().core.registry.pane_exited(pane, Instant::now());
            after_changes(changes, cx);
        }
    }

    /// An OSC 9 / 777 text from `pane`: a lite Codex session's approval signal (spec §2.3).
    pub fn osc(pane: PaneId, body: &str, visible: bool, cx: &mut App) {
        let Some(agents) = cx.try_global::<Agents>() else { return };
        if agents.core.registry.by_pane(pane).is_some_and(|s| s.lite) {
            let changes = cx.global_mut::<Agents>().core.osc(pane, body, visible, Instant::now());
            after_changes(changes, cx);
        }
    }

    /// A key the user typed into terminal pane `pane`: answers the approval dialog of a session waiting there
    /// (no hook reports the answer).
    pub fn pane_key(pane: PaneId, key: PaneKey, cx: &mut App) {
        let Some(agents) = cx.try_global::<Agents>() else { return };
        if !agents.core.registry.by_pane(pane).is_some_and(Session::needs_you) {
            return;
        }
        let visible = workspace::visible_panes(cx).contains(&pane);
        let agents = cx.global_mut::<Agents>();
        let Some((session, changes)) = agents.core.pane_key(pane, key, visible, Instant::now()) else { return };
        if let Some(tails) = &agents.tails {
            tails.approval_answered(session, SystemTime::now());
        }
        after_changes(changes, cx);
    }

    /// The user focused terminal pane `pane` (clears 完成未看 and the session's 已通知 record).
    pub fn pane_focused(pane: PaneId, cx: &mut App) {
        crate::notify::pane_focused(pane, cx);
        let Some(agents) = cx.try_global::<Agents>() else { return };
        if agents.core.registry.sessions().any(|s| s.pane == Some(pane) && s.unseen_done) {
            let changes = cx.global_mut::<Agents>().core.focused(pane);
            after_changes(changes, cx);
        }
    }
}

/// The new cache value for a directory after a lookup, None when the cache stays as it is. A failed lookup
/// (timeout, ...) never replaces what is known: the line must not flicker and the session must not regroup.
fn git_cache_update(cached: Option<&Option<GitInfo>>, outcome: gilvt_agent::QueryOutcome) -> Option<Option<GitInfo>> {
    use gilvt_agent::QueryOutcome;
    let new = match outcome {
        QueryOutcome::Info(i) => Some(i),
        QueryOutcome::NotRepo => None,
        QueryOutcome::Failed => return None,
    };
    (cached != Some(&new)).then_some(new)
}

/// Sets up the global and its background work. Returns where the IPC server sends `Hook` requests.
pub fn init(cx: &mut App) -> async_channel::Sender<Request> {
    let store = state_dir().map_or_else(Store::in_memory, |d| Store::open(&d));
    let (batch_tx, batch_rx) = async_channel::unbounded();
    let (wake_tx, wake_rx) = async_channel::unbounded::<()>();
    cx.set_global(Agents {
        core: Core::new(store),
        tails: Some(Tails::spawn(batch_tx)),
        watching: Vec::new(),
        projects: HashMap::new(),
        resolving: HashSet::new(),
        git: HashMap::new(),
        git_resolving: HashSet::new(),
        term_dirs: HashSet::new(),
        timelines: HashMap::new(),
        recorder: Recorder::spawn(state_dir(), wake_tx),
        pending: Default::default(),
        runtimes: HashMap::new(),
    });
    cx.spawn(async move |cx| {
        while wake_rx.recv().await.is_ok() {
            while wake_rx.try_recv().is_ok() {}
            if cx.update(workspace::notify_all).is_err() {
                break;
            }
        }
    })
    .detach();
    let (hook_tx, hook_rx) = async_channel::unbounded::<Request>();
    cx.spawn(async move |cx| {
        while let Ok(first) = hook_rx.recv().await {
            let mut requests = vec![first];
            requests.extend(std::iter::from_fn(|| hook_rx.try_recv().ok()));
            let at = SystemTime::now();
            let Ok(anchors) = cx.update(|cx| anchors(&requests, cx)) else { break };
            let parsed = cx
                .background_executor()
                .spawn(async move {
                    requests
                        .into_iter()
                        .zip(anchors)
                        .filter_map(|(r, anchor)| match r {
                            Request::Hook { pane, agent, event, payload } => {
                                let h = parse_hook_request(pane, &agent, &event, &payload)?;
                                Some((h, HookFeed { event, payload, anchor, at }))
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let applied = cx.update(|cx| {
                let visible = workspace::visible_panes(cx);
                let now = Instant::now();
                let agents = cx.global_mut::<Agents>();
                let mut changes: Vec<Change> = Vec::new();
                let mut modes: Vec<SessionKey> = Vec::new();
                for (h, feed) in parsed {
                    changes.extend(agents.core.hook(&h, |p| visible.contains(&p), now));
                    if h.meta.agent_id.is_none() {
                        if let Some(key) = agents.core.timeline_key(&h) {
                            let cwd = agents.core.registry.get(&key).and_then(|s| s.cwd.clone()).or_else(|| h.meta.cwd.clone());
                            agents.recorder.signals(&key, cwd, hook_signals(&h.events, Source::Hook, false), feed.at);
                        }
                    }
                    modes.extend(agents.core.note_permission_mode(&h));
                    if let (Some(key), Some(tails)) = (agents.core.timeline_key(&h), &agents.tails) {
                        tails.hook(key, feed);
                    }
                }
                if changes.is_empty() && !modes.is_empty() {
                    // Only the status card's permission mode changed (`after_changes` repaints everything otherwise).
                    cx.defer(move |cx| workspace::notify_sessions(&modes, cx));
                }
                after_changes(changes, cx);
            });
            if applied.is_err() {
                break;
            }
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Ok(first) = batch_rx.recv().await {
            let mut batches = Vec::new();
            let mut views = Vec::new();
            for out in std::iter::once(first).chain(std::iter::from_fn(|| batch_rx.try_recv().ok())) {
                match out {
                    Out::Events(key, batch) => batches.push((key, batch)),
                    Out::Timeline(key, view) => views.push((key, view)),
                }
            }
            let applied = cx.update(|cx| {
                let visible = workspace::visible_panes(cx);
                let now = Instant::now();
                let agents = cx.global_mut::<Agents>();
                let keys: Vec<SessionKey> = views.iter().map(|(key, _)| key.clone()).collect();
                agents.timelines.extend(views);
                for (key, batch) in &batches {
                    let (lite, cwd) = agents.core.registry.get(key).map_or((false, None), |s| (s.lite, s.cwd.clone()));
                    agents.recorder.signals(key, cwd, batch_signals(batch, lite), SystemTime::now());
                }
                let core = &mut agents.core;
                let changes: Vec<Change> =
                    batches.iter().flat_map(|(key, batch)| core.transcript(key, batch, |p| visible.contains(&p), now)).collect();
                if changes.is_empty() && !keys.is_empty() {
                    // `after_changes` repaints every window otherwise.
                    cx.defer(move |cx| workspace::notify_sessions(&keys, cx));
                }
                after_changes(changes, cx);
            });
            if applied.is_err() {
                break;
            }
        }
    })
    .detach();
    cx.spawn(async move |cx| poll_loop(cx).await).detach();
    hook_tx
}

/// Every [`POLL`]: classify each terminal's foreground program, end sessions of panes back at the
/// shell, and bind agents that sent no hooks to their newest transcript.
async fn poll_loop(cx: &mut AsyncApp) {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let state = state_dir();
    let mut ticks = 0u32;
    loop {
        cx.background_executor().timer(POLL).await;
        // ✦ summaries: turn the last second's changes into refresh decisions (S2 §4.5).
        if cx.update(crate::monitor::summaries::tick).is_err() {
            return;
        }
        // The 监控官's chat: silent turns, ignored interrupts, idle processes (S2 §6.2).
        if cx.update(crate::monitor::chat::tick).is_err() {
            return;
        }
        let Ok((pids, claude, codex)) = cx.update(|cx| {
            let agent = &cx.global::<AppSettings>().0.agent;
            (workspace::terminal_pids(cx), agent.claude_commands.clone(), agent.codex_commands.clone())
        }) else {
            return;
        };
        let lease_state = state.clone();
        let (panes, leases, processes): (Vec<(PaneId, Foreground)>, Vec<Lease>, Vec<ProcessObservation>) = cx
            .background_executor()
            .spawn(async move {
                let panes = pids.into_iter()
                    .map(|(pane, pid)| (pane, pid.map_or(Foreground::Unknown, |pid| process::foreground_of(pid, &claude, &codex))))
                    .collect();
                let leases = lease_state.as_deref().map(|state| LeaseStore::open(state).read_all()).unwrap_or_default();
                (panes, leases, scan_agent_processes())
            })
            .await;
        ticks = ticks.wrapping_add(1);
        let Ok(searches) = cx.update(|cx| {
            let history = cx.try_global::<crate::launcher::History>().map(|history| history.entries()).unwrap_or_default();
            let agents = cx.global_mut::<Agents>();
            let mut changes = reconcile_external(agents, &leases, &processes, &history, Instant::now());
            let (pane_changes, searches) = agents.core.poll(&panes, Instant::now(), SystemTime::now());
            changes.extend(pane_changes);
            let clock = ticks % CLOCK_POLLS == 0 && cx.global::<Agents>().core.registry.sessions().any(Session::is_live);
            if changes.is_empty() && clock {
                workspace::notify_all(cx);
            }
            after_changes(changes, cx);
            Agents::track_dirs(workspace::terminal_dirs(cx), cx);
            if ticks % GIT_POLLS == 1 {
                Agents::refresh_git(cx);
            }
            searches
        }) else {
            return;
        };
        let Some(home) = home.clone() else { continue };
        for d in searches {
            let (home, search) = (home.clone(), d.clone());
            let found = cx.background_executor().spawn(async move { discover(&home, &search) }).await;
            if cx.update(|cx| {
                let changes = cx.global_mut::<Agents>().core.bind_found(&d, found, Instant::now());
                after_changes(changes, cx);
            })
            .is_err()
            {
                return;
            }
        }
    }
}

/// Reconciles durable hook leases and the system process scan into external session rows. Only an exact,
/// still-live lease grants process control; command-line hints are display/navigation hints only.
fn reconcile_external(
    agents: &mut Agents,
    leases: &[Lease],
    processes: &[ProcessObservation],
    history: &[gilvt_agent::HistoryEntry],
    now: Instant,
) -> Vec<Change> {
    let mut changes = Vec::new();
    let mut next = HashMap::new();
    let mut claimed = HashSet::new();

    for lease in leases {
        let Some(process) = processes.iter().find(|process| {
            process.agent == lease.agent
                && process.runtime.pid == lease.runtime.pid
                && process.runtime.process_started_at == lease.runtime.process_started_at
        }) else {
            continue;
        };
        let identity = (process.runtime.pid, process.runtime.process_started_at);
        claimed.insert(identity);
        if !lease.is_live() || process.runtime.terminal == TerminalOwner::Gilvt {
            continue;
        }

        let key = lease.key();
        let mut runtime = process.runtime.clone();
        runtime.confidence = BindingConfidence::Exact;
        changes.extend(agents.core.registry.bind_external(
            lease.agent,
            lease.session_id.clone(),
            lease.transcript.clone(),
            lease.cwd.clone(),
            true,
            now,
        ));
        next.insert(key, runtime);
    }

    for process in processes {
        let identity = (process.runtime.pid, process.runtime.process_started_at);
        if claimed.contains(&identity) || process.runtime.terminal == TerminalOwner::Gilvt {
            continue;
        }

        let matched = process.session_hint.as_deref().and_then(|session_id| {
            history
                .iter()
                .find(|entry| entry.agent == process.agent && entry.session_id == session_id)
        });
        let (session_id, transcript, cwd, confidence) = match matched {
            Some(entry) => (
                entry.session_id.clone(),
                Some(entry.transcript.clone()),
                Some(entry.cwd.clone()),
                BindingConfidence::Inferred,
            ),
            None => (
                format!(
                    "{UNBOUND_PREFIX}{}_{}",
                    process.runtime.pid, process.runtime.process_started_at
                ),
                None,
                None,
                BindingConfidence::Unresolved,
            ),
        };
        let key = (process.agent, session_id.clone());
        if next.contains_key(&key) {
            continue;
        }
        let mut runtime = process.runtime.clone();
        runtime.confidence = confidence;
        changes.extend(agents.core.registry.bind_external(
            process.agent,
            session_id,
            transcript,
            cwd,
            false,
            now,
        ));
        next.insert(key, runtime);
    }

    for key in agents.runtimes.keys().filter(|key| !next.contains_key(*key)) {
        if key.1.starts_with(UNBOUND_PREFIX) {
            if agents.core.registry.remove_external(key) {
                changes.push(Change::Ended(key.clone()));
            }
        } else {
            changes.extend(agents.core.registry.external_exited(key, now));
        }
    }
    agents.runtimes = next;
    changes
}

/// Terminal anchors of hook requests, read as they arrive: the pane's absolute cursor line for a session
/// `PreToolUse` (spec §5), else None. Other requests do not touch the terminals.
fn anchors(requests: &[Request], cx: &App) -> Vec<Option<gilvt_agent::Anchor>> {
    requests
        .iter()
        .map(|r| match r {
            Request::Hook { pane, event, payload, .. } => {
                anchor_for(*pane, event, payload, |p| workspace::absolute_cursor_line(p, cx))
            }
            _ => None,
        })
        .collect()
}

fn discover(home: &std::path::Path, d: &Discovery) -> Option<(String, PathBuf)> {
    match d.agent {
        AgentKind::Claude => newest_claude_transcript(home, &d.cwd, d.since),
        AgentKind::Codex => newest_codex_rollout(home, &d.cwd, d.since),
    }
}

/// After a batch: run the notification chain, retarget the tails, resolve new projects, and repaint the windows (deferred, so this
/// is safe while a window is being updated).
fn after_changes(changes: Vec<Change>, cx: &mut App) {
    if changes.is_empty() {
        return;
    }
    crate::notify::on_changes(&changes, cx);
    if changes.iter().any(|c| matches!(c, Change::Added(_))) {
        Agents::refresh_git(cx);
    }
    let agents = cx.global_mut::<Agents>();
    // A live session now carries the key: started by hand or resumed, no longer waiting.
    // Any other agent started by hand in that pane clears its marks too (resuming would type into it).
    for c in &changes {
        if let Change::Added(key) | Change::Updated(key) = c {
            let pane = agents.core.registry.get(key).and_then(|s| s.pane);
            agents.pending.session_live(key, pane);
        }
    }
    let set = agents.core.watch_set();
    if set != agents.watching {
        if let Some(tails) = &agents.tails {
            tails.watch(set.clone());
        }
        agents.watching = set;
    }
    resolve_projects(cx);
    cx.defer(workspace::notify_all);
}

/// Resolves, off the main thread, the project name of every session directory and terminal directory not resolved yet.
fn resolve_projects(cx: &mut App) {
    let agents = cx.global_mut::<Agents>();
    let unresolved: Vec<PathBuf> = agents
        .core
        .registry
        .sessions()
        .filter_map(|s| s.cwd.clone())
        .chain(agents.term_dirs.iter().cloned())
        .filter(|c| !agents.projects.contains_key(c) && !agents.resolving.contains(c))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    agents.resolving.extend(unresolved.iter().cloned());
    for cwd in unresolved {
        cx.spawn(async move |cx| {
            let dir = cwd.clone();
            let name = cx.background_executor().spawn(async move { project::project_name(&dir) }).await;
            let _ = cx.update(|cx| {
                let agents = cx.global_mut::<Agents>();
                agents.resolving.remove(&cwd);
                agents.projects.insert(cwd, name);
                workspace::notify_all(cx);
            });
        })
        .detach();
    }
}

/// See [`Agents::occupied_panes`].
fn occupied_panes<'a>(sessions: impl IntoIterator<Item = &'a Session>, pending: &[PendingResume]) -> HashSet<PaneId> {
    let live = sessions.into_iter().filter(|s| s.is_live()).filter_map(|s| s.pane);
    live.chain(pending.iter().map(|p| p.pane)).collect()
}

#[cfg(test)]
mod occupied_tests {
    use super::state::parse_hook_request;
    use super::*;
    use crate::persist::snapshot::AgentSnap;
    use serde_json::json;

    fn start(core: &mut Core, pane: u64, id: &str) {
        let p = json!({"session_id": id, "transcript_path": format!("/t/{id}.jsonl"), "cwd": "/r", "hook_event_name": "SessionStart"});
        let h = parse_hook_request(Some(pane), "claude", "SessionStart", &p).unwrap();
        core.hook(&h, |_| true, Instant::now());
    }

    fn pending(pane: u64) -> PendingResume {
        let a = AgentSnap { kind: "claude".into(), session_id: "p".into(), name: "n".into(), last_status: "空闲".into() };
        PendingResume::from_snap(pane, None, &a).unwrap()
    }

    #[test]
    fn live_sessions_and_pending_resumes_occupy_their_panes_and_nothing_else_does() {
        let mut core = Core::new(Store::in_memory());
        start(&mut core, 1, "live");
        start(&mut core, 2, "ended");
        core.registry.pane_exited(2, Instant::now());
        let occupied = occupied_panes(core.registry.sessions(), &[pending(3)]);
        assert!(occupied.contains(&1), "live session");
        assert!(!occupied.contains(&2), "ended session frees its pane");
        assert!(occupied.contains(&3), "pending resume");
        assert!(!occupied.contains(&4), "neither");
        assert_eq!(occupied.len(), 2);
    }

    #[test]
    fn a_live_session_without_a_pane_occupies_nothing() {
        let mut core = Core::new(Store::in_memory());
        start(&mut core, 1, "a");
        let mut s = core.registry.sessions().next().unwrap().clone();
        s.pane = None;
        assert!(occupied_panes([&s], &[]).is_empty());
    }
}

#[cfg(test)]
mod git_cache_tests {
    use super::*;
    use gilvt_agent::QueryOutcome;

    fn info(branch: &str) -> GitInfo {
        GitInfo {
            repo_root: "/r".into(),
            common_dir: "/r/.git".into(),
            branch: Some(branch.into()),
            detached_short: None,
            dirty_count: 0,
            ahead: 0,
            behind: 0,
            is_linked_worktree: false,
        }
    }

    #[test]
    fn a_failed_lookup_keeps_the_cached_value_and_stores_nothing_when_there_is_none() {
        let good = Some(info("main"));
        assert_eq!(git_cache_update(Some(&good), QueryOutcome::Failed), None, "kept");
        assert_eq!(git_cache_update(None, QueryOutcome::Failed), None, "nothing stored: the next poll retries");
    }

    #[test]
    fn answers_replace_the_cache_only_when_they_differ() {
        let good = Some(info("main"));
        assert_eq!(git_cache_update(Some(&good), QueryOutcome::Info(info("main"))), None, "unchanged");
        assert_eq!(git_cache_update(Some(&good), QueryOutcome::Info(info("dev"))), Some(Some(info("dev"))));
        assert_eq!(git_cache_update(Some(&good), QueryOutcome::NotRepo), Some(None), "no longer a repository");
        assert_eq!(git_cache_update(None, QueryOutcome::NotRepo), Some(None), "first answer: not a repository");
        assert_eq!(git_cache_update(Some(&None), QueryOutcome::NotRepo), None);
    }
}
