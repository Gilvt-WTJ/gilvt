//! The tail thread's per-session state with real (redacted) transcripts from gilvt-agent's fixtures, copied
//! into temporary directories. No thread, no gpui.

use super::*;
use gilvt_agent::{Anchor, Event, Item, ToolItem, Turn};
use serde_json::json;
use std::io::Write;
use std::time::{Duration, SystemTime};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../gilvt-agent/tests/fixtures");
const RUN1: &str = "11111111-2222-4333-8444-555555555503";
const AGENT_ID: &str = "ab4d80d3c1c0641ac";
const AGENT_CALL: &str = "toolu_016CuZqtckR8UrmNTHWrM4nP";

fn fixture_lines(rel: &str) -> Vec<String> {
    std::fs::read_to_string(format!("{FIXTURES}/{rel}")).unwrap().lines().map(str::to_string).collect()
}

fn write_lines(path: &Path, lines: &[String]) {
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    for line in lines {
        writeln!(f, "{line}").unwrap();
    }
}

/// The run1 session (with its subagent transcript and meta.json) under `dir`; returns the transcript path.
fn copy_run1(dir: &Path) -> PathBuf {
    let transcript = dir.join(format!("{RUN1}.jsonl"));
    std::fs::copy(format!("{FIXTURES}/claude/run1-full/{RUN1}.jsonl"), &transcript).unwrap();
    let subs = dir.join(RUN1).join("subagents");
    std::fs::create_dir_all(&subs).unwrap();
    for ext in ["jsonl", "meta.json"] {
        let name = format!("agent-{AGENT_ID}.{ext}");
        std::fs::copy(format!("{FIXTURES}/claude/run1-full/{RUN1}/subagents/{name}"), subs.join(&name)).unwrap();
    }
    transcript
}

type Views = HashMap<SessionKey, Arc<TimelineView>>;

/// One pass of the tail thread: read, then publish.
fn pass(w: &mut Worker) -> (Vec<(SessionKey, Batch)>, Views) {
    let mut batches = Vec::new();
    w.read(&mut |key, batch| batches.push((key, batch)));
    (batches, w.publish().into_iter().collect())
}

/// What a timeline fed `lines` directly looks like.
fn direct(agent: AgentKind, lines: &[String]) -> Vec<Turn> {
    let mut t = Timeline::new(agent);
    for line in lines {
        t.apply_transcript_line(line);
    }
    t.turns().to_vec()
}

fn turns(v: &TimelineView) -> Vec<Turn> {
    v.turns.iter().map(|t| (**t).clone()).collect()
}

fn tool<'a>(items: &'a [Item], id: &str) -> Option<&'a ToolItem> {
    items.iter().find_map(|i| match i {
        Item::Tool(t) if t.id == id => Some(t),
        _ => None,
    })
}

fn codex_key() -> SessionKey {
    (AgentKind::Codex, "codex-1".into())
}

#[test]
fn history_goes_whole_to_the_timeline_and_reduced_to_the_registry() {
    let dir = tempfile::tempdir().unwrap();
    let rollout = fixture_lines("codex/rollout-patch.jsonl");
    let path = dir.path().join("rollout.jsonl");
    let split = rollout.len() / 2;
    write_lines(&path, &rollout[..split]);
    let key = codex_key();
    let mut w = Worker::default();
    w.watch(vec![(key.clone(), path.clone())]);
    let (batches, views) = pass(&mut w);
    // The registry gets the latest usage / model only …
    let [(k, Batch::Catchup { events, first_prompt })] = batches.as_slice() else { panic!("{batches:?}") };
    assert_eq!(k, &key);
    assert!(events.iter().all(|e| matches!(e, Event::Usage { .. } | Event::Model { .. })), "{events:?}");
    assert_eq!(first_prompt.as_deref(), Some("make a patch then reply ok"), "and the first prompt, for the name");
    // … the timeline every line.
    let v1 = &views[&key];
    assert_eq!(turns(v1), direct(AgentKind::Codex, &rollout[..split]));
    assert_eq!(v1.revision, 1);
    // Later lines are live for both.
    write_lines(&path, &rollout[split..]);
    let (batches, views) = pass(&mut w);
    let [(_, Batch::Live(events))] = batches.as_slice() else { panic!("{batches:?}") };
    assert_eq!(events, &super::parse(&key, &rollout[split..]));
    let v2 = &views[&key];
    assert_eq!(turns(v2), direct(AgentKind::Codex, &rollout));
    assert_eq!(v2.revision, 2);
    // Nothing new: nothing sent.
    assert_eq!(pass(&mut w), (Vec::new(), Views::new()));
}

#[test]
fn lines_and_hooks_reach_only_their_own_session() {
    let dir = tempfile::tempdir().unwrap();
    let codex_path = dir.path().join("rollout.jsonl");
    let rollout = fixture_lines("codex/rollout-complete.jsonl");
    write_lines(&codex_path, &rollout);
    let claude_path = copy_run1(dir.path());
    let codex = codex_key();
    let claude: SessionKey = (AgentKind::Claude, RUN1.into());
    let mut w = Worker::default();
    w.watch(vec![(codex.clone(), codex_path), (claude.clone(), claude_path.clone())]);
    let (_, views) = pass(&mut w);
    assert_eq!(turns(&views[&codex]), direct(AgentKind::Codex, &rollout));
    assert_eq!(views[&claude].agent, AgentKind::Claude);
    assert!(tool(&views[&claude].current().unwrap().items, AGENT_CALL).is_some());

    // A hook of the Codex session: anchored row there, nothing published for Claude.
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
    let prompt = json!({"hook_event_name": "UserPromptSubmit", "prompt": "run it again"});
    w.hook(codex.clone(), HookFeed { event: "UserPromptSubmit".into(), payload: prompt, anchor: None, at });
    let pre = json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_use_id": "call_new",
        "tool_input": {"command": "cargo test"}});
    let anchor = Some(Anchor { pane: 7, line: 1234 });
    w.hook(codex.clone(), HookFeed { event: "PreToolUse".into(), payload: pre, anchor, at });
    let (batches, views) = pass(&mut w);
    assert!(batches.is_empty(), "hooks are not transcript events");
    assert_eq!(views.keys().collect::<Vec<_>>(), [&codex]);
    let current = views[&codex].current().unwrap();
    assert_eq!(current.prompt, "run it again");
    let row = tool(&current.items, "call_new").unwrap();
    assert_eq!((row.anchor, row.started), (anchor, Some(at)));
    // A hook of a session no transcript is watched for (yet) starts its timeline.
    let other: SessionKey = (AgentKind::Claude, "no-transcript".into());
    let prompt = json!({"hook_event_name": "UserPromptSubmit", "prompt": "hi"});
    w.hook(other.clone(), HookFeed { event: String::new(), payload: prompt, anchor: None, at });
    let (_, views) = pass(&mut w);
    assert_eq!(views[&other].current().unwrap().prompt, "hi");
}

#[test]
fn subagent_transcripts_nest_under_their_agent_call() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = copy_run1(dir.path());
    let key: SessionKey = (AgentKind::Claude, RUN1.into());
    let mut w = Worker::default();
    w.watch(vec![(key.clone(), transcript.clone())]);
    let (_, views) = pass(&mut w);
    let turn = views[&key].current().unwrap();
    let agent = tool(&turn.items, AGENT_CALL).unwrap();
    let info = agent.subagent.as_ref().expect("linked by meta.json");
    assert_eq!(info.agent_id, AGENT_ID);
    assert!(agent.children.iter().any(|c| matches!(c, Item::Tool(t) if t.tool == "Read")), "{:#?}", agent.children);
    assert!(w.dirs().contains(&dir.path().join(RUN1).join("subagents")), "subagent directory watched");
    assert!(w.dirs().contains(dir.path()));

    assert!(w.entries[&key].subs[AGENT_ID].linked);

    // A subagent that appears later is picked up on a later pass; its parent link is retried until its
    // meta.json exists.
    let subs = dir.path().join(RUN1).join("subagents");
    let first = fixture_lines(&format!("claude/run1-full/{RUN1}/subagents/agent-{AGENT_ID}.jsonl"));
    write_lines(&subs.join("agent-late0.jsonl"), &first[..1]);
    pass(&mut w);
    assert!(!w.entries[&key].subs["late0"].linked);
    std::fs::write(subs.join("agent-late0.meta.json"), r#"{"agentType":"Explore","toolUseId":"toolu_other"}"#).unwrap();
    pass(&mut w);
    assert!(w.entries[&key].subs["late0"].linked);
    assert_eq!(w.entries[&key].subs.keys().collect::<Vec<_>>(), [AGENT_ID, "late0"]);
}

#[test]
fn subagents_are_read_only_for_claude() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    write_lines(&path, &fixture_lines("codex/rollout-complete.jsonl"));
    std::fs::create_dir_all(dir.path().join("s").join("subagents")).unwrap();
    let mut w = Worker::default();
    w.watch(vec![(codex_key(), path)]);
    pass(&mut w);
    assert_eq!(w.dirs(), HashSet::from([dir.path().to_path_buf()]));
    assert!(w.entries[&codex_key()].subs.is_empty());
}

#[test]
fn ended_sessions_keep_their_timeline_and_resume_without_repeats() {
    let dir = tempfile::tempdir().unwrap();
    let rollout = fixture_lines("codex/rollout-patch.jsonl");
    let path = dir.path().join("rollout.jsonl");
    write_lines(&path, &rollout[..5]);
    let key = codex_key();
    let mut w = Worker::default();
    w.watch(vec![(key.clone(), path.clone())]);
    pass(&mut w);
    // Ended: no longer read or watched, the timeline stays.
    w.watch(Vec::new());
    write_lines(&path, &rollout[5..]);
    assert_eq!(pass(&mut w), (Vec::new(), Views::new()));
    assert!(w.dirs().is_empty());
    assert_eq!(w.entries[&key].fed.timeline.turns(), direct(AgentKind::Codex, &rollout[..5]).as_slice());
    // Watched again: continues where it stopped; what was written meanwhile is history for the registry.
    w.watch(vec![(key.clone(), path.clone())]);
    let (batches, views) = pass(&mut w);
    assert!(batches.iter().all(|(_, b)| matches!(b, Batch::Catchup { .. })), "{batches:?}");
    assert_eq!(turns(&views[&key]), direct(AgentKind::Codex, &rollout));
    // Re-sending the same set changes nothing.
    w.watch(vec![(key.clone(), path.clone())]);
    assert_eq!(pass(&mut w), (Vec::new(), Views::new()));
}

#[test]
fn another_transcript_for_a_session_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.jsonl"), dir.path().join("b.jsonl"));
    write_lines(&a, &fixture_lines("codex/rollout-patch.jsonl"));
    let complete = fixture_lines("codex/rollout-complete.jsonl");
    write_lines(&b, &complete);
    let key = codex_key();
    let mut w = Worker::default();
    w.watch(vec![(key.clone(), a)]);
    pass(&mut w);
    w.watch(vec![(key.clone(), b)]);
    let (batches, views) = pass(&mut w);
    assert!(matches!(batches.as_slice(), [(_, Batch::Catchup { .. })]));
    assert_eq!(turns(&views[&key]), direct(AgentKind::Codex, &complete));
}

#[test]
fn a_panicking_timeline_is_reported_and_left_alone() {
    let key = codex_key();
    let mut w = Worker::default();
    let at = SystemTime::UNIX_EPOCH;
    let feed = |p: &str| HookFeed {
        event: "UserPromptSubmit".into(),
        payload: json!({"hook_event_name": "UserPromptSubmit", "prompt": p}),
        anchor: None,
        at,
    };
    w.hook(key.clone(), feed("one"));
    let v1 = w.publish().pop().unwrap().1;
    assert!(!v1.failed);
    w.entries.get_mut(&key).unwrap().fed.apply(&key, |_| panic!("parser bug (expected by this test)"));
    let v2 = w.publish().pop().unwrap().1;
    assert!(v2.failed);
    assert_eq!(v2.turns, v1.turns, "the last good rows stay");
    w.hook(key.clone(), feed("two"));
    assert!(w.publish().is_empty(), "no longer fed");
}

/// Reads a session bound by `core` the way the tail thread does (its first read is history) and applies
/// the batches to the registry.
fn catch_up(core: &mut crate::agents::state::Core, t0: std::time::Instant) -> Views {
    let mut w = Worker::default();
    w.watch(core.watch_set());
    let (batches, views) = pass(&mut w);
    assert!(matches!(batches.as_slice(), [(_, Batch::Catchup { .. })]), "{batches:?}");
    for (k, b) in &batches {
        core.transcript(k, b, |_| true, t0);
    }
    views
}

/// Checklist I2: a session resumed in a gilvt run that never saw it (⌘⇧R 「会话」, `claude --resume <id>`)
/// sends no prompt hook for its earlier turns, so its name comes from the first real prompt of the transcript
/// read at resume (the history), not 「新会话」. A prompt typed after the resume does not rename it, and the
/// user's rename (sessions.json) still wins. Pinned with a real Claude transcript.
#[test]
fn a_resumed_session_is_named_from_its_history() {
    use crate::agents::state::{parse_hook_request, Core};
    use gilvt_agent::Store;
    let dir = tempfile::tempdir().unwrap();
    let id = "22222222-3333-4444-8555-0000000000a4";
    let path = dir.path().join(format!("{id}.jsonl"));
    std::fs::copy(format!("{FIXTURES}/claude/transcript-interactive.jsonl"), &path).unwrap();
    let key: SessionKey = (AgentKind::Claude, id.into());
    let t0 = std::time::Instant::now();
    let start = json!({"session_id": id, "transcript_path": path, "cwd": "/Users/u/proj",
                       "hook_event_name": "SessionStart", "source": "resume"});
    let first = "Reply with the single word ok.";

    let store_dir = tempfile::tempdir().unwrap();
    let mut core = Core::new(Store::open(store_dir.path()));
    core.hook(&parse_hook_request(Some(1), "claude", "SessionStart", &start).unwrap(), |_| true, t0);
    let views = catch_up(&mut core, t0);
    assert!(!turns(&views[&key]).is_empty(), "the timeline has the earlier turns");
    assert_eq!(core.registry.get(&key).unwrap().name, first, "the sidebar shows the session's name, not 新会话");
    let prompt = json!({"session_id": id, "transcript_path": path, "cwd": "/Users/u/proj",
                        "hook_event_name": "UserPromptSubmit", "prompt": "now something else"});
    core.hook(&parse_hook_request(Some(1), "claude", "UserPromptSubmit", &prompt).unwrap(), |_| true, t0);
    assert_eq!(core.registry.get(&key).unwrap().name, first, "a prompt after the resume keeps the name");

    // A rename saved in an earlier run wins over the history.
    core.registry.rename(&key, "登录修复".into());
    let mut core = Core::new(Store::open(store_dir.path()));
    core.hook(&parse_hook_request(Some(1), "claude", "SessionStart", &start).unwrap(), |_| true, t0);
    catch_up(&mut core, t0);
    let s = core.registry.get(&key).unwrap();
    assert_eq!((s.name.as_str(), s.renamed), ("登录修复", true));
    // Clearing it restores the name from the history.
    core.registry.rename(&key, String::new());
    assert_eq!(core.registry.get(&key).unwrap().name, first);

    // Seen from its first prompt (a new session), the hook names it and the history does not change that.
    let mut core = Core::new(Store::in_memory());
    let fresh = json!({"session_id": id, "transcript_path": path, "cwd": "/Users/u/proj",
                       "hook_event_name": "SessionStart", "source": "startup"});
    core.hook(&parse_hook_request(Some(1), "claude", "SessionStart", &fresh).unwrap(), |_| true, t0);
    core.hook(&parse_hook_request(Some(1), "claude", "UserPromptSubmit", &prompt).unwrap(), |_| true, t0);
    catch_up(&mut core, t0);
    assert_eq!(core.registry.get(&key).unwrap().name, "now something else");
}

/// The same for Codex (`codex resume <id>`): hooked (SessionStart) and lite (no hooks, found by the poll).
#[test]
fn a_resumed_codex_session_is_named_from_its_history() {
    use crate::agents::state::{parse_hook_request, Core, Discovery};
    use gilvt_agent::{Status, Store};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout.jsonl");
    write_lines(&path, &fixture_lines("codex/rollout-patch.jsonl"));
    let key = codex_key();
    let t0 = std::time::Instant::now();
    let first = "make a patch then reply ok";

    let mut core = Core::new(Store::in_memory());
    let start = json!({"session_id": key.1, "transcript_path": path, "cwd": "/Users/u/proj",
                       "hook_event_name": "SessionStart", "source": "resume"});
    core.hook(&parse_hook_request(Some(1), "codex", "SessionStart", &start).unwrap(), |_| true, t0);
    catch_up(&mut core, t0);
    let s = core.registry.get(&key).unwrap();
    assert_eq!((s.name.as_str(), &s.status), (first, &Status::Idle));

    let mut core = Core::new(Store::in_memory());
    let d = Discovery { pane: 2, agent: AgentKind::Codex, cwd: "/Users/u/proj".into(), since: SystemTime::UNIX_EPOCH };
    core.bind_found(&d, Some((key.1.clone(), path.clone())), t0);
    catch_up(&mut core, t0);
    let s = core.registry.get(&key).unwrap();
    assert!(s.lite);
    assert_eq!((s.name.as_str(), &s.status), (first, &Status::Idle), "history names it but runs no turn");
}
