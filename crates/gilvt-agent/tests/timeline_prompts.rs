//! User records that are not turns (records as Claude Code 2.1.284 writes them): `!` shell commands, a
//! subagent's hand-back, built-in slash commands.

use gilvt_agent::{parse_hook, AgentKind, Event, HookInput, Timeline};
use serde_json::{json, Value};

fn user(text: &str) -> String {
    json!({"type": "user", "timestamp": "2026-09-29T15:00:00.000Z", "message": {"role": "user", "content": text}}).to_string()
}

fn reply(id: &str) -> String {
    json!({"type": "assistant", "timestamp": "2026-09-29T15:00:01.000Z", "message": {"id": id, "role": "assistant",
        "model": "claude-sonnet-5-5", "content": [{"type": "text", "text": "done"}], "stop_reason": "end_turn",
        "usage": {"input_tokens": 10, "output_tokens": 5}}})
    .to_string()
}

fn prompts(tl: &Timeline) -> Vec<(u32, &str)> {
    tl.turns().iter().map(|t| (t.index, t.prompt.as_str())).collect()
}

fn hook_prompt(tl: &mut Timeline, prompt: &str) {
    let p = json!({"session_id": "s", "hook_event_name": "UserPromptSubmit", "prompt": prompt});
    tl.apply_hook("UserPromptSubmit", &p, None, std::time::SystemTime::now());
}

#[test]
fn shell_commands_are_not_turns() {
    let mut tl = Timeline::new(AgentKind::Claude);
    for line in [user("run the tests"), reply("m1"), user("<bash-input>ls</bash-input>"),
        user("<bash-stdout>calc docs</bash-stdout><bash-stderr></bash-stderr>"), user("<bash-input>tree</bash-input>"),
        user("<bash-stdout></bash-stdout><bash-stderr>/bin/bash: tree: command not found\n</bash-stderr>")]
    {
        tl.apply_transcript_line(&line);
    }
    hook_prompt(&mut tl, "<bash-input>ls</bash-input>");
    assert_eq!(prompts(&tl), [(1, "run the tests")]);
}

#[test]
fn a_subagent_hand_back_is_not_a_turn() {
    let mut tl = Timeline::new(AgentKind::Claude);
    tl.apply_transcript_line(&user("find the bug"));
    tl.apply_transcript_line(&reply("m1"));
    hook_prompt(&mut tl, "<agent-message from=\"afac3c0331747e188\">\n[Subagent hand-back] The report follows:\n  It is `//`.\n</agent-message>");
    let peer = json!({"type": "user", "isMeta": true, "timestamp": "2026-09-29T15:00:02.000Z",
        "origin": {"kind": "peer", "from": "afac3c0331747e188", "handback": true},
        "message": {"role": "user", "content": "Another Claude session sent a message:\n<agent-message from=\"afac3c0331747e188\">…</agent-message>"}});
    tl.apply_transcript_line(&peer.to_string());
    assert_eq!(prompts(&tl), [(1, "find the bug")]);
}

#[test]
fn a_built_in_slash_command_leaves_no_turn() {
    let mut tl = Timeline::new(AgentKind::Claude);
    tl.apply_transcript_line(&user("first"));
    tl.apply_transcript_line(&reply("m1"));
    tl.apply_transcript_line(&user("<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args></command-args>"));
    tl.apply_transcript_line(&user("<local-command-stdout>Set model to opus</local-command-stdout>"));
    tl.apply_transcript_line(&user("second"));
    tl.apply_transcript_line(&reply("m2"));
    tl.apply_transcript_line(&user("<command-name>/exit</command-name>\n<command-message>exit</command-message>\n<command-args></command-args>"));
    tl.apply_transcript_line(&user("<local-command-stdout>See ya!</local-command-stdout>"));
    assert_eq!(prompts(&tl), [(1, "first"), (2, "second")]);
    assert_eq!(tl.current().unwrap().tokens, 15, "later records go to the turn before");
}

#[test]
fn a_custom_slash_command_keeps_its_turn() {
    let mut tl = Timeline::new(AgentKind::Claude);
    tl.apply_transcript_line(&user("<command-message>review</command-message>\n<command-name>/review</command-name>"));
    tl.apply_transcript_line(&reply("m1"));
    tl.apply_transcript_line(&user("<local-command-stdout>(no content)</local-command-stdout>"));
    assert_eq!(prompts(&tl), [(1, "/review")]);
}

#[test]
fn m3a_names_sessions_after_real_prompts_only() {
    let parse = |prompt: &str| -> Vec<Event> {
        let p: Value = json!({"session_id": "s", "prompt": prompt});
        parse_hook(&HookInput { agent: AgentKind::Claude, event: "UserPromptSubmit", payload: &p })
    };
    for text in ["<bash-input>ls</bash-input>", "<bash-stdout>x</bash-stdout>", "<agent-message from=\"a\">r</agent-message>"] {
        assert_eq!(parse(text), vec![], "{text}");
    }
    assert_eq!(parse("fix it"), vec![Event::PromptSubmit { text: "fix it".into() }]);
}
