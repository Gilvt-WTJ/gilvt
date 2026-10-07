mod common;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use common::fixture;
use gilvt_agent::{
    parse_claude_review, parse_codex_review, ReviewDocument, ReviewItem, ReviewPage,
    ReviewToolStatus, TurnCursor, REVIEW_PAGE_BYTES, REVIEW_PAGE_TURNS,
};

fn claude(id: &str) -> PathBuf {
    fixture("history/home/.claude/projects/-Users-u-gilvt-lab").join(format!("{id}.jsonl"))
}

#[test]
fn loads_claude_prompt_final_reply_thinking_and_tools() {
    let index = parse_claude_review(&claude("3c23fcb6-fb3f-4a54-a896-517c02276c0f")).unwrap();
    let document = ReviewDocument::load(&index, ReviewPage::Latest).unwrap();

    assert_eq!(
        document.snapshot_through,
        index.turns.last().unwrap().cursor
    );
    assert_eq!(document.turns.len(), 1);
    let turn = &document.turns[0];
    assert!(turn.prompt.starts_with("先用 Read 读 calc/calc.py"));
    assert!(!turn.final_reply.is_empty());
    assert!(turn
        .items
        .iter()
        .any(|item| matches!(item, ReviewItem::Tool(_))));
    assert!(turn
        .items
        .iter()
        .any(|item| matches!(item, ReviewItem::Thinking { .. })));
    assert!(turn.truncation.is_none());
}

#[test]
fn loads_codex_tools_outputs_and_final_reply() {
    let path = fixture("history/home/.codex/sessions/2026/09/28")
        .join("rollout-2026-09-28T20-15-44-01a0eb29-4056-78f2-a118-864580a5a3fb.jsonl");
    let index = parse_codex_review(&path).unwrap();
    let document = ReviewDocument::load(&index, ReviewPage::After { cursor: None }).unwrap();

    assert_eq!(document.turns.len(), 3);
    assert!(!document.turns[0].final_reply.is_empty());
    let tools = document.turns[0]
        .items
        .iter()
        .filter_map(|item| match item {
            ReviewItem::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!tools.is_empty());
    assert!(tools.iter().any(|tool| tool.status == ReviewToolStatus::Ok));
    assert!(tools.iter().any(|tool| !tool.detail.input.is_empty()));
}

#[test]
fn interrupted_turn_without_a_final_reply_is_still_reviewable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aborted.jsonl");
    let records = [
        serde_json::json!({
            "type":"session_meta", "timestamp":"2026-10-01T00:00:00Z",
            "payload":{"type":"session_meta", "id":"s", "cwd":"/w", "source":"cli"}
        }),
        serde_json::json!({
            "type":"event_msg", "timestamp":"2026-10-01T00:00:01Z",
            "payload":{"type":"task_started", "turn_id":"t0"}
        }),
        serde_json::json!({
            "type":"event_msg", "timestamp":"2026-10-01T00:00:02Z",
            "payload":{"type":"user_message", "message":"stop me"}
        }),
        serde_json::json!({
            "type":"event_msg", "timestamp":"2026-10-01T00:00:03Z",
            "payload":{"type":"turn_aborted", "turn_id":"t0", "reason":"interrupted"}
        }),
    ];
    fs::write(
        &path,
        records
            .iter()
            .map(|record| format!("{record}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let index = parse_codex_review(&path).unwrap();
    let document = ReviewDocument::load(&index, ReviewPage::Latest).unwrap();
    assert_eq!(document.turns.len(), 1);
    assert!(document.turns[0].final_reply.is_empty());
    assert_eq!(
        document.turns[0].outcome,
        gilvt_agent::ReviewOutcome::Interrupted
    );
}

#[test]
fn pages_forward_and_backward_by_stable_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("many.jsonl");
    let mut body = String::new();
    for n in 0..(REVIEW_PAGE_TURNS + 2) {
        body.push_str(&claude_turn(
            n,
            &format!("prompt {n}"),
            &format!("reply {n}"),
        ));
    }
    fs::write(&path, body).unwrap();
    let index = parse_claude_review(&path).unwrap();

    let first = ReviewDocument::load(&index, ReviewPage::After { cursor: None }).unwrap();
    assert_eq!(first.turns.len(), REVIEW_PAGE_TURNS);
    assert!(!first.has_earlier);
    assert!(first.has_later);

    let next = ReviewDocument::load(
        &index,
        ReviewPage::After {
            cursor: first.turns.last().map(|turn| turn.cursor.clone()),
        },
    )
    .unwrap();
    assert_eq!(next.turns.len(), 2);
    assert!(next.has_earlier);
    assert!(!next.has_later);

    let previous = ReviewDocument::load(
        &index,
        ReviewPage::Before {
            cursor: next.turns[0].cursor.clone(),
        },
    )
    .unwrap();
    assert_eq!(previous.turns.len(), REVIEW_PAGE_TURNS);
    assert_eq!(previous.turns[0].ordinal, 1);
}

#[test]
fn append_after_open_is_outside_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("append.jsonl");
    fs::write(&path, claude_turn(0, "first", "old reply")).unwrap();
    let index = parse_claude_review(&path).unwrap();
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(claude_turn(1, "second", "new reply").as_bytes())
        .unwrap();

    let document = ReviewDocument::load(&index, ReviewPage::Latest).unwrap();
    assert_eq!(document.turns.len(), 1);
    assert_eq!(document.turns[0].final_reply, "old reply");
    assert_eq!(document.snapshot_through, index.turns[0].cursor);
}

#[test]
fn repeated_assistant_message_chunks_are_ordered_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chunks.jsonl");
    let prompt = serde_json::json!({
        "type": "user", "uuid": "p0", "cwd": "/w", "timestamp": "2026-10-01T00:00:00Z",
        "message": {"content": "prompt"}
    });
    let first = serde_json::json!({
        "type": "assistant", "apiBlockIndex": 0, "timestamp": "2026-10-01T00:00:01Z",
        "message": {"id": "m1", "model": "claude", "content": [{"type":"text", "text":"first"}],
                    "usage": {"input_tokens":1, "output_tokens":1}}
    });
    let duplicate = first.clone();
    let second = serde_json::json!({
        "type": "assistant", "apiBlockIndex": 1, "timestamp": "2026-10-01T00:00:02Z",
        "message": {"id": "m1", "model": "claude", "stop_reason":"end_turn",
                    "content": [{"type":"text", "text":"second"}],
                    "usage": {"input_tokens":1, "output_tokens":2}}
    });
    fs::write(&path, format!("{prompt}\n{first}\n{duplicate}\n{second}\n")).unwrap();
    let index = parse_claude_review(&path).unwrap();
    let document = ReviewDocument::load(&index, ReviewPage::Latest).unwrap();
    assert_eq!(document.turns[0].final_reply, "first\nsecond");
}

#[test]
fn oversized_turn_is_a_placeholder_and_unknown_blocks_are_counted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.jsonl");
    let prompt = serde_json::json!({
        "type": "user", "uuid": "p0", "cwd": "/w", "timestamp": "2026-10-01T00:00:00Z",
        "message": {"content": "large"}
    });
    let huge = "x".repeat(REVIEW_PAGE_BYTES + 1024);
    let assistant = serde_json::json!({
        "type": "assistant", "timestamp": "2026-10-01T00:00:01Z",
        "message": {"id":"m0", "model":"claude", "stop_reason":"end_turn",
                    "content":[{"type":"future_block", "value": huge}, {"type":"text", "text":"done"}]}
    });
    fs::write(&path, format!("{prompt}\n{assistant}\n")).unwrap();
    let index = parse_claude_review(&path).unwrap();
    let document = ReviewDocument::load(&index, ReviewPage::Latest).unwrap();
    let turn = &document.turns[0];
    assert!(turn.truncation.is_some());
    assert!(turn.prompt.starts_with("large"));
    assert!(document.bytes_read <= REVIEW_PAGE_BYTES);

    let small = dir.path().join("unknown.jsonl");
    let assistant = serde_json::json!({
        "type": "assistant", "timestamp": "2026-10-01T00:00:01Z",
        "message": {"id":"m0", "model":"claude", "stop_reason":"end_turn",
                    "content":[{"type":"future_block"}, {"type":"text", "text":"done"}]}
    });
    fs::write(&small, format!("{prompt}\n{assistant}\n")).unwrap();
    let index = parse_claude_review(&small).unwrap();
    let document = ReviewDocument::load(&index, ReviewPage::Latest).unwrap();
    assert_eq!(document.compatibility.unknown_blocks, 1);
    assert_eq!(document.turns[0].final_reply, "done");
}

#[test]
fn missing_or_rewritten_snapshot_returns_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("changed.jsonl");
    fs::write(&path, claude_turn(0, "first", "reply")).unwrap();
    let index = parse_claude_review(&path).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(ReviewDocument::load(&index, ReviewPage::Latest).is_err());

    fs::write(&path, claude_turn(0, "first", "reply")).unwrap();
    let index = parse_claude_review(&path).unwrap();
    let original_len = fs::metadata(&path).unwrap().len();
    let replacement = claude_turn(0, "other", "reply");
    assert_eq!(replacement.len() as u64, original_len);
    fs::write(&path, replacement).unwrap();
    assert!(ReviewDocument::load(&index, ReviewPage::Latest).is_err());
}

fn claude_turn(n: usize, prompt: &str, reply: &str) -> String {
    let user = serde_json::json!({
        "type": "user",
        "uuid": format!("p{n}"),
        "cwd": "/w",
        "timestamp": format!("2026-10-01T00:00:{:02}Z", n * 2),
        "message": {"content": prompt}
    });
    let assistant = serde_json::json!({
        "type": "assistant",
        "timestamp": format!("2026-10-01T00:00:{:02}Z", n * 2 + 1),
        "message": {
            "id": format!("m{n}"),
            "model": "claude-test",
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": reply}],
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }
    });
    format!("{user}\n{assistant}\n")
}

#[test]
fn unreviewed_page_starts_after_the_saved_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cursor.jsonl");
    fs::write(
        &path,
        format!(
            "{}{}{}",
            claude_turn(0, "zero", "r0"),
            claude_turn(1, "one", "r1"),
            claude_turn(2, "two", "r2")
        ),
    )
    .unwrap();
    let index = parse_claude_review(&path).unwrap();
    let cursor = TurnCursor::Claude {
        prompt_uuid: "p0".into(),
    };
    let document = ReviewDocument::load(
        &index,
        ReviewPage::After {
            cursor: Some(cursor),
        },
    )
    .unwrap();
    assert_eq!(
        document
            .turns
            .iter()
            .map(|turn| turn.prompt.as_str())
            .collect::<Vec<_>>(),
        ["one", "two"]
    );
}
