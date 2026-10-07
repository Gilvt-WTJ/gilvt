//! Anti-drift for the scenarios the GUI acceptance cases use (`tests/gui/cases/H`, `…/I`): like
//! `scenarios.rs`, each runs headless and what it produced goes through gilvt-agent's registry, timeline
//! and history parsers. Also pins the single source of truth: `crates/gilvt-fake-agent/scenarios/*.toml`
//! are the built-ins, and `tests/gui/scenarios/` holds only symlinks to them.

mod common;

use std::path::Path;

use common::{run, Opts, Run};
use gilvt_agent::{Item, ItemStatus, PlanState, TurnOutcome};
use gilvt_fake_agent::engine::Scripted;
use gilvt_fake_agent::keys::Key;
use gilvt_fake_agent::scenario::{Scenario, BUILTIN};

fn builtin(name: &str) -> Scenario {
    Scenario::builtin(name).unwrap()
}

/// The tool items of turn `turn`.
fn tool_items(r: &Run, turn: usize) -> Vec<gilvt_agent::ToolItem> {
    r.timeline().turns()[turn]
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Tool(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

fn names_and_statuses(items: &[gilvt_agent::ToolItem]) -> Vec<(&str, ItemStatus)> {
    items.iter().map(|t| (t.tool.as_str(), t.status.clone())).collect()
}

fn ends_idle(r: &Run) {
    let s = r.statuses();
    assert_eq!(&s[s.len() - 2..], &["idle".to_string(), "ended".to_string()][..], "{s:?}");
}

#[test]
fn timeline_has_edits_a_failure_with_its_error_lines_and_a_long_reply() {
    let r = run(&builtin("timeline"), vec![], Opts::default());
    ends_idle(&r);
    let items = tool_items(&r, 0);
    assert_eq!(
        names_and_statuses(&items),
        [
            ("Read", ItemStatus::Ok),
            ("Edit", ItemStatus::Ok),
            ("Write", ItemStatus::Ok),
            ("Bash", ItemStatus::Ok),
            ("Bash", ItemStatus::Failed { exit: Some(101) }),
            ("Bash", ItemStatus::Ok),
        ]
    );
    assert_eq!(items[1].lines, Some((2, 1)), "Edit: +2 −1");
    assert_eq!(items[2].lines, Some((1, 0)), "Write: +1");
    // H10's judge lists these: the marked lines, not the last three.
    assert_eq!(
        items[4].error_excerpt,
        [
            "Error: cannot find value `greeting` in this scope",
            "test tests::greets ... FAILED",
            "test result: FAILED. 1 passed; 1 failed; 0 ignored",
        ]
    );
    // H5: the thinking row the filters hide and 「全部」 brings back, before the first tool.
    match r.timeline().turns()[0].items.first() {
        Some(Item::Thinking { secs, text }) => {
            assert_eq!(text, &["Read README.md first, then edit it and run the tests."]);
            assert!(secs.is_some_and(|s| s >= 0.7), "{secs:?}: timed from the prompt record");
        }
        other => panic!("first item: {other:?}"),
    }
    assert_eq!(r.timeline().turns()[0].outcome, TurnOutcome::Done);
    assert!(r.text().contains("line 100: end of the long reply"));
    assert!(r.hooks().iter().any(|(e, p)| e == "PreToolUse" && p["tool_input"]["command"] == "echo early"), "anchored by a hook");
}

#[test]
fn timeline_codex_replaces_the_plan_and_counts_the_patch() {
    let r = run(&builtin("timeline-codex"), vec![], Opts::default());
    ends_idle(&r);
    let items = tool_items(&r, 0);
    let work: Vec<_> = items.iter().filter(|t| t.tool != "update_plan").collect();
    assert_eq!(work.iter().map(|t| t.tool.as_str()).collect::<Vec<_>>(), ["exec_command", "apply_patch", "exec_command"]);
    assert!(work.iter().all(|t| t.status == ItemStatus::Ok));
    assert_eq!(work[1].lines, Some((3, 1)), "apply_patch: +3 −1");
    let tl = r.timeline();
    let plan: Vec<(&str, PlanState)> = tl.plan().iter().map(|p| (p.text.as_str(), p.state)).collect();
    assert_eq!(plan, [("读代码", PlanState::Done), ("改代码", PlanState::Done), ("跑测试", PlanState::Done)]);
}

#[test]
fn timeline_live_runs_52_numbered_bash_calls_in_order() {
    let r = run(&builtin("timeline-live"), vec![], Opts::default());
    ends_idle(&r);
    let items = tool_items(&r, 0);
    assert_eq!(items.len(), 52);
    assert!(items.iter().all(|t| t.tool == "Bash" && t.status == ItemStatus::Ok));
    assert_eq!(items[0].detail.input[0].1, "echo step 01");
    assert_eq!(items[51].detail.input[0].1, "echo step 52");
}

#[test]
fn long_tool_runs_or_is_interrupted() {
    let r = run(&builtin("long-tool"), vec![], Opts::default());
    assert!(r.statuses().contains(&"tool:Bash(sleep 30)".to_string()), "{:?}", r.statuses());
    ends_idle(&r);
    let esc = run(&builtin("long-tool"), vec![Scripted::During(Key::Esc)], Opts::default());
    assert_eq!(esc.timeline().turns()[0].outcome, TurnOutcome::Interrupted);
    assert_eq!(names_and_statuses(&tool_items(&esc, 0)), [("Bash", ItemStatus::Interrupted)]);
}

#[test]
fn think_long_is_thinking_until_its_reply() {
    let r = run(&builtin("think-long"), vec![], Opts::default());
    let s = r.statuses();
    assert!(s.contains(&"thinking".to_string()), "gilvt sees 思考中 after the submitted prompt: {s:?}");
    ends_idle(&r);
    let tl = r.timeline();
    assert_eq!(tl.turns()[0].prompt, "Think for a while");
}

#[test]
fn long_title_has_a_long_prompt_and_a_long_command() {
    let r = run(&builtin("long-title"), vec![], Opts::default());
    assert!(r.statuses().iter().any(|s| s.starts_with("tool:Bash(cargo test")), "{:?}", r.statuses());
    ends_idle(&r);
    assert!(r.timeline().turns()[0].prompt.chars().count() > 100, "the title has to overflow a 240 px row");
}

#[test]
fn three_turns_one_failed() {
    let r = run(&builtin("three-turns"), vec![], Opts::default());
    let tl = r.timeline();
    let turns: Vec<(&str, &TurnOutcome)> = tl.turns().iter().map(|t| (t.prompt.as_str(), &t.outcome)).collect();
    assert_eq!(
        turns,
        [
            ("第一轮：列出文件", &TurnOutcome::Done),
            ("第二轮：拉取依赖", &TurnOutcome::Failed { message: "Network connection lost".into() }),
            ("第三轮：跑测试", &TurnOutcome::Done),
        ]
    );
    ends_idle(&r);
}

#[test]
fn monitor_slow_is_three_turns_with_a_slow_first_tool() {
    let scenario = builtin("monitor-slow");
    let r = run(&scenario, vec![], Opts::default());
    let tl = r.timeline();
    let turns: Vec<(&str, &TurnOutcome)> = tl.turns().iter().map(|t| (t.prompt.as_str(), &t.outcome)).collect();
    assert_eq!(
        turns,
        [
            ("第一轮：列出文件", &TurnOutcome::Done),
            ("第二轮：拉取依赖", &TurnOutcome::Failed { message: "Network connection lost".into() }),
            ("第三轮：跑测试", &TurnOutcome::Done),
        ]
    );
    ends_idle(&r);
    let first_tool = scenario.steps.iter().find_map(|s| match s {
        gilvt_fake_agent::scenario::Step::Tool(t) => Some(t.ms),
        _ => None,
    });
    assert!(first_tool.is_some_and(|ms| ms >= 3000), "running for several 1 s ticks: {first_tool:?}");
}

#[test]
fn lite_mixed_has_no_hooks_and_every_row_kind() {
    let k = vec![Scripted::Key(Key::Char('n')), Scripted::During(Key::Esc)];
    let r = run(&builtin("lite-mixed"), k, Opts::default());
    assert!(r.hooks().is_empty());
    let first = tool_items(&r, 0);
    assert_eq!(
        names_and_statuses(&first),
        [("Bash", ItemStatus::Ok), ("Bash", ItemStatus::Failed { exit: Some(101) }), ("Bash", ItemStatus::Denied)]
    );
    assert!(first.iter().all(|t| t.anchor.is_none()), "no hooks, no anchors");
    let tl = r.timeline();
    assert_eq!(tl.turns()[0].outcome, TurnOutcome::Done, "skip_tool continues the turn");
    assert_eq!(tl.turns()[1].outcome, TurnOutcome::Interrupted);
    // Transcript only: Claude records an Esc'd call like a rejection, so gilvt shows 已拒绝 (yellow, like
    // 已中断); with hooks the same call is Interrupted (`long_tool_runs_or_is_interrupted`).
    assert_eq!(names_and_statuses(&tool_items(&r, 1)), [("Bash", ItemStatus::Denied)]);
}

#[test]
fn todo_states_shows_all_three_then_completes() {
    let r = run(&builtin("todo-states"), vec![], Opts::default());
    assert!(r.text().contains("写测试") && r.text().contains("更新文档"));
    let tl = r.timeline();
    assert_eq!(tl.plan().len(), 3);
    assert!(tl.plan().iter().all(|p| p.state == PlanState::Done));
    ends_idle(&r);
}

#[test]
fn history_seeds_are_listed_with_two_turns() {
    let c = run(&builtin("hist-claude"), vec![], Opts { prompt: Some("i1 首条提示词".into()), ..Opts::default() });
    let e = gilvt_agent::parse_claude_session(&c.transcript).expect("listed");
    assert_eq!((e.turns, e.first_prompt.as_str(), e.session_id.as_str()), (2, "i1 首条提示词", c.session_id.as_str()));
    let x = run(&builtin("hist-codex"), vec![], Opts::default());
    let e = gilvt_agent::parse_codex_rollout(&x.transcript).expect("listed");
    assert_eq!((e.turns, e.first_prompt.as_str()), (2, "历史会话：检查测试"));
    assert_eq!(e.cwd, x.cwd);
    // Answered with no keys at all (a headless seed ends at the first thing that waits for one).
    assert_eq!((c.code, x.code), (0, 0));
}

fn read(r: &Run, name: &str) -> Option<String> {
    std::fs::read_to_string(r.cwd.join(name)).ok()
}

#[test]
fn artifacts_basic_writes_two_files_and_passes_a_test() {
    let r = run(&builtin("artifacts-basic"), vec![], Opts::default());
    ends_idle(&r);
    assert_eq!(read(&r, "hello.txt").as_deref(), Some("hello\n"));
    assert_eq!(read(&r, "README.md").as_deref(), Some("# Demo\n\nHello!\n"));
    let tl = r.timeline();
    assert_eq!(tl.turns()[0].reply, "Added hello.txt and updated README.md.");
    let items = tool_items(&r, 0);
    assert_eq!(names_and_statuses(&items), [("Write", ItemStatus::Ok), ("Edit", ItemStatus::Ok), ("Bash", ItemStatus::Ok)]);
}

#[test]
fn artifacts_turns_has_three_turns_and_rewrites_a_txt() {
    let r = run(&builtin("artifacts-turns"), vec![], Opts::default());
    ends_idle(&r);
    let tl = r.timeline();
    assert_eq!(tl.turns().len(), 3);
    assert_eq!(
        names_and_statuses(&tool_items(&r, 0)),
        [("Write", ItemStatus::Ok), ("Bash", ItemStatus::Failed { exit: Some(101) })]
    );
    assert!(tool_items(&r, 1).is_empty(), "turn 2 only replies");
    assert_eq!(tl.turns()[1].reply, "I changed a.txt and the test run failed.");
    assert_eq!(names_and_statuses(&tool_items(&r, 2)), [("Write", ItemStatus::Ok)]);
    assert_eq!(read(&r, "a.txt").as_deref(), Some("turn three\n"), "the last write wins");
}

#[test]
fn artifacts_followups_is_one_task_over_three_turns() {
    let r = run(&builtin("artifacts-followups"), vec![], Opts::default());
    ends_idle(&r);
    let tl = r.timeline();
    assert_eq!(tl.turns().len(), 3);
    for i in 0..3 {
        assert_eq!(names_and_statuses(&tool_items(&r, i)), [("Write", ItemStatus::Ok)], "turn {i}");
    }
    assert_eq!(read(&r, "a.txt").as_deref(), Some("one\ntwo\n"), "the second turn rewrites a.txt");
    assert_eq!(read(&r, "b.txt").as_deref(), Some("bee\n"));
}

#[test]
fn artifacts_many_writes_ten_files_in_one_turn() {
    let r = run(&builtin("artifacts-many"), vec![], Opts::default());
    ends_idle(&r);
    assert_eq!(r.timeline().turns().len(), 1);
    for i in 1..=10 {
        assert_eq!(read(&r, &format!("f{i:02}.txt")).as_deref(), Some("x\n"), "f{i:02}.txt");
    }
}

#[test]
fn artifacts_running_writes_before_a_thirty_second_tool() {
    let mut scenario = builtin("artifacts-running");
    // Esc would interrupt the thinking wait (which gives gilvt time for its `before` snapshot in the GUI);
    // headless there is nothing to wait for, so drop it and let Esc land on the long tool.
    scenario.steps.retain(|s| !matches!(s, gilvt_fake_agent::scenario::Step::Thinking { .. }));
    let r = run(&scenario, vec![Scripted::During(Key::Esc)], Opts::default());
    assert_eq!(read(&r, "live.txt").as_deref(), Some("live\n"), "written when the tool starts");
    assert_eq!(r.timeline().turns()[0].outcome, TurnOutcome::Interrupted, "still running when Esc came");
}

#[test]
fn one_source_of_truth_for_scenarios() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios");
    let gui_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/gui/scenarios");
    let names = |dir: &Path| {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".toml").map(str::to_string))
            .collect();
        v.sort();
        v
    };
    let mut builtins: Vec<String> = BUILTIN.iter().map(|(n, _)| n.to_string()).collect();
    builtins.sort();
    assert_eq!(names(&crate_dir), builtins, "every scenarios/*.toml is a built-in and vice versa");
    assert_eq!(names(&gui_dir), builtins, "tests/gui/scenarios lists exactly the built-ins");
    for name in &builtins {
        let link = gui_dir.join(format!("{name}.toml"));
        let target = std::fs::read_link(&link).unwrap_or_else(|_| panic!("{} must be a symlink", link.display()));
        assert_eq!(target, Path::new("../../../crates/gilvt-fake-agent/scenarios").join(format!("{name}.toml")));
    }
}
