//! The titles the agents keep for their own sessions (spec 2026-10-02): Claude's `ai-title` / `custom-title`
//! records (the last of each wins), Codex's `session_index.jsonl`, and the first informative prompt.

mod common;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use common::fixture;
use gilvt_agent::{parse_claude_session, AgentKind, HistoryEntry, HistoryIndex};

fn claude_turn(n: usize, prompt: &str) -> String {
    let user = serde_json::json!({
        "type":"user", "uuid":format!("p{n}"), "cwd":"/w",
        "timestamp":format!("2026-10-01T00:00:{:02}Z", n * 2),
        "message":{"content":prompt}
    });
    let assistant = serde_json::json!({
        "type":"assistant", "timestamp":format!("2026-10-01T00:00:{:02}Z", n * 2 + 1),
        "message":{"id":format!("m{n}"), "model":"claude", "stop_reason":"end_turn",
                   "content":[{"type":"text", "text":"ok"}],
                   "usage":{"input_tokens":1, "output_tokens":1}}
    });
    format!("{user}\n{assistant}\n")
}

fn ai(title: &str) -> String {
    format!("{}\n", serde_json::json!({"type":"ai-title","aiTitle":title,"sessionId":"s"}))
}

fn custom(title: &str) -> String {
    format!("{}\n", serde_json::json!({"type":"custom-title","customTitle":title,"sessionId":"s"}))
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, text).unwrap();
    path
}

fn append(path: &Path, text: &str) {
    fs::OpenOptions::new().append(true).open(path).unwrap().write_all(text.as_bytes()).unwrap();
}

fn entry(index: &HistoryIndex, id: &str) -> HistoryEntry {
    index.entries().into_iter().find(|e| e.session_id == id).unwrap_or_else(|| panic!("no {id}"))
}

#[test]
fn claude_titles_are_the_last_of_each_kind() {
    let dir = tempfile::tempdir().unwrap();
    let text = format!(
        "{}{}{}{}{}",
        claude_turn(0, "继续"),
        ai("第一个标题"),
        claude_turn(1, "整理 README 的结构"),
        ai("最终的标题"),
        custom("我改的名字")
    );
    let path = write(dir.path(), "s.jsonl", &text);
    let e = parse_claude_session(&path).unwrap();
    assert_eq!(e.ai_title.as_deref(), Some("最终的标题"));
    assert_eq!(e.custom_title.as_deref(), Some("我改的名字"));
    assert_eq!(e.first_prompt, "继续");
    // 「继续」 is an answer, not a topic.
    assert_eq!(e.topic_prompt, "整理 README 的结构");
}

#[test]
fn without_titles_the_fields_are_empty_and_the_topic_is_the_first_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "s.jsonl", &claude_turn(0, "fix the login redirect loop"));
    let e = parse_claude_session(&path).unwrap();
    assert_eq!((e.ai_title, e.custom_title), (None, None));
    assert_eq!(e.topic_prompt, "fix the login redirect loop");
    // No informative prompt at all: no topic.
    let path = write(dir.path(), "t.jsonl", &claude_turn(0, "继续"));
    assert_eq!(parse_claude_session(&path).unwrap().topic_prompt, "");
}

#[test]
fn blank_and_multi_line_titles_are_normalized() {
    let dir = tempfile::tempdir().unwrap();
    let text = format!("{}{}{}", claude_turn(0, "整理 README"), ai("  Session  管理开发 \nsecond"), custom("   "));
    let e = parse_claude_session(&write(dir.path(), "s.jsonl", &text)).unwrap();
    assert_eq!(e.ai_title.as_deref(), Some("Session 管理开发"));
    assert_eq!(e.custom_title, None, "a blank title is no title");
}

#[test]
fn a_title_written_later_arrives_with_an_append_and_an_old_one_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = home.join(".claude/projects/-w");
    fs::create_dir_all(&project).unwrap();
    let path = write(&project, "s.jsonl", &format!("{}{}", claude_turn(0, "整理 README"), claude_turn(1, "再补测试")));
    let mut index = HistoryIndex::load(Some(&dir.path().join("state")));
    index.refresh(&home);
    assert_eq!(entry(&index, "s").ai_title, None);

    // The agent names the session after the next turn.
    append(&path, &ai("整理文档"));
    append(&path, &claude_turn(2, "第三个"));
    index.refresh(&home);
    assert_eq!(entry(&index, "s").ai_title.as_deref(), Some("整理文档"));

    // A turn with no title record keeps the title the index already had.
    append(&path, &claude_turn(3, "第四个"));
    index.refresh(&home);
    assert_eq!(entry(&index, "s").ai_title.as_deref(), Some("整理文档"));

    // A newer title replaces it, and a custom one is picked up too.
    append(&path, &ai("文档与测试"));
    append(&path, &custom("手写的"));
    index.refresh(&home);
    let e = entry(&index, "s");
    assert_eq!((e.ai_title.as_deref(), e.custom_title.as_deref()), (Some("文档与测试"), Some("手写的")));
    // Everything an incremental scan found equals a full parse.
    assert_eq!(e, parse_claude_session(&path).unwrap());
}

const CODEX_ID: &str = "01a0eb29-4056-78f2-a118-864580a5a3fb";

fn codex_home() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    copy_dir(&fixture("history/home"), &home);
    let state = dir.path().join("state");
    (dir, home, state)
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

#[test]
fn codex_titles_come_from_the_session_index_and_the_last_row_wins() {
    let (_dir, home, state) = codex_home();
    let mut index = HistoryIndex::load(Some(&state));
    index.refresh(&home);
    let claude_titles = |index: &HistoryIndex| -> Vec<(String, Option<String>)> {
        index.entries().into_iter().filter(|e| e.agent == AgentKind::Claude).map(|e| (e.session_id, e.ai_title)).collect()
    };
    let claude_before = claude_titles(&index);
    assert!(!claude_before.is_empty());
    let before = entry(&index, CODEX_ID);
    assert_eq!((before.agent, before.ai_title.clone()), (AgentKind::Codex, None), "no index file yet");

    let rows = [
        serde_json::json!({"id":CODEX_ID,"thread_name":"旧名字","updated_at":"2026-09-30T00:00:00Z"}),
        serde_json::json!({"id":"someone-else","thread_name":"别人的","updated_at":"2026-09-30T00:00:00Z"}),
        serde_json::json!({"id":CODEX_ID,"thread_name":"  调研 Codex\n验证 ","updated_at":"2026-10-01T00:00:00Z"}),
    ];
    let text: String = rows.iter().map(|r| format!("{r}\n")).collect();
    fs::write(home.join(".codex/session_index.jsonl"), text).unwrap();
    index.refresh(&home);
    let after = entry(&index, CODEX_ID);
    assert_eq!(after.ai_title.as_deref(), Some("调研 Codex"));
    // Claude sessions never take a Codex title, and an unreadable row is skipped.
    assert_eq!(claude_titles(&index), claude_before);
    let mut broken = fs::read(home.join(".codex/session_index.jsonl")).unwrap();
    broken.extend_from_slice(b"{not json\n");
    fs::write(home.join(".codex/session_index.jsonl"), broken).unwrap();
    index.refresh(&home);
    assert_eq!(entry(&index, CODEX_ID).ai_title.as_deref(), Some("调研 Codex"));

    // Removing the file removes the titles.
    fs::remove_file(home.join(".codex/session_index.jsonl")).unwrap();
    index.refresh(&home);
    assert_eq!(entry(&index, CODEX_ID).ai_title, None);
}
