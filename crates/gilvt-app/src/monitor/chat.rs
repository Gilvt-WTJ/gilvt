//! The 监控官's conversation (S2 §6.2): one CLI process for the whole app, started by the first message and ended
//! after 30 idle minutes, by 「新对话」, when the 监控官 is turned off, and when gilvt quits. The history lives only
//! here. Windows read `Arc<ChatView>`; changes call `workspace::notify_all` (text deltas at most every 50 ms).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gilvt_monitor::chat::{self, ChatConfig, ChatEvent, ChatHandle, McpLaunch, CHAT_INSTRUCTIONS};
use gilvt_monitor::provider::{ProviderError, ProviderKind};
use gpui::{App, Global};

use super::chat_model::{self, Conversation, ErrorCard, Outgoing, Role, Status, Timeout, Timers};
use super::tools::{new_token, set_chat_token};
use crate::launch::ShellEnv;
use crate::settings::{MonitorProvider, MonitorSettings};
use crate::theme::AppSettings;

pub const ENV_RECORD: &str = "GILVT_MONITOR_RECORD";
const FLUSH_EVERY: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcessInfo {
    pub running: bool,
    /// "claude" | "codex" while running.
    pub provider: Option<&'static str>,
    pub pid: Option<u32>,
    /// Turns the running process finished.
    pub turns: u64,
    /// Processes started in this gilvt run.
    pub starts: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatView {
    pub revision: u64,
    pub conv: Conversation,
    /// The configured channel: "claude" | "codex".
    pub provider: &'static str,
    /// "Claude" | "Codex".
    pub provider_label: &'static str,
    /// The configured chat model; "" = the CLI's default.
    pub model: String,
    /// An answer finished since the user last looked at the panel.
    pub unread: bool,
    pub process: ProcessInfo,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Launch {
    provider: MonitorProvider,
    model: Option<String>,
    command: Option<String>,
}

impl Launch {
    fn of(m: &MonitorSettings) -> Launch {
        Launch { provider: m.provider, model: m.chat_model().map(String::from), command: m.command().map(String::from) }
    }
}

fn provider_ids(p: MonitorProvider) -> (&'static str, &'static str) {
    match p {
        MonitorProvider::Claude => ("claude", "Claude"),
        MonitorProvider::Codex => ("codex", "Codex"),
    }
}

enum Proc {
    None,
    Starting,
    Running { handle: ChatHandle, launch: Launch, answering: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProcState {
    None,
    Starting,
    Idle,
    Answering,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SendPlan {
    Write,
    InterruptThenQueue,
    Queue,
    Start,
    Restart,
}

/// What a new message does (S2 §6.2): start the process, write it, or interrupt the current answer first.
pub(crate) fn send_plan(state: ProcState, same_launch: bool, queue_empty: bool) -> SendPlan {
    match state {
        ProcState::None => SendPlan::Start,
        ProcState::Starting => SendPlan::Queue,
        _ if !same_launch => SendPlan::Restart,
        ProcState::Idle => SendPlan::Write,
        ProcState::Answering if queue_empty => SendPlan::InterruptThenQueue,
        ProcState::Answering => SendPlan::Queue,
    }
}

pub struct Chat {
    conv: Conversation,
    view: Arc<ChatView>,
    revision: u64,
    proc: Proc,
    queue: Vec<Outgoing>,
    /// The first queued message's bubble, counted among the user bubbles only (`abandon` removes empty answers,
    /// never a user bubble, so this stays right where an index into `conv.messages` would shift).
    queue_start: Option<usize>,
    /// Sent before the next message: the history before it, for a process that does not know it.
    prelude: Option<String>,
    /// +1 whenever a process is let go or exits: its later events are ignored.
    gen: u64,
    timers: Timers,
    starts: u64,
    process_turns: u64,
    unread: bool,
    record: Option<PathBuf>,
    flush_pending: bool,
    last_flush: Instant,
}

impl Global for Chat {}

pub fn log_path() -> Option<PathBuf> {
    crate::agents::state_dir().map(|d| d.join("monitor").join("chat.log"))
}

pub fn init(cx: &mut App) {
    let record = std::env::var_os(ENV_RECORD).filter(|v| !v.is_empty()).map(PathBuf::from);
    // Panes never see it.
    std::env::remove_var(ENV_RECORD);
    let now = Instant::now();
    let empty = ChatView { revision: 0, conv: Conversation::default(), provider: "claude", provider_label: "Claude", model: String::new(), unread: false, process: ProcessInfo::default() };
    cx.set_global(Chat {
        conv: Conversation::default(),
        view: Arc::new(empty),
        revision: 0,
        proc: Proc::None,
        queue: Vec::new(),
        queue_start: None,
        prelude: None,
        gen: 0,
        timers: Timers::new(now),
        starts: 0,
        process_turns: 0,
        unread: false,
        record,
        flush_pending: false,
        last_flush: now,
    });
    refresh_view(cx);
    cx.on_app_quit(|cx| {
        shutdown(cx);
        async {}
    })
    .detach();
}

pub fn view(cx: &App) -> Arc<ChatView> {
    cx.global::<Chat>().view.clone()
}

/// DebugState's `chat_process`.
pub fn process_state(cx: &App) -> crate::debug_state::ChatProcessState {
    let Some(c) = cx.try_global::<Chat>() else { return Default::default() };
    let v = &c.view;
    crate::debug_state::ChatProcessState { running: v.process.running, status: v.conv.status.id(), provider: v.process.provider, pid: v.process.pid, turns: v.process.turns, starts: v.process.starts }
}

fn state(c: &Chat) -> ProcState {
    match &c.proc {
        Proc::None => ProcState::None,
        Proc::Starting => ProcState::Starting,
        Proc::Running { answering: true, .. } => ProcState::Answering,
        Proc::Running { .. } => ProcState::Idle,
    }
}

/// Queues `out`, whose bubble was just pushed.
fn enqueue(c: &mut Chat, out: Outgoing) {
    if c.queue.is_empty() {
        c.queue_start = Some(c.conv.messages.iter().filter(|m| m.role == Role::User).count().saturating_sub(1));
    }
    c.queue.push(out);
}

/// The history a new process is told before the queued messages (never the queued messages themselves: they are
/// written after it).
fn set_prelude(c: &mut Chat) {
    c.prelude = c.queue_start.and_then(|n| {
        let end = c.conv.messages.iter().enumerate().filter(|(_, m)| m.role == Role::User).nth(n).map_or(c.conv.messages.len(), |(i, _)| i);
        chat_model::prelude(&c.conv.messages[..end])
    });
}

/// Writes `out` to the running process.
fn write(c: &mut Chat, out: Outgoing) {
    let prelude = c.prelude.take();
    if let Proc::Running { handle, answering, .. } = &mut c.proc {
        handle.send(&chat_model::wire_text(&out, prelude.as_deref()));
        *answering = true;
        c.conv.status = Status::Answering;
        c.timers.output(Instant::now());
    }
}

/// Drops the process (its handle stops it) and ignores whatever it still says, its exit included.
fn let_go(c: &mut Chat) {
    c.gen += 1;
    c.proc = Proc::None;
    c.timers.interrupted_at = None;
    c.conv.abandon();
    if matches!(c.conv.status, Status::Starting | Status::Answering | Status::Stopping) {
        c.conv.status = Status::Idle;
    }
}

pub fn send(out: Outgoing, cx: &mut App) {
    let m = cx.global::<AppSettings>().0.monitor.clone();
    if !m.enabled || out.text.trim().is_empty() {
        return;
    }
    let launch = Launch::of(&m);
    let now = Instant::now();
    let plan = {
        let c = cx.global_mut::<Chat>();
        c.unread = false;
        c.timers.used(now);
        let same = matches!(&c.proc, Proc::Running { launch: l, .. } if *l == launch);
        let plan = send_plan(state(c), same, c.queue.is_empty());
        if plan != SendPlan::Restart {
            c.conv.push_user(&out);
        }
        match plan {
            SendPlan::Write => write(c, out),
            SendPlan::InterruptThenQueue => {
                if let Proc::Running { handle, .. } = &c.proc {
                    handle.interrupt();
                }
                c.timers.interrupted_at = Some(now);
                c.conv.status = Status::Stopping;
                enqueue(c, out);
            }
            SendPlan::Queue | SendPlan::Start => enqueue(c, out),
            SendPlan::Restart => switch(c, out, &launch),
        }
        plan
    };
    if matches!(plan, SendPlan::Start | SendPlan::Restart) {
        start_process(launch, cx);
    }
    flush(cx);
}

/// New settings: the old process goes, 「已切换到 …」 is said, then `out` is queued for the new one.
fn switch(c: &mut Chat, out: Outgoing, launch: &Launch) {
    let_go(c);
    let (_, label) = provider_ids(launch.provider);
    let model = launch.model.clone().unwrap_or_else(|| crate::i18n::text("CLI 默认", "CLI default").into());
    c.conv.notice(&if crate::i18n::english() {
        format!("Switched to {label} · {model}")
    } else {
        format!("已切换到 {label} · {model}")
    });
    c.conv.push_user(&out);
    enqueue(c, out);
}

/// What `chat::start` needs; `path` stays None (the login PATH is filled in on the starting thread).
fn config(m: &MonitorSettings, record: Option<PathBuf>, cx: &App) -> Result<ChatConfig, ErrorCard> {
    let (_, label) = provider_ids(m.provider);
    let card = |text: &str| ErrorCard { title: crate::monitor::chat_model::cannot_start_title(label), text: text.to_string() };
    let env = cx.try_global::<ShellEnv>();
    let gilvt = env
        .and_then(|e| e.bin_dir.as_ref())
        .map(|d| d.join("gilvt"))
        .filter(|p| p.is_file())
        .ok_or_else(|| {
            card(crate::i18n::text(
                "找不到 gilvt 命令行（应在 Gilvt.app 里），监控官没有可用的只读工具。请用完整的 Gilvt.app 运行。",
                "Could not find the gilvt CLI (it should be inside Gilvt.app), so the Monitor has no read-only tools. Run the complete Gilvt.app.",
            ))
        })?;
    let socket = env.and_then(|e| e.socket.clone()).ok_or_else(|| {
        card(crate::i18n::text(
            "gilvt 的本地通信没有启动（见启动日志），监控官的工具无法连接 gilvt。",
            "gilvt's local IPC is not running (see the startup log), so the Monitor's tools cannot reach gilvt.",
        ))
    })?;
    Ok(ChatConfig {
        kind: match m.provider {
            MonitorProvider::Claude => ProviderKind::Claude,
            MonitorProvider::Codex => ProviderKind::Codex,
        },
        program: m.command().map(String::from),
        model: m.chat_model().map(String::from),
        run_dir: crate::settings_window::probe::run_dir(),
        path: None,
        instructions: CHAT_INSTRUCTIONS.to_string(),
        mcp: McpLaunch { gilvt, socket, token: new_token() },
        log: log_path(),
        record,
    })
}

fn start_process(launch: Launch, cx: &mut App) {
    let m = cx.global::<AppSettings>().0.monitor.clone();
    let record = cx.global::<Chat>().record.clone();
    let cfg = match config(&m, record, cx) {
        Ok(cfg) => cfg,
        Err(card) => {
            let c = cx.global_mut::<Chat>();
            c.queue.clear();
            c.queue_start = None;
            c.conv.error_card(card);
            // No process now: a token an earlier one held is void too.
            set_chat_token(None, cx);
            return;
        }
    };
    set_chat_token(Some(cfg.mcp.token.clone()), cx);
    let program = cfg.program.clone().unwrap_or_else(|| cfg.kind.default_program().to_string());
    let gen = {
        let c = cx.global_mut::<Chat>();
        c.gen += 1;
        set_prelude(c);
        c.proc = Proc::Starting;
        c.conv.status = Status::Starting;
        c.gen
    };
    // The login shell (first use) and Codex's `features list` can each take up to 10 s: on a thread of their own,
    // never on the main thread or gpui's executor.
    let rx = crate::settings_window::probe::spawn(move || chat::start(&ChatConfig { path: crate::monitor::summaries::login_path(), ..cfg }));
    cx.spawn(async move |cx| {
        let result = rx.recv().await.unwrap_or_else(|_| Err(ProviderError::Protocol(crate::i18n::text("对话进程没有启动", "the chat process did not start").into())));
        let _ = cx.update(|cx| on_started(gen, launch, program, result, cx));
    })
    .detach();
}

fn on_started(gen: u64, launch: Launch, program: String, result: Result<ChatHandle, ProviderError>, cx: &mut App) {
    if cx.global::<Chat>().gen != gen {
        return; // let go meanwhile: dropping `result` stops the process
    }
    match result {
        Ok(handle) => {
            let events = handle.events();
            {
                let c = cx.global_mut::<Chat>();
                c.starts += 1;
                c.process_turns = 0;
                // Nothing an earlier process left (an interrupt it ignored) counts against this one.
                c.timers = Timers::new(Instant::now());
                c.proc = Proc::Running { handle, launch, answering: false };
                c.conv.status = Status::Idle;
                if let Some(out) = chat_model::merge(&std::mem::take(&mut c.queue)) {
                    c.queue_start = None;
                    write(c, out);
                }
            }
            cx.spawn(async move |cx| {
                while let Ok(ev) = events.recv().await {
                    if cx.update(|cx| on_event(gen, ev, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        Err(e) => {
            let (_, label) = provider_ids(launch.provider);
            {
                let c = cx.global_mut::<Chat>();
                c.proc = Proc::None;
                c.queue.clear();
                c.queue_start = None;
                c.prelude = None;
                c.conv.error_card(chat_model::error_card(label, &program, &e));
            }
            set_chat_token(None, cx);
        }
    }
    flush(cx);
}

/// One event of process `gen`; a process let go meanwhile (idle, 「新对话」, killed) is not heard from, so its
/// exit shows no 「进程已退出」.
fn on_event(gen: u64, ev: ChatEvent, cx: &mut App) {
    if cx.global::<Chat>().gen != gen {
        return;
    }
    let names = matches!(ev, ChatEvent::ToolStarted { .. }).then(|| super::tools::names(cx));
    let (exited, restart) = {
        let c = cx.global_mut::<Chat>();
        c.timers.output(Instant::now());
        let name_of = |k: &str| names.as_ref().and_then(|n| n.get(k).cloned());
        c.conv.apply(&ev, &name_of);
        match &ev {
            ChatEvent::TurnEnded(_) => {
                c.timers.interrupted_at = None;
                c.process_turns += 1;
                c.unread = true;
                if let Proc::Running { answering, .. } = &mut c.proc {
                    *answering = false;
                }
                if let Some(out) = chat_model::merge(&std::mem::take(&mut c.queue)) {
                    c.queue_start = None;
                    write(c, out);
                }
                (false, false)
            }
            ChatEvent::Exited { .. } => {
                // Gone mid-turn or mid-interrupt: nothing of that turn is pending any more (an interrupt left set
                // would get the next process killed by `tick`).
                c.proc = Proc::None;
                c.gen += 1;
                c.timers.interrupted_at = None;
                (true, !c.queue.is_empty())
            }
            _ => (false, false),
        }
    };
    if exited {
        set_chat_token(None, cx);
    }
    if restart {
        let launch = Launch::of(&cx.global::<AppSettings>().0.monitor);
        start_process(launch, cx);
    }
    if matches!(ev, ChatEvent::Text { .. }) {
        flush_soon(cx);
    } else {
        flush(cx);
    }
}

/// 「新对话」: history cleared, process ended (the next message starts a fresh one).
pub fn new_conversation(cx: &mut App) {
    {
        let c = cx.global_mut::<Chat>();
        let_go(c);
        c.queue.clear();
        c.queue_start = None;
        c.prelude = None;
        c.unread = false;
        let revision = c.conv.revision + 1;
        c.conv = Conversation { revision, ..Default::default() };
    }
    set_chat_token(None, cx);
    flush(cx);
}

/// A grey line in the chat (e.g. 「查看日志」 could not open the log). Not sent to the 监控官.
pub fn notice(text: &str, cx: &mut App) {
    cx.global_mut::<Chat>().conv.notice(text);
    flush(cx);
}

/// The panel was seen expanded.
pub fn mark_read(cx: &mut App) {
    if std::mem::take(&mut cx.global_mut::<Chat>().unread) {
        flush(cx);
    }
}

/// 「停止」: interrupts the current turn only (queued messages are dropped; their bubbles stay). An interrupt
/// already under way is not sent again: its clock keeps running, so clicking again cannot put off the kill.
pub fn stop(cx: &mut App) {
    let c = cx.global_mut::<Chat>();
    c.queue.clear();
    c.queue_start = None;
    if c.timers.interrupted_at.is_some() {
        flush(cx);
        return;
    }
    if let Proc::Running { handle, answering: true, .. } = &c.proc {
        handle.interrupt();
        c.timers.interrupted_at = Some(Instant::now());
        c.conv.status = Status::Stopping;
    }
    flush(cx);
}

/// Every second (`agents::poll_loop`): silent turns, ignored interrupts, idle processes.
pub fn tick(cx: &mut App) {
    if !cx.has_global::<Chat>() {
        return;
    }
    let now = Instant::now();
    let (timeout, restart) = {
        let c = cx.global_mut::<Chat>();
        if !matches!(c.proc, Proc::Running { .. }) {
            return;
        }
        let answering = matches!(c.proc, Proc::Running { answering: true, .. });
        let Some(t) = c.timers.check(now, answering) else { return };
        match t {
            Timeout::TurnSilent => {
                if let Proc::Running { handle, .. } = &c.proc {
                    handle.interrupt();
                }
                c.timers.interrupted_at = Some(now);
                c.conv.status = Status::Stopping;
                c.conv.notice(crate::i18n::text(
                    "这一轮 5 分钟没有任何输出，已中断",
                    "No output for 5 minutes in this turn; interrupted",
                ));
            }
            Timeout::InterruptIgnored => {
                let_go(c);
                c.conv.notice(crate::i18n::text(
                    "中断没有响应，已结束监控官进程",
                    "The interrupt got no response; the Monitor process was ended",
                ));
            }
            Timeout::Idle => let_go(c),
        }
        (t, !c.queue.is_empty())
    };
    if matches!(timeout, Timeout::InterruptIgnored | Timeout::Idle) {
        set_chat_token(None, cx);
    }
    if restart && timeout == Timeout::InterruptIgnored {
        let launch = Launch::of(&cx.global::<AppSettings>().0.monitor);
        start_process(launch, cx);
    }
    flush(cx);
}

/// `[monitor]` changed at runtime (阶段 2 `config_file::apply_settings`). Off: the process (or the start under way)
/// is let go, the history stays in memory (the panel is not drawn). Provider / model / command: the next message
/// restarts (see [`send`]).
pub fn settings_changed(old: &MonitorSettings, new: &MonitorSettings, cx: &mut App) {
    if !cx.has_global::<Chat>() {
        return;
    }
    if old.enabled && !new.enabled {
        let c = cx.global_mut::<Chat>();
        let_go(c);
        c.queue.clear();
        c.queue_start = None;
        set_chat_token(None, cx);
    }
    flush(cx);
}

/// gilvt is quitting: end the process now (SIGTERM, SIGKILL after 1 s).
pub fn shutdown(cx: &mut App) {
    if let Some(Chat { proc: Proc::Running { handle, .. }, .. }) = cx.try_global::<Chat>() {
        handle.kill_now();
    }
}

fn refresh_view(cx: &mut App) {
    let m = cx.global::<AppSettings>().0.monitor.clone();
    let (provider, provider_label) = provider_ids(m.provider);
    let c = cx.global_mut::<Chat>();
    c.revision += 1;
    c.last_flush = Instant::now();
    let process = match &c.proc {
        Proc::Running { handle, launch, .. } => ProcessInfo {
            running: true,
            provider: Some(provider_ids(launch.provider).0),
            pid: Some(handle.pid()),
            turns: c.process_turns,
            starts: c.starts,
        },
        _ => ProcessInfo { starts: c.starts, ..Default::default() },
    };
    c.view = Arc::new(ChatView {
        revision: c.revision,
        conv: c.conv.clone(),
        provider,
        provider_label,
        model: m.chat_model().unwrap_or("").to_string(),
        unread: c.unread,
        process,
    });
}

fn flush(cx: &mut App) {
    refresh_view(cx);
    crate::workspace::notify_all(cx);
}

/// Text deltas: at most one redraw per [`FLUSH_EVERY`].
fn flush_soon(cx: &mut App) {
    let c = cx.global_mut::<Chat>();
    let due = c.last_flush.elapsed() >= FLUSH_EVERY;
    if due {
        flush(cx);
        return;
    }
    if std::mem::replace(&mut c.flush_pending, true) {
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(FLUSH_EVERY).await;
        let _ = cx.update(|cx| {
            cx.global_mut::<Chat>().flush_pending = false;
            flush(cx);
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use gilvt_monitor::chat::{ChatEvent, TurnEnd};
    use gilvt_monitor::provider::ProviderError;
    use gpui::TestAppContext;

    use super::*;
    use crate::monitor::chat_model::{Role, Status, IDLE_LIMIT, INTERRUPT_GRACE, TURN_SILENCE};
    use crate::monitor::tools;
    use crate::monitor::tools::{authorized, Tokens};
    use crate::settings::Settings;

    #[test]
    fn send_plan() {
        use ProcState::*;
        assert_eq!(super::send_plan(None, true, true), SendPlan::Start, "the first message starts the process");
        assert_eq!(super::send_plan(Starting, true, true), SendPlan::Queue);
        assert_eq!(super::send_plan(Idle, true, true), SendPlan::Write);
        assert_eq!(super::send_plan(Answering, true, true), SendPlan::InterruptThenQueue, "a new message interrupts the answer");
        assert_eq!(super::send_plan(Answering, true, false), SendPlan::Queue, "only the first one interrupts");
        assert_eq!(super::send_plan(Idle, false, true), SendPlan::Restart, "new settings: a new process");
        assert_eq!(super::send_plan(Answering, false, true), SendPlan::Restart);
    }

    #[test]
    fn chat_model_setting() {
        let mut m = MonitorSettings::default();
        assert_eq!(m.chat_model(), Option::None);
        m.model = " sonnet ".into();
        assert_eq!(m.chat_model(), Some("sonnet"));
    }

    fn setup(cx: &mut TestAppContext, enabled: bool) {
        cx.update(|cx| {
            let mut settings = Settings::default();
            settings.monitor.enabled = enabled;
            cx.set_global(AppSettings(settings));
            init(cx);
        });
    }

    fn out(text: &str) -> Outgoing {
        Outgoing { text: text.into(), chips: Vec::new() }
    }

    fn exited() -> ChatEvent {
        ChatEvent::Exited { code: Some(1), stderr_tail: String::new() }
    }

    /// Stands in for `start_process` having launched generation `gen` (no process is started).
    fn starting(cx: &mut App) -> u64 {
        let c = cx.global_mut::<Chat>();
        c.gen += 1;
        c.proc = Proc::Starting;
        c.conv.status = Status::Starting;
        c.gen
    }

    #[gpui::test]
    fn send_does_nothing_while_off(cx: &mut TestAppContext) {
        setup(cx, false);
        cx.update(|cx| {
            let before = view(cx);
            send(out("哪些需要我？"), cx);
            let c = cx.global::<Chat>();
            assert!(c.conv.messages.is_empty() && c.queue.is_empty());
            assert!(matches!(c.proc, Proc::None));
            assert_eq!(view(cx).revision, before.revision, "nothing changed");
        });
    }

    #[gpui::test]
    fn send_without_the_gilvt_cli_shows_a_card(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            cx.set_global(ShellEnv { integration: None, socket: None, bin_dir: None, user: None });
            send(out("哪些需要我？"), cx);
            let v = view(cx);
            assert_eq!(v.conv.status, Status::Error);
            assert_eq!(v.conv.messages[0].role, Role::User, "the question stays");
            let card = v.conv.messages.last().unwrap().card.clone().expect("a red card");
            assert_eq!(card.title, "无法启动 Claude 对话");
            let c = cx.global::<Chat>();
            assert!(matches!(c.proc, Proc::None));
            assert!(c.queue.is_empty() && c.queue_start.is_none());
            assert_eq!(v.process.starts, 0);
            assert!(!v.process.running);
        });
    }

    #[gpui::test]
    fn send_without_a_shell_env_shows_a_card(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            send(out("哪些需要我？"), cx);
            let v = view(cx);
            assert_eq!(v.conv.status, Status::Error);
            assert_eq!(v.conv.messages.last().unwrap().card.as_ref().map(|c| c.title.as_str()), Some("无法启动 Claude 对话"));
        });
    }

    #[gpui::test]
    fn a_failed_start_shows_a_card_and_voids_the_token(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            let token = "c".repeat(32);
            set_chat_token(Some(token.clone()), cx);
            let gen = starting(cx);
            {
                let c = cx.global_mut::<Chat>();
                c.conv.push_user(&out("哪些需要我？"));
                enqueue(c, out("哪些需要我？"));
            }
            let launch = Launch::of(&cx.global::<AppSettings>().0.monitor);
            on_started(gen, launch, "claude".into(), Err(ProviderError::NotFound { program: "claude".into() }), cx);
            let v = view(cx);
            assert_eq!(v.conv.status, Status::Error);
            let card = v.conv.messages.last().unwrap().card.clone().expect("a red card");
            assert_eq!(card.title, "无法启动 Claude 对话");
            let c = cx.global::<Chat>();
            assert!(matches!(c.proc, Proc::None));
            assert!(c.queue.is_empty() && c.queue_start.is_none() && c.prelude.is_none());
            assert!(!authorized(cx.global::<Tokens>(), &token), "the token of a process that never ran is void");
        });
    }

    #[gpui::test]
    fn an_exit_ends_the_interrupt(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            let gen = starting(cx);
            on_event(gen, ChatEvent::TurnStarted, cx);
            cx.global_mut::<Chat>().timers.interrupted_at = Some(Instant::now());
            on_event(gen, exited(), cx);
            let c = cx.global::<Chat>();
            assert_eq!(c.timers.interrupted_at, Option::None, "a later process must not be killed for this interrupt");
            assert!(matches!(c.proc, Proc::None));
            assert_ne!(c.conv.status, Status::Answering, "not in a turn any more");
            assert_eq!(state(c), ProcState::None);
            let last = c.conv.messages.last().unwrap();
            assert_eq!((last.role, last.text.as_str()), (Role::Error, "监控官进程已退出（退出码 1）"), "an unexpected exit is shown");
        });
    }

    #[gpui::test]
    fn a_failed_handshake_shows_one_error_and_the_next_message_starts_afresh(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            let token = "a".repeat(32);
            set_chat_token(Some(token.clone()), cx);
            let gen = starting(cx);
            cx.global_mut::<Chat>().conv.push_user(&out("哪些需要我？"));
            on_event(gen, ChatEvent::TurnStarted, cx);
            // The pump ends the process after a `Failed`: its `Exited` follows (S2 §7).
            on_event(gen, ChatEvent::Failed(ProviderError::Protocol("initialize：boom".into())), cx);
            on_event(gen, exited(), cx);
            let c = cx.global::<Chat>();
            let errors: Vec<&str> = c.conv.messages.iter().filter(|m| m.role == Role::Error).map(|m| m.text.as_str()).collect();
            assert_eq!(errors, ["输出无法解析：initialize：boom"], "shown (not a deliberate exit), and no 「进程已退出」 under it");
            assert_eq!(c.conv.status, Status::Error);
            assert!(matches!(c.proc, Proc::None));
            assert_eq!(super::send_plan(state(c), true, c.queue.is_empty()), SendPlan::Start, "the next message starts a fresh process");
            assert!(!authorized(cx.global::<Tokens>(), &token), "the token is void");
        });
    }

    #[gpui::test]
    fn a_let_go_process_exits_quietly(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            let gen = starting(cx);
            new_conversation(cx);
            on_event(gen, ChatEvent::TurnEnded(TurnEnd::Interrupted), cx);
            on_event(gen, exited(), cx);
            let v = view(cx);
            assert!(v.conv.messages.is_empty(), "no 「进程已退出」 after 「新对话」: {:?}", v.conv.messages);
            assert_eq!(v.conv.status, Status::Idle);
        });
    }

    #[gpui::test]
    fn the_prelude_leaves_out_the_new_question(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            let gen = starting(cx);
            cx.global_mut::<Chat>().conv.push_user(&out("哪些需要我？"));
            on_event(gen, ChatEvent::TextDone { message: "m1".into(), text: "两个会话在等你".into() }, cx);
            on_event(gen, ChatEvent::TurnEnded(TurnEnd::Done), cx);
            on_event(gen, exited(), cx);
            let c = cx.global_mut::<Chat>();
            c.conv.push_user(&out("第二个问题"));
            enqueue(c, out("第二个问题"));
            set_prelude(c);
            let p = c.prelude.clone().expect("a prelude");
            assert!(p.contains("哪些需要我？") && p.contains("两个会话在等你"), "{p}");
            assert!(!p.contains("第二个问题"), "the question itself is sent once, after the prelude: {p}");
        });
    }
    /// An answer that never got text: `abandon` drops its bubble.
    fn empty_answer(c: &mut Chat) {
        c.conv.apply(&ChatEvent::Text { message: "m1".into(), delta: String::new() }, &|_| Option::None);
    }

    #[gpui::test]
    fn an_empty_answer_dropped_by_the_exit_does_not_shift_the_queue(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            let c = cx.global_mut::<Chat>();
            c.conv.push_user(&out("哪些需要我？"));
            empty_answer(c);
            // Sent while answering (InterruptThenQueue), then the process exits.
            c.conv.push_user(&out("第二个问题"));
            enqueue(c, out("第二个问题"));
            c.conv.apply(&exited(), &|_| Option::None);
            set_prelude(c);
            let p = c.prelude.clone().expect("a prelude");
            assert!(p.contains("哪些需要我？"), "{p}");
            assert!(!p.contains("第二个问题"), "the queued question is not part of the history: {p}");
        });
    }

    #[gpui::test]
    fn switching_settings_says_so_before_the_question(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            let launch = Launch::of(&cx.global::<AppSettings>().0.monitor);
            let c = cx.global_mut::<Chat>();
            c.conv.push_user(&out("哪些需要我？"));
            empty_answer(c);
            switch(c, out("第二个问题"), &launch);
            let tail: Vec<(Role, &str)> = c.conv.messages.iter().rev().take(2).rev().map(|m| (m.role, m.text.as_str())).collect();
            assert_eq!(tail, vec![(Role::Notice, "已切换到 Claude · CLI 默认"), (Role::User, "第二个问题")]);
            set_prelude(c);
            let p = c.prelude.clone().expect("a prelude");
            assert!(p.contains("哪些需要我？") && !p.contains("第二个问题"), "{p}");
        });
    }

    #[gpui::test]
    fn a_start_that_cannot_be_made_voids_the_old_token(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            let token = "d".repeat(32);
            set_chat_token(Some(token.clone()), cx);
            send(out("哪些需要我？"), cx);
            assert_eq!(view(cx).conv.status, Status::Error);
            assert!(!authorized(cx.global::<Tokens>(), &token), "no process runs, so no token is valid");
        });
    }

    #[gpui::test]
    fn turning_it_off_lets_the_process_go(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            let token = "e".repeat(32);
            set_chat_token(Some(token.clone()), cx);
            let gen = starting(cx);
            {
                let c = cx.global_mut::<Chat>();
                c.conv.push_user(&out("哪些需要我？"));
                enqueue(c, out("哪些需要我？"));
                c.timers.interrupted_at = Some(Instant::now());
            }
            let on = cx.global::<AppSettings>().0.monitor.clone();
            let off = MonitorSettings { enabled: false, ..on.clone() };
            cx.global_mut::<AppSettings>().0.monitor = off.clone();
            settings_changed(&on, &off, cx);
            {
                let c = cx.global::<Chat>();
                assert!(c.queue.is_empty() && c.queue_start.is_none());
                assert!(matches!(c.proc, Proc::None));
                assert_eq!(c.timers.interrupted_at, Option::None);
                assert_eq!(c.conv.status, Status::Idle, "not starting any more");
                assert_eq!(c.conv.messages.len(), 1, "the history stays");
            }
            assert!(!authorized(cx.global::<Tokens>(), &token));
            // The start that was under way ends later: nobody hears of it.
            let launch = Launch::of(&on);
            on_started(gen, launch, "claude".into(), Err(ProviderError::NotFound { program: "claude".into() }), cx);
            on_event(gen, exited(), cx);
            let v = view(cx);
            assert_eq!(v.conv.messages.len(), 1, "no card for a process let go: {:?}", v.conv.messages);
            assert_eq!(v.conv.status, Status::Idle);
        });
    }

    #[gpui::test]
    fn other_setting_changes_keep_the_chat(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            let token = "f".repeat(32);
            set_chat_token(Some(token.clone()), cx);
            starting(cx);
            let on = cx.global::<AppSettings>().0.monitor.clone();
            let new = MonitorSettings { model: "sonnet".into(), ..on.clone() };
            settings_changed(&on, &new, cx);
            assert!(matches!(cx.global::<Chat>().proc, Proc::Starting), "the next message restarts, not this");
            assert!(authorized(cx.global::<Tokens>(), &token));
        });
    }

    #[gpui::test]
    fn tick_without_a_process_does_nothing(cx: &mut TestAppContext) {
        cx.update(tick); // before `init`
        setup(cx, true);
        cx.update(|cx| {
            cx.global_mut::<Chat>().timers = Timers::new(Instant::now() - Duration::from_secs(3600));
            let before = view(cx).revision;
            tick(cx);
            assert_eq!(view(cx).revision, before);
            starting(cx);
            let before = view(cx).revision;
            tick(cx);
            assert_eq!(view(cx).revision, before, "a process still starting is not timed");
            assert!(matches!(cx.global::<Chat>().proc, Proc::Starting));
        });
    }

    #[gpui::test]
    fn stop_without_a_process_drops_the_queue(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            {
                let c = cx.global_mut::<Chat>();
                c.conv.push_user(&out("哪些需要我？"));
                enqueue(c, out("哪些需要我？"));
            }
            stop(cx);
            let c = cx.global::<Chat>();
            assert!(c.queue.is_empty() && c.queue_start.is_none());
            assert_eq!(c.timers.interrupted_at, Option::None, "nothing to interrupt");
            assert_eq!(c.conv.messages.len(), 1, "the bubble stays");
            shutdown(cx); // nothing to kill
        });
    }

    // ---- With a real process: a shell script that swallows its input (no CLI, no model). ----

    fn ago(d: Duration) -> Instant {
        Instant::now().checked_sub(d).expect("the machine has been up longer than that")
    }

    struct Fake {
        dir: tempfile::TempDir,
        gen: u64,
        pid: u32,
        token: String,
        events: async_channel::Receiver<ChatEvent>,
    }

    /// Starts the fake CLI and installs it as the running chat process (as `on_started` would, minus the pump).
    fn running(cx: &mut App, answering: bool) -> Fake {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-cli");
        std::fs::write(&script, "#!/bin/sh\nexec cat >/dev/null\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let token = new_token();
        let cfg = ChatConfig {
            kind: ProviderKind::Claude,
            program: Some(script.display().to_string()),
            model: Option::None,
            run_dir: dir.path().join("run"),
            path: Option::None,
            instructions: "test".into(),
            mcp: McpLaunch { gilvt: dir.path().join("gilvt"), socket: dir.path().join("sock"), token: token.clone() },
            log: Option::None,
            record: Option::None,
        };
        let handle = match chat::start(&cfg) {
            Ok(h) => h,
            Err(e) => panic!("the fake CLI did not start: {}", e.message()),
        };
        let (pid, events) = (handle.pid(), handle.events());
        set_chat_token(Some(token.clone()), cx);
        let launch = Launch::of(&cx.global::<AppSettings>().0.monitor);
        let c = cx.global_mut::<Chat>();
        c.gen += 1;
        c.starts += 1;
        c.timers = Timers::new(Instant::now());
        c.proc = Proc::Running { handle, launch, answering };
        c.conv.status = if answering { Status::Answering } else { Status::Idle };
        Fake { dir, gen: c.gen, pid, token, events }
    }

    /// Waits (≤ 5 s) for the process to be gone and reaped.
    fn gone(pid: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            // SAFETY: signal 0 only checks that the process exists.
            if unsafe { libc::kill(pid as i32, 0) } != 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Hands the let-go process's last events (its exit included) to `on_event`, as its pump would.
    fn drain(f: &Fake, cx: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while let Some(ev) = chat::recv_until(&f.events, deadline) {
            on_event(f.gen, ev, cx);
        }
    }

    fn has_exit_card(cx: &App) -> bool {
        cx.global::<Chat>().conv.messages.iter().any(|m| m.text.starts_with("监控官进程已退出"))
    }

    #[gpui::test]
    fn a_silent_turn_is_interrupted(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            let f = running(cx, true);
            cx.global_mut::<Chat>().timers.last_output = ago(TURN_SILENCE + Duration::from_secs(1));
            tick(cx);
            {
                let c = cx.global::<Chat>();
                assert!(c.timers.interrupted_at.is_some());
                assert_eq!(c.conv.status, Status::Stopping);
                assert_eq!(c.conv.messages.last().map(|m| m.text.as_str()), Some("这一轮 5 分钟没有任何输出，已中断"));
                assert!(matches!(c.proc, Proc::Running { .. }), "only the turn is interrupted");
            }
            assert!(authorized(cx.global::<Tokens>(), &f.token));
            new_conversation(cx);
            assert!(gone(f.pid), "the process ends with 「新对话」");
        });
    }

    #[gpui::test]
    fn stop_does_not_put_off_a_pending_interrupt(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            let f = running(cx, true);
            stop(cx);
            let first = cx.global::<Chat>().timers.interrupted_at.expect("stop interrupts the answer");
            assert_eq!(cx.global::<Chat>().conv.status, Status::Stopping);
            {
                let c = cx.global_mut::<Chat>();
                c.timers.interrupted_at = Some(ago(Duration::from_secs(4)));
                c.conv.push_user(&out("还有一个问题"));
                enqueue(c, out("还有一个问题"));
            }
            let pending = cx.global::<Chat>().timers.interrupted_at;
            assert_ne!(pending, Some(first));
            stop(cx);
            let c = cx.global::<Chat>();
            assert_eq!(c.timers.interrupted_at, pending, "a second 「停止」 keeps the first interrupt's clock");
            assert!(c.queue.is_empty() && c.queue_start.is_none(), "the queue is still dropped");
            new_conversation(cx);
            assert!(gone(f.pid));
        });
    }

    #[gpui::test]
    fn an_ignored_interrupt_restarts_for_the_queued_message(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            let f = running(cx, true);
            // The restart's own start finds no CLI (a bare name nowhere on PATH; Codex writes no file first).
            std::fs::write(f.dir.path().join("gilvt"), "").unwrap();
            cx.set_global(ShellEnv { integration: None, socket: Some(f.dir.path().join("sock")), bin_dir: Some(f.dir.path().to_path_buf()), user: None });
            {
                let m = &mut cx.global_mut::<AppSettings>().0.monitor;
                m.provider = MonitorProvider::Codex;
                m.command = "gilvt-test-no-such-cli".into();
            }
            {
                let c = cx.global_mut::<Chat>();
                c.conv.push_user(&out("哪些需要我？"));
                c.conv.apply(&ChatEvent::TextDone { message: "m1".into(), text: "两个会话在等你".into() }, &|_| Option::None);
                c.conv.push_user(&out("第二个问题"));
                enqueue(c, out("第二个问题"));
                c.timers.interrupted_at = Some(ago(INTERRUPT_GRACE + Duration::from_secs(1)));
            }
            tick(cx);
            {
                let c = cx.global::<Chat>();
                assert!(c.gen > f.gen + 1, "let go, then a new start");
                assert!(matches!(c.proc, Proc::Starting));
                assert_eq!(c.timers.interrupted_at, Option::None);
                assert!(c.conv.messages.iter().any(|m| m.text == "中断没有响应，已结束监控官进程"));
                let p = c.prelude.clone().expect("a prelude for the new process");
                assert!(p.contains("哪些需要我？") && !p.contains("第二个问题"), "{p}");
                assert_eq!(c.queue.len(), 1, "the queued message waits for the new process");
            }
            assert!(!authorized(cx.global::<Tokens>(), &f.token), "the old token is void");
            assert!(tools::has_chat_token(cx), "the new process has its own");
            assert!(gone(f.pid));
            drain(&f, cx);
            assert!(!has_exit_card(cx), "a kill is not an unexpected exit");
        });
    }

    #[gpui::test]
    fn an_ignored_interrupt_without_a_queue_just_ends(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            let f = running(cx, true);
            cx.global_mut::<Chat>().timers.interrupted_at = Some(ago(INTERRUPT_GRACE + Duration::from_secs(1)));
            tick(cx);
            {
                let c = cx.global::<Chat>();
                assert!(matches!(c.proc, Proc::None), "no restart");
                assert_eq!(c.gen, f.gen + 1);
                assert_eq!(c.timers.interrupted_at, Option::None);
                assert_eq!(c.conv.status, Status::Idle);
                assert_eq!(c.conv.messages.last().map(|m| m.text.as_str()), Some("中断没有响应，已结束监控官进程"));
            }
            assert!(!tools::has_chat_token(cx));
            assert!(gone(f.pid));
            drain(&f, cx);
            assert!(!has_exit_card(cx));
        });
    }

    #[gpui::test]
    fn an_idle_process_ends_quietly(cx: &mut TestAppContext) {
        setup(cx, true);
        cx.update(|cx| {
            let f = running(cx, false);
            cx.global_mut::<Chat>().timers.last_use = ago(IDLE_LIMIT + Duration::from_secs(1));
            tick(cx);
            {
                let c = cx.global::<Chat>();
                assert!(matches!(c.proc, Proc::None));
                assert_eq!(c.conv.status, Status::Idle);
                assert!(c.conv.messages.is_empty(), "nothing is said: {:?}", c.conv.messages);
            }
            assert!(!tools::has_chat_token(cx));
            assert!(gone(f.pid));
            drain(&f, cx);
            assert!(!has_exit_card(cx) && cx.global::<Chat>().conv.messages.is_empty());
        });
    }
}
