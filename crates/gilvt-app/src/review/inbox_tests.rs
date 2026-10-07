use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use gilvt_agent::{
    AgentKind, HistoryEntry, ReviewOutcome, ReviewSessionIndex, ReviewState, ReviewTurnIndex,
    TurnCursor,
};

use super::inbox::{build_inbox, ReviewPriority, ReviewRuntime, ReviewSort};

fn key(n: usize) -> (AgentKind, String) {
    (AgentKind::Claude, format!("s{n:04}"))
}

fn cursor(n: usize) -> TurnCursor {
    TurnCursor::Claude {
        prompt_uuid: format!("p{n}"),
    }
}

fn turn(n: usize, at: u64, outcome: ReviewOutcome) -> ReviewTurnIndex {
    ReviewTurnIndex {
        cursor: cursor(n),
        ordinal: n as u32 + 1,
        start_offset: n as u64 * 10,
        end_offset: n as u64 * 10 + 10,
        fingerprint: format!("f{n}"),
        prompt_preview: format!("prompt {n}"),
        reply_preview: format!("reply {n}"),
        started_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(at - 1)),
        completed_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(at)),
        outcome,
        tool_count: 1,
        failed_tool_count: 0,
        lines_added: 2,
        lines_removed: 1,
        tokens: 10,
    }
}

fn review(n: usize, turns: Vec<ReviewTurnIndex>) -> ReviewSessionIndex {
    ReviewSessionIndex {
        key: key(n),
        transcript: PathBuf::from(format!("/w/s{n}.jsonl")),
        transcript_size: turns.last().map_or(0, |turn| turn.end_offset),
        scanned_through: turns.last().map_or(0, |turn| turn.end_offset),
        incomplete_tail: false,
        turns,
    }
}

fn history(n: usize) -> HistoryEntry {
    HistoryEntry {
        agent: AgentKind::Claude,
        session_id: key(n).1,
        cwd: PathBuf::from(format!("/work/p{}", n % 3)),
        transcript: PathBuf::from(format!("/w/s{n}.jsonl")),
        first_prompt: format!("session {n}"),
        topic_prompt: String::new(),
        custom_title: None,
        ai_title: None,
        started: None,
        last_active: SystemTime::UNIX_EPOCH,
        turns: 1,
        model: None,
        size: 0,
    }
}

#[test]
fn cursor_snooze_and_aggregates_define_the_pending_slice() {
    let reviews = vec![review(
        0,
        vec![
            turn(0, 10, ReviewOutcome::Done),
            turn(1, 20, ReviewOutcome::Done),
            turn(2, 30, ReviewOutcome::Done),
        ],
    )];
    let mut states = HashMap::new();
    states.insert(
        key(0),
        ReviewState {
            reviewed_through: Some(cursor(0)),
            pinned: true,
            ..ReviewState::default()
        },
    );
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    let items = build_inbox(
        &[history(0)],
        &reviews,
        &states,
        &HashMap::new(),
        now,
        ReviewSort::Smart,
    );
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].unreviewed_count, 2);
    assert_eq!(items[0].first_unreviewed, cursor(1));
    assert_eq!(items[0].snapshot_through, cursor(2));
    assert_eq!(
        (
            items[0].tool_count,
            items[0].lines_added,
            items[0].lines_removed
        ),
        (2, 4, 2)
    );

    states.get_mut(&key(0)).unwrap().snoozed_until = Some(now + Duration::from_secs(1));
    assert!(build_inbox(
        &[history(0)],
        &reviews,
        &states,
        &HashMap::new(),
        now,
        ReviewSort::Smart
    )
    .is_empty());
    assert_eq!(
        build_inbox(
            &[history(0)],
            &reviews,
            &states,
            &HashMap::new(),
            now + Duration::from_secs(1),
            ReviewSort::Smart,
        )
        .len(),
        1
    );
}

#[test]
fn smart_sort_uses_buckets_pin_time_and_stable_key() {
    let reviews = vec![
        review(0, vec![turn(0, 40, ReviewOutcome::Done)]),
        review(
            1,
            vec![turn(
                0,
                30,
                ReviewOutcome::Failed {
                    message: "x".into(),
                },
            )],
        ),
        review(2, vec![turn(0, 20, ReviewOutcome::Done)]),
        review(3, vec![turn(0, 10, ReviewOutcome::Done)]),
        review(4, vec![turn(0, 5, ReviewOutcome::Done)]),
    ];
    let histories = (0..5).map(history).collect::<Vec<_>>();
    let mut states = HashMap::new();
    states.insert(
        key(3),
        ReviewState {
            pinned: true,
            ..ReviewState::default()
        },
    );
    states.insert(
        key(4),
        ReviewState {
            reviewed_through: Some(TurnCursor::Claude {
                prompt_uuid: "gone".into(),
            }),
            ..ReviewState::default()
        },
    );
    let runtime = HashMap::from([
        (
            key(0),
            ReviewRuntime {
                running_in_gilvt: true,
                needs_you: true,
                waiting_for: Some(Duration::from_secs(5)),
            },
        ),
        (
            key(2),
            ReviewRuntime {
                running_in_gilvt: true,
                ..ReviewRuntime::default()
            },
        ),
    ]);
    let items = build_inbox(
        &histories,
        &reviews,
        &states,
        &runtime,
        SystemTime::UNIX_EPOCH + Duration::from_secs(100),
        ReviewSort::Smart,
    );
    assert_eq!(
        items
            .iter()
            .map(|item| item.key.clone())
            .collect::<Vec<_>>(),
        [key(0), key(4), key(1), key(3), key(2)]
    );
    assert_eq!(items[0].priority, ReviewPriority::NeedsYou);
    assert!(items[1].cursor_stale);
    assert_eq!(items[1].priority, ReviewPriority::Failed);
    assert_eq!(items[4].priority, ReviewPriority::RunningWithResults);
}

#[test]
fn alternate_sorts_are_deterministic_for_two_thousand_sessions() {
    let histories = (0..2000).map(history).collect::<Vec<_>>();
    let reviews = (0..2000)
        .map(|n| review(n, vec![turn(0, (n % 17) as u64 + 1, ReviewOutcome::Done)]))
        .collect::<Vec<_>>();
    for sort in [ReviewSort::Recent, ReviewSort::Project, ReviewSort::Oldest] {
        let first = build_inbox(
            &histories,
            &reviews,
            &HashMap::new(),
            &HashMap::new(),
            SystemTime::UNIX_EPOCH,
            sort,
        );
        let second = build_inbox(
            &histories,
            &reviews,
            &HashMap::new(),
            &HashMap::new(),
            SystemTime::UNIX_EPOCH,
            sort,
        );
        assert_eq!(first, second);
    }
}

#[test]
fn a_queue_row_is_named_with_the_agents_title_not_the_first_prompt() {
    let reviews = [review(0, vec![turn(0, 10, ReviewOutcome::Done)])];
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    let title_of = |entry: HistoryEntry| {
        build_inbox(
            &[entry],
            &reviews,
            &HashMap::new(),
            &HashMap::new(),
            now,
            ReviewSort::Smart,
        )[0]
        .title
        .clone()
    };
    let mut entry = history(0);
    entry.first_prompt = "继续".into();
    entry.topic_prompt = "整理 README 的结构".into();
    assert_eq!(title_of(entry.clone()), "整理 README 的结构");
    entry.ai_title = Some("整理文档".into());
    assert_eq!(title_of(entry.clone()), "整理文档");
    entry.custom_title = Some("我的名字".into());
    assert_eq!(title_of(entry), "我的名字");
}

#[test]
fn an_archived_session_leaves_the_inbox_until_a_new_turn_arrives() {
    let reviews = vec![review(0, vec![turn(0, 10, ReviewOutcome::Done)])];
    let mut entry = history(0);
    entry.turns = 3;
    let mut states = HashMap::new();
    states.insert(
        key(0),
        ReviewState {
            archived_at: Some(SystemTime::UNIX_EPOCH),
            archived_turns: 3,
            ..ReviewState::default()
        },
    );
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    let run = |entries: &[HistoryEntry]| {
        build_inbox(
            entries,
            &reviews,
            &states,
            &HashMap::new(),
            now,
            ReviewSort::Smart,
        )
    };
    assert!(run(&[entry.clone()]).is_empty());
    entry.turns = 4;
    assert_eq!(run(&[entry]).len(), 1, "a new turn brings it back");
    assert_eq!(run(&[]).len(), 1, "no history entry counts as not archived");
}
