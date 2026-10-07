//! Transcript discovery in a fake home, process recognition, and tailing a transcript into a lite session.

mod common;

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::{fixture, key, Driver};
use gilvt_agent::{
    agent_of_process, claude_project_dir_name, newest_claude_transcript, newest_codex_rollout, parse_transcript_line,
    process_basename,
    AgentKind, Status, Tail,
};

fn write_at(path: &Path, text: &str, mtime: SystemTime) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    File::options().write(true).open(path).unwrap().set_modified(mtime).unwrap();
}

fn ago(secs: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(secs)
}

fn codex_meta(id: &str, cwd: &str) -> String {
    let meta = serde_json::json!({"timestamp": "t", "type": "session_meta",
        "payload": {"session_id": id, "id": id, "cwd": cwd, "originator": "codex_tui", "base_instructions": "…"}});
    format!("{meta}\n")
}

#[test]
fn claude_project_directory_names() {
    // Same shape as the spike's real directory: `/`, `.`, `_` all become `-`, existing `-` stay.
    let name = claude_project_dir_name(Path::new("/Users/u/github_repos/acme_web_monorepo/.claude/worktrees/w-1"));
    assert_eq!(name, "-Users-u-github-repos-acme-web-monorepo--claude-worktrees-w-1");
    assert_eq!(claude_project_dir_name(Path::new("/Users/u/项目 a")), "-Users-u----a");
    assert_eq!(claude_project_dir_name(Path::new("/x/🦀")), "-x---");
}

#[test]
fn newest_claude_transcript_for_a_cwd() {
    let home = tempfile::tempdir().unwrap();
    let cwd = home.path().join("proj");
    fs::create_dir_all(&cwd).unwrap();
    let dir = home.path().join(".claude/projects").join(claude_project_dir_name(&cwd));
    write_at(&dir.join("aaaa.jsonl"), "{}\n", ago(30));
    write_at(&dir.join("bbbb.jsonl"), "{}\n", ago(10));
    write_at(&dir.join("cccc.txt"), "{}\n", ago(1));
    // Subagent transcripts live in <session>/subagents/ and are not candidates.
    write_at(&dir.join("bbbb/subagents/agent-x.jsonl"), "{}\n", ago(0));
    let other = home.path().join(".claude/projects").join(claude_project_dir_name(&home.path().join("other")));
    write_at(&other.join("zzzz.jsonl"), "{}\n", ago(0));

    let found = newest_claude_transcript(home.path(), &cwd, ago(60));
    assert_eq!(found, Some(("bbbb".to_string(), dir.join("bbbb.jsonl"))));
    assert_eq!(newest_claude_transcript(home.path(), &cwd, ago(5)), None, "all older than `since`");
    assert_eq!(newest_claude_transcript(home.path(), &home.path().join("nowhere"), ago(60)), None);
}

#[test]
fn claude_transcript_under_a_symlinked_or_long_cwd() {
    let home = tempfile::tempdir().unwrap();
    let real = home.path().join("real");
    fs::create_dir_all(&real).unwrap();
    let link = home.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    // Claude Code records the resolved path.
    let dir = home.path().join(".claude/projects").join(claude_project_dir_name(&real.canonicalize().unwrap()));
    write_at(&dir.join("s1.jsonl"), "{}\n", ago(1));
    assert_eq!(newest_claude_transcript(home.path(), &link, ago(60)).map(|f| f.0), Some("s1".into()));

    // Names past 200 chars are cut and get a hash suffix.
    let long = home.path().join("d".repeat(120)).join("e".repeat(120));
    let name = claude_project_dir_name(&long);
    let dir = home.path().join(".claude/projects").join(format!("{}-1a2b3c", &name[..200]));
    write_at(&dir.join("s2.jsonl"), "{}\n", ago(1));
    assert_eq!(newest_claude_transcript(home.path(), &long, ago(60)).map(|f| f.0), Some("s2".into()));
}

#[test]
fn newest_codex_rollout_matching_the_cwd() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join(".codex/sessions");
    let rollout = |day: &str, name: &str| sessions.join(day).join(format!("rollout-{name}.jsonl"));
    let mine = rollout("2026/09/24", "2026-09-24T17-50-18-mine");
    write_at(&mine, &codex_meta("id-mine", "/Users/u/proj"), ago(20));
    // Newer, but another project.
    write_at(&rollout("2026/09/24", "2026-09-24T17-55-00-other"), &codex_meta("id-other", "/Users/u/other"), ago(5));
    // Newer and the same project, but not a rollout / not a session_meta first line.
    write_at(&sessions.join("2026/09/24/notes.jsonl"), &codex_meta("id-notes", "/Users/u/proj"), ago(1));
    write_at(&rollout("2026/09/24", "broken"), "{\"type\":\"event_msg\"}\n", ago(1));
    // Yesterday (across a month boundary here) is searched too; older days are not.
    let yesterday = rollout("2026/08/31", "2026-08-31T23-59-00-y");
    write_at(&yesterday, &codex_meta("id-y", "/Users/u/yproj"), ago(40));
    write_at(&rollout("2026/08/30", "2026-08-30T10-00-00-old"), &codex_meta("id-old", "/Users/u/oldproj"), ago(50));

    let cwd = Path::new("/Users/u/proj/");
    assert_eq!(newest_codex_rollout(home.path(), cwd, ago(60)), Some(("id-mine".into(), mine)));
    assert_eq!(
        newest_codex_rollout(home.path(), Path::new("/Users/u/yproj"), ago(60)),
        Some(("id-y".into(), yesterday))
    );
    assert_eq!(newest_codex_rollout(home.path(), Path::new("/Users/u/oldproj"), ago(60)), None);
    assert_eq!(newest_codex_rollout(home.path(), cwd, ago(10)), None, "older than `since`");
    assert_eq!(newest_codex_rollout(&home.path().join("empty"), cwd, ago(60)), None);
}

#[test]
fn processes() {
    assert_eq!(agent_of_process("claude", None), Some(AgentKind::Claude));
    assert_eq!(agent_of_process("claude.exe", None), Some(AgentKind::Claude));
    assert_eq!(agent_of_process("/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/bin/claude.EXE", None), Some(AgentKind::Claude));
    assert_eq!(agent_of_process("/Users/u/.local/bin/claude", Some("claude --resume x")), Some(AgentKind::Claude));
    assert_eq!(agent_of_process("codex", None), Some(AgentKind::Codex));
    assert_eq!(agent_of_process("codex-aarch64-apple-darwin", None), Some(AgentKind::Codex));
    let npm_codex = "node /opt/homebrew/lib/node_modules/@openai/codex/bin/codex.js";
    assert_eq!(agent_of_process("node", Some(npm_codex)), Some(AgentKind::Codex));
    let npm_claude = "node /opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js";
    assert_eq!(agent_of_process("node", Some(npm_claude)), Some(AgentKind::Claude));
    assert_eq!(agent_of_process("node", Some("node server.js")), None);
    assert_eq!(agent_of_process("node", None), None);
    assert_eq!(agent_of_process("zsh", Some("codex")), None);
    assert_eq!(agent_of_process("vim", Some("vim claude.md")), None);
}

#[test]
fn process_basenames() {
    assert_eq!(process_basename("claude.exe"), "claude");
    assert_eq!(process_basename("/x/bin/Claude.EXE"), "Claude");
    assert_eq!(process_basename("codex"), "codex");
    assert_eq!(process_basename(".exe"), ".exe");
    assert_eq!(process_basename("a.exe.md"), "a.exe.md");
    assert_eq!(process_basename("中.exe"), "中");
}

/// Lite path end to end: discover the transcript, tail it while it is being written, feed the registry.
#[test]
fn tailing_a_discovered_transcript_drives_a_lite_session() {
    let home = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/Users/u/proj");
    let id = "11111111-2222-4333-8444-555555555503";
    let path = home.path().join(".claude/projects").join(claude_project_dir_name(&cwd)).join(format!("{id}.jsonl"));
    let all = fs::read_to_string(fixture("claude/transcript-run1.jsonl")).unwrap();
    // Cut halfway through the record after the first prompt, as if Claude Code were mid-write.
    let prompt = all.find("\"type\": \"user\"").unwrap();
    let next_line = prompt + all[prompt..].find('\n').unwrap() + 1;
    let (head, rest) = all.split_at(next_line + 50);
    write_at(&path, head, SystemTime::now());

    let (found, found_path) = newest_claude_transcript(home.path(), &cwd, ago(60)).unwrap();
    assert_eq!((found.as_str(), &found_path), (id, &path));
    let mut d = Driver::new();
    d.reg.bind_lite(1, AgentKind::Claude, found, found_path.clone(), cwd, d.now());
    let k = key(AgentKind::Claude, id);
    let mut tail = Tail::new(found_path);
    let feed = |d: &mut Driver, tail: &mut Tail| {
        for line in tail.read_new().unwrap() {
            let now = d.tick();
            let events = parse_transcript_line(AgentKind::Claude, &line);
            d.reg.apply_transcript(&k, &events, true, now);
        }
    };
    feed(&mut d, &mut tail);
    assert_eq!(d.session(&k).status, Status::Thinking, "the prompt line is complete; the next one is partial");
    fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(rest.as_bytes()).unwrap();
    feed(&mut d, &mut tail);
    let s = d.session(&k);
    assert_eq!(s.status, Status::Idle);
    assert!(s.lite);
    assert_eq!(s.context, Some((8 + 277 + 33321, None)));
}
