//! ✦ AI summaries (S2 §4.5): which sessions and terminals need one (checked every second by `tick`), a queue
//! run by at most [`PARALLEL`] background threads calling the user's CLI, and the results as immutable views
//! the monitor cards and the sidebar read.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use gilvt_agent::{PaneId, Session, SessionKey, Turn, TurnOutcome};
use gilvt_monitor::cache::{self, Cached};
use gilvt_monitor::input::{self, AgentDigest, CommandDigest, Covers, Previous, TerminalDigest};
use gilvt_monitor::output::{self, Summary};
use gilvt_monitor::policy::{self, PolicyConfig, Priority, Queue, Seen, Track, MAX_FAILURES};
use gilvt_monitor::privacy::Exclusions;
use gilvt_monitor::provider::{self, process, OneShot, ProviderConfig, ProviderError, ProviderKind};
use gilvt_term::CommandBlock;
use gpui::{App, Global};

use crate::agents::{Agents, TimelineView};
use crate::settings::{MonitorProvider, MonitorSettings};
use crate::sidebar::model::{status_group, status_line};
use crate::theme::AppSettings;
use crate::workspace;

pub const PARALLEL: usize = 2;
pub const TIMEOUT: Duration = Duration::from_secs(90);
/// How long the login shell may take to report its PATH.
const LOGIN_PATH_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a summary CLI gets after SIGTERM when gilvt quits.
const QUIT_GRACE: Duration = Duration::from_secs(1);

/// The user's login-shell PATH (`$SHELL`, else the password database's shell), read once: the first caller
/// runs the shell, later ones wait for it. None (gilvt's own PATH) when there is no shell or it fails.
pub(crate) fn login_path() -> Option<String> {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    PATH.get_or_init(|| {
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).or_else(|| gilvt_shell::user_and_shell().map(|(_, s)| s))?;
        process::login_shell_path(&shell, LOGIN_PATH_TIMEOUT)
    })
    .clone()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SumState {
    Ready,
    Pending,
    Failed(String),
    /// Automatic summaries stopped after [`MAX_FAILURES`] failures in a row.
    Paused(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SummaryView {
    /// The latest successful summary (kept through later failures and while a new one runs).
    pub summary: Option<Summary>,
    pub generated_at: Option<SystemTime>,
    pub covers: Option<Covers>,
    /// The activity `summary` covered.
    pub activity: u64,
    pub state: SumState,
}

/// What `tick` saw of a subject last time, to turn changes into triggers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Observed {
    /// `status_group` (0 需要你, 1 出错, …); terminals use 9.
    pub group: usize,
    /// The last turn is over (terminals: always true).
    pub done: bool,
    pub activity: u64,
    pub terminal: bool,
}

pub(crate) struct Done {
    pub key: String,
    pub activity: u64,
    pub covers: Covers,
    pub previous_goal: Option<String>,
    pub manual: bool,
    pub result: Result<String, ProviderError>,
}

struct Job {
    activity: u64,
    covers: Covers,
    request: OneShot,
    previous_goal: Option<String>,
}

pub struct Summaries {
    views: HashMap<String, Arc<SummaryView>>,
    tracks: HashMap<String, Track>,
    observed: HashMap<String, Observed>,
    queue: Queue<Job>,
    running: HashSet<String>,
    manual: HashSet<String>,
    loaded: HashSet<String>,
    cache_dir: Option<PathBuf>,
    run_dir: Option<PathBuf>,
    done_tx: async_channel::Sender<Done>,
    /// `[monitor] exclude_paths`, expanded once and recomputed when the setting changes; asked from render
    /// paths too (`&App`), hence the cell.
    exclusions: RefCell<Exclusions>,
}

impl Global for Summaries {}

pub fn agent_key(k: &SessionKey) -> String {
    format!("agent:{}:{}", k.0.name(), k.1)
}

pub fn pane_key(p: PaneId) -> String {
    format!("pane:{p}")
}

/// Grows as a session progresses and is the same after a restart (timelines are re-read from the
/// transcripts): last turn's number, then its tool calls, then whether it ended.
pub(crate) fn activity_of_turns(turns: &[Arc<Turn>]) -> u64 {
    turns.last().map_or(0, |t| (u64::from(t.index) << 32) | ((t.steps as u64 & 0x7fff_ffff) << 1) | u64::from(t.outcome != TurnOutcome::Running))
}

pub fn agent_activity(v: &TimelineView) -> u64 {
    activity_of_turns(&v.turns)
}

pub(crate) fn trigger_of(before: Option<Observed>, now: Observed) -> Option<Priority> {
    let Some(before) = before else {
        // Terminals are listed from their first finished command on (blocks are in memory only); sessions are
        // seen for the first time at startup too, which is not news.
        return now.terminal.then_some(Priority::Ended);
    };
    if now.group <= 1 && before.group != now.group {
        return Some(Priority::Attention);
    }
    let ended = if now.terminal { now.activity > before.activity } else { now.done && !before.done && now.activity != before.activity };
    ended.then_some(Priority::Ended)
}

/// A request for this session can act (it is a summaries subject): live, with a timeline. Ended sessions are
/// not: their transcript is final, their saved summary only shows. Terminals: [`terminal_commands`] is Some.
pub fn agent_requestable(s: &Session, has_timeline: bool) -> bool {
    s.is_live() && has_timeline
}

/// While a summary of the subject runs nothing new is decided; its trigger is kept as `deferred` so the
/// first tick after the run finishes still acts on it. Returns whether to decide now.
pub(crate) fn hold_while_running(track: &mut Track, running: bool, trigger: Option<Priority>) -> bool {
    if running {
        track.deferred = track.deferred.max(trigger);
    }
    !running
}

/// The exclusions for the current settings (computed again only when `exclude_paths` changed). `f` must not
/// call back into this (nested calls are fine: `f` only holds a shared borrow).
pub fn with_exclusions<R>(cx: &App, f: impl FnOnce(&Exclusions) -> R) -> R {
    let raw = &cx.global::<AppSettings>().0.monitor.exclude_paths;
    match cx.try_global::<Summaries>() {
        Some(s) => with_refreshed(&s.exclusions, raw, f),
        None => f(&Exclusions::new(raw, home().as_deref())),
    }
}

/// Refresh `cell` in its own short mutable borrow, then run `f` under a shared one. A nested call refreshes with
/// `try_borrow_mut` (a no-op while an outer call's shared borrow is alive, and the settings are the same then).
fn with_refreshed<R>(cell: &RefCell<Exclusions>, raw: &[String], f: impl FnOnce(&Exclusions) -> R) -> R {
    if let Ok(mut ex) = cell.try_borrow_mut() {
        ex.refresh(raw);
    }
    let ex = cell.borrow();
    f(&ex)
}

/// `cwd` is under `[monitor] exclude_paths` (answers are cached per directory).
pub fn is_excluded(cwd: Option<&Path>, cx: &App) -> bool {
    with_exclusions(cx, |ex| ex.excluded(cwd))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The blocks of [`terminal_commands`] that may be sent (same rule; `blocks` newest first, order kept).
pub(crate) fn shown_blocks<'a>(blocks: &'a [CommandBlock], current_cwd: Option<&Path>, ex: &Exclusions) -> Vec<&'a CommandBlock> {
    blocks
        .iter()
        .enumerate()
        .filter(|(i, b)| {
            let end = b.end_cwd.as_deref().or_else(|| if *i == 0 { current_cwd } else { blocks[i - 1].cwd.as_deref() });
            !ex.excluded(b.cwd.as_deref()) && !ex.excluded(end) && !b.command.as_deref().is_some_and(|c| ex.mentioned(c))
        })
        .map(|(_, b)| b)
        .collect()
}

/// The commands of a terminal that may be sent (`blocks` newest first, `current_cwd` the terminal's directory
/// now), with the id of the newest finished one as the terminal's activity. Left out: a command that started
/// in an excluded directory, ended in one (`cd ~/secret && …`: its `end_cwd`, else the next block's start
/// directory, else for the newest the current one), or names one in its command line. None: nothing finished
/// to show.
pub(crate) fn terminal_commands(blocks: &[CommandBlock], current_cwd: Option<&Path>, ex: &Exclusions) -> Option<(u64, Vec<CommandDigest>)> {
    let shown = shown_blocks(blocks, current_cwd, ex);
    let activity = shown.iter().find(|b| !b.running())?.id;
    let commands = shown
        .into_iter()
        .map(|b| CommandDigest {
            command: b.command.clone().unwrap_or_else(|| "（命令未知）".into()),
            cwd: b.cwd.clone(),
            exit: b.exit,
            running: b.running(),
            took: b.ended.and_then(|e| e.duration_since(b.started).ok()),
            output_tail: b.output_tail.clone(),
        })
        .collect();
    Some((activity, commands))
}

pub(crate) fn apply_done(view: Option<&SummaryView>, track: &mut Track, done: Done, now: SystemTime) -> SummaryView {
    match done.result {
        Ok(text) => {
            track.failures = 0;
            track.covered = Some(done.activity);
            SummaryView {
                summary: Some(output::parse(&text, done.previous_goal.as_deref())),
                generated_at: Some(now),
                covers: Some(done.covers),
                activity: done.activity,
                state: SumState::Ready,
            }
        }
        Err(e) => {
            track.failures += 1;
            let message = e.message();
            let state = if !done.manual && track.failures >= MAX_FAILURES { SumState::Paused(message) } else { SumState::Failed(message) };
            match view {
                Some(v) => SummaryView { state, ..v.clone() },
                None => SummaryView { summary: None, generated_at: None, covers: None, activity: 0, state },
            }
        }
    }
}

pub fn init(state_dir: Option<PathBuf>, cx: &mut App) {
    let (done_tx, done_rx) = async_channel::unbounded::<Done>();
    cx.set_global(Summaries {
        views: HashMap::new(),
        tracks: HashMap::new(),
        observed: HashMap::new(),
        queue: Queue::default(),
        running: HashSet::new(),
        manual: HashSet::new(),
        loaded: HashSet::new(),
        cache_dir: state_dir.as_ref().map(|d| d.join("monitor/summaries")),
        run_dir: state_dir.as_ref().map(|d| d.join("monitor/run")),
        done_tx,
        exclusions: RefCell::new(Exclusions::new(&cx.global::<AppSettings>().0.monitor.exclude_paths, home().as_deref())),
    });
    if cx.global::<AppSettings>().0.monitor.enabled {
        warm_login_path();
    }
    // In-flight summary CLIs do not outlive gilvt (spec §7).
    cx.on_app_quit(|_| {
        process::kill_all(QUIT_GRACE);
        async {}
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Ok(done) = done_rx.recv().await {
            if cx.update(|cx| finish(done, cx)).is_err() {
                break;
            }
        }
    })
    .detach();
}

pub fn view(key: &str, cx: &App) -> Option<Arc<SummaryView>> {
    cx.try_global::<Summaries>()?.views.get(key).cloned()
}

/// 「✦ 重新总结」 / 「✦ 生成总结」 / 「重试」 / `s`: summarize `key` now (at the next tick, i.e. right away).
pub fn request(key: String, cx: &mut App) {
    if cx.has_global::<Summaries>() {
        cx.global_mut::<Summaries>().manual.insert(key);
    }
    cx.defer(tick);
}

/// Sessions whose files went to the Trash: their summaries go too.
pub fn forget(keys: &[SessionKey], cx: &mut App) {
    if !cx.has_global::<Summaries>() {
        return;
    }
    let s = cx.global_mut::<Summaries>();
    for k in keys {
        let key = agent_key(k);
        s.views.remove(&key);
        s.tracks.remove(&key);
        s.observed.remove(&key);
        s.queue.remove(&key);
        if let Some(dir) = &s.cache_dir {
            cache::remove(dir, &key);
        }
    }
}

enum Subject {
    Agent { key: String, session: Session, name: String, timeline: Arc<TimelineView>, observed: Observed },
    Terminal { key: String, name: String, cwd: Option<PathBuf>, commands: Vec<CommandDigest>, observed: Observed },
}

impl Subject {
    fn key(&self) -> &str {
        match self {
            Subject::Agent { key, .. } | Subject::Terminal { key, .. } => key,
        }
    }
    fn observed(&self) -> Observed {
        match self {
            Subject::Agent { observed, .. } | Subject::Terminal { observed, .. } => *observed,
        }
    }
    fn running(&self) -> bool {
        matches!(self, Subject::Agent { session, .. } if session.status.is_running())
    }
}

/// Live, non-excluded sessions with a timeline, and terminals (not hosting a live agent) with a finished command
/// outside the excluded directories.
fn subjects(excluded: &Exclusions, cx: &App) -> Vec<Subject> {
    let agents = cx.global::<Agents>();
    let mut out = Vec::new();
    for s in agents.registry().sessions() {
        if excluded.excluded(s.cwd.as_deref()) {
            continue;
        }
        let timeline = agents.timeline(&s.key);
        if !agent_requestable(s, timeline.is_some()) {
            continue;
        }
        let Some(timeline) = timeline else { continue };
        let done = timeline.turns.last().is_some_and(|t| t.outcome != TurnOutcome::Running);
        let observed = Observed { group: status_group(s), done, activity: agent_activity(&timeline), terminal: false };
        let name = if s.name.is_empty() { "新会话".to_string() } else { s.name.clone() };
        out.push(Subject::Agent { key: agent_key(&s.key), session: s.clone(), name, timeline, observed });
    }
    let agent_panes: HashSet<PaneId> = agents.registry().sessions().filter(|s| s.is_live()).filter_map(|s| s.pane).collect();
    for w in workspace::workspaces(cx) {
        let Ok(ws) = w.read(cx) else { continue };
        for t in ws.terminal_panes(cx) {
            if agent_panes.contains(&t.pane) || excluded.excluded(t.cwd.as_deref()) {
                continue;
            }
            let Some(view) = ws.terminal_view(t.pane) else { continue };
            let recent = view.read(cx).commands().recent(input::TERMINAL_COMMANDS);
            let Some((activity, commands)) = terminal_commands(&recent, t.cwd.as_deref(), excluded) else { continue };
            let observed = Observed { group: 9, done: true, activity, terminal: true };
            out.push(Subject::Terminal { key: pane_key(t.pane), name: t.title.clone(), cwd: t.cwd.clone(), commands, observed });
        }
    }
    out
}

fn build_job(sub: &Subject, view: Option<&SummaryView>) -> Job {
    match sub {
        Subject::Agent { session, name, timeline, observed, .. } => {
            let status = status_line(session).1;
            let done = timeline.plan.iter().filter(|p| p.state == gilvt_agent::PlanState::Done).count();
            let digest = AgentDigest {
                name,
                agent: match session.agent() {
                    gilvt_agent::AgentKind::Claude => "Claude Code",
                    gilvt_agent::AgentKind::Codex => "Codex",
                },
                cwd: session.cwd.as_deref(),
                status: &status,
                todo: (!timeline.plan.is_empty()).then_some((done, timeline.plan.len())),
                turns: &timeline.turns,
            };
            let previous = view.and_then(|v| Some((v.summary.as_ref()?, v.covers?)));
            let prev = previous.map(|(s, c)| Previous {
                goal: s.goal.as_deref(),
                recent: &s.recent,
                through_turn: match c {
                    Covers::Turns(_, b) => b,
                    Covers::Commands(_) => 0,
                },
            });
            let (request, covers) = input::agent_request(&digest, prev.as_ref());
            Job { activity: observed.activity, covers, request, previous_goal: previous.and_then(|(s, _)| s.goal.clone()) }
        }
        Subject::Terminal { name, cwd, commands, observed, .. } => {
            let (request, covers) = input::terminal_request(&TerminalDigest { name, cwd: cwd.as_deref(), commands });
            Job { activity: observed.activity, covers, request, previous_goal: None }
        }
    }
}

fn provider_config(cfg: &MonitorSettings, run_dir: PathBuf) -> ProviderConfig {
    ProviderConfig {
        kind: match cfg.provider {
            MonitorProvider::Claude => ProviderKind::Claude,
            MonitorProvider::Codex => ProviderKind::Codex,
        },
        program: cfg.command().map(str::to_string),
        model: cfg.summary_model().map(str::to_string),
        run_dir,
        path: None,
    }
}

/// Called every second (`agents::poll_loop`) and after a request.
pub fn tick(cx: &mut App) {
    if !cx.has_global::<Summaries>() || !cx.has_global::<Agents>() {
        return;
    }
    let cfg = cx.global::<AppSettings>().0.monitor.clone();
    if !cfg.enabled {
        return;
    }
    let (subjects, sessions) = with_exclusions(cx, |ex| {
        // Saved summaries of every non-excluded session, ended ones too (their cards show them after a restart).
        let sessions: Vec<String> = cx.global::<Agents>().registry().sessions().filter(|s| !ex.excluded(s.cwd.as_deref())).map(|s| agent_key(&s.key)).collect();
        (subjects(ex, cx), sessions)
    });
    let policy_cfg = PolicyConfig { auto: cfg.auto_summary, interval: cfg.interval() };
    let now = Instant::now();
    let s = cx.global_mut::<Summaries>();
    for key in &sessions {
        s.load_cached(key, true);
    }
    for sub in &subjects {
        let key = sub.key().to_string();
        s.load_cached(&key, matches!(sub, Subject::Agent { .. }));
        let mut trigger = trigger_of(s.observed.insert(key.clone(), sub.observed()), sub.observed());
        let track = s.tracks.entry(key.clone()).or_default();
        if !hold_while_running(track, s.running.contains(&key), trigger) {
            continue;
        }
        if s.manual.remove(&key) {
            trigger = Some(Priority::Manual);
        }
        let seen = Seen { activity: sub.observed().activity, running: sub.running(), trigger };
        if let Some(p) = policy::apply(&policy_cfg, track, &seen, now) {
            let job = build_job(sub, s.views.get(&key).map(|v| &**v));
            s.queue.push(key, p, job);
        }
    }
    // A manual request or a queued job for something no longer listed (closed, ended, excluded) is dropped:
    // nothing is sent about it any more.
    let listed: HashSet<&str> = subjects.iter().map(Subject::key).collect();
    s.manual.retain(|k| listed.contains(k.as_str()) && s.running.contains(k));
    s.queue.retain(|k| listed.contains(k));
    dispatch(&cfg, cx);
}

impl Summaries {
    fn load_cached(&mut self, key: &str, agent: bool) {
        if !agent || !self.loaded.insert(key.to_string()) || self.views.contains_key(key) {
            return;
        }
        let Some(c) = self.cache_dir.as_deref().and_then(|d| cache::load(d, key)) else { return };
        self.tracks.entry(key.to_string()).or_default().covered = Some(c.activity);
        self.views.insert(
            key.to_string(),
            Arc::new(SummaryView { summary: Some(c.summary), generated_at: Some(c.generated_at), covers: Some(c.covers), activity: c.activity, state: SumState::Ready }),
        );
    }
}

fn dispatch(cfg: &MonitorSettings, cx: &mut App) {
    let s = cx.global_mut::<Summaries>();
    let Some(run_dir) = s.run_dir.clone() else { return };
    let mut started = false;
    while s.running.len() < PARALLEL {
        let Some((key, priority, job)) = s.queue.pop() else { break };
        s.running.insert(key.clone());
        s.tracks.entry(key.clone()).or_default().last_run = Some(Instant::now());
        let old = s.views.get(&key).map(|v| (**v).clone());
        let pending = match old {
            Some(v) => SummaryView { state: SumState::Pending, ..v },
            None => SummaryView { summary: None, generated_at: None, covers: None, activity: 0, state: SumState::Pending },
        };
        s.views.insert(key.clone(), Arc::new(pending));
        let tx = s.done_tx.clone();
        let provider = provider_config(cfg, run_dir.clone());
        std::thread::Builder::new()
            .name("gilvt-summary".into())
            .spawn(move || {
                let provider = ProviderConfig { path: login_path(), ..provider };
                let result = provider::summarize(&provider, &job.request, TIMEOUT);
                let _ = tx.send_blocking(Done { key, activity: job.activity, covers: job.covers, previous_goal: job.previous_goal, manual: priority == Priority::Manual, result });
            })
            .expect("spawn a summary thread");
        started = true;
    }
    if started {
        cx.defer(workspace::notify_all);
    }
}

fn finish(done: Done, cx: &mut App) {
    // The session's directory may have been excluded while the call ran.
    let excluded = done.key.starts_with("agent:") && with_exclusions(cx, |ex| excluded_agent_keys(ex, cx).contains(&done.key));
    cx.global_mut::<Summaries>().store(done, excluded);
    cx.defer(workspace::notify_all);
    tick(cx);
}

impl Summaries {
    /// A call finished: its result becomes the subject's view (and, for a session, its cache), unless the subject
    /// is `excluded` now, when nothing of it is kept.
    fn store(&mut self, done: Done, excluded: bool) {
        self.running.remove(&done.key);
        if excluded {
            return;
        }
        let key = done.key.clone();
        let agent = key.starts_with("agent:");
        let track = self.tracks.entry(key.clone()).or_default();
        let view = apply_done(self.views.get(&key).map(|v| &**v), track, done, SystemTime::now());
        if agent && view.state == SumState::Ready {
            if let (Some(dir), Some(summary), Some(at), Some(covers)) = (&self.cache_dir, &view.summary, view.generated_at, view.covers) {
                let _ = cache::save(dir, &Cached { key: key.clone(), summary: summary.clone(), generated_at: at, covers, activity: view.activity });
            }
        }
        self.views.insert(key, Arc::new(view));
    }

    /// `[monitor] enabled` turned off: drop what waits (running calls finish and are cached as usual), and what
    /// was seen, so turning it on again does not take every old session for news.
    fn disable(&mut self) {
        self.queue = Queue::default();
        self.manual.clear();
        self.observed.clear();
    }

    /// Subjects that became excluded: forget everything in memory about them (their cache files stay and are
    /// loaded again once they are no longer excluded). A call already running finishes and [`Summaries::store`]
    /// throws its result away.
    fn drop_keys(&mut self, keys: &[String]) {
        for key in keys {
            self.views.remove(key);
            self.tracks.remove(key);
            self.observed.remove(key);
            self.manual.remove(key);
            self.loaded.remove(key);
            self.queue.remove(key);
        }
    }
}

/// The keys of the sessions (`(key, cwd)`) whose directory `ex` excludes.
fn excluded_keys<'a>(sessions: impl Iterator<Item = (&'a SessionKey, Option<&'a Path>)>, ex: &Exclusions) -> Vec<String> {
    sessions.filter(|(_, cwd)| ex.excluded(*cwd)).map(|(k, _)| agent_key(k)).collect()
}

/// The sessions gilvt knows of (the registry's, and those waiting to be resumed) that `ex` excludes.
fn excluded_agent_keys(ex: &Exclusions, cx: &App) -> Vec<String> {
    let Some(agents) = cx.try_global::<Agents>() else { return Vec::new() };
    let sessions = agents.registry().sessions().map(|s| (&s.key, s.cwd.as_deref()));
    let pending = agents.pending().iter().map(|p| (&p.key, p.cwd.as_deref()));
    excluded_keys(sessions.chain(pending), ex)
}

/// `[monitor]` changed at runtime (S2 §5.1). Provider / model / command changes need nothing here: every call
/// reads the settings when it starts.
pub fn settings_changed(old: &MonitorSettings, new: &MonitorSettings, cx: &mut App) {
    if !cx.has_global::<Summaries>() {
        return;
    }
    if old.enabled && !new.enabled {
        cx.global_mut::<Summaries>().disable();
    }
    if !old.enabled && new.enabled {
        warm_login_path();
    }
    if old.exclude_paths != new.exclude_paths {
        // `with_exclusions` takes the new list here (the settings are already in place).
        let keys = with_exclusions(cx, |ex| excluded_agent_keys(ex, cx));
        cx.global_mut::<Summaries>().drop_keys(&keys);
    }
}

/// Reads the login PATH in the background, so it is ready before the first summary needs it.
fn warm_login_path() {
    std::thread::Builder::new().name("gilvt-login-path".into()).spawn(|| drop(login_path())).ok();
}

#[cfg(test)]
#[path = "summaries_tests.rs"]
mod tests;
