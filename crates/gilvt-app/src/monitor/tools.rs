//! Answers the 监控官's tool calls (`Request::Monitor`, S2 §6.1) on the main thread: the token first, then the same
//! data the card wall shows (`gather::all_cards`). A session in an excluded directory does not exist here.
//!
//! Neither the token nor a `Request::Monitor` / `Query` is ever logged or `Debug`-printed (the token is a secret).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use gilvt_agent::{Item, PaneId, Turn, TurnOutcome};
use gilvt_ipc::{Query, Request, Response};
use gilvt_monitor::input::CommandDigest;
use gilvt_monitor::tools::{self, SessionDetail, SessionInfo, Source, SummaryInfo, TurnBrief};
use gpui::{App, Entity, Global};
use serde_json::Value;

use super::gather::{self, WallCard};
use super::model::{Card, TerminalCard, CATCHUP_BLOCKS};
use super::summaries;
use crate::agents::Agents;
use crate::terminal_view::TerminalView;
use crate::theme::AppSettings;

pub const BAD_TOKEN: &str = "token 无效：只有 gilvt 自己启动的监控官进程可以调用这些工具";
pub const DISABLED: &str = "监控官未开启";

/// [`DISABLED`] in the interface language, for gilvt's own UI (the model is told [`DISABLED`]).
pub fn disabled() -> &'static str {
    crate::i18n::text(DISABLED, "Monitor is off")
}
/// Turns summarized by `get_session` (the newest).
const BRIEF_TURNS: usize = 20;

/// The tokens accepted now: the chat process's and 「测试连接」's (each void once its process ends).
/// No `Debug`: the tokens are secrets.
#[derive(Default)]
pub struct Tokens {
    chat: Option<String>,
    test: Option<String>,
}

impl Global for Tokens {}

/// 128 random bits as 32 hex characters.
pub fn new_token() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).is_err() {
        use std::hash::{BuildHasher, Hasher};
        for (i, chunk) in bytes.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
            h.write_usize(i);
            chunk.copy_from_slice(&h.finish().to_le_bytes());
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time comparison (the token is a secret).
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn authorized(t: &Tokens, token: &str) -> bool {
    !token.is_empty() && [&t.chat, &t.test].into_iter().flatten().any(|valid| same(valid.as_bytes(), token.as_bytes()))
}

pub fn set_chat_token(token: Option<String>, cx: &mut App) {
    cx.default_global::<Tokens>().chat = token;
}

/// Whether a chat token is issued now (tests; the token itself is never handed out).
#[cfg(test)]
pub fn has_chat_token(cx: &App) -> bool {
    cx.try_global::<Tokens>().is_some_and(|t| t.chat.is_some())
}

pub fn set_test_token(token: Option<String>, cx: &mut App) {
    cx.default_global::<Tokens>().test = token;
}

fn tool_error(text: &str) -> Response {
    Response::Tool { text: text.to_string(), is_error: true }
}

/// One tool call: the token is checked before anything else is looked at.
pub fn answer(token: &str, tool: &str, args: &Value, cx: &App) -> Response {
    if !cx.try_global::<Tokens>().is_some_and(|t| authorized(t, token)) {
        return tool_error(BAD_TOKEN);
    }
    if !cx.global::<AppSettings>().0.monitor.enabled {
        return tool_error(DISABLED);
    }
    let call = match tools::parse_call(tool, args) {
        Ok(c) => c,
        Err(e) => return tool_error(&format!("参数错误：{e}")),
    };
    match tools::respond(&call, &AppSource::new(cx)) {
        Ok(t) => Response::Tool { text: t.text, is_error: false },
        Err(e) => tool_error(&e),
    }
}

/// One query from the IPC server's monitor channel (`ipc_bridge`); expired ones are skipped (nobody waits).
pub fn answer_query(q: Query, cx: &mut App) {
    if q.expired_at(Instant::now()) {
        return;
    }
    let resp = match &q.request {
        Request::Monitor { token, tool, args } => answer(token, tool, args, cx),
        _ => Response::Error { message: "not a monitor request".into() },
    };
    q.respond(resp);
}

/// Key → name of every card the 监控官 may see (the chat's tool rows say 「正在读取 web-login …」).
pub fn names(cx: &App) -> HashMap<String, String> {
    gather::all_cards(cx).into_iter().filter(|w| !w.card.excluded()).map(|w| (w.card.key(), w.card.name().to_string())).collect()
}

/// What a running command reads as when the command itself may not be sent.
const HIDDEN_RUNNING: &str = "命令运行中";

/// The wall's cards as the model may see them: excluded cards dropped, and every terminal card reduced to the
/// commands `kept(pane, cwd)` lets through (block ids, `summaries::shown_blocks`: the `get_commands` rule). A
/// hidden newest command takes its error line with it and is not replaced by an older one (status 「空闲」);
/// a hidden running one reads 「● 命令运行中」.
fn visible_cards(cards: Vec<WallCard>, kept: impl Fn(PaneId, Option<&Path>) -> HashSet<u64>) -> Vec<WallCard> {
    cards
        .into_iter()
        .filter(|w| !w.card.excluded())
        .map(|mut w| {
            if let Card::Terminal(t) = &mut w.card {
                redact_terminal(t, &kept(t.pane, w.cwd.as_deref()));
            }
            w
        })
        .collect()
}

fn redact_terminal(t: &mut TerminalCard, kept: &HashSet<u64>) {
    // `catchup` lists every block the card was built from, newest (the running one, if any) first.
    let newest_hidden = t.catchup.first().is_none_or(|b| !kept.contains(&b.id));
    if newest_hidden {
        t.error_line = None;
        if let Some((command, _)) = &mut t.running {
            *command = HIDDEN_RUNNING.to_string();
        }
    }
    if t.last.as_ref().is_some_and(|l| !kept.contains(&l.id)) {
        t.last = None;
    }
    t.catchup.retain(|b| kept.contains(&b.id));
}

/// The terminal view showing `pane`, in any window.
fn terminal_view(cx: &App, pane: PaneId) -> Option<Entity<TerminalView>> {
    crate::workspace::workspaces(cx).into_iter().find_map(|w| w.read(cx).ok()?.terminal_view(pane).cloned())
}

/// The card's ✦ summary for the model: only a real summary (ready / stale, or pending with the previous text).
/// `age` is the header's field after 「✦ AI 总结 · 」 (「刚刚」 / 「3 分钟前」); while pending, the 「上次 …」 one.
fn summary_info(w: &WallCard) -> Option<SummaryInfo> {
    let s = w.card.summary().filter(|s| matches!(s.state, "ready" | "stale" | "pending"))?;
    let recent = s.recent.clone()?;
    let fields: Vec<&str> = s.header.split(" · ").skip(1).collect();
    let age = fields.iter().find_map(|f| f.strip_prefix("上次 ").or_else(|| f.strip_prefix("previous "))).or(fields.first().copied()).unwrap_or("").to_string();
    Some(SummaryInfo { goal: s.goal.clone(), recent, age })
}

pub fn session_info(w: &WallCard) -> SessionInfo {
    let (_, status, _) = crate::monitor::view::card_lines(&w.card);
    let summary = summary_info(w);
    let cwd = w.cwd.as_ref().map(|p| p.display().to_string());
    match &w.card {
        Card::Agent(a) => SessionInfo {
            key: w.card.key(),
            kind: "agent",
            agent: Some(a.key.0.name()),
            name: a.name.clone(),
            group: w.group.id(),
            status,
            location: a.location.clone(),
            cwd,
            git: (!a.git.is_empty()).then(|| a.git.clone()),
            summary,
        },
        Card::Terminal(t) => SessionInfo {
            key: w.card.key(),
            kind: "terminal",
            agent: None,
            name: t.name.clone(),
            group: w.group.id(),
            status,
            location: t.location.clone(),
            cwd,
            git: None,
            summary,
        },
    }
}

fn outcome(o: &TurnOutcome) -> String {
    match o {
        TurnOutcome::Running => "进行中".into(),
        TurnOutcome::Done => "完成".into(),
        TurnOutcome::Interrupted => "被中断".into(),
        TurnOutcome::Failed { message } => format!("失败：{}", gilvt_monitor::output::clip_chars(message, 120)),
    }
}

fn brief(t: &Turn) -> TurnBrief {
    let (added, removed) = t
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Tool(x) => x.lines,
            _ => None,
        })
        .fold((0, 0), |(a, r), (x, y)| (a + x, r + y));
    let took = t.started.zip(t.ended).and_then(|(s, e)| e.duration_since(s).ok()).map(crate::sidebar::model::duration_label);
    TurnBrief {
        turn: t.index,
        prompt: gilvt_monitor::output::clip_chars(t.prompt.lines().next().unwrap_or("").trim(), 120),
        result: outcome(&t.outcome),
        took,
        added,
        removed,
    }
}

/// The wall's cards the 监控官 may see ([`visible_cards`]), and how to read more about them. Every per-key lookup
/// goes through `card`, over the visible cards only: an excluded key is simply not found.
struct AppSource<'a> {
    cx: &'a App,
    cards: Vec<WallCard>,
}

impl<'a> AppSource<'a> {
    fn new(cx: &'a App) -> Self {
        let kept = |pane: PaneId, cwd: Option<&Path>| -> HashSet<u64> {
            let Some(view) = terminal_view(cx, pane) else { return HashSet::new() };
            // The blocks the card was built from (`gather::model_for`).
            let blocks = view.read(cx).commands().recent(CATCHUP_BLOCKS);
            summaries::with_exclusions(cx, |ex| summaries::shown_blocks(&blocks, cwd, ex).iter().map(|b| b.id).collect())
        };
        Self::over(cx, visible_cards(gather::all_cards(cx), kept))
    }

    /// Over cards already passed through [`visible_cards`].
    fn over(cx: &'a App, cards: Vec<WallCard>) -> Self {
        AppSource { cx, cards }
    }

    fn card(&self, key: &str) -> Option<&WallCard> {
        self.cards.iter().find(|w| w.card.key() == key)
    }

    fn terminal(&self, key: &str) -> Option<Entity<TerminalView>> {
        terminal_view(self.cx, self.card(key)?.card.pane()?)
    }

    fn timeline(&self, key: &str) -> Option<Vec<Arc<Turn>>> {
        let Card::Agent(a) = &self.card(key)?.card else { return None };
        self.cx.try_global::<Agents>()?.timeline(&a.key).map(|t| t.turns.clone())
    }
}

impl Source for AppSource<'_> {
    fn sessions(&self) -> Vec<SessionInfo> {
        self.cards.iter().map(session_info).collect()
    }

    fn detail(&self, key: &str) -> Option<SessionDetail> {
        let w = self.card(key)?;
        let info = session_info(w);
        Some(match &w.card {
            Card::Agent(a) => {
                let turns = self.timeline(key).unwrap_or_default();
                let start = turns.len().saturating_sub(BRIEF_TURNS);
                SessionDetail {
                    info,
                    todo: a.todo.map(|(d, t)| format!("{d}/{t}")),
                    context: a.context.map(|c| format!("{}%", (c * 100.).round() as u32)),
                    turns: turns[start..].iter().map(|t| brief(t)).collect(),
                }
            }
            Card::Terminal(_) => SessionDetail { info, todo: None, context: None, turns: Vec::new() },
        })
    }

    fn turns(&self, key: &str) -> Option<Vec<Arc<Turn>>> {
        self.timeline(key)
    }

    /// Blocks run in, ending in, or naming an excluded directory are left out (the summaries service's own filter).
    fn commands(&self, key: &str, limit: usize) -> Option<Vec<CommandDigest>> {
        let w = self.card(key)?;
        let view = self.terminal(key)?;
        let blocks = view.read(self.cx).commands().recent(limit);
        Some(summaries::with_exclusions(self.cx, |ex| summaries::terminal_commands(&blocks, w.cwd.as_deref(), ex)).map(|(_, c)| c).unwrap_or_default())
    }

    /// Scrollback and screen (the last `lines` logical lines); the visible screen alone on the alternate screen.
    /// `lines_text` / `screen_tail` take the terminal lock themselves, only while copying those lines.
    fn screen(&self, key: &str, lines: usize) -> Option<String> {
        let view = self.terminal(key)?;
        let session = &view.read(self.cx).session;
        session.lines_text(0, i64::MAX as u64, lines).or_else(|| {
            let tail = session.screen_tail(lines);
            (!tail.is_empty()).then(|| tail.join("\n"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::model::{Group, TerminalCard};

    #[test]
    fn tokens_are_128_random_bits() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn only_current_tokens_are_accepted() {
        let mut t = Tokens::default();
        assert!(!authorized(&t, ""), "no token yet: nothing is accepted, not even an empty one");
        t.chat = Some("aaaa".into());
        assert!(authorized(&t, "aaaa"));
        assert!(!authorized(&t, "aaab") && !authorized(&t, "aaa") && !authorized(&t, ""));
        t.test = Some("bbbb".into());
        assert!(authorized(&t, "aaaa") && authorized(&t, "bbbb"), "chat and 测试连接 at once");
        t.chat = Some("cccc".into());
        assert!(!authorized(&t, "aaaa"), "an old chat token is void at once");
    }

    #[test]
    fn terminal_session_info() {
        let card = Card::Terminal(TerminalCard {
            pane: 3,
            name: "zsh".into(),
            cwd: "~/repo".into(),
            location: "zsh · 左".into(),
            running: None,
            last: None,
            error_line: None,
            foreground: None,
            catchup: Vec::new(),
            summary: None,
            excluded: false,
        });
        let info = session_info(&WallCard { group: Group::Terminals, card, cwd: Some("/Users/me/repo".into()) });
        assert_eq!((info.key.as_str(), info.kind, info.agent, info.group), ("pane:3", "terminal", None, "terminals"));
        assert_eq!(info.cwd.as_deref(), Some("/Users/me/repo"));
        assert_eq!(info.name, "zsh");
    }

    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::time::Duration;

    use gilvt_agent::{AgentKind, Session};
    use gilvt_monitor::privacy::Exclusions;
    use gilvt_monitor::tools::{respond, NOT_FOUND};
    use gilvt_term::CommandBlock;
    use gpui::TestAppContext;
    use serde_json::json;

    use crate::monitor::model::{self, AgentIn, SummarySlot, TerminalIn};
    use crate::monitor::MonitorUi;
    use crate::settings::Settings;

    const REPO: &str = "/repo";

    fn block(id: u64, cmd: &str, end_cwd: Option<&str>, ended: bool) -> CommandBlock {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        CommandBlock {
            id,
            command: Some(cmd.into()),
            cwd: Some(REPO.into()),
            end_cwd: end_cwd.map(PathBuf::from),
            started: t0,
            ended: ended.then_some(t0 + Duration::from_secs(5)),
            exit: ended.then_some(1),
            output_tail: Some(format!("output of {cmd}")),
            start_line: None,
            end_line: None,
        }
    }

    fn term(pane: u64, blocks: Vec<CommandBlock>, excluded: bool) -> TerminalIn {
        TerminalIn { pane, name: format!("zsh{pane}"), cwd: Some(REPO.into()), location: String::new(), foreground: None, blocks, summary: SummarySlot::Hidden, excluded }
    }

    fn agent_in(s: &Session, excluded: bool) -> AgentIn<'_> {
        AgentIn { session: s, name: s.key.1.clone(), location: String::new(), git: None, archived: false, plan: &[], turns: &[], cards: &[], summary: SummarySlot::Hidden, excluded }
    }

    /// The wall's cards as `gather::all_cards` returns them (every terminal in `REPO`).
    fn wall_cards(agents: &[AgentIn], terminals: &[TerminalIn]) -> Vec<WallCard> {
        let ui = MonitorUi { ended_open: true, ..Default::default() };
        let m = model::build(agents, terminals, &ui, Instant::now(), SystemTime::UNIX_EPOCH + Duration::from_secs(2000), None);
        m.groups
            .into_iter()
            .flat_map(|g| {
                let group = g.group;
                g.cards.into_iter().map(move |card| {
                    let cwd = matches!(card, Card::Terminal(_)).then(|| PathBuf::from(REPO));
                    WallCard { group, card, cwd }
                })
            })
            .collect()
    }

    #[test]
    fn terminal_status_never_shows_a_hidden_command() {
        let ex = Exclusions::new(&["/secret".into()], None);
        let terminals = vec![
            // The newest command ended in an excluded directory (`cd /secret/app && make deploy`).
            term(1, vec![block(12, "make deploy", Some("/secret/app"), true), block(11, "ls", Some(REPO), true)], false),
            // The running command names one.
            term(2, vec![block(22, "cat /secret/creds", None, false), block(21, "git status", Some(REPO), true)], false),
        ];
        let blocks: HashMap<u64, Vec<CommandBlock>> = terminals.iter().map(|t| (t.pane, t.blocks.clone())).collect();
        let cards = visible_cards(wall_cards(&[], &terminals), |pane, cwd| summaries::shown_blocks(&blocks[&pane], cwd, &ex).iter().map(|b| b.id).collect());
        let infos: Vec<SessionInfo> = cards.iter().map(session_info).collect();
        assert_eq!(infos.len(), 2);
        for (info, w) in infos.iter().zip(&cards) {
            for hidden in ["make deploy", "/secret", "output of"] {
                assert!(!info.status.contains(hidden), "{} leaks {hidden:?}: {}", info.key, info.status);
            }
            let Card::Terminal(t) = &w.card else { panic!() };
            assert!(t.error_line.is_none() && t.catchup.iter().all(|b| !b.command.contains("make deploy") && !b.command.contains("/secret")));
        }
        assert_eq!(infos[0].status, "空闲", "the hidden last command is not replaced by an older one");
        assert!(infos[1].status.starts_with("● 命令运行中"), "{}", infos[1].status);
    }

    #[test]
    fn a_terminal_without_its_view_shows_no_command() {
        let cards = visible_cards(wall_cards(&[], &[term(1, vec![block(2, "make", None, true)], false)]), |_, _| HashSet::new());
        assert_eq!(session_info(&cards[0]).status, "空闲");
    }

    #[gpui::test]
    fn excluded_sessions_do_not_exist_for_any_tool(cx: &mut TestAppContext) {
        let now = Instant::now();
        let seen = Session::new((AgentKind::Claude, "seen".into()), now);
        let hidden = Session::new((AgentKind::Claude, "hidden".into()), now);
        let all = wall_cards(&[agent_in(&seen, false), agent_in(&hidden, true)], &[term(9, Vec::new(), true)]);
        assert_eq!(all.len(), 3);
        let cards = visible_cards(all, |_, _| HashSet::new());
        assert_eq!(cards.iter().map(|w| w.card.key()).collect::<Vec<_>>(), ["agent:claude:seen"]);
        cx.update(|cx| {
            cx.set_global(AppSettings(Settings::default()));
            let src = AppSource::over(cx, cards);
            let list = respond(&tools::parse_call("list_sessions", &json!({})).unwrap(), &src).unwrap().text;
            assert!(list.contains("agent:claude:seen") && !list.contains("hidden") && !list.contains("pane:9"), "{list}");
            assert!(respond(&tools::parse_call("get_session", &json!({"key": "agent:claude:seen"})).unwrap(), &src).is_ok());
            let calls = [
                ("get_session", json!({"key": "agent:claude:hidden"})),
                ("get_timeline", json!({"key": "agent:claude:hidden", "turns": "last:1"})),
                ("read_screen", json!({"key": "agent:claude:hidden"})),
                ("get_session", json!({"key": "pane:9"})),
                ("get_commands", json!({"key": "pane:9"})),
                ("read_screen", json!({"key": "pane:9"})),
            ];
            for (tool, args) in calls {
                let key = args["key"].as_str().unwrap().to_string();
                let err = respond(&tools::parse_call(tool, &args).unwrap(), &src).unwrap_err();
                assert_eq!(err, format!("{NOT_FOUND}：{key}"), "{tool} {key}");
            }
        });
    }

    fn tool_text(r: Response) -> (String, bool) {
        match r {
            Response::Tool { text, is_error } => (text, is_error),
            _ => panic!("not a tool answer"),
        }
    }

    #[gpui::test]
    fn the_token_is_checked_before_anything_else(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut settings = Settings::default();
            settings.monitor.enabled = false;
            cx.set_global(AppSettings(settings));
            let args = json!({});
            let call = |token: &str, cx: &App| tool_text(answer(token, "list_sessions", &args, cx));
            assert_eq!(call("x", cx), (BAD_TOKEN.to_string(), true), "no token issued yet");
            let (chat, test) = ("a".repeat(32), "b".repeat(32));
            set_chat_token(Some(chat.clone()), cx);
            assert_eq!(call("wrong", cx), (BAD_TOKEN.to_string(), true), "a wrong token is refused even while the monitor is off");
            assert_eq!(call("", cx), (BAD_TOKEN.to_string(), true));
            assert_eq!(call(&chat, cx), (DISABLED.to_string(), true));
            set_test_token(Some(test.clone()), cx);
            assert_eq!(call(&test, cx), (DISABLED.to_string(), true));
            set_chat_token(None, cx);
            assert_eq!(call(&chat, cx), (BAD_TOKEN.to_string(), true), "a chat token is void once its process ended");
            // On: the token opens the tools (no window → no sessions).
            cx.global_mut::<AppSettings>().0.monitor.enabled = true;
            let (text, is_error) = call(&test, cx);
            assert!(!is_error && text.contains("\"sessions\":[]"), "{text}");
        });
    }
}
