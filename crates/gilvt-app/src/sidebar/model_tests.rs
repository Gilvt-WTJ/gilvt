use super::*;

fn session(agent: AgentKind, id: &str, status: Status, t0: Instant) -> Session {
    let mut s = Session::new((agent, id.to_string()), t0);
    s.status = status;
    s.name = format!("task {id}");
    s.pane = Some(id.len() as u64);
    s
}

fn item<'a>(s: &'a Session, project: &str) -> Item<'a> {
    Item { session: s, project: project.into(), location: format!("{project} · 左"), current: false, reachable: true, git: None, cwd: String::new(), agent_title: None, archived: false }
}

fn clock(_: Instant) -> String {
    "12:40".into()
}

fn keys(rows: &[Row]) -> Vec<&str> {
    rows.iter().map(|r| r.key.as_ref().unwrap().1.as_str()).collect()
}

#[test]
fn needs_you_first_longest_wait_first() {
    let t0 = Instant::now();
    let now = t0 + Duration::from_secs(300);
    let mut a = session(AgentKind::Claude, "a", Status::Asking { question: "要不要同时更新 README？".into() }, t0);
    a.waiting_since = Some(t0 + Duration::from_secs(260));
    let mut b = session(AgentKind::Codex, "b", Status::NeedsApproval { action: "Bash(rm -rf build/)".into() }, t0);
    b.waiting_since = Some(t0 + Duration::from_secs(180));
    let c = session(AgentKind::Claude, "c", Status::Tool { label: "Bash(go test ./...)".into() }, t0);
    let items = [item(&a, "api"), item(&b, "repo"), item(&c, "repo")];
    let m = build(&items, &[], &SidebarState::default(), now, &clock);
    assert_eq!(m.live, 3);
    let need = &m.sections[0];
    assert_eq!((need.kind, need.title.as_str()), (SectionKind::NeedsYou, "需要你 · 2"));
    assert_eq!(keys(&need.rows), ["b", "a"]);
    assert!(need.rows.iter().all(|r| r.needs_you && r.tone == Tone::Waiting));
    assert_eq!(need.rows[0].status, "⏳ 等待审批 · Bash(rm -rf build/)");
    assert_eq!(need.rows[0].time, "2 分钟");
    assert_eq!(need.rows[1].status, "? 在问你 · 要不要同时更新 README？");
    assert_eq!(need.rows[1].time, "40 秒");
    assert_eq!((need.rows[0].letter, need.rows[1].letter), ('X', 'C'));
    // By project, in first-seen order; waiting sessions are listed in their group too.
    let groups: Vec<_> = m.sections[1..].iter().map(|s| (s.title.as_str(), keys(&s.rows))).collect();
    assert_eq!(groups, [("api", vec!["a"]), ("repo", vec!["b", "c"])]);
    let running = &m.sections[2].rows[1];
    assert_eq!((running.tone, running.status.as_str(), running.time.as_str()), (Tone::Running, "● 执行中 · Bash(go test ./...)", "执行中"));
    let order: Vec<_> = session_order(&m).into_iter().map(|t| t.0).collect();
    assert_eq!(order, vec![a.key.clone(), b.key.clone(), c.key.clone()]);
}

#[test]
fn idle_groups_collapse_by_default_until_toggled() {
    let t0 = Instant::now();
    let idle = session(AgentKind::Claude, "i", Status::Idle, t0);
    let mut done = session(AgentKind::Claude, "d", Status::Idle, t0);
    done.unseen_done = true;
    done.last_turn = Some(Duration::from_secs(250));
    let items = [item(&idle, "gilvt"), item(&done, "repo")];
    let mut view = SidebarState::default();
    let m = build(&items, &[], &view, t0, &clock);
    let gilvt = &m.sections[0];
    assert!(gilvt.collapsed);
    assert_eq!(gilvt.note, "1 个空闲，已折叠");
    assert_eq!(gilvt.rows.len(), 1, "rows are kept for switching");
    assert_eq!(gilvt.rows[0].status, "空闲 · 等你输入");
    assert_eq!(gilvt.rows[0].time, "12:40");
    let repo = &m.sections[1];
    assert!(!repo.collapsed, "a group with 完成未看 stays open");
    assert_eq!((repo.rows[0].tone, repo.rows[0].status.as_str()), (Tone::Done, "✓ 完成未看 · 用时 4 分钟"));
    view.toggle("project:gilvt", true);
    assert!(!build(&items, &[], &view, t0, &clock).sections[0].collapsed);
    view.toggle("project:repo", false);
    let m = build(&items, &[], &view, t0, &clock);
    assert!(m.sections[1].collapsed);
    assert_eq!(m.sections[1].note, "1 个会话，已折叠");
    // The default changes (the group became idle): the toggle no longer applies.
    assert!(view.collapsed("project:repo", true));
}

#[test]
fn grouping_by_status_and_ended_sessions() {
    let t0 = Instant::now();
    let err = session(AgentKind::Codex, "e", Status::Error { message: "stream disconnected\nmore".into() }, t0);
    let think = session(AgentKind::Claude, "t", Status::Thinking, t0);
    let idle = session(AgentKind::Claude, "i", Status::Idle, t0);
    let mut gone = session(AgentKind::Codex, "g", Status::Ended, t0 + Duration::from_secs(1));
    gone.context = Some((900, Some(1000)));
    let old = session(AgentKind::Codex, "o", Status::Ended, t0);
    let mut view = SidebarState::new(false, Grouping::Status);
    let items = [item(&idle, "p"), item(&think, "p"), item(&err, "p"), item(&old, "p"), item(&gone, "p")];
    let m = build(&items, &[], &view, t0, &clock);
    let titles: Vec<_> = m.sections.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, ["出错", "执行中", "空闲", "已结束 · 2"]);
    assert_eq!(m.sections[0].rows[0].status, "✕ 出错 · stream disconnected");
    assert_eq!(m.sections[0].rows[0].tone, Tone::Error);
    assert_eq!(m.sections[1].rows[0].status, "● 思考中");
    assert!(m.sections[2].collapsed);
    let ended = &m.sections[3];
    assert!(ended.collapsed);
    assert_eq!(keys(&ended.rows), ["g", "o"], "most recently ended first");
    assert!(ended.rows.iter().all(|r| r.ended && r.location.is_empty() && r.context.is_none() && r.status == "已结束"));
    view.ended_open = true;
    assert!(!build(&items, &[], &view, t0, &clock).sections[3].collapsed);
    assert_eq!(session_order(&m).len(), 3, "ended sessions are not switched to");
}

#[test]
fn markers_and_context_meter() {
    let t0 = Instant::now();
    let mut s = session(AgentKind::Claude, "a", Status::Thinking, t0);
    s.lite = true;
    s.muted = true;
    s.background_tasks = 2;
    s.context = Some((170_000, None));
    let mut it = item(&s, "p");
    it.current = true;
    let r = &build(&[it], &[], &SidebarState::default(), t0, &clock).sections[0].rows[0];
    assert!(r.lite && r.muted && r.current);
    assert_eq!(r.status, "● 后台任务运行中 · 2 个后台任务");
    assert_eq!(r.context, Some((0.85, Meter::Warn)));
    s.context = Some((950, Some(1000)));
    s.name.clear();
    let r = &build(&[item(&s, "p")], &[], &SidebarState::default(), t0, &clock).sections[0].rows[0];
    assert_eq!(r.context.map(|c| c.1), Some(Meter::Full));
    assert_eq!(r.name, "新会话");
    s.context = Some((100, Some(1000)));
    let r = &build(&[item(&s, "p")], &[], &SidebarState::default(), t0, &clock).sections[0].rows[0];
    assert_eq!(r.context.map(|c| c.1), Some(Meter::Normal));
}

#[test]
fn labels() {
    assert_eq!(duration_label(Duration::from_secs(59)), "59 秒");
    assert_eq!(duration_label(Duration::from_secs(60)), "1 分钟");
    assert_eq!(duration_label(Duration::from_secs(7300)), "2 小时");
    assert_eq!(location_text("api-server", "", None), "api-server");
    assert_eq!(location_text("repo", "左", None), "repo · 左");
    assert_eq!(location_text("repo", "", Some("窗口 2")), "repo · 窗口 2");
    assert_eq!(location_text("repo", "下", Some("窗口 2")), "repo · 下 · 窗口 2");
}

fn target(s: &Session) -> Target {
    (s.key.clone(), s.pane.unwrap())
}

#[test]
fn next_needs_you_cycles_by_wait() {
    let t0 = Instant::now();
    let now = t0 + Duration::from_secs(60);
    let wait = |id: &str, secs: u64| {
        let mut s = session(AgentKind::Claude, id, Status::NeedsApproval { action: String::new() }, t0);
        s.waiting_since = Some(t0 + Duration::from_secs(secs));
        s
    };
    let (a, bb, c) = (wait("a", 30), wait("bb", 10), session(AgentKind::Claude, "ccc", Status::Idle, t0));
    let model = |sessions: &[&Session]| {
        let items: Vec<Item> = sessions.iter().map(|s| item(s, "p")).collect();
        build(&items, &[], &SidebarState::default(), now, &clock)
    };
    let m = model(&[&a, &bb, &c]);
    assert_eq!(next_needs_you(&m, None), Some(target(&bb)), "longest wait first");
    assert_eq!(next_needs_you(&m, Some(&c.key)), Some(target(&bb)), "current not waiting");
    assert_eq!(next_needs_you(&m, Some(&bb.key)), Some(target(&a)));
    assert_eq!(next_needs_you(&m, Some(&a.key)), Some(target(&bb)), "wraps");
    assert_eq!(next_needs_you(&model(&[&c]), None), None);
    assert_eq!(next_needs_you(&model(&[]), None), None, "no sessions");
    assert_eq!(next_needs_you(&model(&[&a]), Some(&a.key)), Some(target(&a)), "the only one stays");
    let mut ended = wait("x", 0);
    ended.status = Status::Ended;
    assert_eq!(next_needs_you(&model(&[&ended]), None), None);
}

#[test]
fn switching_skips_sessions_without_a_pane() {
    let t0 = Instant::now();
    let a = session(AgentKind::Claude, "a", Status::Thinking, t0);
    let mut gone = session(AgentKind::Claude, "gg", Status::NeedsApproval { action: String::new() }, t0);
    gone.waiting_since = Some(t0);
    let mut unbound = session(AgentKind::Codex, "u", Status::Idle, t0);
    unbound.pane = None;
    let mut closed = item(&gone, "p");
    closed.reachable = false;
    let items = [item(&a, "p"), closed, item(&unbound, "p")];
    let m = build(&items, &[], &SidebarState::default(), t0, &clock);
    assert_eq!(m.sections[0].rows[0].pane, None, "listed, but not clickable");
    assert_eq!(session_order(&m), vec![target(&a)]);
    assert_eq!(next_needs_you(&m, None), None);
}

#[test]
fn session_order_spans_windows_in_sidebar_order() {
    // Sessions of two windows, registry order a (window 1), b (window 2), c (window 1), d (window 2);
    // grouped by project: repo = [a, c], api = [b, d] — the same order in every window.
    let t0 = Instant::now();
    let a = session(AgentKind::Claude, "a", Status::Thinking, t0);
    let b = session(AgentKind::Codex, "bb", Status::Thinking, t0);
    let c = session(AgentKind::Claude, "ccc", Status::Thinking, t0);
    let d = session(AgentKind::Codex, "dddd", Status::Thinking, t0);
    let mut items = [item(&a, "repo"), item(&b, "api"), item(&c, "repo"), item(&d, "api")];
    items[1].location = "api · 窗口 2".into();
    items[3].location = "api · 窗口 2".into();
    items[2].current = true;
    let m = build(&items, &[], &SidebarState::default(), t0, &clock);
    let order = session_order(&m);
    assert_eq!(order, vec![target(&a), target(&c), target(&b), target(&d)]);
    assert_eq!(neighbor(&order, Some(&c.key), Step::Next), Some(target(&b)), "into the other window");
    assert_eq!(neighbor(&order, Some(&a.key), Step::Prev), Some(target(&d)), "wraps to the last");
    // By status the order follows the status groups.
    let mut view = SidebarState::new(false, Grouping::Status);
    let err = session(AgentKind::Codex, "eeeee", Status::Error { message: String::new() }, t0);
    let items = [item(&a, "repo"), item(&err, "api")];
    let m = build(&items, &[], &view, t0, &clock);
    assert_eq!(session_order(&m), vec![target(&err), target(&a)]);
    view.grouping = Grouping::Project;
    assert_eq!(session_order(&build(&items, &[], &view, t0, &clock)), vec![target(&a), target(&err)]);
}

#[test]
fn neighbor_wraps_and_starts_at_the_ends() {
    let k = |id: &str, pane: u64| ((AgentKind::Codex, id.to_string()), pane);
    let key = |id: &str| (AgentKind::Codex, id.to_string());
    let rows = [k("a", 1), k("b", 2), k("c", 3)];
    assert_eq!(neighbor(&rows, Some(&key("a")), Step::Next), Some(k("b", 2)));
    assert_eq!(neighbor(&rows, Some(&key("c")), Step::Next), Some(k("a", 1)));
    assert_eq!(neighbor(&rows, Some(&key("a")), Step::Prev), Some(k("c", 3)));
    assert_eq!(neighbor(&rows, None, Step::Next), Some(k("a", 1)));
    assert_eq!(neighbor(&rows, Some(&key("zz")), Step::Prev), Some(k("c", 3)), "current not in the list");
    assert_eq!(neighbor(&rows, Some(&key("zz")), Step::Next), Some(k("a", 1)));
    assert_eq!(neighbor(&rows[..1], Some(&key("a")), Step::Next), Some(k("a", 1)), "single session");
    assert_eq!(neighbor(&rows[..1], Some(&key("a")), Step::Prev), Some(k("a", 1)));
    assert_eq!(neighbor(&[], None, Step::Next), None);
    assert_eq!(neighbor(&[], Some(&key("a")), Step::Prev), None);
}

#[test]
fn visible_rows_skip_collapsed_sections_but_keep_the_rename_row() {
    let t0 = Instant::now();
    let mut a = session(AgentKind::Claude, "a", Status::Asking { question: "?".into() }, t0);
    a.waiting_since = Some(t0);
    let idle = session(AgentKind::Claude, "i", Status::Idle, t0);
    let items = [item(&a, "api"), item(&idle, "gilvt")];
    let m = build(&items, &[], &SidebarState::default(), t0, &clock);
    let shown = |renaming: Option<&SessionKey>| -> Vec<(usize, String)> {
        visible_rows(&m, renaming).into_iter().map(|(s, r)| (s, r.key.as_ref().unwrap().1.clone())).collect()
    };
    // 需要你 · a, api · a (the duplicate), gilvt collapsed (all idle).
    assert_eq!(shown(None), [(0, "a".to_string()), (1, "a".to_string())]);
    // Renaming the collapsed session shows its row; renaming a duplicated one changes nothing.
    assert_eq!(shown(Some(&idle.key)), [(0, "a".to_string()), (1, "a".to_string()), (2, "i".to_string())]);
    assert_eq!(shown(Some(&a.key)).len(), 2);
    // `all_rows` lists every row with whether it is drawn; `visible_rows` is its drawn part.
    let all = |renaming: Option<&SessionKey>| -> Vec<(usize, String, bool)> {
        all_rows(&m, renaming).into_iter().map(|(s, r, drawn)| (s, r.key.as_ref().unwrap().1.clone(), drawn)).collect()
    };
    assert_eq!(all(None), [(0, "a".to_string(), true), (1, "a".to_string(), true), (2, "i".to_string(), false)]);
    assert_eq!(all(Some(&idle.key))[2], (2, "i".to_string(), true));
}

#[test]
fn pending_rows_are_muted_and_say_待恢复() {
    use crate::agents::PendingResume;
    let p = PendingResume { key: (AgentKind::Codex, "s9".into()), pane: 4, cwd: None, name: "写迁移".into(), last_status: "空闲".into() };
    let rows = pending_rows(&[p]);
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!((r.letter, r.name.as_str(), r.status.as_str()), ('X', "写迁移", "待恢复"));
    assert_eq!(r.tone, Tone::Muted);
    assert_eq!(r.pane, Some(4));
    assert!(!r.ended && !r.needs_you);
}

#[test]
fn pending_section_sits_before_the_groups_and_is_not_in_the_switching_order() {
    use crate::agents::PendingResume;
    let t0 = Instant::now();
    let mut a = session(AgentKind::Claude, "a", Status::Asking { question: "?".into() }, t0);
    a.waiting_since = Some(t0);
    let b = session(AgentKind::Claude, "b", Status::Idle, t0);
    let items = [item(&a, "api"), item(&b, "repo")];
    let p = PendingResume { key: (AgentKind::Claude, "p".into()), pane: 9, cwd: None, name: "".into(), last_status: String::new() };
    let m = with_pending(build(&items, &[], &SidebarState::default(), t0, &clock), &[p]);
    let kinds: Vec<_> = m.sections.iter().map(|s| s.kind).collect();
    assert_eq!(kinds, [SectionKind::NeedsYou, SectionKind::Pending, SectionKind::Group, SectionKind::Group]);
    assert_eq!(m.sections[1].title, "待恢复 · 1");
    assert_eq!(m.sections[1].rows[0].name, "（未命名）");
    assert_eq!(m.live, 2, "pending sessions are not live");
    let order: Vec<_> = session_order(&m).into_iter().map(|t| t.0 .1).collect();
    assert_eq!(order, ["a", "b"]);
    assert_eq!(next_needs_you(&m, None).map(|t| t.0 .1), Some("a".to_string()));
    // No pending: the model is unchanged.
    let plain = build(&items, &[], &SidebarState::default(), t0, &clock);
    assert_eq!(with_pending(plain.clone(), &[]), plain);
}

#[test]
fn a_row_is_named_with_the_agents_title_unless_the_user_renamed_it() {
    let t0 = Instant::now();
    let mut s = session(AgentKind::Claude, "a", Status::Idle, t0); // name: "task a" (from its first prompt)
    let name_of = |it: &Item| build(std::slice::from_ref(it), &[], &SidebarState::default(), t0, &clock).sections[0].rows[0].name.clone();
    assert_eq!(name_of(&item(&s, "api")), "task a", "no agent title: the name it has");
    let titled = Item { agent_title: Some("整理文档".into()), ..item(&s, "api") };
    assert_eq!(name_of(&titled), "整理文档");
    // A rename in gilvt wins over the agent's title.
    s.name = "我的名字".into();
    s.renamed = true;
    let titled = Item { agent_title: Some("整理文档".into()), ..item(&s, "api") };
    assert_eq!(name_of(&titled), "我的名字");
}

#[test]
fn row_carries_the_git_line_and_it_is_empty_without_git() {
    let t0 = Instant::now();
    let s = session(AgentKind::Claude, "a", Status::Idle, t0);
    let with = Item { git: Some("⎇ main ●2".into()), ..item(&s, "api") };
    let without = item(&s, "api");
    let git_of = |it: &Item| build(std::slice::from_ref(it), &[], &SidebarState::default(), t0, &clock).sections[0].rows[0].git.clone();
    assert_eq!(git_of(&with), "⎇ main ●2");
    assert_eq!(git_of(&without), "");
}

fn terminal(pane: u64, name: &str, project: &str) -> TerminalItem {
    TerminalItem {
        pane,
        name: name.into(),
        project: project.into(),
        location: format!("{project} · 左"),
        git: None,
        cwd: format!("/work/{project}"),
        current: false,
    }
}

#[test]
fn terminals_follow_agents_in_their_project_group() {
    let t0 = Instant::now();
    let a = session(AgentKind::Claude, "a", Status::Tool { label: "Bash(ls)".into() }, t0);
    let items = [item(&a, "repo")];
    let terms = [terminal(7, "zsh", "repo"), terminal(8, "vim", "other")];
    let m = build(&items, &terms, &SidebarState::default(), t0, &clock);
    let groups: Vec<_> = m.sections.iter().map(|s| (s.title.as_str(), s.rows.iter().map(|r| r.kind).collect::<Vec<_>>())).collect();
    assert_eq!(groups, [("repo", vec![RowKind::Agent, RowKind::Terminal]), ("other", vec![RowKind::Terminal])]);
    let t = &m.sections[0].rows[1];
    assert_eq!((t.key.clone(), t.pane, t.name.as_str(), t.status.as_str()), (None, Some(7), "zsh", ""));
    assert_eq!((m.live, m.terminals), (1, 2));
}

#[test]
fn a_project_with_only_terminals_is_expanded() {
    let t0 = Instant::now();
    let m = build(&[], &[terminal(8, "zsh", "solo")], &SidebarState::default(), t0, &clock);
    let s = &m.sections[0];
    assert_eq!((s.title.as_str(), s.collapsed, s.default_collapsed), ("solo", false, false));
    assert_eq!((m.live, m.terminals), (0, 1));
}

#[test]
fn terminals_never_need_you_and_never_join_switching() {
    let t0 = Instant::now();
    let now = t0 + Duration::from_secs(60);
    let mut a = session(AgentKind::Claude, "a", Status::NeedsApproval { action: "Bash(ls)".into() }, t0);
    a.waiting_since = Some(t0);
    let items = [item(&a, "repo")];
    let m = build(&items, &[terminal(7, "zsh", "repo")], &SidebarState::default(), now, &clock);
    assert_eq!(m.sections[0].kind, SectionKind::NeedsYou);
    assert_eq!(m.sections[0].rows.len(), 1);
    let order: Vec<_> = session_order(&m).into_iter().map(|t| t.1).collect();
    assert_eq!(order, vec![a.pane.unwrap()], "only the agent is a keyboard-switching target");
    assert_eq!(next_needs_you(&m, None).map(|t| t.0), Some(a.key.clone()));
}

#[test]
fn by_status_terminals_form_a_collapsed_last_group() {
    let t0 = Instant::now();
    let a = session(AgentKind::Claude, "a", Status::Tool { label: "Bash(ls)".into() }, t0);
    let items = [item(&a, "repo")];
    let mut view = SidebarState::new(false, Grouping::Status);
    let terms = [terminal(7, "zsh", "repo"), terminal(8, "vim", "repo")];
    let m = build(&items, &terms, &view, t0, &clock);
    let last = m.sections.last().unwrap();
    assert_eq!((last.kind, last.id.as_str(), last.title.as_str()), (SectionKind::Terminals, "terminals", "终端 · 2"));
    assert_eq!((last.collapsed, last.note.as_str(), last.rows.len()), (true, "2 个，已折叠", 2));
    view.toggle("terminals", true);
    assert!(!build(&items, &terms, &view, t0, &clock).sections.last().unwrap().collapsed);
    // Without terminals there is no such group, by project or by status.
    let none = build(&items, &[], &view, t0, &clock);
    assert!(none.sections.iter().all(|s| s.kind != SectionKind::Terminals));
}

#[test]
fn header_counts_terminals_only_when_there_are_some() {
    let t0 = Instant::now();
    let a = session(AgentKind::Claude, "a", Status::Idle, t0);
    let items = [item(&a, "repo")];
    let with = build(&items, &[terminal(7, "zsh", "repo")], &SidebarState::default(), t0, &clock);
    assert_eq!(header_text(&with), "会话 · 1 · 终端 1");
    let without = build(&items, &[], &SidebarState::default(), t0, &clock);
    assert_eq!(header_text(&without), "会话 · 1");
    assert_eq!(header_text(&build(&[], &[], &SidebarState::default(), t0, &clock)), "会话 · 0");
}

#[test]
fn terminal_names_are_one_line_and_never_empty() {
    assert_eq!(terminal_name("  vim\nfoo.rs  "), "vim foo.rs");
    assert_eq!(terminal_name("zsh"), "zsh");
    assert_eq!(terminal_name(""), "终端");
    assert_eq!(terminal_name(" \n\t "), "终端");
}

#[test]
fn a_terminal_without_a_directory_goes_to_the_terminal_project() {
    assert_eq!(terminal_project(Some("repo".into())), "repo");
    assert_eq!(terminal_project(None), "终端");
}

#[test]
fn tooltip_lines_skip_empty_fields() {
    let t0 = Instant::now();
    let m = build(&[], &[terminal(7, "zsh", "repo")], &SidebarState::default(), t0, &clock);
    let lines = tooltip_lines(&m.sections[0].rows[0]);
    assert_eq!(lines, ["zsh", "repo · 左", "/work/repo"], "a terminal row has no status line and no git here");
    let a = session(AgentKind::Claude, "a", Status::Tool { label: "Bash(ls)".into() }, t0);
    let mut it = item(&a, "repo");
    it.cwd = "/work/repo".into();
    it.git = Some("⎇ main ●2".into());
    let m = build(&[it], &[], &SidebarState::default(), t0, &clock);
    assert_eq!(tooltip_lines(&m.sections[0].rows[0]), ["task a", "● 执行中 · Bash(ls)", "repo · 左", "/work/repo", "⎇ main ●2"]);
}

#[test]
fn renaming_never_matches_a_terminal_row() {
    let t0 = Instant::now();
    let a = session(AgentKind::Claude, "a", Status::Tool { label: "Bash(ls)".into() }, t0);
    let items = [item(&a, "repo")];
    let m = build(&items, &[terminal(7, "zsh", "repo")], &SidebarState::default(), t0, &clock);
    let rows = all_rows(&m, Some(&a.key));
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|(_, _, drawn)| *drawn), "nothing is collapsed");
    let only_terminal = build(&[], &[terminal(7, "zsh", "repo")], &SidebarState::default(), t0, &clock);
    assert_eq!(all_rows(&only_terminal, Some(&a.key)).len(), 1);
}

#[test]
fn archived_ended_sessions_leave_the_ended_group_and_ended_rows_show_their_directory() {
    let t0 = Instant::now();
    let kept = session(AgentKind::Claude, "k", Status::Ended, t0);
    let archived = session(AgentKind::Claude, "a", Status::Ended, t0);
    let items = [
        Item { cwd: "/Users/u/my proj".into(), ..item(&kept, "p") },
        Item { archived: true, ..item(&archived, "p") },
    ];
    let m = build(&items, &[], &SidebarState::default(), t0, &clock);
    let ended = m.sections.iter().find(|s| s.kind == SectionKind::Ended).unwrap();
    assert_eq!(ended.title, "已结束 · 1");
    assert_eq!(keys(&ended.rows), ["k"]);
    assert_eq!((ended.rows[0].dir.as_str(), ended.rows[0].cwd.as_str()), ("my proj", "/Users/u/my proj"));
    let only = [Item { archived: true, ..item(&archived, "p") }];
    assert!(build(&only, &[], &SidebarState::default(), t0, &clock).sections.iter().all(|s| s.kind != SectionKind::Ended));
}

#[test]
fn first_sentence_of_a_summary() {
    assert_eq!(first_sentence("接口改完了。正在跑测试"), "接口改完了。");
    assert_eq!(first_sentence("修好 1/2 个用例；正在重跑"), "修好 1/2 个用例；正在重跑");
    assert_eq!(first_sentence("Done. Next"), "Done.");
    assert_eq!(first_sentence(""), "");
}

#[test]
fn with_summaries_fills_rows() {
    let t0 = Instant::now();
    let a = session(AgentKind::Claude, "a", Status::Tool { label: "x".into() }, t0);
    let b = session(AgentKind::Claude, "b", Status::Tool { label: "x".into() }, t0);
    let items = [item(&a, "lab"), item(&b, "lab")];
    let m = build(&items, &[], &SidebarState::default(), t0, &clock);
    let m = with_summaries(m, |r| (r.name == "task a").then(|| "近期 a".to_string()));
    let rows: Vec<&Row> = m.sections.iter().flat_map(|s| s.rows.iter()).collect();
    assert_eq!(rows.iter().find(|r| r.name == "task a").unwrap().summary.as_deref(), Some("近期 a"));
    assert_eq!(rows.iter().find(|r| r.name == "task b").unwrap().summary, None);
}

#[test]
fn window_label_follows_the_language() {
    assert_eq!(window_label(2), "窗口 2");
    assert_eq!(crate::i18n::with_language(crate::i18n::Language::English, || window_label(2)), "Window 2");
}
