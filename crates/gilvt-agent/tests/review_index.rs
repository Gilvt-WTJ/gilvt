mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::fixture;
use gilvt_agent::{
    parse_claude_review, parse_codex_review, AgentKind, HistoryIndex, ReviewOutcome, TurnCursor,
};

fn claude(id: &str) -> PathBuf {
    fixture("history/home/.claude/projects/-Users-u-gilvt-lab").join(format!("{id}.jsonl"))
}

#[test]
fn claude_review_uses_prompt_uuid_and_keeps_the_final_reply() {
    let path = claude("3c23fcb6-fb3f-4a54-a896-517c02276c0f");
    let review = parse_claude_review(&path).unwrap();
    assert_eq!(review.key.0, AgentKind::Claude);
    assert_eq!(review.key.1, "3c23fcb6-fb3f-4a54-a896-517c02276c0f");
    assert_eq!(review.transcript, path);
    assert!(review.incomplete_tail);
    assert!(review.scanned_through < review.transcript_size);
    assert_eq!(review.turns.len(), 1);
    let turn = &review.turns[0];
    assert_eq!(
        turn.cursor,
        TurnCursor::Claude {
            prompt_uuid: "912bf421-8aca-46cd-a103-ba8d9cd61b6d".into()
        }
    );
    assert_eq!(turn.ordinal, 1);
    assert_eq!(turn.outcome, ReviewOutcome::Done);
    assert!(turn.prompt_preview.starts_with("先用 Read 读 calc/calc.py"));
    assert!(!turn.reply_preview.is_empty());
    assert!(turn.tool_count >= 2);
    assert!(turn.end_offset <= review.scanned_through);
}

#[test]
fn codex_review_uses_turn_ids_and_indexes_all_completed_turns() {
    let path = fixture("history/home/.codex/sessions/2026/09/28")
        .join("rollout-2026-09-28T20-15-44-01a0eb29-4056-78f2-a118-864580a5a3fb.jsonl");
    let review = parse_codex_review(&path).unwrap();
    assert_eq!(review.key.0, AgentKind::Codex);
    assert_eq!(review.turns.len(), 3);
    assert_eq!(
        review
            .turns
            .iter()
            .map(|turn| turn.ordinal)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(
        review.turns[0].cursor,
        TurnCursor::Codex {
            turn_id: "01a0eba7-4b75-7da2-a928-20b1189202a2".into()
        }
    );
    assert!(review
        .turns
        .iter()
        .all(|turn| turn.outcome == ReviewOutcome::Done));
    assert!(review
        .turns
        .windows(2)
        .all(|pair| pair[0].end_offset <= pair[1].start_offset));
    assert!(!review.turns[0].reply_preview.is_empty());
}

#[test]
fn codex_failure_and_patch_summaries_are_bounded_index_data() {
    let complete = parse_codex_review(
        &fixture("history/home/.codex/sessions/2026/09/28")
            .join("rollout-2026-09-28T20-15-44-01a0eb29-4056-78f2-a118-864580a5a3fb.jsonl"),
    )
    .unwrap();
    assert_eq!(
        (
            complete.turns[0].lines_added,
            complete.turns[0].lines_removed
        ),
        (1, 0)
    );
    assert!(complete.turns[0].tool_count >= 2);

    let aborted = parse_codex_review(&fixture("codex/rollout-aborted.jsonl")).unwrap();
    assert_eq!(aborted.turns.len(), 1);
    assert_eq!(aborted.turns[0].outcome, ReviewOutcome::Interrupted);
    assert_eq!(aborted.turns[0].failed_tool_count, 0);
}

#[test]
fn history_cache_persists_the_review_projection() {
    let state = tempfile::tempdir().unwrap();
    let mut index = HistoryIndex::load(Some(state.path()));
    let entries = index.refresh(&fixture("history/home"));
    let reviews = index.review_sessions();
    assert_eq!(reviews.len(), entries.len());
    for review in &reviews {
        assert!(entries.iter().any(|entry| {
            entry.agent == review.key.0
                && entry.session_id == review.key.1
                && entry.transcript == review.transcript
        }));
    }
    assert!(reviews
        .iter()
        .any(|review| review.key.0 == AgentKind::Codex && review.turns.len() == 3));
    assert_eq!(
        HistoryIndex::load(Some(state.path())).review_sessions(),
        reviews
    );
    assert!(HistoryIndex::load(None)
        .refresh(Path::new("/nonexistent"))
        .is_empty());
}

#[test]
fn missing_prompt_id_uses_a_stable_fallback_and_ignores_a_partial_tail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let prompt = serde_json::json!({
        "type": "user",
        "isSidechain": false,
        "cwd": "/w",
        "timestamp": "2026-10-01T00:00:00Z",
        "message": {"role": "user", "content": "review me"}
    });
    let reply = serde_json::json!({
        "type": "assistant",
        "isSidechain": false,
        "timestamp": "2026-10-01T00:00:01Z",
        "message": {
            "role": "assistant",
            "model": "claude-test",
            "id": "m1",
            "content": [{"type": "text", "text": "finished"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 2, "output_tokens": 3}
        }
    });
    fs::write(&path, format!("{prompt}\n{reply}\n{{partial")).unwrap();

    let first = parse_claude_review(&path).unwrap();
    let second = parse_claude_review(&path).unwrap();
    assert!(first.incomplete_tail);
    assert_eq!(first.turns, second.turns);
    assert!(matches!(first.turns[0].cursor, TurnCursor::Fallback { .. }));
    assert_eq!(first.turns[0].reply_preview, "finished");
    assert_eq!(first.turns[0].tokens, 5);
}

#[test]
fn claude_api_error_is_a_failed_review_turn() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("failed.jsonl");
    let prompt = serde_json::json!({
        "type": "user", "uuid": "p1", "cwd": "/w", "timestamp": "2026-10-01T00:00:00Z",
        "message": {"content": "try it"}
    });
    let error = serde_json::json!({
        "type": "assistant", "isApiErrorMessage": true, "timestamp": "2026-10-01T00:00:01Z",
        "message": {"content": [{"type": "text", "text": "Network connection lost"}]}
    });
    fs::write(&path, format!("{prompt}\n{error}\n")).unwrap();
    let review = parse_claude_review(&path).unwrap();
    assert_eq!(
        review.turns[0].outcome,
        ReviewOutcome::Failed {
            message: "Network connection lost".into()
        }
    );
}
