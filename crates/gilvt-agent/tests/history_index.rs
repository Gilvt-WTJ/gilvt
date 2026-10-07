//! The history index over a copy of the fixture home: ordering, the (size, mtime) cache and its file.

mod common;

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::fixture;
use gilvt_agent::{parse_claude_review, parse_codex_review, HistoryEntry, HistoryIndex};
use serde_json::Value;

const PROJECT: &str = ".claude/projects/-Users-u-gilvt-lab";
const SHORT: &str = "3c23fcb6-fb3f-4a54-a896-517c02276c0f";

/// The five kept sessions of the fixture home, newest `last_active` first.
const ORDER: [&str; 5] = [
    "01a0eb29-4056-78f2-a118-864580a5a3fb",
    "feadcb4e-1c64-42c1-bfba-5611528a574d",
    SHORT,
    "be216f9f-69a9-4d73-9e98-738fb2dfabfa",
    "7e9f0ca8-1dfe-45bf-8c81-1e6dc0ae4b60",
];

fn ids(entries: &[HistoryEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.session_id.as_str()).collect()
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A temp dir holding `home/` (a copy of the fixture home) and `state/`.
fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    copy_dir(&fixture("history/home"), &home);
    let state = dir.path().join("state");
    (dir, home, state)
}

fn mtime(path: &Path) -> SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

fn set_mtime(path: &Path, at: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

/// Same length, nothing parseable.
fn scribble(path: &Path) {
    let at = mtime(path);
    let len = fs::metadata(path).unwrap().len() as usize;
    fs::write(path, format!("{}\n", "x".repeat(len - 1))).unwrap();
    set_mtime(path, at);
}

fn cache(state: &Path) -> Value {
    serde_json::from_slice(&fs::read(state.join("history.json")).unwrap()).unwrap()
}

#[test]
fn lists_interactive_sessions_newest_first() {
    let mut index = HistoryIndex::load(None);
    let entries = index.refresh(&fixture("history/home"));
    assert_eq!(ids(&entries), ORDER);
    assert_eq!(index.entries(), entries);
    assert!(HistoryIndex::load(None)
        .refresh(Path::new("/nonexistent"))
        .is_empty());
}

#[test]
fn unchanged_files_come_from_the_cache() {
    let (_dir, home, state) = setup();
    let first = HistoryIndex::load(Some(&state)).refresh(&home);
    assert!(state.join("history.json").is_file());
    assert!(!state.join("history.json.tmp").exists());
    // Unparsable now, but size and mtime are the same: not read again, in this index or a reloaded one.
    let short = home.join(PROJECT).join(format!("{SHORT}.jsonl"));
    scribble(&short);
    let rollout = home.join(".codex/sessions/2026/09/28/rollout-2026-09-28T20-15-44-01a0eb29-4056-78f2-a118-864580a5a3fb.jsonl");
    scribble(&rollout);
    let mut index = HistoryIndex::load(Some(&state));
    assert_eq!(index.entries(), first, "loaded from the file");
    assert_eq!(index.refresh(&home), first);
}

#[test]
fn changed_files_are_parsed_again() {
    let (_dir, home, state) = setup();
    let mut index = HistoryIndex::load(Some(&state));
    index.refresh(&home);
    let short = home.join(PROJECT).join(format!("{SHORT}.jsonl"));

    // Size: a complete line appended (the fixture ends with a half line; finish it first).
    let prompt = r#"{"type":"user","isSidechain":false,"cwd":"/Users/u/gilvt-lab","timestamp":"2026-09-30T00:00:00Z","message":{"role":"user","content":"再来一次"}}"#;
    let text = fs::read_to_string(&short).unwrap();
    fs::write(&short, format!("{text}\n{prompt}\n")).unwrap();
    let entries = index.refresh(&home);
    assert_eq!(entries[0].session_id, SHORT, "now the newest");
    assert_eq!(entries[0].turns, 2);

    // Mtime alone: same size, but unparsable and touched → reparsed, and now excluded.
    let at = mtime(&short) + Duration::from_secs(5);
    scribble(&short);
    set_mtime(&short, at);
    let entries = index.refresh(&home);
    assert!(!ids(&entries).contains(&SHORT));
    let saved = cache(&state);
    let key = short.to_str().unwrap();
    assert_eq!(
        saved["files"][key]["entry"],
        Value::Null,
        "cached as excluded"
    );
}

#[test]
fn vanished_files_are_dropped() {
    let (_dir, home, state) = setup();
    let mut index = HistoryIndex::load(Some(&state));
    index.refresh(&home);
    let short = home.join(PROJECT).join(format!("{SHORT}.jsonl"));
    assert!(cache(&state)["files"]
        .get(short.to_str().unwrap())
        .is_some());
    fs::remove_file(&short).unwrap();
    let entries = index.refresh(&home);
    assert_eq!(entries.len(), ORDER.len() - 1);
    assert!(!ids(&entries).contains(&SHORT));
    assert!(
        cache(&state)["files"]
            .get(short.to_str().unwrap())
            .is_none(),
        "saved without it"
    );
}

#[test]
fn forget_drops_and_saves() {
    let (_dir, home, state) = setup();
    let mut index = HistoryIndex::load(Some(&state));
    let entries = index.refresh(&home);
    let gone = entries[0].transcript.clone();
    index.forget(&gone);
    index.forget(Path::new("/not/indexed.jsonl"));
    assert_eq!(ids(&index.entries()), ORDER[1..]);
    assert!(cache(&state)["files"].get(gone.to_str().unwrap()).is_none());
}

#[test]
fn other_versions_and_broken_files_are_discarded() {
    let (_dir, home, state) = setup();
    HistoryIndex::load(Some(&state)).refresh(&home);
    let short = home.join(PROJECT).join(format!("{SHORT}.jsonl"));
    let key = short.to_str().unwrap().to_string();
    let mut saved = cache(&state);
    saved["files"][&key]["entry"]["first_prompt"] = "from the cache".into();
    let write =
        |v: &Value| fs::write(state.join("history.json"), serde_json::to_vec(v).unwrap()).unwrap();
    let prompt_of = |entries: Vec<HistoryEntry>| {
        entries
            .into_iter()
            .find(|e| e.session_id == SHORT)
            .map(|e| e.first_prompt)
            .unwrap()
    };

    write(&saved);
    assert_eq!(
        prompt_of(HistoryIndex::load(Some(&state)).refresh(&home)),
        "from the cache"
    );

    saved["version"] = 999.into();
    write(&saved);
    assert!(HistoryIndex::load(Some(&state)).entries().is_empty());
    let real = prompt_of(HistoryIndex::load(Some(&state)).refresh(&home));
    assert!(real.starts_with("先用 Read 读 calc/calc.py"), "{real}");
    assert_eq!(
        cache(&state)["version"],
        5,
        "rewritten in the current version"
    );

    fs::write(state.join("history.json"), "{broken").unwrap();
    let mut index = HistoryIndex::load(Some(&state));
    assert!(index.entries().is_empty());
    assert_eq!(ids(&index.refresh(&home)), ORDER);
}

#[test]
fn claude_append_is_merged_but_rewrite_and_truncate_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = home.join(".claude/projects/-w");
    fs::create_dir_all(&project).unwrap();
    let path = project.join("session.jsonl");
    let state = dir.path().join("state");
    let first = claude_turn(0, "alpha", "r0");
    let second = claude_turn(1, "second", "r1");
    let third = claude_turn(2, "third", "r2");
    fs::write(&path, format!("{first}{second}")).unwrap();
    let mut index = HistoryIndex::load(Some(&state));
    index.refresh(&home);
    let before = index.review_sessions();
    assert_eq!(before[0].turns.len(), 2);

    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(third.as_bytes())
        .unwrap();
    index.refresh(&home);
    assert_eq!(
        index.review_sessions()[0],
        parse_claude_review(&path).unwrap()
    );

    let rewritten = format!("{}{second}{third}", claude_turn(0, "omega", "r0"));
    fs::write(&path, rewritten).unwrap();
    index.refresh(&home);
    assert_eq!(
        index
            .entries()
            .into_iter()
            .find(|entry| entry.session_id == "session")
            .unwrap()
            .first_prompt,
        "omega"
    );

    fs::write(&path, claude_turn(0, "short", "only")).unwrap();
    index.refresh(&home);
    assert_eq!(index.review_sessions()[0].turns.len(), 1);
}

#[test]
fn codex_append_matches_a_full_parse_including_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let sessions = home.join(".codex/sessions/2026/10/01");
    fs::create_dir_all(&sessions).unwrap();
    let path = sessions.join("rollout-session.jsonl");
    let state = dir.path().join("state");
    let meta = serde_json::json!({
        "type":"session_meta", "timestamp":"2026-10-01T00:00:00Z",
        "payload":{"id":"codex-session", "cwd":"/w", "source":"cli"}
    });
    fs::write(
        &path,
        format!("{meta}\n{}{}", codex_turn(0, 10), codex_turn(1, 25)),
    )
    .unwrap();
    let mut index = HistoryIndex::load(Some(&state));
    index.refresh(&home);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(codex_turn(2, 40).as_bytes())
        .unwrap();

    index.refresh(&home);
    assert_eq!(
        index.review_sessions()[0],
        parse_codex_review(&path).unwrap()
    );
    assert_eq!(
        index.review_sessions()[0]
            .turns
            .iter()
            .map(|turn| turn.tokens)
            .collect::<Vec<_>>(),
        [10, 15, 15]
    );
}

fn claude_turn(n: usize, prompt: &str, reply: &str) -> String {
    let user = serde_json::json!({
        "type":"user", "uuid":format!("p{n}"), "cwd":"/w",
        "timestamp":format!("2026-10-01T00:00:{:02}Z", n * 2),
        "message":{"content":prompt}
    });
    let assistant = serde_json::json!({
        "type":"assistant", "timestamp":format!("2026-10-01T00:00:{:02}Z", n * 2 + 1),
        "message":{"id":format!("m{n}"), "model":"claude", "stop_reason":"end_turn",
                   "content":[{"type":"text", "text":reply}],
                   "usage":{"input_tokens":1, "output_tokens":1}}
    });
    format!("{user}\n{assistant}\n")
}

fn codex_turn(n: usize, total_tokens: u64) -> String {
    let turn_id = format!("t{n}");
    let records = [
        serde_json::json!({
            "type":"event_msg", "timestamp":format!("2026-10-01T00:00:{:02}Z", n * 3 + 1),
            "payload":{"type":"task_started", "turn_id":turn_id}
        }),
        serde_json::json!({
            "type":"event_msg", "timestamp":format!("2026-10-01T00:00:{:02}Z", n * 3 + 2),
            "payload":{"type":"user_message", "message":format!("prompt {n}")}
        }),
        serde_json::json!({
            "type":"event_msg", "timestamp":format!("2026-10-01T00:00:{:02}Z", n * 3 + 3),
            "payload":{"type":"token_count", "info":{"total_token_usage":{"total_tokens":total_tokens}}}
        }),
        serde_json::json!({
            "type":"event_msg", "timestamp":format!("2026-10-01T00:00:{:02}Z", n * 3 + 3),
            "payload":{"type":"task_complete", "turn_id":turn_id, "last_agent_message":format!("reply {n}")}
        }),
    ];
    records.iter().map(|record| format!("{record}\n")).collect()
}
