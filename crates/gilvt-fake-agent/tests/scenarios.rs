//! Anti-drift (acceptance spec §4.3): every built-in scenario runs headless, and what it produced goes
//! through gilvt-agent's real parsers, registry and timeline; hook payloads must have the recorded
//! fixtures' fields.

mod common;

use common::{keys, run, short, Opts, Run};
use gilvt_agent::{Item, ItemStatus, PlanState, TurnOutcome};
use gilvt_fake_agent::engine::{line, Scripted};
use gilvt_fake_agent::keys::Key;
use gilvt_fake_agent::recorder::Trace;
use gilvt_fake_agent::scenario::{Scenario, BUILTIN};

fn builtin(name: &str) -> Scenario {
    Scenario::builtin(name).unwrap()
}

fn statuses(expected: &[&str]) -> Vec<String> {
    expected.iter().map(|s| s.to_string()).collect()
}

/// The tool items of turn `turn`, as (tool, status).
fn tools(r: &Run, turn: usize) -> Vec<(String, ItemStatus)> {
    let tl = r.timeline();
    tl.turns()[turn]
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Tool(t) => Some((t.tool.clone(), t.status.clone())),
            _ => None,
        })
        .collect()
}

/// The hook commands really ran, with the payloads of the trace, in order.
fn assert_hooks_captured(r: &Run) {
    let traced: Vec<serde_json::Value> = r.hooks().into_iter().map(|(_, p)| p).collect();
    assert_eq!(r.captured_hooks(), traced);
}

#[test]
fn default_answers_the_command_line_prompt() {
    let r = run(&builtin("default"), vec![], Opts { prompt: Some("hello there".into()), ..Opts::default() });
    assert_eq!(r.code, 0);
    assert_eq!(r.statuses(), statuses(&["idle", "thinking", "idle", "ended"]));
    let tl = r.timeline();
    assert_eq!(tl.turns().len(), 1);
    assert_eq!((tl.turns()[0].prompt.as_str(), &tl.turns()[0].outcome), ("hello there", &TurnOutcome::Done));
    assert!(r.text().contains("❯ hello there\n⏺ OK: hello there\n"), "{}", r.text());
    assert!(r.text().contains("Resume this session with:\nclaude --resume "), "{}", r.text());
    assert_hooks_captured(&r);
}

#[test]
fn default_waits_for_a_typed_prompt_and_keeps_answering() {
    let mut k = line("first");
    k.extend(line("第二个问题"));
    k.extend(line("/exit"));
    let r = run(&builtin("default"), keys(k), Opts::default());
    assert_eq!(r.statuses(), statuses(&["idle", "thinking", "idle", "thinking", "idle", "ended"]));
    let tl = r.timeline();
    let prompts: Vec<&str> = tl.turns().iter().map(|t| t.prompt.as_str()).collect();
    assert_eq!(prompts, ["first", "第二个问题"]);
    assert!(tl.turns().iter().all(|t| t.outcome == TurnOutcome::Done));
    let (_, end) = r.hooks().pop().unwrap();
    assert_eq!(end["reason"], "prompt_input_exit");
    assert_hooks_captured(&r);
}

#[test]
fn ask_question_answered() {
    let r = run(&builtin("ask-question"), keys([Key::Down, Key::Enter]), Opts::default());
    assert_eq!(r.statuses(), statuses(&["idle", "thinking", "asking:Cats or dogs?", "thinking", "idle", "ended"]));
    let tl = r.timeline();
    assert_eq!(tl.turns()[0].outcome, TurnOutcome::Done);
    assert_eq!(tools(&r, 0), vec![("AskUserQuestion".to_string(), ItemStatus::Ok)]);
    assert!(r.text().contains(" ❯ 1. Cats\n   2. Dogs\n"), "{}", r.text());
    assert!(r.text().contains("⏺ You picked Dogs."), "{}", r.text());
    assert_hooks_captured(&r);
}

#[test]
fn ask_question_declined() {
    let r = run(&builtin("ask-question"), keys([Key::Esc]), Opts::default());
    // The rejection (transcript) resumes the session, the interruption ends the turn; no Stop fires.
    assert_eq!(r.statuses(), statuses(&["idle", "thinking", "asking:Cats or dogs?", "thinking", "idle", "ended"]));
    assert_eq!(r.timeline().turns()[0].outcome, TurnOutcome::Interrupted);
    assert!(!r.hooks().iter().any(|(e, _)| e == "Stop"));
    assert!(!r.text().contains("You picked"));
    assert_hooks_captured(&r);
}

#[test]
fn approve_bash_yes() {
    let r = run(&builtin("approve-bash"), keys([Key::Char('1')]), Opts::default());
    assert_eq!(
        r.statuses(),
        statuses(&[
            "idle",
            "thinking",
            "tool:Bash(ls build)",
            "thinking",
            "tool:Bash(rm -rf build)",
            "approval:Bash(rm -rf build)",
            "tool:Bash(rm -rf build)",
            "thinking",
            "idle",
            "ended",
        ])
    );
    assert_eq!(tools(&r, 0), vec![("Bash".into(), ItemStatus::Ok), ("Bash".into(), ItemStatus::Ok)]);
    assert!(r.text().contains("Do you want to proceed?\n ❯ 1. Yes\n   2. No\n"), "{}", r.text());
    assert_hooks_captured(&r);
}

#[test]
fn approve_bash_no_interrupts_the_turn() {
    let r = run(&builtin("approve-bash"), keys([Key::Char('n')]), Opts::default());
    let s = r.statuses();
    assert_eq!(&s[s.len() - 3..], &statuses(&["tool:Bash(rm -rf build)", "idle", "ended"])[..]);
    assert!(s.contains(&"approval:Bash(rm -rf build)".to_string()));
    assert_eq!(r.timeline().turns()[0].outcome, TurnOutcome::Interrupted);
    assert_eq!(tools(&r, 0)[1].1, ItemStatus::Denied);
    assert!(!r.text().contains("Removed build/."));
    assert_hooks_captured(&r);
}

#[test]
fn edit_files_and_a_failing_command() {
    let r = run(&builtin("edit-files"), vec![], Opts::default());
    let s = r.statuses();
    for want in ["tool:Read(README.md)", "tool:Edit(README.md)", "tool:Write(hello.txt)", "tool:Bash(cargo test)", "idle"] {
        assert!(s.contains(&want.to_string()), "{want} in {s:?}");
    }
    let t = tools(&r, 0);
    assert_eq!(t.len(), 4);
    assert_eq!(t[3], ("Bash".into(), ItemStatus::Failed { exit: Some(101) }));
    assert!(r.hooks().iter().any(|(e, p)| e == "PostToolUseFailure" && p["tool_name"] == "Bash"));
    let edit = r.records().into_iter().find(|rec| rec.pointer("/message/content/0/name") == Some(&"Edit".into())).unwrap();
    let path = r.cwd.join("README.md");
    assert_eq!(edit.pointer("/message/content/0/input/file_path").unwrap(), path.to_str().unwrap(), "{{cwd}} is filled in");
    assert_hooks_captured(&r);
}

#[test]
fn todo_list_advances() {
    let r = run(&builtin("todo"), vec![], Opts::default());
    let tl = r.timeline();
    let plan: Vec<(&str, PlanState)> = tl.plan().iter().map(|p| (p.text.as_str(), p.state)).collect();
    assert_eq!(plan, [("写测试", PlanState::Done), ("Run the tests", PlanState::Done)]);
    assert!(r.statuses().iter().any(|s| s.starts_with("tool:TodoWrite")));
    assert!(r.text().contains("⏺ Update Todos\n  ⎿  ◼ 写测试\n     ☐ Run the tests\n"), "{}", r.text());
    assert_hooks_captured(&r);
}

#[test]
fn api_error_fails_the_turn() {
    let r = run(&builtin("api-error"), vec![], Opts::default());
    assert_eq!(r.statuses(), statuses(&["idle", "thinking", "error:Network connection lost", "ended"]));
    assert_eq!(r.timeline().turns()[0].outcome, TurnOutcome::Failed { message: "Network connection lost".into() });
    assert!(!r.hooks().iter().any(|(e, _)| e == "Stop"), "StopFailure instead");
    assert!(r.text().contains("API Error: Network connection lost"));
    assert_hooks_captured(&r);
}

#[test]
fn lite_sends_no_hooks() {
    let r = run(&builtin("lite"), vec![], Opts::default());
    assert!(r.hooks().is_empty());
    assert!(!r.hooks_file.exists());
    assert_eq!(r.statuses(), statuses(&["idle", "thinking", "tool:Bash(ls)", "thinking", "idle"]));
    let found = gilvt_agent::newest_claude_transcript(&r.home, &r.cwd, common::t0() - std::time::Duration::from_secs(3600));
    assert_eq!(found, Some((r.session_id.clone(), r.transcript.clone())), "gilvt's lite discovery finds it");
}

#[test]
fn codex_basic() {
    let r = run(&builtin("codex-basic"), vec![], Opts::default());
    let s = r.statuses();
    assert_eq!(s.first().map(String::as_str), Some("idle"));
    assert_eq!(&s[s.len() - 4..], &statuses(&["tool:Bash(echo hi)", "thinking", "idle", "ended"])[..]);
    assert!(s.iter().any(|x| x.starts_with("tool:update_plan")), "{s:?}");
    let tl = r.timeline();
    assert_eq!(tl.turns()[0].outcome, TurnOutcome::Done);
    assert_eq!(tl.plan().len(), 2);
    let t = tools(&r, 0);
    assert_eq!(t.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["update_plan", "exec_command"]);
    assert!(t.iter().all(|(_, st)| *st == ItemStatus::Ok));
    assert!(r.text().contains("To continue this session, run codex resume "));
    let found = gilvt_agent::newest_codex_rollout(&r.home, &r.cwd, common::t0() - std::time::Duration::from_secs(3600));
    assert_eq!(found, Some((r.session_id.clone(), r.transcript.clone())));
    assert_hooks_captured(&r);
}

#[test]
fn codex_approve_yes_and_no() {
    let yes = run(&builtin("codex-approve"), keys([Key::Enter]), Opts::default());
    assert_eq!(
        yes.statuses(),
        statuses(&["idle", "thinking", "tool:Bash(rm -rf build)", "approval:Bash(rm -rf build)", "tool:Bash(rm -rf build)", "thinking", "idle", "ended"])
    );
    assert_hooks_captured(&yes);
    let no = run(&builtin("codex-approve"), keys([Key::Esc]), Opts::default());
    let s = no.statuses();
    assert_eq!(&s[s.len() - 4..], &statuses(&["approval:Bash(rm -rf build)", "tool:Bash(rm -rf build)", "idle", "ended"])[..]);
    assert_eq!(no.timeline().turns()[0].outcome, TurnOutcome::Interrupted);
}

#[test]
fn esc_while_a_tool_runs_interrupts() {
    let keys = vec![Scripted::During(Key::Esc)];
    let r = run(&builtin("approve-bash"), keys, Opts::default());
    let s = r.statuses();
    assert_eq!(&s[s.len() - 3..], &statuses(&["tool:Bash(ls build)", "idle", "ended"])[..], "{s:?}");
    assert_eq!(r.timeline().turns()[0].outcome, TurnOutcome::Interrupted);
    assert!(!r.hooks().iter().any(|(e, _)| e == "PermissionRequest"), "the rest of the turn is skipped");
}

#[test]
fn resuming_appends_to_the_same_transcript() {
    let first = run(&builtin("default"), vec![], Opts { prompt: Some("one".into()), ..Opts::default() });
    let before = first.records().len();
    let setup = common::Setup { dir: first.dir, home: first.home, cwd: first.cwd, hooks_file: first.hooks_file };
    let again = common::run_in(setup, &builtin("default"), vec![], Opts { prompt: Some("two".into()), resume: Some(first.session_id.clone()) });
    assert_eq!(again.transcript, first.transcript);
    let records = again.records();
    assert!(records.len() > before);
    assert_eq!(records[before]["parentUuid"], records[before - 1]["uuid"], "the uuid chain continues");
    let start = &again.hooks()[0].1;
    assert_eq!((start["source"].as_str(), start["session_id"].as_str()), (Some("resume"), Some(first.session_id.as_str())));
    let history = gilvt_agent::parse_claude_session(&again.transcript).expect("listed in the history");
    assert_eq!(history.first_prompt, "one");
    assert_eq!(history.session_id, first.session_id);
}

#[test]
fn every_builtin_is_covered() {
    let mut covered = vec!["default", "ask-question", "approve-bash", "edit-files", "todo", "api-error", "lite", "codex-basic", "codex-approve"];
    // tests/gui_scenarios.rs: the GUI acceptance cases' scenarios.
    covered.extend(["timeline", "timeline-live", "timeline-codex", "long-tool", "three-turns", "lite-mixed", "todo-states", "hist-claude", "hist-codex", "think-long", "long-title", "monitor-slow", "monitor-slow-en"]);
    // tests/gui_scenarios.rs: the artifacts tab's scenarios (the tools write real files).
    covered.extend(["artifacts-basic", "artifacts-turns", "artifacts-followups", "artifacts-many", "artifacts-running"]);
    for (name, _) in BUILTIN {
        assert!(covered.contains(&name), "add a test for the built-in scenario {name}");
    }
}

#[test]
fn trace_order_puts_prompts_before_their_records() {
    let r = run(&builtin("ask-question"), keys([Key::Enter]), Opts::default());
    let first_hook = r.trace.iter().position(|t| matches!(t, Trace::Hook { event, .. } if event == "UserPromptSubmit")).unwrap();
    let first_line = r.trace.iter().position(|t| matches!(t, Trace::Line(_))).unwrap();
    assert!(first_hook < first_line);
    assert_eq!(short(&gilvt_agent::Status::Idle), "idle");
}

#[test]
fn a_thinking_step_is_a_thinking_row_for_both_agents() {
    for agent in ["claude", "codex"] {
        let text = format!(
            "agent = \"{agent}\"\n[[step]]\nprompt = \"p\"\n[[step]]\nthinking = \"Plan: {{prompt}}\"\nms = 1200\n[[step]]\nreply = \"r\"\n[[step]]\nidle = true"
        );
        let r = run(&Scenario::parse("thinking", &text).unwrap(), vec![], Opts::default());
        let tl = r.timeline();
        let turn = &tl.turns()[0];
        assert_eq!(turn.outcome, TurnOutcome::Done, "{agent}: the thinking record does not end the turn");
        match turn.items.as_slice() {
            [Item::Thinking { secs, text }] => {
                assert_eq!(text, &["Plan: p"], "{agent}");
                // Claude: timed from the prompt record. Codex writes `agent_reasoning` right before the
                // `reasoning` item (as codex-cli does), so its duration reads as about 0.
                let want = if agent == "claude" { 1.1..2.0 } else { 0.0..0.1 };
                assert!(secs.is_some_and(|s| want.contains(&s)), "{agent}: {secs:?}");
            }
            other => panic!("{agent}: {other:?}"),
        }
        let shown = if agent == "claude" { "✻ Thought for 2s" } else { "• Thinking (2s)" };
        assert!(r.text().contains(shown), "{agent}: {}", r.text());
    }
}
