//! The key state machines: the idle prompt, approval dialogs and questions.

mod common;

use common::{keys, run, Opts};
use gilvt_agent::TurnOutcome;
use gilvt_fake_agent::engine::{line, Scripted};
use gilvt_fake_agent::keys::Key;
use gilvt_fake_agent::scenario::Scenario;

fn scenario(text: &str) -> Scenario {
    Scenario::parse("test", text).unwrap()
}

fn prompts(r: &common::Run) -> Vec<String> {
    r.timeline().turns().iter().map(|t| t.prompt.clone()).collect()
}

#[test]
fn typed_lines_echo_and_backspace_works() {
    let mut k = vec![Key::Char('a'), Key::Char('b'), Key::Backspace, Key::Char('c'), Key::Char('猫'), Key::Backspace, Key::Char('狗')];
    k.extend([Key::Enter, Key::Enter]);
    let r = run(&Scenario::builtin("default").unwrap(), keys(k), Opts::default());
    assert_eq!(prompts(&r), ["ac狗"]);
    assert!(r.screen.contains("ab\x08 \x08c猫\x08\x08  \x08\x08狗\n"), "{:?}", r.screen);
    assert!(r.text().contains("⏺ OK: ac狗"));
}

#[test]
fn empty_lines_are_ignored_and_exit_ends_the_session() {
    let mut k = vec![Key::Enter, Key::Char(' '), Key::Enter];
    k.extend([Key::Backspace]);
    k.extend(line("/exit"));
    k.extend(line("never read"));
    let r = run(&Scenario::builtin("default").unwrap(), keys(k), Opts::default());
    assert!(prompts(&r).is_empty());
    assert_eq!(r.code, 0);
    assert_eq!(r.hooks().last().unwrap().1["reason"], "prompt_input_exit");
}

#[test]
fn ctrl_c_twice_exits_and_the_first_one_hints() {
    let r = run(&Scenario::builtin("default").unwrap(), keys([Key::CtrlC, Key::CtrlC]), Opts::default());
    assert!(r.text().contains("Press Ctrl-C again to exit"));
    assert_eq!((r.code, r.hooks().last().unwrap().1["reason"].as_str()), (0, Some("prompt_input_exit")));
    // Ctrl-C with text clears the line; a key in between resets the double press.
    let mut k = vec![Key::Char('x'), Key::CtrlC, Key::CtrlC, Key::Char('y'), Key::Backspace, Key::CtrlC];
    k.extend(line("still here"));
    k.push(Key::CtrlD);
    let r = run(&Scenario::builtin("default").unwrap(), keys(k), Opts::default());
    assert_eq!(prompts(&r), ["still here"]);
}

#[test]
fn ctrl_u_clears_the_typed_line() {
    // drive.sh's retype after a lost echo: Ctrl-U, then the whole line again.
    let mut k = vec![Key::Char('c'), Key::Char('d'), Key::CtrlU];
    k.extend(line("cats"));
    k.push(Key::CtrlD);
    let r = run(&Scenario::builtin("default").unwrap(), keys(k), Opts::default());
    assert_eq!(prompts(&r), ["cats"]);
    assert!(r.screen.contains("cd\r\x1b[K"), "{:?}", r.screen);
}

#[test]
fn a_bracketed_paste_is_inserted_into_the_line() {
    // drive.sh pastes instead of typing while an input method is active; gilvt brackets it.
    let mut k = vec![Key::Char('x'), Key::CtrlU, Key::Paste("i5 粘贴的".into()), Key::Char('!')];
    k.extend([Key::Enter, Key::CtrlD]);
    let r = run(&Scenario::builtin("default").unwrap(), keys(k), Opts::default());
    assert_eq!(prompts(&r), ["i5 粘贴的!"]);
    assert!(r.text().contains("⏺ OK: i5 粘贴的!"));
}

#[test]
fn ctrl_d_on_an_empty_line_exits() {
    let r = run(&Scenario::builtin("default").unwrap(), keys([Key::Char('a'), Key::CtrlD, Key::Backspace, Key::CtrlD]), Opts::default());
    assert!(prompts(&r).is_empty());
    assert_eq!(r.hooks().last().unwrap().0, "SessionEnd");
}

const APPROVE: &str = r#"
[[step]]
prompt = "go"
[[step]]
approve = { tool = "Bash", input = { command = "rm -rf build" } }
on_no = "skip_tool"
[[step]]
reply = "after"
[[step]]
idle = true
"#;

/// Whether the approved command ran (PostToolUse fired).
fn ran(r: &common::Run) -> bool {
    r.hooks().iter().any(|(e, _)| e == "PostToolUse")
}

#[test]
fn approval_keys() {
    let s = scenario(APPROVE);
    for (k, yes) in [
        (vec![Key::Char('y')], true),
        (vec![Key::Char('1')], true),
        (vec![Key::Enter], true),
        (vec![Key::Char('n')], false),
        (vec![Key::Char('2')], false),
        (vec![Key::Esc], false),
        (vec![Key::CtrlC], false),
        (vec![Key::Down, Key::Enter], false),
        (vec![Key::Down, Key::Up, Key::Enter], true),
        (vec![Key::Char('x'), Key::Up, Key::Char('1')], true),
    ] {
        let r = run(&s, keys(k.clone()), Opts::default());
        assert_eq!(ran(&r), yes, "{k:?}");
        // skip_tool: the turn goes on either way.
        assert!(r.text().contains("⏺ after"), "{k:?}");
        assert_eq!(r.timeline().turns()[0].outcome, TurnOutcome::Done, "{k:?}");
    }
    let r = run(&s, keys([Key::Down, Key::Enter]), Opts::default());
    assert!(r.screen.contains("\x1b[2A\r\x1b[J   1. Yes\n ❯ 2. No\n"), "the cursor is redrawn");
}

#[test]
fn question_keys() {
    let s = Scenario::builtin("ask-question").unwrap();
    for (k, answer) in [
        (vec![Key::Enter], "Cats"),
        (vec![Key::Char('2'), Key::Enter], "Dogs"),
        (vec![Key::Down, Key::Down, Key::Enter], "Dogs"),
        (vec![Key::Up, Key::Enter], "Cats"),
        (vec![Key::Char('9'), Key::Enter], "Cats"),
    ] {
        let r = run(&s, keys(k.clone()), Opts::default());
        assert!(r.text().contains(&format!("You picked {answer}.")), "{k:?}");
        let answered = r.hooks().into_iter().find(|(e, _)| e == "PostToolUse").unwrap().1;
        assert_eq!(answered["tool_response"]["answers"]["Cats or dogs?"], answer);
    }
}

#[test]
fn the_command_line_prompt_replaces_the_first_prompt_step() {
    let r = run(&scenario(APPROVE), keys([Key::Enter]), Opts { prompt: Some("from argv".into()), ..Opts::default() });
    assert_eq!(prompts(&r), ["from argv"]);
}

#[test]
fn exit_steps_set_the_exit_code() {
    let r = run(&scenario("[[step]]\nprompt = \"a\"\n[[step]]\nreply = \"b\"\n[[step]]\nidle = true\n[[step]]\nexit = 7"), vec![], Opts::default());
    assert_eq!(r.code, 7);
    let (event, payload) = r.hooks().pop().unwrap();
    assert_eq!((event.as_str(), payload["reason"].as_str()), ("SessionEnd", Some("other")));
    assert!(r.text().contains("claude --resume "));
}

#[test]
fn esc_during_a_sleep_interrupts_the_turn() {
    let s = scenario("[[step]]\nprompt = \"a\"\n[[step]]\nsleep = \"5s\"\n[[step]]\nreply = \"b\"\n[[step]]\nidle = true\n[[step]]\nreply = \"next turn\"");
    let mut k = vec![Scripted::During(Key::Esc)];
    k.extend(keys(line("again")));
    let r = run(&s, k, Opts::default());
    let tl = r.timeline();
    assert_eq!(tl.turns()[0].outcome, TurnOutcome::Interrupted);
    assert_eq!((tl.turns()[1].prompt.as_str(), &tl.turns()[1].outcome), ("again", &TurnOutcome::Done));
    assert!(!r.text().contains("⏺ b\n") && r.text().contains("⏺ next turn"));
}

#[test]
fn a_fixed_session_id_is_used() {
    let r = run(&scenario("session = \"00000000-0000-4000-8000-0000000000aa\"\n[[step]]\nreply = \"x\""), vec![], Opts { prompt: Some("p".into()), ..Opts::default() });
    assert_eq!(r.session_id, "00000000-0000-4000-8000-0000000000aa");
    assert!(r.transcript.ends_with("00000000-0000-4000-8000-0000000000aa.jsonl"));
    assert!(r.hooks().iter().all(|(_, p)| p["session_id"] == "00000000-0000-4000-8000-0000000000aa"));
}

#[test]
fn claude_sets_the_title_and_codex_does_not() {
    let r = run(&Scenario::builtin("default").unwrap(), vec![], Opts { prompt: Some("fix it".into()), ..Opts::default() });
    assert!(r.screen.contains("\x1b]0;⠂ fix it\x07") && r.screen.contains("\x1b]0;✳ Claude Code\x07"));
    let quiet = run(&scenario("cwd_title = false\n[[step]]\nreply = \"x\""), vec![], Opts { prompt: Some("p".into()), ..Opts::default() });
    assert!(!quiet.screen.contains("\x1b]0;"));
    let codex = run(&Scenario::builtin("codex-basic").unwrap(), vec![], Opts::default());
    assert!(!codex.screen.contains("\x1b]0;"));
}

#[test]
fn keys_typed_while_a_tool_runs_reach_the_next_dialog() {
    // `1` arrives during the 300 ms `ls`: it answers the approval that follows, as in a real terminal.
    let r = run(&Scenario::builtin("approve-bash").unwrap(), vec![Scripted::During(Key::Char('1'))], Opts::default());
    assert!(r.text().contains("⏺ Removed build/."), "{}", r.text());
    assert_eq!(r.timeline().turns()[0].outcome, TurnOutcome::Done);
}
