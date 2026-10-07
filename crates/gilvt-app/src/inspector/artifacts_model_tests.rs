use std::time::Duration;

use gilvt_agent::{Item, ItemStatus, ToolItem, Turn, TurnOutcome};
use gilvt_snapshot::ledger::{TurnRecord, TurnState};
use gilvt_snapshot::{ChangeStatus, FileChange, TreeId};

use super::*;

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn turn(index: u32, prompt: &str, start: u64, end: Option<u64>) -> Turn {
    Turn {
        index,
        prompt: prompt.into(),
        started: Some(at(start)),
        ended: end.map(at),
        outcome: if end.is_some() { TurnOutcome::Done } else { TurnOutcome::Running },
        items: Vec::new(),
        tokens: 0,
        steps: 0,
        reply: String::new(),
    }
}

fn change(path: &str, status: ChangeStatus, added: u32, removed: u32) -> FileChange {
    FileChange { path: path.into(), old_path: None, status, added, removed, binary: false }
}

fn record(seq: u32, prompt: &str, start: u64, state: TurnState, changes: Option<Vec<FileChange>>) -> TurnRecord {
    TurnRecord {
        seq,
        prompt: prompt.into(),
        started_ms: start * 1000,
        ended_ms: Some((start + 5) * 1000),
        repo_root: Some("/repo".into()),
        before: Some(TreeId(format!("b{seq}"))),
        after: Some(TreeId(format!("a{seq}"))),
        changes,
        skipped_large: Vec::new(),
        state,
    }
}

fn arcs<const N: usize>(turns: [Turn; N]) -> Vec<Arc<Turn>> {
    turns.into_iter().map(Arc::new).collect()
}

fn clock(t: SystemTime) -> String {
    format!("@{}", t.duration_since(UNIX_EPOCH).unwrap().as_secs())
}

fn no_diff(_: &CardRange) -> Option<Result<Vec<FileChange>, String>> {
    None
}

fn inputs<'a>(turns: &'a [Arc<Turn>], records: &'a [TurnRecord]) -> Inputs<'a> {
    Inputs { turns, records, live: None, fallback_root: None, clock: &clock, diff: &no_diff }
}

fn shell(summary: &str, status: ItemStatus) -> Item {
    let mut t = ToolItem::new("t1", "Bash");
    t.summary = summary.into();
    t.status = status;
    Item::Tool(t)
}

#[test]
fn a_finished_turn_becomes_a_card_with_its_files() {
    let mut t = turn(3, "给 /users 加分页\n详细说明", 1000, Some(1102));
    t.reply = "已完成分页。".into();
    let r = record(1, "给 /users 加分页", 1000, TurnState::Done, Some(vec![
        change("internal/handler.go", ChangeStatus::M, 18, 2),
        change("internal/repo.go", ChangeStatus::A, 40, 0),
    ]));
    let a = build(&inputs(&arcs([t]), &[r]));
    let c = &a.cards[0];
    assert_eq!((c.turn, c.title.as_str(), c.state, c.quote.as_str()), (Some(3), "给 /users 加分页", CardState::Done, "已完成分页。"));
    assert_eq!(c.took.as_deref(), Some("1m42s"));
    assert_eq!(c.started, "@1000");
    assert_eq!(c.files.len(), 2);
    assert_eq!((c.files[0].status, c.files[0].added, c.files[0].removed), ('M', 18, 2));
    assert_eq!(c.files[1].abs, PathBuf::from("/repo/internal/repo.go"));
    assert_eq!(c.range.as_ref().unwrap().before, TreeId("b1".into()));
}

#[test]
fn cards_are_newest_first_and_mark_files_changed_later() {
    let turns = [turn(1, "one", 100, Some(110)), turn(2, "two", 200, Some(210))];
    let records = [
        record(1, "one", 100, TurnState::Done, Some(vec![change("a.rs", ChangeStatus::M, 1, 0)])),
        record(2, "two", 200, TurnState::Done, Some(vec![change("a.rs", ChangeStatus::M, 2, 1), change("b.rs", ChangeStatus::A, 5, 0)])),
    ];
    let a = build(&inputs(&arcs(turns), &records));
    assert_eq!(a.cards.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), vec!["two", "one"]);
    assert!(a.cards[1].touched_later, "turn 1's a.rs was changed again by turn 2");
    assert!(!a.cards[0].touched_later);
}

#[test]
fn a_turn_without_file_changes_is_quiet() {
    let a = build(&inputs(&arcs([turn(1, "explain", 100, Some(105))]), &[record(1, "explain", 100, TurnState::Done, Some(vec![]))]));
    assert_eq!((a.cards[0].state, a.cards[0].files.len()), (CardState::Quiet, 0));
}

#[test]
fn a_running_turn_shows_the_live_files() {
    let t = turn(1, "go", 100, None);
    let mut r = record(1, "go", 100, TurnState::Running, None);
    r.after = None;
    r.ended_ms = None;
    let live = [change("a.rs", ChangeStatus::M, 6, 0)];
    let a = build(&Inputs { live: Some(&live), ..inputs(&arcs([t]), &[r]) });
    assert_eq!(a.cards[0].state, CardState::Running);
    assert_eq!(a.cards[0].files[0].path, "a.rs");
    assert!(a.cards[0].range.is_none(), "no `after` yet: nothing to open a diff against");
}

#[test]
fn degraded_turns_say_why() {
    let turns = [turn(1, "old", 10, Some(20)), turn(2, "nogit", 100, Some(110)), turn(3, "broke", 200, Some(210)), turn(4, "quit", 300, Some(310))];
    let records = [
        record(1, "nogit", 100, TurnState::NotGit, None),
        record(2, "broke", 200, TurnState::Failed("git: boom".into()), None),
        record(3, "quit", 300, TurnState::Lost, None),
    ];
    let a = build(&inputs(&arcs(turns), &records));
    let notes: Vec<(&str, Vec<Notice>)> = a.cards.iter().rev().map(|c| (c.title.as_str(), c.notices.clone())).collect();
    assert_eq!(notes[0], ("old", vec![Notice::NoSnapshot]));
    assert_eq!(notes[1], ("nogit", vec![Notice::NotGit]));
    assert_eq!(notes[2], ("broke", vec![Notice::Failed("git: boom".into())]));
    assert_eq!(notes[3], ("quit", vec![Notice::Lost]));
    assert!(a.cards.iter().all(|c| c.state == CardState::Degraded));
    assert_eq!(Notice::NoSnapshot.text(), "无快照：这一轮发生时 gilvt 没在记录");
    assert_eq!(Notice::NotGit.text(), "非 git 目录，暂不记录文件改动");
    assert_eq!(Notice::LargeSkipped(2).text(), "2 个大文件已跳过");
}

#[test]
fn large_files_are_noted_on_a_card_that_has_files() {
    let mut r = record(1, "x", 100, TurnState::Done, Some(vec![change("a.rs", ChangeStatus::M, 1, 0)]));
    r.skipped_large = vec!["big.bin".into()];
    let a = build(&inputs(&arcs([turn(1, "x", 100, Some(110))]), &[r]));
    assert_eq!(a.cards[0].notices, vec![Notice::LargeSkipped(1)]);
    assert_eq!(a.cards[0].state, CardState::Done);
}

#[test]
fn records_whose_turn_left_the_timeline_still_get_a_card() {
    let r = record(7, "trimmed away", 50, TurnState::Done, Some(vec![change("a.rs", ChangeStatus::M, 1, 0)]));
    let a = build(&inputs(&arcs([turn(9, "recent", 900, Some(910))]), &[r]));
    assert_eq!(a.cards.len(), 2);
    assert_eq!((a.cards[1].title.as_str(), a.cards[1].turn, a.cards[1].state), ("trimmed away", None, CardState::Done));
}

#[test]
fn turns_pair_with_records_by_prompt_and_time() {
    let turns = [turn(1, "same words", 100, Some(110)), turn(2, "same words", 5000, Some(5010)), turn(3, "other", 6000, Some(6010))];
    let records = [record(1, "same words", 5001, TurnState::Done, None), record(2, "other", 6001, TurnState::Done, None)];
    assert_eq!(match_turns(&turns, &records), vec![None, Some(0), Some(1)], "the repeated prompt pairs with the nearer record");
    let mut no_time = turn(4, "other", 0, None);
    no_time.started = None;
    assert_eq!(match_turns(&[no_time], &records), vec![Some(1)], "without a start time only the prompt is compared");
}

#[test]
fn titles_are_cut_and_use_the_first_line() {
    let long = "x".repeat(80);
    let a = build(&inputs(&arcs([turn(1, &long, 1, Some(2))]), &[]));
    assert_eq!(a.cards[0].title.chars().count(), 48, "47 chars and an ellipsis: at most 48 in all");
    assert!(a.cards[0].title.ends_with('…'));
}

#[test]
fn test_results_come_from_the_last_finished_test_command() {
    let mut t = turn(1, "x", 1, Some(2));
    t.items = vec![
        shell("go test ./...", ItemStatus::Failed { exit: Some(1) }),
        shell("ls", ItemStatus::Ok),
        shell("go test ./...", ItemStatus::Ok),
        shell("cargo test", ItemStatus::Running),
    ];
    assert_eq!(test_result(&t), Some(TestResult { command: "go test ./...".into(), ok: true, exit: None }));
    t.items = vec![shell("make test", ItemStatus::Failed { exit: Some(2) })];
    assert_eq!(test_result(&t), Some(TestResult { command: "make test".into(), ok: false, exit: Some(2) }));
    t.items = vec![shell("echo hi", ItemStatus::Ok)];
    assert_eq!(test_result(&t), None);
    for cmd in ["npm test", "pnpm test", "pytest -q", "cargo test --all"] {
        t.items = vec![shell(cmd, ItemStatus::Ok)];
        assert!(test_result(&t).is_some(), "{cmd}");
    }
}

#[test]
fn relative_times() {
    let now = at(1_000_000);
    assert_eq!(relative(now, at(999_990)), "刚刚");
    assert_eq!(relative(now, at(1_000_000 - 120)), "2 分钟前");
    assert_eq!(relative(now, at(1_000_000 - 7300)), "2 小时前");
    assert_eq!(relative(now, at(1_000_000 - 3 * 86_400)), "3 天前");
    assert_eq!(relative(at(10), at(100)), "刚刚", "a clock that ran backwards");
}

#[test]
fn arrow_keys_walk_cards_and_the_files_of_open_cards() {
    let mk = |key: u32, n: usize| ArtCard {
        files: (0..n).map(|i| FileRow { path: format!("f{i}"), abs: PathBuf::new(), status: 'M', added: 1, removed: 0, binary: false, old_path: None }).collect(),
        ..super::tests_support::card(key)
    };
    let a = Artifacts { summary: None, net: None, cards: vec![mk(2, 10), mk(1, 1)], quiet_groups: Vec::new(), blocks: vec![Block::Card(0), Block::Card(1)] };
    let open = |all: &dyn Fn(u32) -> bool| nav_rows(&a, &Open { net: false, card: &|c| c.key == 2, all, quiet: &|_| false });
    let rows = open(&|_| false);
    assert_eq!(rows.len(), 1 + SHOWN_FILES + 1, "an open card lists at most 8 files; a closed one only itself");
    assert_eq!(rows[0], Sel::Card(2));
    assert_eq!(rows[1], Sel::File(2, 0));
    assert_eq!(*rows.last().unwrap(), Sel::Card(1));
    let all = open(&|k| k == 2);
    assert_eq!(all.len(), 1 + 10 + 1);
    assert_eq!(step(&rows, None, 1), Some(Sel::Card(2)));
    assert_eq!(step(&rows, Some(Sel::Card(2)), 1), Some(Sel::File(2, 0)));
    assert_eq!(step(&rows, Some(Sel::Card(2)), -1), Some(Sel::Card(2)), "stays at the top");
    assert_eq!(step(&rows, Some(Sel::Card(1)), 1), Some(Sel::Card(1)), "stays at the bottom");
    assert_eq!(step(&rows, Some(Sel::File(2, 99)), 1), Some(Sel::Card(2)), "a row that vanished restarts at the top");
    assert_eq!(step(&[], None, 1), None);
}

#[test]
fn the_first_changed_line_of_a_file() {
    assert_eq!(first_changed_line("a\nb\nc\n", "a\nB\nc\n"), 2);
    assert_eq!(first_changed_line("a\nb\n", "a\nb\nc\n"), 3);
    assert_eq!(first_changed_line("a\nb\n", "a\n"), 2);
    assert_eq!(first_changed_line("", "x\n"), 1);
    assert_eq!(first_changed_line("same\n", "same\n"), 1);
}

#[test]
fn replies_join_the_task_before_them() {
    let turns = arcs([turn(1, "给 /users 加分页。要支持 cursor", 100, Some(110)), turn(2, "继续", 200, Some(210)), turn(3, "好的", 300, Some(310))]);
    let records = [
        record(1, "给 /users 加分页。要支持 cursor", 100, TurnState::Done, Some(vec![change("a.go", ChangeStatus::M, 5, 1)])),
        record(2, "继续", 200, TurnState::Done, Some(vec![change("a.go", ChangeStatus::M, 2, 2)])),
        record(3, "好的", 300, TurnState::Done, Some(vec![change("b.go", ChangeStatus::A, 9, 0)])),
    ];
    let a = build(&inputs(&turns, &records));
    assert_eq!(a.cards.len(), 1);
    let c = &a.cards[0];
    assert_eq!((c.key, c.turn, c.turns.clone()), (1, Some(1), vec![1, 2, 3]));
    assert_eq!(c.title, "给 /users 加分页", "tidy_prompt: the first sentence");
    assert_eq!(c.follow_ups, vec!["继续".to_string(), "好的".to_string()]);
    assert_eq!((c.started.as_str(), c.took.as_deref()), ("@100", Some("3m30s")));
    assert!(c.computing, "three turns: the net diff is asked for");
    assert!(c.counts_hidden, "the union's numbers are not the task's");
    let paths: Vec<&str> = c.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["a.go", "b.go"], "until then the union, by path");
    let r = c.range.as_ref().unwrap();
    assert_eq!((r.before.0.as_str(), r.after.0.as_str()), ("b1", "a3"));
    assert_eq!((r.label.as_str(), r.scope), ("本任务（第 1 轮前 → 第 3 轮后）", "这个任务"));
}

#[test]
fn a_task_uses_the_net_diff_once_it_is_in() {
    let turns = arcs([turn(1, "改 a", 100, Some(110)), turn(2, "继续", 200, Some(210))]);
    let records = [
        record(1, "改 a", 100, TurnState::Done, Some(vec![change("a.go", ChangeStatus::M, 5, 1)])),
        record(2, "继续", 200, TurnState::Done, Some(vec![change("a.go", ChangeStatus::M, 2, 2)])),
    ];
    let net = |r: &CardRange| (r.before.0 == "b1" && r.after.0 == "a2").then(|| Ok(vec![change("a.go", ChangeStatus::M, 6, 2)]));
    let a = build(&Inputs { diff: &net, ..inputs(&turns, &records) });
    let c = &a.cards[0];
    assert!(!c.computing);
    assert_eq!((c.files.len(), c.files[0].added, c.files[0].removed), (1, 6, 2));
}

#[test]
fn a_single_turn_task_needs_no_diff() {
    let records = [record(1, "改 a", 100, TurnState::Done, Some(vec![change("a.go", ChangeStatus::M, 5, 1)]))];
    let a = build(&inputs(&arcs([turn(4, "改 a", 100, Some(110))]), &records));
    let c = &a.cards[0];
    assert!(!c.computing);
    assert_eq!(c.files[0].added, 5);
    assert_eq!((c.range.as_ref().unwrap().label.as_str(), c.range.as_ref().unwrap().scope), ("本轮（第 4 轮前 → 后）", "这一轮"));
}

#[test]
fn grouping_keeps_non_replies_apart() {
    let turns = arcs([turn(1, "改 a", 100, Some(110)), turn(2, "不对，改成 cursor 分页", 200, Some(210)), turn(3, "/compact", 300, Some(310)), turn(4, "好的，再把 b 也改了", 400, Some(410))]);
    let a = build(&inputs(&turns, &[]));
    assert_eq!(a.cards.len(), 4);
}

#[test]
fn a_leading_reply_starts_its_own_task() {
    let a = build(&inputs(&arcs([turn(1, "继续", 100, Some(110)), turn(2, "ok", 200, Some(210))]), &[]));
    assert_eq!(a.cards.len(), 1);
    assert_eq!((a.cards[0].title.as_str(), a.cards[0].follow_ups.clone()), ("继续", vec!["ok".to_string()]));
}

#[test]
fn a_lost_member_inside_a_task_keeps_the_net_range() {
    let turns = arcs([turn(1, "改 a", 100, Some(110)), turn(2, "继续", 200, Some(210)), turn(3, "继续", 300, Some(310))]);
    let mut lost = record(2, "继续", 200, TurnState::Lost, None);
    lost.after = None;
    let records = [
        record(1, "改 a", 100, TurnState::Done, Some(vec![change("a.go", ChangeStatus::M, 1, 0)])),
        lost,
        record(3, "继续", 300, TurnState::Done, Some(vec![change("a.go", ChangeStatus::M, 1, 0)])),
    ];
    let a = build(&inputs(&turns, &records));
    let c = &a.cards[0];
    assert_eq!(c.state, CardState::Done);
    assert_eq!(c.range.as_ref().map(|r| (r.before.0.clone(), r.after.0.clone())), Some(("b1".into(), "a3".into())));
    assert_eq!(c.notices, vec![Notice::InTurn(2, Box::new(Notice::Lost))]);
    assert_eq!(c.notices[0].text(), "第 2 轮：中断：没有看到这一轮的结束");
}

#[test]
fn a_failed_diff_falls_back_to_the_union() {
    let turns = arcs([turn(1, "改 a", 100, Some(110)), turn(2, "继续", 200, Some(210))]);
    let records = [
        record(1, "改 a", 100, TurnState::Done, Some(vec![change("a.go", ChangeStatus::M, 5, 1)])),
        record(2, "继续", 200, TurnState::Done, Some(vec![change("b.go", ChangeStatus::M, 2, 2)])),
    ];
    let fail = |_: &CardRange| Some(Err("快照已清理".to_string()));
    let c = &build(&Inputs { diff: &fail, ..inputs(&turns, &records) }).cards[0];
    assert!(!c.computing);
    assert!(c.counts_hidden);
    assert_eq!(c.files.len(), 2);
    assert!(c.notices.contains(&Notice::DiffFailed("快照已清理".into())));
}

#[test]
fn a_multi_turn_task_without_a_range_hides_the_counts() {
    let turns = arcs([turn(1, "改 a", 100, Some(110)), turn(2, "继续", 200, Some(210)), turn(3, "继续", 300, Some(310))]);
    let mut lost = record(3, "继续", 300, TurnState::Lost, None);
    lost.after = None;
    let mut first = record(1, "改 a", 100, TurnState::Lost, Some(vec![change("a.go", ChangeStatus::M, 3, 0)]));
    first.after = None;
    let records = [first, record(2, "继续", 200, TurnState::NotGit, None), lost];
    let c = &build(&inputs(&turns, &records)).cards[0];
    assert!(c.range.is_none());
    assert!(!c.computing, "nothing to compute");
    assert!(c.counts_hidden);
    assert_eq!(c.files.len(), 1);
}

#[test]
fn a_filler_line_followed_by_more_opens_a_new_task() {
    let turns = arcs([turn(1, "改 a", 100, Some(110)), turn(2, "好的\n另外把 b 也改成 cursor 分页", 200, Some(210)), turn(3, "继续", 300, Some(310))]);
    let a = build(&inputs(&turns, &[]));
    assert_eq!(a.cards.iter().map(|c| c.turns.clone()).collect::<Vec<_>>(), vec![vec![2, 3], vec![1]]);
}

#[test]
fn a_multi_turn_range_over_one_turn_is_labelled_as_that_turn() {
    let turns = arcs([turn(1, "改 a", 100, Some(110)), turn(2, "继续", 200, Some(210))]);
    let mut lost = record(2, "继续", 200, TurnState::Lost, None);
    lost.after = None;
    let records = [record(1, "改 a", 100, TurnState::Done, Some(vec![change("a.go", ChangeStatus::M, 5, 1)])), lost];
    let c = &build(&inputs(&turns, &records)).cards[0];
    let r = c.range.as_ref().unwrap();
    assert_eq!((r.label.as_str(), r.scope), ("本轮（第 1 轮前 → 后）", "这一轮"));
    assert!(!c.computing && !c.counts_hidden, "one turn's own changes are the net diff");
}

#[test]
fn a_task_with_no_recorded_turn_says_so_once() {
    let turns = arcs([turn(1, "改 a", 100, Some(110)), turn(2, "继续", 200, Some(210))]);
    let c = &build(&inputs(&turns, &[])).cards[0];
    assert_eq!(c.notices, vec![Notice::NoSnapshot]);
    assert_eq!(c.state, CardState::Degraded);
}

#[test]
fn a_task_without_any_done_turn_is_degraded() {
    let turns = arcs([turn(1, "改 a", 100, Some(110)), turn(2, "继续", 200, Some(210))]);
    let records = [record(1, "改 a", 100, TurnState::NotGit, None), record(2, "继续", 200, TurnState::NotGit, None)];
    let c = &build(&inputs(&turns, &records)).cards[0];
    assert_eq!(c.state, CardState::Degraded);
    assert!(c.range.is_none());
}

#[test]
fn a_running_follow_up_asks_for_the_task_base() {
    let turns = arcs([turn(1, "改 a", 100, Some(110)), turn(2, "继续", 200, None)]);
    let mut running = record(2, "继续", 200, TurnState::Running, None);
    running.after = None;
    running.ended_ms = None;
    let records = [record(1, "改 a", 100, TurnState::Done, Some(vec![change("a.go", ChangeStatus::M, 5, 1)])), running];
    let live = [change("a.go", ChangeStatus::M, 7, 1), change("c.go", ChangeStatus::A, 3, 0)];
    let a = build(&Inputs { live: Some(&live), ..inputs(&turns, &records) });
    let c = &a.cards[0];
    assert_eq!(c.state, CardState::Running);
    assert_eq!(c.live_base, Some(TreeId("b1".into())));
    assert_eq!(c.files.len(), 2, "the live list (measured from the task's base) is shown as is");
    assert!(c.range.is_none(), "an earlier member's range would open a stale diff for the live files");
}

#[test]
fn touched_later_names_the_first_later_turn() {
    let turns = arcs([turn(1, "one", 100, Some(110)), turn(2, "two", 200, Some(210)), turn(3, "three", 300, Some(310))]);
    let records = [
        record(1, "one", 100, TurnState::Done, Some(vec![change("a.rs", ChangeStatus::M, 1, 0)])),
        record(2, "two", 200, TurnState::Done, Some(vec![change("a.rs", ChangeStatus::M, 1, 0)])),
        record(3, "three", 300, TurnState::Done, Some(vec![change("a.rs", ChangeStatus::M, 1, 0)])),
    ];
    let a = build(&inputs(&turns, &records));
    let one = a.cards.iter().find(|c| c.turn == Some(1)).unwrap();
    assert_eq!((one.touched_later, one.touched_later_turn), (true, Some(2)));
}

#[test]
fn runs_of_quiet_tasks_fold_into_one_group() {
    let turns = arcs([turn(1, "改 a", 100, Some(110)), turn(2, "解释一下", 200, Some(210)), turn(3, "/compact", 300, Some(310)), turn(4, "改 b", 400, Some(410)), turn(5, "说说", 500, Some(510))]);
    let records = [
        record(1, "改 a", 100, TurnState::Done, Some(vec![change("a", ChangeStatus::M, 1, 0)])),
        record(2, "解释一下", 200, TurnState::Done, Some(vec![])),
        record(3, "/compact", 300, TurnState::Done, Some(vec![])),
        record(4, "改 b", 400, TurnState::Done, Some(vec![change("b", ChangeStatus::M, 1, 0)])),
        record(5, "说说", 500, TurnState::Done, Some(vec![])),
    ];
    let a = build(&inputs(&turns, &records));
    assert_eq!(a.quiet_groups.len(), 2, "a single quiet task folds too");
    assert_eq!(a.quiet_groups[1].cards, vec![3, 2], "newest first");
    assert_eq!(a.quiet_groups[1].titles, vec!["/compact".to_string(), "解释一下".to_string()]);
    assert_eq!(a.quiet_groups[1].key, 3);
    let kinds: Vec<&str> = a.blocks.iter().map(|b| match b { Block::Card(_) => "card", Block::Quiet(_) => "quiet" }).collect();
    assert_eq!(kinds, vec!["quiet", "card", "quiet", "card"]);
    assert_eq!(a.cards.iter().find(|c| c.key == 2).unwrap().quiet_group, Some(3));
    assert_eq!(newest_open_by_default(&a), Some(4), "the newest non-folded card");
}

#[test]
fn the_session_net_counts_only_files_the_agent_touched() {
    let turns = arcs([turn(2, "改 a", 100, Some(110)), turn(3, "改 b", 200, Some(210))]);
    let records = [
        record(1, "改 a", 100, TurnState::Done, Some(vec![change("a", ChangeStatus::M, 5, 0)])),
        record(2, "改 b", 200, TurnState::Done, Some(vec![change("a", ChangeStatus::M, 0, 2), change("b", ChangeStatus::A, 3, 0)])),
    ];
    let net = |r: &CardRange| (r.scope == "本会话").then(|| Ok(vec![change("a", ChangeStatus::M, 3, 0), change("b", ChangeStatus::A, 3, 0), change("hand.txt", ChangeStatus::M, 1, 1)]));
    let a = build(&Inputs { diff: &net, ..inputs(&turns, &records) });
    let n = a.net.as_ref().unwrap();
    assert_eq!(n.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
    assert_eq!(n.excluded, 1);
    assert_eq!(n.range.label, "本会话（第 2 轮前 → 第 3 轮后）");
    // R6: the end is the timeline turn's (210), which `Member::ended` prefers over the record's (205).
    assert_eq!(n.range_label, "@100 第 2 轮前 → @210 第 3 轮后");
    assert_eq!(a.summary, Some(Summary { files: 2, added: Some(6), removed: Some(0), computing: false }));
}

#[test]
fn before_the_net_is_in_the_summary_has_no_line_counts() {
    let records = [record(1, "改 a", 100, TurnState::Done, Some(vec![change("a", ChangeStatus::M, 5, 0)])), record(2, "改 b", 200, TurnState::Done, Some(vec![change("a", ChangeStatus::M, 2, 2)]))];
    let a = build(&inputs(&arcs([turn(1, "改 a", 100, Some(110)), turn(2, "改 b", 200, Some(210))]), &records));
    assert_eq!(a.summary, Some(Summary { files: 1, added: None, removed: None, computing: true }));
    assert!(a.net.as_ref().unwrap().computing);
}

#[test]
fn without_any_snapshot_pair_there_is_no_net() {
    let a = build(&inputs(&arcs([turn(1, "x", 100, Some(110))]), &[record(1, "x", 100, TurnState::NotGit, None)]));
    assert!(a.net.is_none());
    assert_eq!(a.summary, None);
}

#[test]
fn several_repositories_count_the_newest_only() {
    let mut other = record(1, "改 a", 100, TurnState::Done, Some(vec![change("a", ChangeStatus::M, 1, 0)]));
    other.repo_root = Some("/elsewhere/lib".into());
    let records = [other, record(2, "改 b", 200, TurnState::Done, Some(vec![change("b", ChangeStatus::M, 1, 0)]))];
    let a = build(&inputs(&arcs([turn(1, "改 a", 100, Some(110)), turn(2, "改 b", 200, Some(210))]), &records));
    let n = a.net.unwrap();
    assert_eq!((n.range.before.0.as_str(), n.range.after.0.as_str()), ("b2", "a2"));
    assert_eq!(n.only_repo.as_deref(), Some("repo"));
}

#[test]
fn arrows_walk_net_cards_and_quiet_groups() {
    let turns = arcs([turn(1, "改 a", 100, Some(110)), turn(2, "解释", 200, Some(210))]);
    let records = [record(1, "改 a", 100, TurnState::Done, Some(vec![change("a", ChangeStatus::M, 1, 0)])), record(2, "解释", 200, TurnState::Done, Some(vec![]))];
    let ok = |_: &CardRange| Some(Ok(vec![change("a", ChangeStatus::M, 1, 0)]));
    let a = build(&Inputs { diff: &ok, ..inputs(&turns, &records) });
    let closed = nav_rows(&a, &Open { net: false, card: &|_| false, all: &|_| false, quiet: &|_| false });
    assert_eq!(closed, vec![Sel::Net, Sel::Quiet(2), Sel::Card(1)]);
    let open = nav_rows(&a, &Open { net: true, card: &|c| c.key == 1, all: &|_| false, quiet: &|_| true });
    assert_eq!(open, vec![Sel::Net, Sel::NetFile(0), Sel::Quiet(2), Sel::Card(2), Sel::Card(1), Sel::File(1, 0)]);
}

#[test]
fn without_a_range_the_summary_still_counts_done_files() {
    let mut no_before = record(1, "改 a", 100, TurnState::Done, Some(vec![change("a", ChangeStatus::M, 5, 0)]));
    no_before.before = None;
    let a = build(&inputs(&arcs([turn(1, "改 a", 100, Some(110))]), &[no_before]));
    assert!(a.net.is_none());
    assert_eq!(a.summary, Some(Summary { files: 1, added: None, removed: None, computing: false }));
    // Several repositories, and the newest one has no Done `after` yet.
    let mut other = record(1, "改 a", 100, TurnState::Done, Some(vec![change("a", ChangeStatus::M, 1, 0)]));
    other.repo_root = Some("/elsewhere/lib".into());
    let mut running = record(2, "改 b", 200, TurnState::Running, None);
    running.after = None;
    running.ended_ms = None;
    let a = build(&inputs(&arcs([turn(1, "改 a", 100, Some(110)), turn(2, "改 b", 200, None)]), &[other, running]));
    assert!(a.net.is_none());
    assert_eq!(a.summary, Some(Summary { files: 1, added: None, removed: None, computing: false }));
}

#[test]
fn a_net_whose_files_are_all_excluded_still_has_a_summary() {
    let records = [record(1, "改 a", 100, TurnState::Done, Some(vec![change("a", ChangeStatus::M, 1, 0)]))];
    let hand = |_: &CardRange| Some(Ok(vec![change("hand.txt", ChangeStatus::M, 1, 1)]));
    let a = build(&Inputs { diff: &hand, ..inputs(&arcs([turn(1, "改 a", 100, Some(110))]), &records) });
    let n = a.net.as_ref().unwrap();
    assert_eq!((n.files.len(), n.excluded), (0, 1));
    assert_eq!(a.summary, Some(Summary { files: 0, added: Some(0), removed: Some(0), computing: false }));
}

#[test]
fn a_failed_net_leaves_the_summary_without_counts() {
    let records = [record(1, "改 a", 100, TurnState::Done, Some(vec![change("a", ChangeStatus::M, 1, 0)]))];
    let fail = |_: &CardRange| Some(Err("快照已清理".to_string()));
    let a = build(&Inputs { diff: &fail, ..inputs(&arcs([turn(1, "改 a", 100, Some(110))]), &records) });
    assert_eq!(a.net.as_ref().unwrap().failed.as_deref(), Some("快照已清理"));
    assert_eq!(a.summary, Some(Summary { files: 1, added: None, removed: None, computing: false }));
}

#[test]
fn the_net_keeps_a_rename_whose_old_path_a_turn_touched() {
    let records = [record(1, "改 old", 100, TurnState::Done, Some(vec![change("old.rs", ChangeStatus::M, 1, 0)]))];
    let renamed = |_: &CardRange| {
        let mut r = change("new.rs", ChangeStatus::R, 1, 0);
        r.old_path = Some("old.rs".into());
        Some(Ok(vec![r, change("hand.txt", ChangeStatus::M, 1, 0)]))
    };
    let a = build(&Inputs { diff: &renamed, ..inputs(&arcs([turn(1, "改 old", 100, Some(110))]), &records) });
    let n = a.net.as_ref().unwrap();
    assert_eq!(n.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["new.rs"]);
    assert_eq!(n.excluded, 1);
}

#[test]
fn the_live_list_counts_as_touched() {
    let mut running = record(2, "改 c", 200, TurnState::Running, None);
    running.after = None;
    running.ended_ms = None;
    let records = [record(1, "改 a", 100, TurnState::Done, Some(vec![change("a", ChangeStatus::M, 1, 0)])), running];
    let live = [change("c", ChangeStatus::A, 2, 0)];
    let diff = |_: &CardRange| Some(Ok(vec![change("a", ChangeStatus::M, 1, 0), change("c", ChangeStatus::A, 2, 0)]));
    let turns = arcs([turn(1, "改 a", 100, Some(110)), turn(2, "改 c", 200, None)]);
    let a = build(&Inputs { live: Some(&live), diff: &diff, ..inputs(&turns, &records) });
    assert_eq!(a.net.as_ref().unwrap().excluded, 0);
}

#[test]
fn an_all_quiet_session_is_one_group_with_nothing_open() {
    let turns = arcs([turn(1, "解释一下", 100, Some(110)), turn(2, "说说", 200, Some(210))]);
    let records = [record(1, "解释一下", 100, TurnState::Done, Some(vec![])), record(2, "说说", 200, TurnState::Done, Some(vec![]))];
    let a = build(&inputs(&turns, &records));
    assert_eq!(a.quiet_groups.len(), 1);
    assert_eq!(a.quiet_groups[0].cards, vec![2, 1]);
    assert_eq!(a.blocks, vec![Block::Quiet(0)]);
    assert_eq!(newest_open_by_default(&a), None);
}

#[test]
fn a_quiet_run_at_the_bottom_folds_too() {
    let turns = arcs([turn(1, "解释一下", 100, Some(110)), turn(2, "说说", 200, Some(210)), turn(3, "改 a", 300, Some(310))]);
    let records = [
        record(1, "解释一下", 100, TurnState::Done, Some(vec![])),
        record(2, "说说", 200, TurnState::Done, Some(vec![])),
        record(3, "改 a", 300, TurnState::Done, Some(vec![change("a", ChangeStatus::M, 1, 0)])),
    ];
    let a = build(&inputs(&turns, &records));
    assert_eq!(a.blocks, vec![Block::Card(0), Block::Quiet(0)]);
    assert_eq!(a.quiet_groups[0].cards, vec![2, 1]);
    assert_eq!(newest_open_by_default(&a), Some(3));
}
