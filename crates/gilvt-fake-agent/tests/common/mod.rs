//! Running scenarios headless (temp HOME, real `sh -c` hooks appending to a file) and replaying what they
//! produced into gilvt-agent's parsers, registry and timeline.

#![allow(dead_code)] // each test binary uses a different subset

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gilvt_agent::{hook_meta, parse_hook, parse_transcript_line, AgentKind, HookInput, Registry, Status, Store, Timeline};
use gilvt_fake_agent::claude::{self, ClaudeSession};
use gilvt_fake_agent::codex::{self, CodexSession};
use gilvt_fake_agent::engine::{Engine, ScriptedIo, Scripted, Settings};
use gilvt_fake_agent::hooks::{codex_hook_key, trust_hash, Hooks};
use gilvt_fake_agent::recorder::{Output, Recorder, Trace};
use gilvt_fake_agent::scenario::Scenario;
use gilvt_fake_agent::Agent;
use serde_json::{json, Value};

/// The events gilvt's `--settings` file hooks (gilvt-cli `hook/claude.rs`), with its matchers.
pub const CLAUDE_EVENTS: [&str; 15] = [
    "SessionStart", "SessionEnd", "UserPromptSubmit", "PreToolUse", "PermissionRequest", "PostToolUse", "PostToolUseFailure",
    "PermissionDenied", "Notification", "Stop", "StopFailure", "SubagentStart", "SubagentStop", "PreCompact", "PostCompact",
];
const CLAUDE_MATCHER_EVENTS: [&str; 6] =
    ["PreToolUse", "PermissionRequest", "PostToolUse", "PostToolUseFailure", "PermissionDenied", "Notification"];
/// The events gilvt passes to codex as `-c hooks.<Event>=…` (gilvt-cli `hook/codex.rs`).
pub const CODEX_EVENTS: [&str; 9] =
    ["SessionStart", "UserPromptSubmit", "PreToolUse", "PermissionRequest", "PostToolUse", "Stop", "SessionEnd", "SubagentStart", "SubagentStop"];

pub struct Run {
    pub dir: tempfile::TempDir,
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub agent: Agent,
    pub session_id: String,
    pub transcript: PathBuf,
    pub hooks_file: PathBuf,
    pub trace: Vec<Trace>,
    pub screen: String,
    pub code: i32,
}

pub fn t0() -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(1_790_303_043_230)
}

pub struct Setup {
    pub dir: tempfile::TempDir,
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub hooks_file: PathBuf,
}

impl Setup {
    pub fn new() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (home, cwd) = (root.join("home"), root.join("proj"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        Setup { dir, home, cwd, hooks_file: root.join("hooks.jsonl") }
    }

    /// The command every hook runs: append the payload (one JSON line) to `hooks_file`.
    pub fn capture_command(&self) -> String {
        let f = self.hooks_file.display();
        format!("cat >> '{f}'; echo >> '{f}'")
    }

    /// A `--settings` file shaped like gilvt's (`{"hooks": {<Event>: [{"matcher": "*"?, "hooks": [...]}]}}`).
    pub fn claude_settings(&self) -> PathBuf {
        let mut hooks = serde_json::Map::new();
        for event in CLAUDE_EVENTS {
            let mut group = json!({"hooks": [{"type": "command", "command": self.capture_command()}]});
            if CLAUDE_MATCHER_EVENTS.contains(&event) {
                group["matcher"] = "*".into();
            }
            hooks.insert(event.into(), json!([group]));
        }
        let path = self.dir.path().join("settings.json");
        std::fs::write(&path, serde_json::to_string_pretty(&json!({"hooks": hooks})).unwrap()).unwrap();
        path
    }

    /// `-c` flags shaped like gilvt's: `hooks.<Event>=[{hooks=[{type="command",command="…"}]}]` plus the
    /// `hooks.state` trust record the fake's app-server reported.
    pub fn codex_config(&self) -> Vec<String> {
        let cmd = self.capture_command();
        let quoted = format!("\"{}\"", cmd.replace('\\', "\\\\").replace('"', "\\\""));
        let mut flags: Vec<String> = CODEX_EVENTS.iter().map(|e| format!("hooks.{e}=[{{hooks=[{{type=\"command\",command={quoted}}}]}}]")).collect();
        let state: Vec<String> = CODEX_EVENTS
            .iter()
            .map(|e| format!("\"{}\"={{trusted_hash=\"{}\"}}", codex_hook_key(e, 0, 0), trust_hash(e, &cmd)))
            .collect();
        flags.push(format!("hooks.state={{{}}}", state.join(",")));
        flags
    }
}

/// Options for [`run`].
#[derive(Default)]
pub struct Opts {
    pub prompt: Option<String>,
    pub resume: Option<String>,
}

/// Runs `scenario` headless with `keys`.
pub fn run(scenario: &Scenario, keys: Vec<Scripted>, opts: Opts) -> Run {
    let setup = Setup::new();
    run_in(setup, scenario, keys, opts)
}

pub fn run_in(setup: Setup, scenario: &Scenario, keys: Vec<Scripted>, opts: Opts) -> Run {
    let agent = scenario.agent.unwrap_or(Agent::Claude);
    let hooks = match (scenario.lite, agent) {
        (true, _) => Hooks::none(),
        (false, Agent::Claude) => Hooks::from_claude_settings(setup.claude_settings().to_str().unwrap(), &setup.cwd).unwrap(),
        (false, Agent::Codex) => Hooks::from_codex_config(&setup.codex_config()),
    };
    let settings = Settings {
        agent,
        cwd: setup.cwd.clone(),
        initial_prompt: opts.prompt.clone(),
        resumed: opts.resume.is_some(),
        cwd_title: scenario.cwd_title,
    };
    let io = ScriptedIo::new(t0(), keys);
    let steps = scenario.steps.clone();
    let (session_id, transcript, trace, screen, code) = match agent {
        Agent::Claude => {
            let id = opts.resume.clone().or(scenario.session.clone()).unwrap_or_else(gilvt_fake_agent::clock::uuid_v4);
            let path = claude::transcript_path(&setup.home, &setup.cwd, &id);
            let rec = ClaudeSession::new(id.clone(), setup.cwd.clone(), Output::new(path.clone(), hooks, scenario.lite).with_trace());
            finish(Engine::new(rec, io, steps, settings), id, path)
        }
        Agent::Codex => {
            let id = opts.resume.clone().or(scenario.session.clone()).unwrap_or_else(|| gilvt_fake_agent::clock::uuid_v7(t0()));
            let path = codex::find_rollout(&setup.home, &id).unwrap_or_else(|| codex::rollout_path(&setup.home, t0(), &id));
            let rec = CodexSession::new(id.clone(), setup.cwd.clone(), Output::new(path.clone(), hooks, scenario.lite).with_trace());
            finish(Engine::new(rec, io, steps, settings), id, path)
        }
    };
    let Setup { dir, home, cwd, hooks_file } = setup;
    Run { dir, home, cwd, agent, session_id, transcript, hooks_file, trace, screen, code }
}

fn finish<R: Recorder>(mut e: Engine<R, ScriptedIo>, id: String, path: PathBuf) -> (String, PathBuf, Vec<Trace>, String, i32) {
    let code = e.run();
    (id, path, e.rec.out().trace().to_vec(), e.io.screen.clone(), code)
}

pub fn keys(k: impl IntoIterator<Item = gilvt_fake_agent::keys::Key>) -> Vec<Scripted> {
    k.into_iter().map(Scripted::Key).collect()
}

impl Run {
    pub fn kind(&self) -> AgentKind {
        match self.agent {
            Agent::Claude => AgentKind::Claude,
            Agent::Codex => AgentKind::Codex,
        }
    }

    /// The screen without OSC titles and CSI sequences.
    pub fn text(&self) -> String {
        strip_ansi(&self.screen)
    }

    /// Hook payloads in order, from the trace.
    pub fn hooks(&self) -> Vec<(String, Value)> {
        self.trace
            .iter()
            .filter_map(|t| match t {
                Trace::Hook { event, payload } => Some((event.clone(), payload.clone())),
                _ => None,
            })
            .collect()
    }

    /// What the hook commands really received (the capture file).
    pub fn captured_hooks(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(&self.hooks_file).unwrap_or_default();
        text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    pub fn lines(&self) -> Vec<String> {
        self.trace
            .iter()
            .filter_map(|t| match t {
                Trace::Line(l) => Some(l.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn records(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(&self.transcript).unwrap();
        text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    /// The session's status after every trace item, consecutive duplicates removed, as gilvt's registry
    /// computes it (hooks bind the session to pane 1; a lite session is bound like gilvt's foreground poll
    /// does, from the transcript alone).
    pub fn statuses(&self) -> Vec<String> {
        let agent = self.kind();
        let key = (agent, self.session_id.clone());
        let mut reg = Registry::new(Store::in_memory());
        let t0 = Instant::now();
        let mut secs = 0;
        let mut now = || {
            secs += 1;
            t0 + Duration::from_secs(secs)
        };
        let mut out: Vec<String> = Vec::new();
        let lite = !self.trace.iter().any(|t| matches!(t, Trace::Hook { .. }));
        if lite {
            reg.bind_lite(1, agent, self.session_id.clone(), self.transcript.clone(), self.cwd.clone(), now());
            out.extend(reg.get(&key).map(|s| short(&s.status)));
        }
        for item in &self.trace {
            match item {
                Trace::Hook { event, payload } => {
                    let events = parse_hook(&HookInput { agent, event, payload });
                    let meta = hook_meta(payload).expect("hook identity");
                    reg.apply_hook(Some(1), agent, &meta, &events, true, now());
                }
                Trace::Line(line) => {
                    reg.apply_transcript(&key, &parse_transcript_line(agent, line), true, now());
                }
                Trace::ApprovalAnswered => {
                    reg.approval_answered(1, true, now());
                }
            }
            if let Some(s) = reg.get(&key) {
                let short = short(&s.status);
                if out.last() != Some(&short) {
                    out.push(short);
                }
            }
        }
        out
    }

    /// The session's timeline, fed like gilvt feeds it (hooks and transcript lines in order).
    pub fn timeline(&self) -> Timeline {
        let mut tl = Timeline::new(self.kind());
        let mut t = t0();
        for item in &self.trace {
            t += Duration::from_millis(10);
            match item {
                Trace::Hook { event, payload } => tl.apply_hook(event, payload, None, t),
                Trace::Line(line) => tl.apply_transcript_line(line),
                Trace::ApprovalAnswered => tl.approval_answered(t),
            }
        }
        tl
    }
}

pub fn short(s: &Status) -> String {
    match s {
        Status::Idle => "idle".into(),
        Status::Thinking => "thinking".into(),
        Status::Tool { label } => format!("tool:{label}"),
        Status::NeedsApproval { action } => format!("approval:{action}"),
        Status::Asking { question } => format!("asking:{question}"),
        Status::Error { message } => format!("error:{message}"),
        Status::Ended => "ended".into(),
    }
}

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../gilvt-agent/tests/fixtures")
}

/// Every JSON line of the fixture files under `dir` (recursively) whose name matches `pred`.
pub fn fixture_lines(dir: &Path, pred: &dyn Fn(&str) -> bool) -> Vec<Value> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "jsonl") && pred(&p.file_name().unwrap().to_string_lossy()) {
                let text = std::fs::read_to_string(&p).unwrap();
                out.extend(text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()));
            }
        }
    }
    out
}

pub fn keys_of(v: &Value) -> BTreeSet<String> {
    v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
}

/// `s` without OSC (`ESC ] … BEL`) and CSI (`ESC [ … final`) sequences.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(']') => {
                for c in chars.by_ref() {
                    if c == '\x07' {
                        break;
                    }
                }
            }
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}
