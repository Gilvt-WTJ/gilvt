//! All sessions gilvt knows about in this run, bound to panes, fed by hooks and transcripts.

use std::path::PathBuf;
use std::time::Instant;

use crate::event::{AgentKind, Event, HookMeta};
use crate::status::{session_name, step, PaneId, Session, SessionKey, Status};
use crate::store::Store;

/// What changed, for the UI / notifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Added(SessionKey),
    Updated(SessionKey),
    Ended(SessionKey),
}

/// The app-wide session registry (shared across windows). Ended sessions stay listed for this run.
pub struct Registry {
    sessions: Vec<Session>,
    store: Store,
}

impl Registry {
    pub fn new(store: Store) -> Self {
        Registry { sessions: Vec::new(), store }
    }

    /// Hook path: binds pane + session and applies the events. A new session in a pane ends the pane's
    /// previous one. Hooks from inside a subagent (`meta.agent_id`) never create a session, and neither
    /// does a lone `SessionEnd` of a session this run never saw.
    pub fn apply_hook(
        &mut self,
        pane: Option<PaneId>,
        agent: AgentKind,
        meta: &HookMeta,
        events: &[Event],
        pane_visible: bool,
        now: Instant,
    ) -> Vec<Change> {
        let key = (agent, meta.session_id.clone());
        let mut changes = Vec::new();
        let added = match self.index(&key) {
            Some(_) => false,
            None if meta.agent_id.is_some() || events.iter().all(|e| *e == Event::SessionEnd) => return changes,
            None => {
                self.insert(Session::new(key.clone(), now));
                true
            }
        };
        if let Some(pane) = pane {
            changes.extend(self.end_others_in_pane(pane, &key, now));
        }
        let i = self.index(&key).expect("inserted above");
        let s = &mut self.sessions[i];
        let was_live = s.is_live();
        let mut changed = bind(s, pane, meta.cwd.clone(), meta.transcript_path.clone(), now);
        if s.lite {
            s.lite = false;
            changed = true;
        }
        for event in events {
            changed |= step(s, event, pane_visible, now);
        }
        changes.extend(change_of(s, added, was_live, changed));
        changes
    }

    /// Transcript path. Lite sessions take every event; hooked sessions take only what hooks lack: usage and
    /// model, interruptions (Esc), for Claude rejections, for Codex errors.
    pub fn apply_transcript(
        &mut self,
        key: &SessionKey,
        events: &[Event],
        pane_visible: bool,
        now: Instant,
    ) -> Vec<Change> {
        let Some(i) = self.index(key) else { return Vec::new() };
        let s = &mut self.sessions[i];
        let was_live = s.is_live();
        let (lite, agent) = (s.lite, s.agent());
        let mut changed = false;
        for event in events.iter().filter(|e| lite || supplements(agent, e)) {
            changed |= step(s, event, pane_visible, now);
        }
        change_of(s, false, was_live, changed).into_iter().collect()
    }

    /// The first real prompt of the session's history (its transcript as it was when gilvt started watching):
    /// names a session that has no automatic name yet, e.g. one resumed from an earlier run, which sends no
    /// prompt hook for it. The user's rename still wins; a name already taken from a prompt is kept.
    pub fn name_from_history(&mut self, key: &SessionKey, first_prompt: &str, now: Instant) -> Vec<Change> {
        let Some(i) = self.index(key) else { return Vec::new() };
        let s = &mut self.sessions[i];
        if !s.auto_name.is_empty() {
            return Vec::new();
        }
        s.auto_name = session_name(first_prompt);
        if s.auto_name.is_empty() {
            return Vec::new();
        }
        if !s.renamed {
            s.name = s.auto_name.clone();
            s.updated = now;
            return vec![Change::Updated(key.clone())];
        }
        Vec::new()
    }

    /// Fallback binding when no hook arrived: the foreground program in `pane` is an agent and `transcript`
    /// is the newest transcript for its cwd. Never replaces a live hooked session in that pane.
    pub fn bind_lite(
        &mut self,
        pane: PaneId,
        agent: AgentKind,
        session_id: String,
        transcript: PathBuf,
        cwd: PathBuf,
        now: Instant,
    ) -> Vec<Change> {
        let key = (agent, session_id);
        if self.by_pane(pane).is_some_and(|s| !s.lite && s.key != key) {
            return Vec::new();
        }
        let mut changes = self.end_others_in_pane(pane, &key, now);
        match self.index(&key) {
            Some(i) => {
                let s = &mut self.sessions[i];
                if bind(s, Some(pane), Some(cwd), Some(transcript), now) {
                    changes.push(Change::Updated(key));
                }
            }
            None => {
                let mut s = Session::new(key.clone(), now);
                s.lite = true;
                bind(&mut s, Some(pane), Some(cwd), Some(transcript), now);
                self.insert(s);
                changes.push(Change::Added(key));
            }
        }
        changes
    }

    /// Binds a process discovered outside gilvt to a session. Polling an already-known process does not
    /// reset its live hook/transcript status on every pass.
    pub fn bind_external(
        &mut self,
        agent: AgentKind,
        session_id: String,
        transcript: Option<PathBuf>,
        cwd: Option<PathBuf>,
        exact: bool,
        now: Instant,
    ) -> Vec<Change> {
        let key = (agent, session_id);
        match self.index(&key) {
            Some(i) => {
                let session = &mut self.sessions[i];
                let mut changed = bind(session, None, cwd, transcript, now);
                if exact && session.lite {
                    session.lite = false;
                    changed = true;
                }
                changed.then(|| Change::Updated(key)).into_iter().collect()
            }
            None => {
                let mut session = Session::new(key.clone(), now);
                session.lite = !exact;
                bind(&mut session, None, cwd, transcript, now);
                self.insert(session);
                vec![Change::Added(key)]
            }
        }
    }

    /// The foreground of `pane` went back to the shell, or the pane closed: its live sessions end.
    pub fn pane_exited(&mut self, pane: PaneId, now: Instant) -> Vec<Change> {
        let mut changes = Vec::new();
        for s in self.sessions.iter_mut().filter(|s| s.pane == Some(pane) && s.is_live()) {
            step(s, &Event::SessionEnd, false, now);
            changes.push(Change::Ended(s.key.clone()));
        }
        changes
    }

    /// A process scanner proved that an external session's process ended. Pane-bound sessions are owned by
    /// the pane foreground tracker and are deliberately left alone here.
    pub fn external_exited(&mut self, key: &SessionKey, now: Instant) -> Vec<Change> {
        let Some(i) = self.index(key) else { return Vec::new() };
        let session = &mut self.sessions[i];
        if session.pane.is_some() || !session.is_live() {
            return Vec::new();
        }
        step(session, &Event::SessionEnd, false, now);
        vec![Change::Ended(key.clone())]
    }

    /// Removes an ephemeral unbound external-process row instead of retaining it as session history.
    pub fn remove_external(&mut self, key: &SessionKey) -> bool {
        let Some(index) = self.index(key) else { return false };
        if self.sessions[index].pane.is_some() {
            return false;
        }
        self.sessions.remove(index);
        true
    }

    /// The user answered an approval dialog in `pane` (see [`Event::ApprovalAnswered`]). Only a live session
    /// waiting for an approval there changes.
    pub fn approval_answered(&mut self, pane: PaneId, pane_visible: bool, now: Instant) -> Vec<Change> {
        let Some(i) = self.by_pane(pane).and_then(|s| self.index(&s.key)) else { return Vec::new() };
        let s = &mut self.sessions[i];
        if !matches!(s.status, Status::NeedsApproval { .. }) {
            return Vec::new();
        }
        let changed = step(s, &Event::ApprovalAnswered, pane_visible, now);
        change_of(s, false, true, changed).into_iter().collect()
    }

    /// The user focused this pane: clears 完成未看.
    pub fn pane_focused(&mut self, pane: PaneId) -> Vec<Change> {
        let mut changes = Vec::new();
        for s in self.sessions.iter_mut().filter(|s| s.pane == Some(pane) && s.unseen_done) {
            s.unseen_done = false;
            changes.push(Change::Updated(s.key.clone()));
        }
        changes
    }

    /// Renames a session (persisted); an empty name restores the automatic one.
    pub fn rename(&mut self, key: &SessionKey, name: String) {
        let stored = Some(name.trim().to_string()).filter(|n| !n.is_empty());
        if let Some(i) = self.index(key) {
            self.sessions[i].set_rename(stored.clone());
        }
        let _ = self.store.update(key, |e| e.name = stored);
    }

    /// Mutes / unmutes a session's notifications (persisted).
    pub fn set_muted(&mut self, key: &SessionKey, muted: bool) {
        if let Some(i) = self.index(key) {
            self.sessions[i].muted = muted;
        }
        let _ = self.store.update(key, |e| e.muted = muted);
    }

    /// Drops an ended session from this run's list (its files went to the Trash, M3c §4.3). A live session
    /// stays; returns whether one was dropped. The stored rename / mute are kept for a restore from the Trash.
    pub fn forget(&mut self, key: &SessionKey) -> bool {
        match self.index(key) {
            Some(i) if !self.sessions[i].is_live() => {
                self.sessions.remove(i);
                true
            }
            _ => false,
        }
    }

    /// The user's rename of a session (persisted), also for sessions not seen in this run, e.g. a history
    /// entry. None: never renamed, or the rename was cleared.
    pub fn saved_name(&self, key: &SessionKey) -> Option<String> {
        self.store.get(key).name
    }

    pub fn get(&self, key: &SessionKey) -> Option<&Session> {
        self.sessions.iter().find(|s| &s.key == key)
    }

    /// The live (non-Ended) session in `pane`.
    pub fn by_pane(&self, pane: PaneId) -> Option<&Session> {
        self.sessions.iter().filter(|s| s.pane == Some(pane) && s.is_live()).max_by_key(|s| s.updated)
    }

    /// All sessions, in the order they were first seen.
    pub fn sessions(&self) -> impl Iterator<Item = &Session> {
        self.sessions.iter()
    }

    fn index(&self, key: &SessionKey) -> Option<usize> {
        self.sessions.iter().position(|s| &s.key == key)
    }

    fn insert(&mut self, mut s: Session) {
        let saved = self.store.get(&s.key);
        s.muted = saved.muted;
        if saved.name.is_some() {
            s.set_rename(saved.name);
        }
        self.sessions.push(s);
    }

    fn end_others_in_pane(&mut self, pane: PaneId, key: &SessionKey, now: Instant) -> Vec<Change> {
        let mut changes = Vec::new();
        for s in self.sessions.iter_mut().filter(|s| s.pane == Some(pane) && &s.key != key && s.is_live()) {
            step(s, &Event::SessionEnd, false, now);
            changes.push(Change::Ended(s.key.clone()));
        }
        changes
    }
}

/// Updates where the session lives; returns whether anything changed.
fn bind(
    s: &mut Session,
    pane: Option<PaneId>,
    cwd: Option<PathBuf>,
    transcript: Option<PathBuf>,
    now: Instant,
) -> bool {
    let before = (s.pane, s.cwd.clone(), s.transcript.clone());
    s.pane = pane.or(s.pane);
    s.cwd = cwd.or(s.cwd.take());
    s.transcript = transcript.or(s.transcript.take());
    let changed = before != (s.pane, s.cwd.clone(), s.transcript.clone());
    if changed {
        s.updated = now;
    }
    changed
}

/// What a hooked session takes from its transcript: what no hook reports.
fn supplements(agent: AgentKind, event: &Event) -> bool {
    match event {
        Event::Usage { .. } | Event::Model { .. } => true,
        // Esc fires no hook (Claude: nor does rejecting an approval or dismissing a question).
        Event::Interrupted => true,
        Event::PermissionDenied => agent == AgentKind::Claude,
        // An aborted Codex turn fires no hook.
        Event::Error { .. } => agent == AgentKind::Codex,
        _ => false,
    }
}

fn change_of(s: &Session, added: bool, was_live: bool, changed: bool) -> Option<Change> {
    if added {
        Some(Change::Added(s.key.clone()))
    } else if was_live && s.status == Status::Ended {
        Some(Change::Ended(s.key.clone()))
    } else {
        changed.then(|| Change::Updated(s.key.clone()))
    }
}
