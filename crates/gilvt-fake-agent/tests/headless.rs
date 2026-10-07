//! `--headless` seeding (`drive.sh seed`): the real binary writes a finished past session into HOME, dated
//! `--age` ago, without a terminal, and replaces an earlier seed of the same id.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

const BIN: &str = env!("CARGO_BIN_EXE_gilvt-fake-agent");
const CLAUDE_ID: &str = "c1a0de00-0000-4000-8000-000000000001";
const CODEX_ID: &str = "01990000-0000-7000-8000-000000000001";

fn seed(s: &common::Setup, agent: &str, args: &[&str]) -> Output {
    Command::new(BIN)
        .env_clear()
        .env("HOME", &s.home)
        .env("PATH", "/usr/bin:/bin")
        .env("GILVT_FAKE_AGENT", agent)
        .env("GILVT_SANDBOX_HOME", &s.home)
        .arg("--headless")
        .args(args)
        .current_dir(&s.cwd)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
}

/// `<agent> <id> <path>` from stdout.
fn printed(out: &Output) -> (String, String, PathBuf) {
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let mut parts = text.splitn(3, ' ');
    let (a, id, path) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
    (a.to_string(), id.to_string(), PathBuf::from(path))
}

fn about(t: SystemTime, want: SystemTime, slack: Duration) -> bool {
    let d = t.duration_since(want).or_else(|e| Ok::<_, ()>(e.duration())).unwrap();
    d <= slack
}

fn mtime(p: &Path) -> SystemTime {
    std::fs::metadata(p).unwrap().modified().unwrap()
}

#[test]
fn claude_seed_is_backdated_and_replaced_on_reseed() {
    let s = common::Setup::new();
    let args = ["--age", "8d", "--session-id", CLAUDE_ID, "@scenario:hist-claude i1 首条提示词"];
    let out = seed(&s, "claude", &args);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let (agent, id, path) = printed(&out);
    assert_eq!((agent.as_str(), id.as_str()), ("claude", CLAUDE_ID));
    assert_eq!(path, gilvt_fake_agent::claude::transcript_path(&s.home, &s.cwd, CLAUDE_ID));
    let e = gilvt_agent::parse_claude_session(&path).expect("listed in the history");
    assert_eq!((e.turns, e.first_prompt.as_str()), (2, "i1 首条提示词"));
    let eight_days_ago = SystemTime::now() - Duration::from_secs(8 * 86_400);
    assert!(about(e.last_active, eight_days_ago, Duration::from_secs(120)), "{:?}", e.last_active);
    assert!(about(mtime(&path), e.last_active, Duration::from_secs(2)), "the mtime follows the last record");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("Welcome"), "no TUI on stdout");

    let again = seed(&s, "claude", &args);
    assert_eq!(again.status.code(), Some(0));
    assert_eq!(gilvt_agent::parse_claude_session(&path).unwrap().turns, 2, "a reseed replaces the file");
}

#[test]
fn codex_seed_takes_a_session_id_and_the_date_directory_of_its_age() {
    let s = common::Setup::new();
    let args = ["--age", "2h", "--session-id", CODEX_ID, "@scenario:hist-codex"];
    let out = seed(&s, "codex", &args);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let (agent, id, path) = printed(&out);
    assert_eq!((agent.as_str(), id.as_str()), ("codex", CODEX_ID));
    let start = SystemTime::now() - Duration::from_secs(2 * 3600);
    let dated = gilvt_fake_agent::codex::rollout_path(&s.home, start, CODEX_ID);
    assert_eq!(path.parent(), dated.parent(), "sessions/YYYY/MM/DD of two hours ago");
    let e = gilvt_agent::parse_codex_rollout(&path).expect("listed in the history");
    assert_eq!((e.turns, e.session_id.as_str(), e.cwd.as_path()), (2, CODEX_ID, s.cwd.as_path()));
    assert!(about(e.last_active, start, Duration::from_secs(120)));

    // The reseed's file is named after its own start second, which may be one later than the first's.
    let again = seed(&s, "codex", &args);
    assert_eq!(again.status.code(), Some(0));
    let (_, _, reseeded) = printed(&again);
    assert_eq!(reseeded.parent(), path.parent());
    assert_eq!(gilvt_fake_agent::codex::find_rollout(&s.home, CODEX_ID), Some(reseeded));
    let files = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
    assert_eq!(files, 1, "the earlier rollout was replaced");
}

/// "The agent finished another turn while the review was open": `--append` adds the scenario's turns after the
/// ones already in the transcript, keeping their identities, so a saved review position stays valid. It deletes
/// nothing, so it needs no sandbox HOME.
#[test]
fn append_adds_turns_after_the_existing_ones_without_touching_them() {
    for (agent, id, scenario, first_prompt) in [
        ("claude", CLAUDE_ID, "@scenario:hist-claude", "追加前的首条提示词"),
        ("codex", CODEX_ID, "@scenario:hist-codex", "追加前的首条提示词"),
    ] {
        let s = common::Setup::new();
        let first = seed(&s, agent, &["--session-id", id, &format!("{scenario} {first_prompt}")]);
        assert_eq!(first.status.code(), Some(0), "{agent}: {}", String::from_utf8_lossy(&first.stderr));
        let (_, _, path) = printed(&first);
        let before = std::fs::read(&path).unwrap();

        // No GILVT_SANDBOX_HOME: appending never deletes.
        let out = Command::new(BIN)
            .env_clear()
            .env("HOME", &s.home)
            .env("PATH", "/usr/bin:/bin")
            .env("GILVT_FAKE_AGENT", agent)
            .args(["--headless", "--append", "--session-id", id, &format!("{scenario} 追加的一轮")])
            .current_dir(&s.cwd)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{agent}: {}", String::from_utf8_lossy(&out.stderr));
        let (_, appended_id, appended_path) = printed(&out);
        assert_eq!((appended_id.as_str(), &appended_path), (id, &path), "{agent}: the same session, the same file");

        let after = std::fs::read(&path).unwrap();
        assert!(after.len() > before.len() && after.starts_with(&before), "{agent}: the earlier records are untouched");
        let turns = match agent {
            "claude" => gilvt_agent::parse_claude_session(&path).unwrap().turns,
            _ => gilvt_agent::parse_codex_rollout(&path).unwrap().turns,
        };
        assert_eq!(turns, 4, "{agent}: two seeded turns plus two appended");
        if agent == "codex" {
            assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1, "no second rollout file");
        }
    }
}

/// The titles the agents keep for a session: Claude writes `ai-title` / `custom-title` records into the
/// transcript, Codex a row in `~/.codex/session_index.jsonl`. `--ai-title` / `--custom-title` seed them.
#[test]
fn seeded_titles_are_what_the_history_index_reads() {
    let s = common::Setup::new();
    let out = seed(&s, "claude", &["--session-id", CLAUDE_ID, "--ai-title", "整理文档", "--custom-title", "我的名字", "@scenario:hist-claude 继续"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let (_, _, path) = printed(&out);
    let e = gilvt_agent::parse_claude_session(&path).unwrap();
    assert_eq!((e.ai_title.as_deref(), e.custom_title.as_deref()), (Some("整理文档"), Some("我的名字")));
    assert_eq!(e.turns, 2, "the titles are not turns");

    let out = seed(&s, "codex", &["--session-id", CODEX_ID, "--ai-title", "调研验证", "@scenario:hist-codex"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let mut index = gilvt_agent::HistoryIndex::load(None);
    let codex = index.refresh(&s.home).into_iter().find(|e| e.session_id == CODEX_ID).unwrap();
    assert_eq!(codex.ai_title.as_deref(), Some("调研验证"));
    // A second seed of another Codex session adds a row; the first keeps its title.
    let other = "01990000-0000-7000-8000-000000000002";
    let out = seed(&s, "codex", &["--session-id", other, "--ai-title", "另一个", "@scenario:hist-codex"]);
    assert_eq!(out.status.code(), Some(0));
    let entries = index.refresh(&s.home);
    let title = |id: &str| entries.iter().find(|e| e.session_id == id).and_then(|e| e.ai_title.clone());
    assert_eq!((title(CODEX_ID).as_deref(), title(other).as_deref()), (Some("调研验证"), Some("另一个")));
}

#[test]
fn append_without_an_earlier_session_just_creates_it() {
    let s = common::Setup::new();
    let out = seed(&s, "claude", &["--append", "--session-id", CLAUDE_ID, "@scenario:hist-claude 新建"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let (_, _, path) = printed(&out);
    assert_eq!(gilvt_agent::parse_claude_session(&path).unwrap().turns, 2);
}

#[test]
fn headless_usage_errors() {
    let s = common::Setup::new();
    for (args, needle) in [
        (&["--resume", CLAUDE_ID, "@scenario:hist-claude"][..], "cannot --resume"),
        (&["--age", "8w", "@scenario:hist-claude"][..], "--age"),
        (&["@scenario:default"][..], "wrote nothing"),
        (&["@scenario:no-such-scenario"][..], "no-such-scenario"),
    ] {
        let out = seed(&s, "claude", args);
        assert_eq!(out.status.code(), Some(3), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains(needle), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    assert!(!s.home.join(".claude").exists() || std::fs::read_dir(s.home.join(".claude/projects")).map_or(true, |d| d.count() == 0));
}

/// I16 (first GUI run): the ⌘⇧N model row offered no Codex preset after `seed hist-codex --age 1d`. The seed
/// itself is fine: gilvt's history scanner lists it, one day old, with the model the presets come from
/// (`codex_models`: Codex entries active within 30 days, `HistoryEntry::model`). What the GUI missed is the
/// scan: gilvt rescans only at launch and on ⌘⇧R, and the index scanned before the seed does not have it.
#[test]
fn a_codex_seed_carries_the_model_the_presets_need_once_the_history_is_rescanned() {
    let s = common::Setup::new();
    let mut index = gilvt_agent::HistoryIndex::load(None);
    assert!(index.refresh(&s.home).is_empty(), "launch: nothing seeded yet");

    let args = ["--age", "1d", "--session-id", CODEX_ID, "@scenario:hist-codex i16 模型预设的来源"];
    let out = seed(&s, "codex", &args);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(index.entries().is_empty(), "the index gilvt holds is not updated by the seed");

    let entries = index.refresh(&s.home);
    assert_eq!(entries.len(), 1, "the next scan (⌘⇧R) lists the seed");
    let e = &entries[0];
    assert_eq!((e.agent, e.model.as_deref()), (gilvt_agent::AgentKind::Codex, Some(gilvt_fake_agent::codex::MODEL)));
    let age = SystemTime::now().duration_since(e.last_active).unwrap();
    assert!(age > Duration::from_secs(86_400 - 120) && age < Duration::from_secs(30 * 86_400), "{age:?}: inside the 30-day window");
}

#[test]
fn replacing_a_seed_needs_the_sandbox_home() {
    let s = common::Setup::new();
    let args = ["--session-id", CLAUDE_ID, "@scenario:hist-claude first"];
    let out = seed(&s, "claude", &args);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let (_, _, path) = printed(&out);
    let before = std::fs::read(&path).unwrap();
    for sandbox in [None, Some(s.cwd.as_os_str())] {
        let mut cmd = Command::new(BIN);
        cmd.env_clear().env("HOME", &s.home).env("PATH", "/usr/bin:/bin").env("GILVT_FAKE_AGENT", "claude");
        if let Some(other) = sandbox {
            cmd.env("GILVT_SANDBOX_HOME", other);
        }
        let out = cmd.arg("--headless").args(["--session-id", CLAUDE_ID, "@scenario:hist-claude second"]).current_dir(&s.cwd).output().unwrap();
        assert_eq!(out.status.code(), Some(3), "refused without the sandbox HOME ({sandbox:?})");
        assert!(String::from_utf8_lossy(&out.stderr).contains("GILVT_SANDBOX_HOME"), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(std::fs::read(&path).unwrap(), before, "the earlier seed is kept");
    }
    // A new id needs no sandbox: nothing is deleted.
    let out = Command::new(BIN)
        .env_clear()
        .env("HOME", &s.home)
        .env("PATH", "/usr/bin:/bin")
        .env("GILVT_FAKE_AGENT", "claude")
        .args(["--headless", "@scenario:hist-claude third"])
        .current_dir(&s.cwd)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
}
