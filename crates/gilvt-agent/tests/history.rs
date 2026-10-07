//! Parsing one past session per file (M3c §3.1): prompts and turns, exclusions, companions. The fixtures
//! under `history/home` are redacted copies of real `~/.claude` / `~/.codex` files.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::fixture;
use gilvt_agent::{parse_claude_session, parse_codex_rollout, session_files, AgentKind, FIRST_PROMPT_MAX};

fn claude(id: &str) -> PathBuf {
    fixture("history/home/.claude/projects/-Users-u-gilvt-lab").join(format!("{id}.jsonl"))
}

fn codex(day: &str, name: &str) -> PathBuf {
    fixture("history/home/.codex/sessions").join(day).join(format!("rollout-{name}.jsonl"))
}

fn at_ms(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

fn len(path: &Path) -> u64 {
    fs::metadata(path).unwrap().len()
}

const CLI: &str = "be216f9f-69a9-4d73-9e98-738fb2dfabfa";
const RESUMED: &str = "feadcb4e-1c64-42c1-bfba-5611528a574d";
const CODEX_CLI: (&str, &str) = ("2026/09/28", "2026-09-28T20-15-44-01a0eb29-4056-78f2-a118-864580a5a3fb");

#[test]
fn claude_interactive_session() {
    let path = claude(CLI);
    let e = parse_claude_session(&path).unwrap();
    assert_eq!(e.agent, AgentKind::Claude);
    assert_eq!(e.session_id, CLI);
    assert_eq!(e.cwd, Path::new("/Users/u/gilvt-lab"));
    assert_eq!(e.transcript, path);
    // The prompt starts with a space in the file.
    assert_eq!(e.first_prompt, "请运行 python3 -m unittest -v，然后在 docs/notes.md 末尾追加一行「H1 claude 测试」。不要修改其他文件。");
    // 9 typed prompts; `!ls` / `!tree` (bash-input / -stdout), the subagent's task-notification, two
    // interruptions and `/exit` with its local output are not turns.
    assert_eq!(e.turns, 9);
    assert_eq!(e.started, Some(at_ms(1_790_659_906_735)));
    assert_eq!(e.last_active, at_ms(1_790_696_617_292));
    assert_eq!(e.model.as_deref(), Some("claude-sonnet-5-5"));
    let subagent = path.with_extension("").join("subagents/agent-afac3c0331747e188.jsonl");
    assert_eq!(e.size, len(&path) + len(&subagent));
}

#[test]
fn claude_custom_slash_command_is_not_a_prompt() {
    let e = parse_claude_session(&claude("7e9f0ca8-1dfe-45bf-8c81-1e6dc0ae4b60")).unwrap();
    assert_eq!(e.turns, 1, "`/doctor` is not counted");
    assert!(e.first_prompt.starts_with("请运行 python3 -m unittest -v"));
    assert_eq!(e.model, None, "only <synthetic> replies");
}

#[test]
fn claude_session_continued_with_print_is_kept() {
    // Interactive first (`entrypoint: cli`), then `claude -p --resume` appended `sdk-cli` records.
    let e = parse_claude_session(&claude(RESUMED)).unwrap();
    assert_eq!(e.first_prompt, "请运行 bash scripts/slow.sh");
    assert_eq!(e.turns, 4);
    assert_eq!(e.started, Some(at_ms(1_790_698_134_941)));
    assert_eq!(e.last_active, at_ms(1_790_704_818_356));
    assert_eq!(e.model.as_deref(), Some("claude-sonnet-5-5"), "not the later <synthetic> reply");
}

#[test]
fn claude_exclusions() {
    let sdk_first = "00000000-0000-4000-8000-00000000000a"; // the `-p --resume` records alone
    let sidechain_only = "00000000-0000-4000-8000-00000000000b"; // a subagent transcript
    let no_prompt = "04a901c9-d546-431a-942d-caae6be7552e"; // opened and quit
    for id in [sdk_first, sidechain_only, no_prompt] {
        assert!(claude(id).is_file());
        assert_eq!(parse_claude_session(&claude(id)), None, "{id}");
    }
    assert_eq!(parse_claude_session(Path::new("/nonexistent/x.jsonl")), None);
}

#[test]
fn partial_last_line_is_skipped() {
    // The fixture's last line is cut in half (no newline).
    let e = parse_claude_session(&claude("3c23fcb6-fb3f-4a54-a896-517c02276c0f")).unwrap();
    assert_eq!(e.turns, 1);
    assert_eq!(e.last_active, at_ms(1_790_702_956_302));

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let prompt = |text: &str, ts: &str| {
        serde_json::json!({"type": "user", "isSidechain": false, "entrypoint": "cli", "cwd": "/w", "timestamp": ts,
            "message": {"role": "user", "content": text}})
        .to_string()
    };
    let first = prompt("first", "2026-09-29T01:00:00Z");
    let unfinished = prompt("second", "2026-09-29T02:00:00Z");
    fs::write(&path, format!("{first}\n{{not json\n{unfinished}")).unwrap();
    let e = parse_claude_session(&path).unwrap();
    assert_eq!((e.turns, e.first_prompt.as_str()), (1, "first"));
    assert_eq!(e.last_active, at_ms(1_790_643_600_000));
    fs::write(&path, format!("{first}\n{unfinished}\n")).unwrap();
    assert_eq!(parse_claude_session(&path).unwrap().turns, 2, "complete once the newline is there");
}

#[test]
fn last_active_falls_back_to_mtime() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    fs::write(&path, "{\"type\":\"user\",\"cwd\":\"/w\",\"message\":{\"content\":\"hi\"}}\n").unwrap();
    let e = parse_claude_session(&path).unwrap();
    assert_eq!(e.started, None);
    assert_eq!(e.last_active, fs::metadata(&path).unwrap().modified().unwrap());
    assert_eq!(e.size, len(&path));
}

#[test]
fn codex_interactive_rollout() {
    let path = codex(CODEX_CLI.0, CODEX_CLI.1);
    let e = parse_codex_rollout(&path).unwrap();
    assert_eq!(e.agent, AgentKind::Codex);
    assert_eq!(e.session_id, "01a0eb29-4056-78f2-a118-864580a5a3fb");
    assert_eq!(e.cwd, Path::new("/Users/u/gilvt-lab"));
    // Cut to FIRST_PROMPT_MAX chars, the ellipsis included.
    let expected = "Run `python3 -m unittest -v`, then append the line \"H1 codex test\" to docs/note…";
    assert_eq!(e.first_prompt, expected);
    assert_eq!(e.first_prompt.chars().count(), FIRST_PROMPT_MAX);
    assert_eq!(e.turns, 3);
    assert_eq!(e.started, Some(at_ms(1_790_660_004_778)));
    assert_eq!(e.last_active, at_ms(1_790_704_895_870));
    assert_eq!(e.model.as_deref(), Some("gpt-5.6-sol"), "the latest turn_context");
    assert_eq!(e.size, len(&path) + len(&path.with_extension("jsonl.langsmith")));
}

#[test]
fn codex_exclusions() {
    let subagent = codex("2026/09/23", "2026-09-23T18-41-59-01a0d113-a182-70f3-ad99-f5d499cb5fe3");
    let exec = codex("2026/09/24", "2026-09-24T17-54-24-01a0d60e-6d57-7030-80f1-f26c4e757a9f");
    let no_prompt = codex("2026/09/28", "2026-09-28T21-00-00-00000000-0000-7000-8000-00000000000c");
    for path in [subagent, exec, no_prompt] {
        assert!(path.is_file());
        assert_eq!(parse_codex_rollout(&path), None, "{}", path.display());
    }
}

#[test]
fn codex_sources_and_originators() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-x.jsonl");
    let rollout = |source: serde_json::Value, originator: &str| {
        let meta = serde_json::json!({"timestamp": "2026-09-29T01:00:00Z", "type": "session_meta",
            "payload": {"id": "t1", "session_id": "t1", "cwd": "/w", "source": source, "originator": originator}});
        let prompt = serde_json::json!({"timestamp": "2026-09-29T01:00:01Z", "type": "event_msg",
            "payload": {"type": "user_message", "message": "\n\n  hi there  \nmore"}});
        fs::write(&path, format!("{meta}\n{prompt}\n")).unwrap();
        parse_codex_rollout(&path)
    };
    let e = rollout("vscode".into(), "gdpa-agent-box").unwrap();
    assert_eq!((e.session_id.as_str(), e.first_prompt.as_str(), e.turns), ("t1", "hi there", 1));
    assert!(rollout("unknown".into(), "codex_cli_rs").is_some());
    assert!(rollout("cli".into(), "codex_exec").is_none());
    assert!(rollout("exec".into(), "codex-tui").is_none());
    assert!(rollout(serde_json::json!({"subagent": {"thread_spawn": {}}}), "codex-tui").is_none());
    fs::write(&path, "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"hi\"}}\n").unwrap();
    assert_eq!(parse_codex_rollout(&path), None, "no session_meta first");
}

#[test]
fn files_of_a_session() {
    let with_dir = parse_claude_session(&claude(CLI)).unwrap();
    assert_eq!(session_files(&with_dir), vec![claude(CLI), claude(CLI).with_extension("")]);
    let alone = parse_claude_session(&claude(RESUMED)).unwrap();
    assert_eq!(session_files(&alone), vec![claude(RESUMED)]);
    let rollout = codex(CODEX_CLI.0, CODEX_CLI.1);
    let e = parse_codex_rollout(&rollout).unwrap();
    assert_eq!(session_files(&e), vec![rollout.clone(), rollout.with_extension("jsonl.langsmith")]);
}
