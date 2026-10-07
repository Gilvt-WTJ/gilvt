use super::*;
use gilvt_agent::{BindingConfidence, RuntimeRef, TerminalOwner};

const DAY: u64 = 24 * 3600;

fn at(days_ago: u64) -> SystemTime {
    now() - Duration::from_secs(days_ago * DAY)
}

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000)
}

fn entry(agent: AgentKind, id: &str, cwd: &str, prompt: &str, days_ago: u64) -> HistoryEntry {
    HistoryEntry {
        agent,
        session_id: id.into(),
        cwd: cwd.into(),
        transcript: format!("/Users/u/.claude/projects/p/{id}.jsonl").into(),
        first_prompt: prompt.into(),
        topic_prompt: String::new(),
        custom_title: None,
        ai_title: None,
        started: None,
        last_active: at(days_ago),
        turns: 3,
        model: None,
        size: 2_000_000,
    }
}

/// gilvt-lab (current) and acme_web_monorepo, newest first.
fn entries() -> Vec<HistoryEntry> {
    vec![
        entry(AgentKind::Claude, "a1", "/Users/u/gilvt-lab", "先用 Read 读 calc/calc.py，再运行 git status", 0),
        entry(AgentKind::Claude, "b2", "/Users/u/gilvt-lab/calc", "请运行 python3 -m unittest -v", 0),
        entry(AgentKind::Codex, "c3", "/Users/u/gilvt-lab", "Run `python3 -m unittest -v`, then append…", 1),
        entry(AgentKind::Claude, "d4", "/Users/u/acme_web_monorepo", "gilvt M3b 实现计划", 1),
        entry(AgentKind::Claude, "e5", "/Users/u/scratchpad", "spike: hooks capture", 17),
        entry(AgentKind::Claude, "f6", "/Users/u/gilvt-lab", "修复 divide 的整除问题", 8),
    ]
}

/// A project is the directory under /Users/u.
fn project(cwd: &Path) -> Project {
    let root: PathBuf = cwd.iter().take(4).collect();
    Project { name: root.file_name().unwrap().to_string_lossy().into(), root }
}

fn lab() -> Project {
    project(Path::new("/Users/u/gilvt-lab"))
}

struct Case {
    entries: Vec<HistoryEntry>,
    live: HashMap<SessionKey, LiveAt>,
    names: HashMap<SessionKey, String>,
    current: Option<Project>,
}

impl Case {
    fn new() -> Case {
        Case { entries: entries(), live: HashMap::new(), names: HashMap::new(), current: Some(lab()) }
    }

    fn list(&self, query: &str, filters: Filters) -> List {
        let names = |k: &SessionKey| self.names.get(k).cloned();
        let when = |t: SystemTime| format!("{}d", now().duration_since(t).unwrap().as_secs() / DAY);
        build(&Input {
            entries: &self.entries,
            live: &self.live,
            saved_name: &names,
            project: &project,
            current: self.current.as_ref(),
            query,
            filters,
            now: now(),
            when: &when,
            home: Some(Path::new("/Users/u")),
            dir_exists: &|_| true,
            branch: &|_| None,
            archived: &|_| false,
        })
    }
}

fn hist(id: &str, cwd: &str, days_ago: u64) -> HistoryEntry {
    entry(AgentKind::Claude, id, cwd, "p", days_ago)
}

/// Builds with `tweak` applied to a default `Input` (no live sessions, no renames, no current project,
/// home `/Users/u`, every directory exists, no branch known).
fn build_with(entries: &[HistoryEntry], filters: Filters, tweak: impl FnOnce(&mut Input)) -> List {
    build_in(entries, filters, None, tweak)
}

/// [`build_with`] with a current project.
fn build_in(entries: &[HistoryEntry], filters: Filters, current: Option<&Project>, tweak: impl FnOnce(&mut Input)) -> List {
    let live = HashMap::new();
    let names = |_: &SessionKey| None;
    let when = |t: SystemTime| format!("{}d", now().duration_since(t).unwrap().as_secs() / DAY);
    let mut input = Input {
        entries,
        live: &live,
        saved_name: &names,
        project: &project,
        current,
        query: "",
        filters,
        now: now(),
        when: &when,
        home: Some(Path::new("/Users/u")),
        dir_exists: &|_| true,
        branch: &|_| None,
        archived: &|_| false,
    };
    tweak(&mut input);
    build(&input)
}

fn key(agent: AgentKind, id: &str) -> SessionKey {
    (agent, id.to_string())
}

/// Headers as "# title", rows as their session id.
fn shown(list: &List) -> Vec<String> {
    list.lines
        .iter()
        .map(|l| match l {
            Line::Header(h) => format!("# {h}"),
            Line::Row(i) => list.rows[*i].key.1.clone(),
        })
        .collect()
}

const CURRENT: Filters = Filters { scope: Scope::Current, stale: false, archived: false };
const ALL: Filters = Filters { scope: Scope::All, stale: false, archived: false };

#[test]
fn the_current_project_by_default() {
    let list = Case::new().list("", CURRENT);
    // A subdirectory of the repository counts (b2 ran in gilvt-lab/calc).
    assert_eq!(shown(&list), ["# gilvt-lab · 当前项目", "a1", "b2", "c3", "f6"]);
    let r = &list.rows[0];
    assert_eq!((r.letter, r.title.as_str(), r.subtitle().as_str(), r.when.as_str()), ('C', "先用 Read 读 calc/calc.py，再运行 git status", "~/gilvt-lab · 3 轮", "0d"));
    assert_eq!(list.rows[2].letter, 'X');
    assert_eq!(list.line_of(0), Some(1));
    assert_eq!(list.rows.iter().map(|r| r.entry).collect::<Vec<_>>(), [0, 1, 2, 5]);
}

#[test]
fn all_projects_without_a_query_is_one_list_with_projects() {
    let list = Case::new().list("", ALL);
    assert_eq!(shown(&list), ["a1", "b2", "c3", "d4", "e5", "f6"]);
    // The project is no longer in the row text; the directory is.
    assert_eq!(list.rows[3].subtitle(), "~/acme_web_monorepo · 3 轮");
    assert_eq!(list.rows[0].subtitle(), "~/gilvt-lab · 3 轮");
}

#[test]
fn searching_all_projects_puts_the_current_one_first() {
    let list = Case::new().list("  UNITTEST ", ALL);
    assert_eq!(shown(&list), ["# gilvt-lab · 当前项目", "b2", "c3"]);
    let list = Case::new().list("gilvt", ALL);
    // Title, project name and cwd match; the current project's rows do not repeat its name.
    assert_eq!(shown(&list), ["# gilvt-lab · 当前项目", "a1", "b2", "c3", "f6", "# 其他项目", "d4"]);
    assert_eq!((list.rows[0].subtitle().as_str(), list.rows[4].subtitle().as_str()), ("~/gilvt-lab · 3 轮", "~/acme_web_monorepo · 3 轮"));
    // Only other projects match: no empty current section.
    assert_eq!(shown(&Case::new().list("spike", ALL)), ["# 其他项目", "e5"]);
    assert_eq!(shown(&Case::new().list("scratchpad", ALL)), ["# 其他项目", "e5"], "cwd");
}

#[test]
fn session_ids_match_by_prefix() {
    let mut case = Case::new();
    case.entries[3].session_id = "0199aa-d4".into();
    assert_eq!(shown(&case.list("0199", ALL)), ["# 其他项目", "0199aa-d4"]);
    assert!(case.list("aa-d4", ALL).rows.is_empty(), "not inside the id");
}

#[test]
fn renames_are_titles_and_searched() {
    let mut case = Case::new();
    case.names.insert(key(AgentKind::Claude, "f6"), "整除修复".into());
    let list = case.list("整除修复", ALL);
    assert_eq!(list.rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["整除修复"]);
    // The first prompt still matches after a rename.
    assert_eq!(shown(&case.list("divide", CURRENT)), ["# gilvt-lab · 当前项目", "f6"]);
    case.entries[0].first_prompt.clear();
    assert_eq!(case.list("", CURRENT).rows[0].title, "（无提示词）");
}

/// The session ids of a list, without the section headers ("# project").
fn ids(list: &List) -> Vec<String> {
    shown(list).into_iter().filter(|s| !s.starts_with('#')).collect()
}

#[test]
fn agent_titles_name_the_row_and_the_search_finds_titles_and_the_original_words() {
    let mut case = Case::new();
    case.entries[0].ai_title = Some("Calc 重构方案".into()); // a1: 先用 Read 读 calc/calc.py，…
    let list = case.list("", CURRENT);
    assert_eq!(list.rows.iter().find(|r| r.key.1 == "a1").unwrap().title, "Calc 重构方案");
    // The title finds it, and so do the words the session started with.
    assert_eq!(ids(&case.list("重构", ALL)), ["a1"]);
    assert_eq!(ids(&case.list("git status", ALL)), ["a1"]);
    // The user's own title in the agent beats the generated one; both stay searchable.
    case.entries[0].custom_title = Some("我的重构".into());
    assert_eq!(case.list("", CURRENT).rows.iter().find(|r| r.key.1 == "a1").unwrap().title, "我的重构");
    assert_eq!(ids(&case.list("calc 重构", ALL)), ["a1"]);
    assert_eq!(ids(&case.list("我的重构", ALL)), ["a1"]);
    // A rename in gilvt still wins.
    case.names.insert(key(AgentKind::Claude, "a1"), "手动名".into());
    assert_eq!(case.list("", CURRENT).rows.iter().find(|r| r.key.1 == "a1").unwrap().title, "手动名");
}

#[test]
fn a_session_started_with_an_answer_is_named_after_its_first_informative_prompt() {
    let mut case = Case::new();
    case.entries[5].first_prompt = "继续".into(); // f6
    case.entries[5].topic_prompt = "修复 divide 的整除问题。再补测试".into();
    let row_title = |case: &Case| case.list("", ALL).rows.iter().find(|r| r.key.1 == "f6").unwrap().title.clone();
    assert_eq!(row_title(&case), "修复 divide 的整除问题");
    // The words it really started with are still searchable.
    assert_eq!(ids(&case.list("继续", ALL)), ["f6"]);
    assert_eq!(ids(&case.list("整除问题", ALL)), ["f6"]);
}

#[test]
fn the_stale_filter_adds_sizes() {
    let stale = Filters { scope: Scope::All, stale: true, archived: false };
    let list = Case::new().list("", stale);
    assert_eq!(shown(&list), ["e5", "f6"]);
    assert_eq!(list.rows[0].when, "17d · 2.0 MB");
    let list = Case::new().list("", Filters { stale: true, ..CURRENT });
    assert_eq!(shown(&list), ["# gilvt-lab · 当前项目", "f6"]);
    // Exactly 7 days is stale.
    let mut case = Case::new();
    case.entries[0].last_active = at(7);
    assert!(is_stale(&case.entries[0], now()));
    case.entries[0].last_active = at(7) + Duration::from_secs(1);
    assert!(!is_stale(&case.entries[0], now()));
    assert!(!is_stale(&entry(AgentKind::Claude, "z", "/", "x", 0), now() - Duration::from_secs(DAY)), "future");
}

#[test]
fn without_a_current_project_there_is_only_all() {
    let mut case = Case::new();
    case.current = None;
    let list = case.list("", CURRENT);
    assert_eq!(list.rows.len(), 6);
    assert!(list.lines.iter().all(|l| matches!(l, Line::Row(_))));
    assert_eq!(shown(&case.list("gilvt", ALL)), ["a1", "b2", "c3", "d4", "f6"]);
    assert_eq!(effective_scope(CURRENT, None), Scope::All);
}

#[test]
fn live_rows_show_where_they_run() {
    let mut case = Case::new();
    case.live.insert(key(AgentKind::Claude, "b2"), LiveAt { pane: Some(7), location: "左上 pane".into(), runtime: None });
    let list = case.list("", Filters { stale: false, ..CURRENT });
    let r = &list.rows[1];
    assert_eq!(r.live.as_ref().map(|l| l.location.as_str()), Some("左上 pane"));
    assert_eq!(r.when, "");
    assert!(!r.selectable() && list.rows[0].selectable());
}

#[test]
fn pane_locations() {
    let here = |position| PaneAt { window: None, tab: 1, active_tab: true, position };
    assert_eq!(location_word(&here("左上")), "左上 pane");
    assert_eq!(location_word(&here("")), "当前 pane");
    assert_eq!(location_word(&PaneAt { tab: 2, active_tab: false, ..here("右") }), "标签 2 · 右 pane");
    assert_eq!(location_word(&PaneAt { tab: 3, active_tab: false, ..here("") }), "标签 3");
    assert_eq!(location_word(&PaneAt { window: Some(2), ..here("") }), "窗口 2 · 标签 1");
    assert_eq!(location_word(&PaneAt { window: Some(2), tab: 1, active_tab: true, position: "下" }), "窗口 2 · 标签 1 · 下 pane");
}

#[test]
fn messages_for_an_empty_list() {
    let lab = lab();
    assert_eq!(empty_message(0, true, "", CURRENT, Some(&lab)), "正在读取会话…");
    assert_eq!(empty_message(0, false, "x", CURRENT, Some(&lab)), "还没有 Claude / Codex 会话");
    assert_eq!(empty_message(4, false, "x", ALL, Some(&lab)), "无匹配");
    assert_eq!(empty_message(4, false, "", Filters { stale: true, ..ALL }, Some(&lab)), "没有 7 天未活动的会话");
    assert_eq!(empty_message(4, false, "", CURRENT, Some(&lab)), "gilvt-lab 还没有会话，看看「全部项目」");
    assert_eq!(empty_message(4, false, "", CURRENT, None), "没有会话");
}

#[test]
fn confirm_and_pick_copy() {
    assert_eq!(confirm_text(3, 41_000_000, 0), "3 个会话 · 41 MB 将移到废纸篓（可从废纸篓还原）");
    assert_eq!(picks_text(0, 0), "⇧ / ⌘ 点击多选");
    assert_eq!(picks_text(3, 41_200_000), "⇧ / ⌘ 点击多选 · 已选 3 个 · 共 41 MB");
}

#[test]
fn sizes_and_times() {
    assert_eq!(size_label(0), "0 MB");
    assert_eq!(size_label(40_000), "< 0.1 MB");
    assert_eq!(size_label(420_000), "0.4 MB");
    assert_eq!(size_label(9_940_000), "9.9 MB");
    assert_eq!(size_label(9_960_000), "10 MB");
    assert_eq!(size_label(18_300_000), "18 MB");
    assert_eq!(size_label(1_600_000_000), "1.6 GB");

    let t = |year, month, day, hour, minute| LocalTime { year, month, day, hour, minute, second: 0 };
    let now = t(2026, 9, 29, 10, 30);
    let m = |n: u64| Duration::from_secs(n * 60);
    assert_eq!(when_label(t(2026, 9, 29, 10, 30), now, m(0)), "刚刚");
    assert_eq!(when_label(t(2026, 9, 29, 10, 20), now, m(10)), "10 分钟前");
    assert_eq!(when_label(t(2026, 9, 29, 7, 29), now, m(181)), "3 小时前");
    assert_eq!(when_label(t(2026, 9, 28, 22, 41), now, m(709)), "昨天 22:41");
    assert_eq!(when_label(t(2026, 9, 21, 9, 5), now, m(8 * 1440)), "9 月 21 日");
    assert_eq!(when_label(t(2025, 12, 31, 9, 5), now, m(300 * 1440)), "2025 年 12 月 31 日");
    // Across midnight within the hour; across a year end the day before is still 昨天.
    assert_eq!(when_label(t(2026, 9, 28, 23, 55), t(2026, 9, 29, 0, 5), m(10)), "10 分钟前");
    assert_eq!(when_label(t(2025, 12, 31, 20, 0), t(2026, 1, 1, 9, 0), m(780)), "昨天 20:00");
    assert_eq!(when_label(t(2026, 2, 28, 20, 0), t(2026, 3, 1, 9, 0), m(780)), "昨天 20:00");
    assert_eq!(when_label(t(2024, 2, 29, 20, 0), t(2024, 3, 1, 9, 0), m(780)), "昨天 20:00", "leap day");
}

#[test]
fn clock_labels() {
    let t = |year, month, day, hour, minute, second| LocalTime { year, month, day, hour, minute, second };
    let now = t(2026, 9, 29, 10, 30, 0);
    assert_eq!(clock_label(t(2026, 9, 29, 9, 5, 7), now, true), "09:05:07");
    assert_eq!(clock_label(t(2026, 9, 29, 9, 5, 7), now, false), "09:05");
    // Other days: minutes only, with the day in front.
    assert_eq!(clock_label(t(2026, 9, 28, 22, 41, 3), now, true), "昨天 22:41");
    assert_eq!(clock_label(t(2026, 9, 21, 9, 5, 0), now, false), "9 月 21 日 09:05");
    assert_eq!(clock_label(t(2025, 12, 31, 20, 0, 0), t(2026, 1, 1, 9, 0, 0), false), "昨天 20:00");
    assert_eq!(clock_label(t(2025, 12, 30, 20, 0, 0), t(2026, 1, 1, 9, 0, 0), false), "2025 年 12 月 30 日 20:00");
    // A clock set back: a later moment than now still reads as today's time.
    assert_eq!(clock_label(t(2026, 9, 29, 11, 0, 0), now, false), "11:00");
}

fn m(shift: bool, platform: bool) -> Modifiers {
    Modifiers { shift, platform, ..Default::default() }
}

#[test]
fn keys() {
    let none = Modifiers::default();
    assert_eq!(command("enter", none), Some(Key::Enter(Location::Smart)));
    assert_eq!(command("enter", m(false, true)), Some(Key::Enter(Location::Right)));
    assert_eq!(command("enter", m(true, false)), Some(Key::Enter(Location::Smart)), "⇧↩ alone");
    assert_eq!(command("r", m(false, true)), Some(Key::Rename));
    assert_eq!(command("r", none), None, "text");
    assert_eq!(command("c", m(true, true)), Some(Key::CopyId));
    assert_eq!(command("c", m(false, true)), None, "⌘C");
    assert_eq!(command("backspace", m(false, true)), Some(Key::Trash));
    assert_eq!(command("backspace", none), Some(Key::Backspace));
    assert_eq!(command("up", none), Some(Key::Up));
    assert_eq!(command("n", Modifiers { control: true, ..Default::default() }), Some(Key::Down));
    assert_eq!(command("escape", none), Some(Key::Escape));
    assert_eq!(escape(true, true), Esc::Menu);
    assert_eq!(escape(false, true), Esc::Confirm);
    assert_eq!(escape(false, false), Esc::Palette);
}

#[test]
fn only_a_plain_enter_confirms_the_trash_bar() {
    let none = Modifiers::default();
    assert!(confirms_trash(&none, false, false));
    assert!(!confirms_trash(&none, true, false), "a held ↩ never confirms");
    assert!(!confirms_trash(&none, false, true), "a row menu is open");
    for m in [Modifiers::command(), Modifiers::shift(), Modifiers::command_shift(), Modifiers::alt(), Modifiers::control()] {
        assert!(!confirms_trash(&m, false, false), "{m:?}: like the sidebar bar, a modified ↩ is not a confirm");
    }
}

#[test]
fn menu_follows_the_mockup() {
    let labels: Vec<_> = menu_items(true, false).iter().map(|i| i.1).collect();
    assert_eq!(labels, ["恢复", "在右侧恢复", "重命名…", "复制会话 ID", "复制目录路径", "在访达中显示", "归档", "移到废纸篓…"]);
    assert!(menu_items(true, false).iter().all(|i| i.2));
    assert!(!menu_items(false, false)[7].2, "nothing to trash");
}

#[test]
fn rename_values() {
    assert_eq!(rename_initial(Some("整除修复"), "修复 divide"), "整除修复");
    assert_eq!(rename_initial(None, "修复 divide"), "修复 divide");
    assert_eq!(rename_value(" 修复 divide ", None, "修复 divide"), None, "untouched first prompt");
    assert_eq!(rename_value("", None, "修复 divide"), None);
    assert_eq!(rename_value("整除", None, "修复 divide").as_deref(), Some("整除"));
    assert_eq!(rename_value("整除", Some("整除"), "修复 divide"), None);
    assert_eq!(rename_value("", Some("整除"), "修复 divide").as_deref(), Some(""), "back to the first prompt");
}

#[test]
fn resume_outcomes() {
    let e = entry(AgentKind::Claude, "a1", "/Users/u/my lab", "x", 0);
    let all = |_: &Path| true;
    let run = resume(&e, None, "claude", all);
    assert_eq!(run, Resume::Run { dir: "/Users/u/my lab".into(), command: "cd '/Users/u/my lab' && claude --resume a1".into() });
    assert_eq!(run.toast(), None);
    let codex = entry(AgentKind::Codex, "0199", "/Users/u/lab", "x", 0);
    assert_eq!(resume(&codex, None, "codex-w", all), Resume::Run { dir: "/Users/u/lab".into(), command: "cd /Users/u/lab && codex-w resume 0199".into() });
    let live = LiveAt { pane: Some(4), location: "右 pane".into(), runtime: None };
    assert_eq!(resume(&e, Some(&live), "claude", all), Resume::Focus(4));
    let away = LiveAt { pane: None, location: String::new(), runtime: None };
    assert_eq!(resume(&e, Some(&away), "claude", all).toast().as_deref(), Some("该会话正在运行，但不在 gilvt 的窗口里"));
    let runtime = RuntimeRef { pid: 42, process_started_at: 7, pgid: Some(42), tty: Some("/dev/ttys001".into()), terminal: TerminalOwner::Terminal, terminal_pid: Some(3), confidence: BindingConfidence::Exact };
    let external = LiveAt { pane: None, location: runtime.location_label(), runtime: Some(runtime.clone()) };
    assert_eq!(resume(&e, Some(&external), "claude", all), Resume::Navigate(runtime));
    let gone = resume(&e, None, "claude", |p: &Path| p != e.transcript);
    assert_eq!((gone.clone(), gone.toast().as_deref()), (Resume::Gone, Some("该会话已不存在")));
    let no_dir = resume(&e, None, "claude", |p: &Path| p != e.cwd);
    assert_eq!(no_dir.toast().as_deref(), Some("会话目录已不存在：/Users/u/my lab"));
}

// Selection.

fn rows(live: &[&str]) -> Vec<Row> {
    let mut case = Case::new();
    for id in live {
        let agent = if *id == "c3" { AgentKind::Codex } else { AgentKind::Claude };
        case.live.insert(key(agent, id), LiveAt { pane: Some(1), location: String::new(), runtime: None });
    }
    case.list("", ALL).rows
}

fn picked(s: &Selection, rows: &[Row]) -> Vec<String> {
    s.picked(rows).into_iter().map(|i| rows[i].key.1.clone()).collect()
}

#[test]
fn cursor_moves_and_wraps() {
    let rows = rows(&[]);
    let mut s = Selection::default();
    s.step(&rows, false);
    assert_eq!(s.cursor(), 5);
    s.step(&rows, true);
    assert_eq!(s.cursor(), 0);
    s.step(&[], true);
    assert_eq!(s.cursor(), 0);
}

#[test]
fn command_click_toggles_and_skips_running_rows() {
    let rows = rows(&["b2"]);
    let mut s = Selection::default();
    s.click(&rows, 0, Click::Toggle);
    s.click(&rows, 1, Click::Toggle);
    s.click(&rows, 3, Click::Toggle);
    assert_eq!(picked(&s, &rows), ["a1", "d4"], "b2 is running");
    assert_eq!(s.cursor(), 3);
    s.click(&rows, 0, Click::Toggle);
    assert_eq!(picked(&s, &rows), ["d4"]);
    s.click(&rows, 2, Click::Plain);
    assert!(!s.picking(), "a plain click drops the picks");
    assert_eq!(s.cursor(), 2);
}

#[test]
fn shift_click_picks_a_range_from_the_anchor() {
    let rows = rows(&["c3"]);
    let mut s = Selection::default();
    s.click(&rows, 1, Click::Plain);
    s.click(&rows, 4, Click::Range);
    assert_eq!(picked(&s, &rows), ["b2", "d4", "e5"]);
    // Upwards from the same anchor.
    s.click(&rows, 0, Click::Range);
    assert_eq!(picked(&s, &rows), ["a1", "b2", "d4", "e5"]);
    assert_eq!(Click::from_modifiers(m(true, true)), Click::Range);
    assert_eq!(Click::from_modifiers(m(false, true)), Click::Toggle);
    assert_eq!(Click::from_modifiers(Modifiers::default()), Click::Plain);
}

#[test]
fn trash_targets() {
    let rows = rows(&["b2"]);
    let mut s = Selection::default();
    assert_eq!(s.targets(&rows, None), [0], "the cursor row");
    s.set_cursor(&rows, 1);
    assert!(s.targets(&rows, None).is_empty(), "running");
    s.click(&rows, 3, Click::Toggle);
    s.click(&rows, 5, Click::Toggle);
    assert_eq!(s.targets(&rows, None), [3, 5]);
    // The menu of a picked row acts on all picks; of another row, on that row only.
    assert_eq!(s.targets(&rows, Some(5)), [3, 5]);
    assert_eq!(s.targets(&rows, Some(4)), [4]);
    assert!(s.targets(&rows, Some(1)).is_empty());
    s.clear_picks();
    assert!(!s.picking());
}

#[test]
fn a_rebuilt_list_keeps_what_it_can() {
    let all = rows(&[]);
    let mut s = Selection::default();
    s.click(&all, 4, Click::Toggle);
    s.click(&all, 3, Click::Toggle);
    // A refresh: a1 and e5 are gone, d4 moved up.
    let mut fresh = all.clone();
    fresh.remove(0);
    fresh.remove(3);
    s.sync(&fresh, true);
    assert_eq!((s.cursor(), fresh[s.cursor()].key.1.as_str()), (2, "d4"), "the cursor follows its session");
    assert_eq!(picked(&s, &fresh), ["d4"]);
    // d4 runs now.
    let running = rows(&["d4"]);
    s.sync(&running, true);
    assert!(!s.picking(), "running rows are unpicked");
    // A new query: back to the first row.
    s.sync(&running, false);
    assert_eq!(s.cursor(), 0);
    s.set_cursor(&running, 99);
    assert_eq!(s.cursor(), 5, "clamped");
}

#[test]
fn confirm_text_names_the_companion_data_only_when_there_is_some() {
    assert_eq!(confirm_text(2, 2_200_000, 0), "2 个会话 · 2.2 MB 将移到废纸篓（可从废纸篓还原）");
    assert_eq!(
        confirm_text(2, 2_200_000, 400_000),
        "2 个会话 · 2.2 MB + 附属 0.4 MB 将移到废纸篓（可从废纸篓还原）"
    );
}

#[test]
fn every_row_names_its_directory_in_either_scope() {
    let entries = [hist("a", "/Users/u/Workplace/auth", 1), hist("b", "/Users/u/proj", 3)];
    let proj = project(Path::new("/Users/u/proj"));
    for scope in [Scope::Current, Scope::All] {
        let list = build_in(&entries, Filters { scope, ..Filters::default() }, Some(&proj), |_| {});
        assert!(!list.rows.is_empty());
        for row in &list.rows {
            assert!(!row.dir.is_empty(), "{scope:?}: {row:?}");
        }
    }
    let list = build_with(&entries, Filters { scope: Scope::All, ..Filters::default() }, |_| {});
    assert_eq!(list.rows[0].dir, "~/Workplace/auth");
    assert_eq!(list.rows[0].subtitle(), "~/Workplace/auth · 3 轮");
}

#[test]
fn branch_is_shown_when_known_and_a_missing_directory_is_flagged() {
    let entries = [hist("a", "/Users/u/gone", 2)];
    let list = build_with(&entries, Filters { scope: Scope::All, ..Filters::default() }, |i| {
        i.dir_exists = &|_| false;
        i.branch = &|_| Some("feature/payment".into());
    });
    let row = &list.rows[0];
    assert!(row.dir_missing);
    assert_eq!(row.subtitle(), "~/gone · feature/payment · 3 轮");
    let list = build_with(&entries, Filters { scope: Scope::All, ..Filters::default() }, |_| {});
    assert!(!list.rows[0].dir_missing);
}

#[test]
fn archived_sessions_are_hidden_by_default_and_the_only_ones_under_the_archived_filter() {
    let entries = [hist("a", "/Users/u/p", 1), hist("b", "/Users/u/p", 1)];
    let all = Filters { scope: Scope::All, ..Filters::default() };
    let list = build_with(&entries, all, |i| i.archived = &|e| e.session_id == "b");
    assert_eq!(list.rows.iter().map(|r| r.key.1.as_str()).collect::<Vec<_>>(), ["a"]);
    let only = Filters { archived: true, ..all };
    let list = build_with(&entries, only, |i| i.archived = &|e| e.session_id == "b");
    assert_eq!(list.rows.iter().map(|r| r.key.1.as_str()).collect::<Vec<_>>(), ["b"]);
    assert!(list.rows[0].archived);
}

#[test]
fn searching_does_not_reach_archived_sessions_unless_the_filter_is_on() {
    let entries = [hist("needle", "/Users/u/p", 1)];
    let run = |f: Filters| build_with(&entries, f, |i| {
        i.archived = &|_| true;
        i.query = "needle";
    });
    assert!(run(Filters { scope: Scope::All, ..Default::default() }).rows.is_empty());
    assert_eq!(run(Filters { scope: Scope::All, archived: true, ..Default::default() }).rows.len(), 1);
}

#[test]
fn cmd_e_archives_and_the_menu_names_the_action() {
    let m = Modifiers { platform: true, ..Default::default() };
    assert_eq!(command("e", m), Some(Key::Archive));
    assert_eq!(command("e", Modifiers::default()), None);
    let items = menu_items(true, false);
    assert_eq!(items.iter().find(|(i, ..)| *i == MenuItem::Archive).unwrap().1, "归档");
    assert_eq!(menu_items(true, true).iter().find(|(i, ..)| *i == MenuItem::Archive).unwrap().1, "取消归档");
    assert!(items.iter().any(|(i, l, _)| *i == MenuItem::CopyDir && *l == "复制目录路径"));
    assert_eq!(items.last().unwrap().0, MenuItem::Trash, "删除仍在最后");
}

#[test]
fn cmd_shift_k_opens_the_cleanup_wizard() {
    let cmd_shift = Modifiers { platform: true, shift: true, ..Default::default() };
    assert_eq!(command("k", cmd_shift), Some(Key::Cleanup));
    assert_eq!(command("k", Modifiers { platform: true, ..Default::default() }), None);
    assert_eq!(command("k", Modifiers::default()), None);
}

#[test]
fn the_empty_message_under_the_archived_filter() {
    let f = Filters { archived: true, ..Default::default() };
    assert_eq!(empty_message(5, false, "", f, None), "没有已归档的会话");
}

#[test]
fn only_a_past_session_is_archivable() {
    let entries = [hist("a", "/Users/u/p", 1)];
    let mut row = build_with(&entries, Filters { scope: Scope::All, ..Filters::default() }, |_| {}).rows.remove(0);
    assert!(row.archivable());
    row.live = Some(LiveAt { pane: None, location: String::new(), runtime: None });
    assert!(!row.archivable());
}

#[test]
fn a_running_session_still_archived_shows_in_the_default_list_and_not_under_the_archived_filter() {
    // Resumed from 已归档: it runs again, so it belongs with the running rows (⌘E refuses a running one).
    let entries = [hist("a", "/Users/u/p", 1), hist("b", "/Users/u/p", 1)];
    let mut live = HashMap::new();
    live.insert(
        key(AgentKind::Claude, "b"),
        LiveAt {
            pane: Some(3),
            location: String::new(),
            runtime: None,
        },
    );
    let names = |_: &SessionKey| None;
    let when = |_: SystemTime| String::new();
    let list = |filters: Filters| {
        build(&Input {
            entries: &entries,
            live: &live,
            saved_name: &names,
            project: &project,
            current: None,
            query: "",
            filters,
            now: now(),
            when: &when,
            home: Some(Path::new("/Users/u")),
            dir_exists: &|_| true,
            branch: &|_| None,
            archived: &|_| true,
        })
    };
    let all = Filters {
        scope: Scope::All,
        ..Filters::default()
    };
    let default = list(all);
    assert_eq!(
        default
            .rows
            .iter()
            .map(|r| r.key.1.as_str())
            .collect::<Vec<_>>(),
        ["b"]
    );
    assert!(default.rows[0].live.is_some());
    assert!(!default.rows[0].archived);
    let archived = list(Filters {
        archived: true,
        ..all
    });
    assert_eq!(
        archived
            .rows
            .iter()
            .map(|r| r.key.1.as_str())
            .collect::<Vec<_>>(),
        ["a"]
    );
}

#[test]
fn the_confirm_bar_says_the_companion_data_is_being_sized() {
    assert_eq!(
        confirm_text_sizing(2, 2_200_000),
        "2 个会话 · 2.2 MB + 附属 计算中… 将移到废纸篓（可从废纸篓还原）"
    );
}

#[test]
fn the_confirm_bar_opens_at_once_and_takes_only_its_own_companion_size() {
    use crate::launcher::sessions_view::Confirm;
    let entries = vec![hist("a", "/Users/u/p", 1), hist("b", "/Users/u/p", 1)];
    let mut first = Confirm::new(entries.clone());
    assert_eq!(first.text, confirm_text_sizing(2, 4_000_000));
    assert!(first.sizing());
    // A result for another bar (cancelled, replaced) changes nothing.
    let second = Confirm::new(entries.clone());
    assert_ne!(first.id, second.id);
    assert!(!first.companion_sized(second.id, 9_000_000));
    assert_eq!(first.text, confirm_text_sizing(2, 4_000_000));
    assert!(first.companion_sized(first.id, 400_000));
    assert_eq!(first.text, confirm_text(2, 4_000_000, 400_000));
    assert!(first.text.contains("+ 附属 0.4 MB"));
    assert!(!first.sizing());
    // Sized already (the cleanup wizard): final at once.
    let known = Confirm::with_companion(entries, 0);
    assert_eq!(known.text, confirm_text(2, 4_000_000, 0));
    assert!(!known.sizing());
}
