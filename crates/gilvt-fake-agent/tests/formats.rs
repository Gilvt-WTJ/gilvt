//! The fake's hook payloads and records against the recorded fixtures of gilvt-agent: every payload has
//! the fields the real CLI always sends for that event, and none it never sends; every record type the
//! fake writes exists in the fixtures and uses only fields seen there.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{fixture_lines, fixtures, keys, keys_of, run, Opts, Run};
use gilvt_fake_agent::keys::Key;
use gilvt_fake_agent::scenario::Scenario;
use serde_json::Value;

/// Events no fixture recorded; their payloads follow the parsers (`gilvt-agent/src/{claude,codex}.rs`).
const CLAUDE_UNRECORDED: [&str; 2] = ["PostToolUseFailure", "StopFailure"];
const CODEX_UNRECORDED: [&str; 1] = ["PermissionRequest"];

/// A `thinking` step for both agents (no built-in Codex scenario has one).
const THINKING: &str = "[[step]]\nprompt = \"think\"\n[[step]]\nthinking = \"Plan it\"\nms = 10\n[[step]]\nreply = \"done\"\n[[step]]\nidle = true";

/// Every built-in run of `agent`, with keys that reach every branch (answers, rejections), and [`THINKING`].
fn runs(codex: bool) -> Vec<Run> {
    let cases: &[(&str, &[Key])] = if codex {
        &[
            ("codex-basic", &[]),
            ("codex-approve", &[Key::Enter]),
            ("codex-approve", &[Key::Esc]),
            ("timeline-codex", &[]),
            ("hist-codex", &[]),
        ]
    } else {
        &[
            ("default", &[]),
            ("ask-question", &[Key::Enter]),
            ("ask-question", &[Key::Esc]),
            ("approve-bash", &[Key::Char('y')]),
            ("approve-bash", &[Key::Char('n')]),
            ("edit-files", &[]),
            ("todo", &[]),
            ("api-error", &[]),
            ("lite", &[]),
            ("timeline", &[]),
            ("long-tool", &[]),
            ("three-turns", &[]),
            ("lite-mixed", &[Key::Char('n')]),
            ("todo-states", &[]),
            ("hist-claude", &[]),
        ]
    };
    let thinking = Scenario::parse("thinking", &format!("agent = \"{}\"\n{THINKING}", if codex { "codex" } else { "claude" })).unwrap();
    cases
        .iter()
        .map(|(name, k)| (Scenario::builtin(name).unwrap(), k.to_vec()))
        .chain([(thinking, Vec::new())])
        .map(|(s, k)| run(&s, keys(k), Opts { prompt: Some("go".into()), ..Opts::default() }))
        .collect()
}

/// Fixture hook payloads by event (Notification: by `Notification/<type>`).
fn fixture_hooks(agent: &str) -> BTreeMap<String, Vec<Value>> {
    let mut by: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for p in fixture_lines(&fixtures().join(agent), &|n| n.starts_with("hooks-")) {
        by.entry(hook_kind(&p)).or_default().push(p);
    }
    by
}

fn hook_kind(p: &Value) -> String {
    let event = p["hook_event_name"].as_str().unwrap_or("").to_string();
    match p["notification_type"].as_str() {
        Some(t) if event == "Notification" => format!("{event}/{t}"),
        _ => event,
    }
}

fn union_and_intersection(values: &[Value]) -> (BTreeSet<String>, BTreeSet<String>) {
    let sets: Vec<BTreeSet<String>> = values.iter().map(keys_of).collect();
    let union = sets.iter().flatten().cloned().collect();
    let inter = sets.iter().skip(1).fold(sets[0].clone(), |acc, s| acc.intersection(s).cloned().collect());
    (union, inter)
}

/// Fields the interactive CLI sends with every event (`scratchpad_dir`): allowed on any event, even one only
/// recorded from another entrypoint (Claude's SessionEnd comes from the desktop app's run).
fn everywhere(agent: &str) -> BTreeSet<String> {
    let interactive = fixture_lines(&fixtures().join(agent), &|n| n == "hooks-interactive.jsonl");
    if interactive.is_empty() {
        return BTreeSet::new();
    }
    union_and_intersection(&interactive).1
}

fn check_hooks(codex: bool) {
    let (agent, unrecorded): (&str, &[&str]) = if codex { ("codex", &CODEX_UNRECORDED) } else { ("claude", &CLAUDE_UNRECORDED) };
    let recorded = fixture_hooks(agent);
    let common = everywhere(agent);
    let mut seen = BTreeSet::new();
    for r in runs(codex) {
        for (event, payload) in r.hooks() {
            assert_eq!(payload["hook_event_name"], event.as_str());
            let kind = hook_kind(&payload);
            seen.insert(kind.clone());
            let Some(fixture) = recorded.get(&kind) else {
                assert!(unrecorded.contains(&event.as_str()), "{agent} {kind}: no fixture recorded it; add it to *_UNRECORDED if intended");
                continue;
            };
            let (mut union, inter) = union_and_intersection(fixture);
            union.extend(common.iter().cloned());
            let keys = keys_of(&payload);
            let missing: Vec<_> = inter.difference(&keys).collect();
            let invented: Vec<_> = keys.difference(&union).collect();
            assert!(missing.is_empty() && invented.is_empty(), "{agent} {kind}: missing {missing:?}, not in fixtures {invented:?}");
        }
    }
    let must: &[&str] = if codex {
        &["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "PermissionRequest", "Stop", "SessionEnd"]
    } else {
        &[
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PermissionRequest",
            "Notification/permission_prompt",
            "Stop",
            "StopFailure",
            "SessionEnd",
        ]
    };
    for kind in must {
        assert!(seen.contains(*kind), "no built-in scenario sends {agent} {kind}");
    }
}

#[test]
fn claude_hook_payloads_match_the_fixtures() {
    check_hooks(false);
}

#[test]
fn codex_hook_payloads_match_the_fixtures() {
    check_hooks(true);
}

/// A record's kind: Claude `type[/subtype]`, Codex `type[/payload.type]`.
fn record_kind(r: &Value, codex: bool) -> String {
    let t = r["type"].as_str().unwrap_or("").to_string();
    let sub = if codex { r.pointer("/payload/type") } else { r.get("subtype") };
    match sub.and_then(Value::as_str) {
        Some(s) => format!("{t}/{s}"),
        None => t,
    }
}

/// Field names of a record, one level into `payload` / `message` (`payload.cwd`, `message.usage`).
fn record_keys(r: &Value) -> BTreeSet<String> {
    let mut keys = keys_of(r);
    for inner in ["payload", "message"] {
        if let Some(Value::Object(o)) = r.get(inner) {
            keys.extend(o.keys().map(|k| format!("{inner}.{k}")));
        }
    }
    keys
}

fn check_records(codex: bool) {
    let dir = if codex { "codex" } else { "claude" };
    let mut known: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for root in [fixtures().join(dir), fixtures().join(if codex { "history/home/.codex" } else { "history/home/.claude" })] {
        for r in fixture_lines(&root, &|n| !n.starts_with("hooks-")) {
            known.entry(record_kind(&r, codex)).or_default().extend(record_keys(&r));
        }
    }
    for run in runs(codex) {
        for r in run.records() {
            let kind = record_kind(&r, codex);
            let fields = known.get(&kind).unwrap_or_else(|| panic!("{dir}: record kind {kind} is in no fixture"));
            let invented: Vec<_> = record_keys(&r).into_iter().filter(|k| !fields.contains(k)).collect();
            assert!(invented.is_empty(), "{dir} {kind}: fields no fixture has: {invented:?}");
        }
    }
}

#[test]
fn claude_records_use_fixture_fields() {
    check_records(false);
}

#[test]
fn codex_records_use_fixture_fields() {
    check_records(true);
}

#[test]
fn claude_records_carry_what_gilvt_reads() {
    let r = run(&Scenario::builtin("ask-question").unwrap(), keys([Key::Enter]), Opts::default());
    let records = r.records();
    let mut parent: Option<Value> = None;
    for rec in &records {
        assert_eq!(rec["sessionId"], r.session_id.as_str());
        assert_eq!(rec["cwd"], r.cwd.to_str().unwrap());
        assert_eq!((rec["isSidechain"].as_bool(), rec["entrypoint"].as_str()), (Some(false), Some("cli")));
        assert!(rec["timestamp"].as_str().is_some_and(|t| t.ends_with('Z')));
        assert_eq!(rec["parentUuid"], parent.clone().unwrap_or(Value::Null), "one uuid chain");
        parent = Some(rec["uuid"].clone());
    }
    let uses: Vec<&Value> = records.iter().filter(|r| r.pointer("/message/content/0/type") == Some(&"tool_use".into())).collect();
    let results: Vec<&Value> = records.iter().filter(|r| r.pointer("/message/content/0/type") == Some(&"tool_result".into())).collect();
    assert_eq!(uses.len(), results.len());
    for (u, res) in uses.iter().zip(&results) {
        assert_eq!(u.pointer("/message/content/0/id"), res.pointer("/message/content/0/tool_use_id"), "paired");
        assert_eq!(res["sourceToolAssistantUUID"], u["uuid"]);
    }
    let reply = records.iter().rev().find(|r| r["type"] == "assistant").unwrap();
    assert!(reply.pointer("/message/usage/cache_read_input_tokens").and_then(Value::as_u64).is_some());
    assert_eq!(reply.pointer("/message/stop_reason"), Some(&"end_turn".into()));
    let ask = uses[0].pointer("/message/content/0/input/questions/0").unwrap();
    assert_eq!((ask["question"].as_str(), ask["options"][1]["label"].as_str()), (Some("Cats or dogs?"), Some("Dogs")));
}
