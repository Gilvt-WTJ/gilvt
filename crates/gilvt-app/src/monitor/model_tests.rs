use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gilvt_agent::{AgentKind, Session, Status};
use gilvt_term::CommandBlock;

use super::*;
use crate::monitor::summaries::{SumState, SummaryView};
use crate::monitor::{Filter, MonitorUi};
use gilvt_monitor::input::Covers;
use gilvt_monitor::output::Summary;
use std::time::UNIX_EPOCH;

fn session(id: &str, status: Status, now: Instant) -> Session {
    let mut s = Session::new((AgentKind::Claude, id.to_string()), now);
    s.status = status;
    s.pane = Some(id.len() as u64);
    s
}

fn agent<'a>(s: &'a Session) -> AgentIn<'a> {
    AgentIn { session: s, name: s.key.1.clone(), location: "api · 左".into(), git: None, archived: false, plan: &[], turns: &[], cards: &[], summary: SummarySlot::Hidden, excluded: false }
}

fn block(id: u64, cmd: &str, exit: Option<i32>, ended: bool, tail: Option<&str>) -> CommandBlock {
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
    CommandBlock {
        id,
        command: Some(cmd.into()),
        cwd: None,
        end_cwd: None,
        started: t0,
        ended: ended.then_some(t0 + Duration::from_secs(5)),
        exit,
        output_tail: tail.map(str::to_string),
        start_line: Some(10),
        end_line: None,
    }
}

fn term(pane: u64, blocks: Vec<CommandBlock>) -> TerminalIn {
    TerminalIn { pane, name: "zsh".into(), cwd: Some("/Users/me/repo".into()), location: "zsh".into(), foreground: None, blocks, summary: SummarySlot::Hidden, excluded: false }
}

fn terminal_only(blocks: Vec<CommandBlock>) -> TerminalCard {
    let m = build(&[], &[term(7, blocks)], &MonitorUi::default(), Instant::now(), wall(), None);
    let Card::Terminal(t) = m.groups.into_iter().next().unwrap().cards.into_iter().next().unwrap() else { panic!() };
    t
}

fn wall() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1185)
}

#[test]
fn groups_follow_sidebar_status_order() {
    let now = Instant::now();
    let mut waiting = session("w", Status::NeedsApproval { action: "Bash(rm)".into() }, now);
    waiting.waiting_since = Some(now);
    let running = session("run", Status::Thinking, now);
    let mut done = session("done", Status::Idle, now);
    done.unseen_done = true;
    let idle = session("idle", Status::Idle, now);
    let error = session("err", Status::Error { message: "api".into() }, now);
    let ended = session("end", Status::Ended, now);
    let all = [&idle, &done, &running, &error, &waiting, &ended];
    let ins: Vec<AgentIn> = all.iter().map(|s| agent(s)).collect();
    let m = build(&ins, &[term(99, vec![])], &MonitorUi::default(), now, wall(), None);
    let order: Vec<Group> = m.groups.iter().map(|g| g.group).collect();
    assert_eq!(order, vec![Group::NeedsYou, Group::Error, Group::Running, Group::Done, Group::Idle, Group::Terminals, Group::Ended]);
    assert!(m.groups.last().unwrap().collapsed, "已结束 starts collapsed");
    assert_eq!(m.needs_you(), 1);
}

#[test]
fn archived_ended_sessions_are_left_out() {
    let now = Instant::now();
    let ended = session("end", Status::Ended, now);
    let mut a = agent(&ended);
    a.archived = true;
    let m = build(&[a], &[], &MonitorUi::default(), now, wall(), None);
    assert!(m.groups.is_empty());
}

#[test]
fn filter_keeps_one_group_but_counts_all() {
    let now = Instant::now();
    let running = session("run", Status::Thinking, now);
    let ui = MonitorUi { filter: Filter::Only(Group::Terminals), ..Default::default() };
    let m = build(&[agent(&running)], &[term(7, vec![])], &ui, now, wall(), None);
    assert_eq!(m.groups.iter().map(|g| g.group).collect::<Vec<_>>(), vec![Group::Terminals]);
    assert!(m.counts.contains(&(Group::Running, 1)));
    assert!(m.counts.contains(&(Group::Terminals, 1)));
    assert!(!m.counts.iter().any(|(g, _)| *g == Group::NeedsYou), "zero counts are left out");
}

#[test]
fn terminal_card_shows_the_running_command() {
    let now = Instant::now();
    let m = build(&[], &[term(7, vec![block(2, "make test", None, false, None), block(1, "git pull", Some(0), true, None)])], &MonitorUi::default(), now, wall(), None);
    let Card::Terminal(t) = &m.groups[0].cards[0] else { panic!() };
    let (cmd, elapsed) = t.running.clone().unwrap();
    assert_eq!(cmd, "make test");
    assert_eq!(elapsed, "3 分钟");
    assert_eq!(t.last.as_ref().unwrap().command, "git pull");
    assert_eq!(t.error_line, None);
}

#[test]
fn terminal_card_shows_the_last_failure() {
    let now = Instant::now();
    let tail = "running 3 tests\n\nerror: test cache::ttl timed out\n   \n";
    let m = build(&[], &[term(7, vec![block(1, "make test", Some(2), true, Some(tail))])], &MonitorUi::default(), now, wall(), Some(Path::new("/Users/me")));
    let Card::Terminal(t) = &m.groups[0].cards[0] else { panic!() };
    let last = t.last.as_ref().unwrap();
    assert_eq!((last.mark, last.exit), (Mark::Failed, Some(2)));
    assert_eq!(last.ago, "3 分钟前");
    assert_eq!(t.error_line.as_deref(), Some("error: test cache::ttl timed out"));
    assert_eq!(t.cwd, "~/repo");
}

#[test]
fn failed_last_command_shows_its_last_output_line() {
    let b = block(1, "make test", Some(2), true, Some("ok pkg/a\nFAIL pkg/cache TestEvictTTL\n"));
    let card = terminal_only(vec![b]);
    assert_eq!(card.error_line.as_deref(), Some("FAIL pkg/cache TestEvictTTL"));
}

#[test]
fn no_error_line_when_ok_running_or_without_output() {
    assert_eq!(terminal_only(vec![block(1, "ls", Some(0), true, Some("a b"))]).error_line, None);
    let failed = block(1, "make", Some(1), true, Some("boom"));
    let running = block(2, "sleep 5", None, false, None);
    assert_eq!(terminal_only(vec![running, failed]).error_line, None, "a command runs now");
    assert_eq!(terminal_only(vec![block(1, "false", Some(1), true, None)]).error_line, None, "no output");
}

#[test]
fn multi_line_command_shows_its_first_line() {
    let now = Instant::now();
    let blocks = vec![block(2, "for f in *\ndo echo $f\ndone", None, false, None), block(1, "echo 'a\nb'", Some(0), true, None), block(0, "ls\n", Some(0), true, None)];
    let m = build(&[], &[term(7, blocks)], &MonitorUi::default(), now, wall(), None);
    let Card::Terminal(t) = &m.groups[0].cards[0] else { panic!() };
    assert_eq!(t.running.as_ref().unwrap().0, "for f in *…");
    assert_eq!(t.last.as_ref().unwrap().command, "echo 'a…");
    assert_eq!(t.catchup.iter().map(|b| b.command.as_str()).collect::<Vec<_>>(), vec!["for f in *…", "echo 'a…", "ls"]);
}

fn art_card(turn: u32, added: u32) -> crate::inspector::artifacts_model::ArtCard {
    use crate::inspector::artifacts_model::{ArtCard, CardState, FileRow};
    ArtCard {
        key: turn,
        turn: Some(turn),
        turns: vec![turn],
        title: String::new(),
        follow_ups: Vec::new(),
        started: String::new(),
        took: None,
        state: CardState::Done,
        files: vec![FileRow { path: "a.rs".into(), abs: "/r/a.rs".into(), status: 'M', added, removed: 1, binary: false, old_path: None }],
        computing: false,
        counts_hidden: false,
        test: None,
        quote: String::new(),
        touched_later: false,
        touched_later_turn: None,
        notices: Vec::new(),
        range: None,
        live_base: None,
        quiet_group: None,
    }
}

#[test]
fn agent_card_changes_are_the_current_turns() {
    use gilvt_agent::Turn;
    let now = Instant::now();
    let s = session("run", Status::Thinking, now);
    let mk = |index: u32| {
        let mut t = Turn::new_for_test("p");
        t.index = index;
        Arc::new(t)
    };
    // Turn 3 runs and has no card yet; the newest card is turn 2's.
    let turns = vec![mk(1), mk(2), mk(3)];
    let cards = vec![art_card(2, 20), art_card(1, 10)];
    let mut a = agent(&s);
    a.turns = &turns;
    a.cards = &cards;
    let m = build(&[a], &[], &MonitorUi::default(), now, wall(), None);
    let Card::Agent(c) = &m.groups[0].cards[0] else { panic!() };
    assert_eq!(c.turn, Some(3));
    assert_eq!(c.changes, None, "not turn 2's changes");

    let turns = vec![mk(1), mk(2)];
    let mut a = agent(&s);
    a.turns = &turns;
    a.cards = &cards;
    let m = build(&[a], &[], &MonitorUi::default(), now, wall(), None);
    let Card::Agent(c) = &m.groups[0].cards[0] else { panic!() };
    assert_eq!(c.changes, Some(Changes { files: 1, added: Some(20), removed: Some(1) }));
}

#[test]
fn a_task_card_covers_its_follow_ups() {
    use gilvt_agent::Turn;
    let now = Instant::now();
    let s = session("run", Status::Idle, now);
    let mk = |index: u32| {
        let mut t = Turn::new_for_test("p");
        t.index = index;
        Arc::new(t)
    };
    // Turn 2 (「继续」) joined turn 1's task: one card, led by turn 1.
    let turns = vec![mk(1), mk(2)];
    let mut task = art_card(1, 30);
    task.turns = vec![1, 2];
    let cards = vec![task];
    let mut a = agent(&s);
    a.turns = &turns;
    a.cards = &cards;
    let m = build(&[a], &[], &MonitorUi::default(), now, wall(), None);
    let Card::Agent(c) = &m.groups[0].cards[0] else { panic!() };
    assert_eq!(c.turn, Some(2));
    assert_eq!(c.changes, Some(Changes { files: 1, added: Some(30), removed: Some(1) }), "the follow-up's card is its task's");
    let lines: Vec<(u32, Option<Changes>)> = c.catchup.iter().map(|l| (l.turn, l.changes)).collect();
    assert_eq!(lines, vec![(2, Some(Changes { files: 1, added: Some(30), removed: Some(1) })), (1, None)], "the task's changes go on its last turn's line only");
}

#[test]
fn a_task_without_net_counts_shows_only_its_file_count() {
    use crate::monitor::view::{card_lines, catchup_texts};
    use gilvt_agent::Turn;
    let now = Instant::now();
    let s = session("run", Status::Idle, now);
    let mk = |index: u32| {
        let mut t = Turn::new_for_test("p");
        t.index = index;
        Arc::new(t)
    };
    let turns = vec![mk(1), mk(2)];
    // The task's net is still computing, or its counts are hidden: its rows are a union, not a net.
    for (computing, counts_hidden) in [(true, false), (false, true)] {
        let mut task = art_card(1, 30);
        task.turns = vec![1, 2];
        (task.computing, task.counts_hidden) = (computing, counts_hidden);
        let cards = vec![task];
        let mut a = agent(&s);
        a.turns = &turns;
        a.cards = &cards;
        let m = build(&[a], &[], &MonitorUi::default(), now, wall(), None);
        let card = &m.groups[0].cards[0];
        let Card::Agent(c) = card else { panic!() };
        assert_eq!(c.changes, Some(Changes { files: 1, added: None, removed: None }));
        assert!(card_lines(card).2.ends_with("· 1 文件") && !card_lines(card).2.contains('+'), "{}", card_lines(card).2);
        assert!(catchup_texts(card)[0].ends_with("· 1 文件"), "{:?}", catchup_texts(card));
    }
}

#[test]
fn agent_card_elapsed_counts_while_waiting_in_a_turn() {
    let now = Instant::now();
    for status in [Status::NeedsApproval { action: "Bash(rm)".into() }, Status::Asking { question: "?".into() }] {
        let mut s = session("w", status, now);
        s.turn_started = Some(now - Duration::from_secs(120));
        s.last_turn = Some(Duration::from_secs(5));
        let m = build(&[agent(&s)], &[], &MonitorUi::default(), now, wall(), None);
        let Card::Agent(c) = &m.groups[0].cards[0] else { panic!() };
        assert_eq!(c.elapsed.as_deref(), Some("2 分钟"), "{:?}", s.status);
    }
    // Background tasks after the turn's end: the session is still working on it.
    let mut s = session("bg", Status::Idle, now);
    s.background_tasks = 1;
    s.turn_started = Some(now - Duration::from_secs(180));
    s.last_turn = Some(Duration::from_secs(5));
    let m = build(&[agent(&s)], &[], &MonitorUi::default(), now, wall(), None);
    let Card::Agent(c) = &m.groups[0].cards[0] else { panic!() };
    assert_eq!(c.elapsed.as_deref(), Some("3 分钟"));
    // Idle after the turn: the last turn's length.
    let mut s = session("idle", Status::Idle, now);
    s.last_turn = Some(Duration::from_secs(5));
    let m = build(&[agent(&s)], &[], &MonitorUi::default(), now, wall(), None);
    let Card::Agent(c) = &m.groups[0].cards[0] else { panic!() };
    assert_eq!(c.elapsed.as_deref(), Some("用时 5 秒"));
    assert_eq!(crate::monitor::view::card_lines(&m.groups[0].cards[0]).2, "用时 5 秒");
}

#[test]
fn the_last_turns_length_reads_took_in_english() {
    use crate::i18n::{has_chinese, with_language, Language};
    let now = Instant::now();
    let mut s = session("idle", Status::Idle, now);
    s.last_turn = Some(Duration::from_secs(300));
    let (elapsed, meta) = with_language(Language::English, || {
        let m = build(&[agent(&s)], &[], &MonitorUi::default(), now, wall(), None);
        let Card::Agent(c) = &m.groups[0].cards[0] else { panic!() };
        (c.elapsed.clone(), crate::monitor::view::card_lines(&m.groups[0].cards[0]).2)
    });
    assert_eq!(elapsed.as_deref(), Some("took 5m"));
    assert_eq!(meta, "took 5m", "not 「This turn 用时 5m」");
    assert!(!has_chinese(&meta));
    // In a turn: the time so far, prefixed in both languages.
    let mut s = session("w", Status::Thinking, now);
    s.turn_started = Some(now - Duration::from_secs(120));
    let line = |s: &Session| {
        let m = build(&[agent(s)], &[], &MonitorUi::default(), now, wall(), None);
        crate::monitor::view::card_lines(&m.groups[0].cards[0]).2
    };
    assert_eq!(with_language(Language::English, || line(&s)), "This turn 2m");
    assert_eq!(line(&s), "本轮 2 分钟");
}

#[test]
fn terminal_lines_and_tab_title_in_english() {
    use crate::i18n::{has_chinese, with_language, Language};
    let now = Instant::now();
    let mut b = block(1, "", Some(2), true, None);
    b.command = None;
    let (last, title) = with_language(Language::English, || {
        let m = build(&[], &[term(7, vec![b])], &MonitorUi::default(), now, wall(), None);
        let Card::Terminal(t) = &m.groups[0].cards[0] else { panic!() };
        (crate::monitor::view::block_text(t.last.as_ref().unwrap(), true), tab_title_with_needs_you("◎ Monitor".into(), 2))
    });
    assert!(last.starts_with("Last: ✗ (unknown command) · exit 2"), "{last}");
    assert!(last.ends_with(" ago"), "{last}");
    assert!(!has_chinese(&last), "{last}");
    assert_eq!(title, "◎ Monitor · 2 need you");
}

#[test]
fn unknown_command_text() {
    let now = Instant::now();
    let mut b = block(1, "", Some(0), true, None);
    b.command = None;
    let m = build(&[], &[term(7, vec![b])], &MonitorUi::default(), now, wall(), None);
    let Card::Terminal(t) = &m.groups[0].cards[0] else { panic!() };
    assert_eq!(t.last.as_ref().unwrap().command, "（命令未知）");
}

#[test]
fn agent_card_meta_and_todo() {
    use gilvt_agent::{PlanItem, PlanState};
    let now = Instant::now();
    let mut s = session("run", Status::Tool { label: "Bash(pnpm test)".into() }, now);
    s.turn_started = Some(now - Duration::from_secs(360));
    s.context = Some((48_000, Some(100_000)));
    let plan = [
        PlanItem { text: "a".into(), state: PlanState::Done },
        PlanItem { text: "b".into(), state: PlanState::Done },
        PlanItem { text: "c".into(), state: PlanState::Active },
    ];
    let mut a = agent(&s);
    a.plan = &plan;
    a.git = Some("⎇ main ●7 ↑2".into());
    let m = build(&[a], &[], &MonitorUi::default(), now, wall(), None);
    let Card::Agent(c) = &m.groups[0].cards[0] else { panic!() };
    assert_eq!(c.status, "● 执行中 · Bash(pnpm test)");
    assert_eq!(c.elapsed.as_deref(), Some("6 分钟"));
    assert_eq!(c.todo, Some((2, 3)));
    assert_eq!(c.git, "⎇ main ●7 ↑2");
    assert!((c.context.unwrap() - 0.48).abs() < 0.01);
}

#[test]
fn catchup_lists_turns_newest_first() {
    use gilvt_agent::{Turn, TurnOutcome};
    let now = Instant::now();
    let s = session("t", Status::Idle, now);
    let mk = |index: u32, prompt: &str, outcome: TurnOutcome| {
        let mut t = Turn::new_for_test(prompt);
        t.index = index;
        t.outcome = outcome;
        t.started = Some(SystemTime::UNIX_EPOCH);
        t.ended = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(90));
        Arc::new(t)
    };
    let turns = vec![mk(1, "第一轮：列出文件\n细节", TurnOutcome::Done), mk(2, "第二轮", TurnOutcome::Failed { message: "x".into() })];
    let mut a = agent(&s);
    a.turns = &turns;
    let m = build(&[a], &[], &MonitorUi::default(), now, wall(), None);
    let Card::Agent(c) = &m.groups[0].cards[0] else { panic!() };
    assert_eq!(c.catchup.iter().map(|l| l.turn).collect::<Vec<_>>(), vec![2, 1]);
    assert_eq!(c.catchup[1].prompt, "第一轮：列出文件");
    assert_eq!(c.catchup[0].mark, Mark::Failed);
    assert_eq!(c.catchup[1].took.as_deref(), Some("1 分钟"));
}

#[test]
fn agent_pane_has_no_terminal_card() {
    // The caller drops occupied panes (sidebar `collect` does); the model never invents terminal cards.
    let now = Instant::now();
    let s = session("run", Status::Thinking, now);
    let m = build(&[agent(&s)], &[], &MonitorUi::default(), now, wall(), None);
    assert!(m.groups.iter().all(|g| g.group != Group::Terminals));
}

#[test]
fn ticking_whenever_cards_are_drawn() {
    let now = Instant::now();
    assert!(!build(&[], &[], &MonitorUi::default(), now, wall(), None).ticking(), "an empty wall has nothing to refresh");

    // Relative times (「3 分钟前」) move too: an idle agent and a finished command still tick.
    let idle = session("idle", Status::Idle, now);
    assert!(build(&[agent(&idle)], &[term(7, vec![block(1, "ls", Some(0), true, None)])], &MonitorUi::default(), now, wall(), None).ticking());
    let running = session("run", Status::Thinking, now);
    assert!(build(&[agent(&running)], &[], &MonitorUi::default(), now, wall(), None).ticking());

    // Only the collapsed 已结束 is drawn: no card shows a time.
    let mut ended = session("end", Status::Ended, now);
    ended.pane = None;
    assert!(!build(&[agent(&ended)], &[], &MonitorUi::default(), now, wall(), None).ticking());
}

#[test]
fn filter_on_an_emptied_group_falls_back_to_all() {
    let now = Instant::now();
    let running = session("run", Status::Thinking, now);
    let ui = MonitorUi { filter: Filter::Only(Group::NeedsYou), ..Default::default() };
    let m = build(&[agent(&running)], &[term(7, vec![])], &ui, now, wall(), None);
    assert_eq!(m.filter, Filter::All);
    assert_eq!(m.groups.iter().map(|g| g.group).collect::<Vec<_>>(), vec![Group::Running, Group::Terminals]);

    // A filter on a group that has cards applies as stored.
    let ui = MonitorUi { filter: Filter::Only(Group::Terminals), ..Default::default() };
    let m = build(&[agent(&running)], &[term(7, vec![])], &ui, now, wall(), None);
    assert_eq!(m.filter, Filter::Only(Group::Terminals));
    assert_eq!(m.groups.iter().map(|g| g.group).collect::<Vec<_>>(), vec![Group::Terminals]);
}

#[test]
fn a_fallen_back_filter_is_reset_to_all() {
    // 需要你 empties: the wall shows 全部, and the stored filter follows, so a later approval request does not
    // snap the wall back to 需要你 alone.
    let now = Instant::now();
    let running = session("run", Status::Thinking, now);
    let mut ui = MonitorUi { filter: Filter::Only(Group::NeedsYou), ..Default::default() };
    let m = build(&[agent(&running)], &[term(7, vec![])], &ui, now, wall(), None);
    ui.settle(&m);
    assert_eq!(ui.filter, Filter::All);
    let mut waiting = session("w", Status::NeedsApproval { action: "Bash(rm)".into() }, now);
    waiting.waiting_since = Some(now);
    let m = build(&[agent(&running), agent(&waiting)], &[term(7, vec![])], &ui, now, wall(), None);
    assert_eq!(m.filter, Filter::All);
    assert_eq!(m.groups.len(), 3);
    // A filter that applies is kept.
    let mut ui = MonitorUi { filter: Filter::Only(Group::Terminals), ..Default::default() };
    ui.settle(&build(&[], &[term(7, vec![])], &ui, now, wall(), None));
    assert_eq!(ui.filter, Filter::Only(Group::Terminals));
}

#[test]
fn needs_you_count_matches_the_model() {
    let now = Instant::now();
    let mut waiting = session("w", Status::NeedsApproval { action: "Bash(rm)".into() }, now);
    waiting.waiting_since = Some(now);
    let mut asking = session("ask", Status::Asking { question: "?".into() }, now);
    asking.waiting_since = Some(now);
    let running = session("run", Status::Thinking, now);
    let ended = session("end", Status::Ended, now);
    let all = [waiting, asking, running, ended];
    let ins: Vec<AgentIn> = all.iter().map(agent).collect();
    let m = build(&ins, &[], &MonitorUi::default(), now, wall(), None);
    assert_eq!(m.needs_you(), 2);
    assert_eq!(needs_you_count(&all), 2);
}

#[test]
fn monitor_tab_title_counts_who_needs_you() {
    assert_eq!(tab_title_with_needs_you("◎ 监控官".into(), 0), "◎ 监控官");
    assert_eq!(tab_title_with_needs_you("◎ 监控官".into(), 2), "◎ 监控官 · 2 需要你");
}

#[test]
fn step_selection_moves_and_clamps() {
    let keys: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
    // First press selects the first card, whatever the direction.
    assert_eq!(step_selection(&keys, None, "down").as_deref(), Some("a"));
    assert_eq!(step_selection(&keys, None, "up").as_deref(), Some("a"));
    assert_eq!(step_selection(&keys, Some("gone"), "right").as_deref(), Some("a"));
    assert_eq!(step_selection(&keys, Some("a"), "down").as_deref(), Some("b"));
    assert_eq!(step_selection(&keys, Some("b"), "left").as_deref(), Some("a"));
    // Clamps at both ends.
    assert_eq!(step_selection(&keys, Some("c"), "right").as_deref(), Some("c"));
    assert_eq!(step_selection(&keys, Some("a"), "up").as_deref(), Some("a"));
    // Empty wall, other keys.
    assert_eq!(step_selection(&[], None, "down"), None);
    assert_eq!(step_selection(&keys, Some("a"), "enter"), None);
}

fn sv(state: SumState, summary: Option<(&str, &str)>, at_secs: u64) -> Arc<SummaryView> {
    Arc::new(SummaryView {
        summary: summary.map(|(g, r)| Summary { goal: (!g.is_empty()).then(|| g.to_string()), recent: r.into() }),
        generated_at: summary.map(|_| UNIX_EPOCH + Duration::from_secs(at_secs)),
        covers: summary.map(|_| Covers::Turns(2, 3)),
        activity: 1,
        state,
    })
}

const NOW: u64 = 10_000;

fn line(slot: SummarySlot, terminal: bool) -> Option<SummaryLine> {
    summary_line(&slot, terminal, UNIX_EPOCH + Duration::from_secs(NOW))
}

#[test]
fn hidden_draws_nothing_and_empty_offers_a_button() {
    assert_eq!(line(SummarySlot::Hidden, false), None);
    let l = line(SummarySlot::Empty, false).unwrap();
    assert_eq!((l.state, l.header.as_str(), l.clickable), ("none", "✦ 生成总结", true));
}

#[test]
fn ready_header_has_age_and_coverage() {
    let l = line(SummarySlot::View { view: sv(SumState::Ready, Some(("迁到 JWT", "在跑测试")), NOW - 180), stale: false, actionable: true }, false).unwrap();
    assert_eq!(l.state, "ready");
    assert_eq!(l.header, "✦ AI 总结 · 3 分钟前 · 覆盖第 2–3 轮");
    assert_eq!(l.goal.as_deref(), Some("迁到 JWT"));
    assert_eq!(l.recent.as_deref(), Some("在跑测试"));
    let fresh = line(SummarySlot::View { view: sv(SumState::Ready, Some(("g", "r")), NOW - 20), stale: true, actionable: true }, false).unwrap();
    assert_eq!(fresh.state, "stale");
    assert_eq!(fresh.header, "✦ AI 总结 · 刚刚 · 覆盖第 2–3 轮 · 有新进展");
}

#[test]
fn pending_with_and_without_an_old_summary() {
    let l = line(SummarySlot::View { view: sv(SumState::Pending, None, 0), stale: false, actionable: true }, false).unwrap();
    assert_eq!((l.state, l.header.as_str(), l.recent.clone()), ("pending", "✦ AI 总结 · 生成中…", None));
    let l = line(SummarySlot::View { view: sv(SumState::Pending, Some(("g", "r")), NOW - 120), stale: false, actionable: true }, false).unwrap();
    assert_eq!(l.header, "✦ AI 总结 · 更新中… · 上次 2 分钟前 · 覆盖第 2–3 轮");
    assert_eq!(l.recent.as_deref(), Some("r"));
}

#[test]
fn failures_offer_a_retry() {
    let l = line(SummarySlot::View { view: sv(SumState::Failed("超时".into()), None, 0), stale: false, actionable: true }, false).unwrap();
    assert_eq!((l.state, l.header.as_str(), l.clickable), ("failed", "✦ 总结失败：超时 · 重试", true));
    let l = line(SummarySlot::View { view: sv(SumState::Paused("未找到 claude".into()), Some(("g", "r")), NOW - 60), stale: false, actionable: true }, false).unwrap();
    assert_eq!(l.header, "✦ 已暂停自动总结：未找到 claude · 重试");
    assert_eq!(l.recent.as_deref(), Some("r"), "the old summary stays");
}

#[test]
fn terminals_have_no_goal() {
    let l = line(SummarySlot::View { view: sv(SumState::Ready, Some(("x", "r")), NOW), stale: false, actionable: true }, true).unwrap();
    assert_eq!(l.goal, None);
}

/// A single live agent session's model, with its card's summary slot set to `slot`.
fn build_one_agent_with(slot: SummarySlot) -> MonitorModel {
    let now = Instant::now();
    let s = session("run", Status::Thinking, now);
    let mut a = agent(&s);
    a.summary = slot;
    build(&[a], &[], &MonitorUi::default(), now, wall(), None)
}

#[test]
fn cards_carry_their_summary_line() {
    let model = build_one_agent_with(SummarySlot::Empty);
    let Card::Agent(a) = &model.groups[0].cards[0] else { panic!() };
    assert_eq!(a.summary.as_ref().map(|s| s.state), Some("none"));
    assert_eq!(model.groups[0].cards[0].summary().map(|s| s.header.as_str()), Some("✦ 生成总结"));
    assert_eq!(terminal_only(vec![]).summary, None, "Hidden draws no block");
}

/// An ended session's card with `slot`.
fn build_one_ended_with(slot: SummarySlot) -> MonitorModel {
    let now = Instant::now();
    let s = session("end", Status::Ended, now);
    let mut a = agent(&s);
    a.summary = slot;
    let ui = MonitorUi { ended_open: true, ..Default::default() };
    build(&[a], &[], &ui, now, wall(), None)
}

#[test]
fn an_ended_card_shows_its_saved_summary_without_buttons() {
    let view = sv(SumState::Ready, Some(("g", "r")), NOW - 60);
    let m = build_one_ended_with(summary_slot(Some(view), Some(1), false));
    let s = m.groups[0].cards[0].summary().unwrap();
    assert_eq!((s.state, s.recent.as_deref(), s.actionable, s.clickable), ("ready", Some("r"), false, false));
    // A failed view keeps its text, but its 重试 is not a button.
    let failed = summary_slot(Some(sv(SumState::Failed("超时".into()), Some(("g", "r")), NOW)), None, false);
    let l = line(failed, false).unwrap();
    assert_eq!((l.header.as_str(), l.clickable, l.actionable), ("✦ 总结失败：超时 · 重试", false, false));
}

#[test]
fn an_ended_card_without_a_saved_summary_draws_no_block() {
    assert_eq!(summary_slot(None, None, false), SummarySlot::Hidden);
    let m = build_one_ended_with(summary_slot(None, None, false));
    assert_eq!(m.groups[0].cards[0].summary(), None);
}

#[test]
fn a_requestable_card_is_actionable() {
    assert_eq!(summary_slot(None, Some(3), true), SummarySlot::Empty);
    let m = build_one_agent_with(summary_slot(Some(sv(SumState::Ready, Some(("g", "r")), NOW)), Some(1), true));
    let s = m.groups[0].cards[0].summary().unwrap();
    assert!(s.actionable);
    let SummarySlot::View { stale, .. } = summary_slot(Some(sv(SumState::Ready, Some(("g", "r")), NOW)), Some(2), true) else { panic!() };
    assert!(stale, "activity moved past the summary's");
}

#[test]
fn excluded_cards_are_not_askable() {
    let now = Instant::now();
    let s = session("a1", Status::Idle, now);
    let mut a = agent(&s);
    a.excluded = true;
    let mut t = term(4, Vec::new());
    t.excluded = true;
    let m = build(&[a], &[t], &MonitorUi::default(), now, wall(), None);
    let cards: Vec<&Card> = m.groups.iter().flat_map(|g| &g.cards).collect();
    assert!(!cards.is_empty() && cards.iter().all(|c| c.excluded()));
}

#[test]
fn ask_candidates_follow_the_wall_and_skip_excluded() {
    use crate::monitor::chat_view::candidates;
    let now = Instant::now();
    let s1 = session("a1", Status::Idle, now);
    let s2 = session("a2", Status::Idle, now);
    let a1 = agent(&s1);
    let mut a2 = agent(&s2);
    a2.excluded = true;
    let model = build(&[a1, a2], &[term(5, Vec::new())], &MonitorUi::default(), now, wall(), None);
    assert!(model.groups.iter().flat_map(|g| &g.cards).any(|c| c.excluded() && c.key().ends_with(":a2")), "the excluded card is on the wall");
    let keys: Vec<String> = candidates(&model).into_iter().map(|c| c.key).collect();
    assert_eq!(keys.len(), 2, "a1 and the terminal");
    assert_eq!(keys, model.groups.iter().flat_map(|g| &g.cards).filter(|c| !c.excluded()).map(|c| c.key()).collect::<Vec<_>>());
    assert!(!keys.iter().any(|k| k.ends_with(":a2")));
    assert!(candidates(&model).iter().all(|c| c.label.contains(" · ")), "name · location");
}

#[test]
fn session_links_skip_excluded_and_unknown_sessions() {
    use crate::monitor::chat_view::link_target;
    let now = Instant::now();
    let s1 = session("a1", Status::Idle, now);
    let s2 = session("a2", Status::Idle, now);
    let mut a2 = agent(&s2);
    a2.excluded = true;
    let model = build(&[agent(&s1), a2], &[term(5, Vec::new())], &MonitorUi::default(), now, wall(), None);
    let key = |suffix: &str| model.groups.iter().flat_map(|g| &g.cards).map(|c| c.key()).find(|k| k.ends_with(suffix)).unwrap();
    assert_eq!(link_target(&model, &key(":a1")), Some(s1.pane));
    assert_eq!(link_target(&model, &key(":5")), Some(Some(5)), "a terminal's pane");
    assert_eq!(link_target(&model, &key(":a2")), None, "excluded: plain text, no jump");
    assert_eq!(link_target(&model, "agent:claude:nope"), None);
}

#[test]
fn links_and_candidates_ignore_the_wall_filter() {
    use crate::monitor::chat_view::{candidates, link_target};
    let now = Instant::now();
    let s1 = session("a1", Status::Idle, now);
    let s2 = session("a2", Status::Idle, now);
    let wall_with = |ui: &MonitorUi| {
        let mut a2 = agent(&s2);
        a2.excluded = true;
        build(&[agent(&s1), a2], &[term(5, Vec::new())], ui, now, wall(), None)
    };
    let all = wall_with(&MonitorUi::default());
    let filtered = wall_with(&MonitorUi { filter: Filter::Only(Group::Terminals), ..MonitorUi::default() });
    assert_eq!(filtered.filter, Filter::Only(Group::Terminals));
    let a1_key = all.groups.iter().flat_map(|g| &g.cards).map(|c| c.key()).find(|k| k.ends_with(":a1")).unwrap();
    assert!(!filtered.groups.iter().flat_map(|g| &g.cards).any(|c| c.key() == a1_key), "a1 is not drawn under 终端");
    assert_eq!(link_target(&filtered, &a1_key), Some(s1.pane), "its link still jumps");
    assert_eq!(candidates(&filtered), candidates(&all), "the @ list is the whole wall");
    assert_eq!(candidates(&filtered).len(), 2, "a1 and the terminal; a2 is excluded");
}
