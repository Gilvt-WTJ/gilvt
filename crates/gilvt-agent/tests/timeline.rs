//! Timeline from captured transcripts, rollouts and hooks (tests/fixtures, redacted): turns, items, merge
//! order, statuses, thinking and tokens.

mod common;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{hooks, lines};
use gilvt_agent::{AgentKind, Anchor, Item, ItemStatus, Timeline, ToolItem, Turn, TurnOutcome};
use serde_json::{json, Value};

const RUN1: &str = "claude/run1-full/11111111-2222-4333-8444-555555555503.jsonl";

/// Hooks arrive one second apart from this instant (2026-09-25T00:23:30Z, during run1).
fn hook_time(i: usize) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_790_295_810 + i as u64)
}

fn anchor(i: usize) -> Anchor {
    Anchor { pane: 7, line: 100 + i as u64 }
}

fn feed_transcript(tl: &mut Timeline, name: &str) {
    for line in lines(name) {
        tl.apply_transcript_line(&line);
    }
}

fn feed_hooks(tl: &mut Timeline, name: &str) {
    for (i, (event, payload)) in hooks(name).iter().enumerate() {
        tl.apply_hook(event, payload, Some(anchor(i)), hook_time(i));
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

fn thinking(turn: &Turn) -> Vec<(Option<f32>, &Vec<String>)> {
    turn.items
        .iter()
        .filter_map(|i| match i {
            Item::Thinking { secs, text } => Some((*secs, text)),
            _ => None,
        })
        .collect()
}

/// `UNIX_EPOCH` + milliseconds.
fn ms(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

#[test]
fn claude_transcript_only_turn() {
    let mut tl = Timeline::new(AgentKind::Claude);
    feed_transcript(&mut tl, RUN1);
    assert_eq!(tl.turns().len(), 1, "the task notification continues the turn, it is not a prompt");
    let turn = &tl.turns()[0];
    assert_eq!(turn.index, 1);
    assert!(turn.prompt.starts_with("Do these steps in order"));
    assert_eq!(turn.outcome, TurnOutcome::Done);
    let names: Vec<(&str, &str)> = tools(turn).iter().map(|t| (t.tool.as_str(), t.summary.as_str())).collect();
    assert_eq!(
        names,
        [
            ("ToolSearch", "select:TaskCreate,TaskUpdate"),
            ("TaskCreate", "write file"),
            ("TaskCreate", "check file"),
            ("Bash", "echo hi > out.txt"),
            ("Write", "out.txt"),
            ("Agent", "Read out.txt file contents"),
            ("TaskUpdate", "completed"),
            ("TaskUpdate", "completed"),
        ]
    );
    // Transcript-only: record timestamps, no anchors.
    let search = tool(turn, "toolu_01CiHYh3Q2SV7SKP6otCMpoL");
    assert_eq!(search.started, Some(ms(1_790_295_817_502))); // 2026-09-25T00:23:37.502Z
    assert_eq!(search.ended, Some(ms(1_790_295_819_987)));
    assert!(tools(turn).iter().all(|t| t.anchor.is_none()));
    // The sandbox-blocked redirect is recorded as a user rejection.
    let bash = tool(turn, "toolu_01T2YqAotcohHw74aCpu7Gim");
    assert_eq!(bash.status, ItemStatus::Denied);
    assert!(bash.error_excerpt.is_empty());
    let write = tool(turn, "toolu_01NNYZiNPdhfgpLtahTiE46y");
    assert_eq!((write.status.clone(), write.lines), (ItemStatus::Ok, Some((1, 0))));
    assert_eq!(write.detail.input.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["content", "file_path"]);
    assert!(write.detail.output[0].starts_with("File created successfully"));
    assert!(tools(turn).iter().all(|t| t.status != ItemStatus::Running));
    // Every reply starts with a (redacted, empty) thinking block; the first took 7 s after the prompt.
    let thoughts = thinking(turn);
    assert_eq!(thoughts.len(), 8);
    assert!((thoughts[0].0.unwrap() - 7.071).abs() < 0.01, "{:?}", thoughts[0].0);
    assert!(thoughts[0].1.is_empty());
    // Split replies counted once per message id.
    assert_eq!(turn.tokens, 254_827);
    assert_eq!(tl.session_tokens(), 254_827);
    assert_eq!(tl.unparsed_streak(), 0);
}

#[test]
fn claude_hooks_first_then_transcript() {
    let mut tl = Timeline::new(AgentKind::Claude);
    feed_hooks(&mut tl, "claude/hooks-run1.jsonl");
    {
        let turn = tl.current().unwrap();
        assert_eq!(turn.started, Some(hook_time(1)));
        // Stop arrived while the background agent ran.
        assert_eq!(turn.outcome, TurnOutcome::Done);
        let ids: Vec<&str> = tools(turn).iter().map(|t| t.id.as_str()).collect();
        // ToolSearch, TaskCreate, Bash (no PostToolUse: blocked), Agent; the subagent's Read is not the session's.
        assert_eq!(ids.len(), 4, "{ids:?}");
        let bash = tool(turn, "toolu_01T2YqAotcohHw74aCpu7Gim");
        assert_eq!(
            (bash.status.clone(), bash.anchor, bash.started),
            (ItemStatus::Running, Some(anchor(6)), Some(hook_time(6)))
        );
        let search = tool(turn, "toolu_01CiHYh3Q2SV7SKP6otCMpoL");
        assert_eq!(
            (search.status.clone(), search.started, search.ended),
            (ItemStatus::Ok, Some(hook_time(2)), Some(hook_time(3)))
        );
    }
    feed_transcript(&mut tl, RUN1);
    assert_eq!(tl.turns().len(), 1, "hook and transcript prompts are one turn");
    let turn = &tl.turns()[0];
    assert_eq!(tools(turn).len(), 8);
    let bash = tool(turn, "toolu_01T2YqAotcohHw74aCpu7Gim");
    assert_eq!(bash.status, ItemStatus::Denied, "the transcript fills the verdict");
    assert_eq!(bash.anchor, Some(anchor(6)));
    assert_eq!(bash.started, Some(hook_time(6)), "hook times win");
    let search = tool(turn, "toolu_01CiHYh3Q2SV7SKP6otCMpoL");
    assert_eq!((search.started, search.ended), (Some(hook_time(2)), Some(hook_time(3))));
    assert!(search.detail.output.is_empty(), "tool_reference blocks carry no text");
    // Transcript-only calls (the second TaskCreate never had hooks in this capture) have no anchor.
    let second = tool(turn, "toolu_01RPiApm1PEYuexayedzzd9H");
    assert_eq!(second.anchor, None);
    assert_eq!(turn.outcome, TurnOutcome::Done);
}

#[test]
fn claude_transcript_first_then_hooks() {
    let mut a = Timeline::new(AgentKind::Claude);
    feed_hooks(&mut a, "claude/hooks-run1.jsonl");
    feed_transcript(&mut a, RUN1);
    let mut b = Timeline::new(AgentKind::Claude);
    feed_transcript(&mut b, RUN1);
    feed_hooks(&mut b, "claude/hooks-run1.jsonl");
    assert_eq!(b.turns().len(), 1);
    let (ta, tb) = (&a.turns()[0], &b.turns()[0]);
    // Rows appear in arrival order (hook-announced calls before transcript-only ones in `a`); the items agree.
    let by_id = |turn| {
        let mut v = tools(turn);
        v.sort_by(|x, y| x.id.cmp(&y.id));
        v
    };
    assert_eq!(by_id(ta), by_id(tb), "same items in both orders");
    assert_eq!(tool(tb, "toolu_016CuZqtckR8UrmNTHWrM4nP").anchor, Some(anchor(7)));
    assert_eq!(ta.tokens, tb.tokens);
}

#[test]
fn claude_approval_then_rejection() {
    // Hooks first: PreToolUse → Running, PermissionRequest → Pending; the transcript then records the "No".
    let mut tl = Timeline::new(AgentKind::Claude);
    let hook_list = hooks("claude/hooks-interactive.jsonl");
    for (i, (event, payload)) in hook_list.iter().enumerate().take(8) {
        tl.apply_hook(event, payload, Some(anchor(i)), hook_time(i));
    }
    assert_eq!(tl.turns().len(), 2);
    assert_eq!(tl.turns()[0].outcome, TurnOutcome::Done);
    let bash = tool(&tl.turns()[1], "toolu_01UU9VJVn7Xf1cVj3XvpZs6g");
    assert_eq!((bash.status.clone(), bash.anchor), (ItemStatus::Pending, Some(anchor(6))));
    for (i, (event, payload)) in hook_list.iter().enumerate().skip(8) {
        tl.apply_hook(event, payload, Some(anchor(i)), hook_time(i));
    }
    feed_transcript(&mut tl, "claude/transcript-interactive.jsonl");
    let turns = tl.turns();
    assert_eq!(turns.len(), 3);
    assert_eq!(turns.iter().map(|t| t.index).collect::<Vec<_>>(), [1, 2, 3]);
    for (turn, id) in [(&turns[1], "toolu_01UU9VJVn7Xf1cVj3XvpZs6g"), (&turns[2], "toolu_01CdQ6S6mgeZMErPzDm8ANFE")] {
        let t = tool(turn, id);
        assert_eq!(t.status, ItemStatus::Denied);
        assert!(t.anchor.is_some());
        assert!(t.error_excerpt.is_empty());
        assert_eq!(turn.outcome, TurnOutcome::Interrupted, "「[Request interrupted by user for tool use]」");
    }
    assert_eq!(tool(&turns[2], "toolu_01CdQ6S6mgeZMErPzDm8ANFE").summary, "red or blue?");
    assert_eq!(turns.iter().map(|t| t.tokens).sum::<u64>(), 101_200);
}

#[test]
fn claude_failed_bash_and_esc() {
    let mut tl = Timeline::new(AgentKind::Claude);
    feed_transcript(&mut tl, "claude/transcript-bash-fail.jsonl");
    let turns = tl.turns();
    assert_eq!(turns.len(), 2);
    let failed = tool(&turns[0], "toolu_fail_1");
    assert_eq!(failed.status, ItemStatus::Failed { exit: Some(101) });
    assert_eq!(
        failed.error_excerpt,
        [
            "test tests::parses ... FAILED",
            "thread 'tests::parses' panicked at src/lib.rs:12:9:",
            "test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out",
        ]
    );
    assert_eq!(failed.detail.output.len(), 18, "all of it: under 20 lines / 4 KB");
    assert_eq!(failed.detail.output[0], "Exit code 101");
    assert_eq!(turns[0].outcome, TurnOutcome::Done, "a failed tool does not fail the turn");
    let thoughts = thinking(&turns[0]);
    assert_eq!(thoughts.len(), 1);
    assert_eq!(thoughts[0].0, Some(3.5));
    assert_eq!(thoughts[0].1, &["The user wants the tests run.", "I'll use cargo test."]);
    // Esc while the command ran: no result record, only the interruption.
    let slow = tool(&turns[1], "toolu_slow_1");
    assert_eq!(slow.status, ItemStatus::Interrupted);
    assert_eq!(slow.ended, Some(ms(1_790_298_067_000)));
    assert_eq!(turns[1].outcome, TurnOutcome::Interrupted);
}

#[test]
fn claude_failure_denial_and_interrupt_hooks() {
    let mut tl = Timeline::new(AgentKind::Claude);
    let p = |event: &str, extra: Value| common::payload("s", event, extra);
    tl.apply_hook("", &p("UserPromptSubmit", json!({"prompt": "go"})), None, hook_time(0));
    for (i, id) in ["t1", "t2", "t3"].iter().enumerate() {
        let pre = p(
            "PreToolUse",
            json!({"tool_name": "Bash", "tool_input": {"command": format!("cmd {id}")}, "tool_use_id": id}),
        );
        tl.apply_hook("", &pre, Some(anchor(i)), hook_time(i + 1));
    }
    let fail = p(
        "PostToolUseFailure",
        json!({"tool_name": "Bash", "tool_use_id": "t1", "error": "Exit code 2\nerror[E0425]: cannot find value `x`"}),
    );
    tl.apply_hook("", &fail, None, hook_time(5));
    let int = p(
        "PostToolUseFailure",
        json!({"tool_name": "Bash", "tool_use_id": "t2", "error": "interrupted", "is_interrupt": true}),
    );
    tl.apply_hook("", &int, None, hook_time(6));
    let denied =
        p("PermissionDenied", json!({"tool_name": "Bash", "tool_use_id": "t3", "reason": "blocked by auto mode"}));
    tl.apply_hook("", &denied, None, hook_time(7));
    // A subagent's hooks never become the session's rows.
    let sub = p(
        "PreToolUse",
        json!({"agent_id": "a1", "tool_name": "Read", "tool_input": {"file_path": "/x"}, "tool_use_id": "t9"}),
    );
    tl.apply_hook("", &sub, Some(anchor(9)), hook_time(8));
    let turn = tl.current().unwrap();
    assert_eq!(tools(turn).len(), 3);
    let t1 = tool(turn, "t1");
    assert_eq!((t1.status.clone(), t1.ended), (ItemStatus::Failed { exit: Some(2) }, Some(hook_time(5))));
    assert_eq!(t1.error_excerpt, ["error[E0425]: cannot find value `x`"]);
    assert_eq!(tool(turn, "t2").status, ItemStatus::Interrupted);
    assert_eq!(tool(turn, "t3").status, ItemStatus::Denied);
    tl.apply_hook("", &p("StopFailure", json!({"error": "rate_limit", "error_details": "429"})), None, hook_time(9));
    assert_eq!(tl.current().unwrap().outcome, TurnOutcome::Failed { message: "429".into() });
}

#[test]
fn transcript_history_goes_before_the_live_turn() {
    // gilvt attaches while turn 2 runs: the hook opens it, then the whole transcript is read from the start.
    let mut tl = Timeline::new(AgentKind::Claude);
    let hook_list = hooks("claude/hooks-interactive.jsonl");
    let (event, prompt2) = &hook_list[5];
    tl.apply_hook(event, prompt2, None, hook_time(5));
    let (event, pre) = &hook_list[6];
    tl.apply_hook(event, pre, Some(anchor(6)), hook_time(6));
    assert_eq!(tl.turns().len(), 1);
    feed_transcript(&mut tl, "claude/transcript-interactive.jsonl");
    let prompts: Vec<&str> = tl.turns().iter().map(|t| t.prompt.as_str()).collect();
    assert_eq!(
        prompts,
        [
            "Reply with the single word ok.",
            "Use the Bash tool to run exactly: touch y.txt",
            "Use AskUserQuestion to ask me: red or blue?"
        ]
    );
    assert_eq!(tl.turns().iter().map(|t| t.index).collect::<Vec<_>>(), [1, 2, 3]);
    let bash = tool(&tl.turns()[1], "toolu_01UU9VJVn7Xf1cVj3XvpZs6g");
    assert_eq!((bash.anchor, bash.status.clone()), (Some(anchor(6)), ItemStatus::Denied));
    assert_eq!(tl.turns()[1].started, Some(hook_time(5)));
}

#[test]
fn claude_hook_only_turn_is_adopted_by_the_transcript_prompt() {
    // Hooks were installed mid-turn (or gilvt restarted): the first hook seen is a `PreToolUse`, not the
    // `UserPromptSubmit` that opened the turn, so it opens an unnamed ("") turn. The transcript replaying
    // from the start must adopt that same turn instead of inserting a duplicate ahead of it.
    let mut tl = Timeline::new(AgentKind::Claude);
    let hook_list = hooks("claude/hooks-run1.jsonl");
    for (i, (event, payload)) in hook_list.iter().enumerate().skip(2) {
        tl.apply_hook(event, payload, Some(anchor(i)), hook_time(i));
    }
    assert_eq!(tl.turns().len(), 1);
    assert_eq!(tl.turns()[0].prompt, "", "no UserPromptSubmit seen yet");
    feed_transcript(&mut tl, RUN1);
    assert_eq!(tl.turns().len(), 1, "the transcript's prompt adopts the hook-only turn, not a new one");
    let turn = tl.current().unwrap();
    assert!(turn.prompt.starts_with("Do these steps in order"), "{:?}", turn.prompt);
    let search = tool(turn, "toolu_01CiHYh3Q2SV7SKP6otCMpoL");
    assert_eq!(search.anchor, Some(anchor(2)), "the hook-only rows are kept, not discarded");
}

#[test]
fn claude_transcript_only_slash_command_opens_its_own_turn() {
    // A custom slash command / skill echoes as `<command-message>…</command-message>` +
    // `<command-name>/foo</command-name>` (+ optional `<command-args>`), which M3a's session naming drops as
    // "not a prompt" — but it does run the model, and the timeline must give it its own turn.
    let mut tl = Timeline::new(AgentKind::Claude);
    let command = json!({
        "type": "user",
        "timestamp": "2026-09-25T00:23:36.000Z",
        "message": {
            "role": "user",
            "content": "<command-message>foo is running…</command-message>\n<command-name>/foo</command-name>\n<command-args>bar baz</command-args>"
        }
    });
    tl.apply_transcript_line(&command.to_string());
    let reply = json!({
        "type": "assistant",
        "timestamp": "2026-09-25T00:23:40.000Z",
        "message": {
            "id": "msg_1",
            "model": "claude-x",
            "usage": {"input_tokens": 10, "output_tokens": 5},
            "content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "echo hi"}}],
            "stop_reason": "tool_use"
        }
    });
    tl.apply_transcript_line(&reply.to_string());
    assert_eq!(tl.turns().len(), 1, "no phantom turn, and no leak into a previous turn");
    let turn = &tl.turns()[0];
    assert_eq!(turn.prompt, "/foo bar baz");
    assert_eq!(tool(turn, "t1").tool, "Bash");
}

#[test]
fn claude_slash_command_turn_matches_the_hooks_raw_prompt() {
    // The hook's `UserPromptSubmit` sends the raw text the user typed ("/foo bar baz"), not the transcript's
    // `<command-name>` / `<command-args>` wrapper: the two must still merge into one turn.
    let mut tl = Timeline::new(AgentKind::Claude);
    let p = |event: &str, extra: Value| common::payload("s", event, extra);
    tl.apply_hook("", &p("UserPromptSubmit", json!({"prompt": "/foo bar baz"})), None, hook_time(0));
    let pre = p("PreToolUse", json!({"tool_name": "Bash", "tool_input": {"command": "echo hi"}, "tool_use_id": "t1"}));
    tl.apply_hook("", &pre, Some(anchor(1)), hook_time(1));
    assert_eq!(tl.turns().len(), 1);
    let command = json!({
        "type": "user",
        "timestamp": "2026-09-25T00:23:36.000Z",
        "message": {
            "role": "user",
            "content": "<command-message>foo is running…</command-message>\n<command-name>/foo</command-name>\n<command-args>bar baz</command-args>"
        }
    });
    tl.apply_transcript_line(&command.to_string());
    let reply = json!({
        "type": "assistant",
        "timestamp": "2026-09-25T00:23:40.000Z",
        "message": {
            "id": "msg_1",
            "model": "claude-x",
            "usage": {"input_tokens": 10, "output_tokens": 5},
            "content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "echo hi"}}],
            "stop_reason": "tool_use"
        }
    });
    tl.apply_transcript_line(&reply.to_string());
    assert_eq!(tl.turns().len(), 1, "hook and transcript prompts for the same command are one turn");
    let turn = &tl.turns()[0];
    let bash = tool(turn, "t1");
    assert_eq!(bash.anchor, Some(anchor(1)), "the hook row is kept, not replaced by a second one");
    assert_eq!(bash.tool, "Bash");
}

#[test]
fn unparsable_lines_are_counted() {
    let mut tl = Timeline::new(AgentKind::Claude);
    for line in ["{not json", "42", r#"{"no":"type"}"#] {
        tl.apply_transcript_line(line);
    }
    assert_eq!(tl.unparsed_streak(), 3);
    tl.apply_transcript_line(r#"{"type":"from-the-future"}"#);
    assert_eq!(tl.unparsed_streak(), 0, "unknown record types are not failures");
    assert!(tl.turns().is_empty());
}

#[test]
fn claude_api_error_interrupts_open_rows() {
    // A hook-announced Bash call is still running when the turn ends in an API error: the failed turn
    // leaves no Running rows behind.
    let mut tl = Timeline::new(AgentKind::Claude);
    let p = |event: &str, extra: Value| common::payload("s", event, extra);
    tl.apply_hook("", &p("UserPromptSubmit", json!({"prompt": "reply ok"})), None, hook_time(0));
    let pre = p("PreToolUse", json!({"tool_name": "Bash", "tool_input": {"command": "sleep 9"}, "tool_use_id": "t1"}));
    tl.apply_hook("", &pre, Some(anchor(1)), hook_time(1));
    assert_eq!(tool(&tl.turns()[0], "t1").status, ItemStatus::Running);
    feed_transcript(&mut tl, "claude/transcript-api-error.jsonl");
    assert_eq!(tl.turns().len(), 1);
    let turn = &tl.turns()[0];
    assert!(matches!(turn.outcome, TurnOutcome::Failed { .. }), "{:?}", turn.outcome);
    assert_eq!(tool(turn, "t1").status, ItemStatus::Interrupted);
}

#[test]
fn the_turn_keeps_the_first_sentence_of_the_last_reply() {
    let mut tl = Timeline::new(AgentKind::Claude);
    feed_transcript(&mut tl, RUN1);
    assert_eq!(tl.turns()[0].reply, "ok", "the fixture's final reply is `ok`");
}

fn claude_user(text: &str) -> String {
    json!({"type": "user", "timestamp": "2026-09-29T15:00:00.000Z", "message": {"role": "user", "content": text}}).to_string()
}

fn claude_text(id: &str, text: &str, stop: &str) -> String {
    json!({"type": "assistant", "timestamp": "2026-09-29T15:00:01.000Z", "message": {"id": id, "role": "assistant",
        "model": "claude-sonnet-5-5", "content": [{"type": "text", "text": text}], "stop_reason": stop,
        "usage": {"input_tokens": 1, "output_tokens": 1}}})
    .to_string()
}

#[test]
fn the_last_reply_wins_and_only_its_first_sentence_is_kept() {
    let mut tl = Timeline::new(AgentKind::Claude);
    for line in [
        claude_user("do it"),
        claude_text("m1", "I will edit the file first.", "tool_use"),
        claude_text("m2", "Added the greeting. All tests pass, nothing else changed.", "end_turn"),
        claude_user("and now?"),
    ] {
        tl.apply_transcript_line(&line);
    }
    assert_eq!(tl.turns()[0].reply, "Added the greeting.");
    assert_eq!(tl.turns()[1].reply, "", "a turn without a reply yet has none");
}
