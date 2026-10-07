//! Fixture loading and a registry driver shared by the integration tests.

#![allow(dead_code)] // each test binary uses a different subset

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gilvt_agent::{
    hook_meta, parse_hook, parse_transcript_line, AgentKind, Change, Event, HookInput, PaneId, Registry, Session,
    SessionKey, Store,
};
use serde_json::{json, Value};

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

pub fn lines(name: &str) -> Vec<String> {
    let text = std::fs::read_to_string(fixture(name)).unwrap();
    text.lines().map(str::to_string).collect()
}

/// Each payload of a hook fixture with its event name, as `gilvt hook <agent> <Event>` delivers it.
pub fn hooks(name: &str) -> Vec<(String, Value)> {
    lines(name)
        .iter()
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            (v["hook_event_name"].as_str().unwrap().to_string(), v)
        })
        .collect()
}

pub fn hook_events(agent: AgentKind, name: &str) -> Vec<Vec<Event>> {
    hooks(name).iter().map(|(event, payload)| parse_hook(&HookInput { agent, event, payload })).collect()
}

pub fn transcript_events(agent: AgentKind, name: &str) -> Vec<Vec<Event>> {
    lines(name).iter().map(|l| parse_transcript_line(agent, l)).collect()
}

/// A payload in the shape both agents send: identity fields plus `extra`.
pub fn payload(session_id: &str, event: &str, extra: Value) -> Value {
    let mut p = json!({
        "session_id": session_id,
        "transcript_path": format!("/Users/u/.claude/projects/-Users-u-proj/{session_id}.jsonl"),
        "cwd": "/Users/u/proj",
        "hook_event_name": event,
    });
    p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    p
}

/// A registry driven with injected time: every call happens one second after the previous one.
pub struct Driver {
    pub reg: Registry,
    pub t0: Instant,
    pub secs: u64,
    pub visible: bool,
}

impl Driver {
    pub fn new() -> Driver {
        Driver { reg: Registry::new(Store::in_memory()), t0: Instant::now(), secs: 0, visible: true }
    }

    pub fn now(&self) -> Instant {
        self.t0 + Duration::from_secs(self.secs)
    }

    pub fn tick(&mut self) -> Instant {
        self.secs += 1;
        self.now()
    }

    /// Delivers one hook payload from `pane`.
    pub fn hook(&mut self, pane: Option<PaneId>, agent: AgentKind, payload: &Value) -> Vec<Change> {
        let now = self.tick();
        let event = payload["hook_event_name"].as_str().unwrap_or("");
        let events = parse_hook(&HookInput { agent, event, payload });
        let meta = hook_meta(payload).unwrap();
        self.reg.apply_hook(pane, agent, &meta, &events, self.visible, now)
    }

    /// Delivers transcript lines for `key`, one call per line.
    pub fn transcript(&mut self, key: &SessionKey, lines: &[String]) -> Vec<Change> {
        let mut changes = Vec::new();
        for line in lines {
            let now = self.tick();
            let events = parse_transcript_line(key.0, line);
            changes.extend(self.reg.apply_transcript(key, &events, self.visible, now));
        }
        changes
    }

    pub fn session(&self, key: &SessionKey) -> &Session {
        self.reg.get(key).unwrap()
    }
}

pub fn key(agent: AgentKind, id: &str) -> SessionKey {
    (agent, id.to_string())
}
