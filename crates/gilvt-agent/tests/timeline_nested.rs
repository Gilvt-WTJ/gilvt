//! Timeline plans (TODO), subagents nested under their Task / Agent call, and the turn / row limits.

mod common;

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{fixture, hooks, lines};
use gilvt_agent::{
    subagent_parent_tool_use, subagent_transcripts, AgentKind, Item, ItemStatus, PlanItem, PlanState, Timeline,
    ToolItem, Turn, MAX_ITEMS, MAX_TURNS,
};
use serde_json::json;

const RUN1: &str = "claude/run1-full/11111111-2222-4333-8444-555555555503.jsonl";
const SUB: &str = "claude/run1-full/11111111-2222-4333-8444-555555555503/subagents/agent-ab4d80d3c1c0641ac.jsonl";
const AGENT_ID: &str = "ab4d80d3c1c0641ac";
const AGENT_CALL: &str = "toolu_016CuZqtckR8UrmNTHWrM4nP";
const SUB_READ: &str = "toolu_013eUBUD9qMq4J1EsWL6KxsN";
/// Index of the parent's Agent tool_use record in RUN1.
const AGENT_LINE: usize = 21;

fn at(i: usize) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_790_295_810 + i as u64)
}

fn feed(tl: &mut Timeline, lines: &[String]) {
    for line in lines {
        tl.apply_transcript_line(line);
    }
}

fn feed_sub(tl: &mut Timeline, lines: &[String]) {
    for line in lines {
        tl.apply_subagent_line(AGENT_ID, line);
    }
}

fn tools(items: &[Item]) -> Vec<&ToolItem> {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Tool(t) => Some(t),
            _ => None,
        })
        .collect()
}

fn tool<'a>(turn: &'a Turn, id: &str) -> &'a ToolItem {
    tools(&turn.items).into_iter().find(|t| t.id == id).unwrap_or_else(|| panic!("no {id} in {:#?}", turn.items))
}

fn plan(tl: &Timeline) -> Vec<(&str, PlanState)> {
    tl.plan().iter().map(|PlanItem { text, state }| (text.as_str(), *state)).collect()
}

/// The subagent's rows under the Agent call: thinking, its Read, thinking (in arrival order).
fn assert_nested(turn: &Turn) {
    assert_nested_in_order(turn, ["thinking", "Read", "thinking"]);
}

fn assert_nested_in_order(turn: &Turn, order: [&str; 3]) {
    let agent = tool(turn, AGENT_CALL);
    let kinds: Vec<&str> = agent
        .children
        .iter()
        .map(|c| match c {
            Item::Tool(t) => t.tool.as_str(),
            Item::Thinking { .. } => "thinking",
            Item::Truncated { .. } => "truncated",
        })
        .collect();
    assert_eq!(kinds, order, "{:#?}", agent.children);
    let read = tools(&agent.children)[0];
    assert_eq!((read.id.as_str(), read.summary.as_str(), read.status.clone()), (SUB_READ, "out.txt", ItemStatus::Ok));
    assert_eq!(read.detail.output, ["1\thi"]);
    assert_eq!(read.anchor, None, "subagent rows have no anchor");
    let info = agent.subagent.as_ref().expect("linked");
    assert_eq!(info.agent_id, AGENT_ID);
    assert_eq!(info.agent_type.as_deref(), Some("general-purpose"));
    assert_eq!(info.result.as_deref(), Some("hi"));
    assert!(info.done);
    assert!(tools(&turn.items).iter().all(|t| t.id != SUB_READ), "not a row of the session itself");
}

#[test]
fn claude_tasks_accumulate() {
    let run1 = lines(RUN1);
    let mut tl = Timeline::new(AgentKind::Claude);
    assert!(tl.plan().is_empty());
    feed(&mut tl, &run1[..11]);
    assert_eq!(plan(&tl), [("write file", PlanState::Todo), ("check file", PlanState::Todo)]);
    feed(&mut tl, &run1[11..]);
    assert_eq!(plan(&tl), [("write file", PlanState::Done), ("check file", PlanState::Done)]);
    // The M3a fixture has no toolUseResult: ids come from 「Task #N created」.
    let mut old = Timeline::new(AgentKind::Claude);
    feed(&mut old, &lines("claude/transcript-run1.jsonl"));
    assert_eq!(plan(&old), plan(&tl));
    // Reading the transcript again (a truncated file re-read from the start) adds nothing.
    feed(&mut tl, &run1);
    assert_eq!(tl.plan().len(), 2);
}

#[test]
fn claude_todo_write_replaces() {
    // No TodoWrite in the captures (Claude Code 2.1.2xx used Task tools); records shaped like run1's tool_use.
    let todo = |id: &str, a: &str, b: &str| {
        json!({"type": "assistant", "timestamp": "2026-09-25T00:23:40.000Z", "isSidechain": false, "message": {
            "model": "claude-haiku-4-5-20251001", "id": format!("msg_{id}"), "role": "assistant", "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": id, "name": "TodoWrite", "input": {"todos": [
                {"content": "write file", "status": a, "activeForm": "Writing file"},
                {"content": "check file", "status": b, "activeForm": "Checking file"}]}}]}})
        .to_string()
    };
    let mut tl = Timeline::new(AgentKind::Claude);
    feed(&mut tl, &lines(RUN1)[..1]);
    tl.apply_transcript_line(&todo("toolu_t1", "in_progress", "pending"));
    assert_eq!(plan(&tl), [("write file", PlanState::Active), ("check file", PlanState::Todo)]);
    tl.apply_transcript_line(&todo("toolu_t2", "completed", "in_progress"));
    assert_eq!(plan(&tl), [("write file", PlanState::Done), ("check file", PlanState::Active)]);
    assert_eq!(tools(&tl.turns()[0].items).len(), 2, "TodoWrite calls are rows too");
}

#[test]
fn codex_update_plan() {
    let mut tl = Timeline::new(AgentKind::Codex);
    feed(&mut tl, &lines("codex/rollout-patch.jsonl"));
    assert_eq!(plan(&tl), [("say hi", PlanState::Active), ("reply ok", PlanState::Todo)]);
}

#[test]
fn subagent_after_the_parent_transcript() {
    let mut tl = Timeline::new(AgentKind::Claude);
    feed(&mut tl, &lines(RUN1));
    {
        // Linked by the result's agentId and finished by the task notification, before its own rows arrive.
        let agent = tool(&tl.turns()[0], AGENT_CALL);
        let info = agent.subagent.as_ref().expect("linked by toolUseResult.agentId");
        assert_eq!((info.result.as_deref(), info.done), (Some("hi"), true));
        assert!(agent.children.is_empty());
        assert_eq!(agent.status, ItemStatus::Ok, "the async launch returned at once");
    }
    feed_sub(&mut tl, &lines(SUB));
    let turn = &tl.turns()[0];
    assert_nested(turn);
    assert_eq!(turn.tokens, 254_827 + 28_817, "the subagent's replies count towards the turn");
    assert_eq!(turn.steps, 8, "subagent rows are not steps of the turn");
}

#[test]
fn subagent_while_it_runs_is_linked_by_its_prompt() {
    let run1 = lines(RUN1);
    let mut tl = Timeline::new(AgentKind::Claude);
    feed(&mut tl, &run1[..=AGENT_LINE]);
    feed_sub(&mut tl, &lines(SUB)[..3]);
    {
        let agent = tool(tl.current().unwrap(), AGENT_CALL);
        assert_eq!(tools(&agent.children).len(), 1, "nested before any explicit link");
        assert_eq!(tools(&agent.children)[0].status, ItemStatus::Running);
        assert_eq!(agent.subagent.as_ref().map(|s| s.done), Some(false));
    }
    feed_sub(&mut tl, &lines(SUB)[3..]);
    feed(&mut tl, &run1[AGENT_LINE + 1..]);
    assert_nested(&tl.turns()[0]);
}

#[test]
fn subagent_rows_before_the_parent_call_wait_for_it() {
    let mut tl = Timeline::new(AgentKind::Claude);
    feed(&mut tl, &lines(RUN1)[..5]);
    feed_sub(&mut tl, &lines(SUB));
    assert!(tl.turns()[0].items.iter().all(|i| !matches!(i, Item::Tool(t) if t.id == SUB_READ)));
    feed(&mut tl, &lines(RUN1)[5..]);
    assert_nested(&tl.turns()[0]);
}

#[test]
fn subagent_hooks_nest_without_anchors() {
    let mut tl = Timeline::new(AgentKind::Claude);
    for (i, (event, payload)) in hooks("claude/hooks-run1.jsonl").iter().enumerate() {
        tl.apply_hook(event, payload, Some(gilvt_agent::Anchor { pane: 1, line: i as u64 }), at(i));
    }
    {
        let agent = tool(tl.current().unwrap(), AGENT_CALL);
        let read = tools(&agent.children)[0];
        assert_eq!(
            (read.id.as_str(), read.anchor, read.started, read.ended),
            (SUB_READ, None, Some(at(11)), Some(at(12)))
        );
        assert_eq!(read.status, ItemStatus::Ok);
        let info = agent.subagent.as_ref().expect("SubagentStart links to the open Agent call");
        assert_eq!((info.result.as_deref(), info.done), (Some("hi"), true), "SubagentStop.last_assistant_message");
    }
    feed(&mut tl, &lines(RUN1));
    feed_sub(&mut tl, &lines(SUB));
    // The hook announced the Read before the subagent transcript's thinking rows arrived.
    assert_nested_in_order(&tl.turns()[0], ["Read", "thinking", "thinking"]);
    let read = tools(&tool(&tl.turns()[0], AGENT_CALL).children)[0];
    assert_eq!(read.started, Some(at(11)), "hook times win inside subagents too");
}

#[test]
fn a_wrong_guess_is_corrected_by_an_explicit_link() {
    let call = |id: &str, ts: &str| {
        json!({"type": "assistant", "timestamp": ts, "message": {"model": "claude-haiku-4-5-20251001", "id": format!("m_{id}"),
            "role": "assistant", "stop_reason": "tool_use", "content": [{"type": "tool_use", "id": id, "name": "Agent",
            "input": {"description": "look", "prompt": "Read out.txt", "subagent_type": "Explore"}}]}})
        .to_string()
    };
    let mut tl = Timeline::new(AgentKind::Claude);
    tl.apply_transcript_line(&json!({"type": "user", "message": {"role": "user", "content": "twice"}}).to_string());
    tl.apply_transcript_line(&call("A1", "2026-09-25T00:24:06.000Z"));
    tl.apply_transcript_line(&call("A2", "2026-09-25T00:24:06.500Z"));
    let sub_lines = lines(SUB);
    let prompt = json!({"type": "user", "isSidechain": true, "agentId": "x1", "parentUuid": null,
        "message": {"role": "user", "content": "Read out.txt"}});
    tl.apply_subagent_line("x1", &prompt.to_string());
    for line in &sub_lines[1..] {
        tl.apply_subagent_line("x1", line);
    }
    let turn = tl.current().unwrap();
    assert_eq!(tool(turn, "A2").children.len(), 3, "same prompt: the latest unclaimed call");
    tl.link_subagent("x1", "A1");
    let turn = tl.current().unwrap();
    assert!(tool(turn, "A2").children.is_empty() && tool(turn, "A2").subagent.is_none());
    let a1 = tool(turn, "A1");
    assert_eq!(a1.children.len(), 3);
    assert_eq!(a1.subagent.as_ref().and_then(|s| s.agent_type.as_deref()), Some("Explore"));
    // A later guess does not undo an explicit link.
    tl.apply_hook("SubagentStart", &json!({"session_id": "s", "agent_id": "x1", "agent_type": "Explore"}), None, at(0));
    assert_eq!(tool(tl.current().unwrap(), "A1").children.len(), 3);
}

#[test]
fn subagent_files_and_meta() {
    let transcript = fixture(RUN1);
    let found = subagent_transcripts(&transcript);
    assert_eq!(found, vec![(AGENT_ID.to_string(), fixture(SUB))]);
    assert_eq!(subagent_parent_tool_use(&fixture(SUB)).as_deref(), Some(AGENT_CALL));
    assert!(subagent_transcripts(&fixture("claude/transcript-run1.jsonl")).is_empty());
    assert_eq!(subagent_parent_tool_use(&PathBuf::from("/nonexistent/agent-x.jsonl")), None);
    // meta.json link before any other line, and before any turn exists: the subagent's rows and tokens must
    // wait for the real turn rather than opening (or pinning) a turn of their own.
    let mut tl = Timeline::new(AgentKind::Claude);
    tl.link_subagent(AGENT_ID, AGENT_CALL);
    feed_sub(&mut tl, &lines(SUB));
    assert!(tl.turns().is_empty(), "buffered subagent input opens no turn of its own");
    feed(&mut tl, &lines(RUN1));
    assert_eq!(tl.turns().len(), 1, "still exactly the one real turn");
    let turn = &tl.turns()[0];
    assert!(!turn.prompt.is_empty(), "the real transcript prompt, not an empty placeholder");
    assert_eq!(tl.current().unwrap().prompt, turn.prompt);
    assert_eq!(turn.tokens, 254_827 + 28_817, "buffered subagent tokens land on the real turn once adopted");
    assert_nested(turn);
}

#[test]
fn hookless_session_subagent_tokens_never_pin_a_stale_turn() {
    // A session with no hooks at all (the "lite" binding, see `discover.rs`): only transcript lines arrive.
    // The subagent's rows / tokens arrive before its parent call exists in any turn, buffering them; an
    // earlier, unrelated turn must not become a stray home for them (the bug: `sub_turn` used to fall back to
    // `hook_turn`, pinning a hook cursor that a later, still-unlinked subagent's tokens landed on).
    let mut tl = Timeline::new(AgentKind::Claude);
    tl.link_subagent(AGENT_ID, AGENT_CALL);
    feed_sub(&mut tl, &lines(SUB));
    assert!(tl.turns().is_empty(), "buffered subagent input opens no turn of its own");
    // A first, unrelated turn runs and ends entirely before the parent call ever appears.
    tl.apply_transcript_line(&prompt_line("turn one"));
    tl.apply_transcript_line(&bash_line("t1"));
    tl.apply_transcript_line(&result_line("t1"));
    assert_eq!(tl.turns().len(), 1);
    // The real transcript (with its own prompt and the Agent call) now runs as the next turn.
    feed(&mut tl, &lines(RUN1));
    assert_eq!(tl.turns().len(), 2, "still exactly the two real turns, no stray hook-only turn");
    let turn = &tl.turns()[1];
    assert_nested(turn);
    assert_eq!(turn.tokens, 254_827 + 28_817, "the buffered tokens land on the parent's own turn");
    assert_eq!(tl.turns()[0].tokens, 0, "not mis-attributed to the earlier, unrelated turn");
}

fn prompt_line(text: &str) -> String {
    json!({"type": "user", "message": {"role": "user", "content": text}}).to_string()
}

fn bash_line(id: &str) -> String {
    json!({"type": "assistant", "message": {"model": "m", "id": format!("m_{id}"), "role": "assistant",
        "stop_reason": "tool_use", "content": [{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": "true"}}]}})
    .to_string()
}

fn result_line(id: &str) -> String {
    json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": "ok"}]}})
        .to_string()
}

#[test]
fn long_turns_fold_their_oldest_rows() {
    let mut tl = Timeline::new(AgentKind::Claude);
    tl.apply_transcript_line(&prompt_line("loop"));
    for i in 0..MAX_ITEMS + 100 {
        tl.apply_transcript_line(&bash_line(&format!("b{i}")));
        tl.apply_transcript_line(&result_line(&format!("b{i}")));
    }
    let turn = tl.current().unwrap();
    assert_eq!(turn.items.len(), MAX_ITEMS);
    assert_eq!(turn.items[0], Item::Truncated { hidden: 101 });
    assert_eq!(turn.steps, MAX_ITEMS + 100);
    let Item::Tool(last) = turn.items.last().unwrap() else { panic!() };
    assert_eq!((last.id.as_str(), last.status.clone()), ("b599", ItemStatus::Ok));
    // A result for a folded row is ignored.
    tl.apply_transcript_line(&result_line("b0"));
}

#[test]
fn old_turns_keep_a_summary() {
    let mut tl = Timeline::new(AgentKind::Claude);
    for i in 0..MAX_TURNS + 5 {
        tl.apply_transcript_line(&prompt_line(&format!("prompt {i}")));
        tl.apply_transcript_line(&bash_line(&format!("t{i}")));
    }
    let turns = tl.turns();
    assert_eq!(turns.len(), MAX_TURNS + 5);
    assert!(turns[..5].iter().all(|t| t.items.is_empty() && t.steps == 1));
    assert!(turns[5..].iter().all(|t| t.items.len() == 1 && t.steps == 1));
    assert_eq!((turns[0].index, turns[0].prompt.as_str()), (1, "prompt 0"));
    assert_eq!(turns[MAX_TURNS + 4].index, (MAX_TURNS + 5) as u32);
}

#[test]
fn background_shell_notification_is_not_a_subagent() {
    // Claude sends the same `<task-notification>` for a `run_in_background` Bash command as for an async
    // subagent: it must not turn the Bash row into a subagent row.
    let mut tl = Timeline::new(AgentKind::Claude);
    let records = [
        json!({"type": "user", "timestamp": "2026-09-25T00:23:30.000Z",
            "message": {"role": "user", "content": "run it in the background"}}),
        json!({"type": "assistant", "timestamp": "2026-09-25T00:23:31.000Z", "message": {
            "id": "msg_1", "model": "claude-x", "usage": {"input_tokens": 1, "output_tokens": 1},
            "content": [{"type": "tool_use", "id": "toolu_bg", "name": "Bash",
                "input": {"command": "sleep 5", "run_in_background": true}}],
            "stop_reason": "tool_use"}}),
        json!({"type": "user", "timestamp": "2026-09-25T00:23:32.000Z", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_bg",
                "content": "Command running in background with ID: bxyz123"}]}}),
        json!({"type": "user", "timestamp": "2026-09-25T00:23:38.000Z", "message": {"role": "user", "content":
            "<task-notification>\n<task-id>bxyz123</task-id>\n<tool-use-id>toolu_bg</tool-use-id>\n<status>completed</status>\n<summary>Background command completed</summary>\n</task-notification>"}}),
    ];
    for r in &records {
        tl.apply_transcript_line(&r.to_string());
    }
    let turn = tl.current().expect("a turn");
    let bash = tool(turn, "toolu_bg");
    assert_eq!(bash.tool, "Bash");
    assert_eq!(bash.subagent, None, "{bash:#?}");
    assert_eq!(bash.status, ItemStatus::Ok);
}

#[test]
fn a_background_subagent_hands_its_report_back_as_a_message() {
    // Claude Code 2.1.284: the Agent call returns at once, the report comes as a peer message, then the
    // task notification only points at it.
    let mut tl = Timeline::new(AgentKind::Claude);
    let records = [
        json!({"type": "user", "timestamp": "2026-09-29T15:26:00.000Z", "message": {"role": "user", "content": "find the bug"}}),
        json!({"type": "assistant", "timestamp": "2026-09-29T15:26:03.000Z", "message": {"id": "msg_1", "model": "claude-x",
            "content": [{"type": "tool_use", "id": "toolu_ag", "name": "Agent",
                "input": {"subagent_type": "general-purpose", "description": "Analyze divide", "prompt": "read calc.py"}}],
            "stop_reason": "tool_use"}}),
        json!({"type": "user", "timestamp": "2026-09-29T15:26:04.000Z",
            "toolUseResult": {"isAsync": true, "status": "async_launched", "agentId": "afac3c0331747e188"},
            "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_ag",
                "content": "Async agent launched successfully.\nagentId: afac3c0331747e188"}]}}),
        json!({"type": "user", "isMeta": true, "timestamp": "2026-09-29T15:26:19.000Z",
            "origin": {"kind": "peer", "from": "afac3c0331747e188", "handback": true,
                "body": "[Subagent hand-back] The text below is the final report of a subagent. The report follows:\n  The bug is in `divide`: it uses `//`. The fix is `/`."},
            "message": {"role": "user", "content": "Another Claude session sent a message:\n<agent-message from=\"afac3c0331747e188\">…</agent-message>"}}),
        json!({"type": "user", "timestamp": "2026-09-29T15:26:19.500Z", "origin": {"kind": "task-notification"},
            "message": {"role": "user", "content": "<task-notification>\n<task-id>afac3c0331747e188</task-id>\n<tool-use-id>toolu_ag</tool-use-id>\n<status>completed</status>\n<result>This agent's report was delivered to you as a message from \"afac3c0331747e188\" (its SubagentHandback call). Read it there; it is not repeated here.\n</result>\n</task-notification>"}}),
    ];
    for r in &records {
        tl.apply_transcript_line(&r.to_string());
    }
    assert_eq!(tl.turns().len(), 1);
    let agent = tool(tl.current().unwrap(), "toolu_ag");
    let sub = agent.subagent.as_ref().expect("a subagent row");
    assert_eq!((sub.result.as_deref(), sub.done), (Some("The bug is in `divide`: it uses `//`."), true));
}
