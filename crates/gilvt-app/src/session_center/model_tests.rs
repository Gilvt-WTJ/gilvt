use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use gilvt_agent::{AgentKind, ReviewOutcome, Session, Status, TurnCursor};

use super::model::{self, LiveSession, Selection, Tab};
use crate::review::{ReviewInboxItem, ReviewPriority};

fn key(id: &str) -> gilvt_agent::SessionKey {
    (AgentKind::Claude, id.into())
}

fn inbox(id: &str, priority: ReviewPriority) -> ReviewInboxItem {
    ReviewInboxItem {
        key: key(id),
        title: format!("title {id}"),
        cwd: PathBuf::from(format!("/repo/{id}")),
        first_unreviewed: TurnCursor::Claude {
            prompt_uuid: format!("p-{id}"),
        },
        snapshot_through: TurnCursor::Claude {
            prompt_uuid: format!("p-{id}"),
        },
        unreviewed_count: 2,
        priority,
        pinned: false,
        running_in_gilvt: false,
        cursor_stale: false,
        search_terms: Vec::new(),
        first_completed_at: Some(SystemTime::UNIX_EPOCH),
        latest_completed_at: Some(SystemTime::UNIX_EPOCH),
        latest_outcome: ReviewOutcome::Done,
        tool_count: 3,
        failed_tool_count: 0,
        lines_added: 4,
        lines_removed: 1,
    }
}

fn live(id: &str, needs_you: bool, waiting: u64) -> LiveSession {
    LiveSession {
        key: key(id),
        pane: None,
        runtime: None,
        title: format!("live {id}"),
        cwd: PathBuf::from(format!("/repo/{id}")),
        needs_you,
        waiting_for: needs_you.then(|| Duration::from_secs(waiting)),
    }
}

#[test]
fn the_queue_search_finds_titles_and_the_words_the_session_started_with() {
    let mut item = inbox("a", ReviewPriority::Completed);
    item.title = "Calc 重构方案".into();
    item.search_terms = vec!["继续 原话关键字alpha".into(), "我的专属名".into()];
    let found = |query: &str| model::build(Tab::Review, query, 1, std::slice::from_ref(&item), &[]).rows.len();
    assert_eq!(found("重构"), 1, "the title");
    assert_eq!(found("ALPHA"), 1, "the first prompt, case-insensitive");
    assert_eq!(found("专属"), 1, "the agent's other title");
    assert_eq!(found("nomatch"), 0);
}

#[test]
fn a_row_carries_what_the_review_needs_from_its_inbox_item() {
    let mut stale = inbox("a", ReviewPriority::Failed);
    stale.cursor_stale = true;
    let rows = model::build(Tab::Review, "", 1, &[stale, inbox("b", ReviewPriority::Completed)], &[]).rows;
    assert!(rows[0].cursor_stale && !rows[1].cursor_stale);
    // The turn the queue showed as the last completed one: what "reviewed, next" will save.
    assert_eq!(
        rows[1].snapshot_through,
        Some(TurnCursor::Claude {
            prompt_uuid: "p-b".into()
        })
    );
    // A running session with nothing pending has neither.
    let running = model::build(Tab::Running, "", 1, &[], &[live("c", false, 0)]).rows;
    assert!(!running[0].cursor_stale && running[0].snapshot_through.is_none());
}

#[test]
fn tabs_are_disjoint_and_counts_are_global() {
    let inbox = vec![
        inbox("a", ReviewPriority::NeedsYou),
        inbox("b", ReviewPriority::Failed),
        inbox("c", ReviewPriority::Completed),
    ];
    let live = vec![live("a", true, 20), live("d", false, 0)];
    let needs = model::build(Tab::NeedsYou, "", 7, &inbox, &live);
    assert_eq!(
        needs
            .rows
            .iter()
            .map(|row| row.key.1.as_str())
            .collect::<Vec<_>>(),
        ["a"]
    );
    assert_eq!(
        (
            needs.counts.needs_you,
            needs.counts.review,
            needs.counts.running,
            needs.counts.all
        ),
        (1, 2, 2, 7)
    );
    let review = model::build(Tab::Review, "", 7, &inbox, &live);
    assert_eq!(
        review
            .rows
            .iter()
            .map(|row| row.key.1.as_str())
            .collect::<Vec<_>>(),
        ["b", "c"]
    );
    let running = model::build(Tab::Running, "", 7, &inbox, &live);
    assert_eq!(
        running
            .rows
            .iter()
            .map(|row| row.key.1.as_str())
            .collect::<Vec<_>>(),
        ["a", "d"]
    );
}

#[test]
fn needs_you_orders_longest_wait_first_and_query_checks_project_agent_and_id() {
    let live = vec![live("short", true, 2), live("long", true, 20)];
    let list = model::build(Tab::NeedsYou, "", 2, &[], &live);
    assert_eq!(
        list.rows
            .iter()
            .map(|row| row.key.1.as_str())
            .collect::<Vec<_>>(),
        ["long", "short"]
    );
    assert_eq!(
        model::build(Tab::NeedsYou, "repo/long", 2, &[], &live)
            .rows
            .len(),
        1
    );
    assert_eq!(
        model::build(Tab::NeedsYou, "claude", 2, &[], &live)
            .rows
            .len(),
        2
    );
    assert_eq!(
        model::build(Tab::NeedsYou, "sho", 2, &[], &live).rows[0]
            .key
            .1,
        "short"
    );
}

#[test]
fn selection_preserves_key_across_reorder_and_clamps_when_removed() {
    let mut selection = Selection::default();
    let first = model::build(
        Tab::Running,
        "",
        3,
        &[],
        &[
            live("a", false, 0),
            live("b", false, 0),
            live("c", false, 0),
        ],
    )
    .rows;
    selection.sync(&first, false);
    selection.step(&first, true);
    assert_eq!(selection.selected(&first).unwrap().key.1, "b");
    let reordered = vec![first[2].clone(), first[1].clone(), first[0].clone()];
    selection.sync(&reordered, true);
    assert_eq!(selection.selected(&reordered).unwrap().key.1, "b");
    selection.sync(&reordered[..1], true);
    assert_eq!(selection.cursor(), 0);
}

#[test]
fn registry_projection_includes_live_sessions_and_wait_duration() {
    let now = Instant::now();
    let mut waiting = Session::new(key("waiting"), now - Duration::from_secs(30));
    waiting.name = "approval".into();
    waiting.status = Status::NeedsApproval {
        action: "Bash".into(),
    };
    waiting.waiting_since = Some(now - Duration::from_secs(12));
    let mut ended = Session::new(key("ended"), now);
    ended.status = Status::Ended;
    let projected = model::live_sessions([&waiting, &ended], now);
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].waiting_for, Some(Duration::from_secs(12)));
}

#[test]
fn two_thousand_rows_are_deterministic() {
    let inbox = (0..2_000)
        .map(|i| inbox(&format!("{i:04}"), ReviewPriority::Completed))
        .collect::<Vec<_>>();
    let first = model::build(Tab::Review, "", inbox.len(), &inbox, &[]);
    let second = model::build(Tab::Review, "", inbox.len(), &inbox, &[]);
    assert_eq!(first, second);
    assert_eq!(first.rows.len(), 2_000);
}

#[test]
fn a_queue_row_shows_its_directory_as_the_palette_does() {
    let home = std::path::Path::new("/Users/u");
    let cwd = PathBuf::from("/Users/u/work/acme_web_monorepo/gilvt/crates/gilvt-app/src");
    let dir = model::row_dir(&cwd, Some(home), |_| true, |_| Some("main".into()));
    assert_eq!(
        dir.dir,
        crate::launcher::shorten_dir(&cwd, Some(home), crate::launcher::DIR_MAX_CHARS),
        "the palette's shortening"
    );
    assert!(dir.dir.starts_with("~/…/"));
    assert!(!dir.missing);
    let mut item = inbox("a", ReviewPriority::Completed);
    item.cwd = PathBuf::from("/Users/u/work/i27");
    item.failed_tool_count = 1;
    let row = &model::build(Tab::Review, "", 1, &[item], &[]).rows[0];
    let dir = model::row_dir(
        &row.cwd,
        Some(home),
        |_| true,
        |_| Some("i27-feature".into()),
    );
    assert_eq!(
        model::header_subtitle(row, &dir),
        "~/work/i27 · i27-feature · Claude"
    );
    assert_eq!(
        model::queue_subtitle(row, &dir),
        "~/work/i27 · i27-feature · Claude · 3 tools · 有失败 · +4 -1"
    );
    // No branch known: no branch segment.
    let dir = model::row_dir(&row.cwd, Some(home), |_| true, |_| None);
    assert_eq!(model::header_subtitle(row, &dir), "~/work/i27 · Claude");
}

#[test]
fn a_queue_row_whose_directory_is_gone_is_flagged_and_an_unknown_cwd_shows_none() {
    let gone = model::row_dir(
        std::path::Path::new("/Users/u/work/gone"),
        None,
        |_| false,
        |_| None,
    );
    assert!(gone.missing);
    assert_eq!(gone.dir, "/Users/u/work/gone");
    // A row without a known cwd (no history entry): no directory, never flagged.
    let unknown = model::row_dir(std::path::Path::new(""), None, |_| false, |_| None);
    assert_eq!(unknown, model::RowDir::default());
    let mut item = inbox("a", ReviewPriority::Completed);
    item.cwd = PathBuf::new();
    item.tool_count = 0;
    item.lines_added = 0;
    item.lines_removed = 0;
    let row = &model::build(Tab::Review, "", 1, &[item], &[]).rows[0];
    assert_eq!(model::queue_subtitle(row, &unknown), "Claude");
}
