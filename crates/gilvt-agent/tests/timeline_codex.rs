//! Codex timeline from captured rollouts and hooks (tests/fixtures/codex, redacted): hook `tool_use_id` ==
//! rollout `call_id`, apply_patch line counts, shell exit codes, aborts, reasoning, tokens.

mod common;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{hooks, lines};
use gilvt_agent::{AgentKind, Anchor, Item, ItemStatus, Timeline, ToolItem, Turn, TurnOutcome};
use serde_json::{json, Value};

fn at(i: usize) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_790_296_880 + i as u64)
}

fn anchor(i: usize) -> Anchor {
    Anchor { pane: 3, line: 40 + i as u64 }
}

fn feed(tl: &mut Timeline, lines: &[String]) {
    for line in lines {
        tl.apply_transcript_line(line);
    }
}

fn feed_hooks(tl: &mut Timeline, name: &str) {
    for (i, (event, payload)) in hooks(name).iter().enumerate() {
        tl.apply_hook(event, payload, Some(anchor(i)), at(i));
    }
}

fn tools(turn: &Turn) -> Vec<&ToolItem> {
    turn.items
        .iter()
        .filter_map(|i| match i {
            Item::Tool(t) => Some(t),
            _ => None,
        })
        .collect()
}

fn tool<'a>(turn: &'a Turn, id: &str) -> &'a ToolItem {
    tools(turn).into_iter().find(|t| t.id == id).unwrap_or_else(|| panic!("no {id} in {:#?}", turn.items))
}

#[test]
fn apply_patch_from_the_rollout() {
    let mut tl = Timeline::new(AgentKind::Codex);
    feed(&mut tl, &lines("codex/rollout-patch.jsonl"));
    assert_eq!(tl.turns().len(), 1);
    let turn = &tl.turns()[0];
    assert_eq!(turn.prompt, "make a patch then reply ok");
    assert_eq!(turn.outcome, TurnOutcome::Done);
    let patch = tool(turn, "call_patch");
    assert_eq!((patch.tool.as_str(), patch.summary.as_str()), ("apply_patch", "hello.txt"));
    assert_eq!((patch.status.clone(), patch.lines), (ItemStatus::Ok, Some((1, 0))));
    assert_eq!(patch.detail.output, ["Success. Updated the following files:", "A hello.txt"]);
    assert_eq!(patch.started, Some(UNIX_EPOCH + Duration::from_millis(1_790_296_886_172)));
    assert_eq!(patch.ended, Some(UNIX_EPOCH + Duration::from_millis(1_790_296_892_653)));
    let plan = tool(turn, "call_plan");
    assert_eq!((plan.tool.as_str(), plan.status.clone()), ("update_plan", ItemStatus::Ok));
    assert_eq!(plan.detail.input[0].0, "plan");
    // token_count totals: 1290 then 2580.
    assert_eq!((turn.tokens, tl.session_tokens()), (2580, 2580));
}

#[test]
fn hooks_and_rollout_merge_by_call_id_in_both_orders() {
    let rollout = lines("codex/rollout-patch.jsonl");
    let mut hooks_first = Timeline::new(AgentKind::Codex);
    feed_hooks(&mut hooks_first, "codex/hooks-exec-patch.jsonl");
    {
        let turn = hooks_first.current().unwrap();
        let patch = tool(turn, "call_patch");
        assert_eq!(patch.lines, Some((1, 0)), "counted from the hook's patch before the rollout arrives");
        assert_eq!((patch.anchor, patch.started, patch.ended), (Some(anchor(4)), Some(at(4)), Some(at(5))));
        assert_eq!(patch.status, ItemStatus::Ok);
    }
    feed(&mut hooks_first, &rollout);
    let mut rollout_first = Timeline::new(AgentKind::Codex);
    feed(&mut rollout_first, &rollout);
    feed_hooks(&mut rollout_first, "codex/hooks-exec-patch.jsonl");
    for tl in [&hooks_first, &rollout_first] {
        assert_eq!(tl.turns().len(), 1, "{:#?}", tl.turns());
        let turn = &tl.turns()[0];
        assert_eq!(tools(turn).len(), 2);
        let patch = tool(turn, "call_patch");
        assert_eq!((patch.anchor, patch.started, patch.ended), (Some(anchor(4)), Some(at(4)), Some(at(5))));
        assert_eq!(tool(turn, "call_plan").anchor, Some(anchor(2)));
        assert_eq!(turn.outcome, TurnOutcome::Done);
        assert_eq!(turn.started, Some(at(1)));
    }
    assert_eq!(hooks_first.turns()[0].items, rollout_first.turns()[0].items);
}

#[test]
fn shell_hooks_use_the_rollout_name() {
    let mut tl = Timeline::new(AgentKind::Codex);
    let hook_list = hooks("codex/hooks-tui.jsonl");
    for (i, (event, payload)) in hook_list.iter().enumerate().take(5) {
        tl.apply_hook(event, payload, Some(anchor(i)), at(i));
    }
    // Codex's PermissionRequest names the tool but not the call.
    let request = json!({"session_id": "s", "hook_event_name": "PermissionRequest", "tool_name": "Bash",
        "tool_input": {"command": "echo hi"}, "turn_id": "t"});
    tl.apply_hook("", &request, None, at(5));
    assert_eq!(tool(tl.current().unwrap(), "call_sh").status, ItemStatus::Pending);
    for (i, (event, payload)) in hook_list.iter().enumerate().skip(5) {
        tl.apply_hook(event, payload, Some(anchor(i)), at(i));
    }
    feed(&mut tl, &lines("codex/rollout-aborted.jsonl"));
    assert_eq!(tl.turns().len(), 1);
    let turn = &tl.turns()[0];
    let sh = tool(turn, "call_sh");
    assert_eq!((sh.tool.as_str(), sh.summary.as_str()), ("exec_command", "echo hi"));
    assert_eq!((sh.status.clone(), sh.anchor), (ItemStatus::Ok, Some(anchor(4))));
    assert_eq!(sh.detail.output, ["hi"]);
    // Stop came first, then the rollout recorded the Esc.
    assert_eq!(turn.outcome, TurnOutcome::Interrupted);
}

#[test]
fn esc_during_a_command() {
    let mut tl = Timeline::new(AgentKind::Codex);
    feed(&mut tl, &lines("codex/rollout-exec-aborted.jsonl"));
    let turn = tl.current().unwrap();
    assert_eq!(turn.prompt, "please write a file");
    let sh = tool(turn, "call_sh");
    assert_eq!((sh.summary.as_str(), sh.status.clone()), ("echo hi > hi.txt", ItemStatus::Interrupted));
    assert_eq!(sh.detail.output, ["aborted by user after 9.4s"]);
    assert_eq!(turn.outcome, TurnOutcome::Interrupted);
}

#[test]
fn failed_command_shows_its_error() {
    // rollout-complete with the command's output replaced by a real-shaped failure.
    let failure = "Chunk ID: d84f7f\nWall time: 0.0000 seconds\nProcess exited with code 1\nOriginal token count: 28\nOutput:\nsed: /Users/u/proj/SKILL.md: No such file or directory\n";
    let rollout: Vec<String> = lines("codex/rollout-complete.jsonl")
        .into_iter()
        .map(|l| {
            let mut r: Value = serde_json::from_str(&l).unwrap();
            if r["payload"]["type"] == "function_call_output" && r["payload"]["call_id"] == "call_sh" {
                r["payload"]["output"] = json!(failure);
            }
            r.to_string()
        })
        .collect();
    let mut tl = Timeline::new(AgentKind::Codex);
    feed(&mut tl, &rollout);
    let sh = tool(tl.current().unwrap(), "call_sh");
    assert_eq!(sh.status, ItemStatus::Failed { exit: Some(1) });
    assert_eq!(sh.error_excerpt, ["sed: /Users/u/proj/SKILL.md: No such file or directory"]);
    assert_eq!(sh.detail.output, ["sed: /Users/u/proj/SKILL.md: No such file or directory"]);
}

#[test]
fn reasoning_summaries_are_thinking() {
    let mut tl = Timeline::new(AgentKind::Codex);
    feed(&mut tl, &lines("codex/rollout-complete.jsonl")[..10]);
    let reasoning = json!({"timestamp": "2026-09-25T00:55:20.484Z", "type": "response_item", "payload": {"type": "reasoning",
        "id": "rs_1", "summary": [{"type": "summary_text", "text": "**Planning the reply**\n\nKeep it short."}],
        "encrypted_content": "<redacted>"}});
    tl.apply_transcript_line(&reasoning.to_string());
    let turn = tl.current().unwrap();
    let Some(Item::Thinking { secs, text }) = turn.items.last() else { panic!("{:#?}", turn.items) };
    // The previous record (user_message) was written at 00:55:16.484.
    assert_eq!(*secs, Some(4.0));
    assert_eq!(text, &["**Planning the reply**", "", "Keep it short."]);
}

#[test]
fn failed_patch_stays_failed_after_a_plain_text_output() {
    // `patch_apply_end success:false` marks the call Failed; the later `custom_tool_call_output` is plain
    // text with no exit code, which must not turn it back to Ok.
    let rollout: Vec<String> = lines("codex/rollout-patch.jsonl")
        .into_iter()
        .map(|l| {
            let mut v: Value = serde_json::from_str(&l).unwrap();
            let p = &mut v["payload"];
            match p["type"].as_str() {
                Some("patch_apply_end") => {
                    p["success"] = json!(false);
                    p["stdout"] = json!("");
                    p["stderr"] = json!("patch rejected: hello.txt exists");
                }
                Some("custom_tool_call_output") => p["output"] = json!("apply_patch verification failed"),
                _ => {}
            }
            v.to_string()
        })
        .collect();
    let mut tl = Timeline::new(AgentKind::Codex);
    feed(&mut tl, &rollout);
    let patch = tool(&tl.turns()[0], "call_patch");
    assert_eq!(patch.status, ItemStatus::Failed { exit: None });
    assert_eq!(patch.detail.output, ["apply_patch verification failed"]);
}

#[test]
fn the_codex_turn_keeps_its_reply() {
    let mut tl = Timeline::new(AgentKind::Codex);
    feed(&mut tl, &lines("codex/rollout-complete.jsonl"));
    assert_eq!(tl.turns().last().unwrap().reply, "ok");
}

#[test]
fn current_codex_background_process_and_subagent_are_visible() {
    let mut tl = Timeline::new(AgentKind::Codex);
    let rollout = lines("codex/rollout-processes.jsonl");
    feed(&mut tl, &rollout[..3]);
    let running = tool(tl.current().unwrap(), "call_background");
    assert_eq!((running.tool.as_str(), running.summary.as_str(), running.status.clone()),
        ("exec_command", "cargo test -p demo", ItemStatus::Running));

    feed(&mut tl, &rollout[3..7]);
    let running = tool(tl.current().unwrap(), "call_spawn");
    assert_eq!(running.subagent.as_ref().map(|sub| sub.done), Some(false));
    assert_eq!(running.status, ItemStatus::Running);

    feed(&mut tl, &rollout[7..]);
    let turn = tl.current().unwrap();
    let items = tools(turn);
    assert_eq!(items.len(), 2, "wait is folded into the original background command");

    let command = tool(turn, "call_background");
    assert_eq!((command.tool.as_str(), command.summary.as_str()), ("exec_command", "cargo test -p demo"));
    assert_eq!(command.status, ItemStatus::Ok);
    assert!(command.detail.output.iter().any(|line| line == "test result: ok"));

    let agent = tool(turn, "call_spawn");
    assert_eq!((agent.tool.as_str(), agent.summary.as_str()), ("spawn_agent", "reviewer"));
    assert_eq!(agent.status, ItemStatus::Ok);
    let sub = agent.subagent.as_ref().expect("Codex spawn_agent is a subagent row");
    assert_eq!((sub.agent_id.as_str(), sub.agent_type.as_deref(), sub.done), ("thread-review", Some("reviewer"), true));
}
