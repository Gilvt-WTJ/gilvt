//! The app's agent state without gpui: the registry plus the foreground tracker, fed hook requests,
//! transcript batches and foreground polls. Time and pane visibility come in, so it is unit-tested.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use gilvt_agent::{
    hook_meta, parse_hook, AgentKind, Change, Event, HookInput, HookMeta, PaneId, Registry, Session, SessionKey, Status, Store,
};

use super::foreground::{Action, Foreground, Tracker};
use super::tails::Batch;

/// A `gilvt hook` request, parsed (off the main thread).
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedHook {
    pub pane: Option<PaneId>,
    pub agent: AgentKind,
    pub meta: HookMeta,
    pub events: Vec<Event>,
    /// The payload's `permission_mode` (Claude and Codex send it on most hooks), for the status card.
    pub permission_mode: Option<String>,
}

/// None for an unknown agent or a payload without a session id.
pub fn parse_hook_request(pane: Option<u64>, agent: &str, event: &str, payload: &serde_json::Value) -> Option<ParsedHook> {
    let agent = AgentKind::from_name(agent)?;
    let meta = hook_meta(payload)?;
    let events = parse_hook(&HookInput { agent, event, payload });
    let permission_mode =
        payload.get("permission_mode").and_then(serde_json::Value::as_str).map(str::trim).filter(|m| !m.is_empty()).map(String::from);
    Some(ParsedHook { pane, agent, meta, events, permission_mode })
}

/// A key the user typed into a terminal pane, as far as answering an approval dialog goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneKey {
    Enter,
    Escape,
    /// Tab without ⇧: Claude's "amend" (type what to do instead; Enter sends it).
    Tab,
    /// A character typed without ⌘ / ⌃ / ⌥.
    Char(char),
}

/// Whether `key` answers the agent's approval dialog. Claude: a digit picks an option, Enter confirms the
/// highlighted one, Esc cancels (a no). Codex: also its letter shortcuts (y / a / n, d for "no, tell Codex").
/// Arrows, Tab (Claude's "amend") and other keys leave the dialog open.
pub fn answers_approval(agent: AgentKind, key: PaneKey) -> bool {
    match key {
        PaneKey::Enter | PaneKey::Escape => true,
        PaneKey::Tab => false,
        PaneKey::Char(c) if c.is_ascii_digit() && c != '0' => true,
        PaneKey::Char(c) => agent == AgentKind::Codex && matches!(c.to_ascii_lowercase(), 'y' | 'a' | 'n' | 'd'),
    }
}

/// How Codex (0.145, `tui.notifications`) starts the OSC 9 text of an approval prompt.
const CODEX_APPROVAL: &str = "Approval requested";

/// Spec §2.3: a lite Codex session (no PermissionRequest hook) takes its pane's OSC 9
/// `Approval requested: <action>` as the approval signal; the rest of the text is the action.
pub fn osc_approval(s: &Session, body: &str) -> Option<Event> {
    if s.agent() != AgentKind::Codex || !s.lite || !s.is_live() {
        return None;
    }
    let rest = body.trim_start().strip_prefix(CODEX_APPROVAL)?;
    let action = rest.strip_prefix(':').unwrap_or(rest).trim();
    Some(Event::PermissionNeeded { action: action.to_string() })
}

/// A transcript search the poll asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    pub pane: PaneId,
    pub agent: AgentKind,
    pub cwd: PathBuf,
    pub since: SystemTime,
}

pub struct Core {
    pub registry: Registry,
    tracker: Tracker,
    /// Latest hook `permission_mode` per session (not kept by the registry; shown on the status card).
    modes: HashMap<SessionKey, String>,
    /// Panes whose approval dialog is in "amend" (Tab): typed text is not an answer, Enter / Esc is. Keyed by
    /// the wait it was pressed in (`waiting_since`), so a later dialog starts afresh.
    amending: HashMap<PaneId, Option<Instant>>,
}

impl Core {
    pub fn new(store: Store) -> Core {
        Core { registry: Registry::new(store), tracker: Tracker::default(), modes: HashMap::new(), amending: HashMap::new() }
    }

    pub fn hook(&mut self, h: &ParsedHook, visible: impl Fn(PaneId) -> bool, now: Instant) -> Vec<Change> {
        let shown = h.pane.is_some_and(&visible);
        self.registry.apply_hook(h.pane, h.agent, &h.meta, &h.events, shown, now)
    }

    /// The session whose timeline takes a hook once [`Core::hook`] applied it: one the registry knows.
    /// Hooks it ignores (a subagent's of an unknown session, a lone SessionEnd) feed no timeline.
    pub fn timeline_key(&self, h: &ParsedHook) -> Option<SessionKey> {
        let key = (h.agent, h.meta.session_id.clone());
        self.registry.get(&key).is_some().then_some(key)
    }

    /// Remembers the hook's `permission_mode` for its session, once the registry knows the session (call after
    /// [`Core::hook`]). Returns the session when the mode changed (it may need a repaint of its own).
    pub fn note_permission_mode(&mut self, h: &ParsedHook) -> Option<SessionKey> {
        let mode = h.permission_mode.as_ref()?;
        let key = self.timeline_key(h)?;
        if self.modes.get(&key) == Some(mode) {
            return None;
        }
        self.modes.insert(key.clone(), mode.clone());
        Some(key)
    }

    /// The session's latest permission mode ("default", "plan", "bypassPermissions", …); None before any hook
    /// carried one (lite sessions never do).
    pub fn permission_mode(&self, key: &SessionKey) -> Option<&str> {
        self.modes.get(key).map(String::as_str)
    }

    /// History is applied as if seen (it must not leave 完成未看 behind); its first prompt names the session
    /// when nothing did yet (a resume sends no prompt hook).
    pub fn transcript(&mut self, key: &SessionKey, batch: &Batch, visible: impl Fn(PaneId) -> bool, now: Instant) -> Vec<Change> {
        match batch {
            Batch::Catchup { events, first_prompt } => {
                let mut changes = self.registry.apply_transcript(key, events, true, now);
                if let Some(prompt) = first_prompt {
                    for c in self.registry.name_from_history(key, prompt, now) {
                        if !changes.contains(&c) {
                            changes.push(c);
                        }
                    }
                }
                changes
            }
            Batch::Live(events) => {
                let shown = self.registry.get(key).and_then(|s| s.pane).is_some_and(&visible);
                self.registry.apply_transcript(key, events, shown, now)
            }
        }
    }

    /// One foreground poll: ends sessions of panes back at the shell (or closed) and returns the
    /// transcript searches for agents that sent no hooks.
    pub fn poll(&mut self, panes: &[(PaneId, Foreground)], now: Instant, wall: SystemTime) -> (Vec<Change>, Vec<Discovery>) {
        let registry = &self.registry;
        let actions = self.tracker.observe(panes, |p| registry.by_pane(p).is_some(), now, wall);
        let mut changes = Vec::new();
        let mut searches = Vec::new();
        for action in actions {
            match action {
                Action::Exited(pane) => changes.extend(self.registry.pane_exited(pane, now)),
                Action::Discover { pane, agent, cwd, since } => searches.push(Discovery { pane, agent, cwd, since }),
            }
        }
        (changes, searches)
    }

    /// Binds what a search found, unless a hook bound the pane meanwhile or the transcript belongs to
    /// a live session in another pane (two agents in one directory).
    pub fn bind_found(&mut self, d: &Discovery, found: Option<(String, PathBuf)>, now: Instant) -> Vec<Change> {
        let Some((id, transcript)) = found else { return Vec::new() };
        let key = (d.agent, id);
        if self.registry.by_pane(d.pane).is_some() {
            return Vec::new();
        }
        if self.registry.get(&key).is_some_and(|s| s.is_live() && s.pane.is_some_and(|p| p != d.pane)) {
            return Vec::new();
        }
        self.registry.bind_lite(d.pane, d.agent, key.1, transcript, d.cwd.clone(), now)
    }

    /// An OSC 9 / 777 text from `pane`: applied like a transcript line when [`osc_approval`] maps it.
    pub fn osc(&mut self, pane: PaneId, body: &str, visible: bool, now: Instant) -> Vec<Change> {
        let Some(s) = self.registry.by_pane(pane) else { return Vec::new() };
        let Some(event) = osc_approval(s, body) else { return Vec::new() };
        let key = s.key.clone();
        self.registry.apply_transcript(&key, &[event], visible, now)
    }

    /// A key typed into `pane`: when it answers the approval dialog of the session waiting there, that session
    /// runs again. Returns the session (its timeline takes the answer too) and the changes.
    pub fn pane_key(&mut self, pane: PaneId, key: PaneKey, visible: bool, now: Instant) -> Option<(SessionKey, Vec<Change>)> {
        let s = self.registry.by_pane(pane)?;
        if !matches!(s.status, Status::NeedsApproval { .. }) {
            self.amending.remove(&pane);
            return None;
        }
        let amending = self.amending.get(&pane) == Some(&s.waiting_since);
        let answers = match key {
            PaneKey::Tab if s.agent() == AgentKind::Claude => {
                self.amending.insert(pane, s.waiting_since);
                false
            }
            PaneKey::Enter | PaneKey::Escape => true,
            _ if amending => false,
            key => answers_approval(s.agent(), key),
        };
        if !answers {
            return None;
        }
        self.amending.remove(&pane);
        let session = s.key.clone();
        Some((session, self.registry.approval_answered(pane, visible, now)))
    }

    pub fn focused(&mut self, pane: PaneId) -> Vec<Change> {
        self.registry.pane_focused(pane)
    }

    /// M3c §2.1: the latest poll saw the pane's shell in the foreground and no live session is bound to it.
    pub fn pane_is_idle_shell(&self, pane: PaneId) -> bool {
        self.tracker.at_shell(pane) && self.registry.by_pane(pane).is_none()
    }

    /// The launcher typed a command into `pane` (see [`Tracker::typed`]).
    pub fn typed(&mut self, pane: PaneId) {
        self.tracker.typed(pane);
    }

    /// Drops an ended session (moved to the Trash) from the registry and its permission mode; a live one stays.
    pub fn forget(&mut self, key: &SessionKey) -> bool {
        let dropped = self.registry.forget(key);
        if dropped {
            self.modes.remove(key);
        }
        dropped
    }

    /// Transcripts to tail: those of every live session.
    pub fn watch_set(&self) -> Vec<(SessionKey, PathBuf)> {
        self.registry
            .sessions()
            .filter(|s| s.is_live())
            .filter_map(|s| Some((s.key.clone(), s.transcript.clone()?)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn payload(id: &str) -> serde_json::Value {
        json!({"session_id": id, "transcript_path": format!("/t/{id}.jsonl"), "cwd": "/r", "hook_event_name": "SessionStart"})
    }

    fn hook(core: &mut Core, pane: u64, event: &str, p: serde_json::Value, visible: bool, now: Instant) -> Vec<Change> {
        let h = parse_hook_request(Some(pane), "claude", event, &p).unwrap();
        core.hook(&h, |_| visible, now)
    }

    #[test]
    fn requests_parse_or_are_dropped() {
        assert!(parse_hook_request(None, "vim", "Stop", &payload("a")).is_none());
        assert!(parse_hook_request(None, "claude", "Stop", &json!({"cwd": "/r"})).is_none());
        let h = parse_hook_request(Some(3), "codex", "Stop", &payload("a")).unwrap();
        assert_eq!((h.pane, h.agent, h.meta.session_id.as_str()), (Some(3), AgentKind::Codex, "a"));
    }

    #[test]
    fn hooks_bind_sessions_to_panes_and_watch_their_transcripts() {
        let t0 = Instant::now();
        let mut core = Core::new(Store::in_memory());
        let key: SessionKey = (AgentKind::Claude, "a".into());
        assert_eq!(hook(&mut core, 1, "SessionStart", payload("a"), true, t0), vec![Change::Added(key.clone())]);
        assert_eq!(core.registry.by_pane(1).map(|s| s.key.clone()), Some(key.clone()));
        assert_eq!(core.watch_set(), vec![(key.clone(), PathBuf::from("/t/a.jsonl"))]);
        let mut stop = payload("a");
        stop["hook_event_name"] = json!("Stop");
        let mut prompt = payload("a");
        prompt["prompt"] = json!("fix the bug");
        hook(&mut core, 1, "UserPromptSubmit", prompt, false, t0);
        hook(&mut core, 1, "Stop", stop, false, t0 + Duration::from_secs(5));
        let s = core.registry.get(&key).unwrap();
        assert_eq!(s.status, Status::Idle);
        assert!(s.unseen_done, "the pane was not visible when the turn ended");
        assert_eq!(core.focused(1), vec![Change::Updated(key.clone())]);
        assert!(!core.registry.get(&key).unwrap().unseen_done);
    }

    #[test]
    fn hooks_feed_the_timeline_of_their_own_session() {
        let t0 = Instant::now();
        let mut core = Core::new(Store::in_memory());
        let h = parse_hook_request(Some(1), "codex", "SessionStart", &payload("a")).unwrap();
        assert_eq!(core.timeline_key(&h), None, "not applied yet");
        core.hook(&h, |_| true, t0);
        assert_eq!(core.timeline_key(&h), Some((AgentKind::Codex, "a".into())));
        // Same id, other agent: another session.
        let claude = parse_hook_request(Some(1), "claude", "Stop", &payload("a")).unwrap();
        assert_eq!(core.timeline_key(&claude), None);
        // A subagent hook of a session gilvt never saw is dropped by the registry and feeds nothing.
        let mut sub = payload("b");
        sub["agent_id"] = json!("ab4d");
        sub["hook_event_name"] = json!("PreToolUse");
        let h = parse_hook_request(Some(2), "claude", "PreToolUse", &sub).unwrap();
        core.hook(&h, |_| true, t0);
        assert_eq!(core.timeline_key(&h), None);
        // Once the session is known, its subagents' hooks go to its timeline.
        hook(&mut core, 2, "SessionStart", payload("b"), true, t0);
        assert_eq!(core.timeline_key(&h), Some((AgentKind::Claude, "b".into())));
    }

    #[test]
    fn hooks_remember_the_permission_mode_of_known_sessions() {
        let t0 = Instant::now();
        let mut core = Core::new(Store::in_memory());
        let key: SessionKey = (AgentKind::Claude, "a".into());
        let mut p = payload("a");
        p["permission_mode"] = json!("default");
        let h = parse_hook_request(Some(1), "claude", "SessionStart", &p).unwrap();
        assert_eq!(h.permission_mode.as_deref(), Some("default"));
        assert_eq!(core.note_permission_mode(&h), None, "unknown session");
        core.hook(&h, |_| true, t0);
        assert_eq!(core.note_permission_mode(&h), Some(key.clone()));
        assert_eq!(core.note_permission_mode(&h), None, "unchanged");
        assert_eq!(core.permission_mode(&key), Some("default"));
        p["permission_mode"] = json!("plan");
        p["hook_event_name"] = json!("UserPromptSubmit");
        let h = parse_hook_request(Some(1), "claude", "UserPromptSubmit", &p).unwrap();
        core.hook(&h, |_| true, t0);
        assert_eq!(core.note_permission_mode(&h), Some(key.clone()));
        assert_eq!(core.permission_mode(&key), Some("plan"));
        // A hook without the field (or with "") keeps the last one.
        let h = parse_hook_request(Some(1), "claude", "Stop", &payload("a")).unwrap();
        assert_eq!(h.permission_mode, None);
        assert_eq!(core.note_permission_mode(&h), None);
        assert_eq!(core.permission_mode(&key), Some("plan"));
        // Moved to the Trash: only once ended, and the mode goes with it.
        assert!(!core.forget(&key));
        core.registry.pane_exited(1, t0);
        assert!(core.forget(&key));
        assert_eq!(core.permission_mode(&key), None);
    }

    #[test]
    fn transcript_history_never_leaves_unseen_done() {
        let t0 = Instant::now();
        let mut core = Core::new(Store::in_memory());
        let d = Discovery { pane: 2, agent: AgentKind::Claude, cwd: "/r".into(), since: SystemTime::UNIX_EPOCH };
        let key: SessionKey = (AgentKind::Claude, "lite".into());
        assert_eq!(core.bind_found(&d, Some(("lite".into(), "/t/lite.jsonl".into())), t0), vec![Change::Added(key.clone())]);
        let turn = vec![Event::PromptSubmit { text: "x".into() }, Event::TurnEnd { background_tasks: 0 }];
        core.transcript(&key, &Batch::Catchup { events: turn.clone(), first_prompt: None }, |_| false, t0);
        assert!(!core.registry.get(&key).unwrap().unseen_done);
        core.transcript(&key, &Batch::Live(turn), |_| false, t0);
        assert!(core.registry.get(&key).unwrap().unseen_done);
    }

    #[test]
    fn found_transcripts_of_other_panes_are_not_stolen() {
        let t0 = Instant::now();
        let mut core = Core::new(Store::in_memory());
        hook(&mut core, 1, "SessionStart", payload("a"), true, t0);
        let d = Discovery { pane: 2, agent: AgentKind::Claude, cwd: "/r".into(), since: SystemTime::UNIX_EPOCH };
        assert!(core.bind_found(&d, Some(("a".into(), "/t/a.jsonl".into())), t0).is_empty());
        assert!(core.bind_found(&d, None, t0).is_empty());
        // A hook bound pane 1 already: its own search result is dropped.
        let d1 = Discovery { pane: 1, ..d };
        assert!(core.bind_found(&d1, Some(("b".into(), "/t/b.jsonl".into())), t0).is_empty());
    }

    #[test]
    fn polls_end_sessions_and_ask_for_lite_searches() {
        let t0 = Instant::now();
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut core = Core::new(Store::in_memory());
        hook(&mut core, 1, "SessionStart", payload("a"), true, t0);
        let agent = |pid| Foreground::Agent { kind: AgentKind::Codex, pid, cwd: Some("/w".into()) };
        let (c, s) = core.poll(&[(1, Foreground::Shell), (2, agent(9))], t0, wall);
        assert!(c.is_empty() && s.is_empty());
        let (c, s) = core.poll(&[(1, Foreground::Shell), (2, agent(9))], t0 + Duration::from_secs(3), wall);
        assert_eq!(c, vec![Change::Ended((AgentKind::Claude, "a".into()))]);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].pane, s[0].agent), (2, AgentKind::Codex));
        assert!(core.watch_set().is_empty(), "ended sessions are not tailed");
    }

    #[test]
    fn idle_shells_have_the_shell_in_front_and_no_live_session() {
        let t0 = Instant::now();
        let mut core = Core::new(Store::in_memory());
        core.poll(&[(1, Foreground::Shell), (2, Foreground::Shell), (3, Foreground::Unknown)], t0, SystemTime::UNIX_EPOCH);
        assert!(core.pane_is_idle_shell(1) && core.pane_is_idle_shell(2) && !core.pane_is_idle_shell(3));
        hook(&mut core, 2, "SessionStart", payload("a"), true, t0);
        assert!(!core.pane_is_idle_shell(2), "a hook made the pane live before the poll noticed");
        core.typed(1);
        assert!(!core.pane_is_idle_shell(1));
    }

    #[test]
    fn codex_osc_approval_marks_lite_sessions_waiting() {
        let t0 = Instant::now();
        let mut core = Core::new(Store::in_memory());
        let d = Discovery { pane: 4, agent: AgentKind::Codex, cwd: "/r".into(), since: SystemTime::UNIX_EPOCH };
        let key: SessionKey = (AgentKind::Codex, "x".into());
        core.bind_found(&d, Some(("x".into(), "/t/x.jsonl".into())), t0);
        // The text codex 0.145 prints (its command is cut with "...").
        let text = "Approval requested: /bin/bash -lc 'echo hi > hi...";
        assert!(core.osc(4, "Plan is ready", false, t0).is_empty());
        assert!(core.osc(5, text, false, t0).is_empty(), "no session in that pane");
        assert_eq!(core.osc(4, text, false, t0), vec![Change::Updated(key.clone())]);
        let s = core.registry.get(&key).unwrap();
        assert_eq!(s.status, Status::NeedsApproval { action: "/bin/bash -lc 'echo hi > hi...".into() });
        assert_eq!(s.waiting_since, Some(t0));
        // The next transcript activity clears it as usual.
        let done = Batch::Live(vec![Event::ToolEnd { tool: String::new(), ok: true }]);
        core.transcript(&key, &done, |_| true, t0 + Duration::from_secs(2));
        assert_eq!(core.registry.get(&key).unwrap().status, Status::Thinking);
    }

    #[test]
    fn osc_approval_only_for_lite_codex() {
        let t0 = Instant::now();
        let mut s = Session::new((AgentKind::Codex, "x".into()), t0);
        let text = "Approval requested: rm -rf build";
        assert_eq!(osc_approval(&s, text), None, "hooked sessions have PermissionRequest");
        s.lite = true;
        assert_eq!(osc_approval(&s, text), Some(Event::PermissionNeeded { action: "rm -rf build".into() }));
        assert_eq!(osc_approval(&s, "Approval requested"), Some(Event::PermissionNeeded { action: String::new() }));
        let by = osc_approval(&s, "Approval requested by github");
        assert_eq!(by, Some(Event::PermissionNeeded { action: "by github".into() }));
        assert_eq!(osc_approval(&s, "Agent turn complete"), None);
        s.status = Status::Ended;
        assert_eq!(osc_approval(&s, text), None);
        let mut claude = Session::new((AgentKind::Claude, "c".into()), t0);
        claude.lite = true;
        assert_eq!(osc_approval(&claude, text), None);
    }

    #[test]
    fn approval_answer_keys() {
        use PaneKey::{Char, Enter, Escape};
        for agent in [AgentKind::Claude, AgentKind::Codex] {
            for key in [Enter, Escape, Char('1'), Char('4')] {
                assert!(answers_approval(agent, key), "{agent:?} {key:?}");
            }
            assert!(!answers_approval(agent, Char('0')));
            assert!(!answers_approval(agent, Char('x')));
        }
        for c in ['y', 'a', 'n', 'd', 'Y'] {
            assert!(answers_approval(AgentKind::Codex, Char(c)), "{c}");
            assert!(!answers_approval(AgentKind::Claude, Char(c)), "{c}");
        }
    }

    #[test]
    fn a_key_answers_only_a_waiting_approval() {
        let t0 = Instant::now();
        let mut core = Core::new(Store::in_memory());
        let key: SessionKey = (AgentKind::Claude, "a".into());
        let input = json!({"command": "bash slow.sh"});
        let with = |event: &str, extra: serde_json::Value| {
            let mut p = payload("a");
            p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            p["hook_event_name"] = event.into();
            p
        };
        hook(&mut core, 4, "UserPromptSubmit", with("UserPromptSubmit", json!({"prompt": "go"})), true, t0);
        hook(&mut core, 4, "PreToolUse", with("PreToolUse", json!({"tool_name": "Bash", "tool_input": input})), true, t0);
        assert_eq!(core.pane_key(4, PaneKey::Char('1'), true, t0), None, "not waiting yet");
        hook(&mut core, 4, "PermissionRequest", with("PermissionRequest", json!({"tool_name": "Bash", "tool_input": input})), true, t0);
        assert_eq!(core.pane_key(4, PaneKey::Char('x'), true, t0), None, "not an answer");
        assert_eq!(core.pane_key(5, PaneKey::Enter, true, t0), None, "another pane");
        let answered = core.pane_key(4, PaneKey::Enter, true, t0 + Duration::from_secs(3));
        assert_eq!(answered, Some((key.clone(), vec![Change::Updated(key.clone())])));
        assert_eq!(core.registry.get(&key).unwrap().status, Status::Tool { label: "Bash(bash slow.sh)".into() });
    }

    #[test]
    fn amending_text_is_not_an_answer() {
        let t0 = Instant::now();
        let mut core = Core::new(Store::in_memory());
        let key: SessionKey = (AgentKind::Claude, "a".into());
        let ask = |core: &mut Core, now: Instant| {
            let mut p = payload("a");
            p["hook_event_name"] = "PermissionRequest".into();
            p["tool_name"] = "Bash".into();
            p["tool_input"] = json!({"command": "ls"});
            hook(core, 4, "PermissionRequest", p, true, now);
        };
        ask(&mut core, t0);
        assert_eq!(core.pane_key(4, PaneKey::Tab, true, t0), None);
        assert_eq!(core.pane_key(4, PaneKey::Char('8'), true, t0), None, "「use port 8080」");
        assert!(core.pane_key(4, PaneKey::Enter, true, t0).is_some(), "sends the amended answer");
        // The next dialog: digits answer again.
        let t1 = t0 + Duration::from_secs(9);
        hook(&mut core, 4, "UserPromptSubmit", {
            let mut p = payload("a");
            p["hook_event_name"] = "UserPromptSubmit".into();
            p["prompt"] = "again".into();
            p
        }, true, t1);
        ask(&mut core, t1);
        assert_eq!(core.pane_key(4, PaneKey::Char('1'), true, t1).map(|(k, _)| k), Some(key));
    }
}
