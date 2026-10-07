use std::sync::Arc;
use std::time::UNIX_EPOCH;

use gilvt_agent::{SubagentInfo, TurnOutcome};
use gilvt_term::ScrollOutcome;

use super::*;

/// The row's text as drawn: the summary, then the status.
fn line(s: &Summary) -> String {
    match &s.status {
        Some(x) => format!("{} {x}", s.text),
        None => s.text.clone(),
    }
}

fn at(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(1_000_000 + ms)
}

fn tool(id: &str, name: &str, summary: &str) -> ToolItem {
    let mut t = ToolItem::new(id, name);
    t.summary = summary.into();
    t.status = ItemStatus::Ok;
    t.started = Some(at(0));
    t.ended = Some(at(100));
    t.anchor = Some(Anchor { pane: 1, line: 40 });
    t
}

fn turn(index: u32, items: Vec<Item>) -> Turn {
    Turn { index, prompt: format!("prompt {index}\nmore"), started: Some(at(0)), ended: Some(at(123_000)), outcome: TurnOutcome::Done, items, tokens: 0, steps: 9, reply: String::new() }
}

fn task(children: Vec<Item>) -> ToolItem {
    let mut t = tool("task1", "Task", "查找现有分页实现");
    t.anchor = None;
    t.subagent = Some(SubagentInfo { agent_id: "a1".into(), agent_type: Some("Explore".into()), result: Some("Found 2 existing patterns".into()), done: true });
    t.children = children;
    t
}

fn tools(rows: &[Row]) -> Vec<(&str, bool, String)> {
    rows.iter()
        .map(|r| match &r.kind {
            RowKind::Tool(t) => (r.key.as_str(), r.sub, line(&summary(t))),
            RowKind::Thinking { .. } => (r.key.as_str(), r.sub, "✻".into()),
            RowKind::Returned(s) => (r.key.as_str(), r.sub, format!("↩ {s}")),
            RowKind::Truncated(n) => (r.key.as_str(), r.sub, format!("另有 {n} 条")),
            RowKind::Empty(s) => (r.key.as_str(), r.sub, s.to_string()),
        })
        .collect()
}

#[test]
fn filters_classify_claude_and_codex_tools() {
    for t in ["Bash", "shell", "exec_command"] {
        assert!(Filter::Bash.matches(&tool("x", t, "")), "{t}");
        assert!(!Filter::Edit.matches(&tool("x", t, "")), "{t}");
    }
    for t in ["Edit", "Write", "MultiEdit", "NotebookEdit", "apply_patch"] {
        assert!(Filter::Edit.matches(&tool("x", t, "")), "{t}");
        assert!(!Filter::Bash.matches(&tool("x", t, "")), "{t}");
    }
    let mut failed = tool("x", "Read", "a.rs");
    assert!(!Filter::Failed.matches(&failed));
    failed.status = ItemStatus::Failed { exit: None };
    assert!(Filter::Failed.matches(&failed));
    assert!(Filter::All.matches(&failed));
    assert_eq!(Filter::ALL.map(Filter::label), ["全部", "Bash", "编辑", "失败"]);
}

#[test]
fn subagent_rows_nest_and_a_matching_child_keeps_its_task_row() {
    let children = vec![Item::Tool(tool("g", "Grep", "\"LIMIT ?\"")), Item::Tool(tool("b", "Bash", "ls"))];
    let t = turn(3, vec![Item::Tool(tool("r", "Read", "handler.go")), Item::Tool(task(children))]);
    let open = Expanded::default();
    let all = turn_rows(&t, Filter::All, &open, false);
    assert_eq!(
        tools(&all),
        [
            ("task1", false, "Task · Explore 查找现有分页实现".to_string()),
            ("ret:task1", true, "↩ Found 2 existing patterns".to_string()),
            ("b", true, "Bash ls".to_string()),
            ("g", true, "Grep \"LIMIT ?\"".to_string()),
            ("r", false, "Read handler.go".to_string()),
        ]
    );
    let RowKind::Tool(task_row) = &all[0].kind else { panic!() };
    assert_eq!((task_row.icon, task_row.icon_ink, task_row.ink), ("⎇", Ink::Subagent, Ink::Subagent));
    // Bash: the task row stays for its matching child; the other child and the ↩ line go.
    let bash = turn_rows(&t, Filter::Bash, &open, false);
    assert_eq!(tools(&bash).iter().map(|r| r.0).collect::<Vec<_>>(), ["task1", "b"]);
    // Nothing inside matches 编辑: the task row goes too.
    assert!(turn_rows(&t, Filter::Edit, &open, false).is_empty());
}

#[test]
fn thinking_is_collapsed_and_hidden_by_filters() {
    let t = turn(1, vec![Item::Thinking { secs: Some(4.0), text: vec!["plan".into(), "more".into()] }, Item::Tool(tool("b", "Bash", "ls"))]);
    let mut open = Expanded::default();
    // Newest first: the Bash call (the later item) is above the thinking block.
    let rows = turn_rows(&t, Filter::All, &open, false);
    assert_eq!(rows[1].key, "think:turn1:0");
    assert_eq!(rows[1].kind, RowKind::Thinking { secs: Some("4s".into()), expandable: true, text: None });
    open.toggle_row("think:turn1:0");
    let rows = turn_rows(&t, Filter::All, &open, false);
    assert_eq!(rows[1].kind, RowKind::Thinking { secs: Some("4s".into()), expandable: true, text: Some(vec!["plan".into(), "more".into()]) });
    assert_eq!(turn_rows(&t, Filter::Bash, &open, false).len(), 1);
    // A redacted block (no text) cannot be opened.
    let t = turn(1, vec![Item::Thinking { secs: None, text: vec![] }]);
    assert_eq!(turn_rows(&t, Filter::All, &open, false)[0].kind, RowKind::Thinking { secs: None, expandable: false, text: None });
}

#[test]
fn expanding_by_id_shows_arguments_and_output() {
    let mut b = tool("b1", "Bash", "go test ./...");
    b.detail.input = vec![("command".into(), "go test ./...".into()), ("description".into(), "Run\nthe tests".into())];
    b.detail.output = (0..30).map(|i| format!("line {i}")).collect();
    let t = turn(1, vec![Item::Tool(b)]);
    let mut open = Expanded::default();
    let RowKind::Tool(r) = &turn_rows(&t, Filter::All, &open, false)[0].kind else { panic!() };
    assert_eq!(r.detail, None);
    open.toggle_row("b1");
    let RowKind::Tool(r) = &turn_rows(&t, Filter::All, &open, false)[0].kind else { panic!() };
    let d = r.detail.clone().unwrap();
    assert_eq!(d[..3], [
        DetailLine::Input { key: "command".into(), value: "go test ./...".into() },
        DetailLine::Input { key: "description".into(), value: "Run".into() },
        DetailLine::More("the tests".into()),
    ]);
    assert_eq!(d.len(), 3 + 20);
    assert!(detail_text(&d).starts_with("command: go test ./...\ndescription: Run\n  the tests\nline 0"));
    open.toggle_row("b1");
    assert!(open.rows.is_empty());
}

#[test]
fn row_states_follow_the_mockup() {
    let row = |t: ToolItem| match turn_rows(&turn(1, vec![Item::Tool(t)]), Filter::All, &Expanded::default(), false).remove(0).kind {
        RowKind::Tool(r) => r,
        _ => panic!(),
    };
    let mut failed = tool("f", "Bash", "go test ./internal/...");
    failed.status = ItemStatus::Failed { exit: Some(1) };
    failed.started = Some(at(0));
    failed.ended = Some(at(3200));
    failed.error_excerpt = vec!["FAIL TestPage_Limit".into()];
    let r = row(failed);
    assert_eq!((r.icon, r.icon_ink, r.ink), ("✗", Ink::Failed, Ink::Failed));
    assert_eq!(line(&summary(&r)), "Bash go test ./internal/... · exit 1");
    assert_eq!(r.error, ["FAIL TestPage_Limit"]);
    assert_eq!(r.timing, Timing::Took("3.2s".into()));

    let mut denied = tool("d", "Bash", "rm -rf build");
    denied.status = ItemStatus::Denied;
    let r = row(denied);
    assert_eq!((r.icon, r.ink), ("⊘", Ink::Warn));
    assert_eq!(line(&summary(&r)), "Bash rm -rf build · 已拒绝");
    assert!(r.error.is_empty());

    let mut stopped = tool("i", "Bash", "sleep 9");
    stopped.status = ItemStatus::Interrupted;
    assert_eq!(line(&summary(&row(stopped))), "Bash sleep 9 · 已中断");

    let mut pending = tool("p", "Bash", "npm test");
    pending.status = ItemStatus::Pending;
    let r = row(pending);
    assert_eq!((r.icon, line(&summary(&r)).as_str()), ("⏳", "Bash npm test · 待审批"));

    let mut running = tool("run", "Bash", "go test");
    running.status = ItemStatus::Running;
    running.ended = None;
    let r = row(running);
    assert_eq!((r.icon, r.icon_ink), ("▶", Ink::Running));
    assert_eq!(r.timing, Timing::Running(Some(at(0))));
    assert_eq!(r.timing.label(at(12_400)), "12s…");
    assert_eq!(Timing::Running(None).label(at(0)), "…");

    let mut plain = tool("n", "Read", "a.rs");
    assert_eq!(row(plain.clone()).ink, Ink::Text);
    plain.anchor = None;
    let r = row(plain);
    assert_eq!((r.icon, r.ink), ("📖", Ink::Muted));
}

#[test]
fn edits_show_line_counts_and_a_clickable_file_name() {
    let mut e = tool("e", "Edit", "handler.go");
    e.lines = Some((18, 2));
    e.detail.input = vec![("file_path".into(), "internal/handler.go".into()), ("old_string".into(), "x".into())];
    let RowKind::Tool(r) = turn_rows(&turn(1, vec![Item::Tool(e)]), Filter::All, &Expanded::default(), false).remove(0).kind else { panic!() };
    let s = summary(&r);
    assert_eq!(s.text, "Update handler.go +18 −2");
    assert_eq!(&s.text[s.file.clone().unwrap()], "handler.go");
    assert_eq!(&s.text[s.added.clone().unwrap()], "+18");
    assert_eq!(&s.text[s.removed.clone().unwrap()], "−2");
    assert_eq!(r.file.as_deref(), Some("internal/handler.go"));
    assert_eq!(r.icon, "✏️");
    assert_eq!(lines_labels(Some((3, 0))), (Some("+3".into()), None));
    assert_eq!(lines_labels(None), (None, None));
}

#[test]
fn file_targets_resolve_against_the_session_cwd() {
    let mut read = tool("r", "Read", "lib.rs");
    read.detail.input = vec![("file_path".into(), "src/lib.rs".into())];
    assert_eq!(file_target(&read).as_deref(), Some("src/lib.rs"));
    let mut patch = tool("p", "apply_patch", "main.rs");
    patch.detail.input = vec![("input".into(), "*** Begin Patch\n*** Update File: /w/src/main.rs\n@@\n-a\n+b".into())];
    assert_eq!(file_target(&patch).as_deref(), Some("/w/src/main.rs"));
    assert_eq!(file_target(&tool("b", "Bash", "ls")), None);

    let cwd = Path::new("/Users/u/repo");
    let home = Path::new("/Users/u");
    assert_eq!(resolve_path("src/lib.rs", Some(cwd), Some(home)), Some(PathBuf::from("/Users/u/repo/src/lib.rs")));
    assert_eq!(resolve_path("/etc/hosts", Some(cwd), Some(home)), Some(PathBuf::from("/etc/hosts")));
    assert_eq!(resolve_path("~/notes.md", Some(cwd), Some(home)), Some(PathBuf::from("/Users/u/notes.md")));
    assert_eq!(resolve_path("src/lib.rs", None, Some(home)), None);
}

#[test]
fn durations_and_history_lines() {
    assert_eq!(duration_label(Duration::from_millis(100)), "0.1s");
    assert_eq!(duration_label(Duration::from_millis(3240)), "3.2s");
    assert_eq!(duration_label(Duration::from_millis(4000)), "4s");
    assert_eq!(duration_label(Duration::from_millis(9970)), "10s");
    assert_eq!(duration_label(Duration::from_secs(21)), "21s");
    assert_eq!(duration_label(Duration::from_secs(72)), "1m12s");
    let mut t = turn(2, vec![]);
    t.prompt = "  \n重构路由注册\n细节".into();
    assert_eq!(history_label(&t, None), "第 2 轮 · 重构路由注册 · 9 步 · 2m03s ✓");
    assert_eq!(history_label(&t, Some("14:10")), "第 2 轮 · 重构路由注册 · 9 步 · 14:10 · 2m03s ✓");
    t.outcome = TurnOutcome::Interrupted;
    t.ended = None;
    t.prompt = String::new();
    assert_eq!(history_label(&t, None), "第 2 轮 · （无提示词） · 9 步 · ✗");
    assert_eq!(history_label(&t, Some("昨天 14:10")), "第 2 轮 · （无提示词） · 9 步 · 昨天 14:10 · ✗");
}

#[test]
fn entries_put_history_below_newest_first_and_expand_in_place() {
    let turns: Vec<Arc<Turn>> = vec![
        Arc::new(turn(1, vec![Item::Tool(tool("a", "Read", "a.rs"))])),
        Arc::new(turn(2, vec![])),
        Arc::new(turn(3, vec![Item::Tool(tool("c", "Bash", "ls"))])),
    ];
    let mut cache = RowCache::with_clock(fake_clock);
    let mut open = Expanded::default();
    let e = cache.entries(&turns, Filter::All, &open, 0);
    assert_eq!(e[0], Entry::Head);
    assert_eq!(e[1], Entry::Title { turn: 3, filter: Filter::All, started: Some("1000s".into()) });
    assert!(matches!(&e[2], Entry::Row(r) if r.key == "c" && !r.history));
    assert!(matches!(&e[3], Entry::History { index: 2, open: false, .. }));
    assert!(matches!(&e[4], Entry::History { index: 1, open: false, .. }));
    assert_eq!(e.len(), 5);

    open.toggle_turn(1);
    open.toggle_turn(2);
    let e = cache.entries(&turns, Filter::All, &open, 1);
    assert!(matches!(&e[3], Entry::History { index: 2, open: true, .. }));
    assert!(matches!(&e[4], Entry::Row(r) if r.history && r.kind == RowKind::Empty("该轮的明细已不再保留")));
    assert!(matches!(&e[6], Entry::Row(r) if r.key == "a" && r.history));

    // Filtered to nothing: a note under the title.
    let e = cache.entries(&turns, Filter::Failed, &Expanded::default(), 2);
    assert_eq!(e[2], Entry::Note("本轮没有符合的事件"));
    let e = cache.entries(&[Arc::new(turn(1, vec![]))], Filter::All, &Expanded::default(), 2);
    assert_eq!(e[2], Entry::Note("本轮暂无事件"));
}

/// Seconds since the epoch, 「s」 marking a clock asked for with seconds.
fn fake_clock(t: SystemTime, seconds: bool) -> String {
    let secs = t.duration_since(UNIX_EPOCH).unwrap().as_secs();
    if seconds { format!("{secs}s") } else { secs.to_string() }
}

#[test]
fn entries_show_when_each_turn_started() {
    let mut unknown = turn(2, vec![]);
    unknown.started = None;
    let turns: Vec<Arc<Turn>> = vec![Arc::new(turn(1, vec![])), Arc::new(unknown.clone()), Arc::new(turn(3, vec![]))];
    let e = RowCache::with_clock(fake_clock).entries(&turns, Filter::All, &Expanded::default(), 0);
    // The current turn's title to the second, history turns to the minute; unknown starts are left out.
    assert_eq!(e[1], Entry::Title { turn: 3, filter: Filter::All, started: Some("1000s".into()) });
    assert!(matches!(&e[3], Entry::History { index: 2, label, started: None, .. } if label == "第 2 轮 · prompt 2 · 9 步 · ✓"));
    assert!(matches!(&e[4], Entry::History { index: 1, label, started: Some(s), .. } if s == "1000" && label == "第 1 轮 · prompt 1 · 9 步 · 1000 · 2m03s ✓"));
    let e = RowCache::with_clock(fake_clock).entries(&[Arc::new(unknown)], Filter::All, &Expanded::default(), 0);
    assert_eq!(e[1], Entry::Title { turn: 2, filter: Filter::All, started: None });
}

#[test]
fn unchanged_turns_are_not_rebuilt() {
    let old = Arc::new(turn(1, vec![Item::Tool(tool("a", "Read", "a.rs"))]));
    let cur = Arc::new(turn(2, vec![Item::Tool(tool("b", "Bash", "ls"))]));
    let mut open = Expanded::default();
    open.toggle_turn(1);
    let mut cache = RowCache::default();
    cache.entries(&[old.clone(), cur.clone()], Filter::All, &open, 0);
    assert_eq!(cache.builds, 2);
    cache.entries(&[old.clone(), cur.clone()], Filter::All, &open, 0);
    assert_eq!(cache.builds, 2);
    // A new snapshot of the current turn: only it is rebuilt.
    let cur2 = Arc::new(turn(2, vec![Item::Tool(tool("b", "Bash", "ls")), Item::Tool(tool("c", "Bash", "pwd"))]));
    let e = cache.entries(&[old.clone(), cur2], Filter::All, &open, 0);
    assert_eq!(cache.builds, 3);
    assert_eq!(e.iter().filter(|e| matches!(e, Entry::Row(_))).count(), 3);
    // Filter / expansion changes (gen) rebuild everything shown.
    cache.entries(&[old, cur], Filter::All, &open, 1);
    assert_eq!(cache.builds, 5);
}

#[test]
fn truncated_rows_and_running_ticks() {
    let mut running = tool("r", "Bash", "sleep");
    running.status = ItemStatus::Running;
    let t = Arc::new(turn(1, vec![Item::Truncated { hidden: 12 }, Item::Tool(running)]));
    let rows = turn_rows(&t, Filter::All, &Expanded::default(), false);
    // The hidden-earlier marker is the oldest item, so it ends up last.
    assert_eq!(rows[1].kind, RowKind::Truncated(12));
    let e = RowCache::default().entries(&[t], Filter::All, &Expanded::default(), 0);
    assert!(has_running(&e));
    let done = Arc::new(turn(1, vec![Item::Tool(tool("r", "Bash", "sleep"))]));
    assert!(!has_running(&RowCache::default().entries(&[done], Filter::All, &Expanded::default(), 0)));
}

#[test]
fn list_changes_are_minimal_splices() {
    assert_eq!(splice_range(&[1, 2, 3], &[1, 2, 3]), None);
    assert_eq!(splice_range(&[1, 2, 3], &[1, 2, 3, 4]), Some((3..3, 1)));
    assert_eq!(splice_range(&[1, 2, 3, 9], &[1, 2, 3, 4, 9]), Some((3..3, 1)));
    assert_eq!(splice_range(&[1, 2, 3], &[1, 5, 3]), Some((1..2, 1)));
    assert_eq!(splice_range(&[1, 2, 3], &[1]), Some((1..3, 0)));
    assert_eq!(splice_range::<i32>(&[], &[1, 2]), Some((0..0, 2)));
    assert_eq!(splice_range(&[1, 1], &[1, 1, 1]), Some((2..2, 1)));
}

#[test]
fn a_click_jumps_or_opens_the_detail() {
    let a = Some(Anchor { pane: 7, line: 120 });
    assert_eq!(jump(None, |_| panic!("no anchor, no scroll")), Jump::ToggleDetail);
    assert_eq!(jump(a, |x| { assert_eq!(x.line, 120); Some(ScrollOutcome::Shown { viewport_row: 8 }) }), Jump::Highlight);
    assert_eq!(jump(a, |_| Some(ScrollOutcome::Evicted)), Jump::Toast("已超出回滚范围"));
    assert_eq!(jump(a, |_| None), Jump::Toast("该 pane 已关闭"));
    assert_eq!(jump(a, |_| Some(ScrollOutcome::AltScreen)), Jump::ToggleDetail);
}

#[test]
fn running_rows_tick_only_while_the_session_is_live() {
    let mut running = tool("r", "Bash", "sleep");
    running.status = ItemStatus::Running;
    let t = Arc::new(turn(1, vec![Item::Tool(running)]));
    let e = RowCache::default().entries(&[t], Filter::All, &Expanded::default(), 0);
    assert!(rows_need_tick(true, &e));
    assert!(!rows_need_tick(false, &e), "an ended session's open rows never finish");
    let done = Arc::new(turn(1, vec![Item::Tool(tool("r", "Bash", "sleep"))]));
    assert!(!rows_need_tick(true, &RowCache::default().entries(&[done], Filter::All, &Expanded::default(), 0)));
}

#[test]
fn jump_needles_name_the_tool_as_claude_shows_it() {
    assert_eq!(jump_needles("Bash", "python3 -m unittest -v 2>&1"), [["Bash(", "python3 -m unittest -v 2"]]);
    assert_eq!(jump_needles("Edit", "notes.md"), [["Update(", "notes.md"]]);
    assert_eq!(jump_needles("Read", "notes.md"), [["Read(", "notes.md"], ["Read ", " file"]], "「Read 1 file」 when folded");
    assert_eq!(jump_needles("Grep", "fn main"), [["Search(", "fn main"]]);
    assert_eq!(jump_needles("Bash", "cargo build…"), [["Bash(", "cargo build"]]);
    assert_eq!(jump_needles("shell", "cargo test"), [["cargo test"]], "Codex: 「• Ran cargo test」");
    assert!(jump_needles("Bash", "  ls  ").is_empty(), "too short to tell calls apart");
    assert_eq!(jump_needles("Agent", "修复 divide 的测试"), [["Agent(", "修复 divide 的测试"]]);
}
