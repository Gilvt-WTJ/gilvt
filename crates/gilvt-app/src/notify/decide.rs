//! Whether a session's change becomes a system notification (spec §4.4), and its copy. Pure: the
//! registry's sessions, where they are seen and the time come in.
//!
//! Chain, per change: the state warrants one → the session is not muted → gilvt is not frontmost or
//! the pane is not visible → this (session, kind) was not notified yet → send. Focusing the pane
//! clears the session's record.
//!
//! OSC 9 / 777 from a pane with a live agent session goes through the mute and visibility steps too,
//! and one popup per session covers both sources: an OSC is held for [`OSC_HOLD`] and dropped when
//! gilvt posted for the session within [`OSC_DEDUPE`] before it or during the hold, or when the
//! session's current state was already notified (until the pane is focused). gilvt's own approval /
//! question / done notification is not shown (but counts as notified) within [`OSC_DEDUPE`] after a
//! shown OSC; errors and the context notice are not what agents' OSCs say, so an OSC never hides them.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gilvt_agent::{truncate_chars, AgentKind, PaneId, Session, SessionKey, Status};

use crate::sidebar::model::duration_label;

/// A finished turn notifies only when it took at least this long.
pub const TURN_MIN: Duration = Duration::from_secs(30);
/// Context use that notifies (once per session).
pub const CONTEXT_MIN: f32 = 0.9;
/// An agent's OSC and gilvt's own notification this close together are one event (Claude's
/// permission_prompt comes ~6 s after its PermissionRequest hook, spec §2.4).
pub const OSC_DEDUPE: Duration = Duration::from_secs(10);
/// How long an agent pane's OSC waits for gilvt's own notification (hooks and OSC race).
pub const OSC_HOLD: Duration = Duration::from_secs(1);
/// Body text is cut to this many characters.
const BODY_MAX: usize = 160;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Approval,
    Question,
    Error,
    Done,
    Context,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Approval => "approval",
            Kind::Question => "question",
            Kind::Error => "error",
            Kind::Done => "done",
            Kind::Context => "context",
        }
    }

    /// Only 需要你 (等待审批 / 在问你) plays a sound.
    pub fn sound(self) -> bool {
        matches!(self, Kind::Approval | Kind::Question)
    }

    /// What an agent's own OSC can be about (permission / question / turn done), so the two merge.
    fn merges_with_osc(self) -> bool {
        matches!(self, Kind::Approval | Kind::Question | Kind::Done)
    }
}

/// Where the user is: gilvt is the frontmost app; the pane is shown in the key window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub frontmost: bool,
    pub visible: bool,
}

impl Seen {
    pub fn by_user(self) -> bool {
        self.frontmost && self.visible
    }
}

/// A notification to show. `pane` is where a click goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Post {
    /// Same id replaces the earlier notification in Notification Center.
    pub id: String,
    /// Groups a session's notifications.
    pub thread: String,
    pub title: String,
    pub subtitle: String,
    pub body: String,
    pub sound: bool,
    pub pane: Option<PaneId>,
    pub session: Option<SessionKey>,
}

/// The phase of a status, for edge detection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Running,
    Approval,
    Question,
    Error,
    Ended,
}

fn phase(s: &Status) -> Phase {
    match s {
        Status::Idle => Phase::Idle,
        Status::Thinking | Status::Tool { .. } => Phase::Running,
        Status::NeedsApproval { .. } => Phase::Approval,
        Status::Asking { .. } => Phase::Question,
        Status::Error { .. } => Phase::Error,
        Status::Ended => Phase::Ended,
    }
}

/// The notification kind a session in `phase` is in (idle = its last turn's done notice).
fn phase_kind(phase: Phase) -> Option<Kind> {
    match phase {
        Phase::Approval => Some(Kind::Approval),
        Phase::Question => Some(Kind::Question),
        Phase::Error => Some(Kind::Error),
        Phase::Idle => Some(Kind::Done),
        Phase::Running | Phase::Ended => None,
    }
}

/// What the session's change warrants, given its phase before the change. Approval / question /
/// error on entry; done on entry into idle after a turn ≥ [`TURN_MIN`]; context while ≥ 90%.
fn warrants(before: Option<Phase>, s: &Session) -> Vec<Kind> {
    let now = phase(&s.status);
    let entered = before != Some(now);
    let mut out = Vec::new();
    match now {
        Phase::Approval if entered => out.push(Kind::Approval),
        Phase::Question if entered => out.push(Kind::Question),
        Phase::Error if entered => out.push(Kind::Error),
        Phase::Idle if entered && before.is_some() && s.last_turn.is_some_and(|d| d >= TURN_MIN) => out.push(Kind::Done),
        _ => {}
    }
    if now != Phase::Ended && s.context_ratio().is_some_and(|r| r >= CONTEXT_MIN) {
        out.push(Kind::Context);
    }
    out
}

#[derive(Debug, Default)]
struct Track {
    phase: Option<Phase>,
    notified: HashSet<Kind>,
    /// The 90% notice was sent (or seen): once per session, not cleared by focus.
    context_done: bool,
    last_post: Option<Instant>,
    last_osc: Option<Instant>,
}

/// The decision chain's memory: per session, the last phase and what was notified.
#[derive(Debug, Default)]
pub struct Chain {
    tracks: HashMap<SessionKey, Track>,
}

impl Chain {
    /// Runs the chain for one session after a change. `project` goes into the title.
    pub fn observe(&mut self, s: &Session, project: &str, seen: Seen, now: Instant) -> Option<Post> {
        let track = self.tracks.entry(s.key.clone()).or_default();
        let kinds = warrants(track.phase, s);
        track.phase = Some(phase(&s.status));
        // At most one edge kind at a time; a pending context notice waits for the next change.
        for kind in kinds {
            if (kind == Kind::Context && track.context_done) || s.muted {
                continue;
            }
            if seen.by_user() {
                track.context_done |= kind == Kind::Context;
                continue;
            }
            if !track.notified.insert(kind) {
                continue;
            }
            // The agent's own popup a moment ago was this one: it counts as notified.
            if kind.merges_with_osc() && track.last_osc.is_some_and(|t| now.saturating_duration_since(t) < OSC_DEDUPE) {
                continue;
            }
            track.context_done |= kind == Kind::Context;
            track.last_post = Some(now);
            return Some(own_post(s, project, kind));
        }
        None
    }

    /// The session ended: forget it.
    pub fn ended(&mut self, key: &SessionKey) {
        self.tracks.remove(key);
    }

    /// The user focused the session's pane: it may notify again.
    pub fn focused(&mut self, key: &SessionKey) {
        if let Some(t) = self.tracks.get_mut(key) {
            t.notified.clear();
        }
    }

    /// Whether an agent OSC received at `received` may show now (after its hold): gilvt did not
    /// post for the session around it, and the session's current state was not notified yet.
    pub fn osc_allowed(&self, key: &SessionKey, received: Instant) -> bool {
        let Some(t) = self.tracks.get(key) else { return true };
        let posted = t.last_post.is_some_and(|p| p >= received || received.duration_since(p) < OSC_DEDUPE);
        let notified = t.phase.and_then(phase_kind).is_some_and(|k| t.notified.contains(&k));
        !posted && !notified
    }

    pub fn osc_shown(&mut self, key: &SessionKey, now: Instant) {
        self.tracks.entry(key.clone()).or_default().last_osc = Some(now);
    }
}

/// What to do with an OSC 9 / 777 notification from a pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OscRoute {
    Drop,
    /// A plain shell pane: show now (today's behavior).
    Plain,
    /// An agent pane: hold, then ask [`Chain::osc_allowed`].
    Agent(SessionKey),
}

/// `focused` = the pane has the keyboard in the key window; `last` = when this pane last showed one.
pub fn osc_route(session: Option<&Session>, focused: bool, seen: Seen, last: Option<Instant>, now: Instant) -> OscRoute {
    if !super::script::allowed(last, now) {
        return OscRoute::Drop;
    }
    match session {
        None if focused => OscRoute::Drop,
        None => OscRoute::Plain,
        Some(s) if s.muted || seen.by_user() => OscRoute::Drop,
        Some(s) => OscRoute::Agent(s.key.clone()),
    }
}

pub fn agent_label(a: AgentKind) -> &'static str {
    match a {
        AgentKind::Claude => "Claude",
        AgentKind::Codex => "Codex",
    }
}

fn thread(s: &Session) -> String {
    format!("gilvt.{}.{}", s.agent().name(), s.session_id())
}

/// Copy per m3a-part3: title 「Claude 等待审批 · <project>」, body the detail, subtitle the session name.
fn own_post(s: &Session, project: &str, kind: Kind) -> Post {
    let agent = agent_label(s.agent());
    let (what, detail) = match (&s.status, kind) {
        (Status::NeedsApproval { action }, Kind::Approval) => ("等待审批".to_string(), action.clone()),
        (Status::Asking { question }, Kind::Question) => ("在问你".to_string(), question.clone()),
        (Status::Error { message }, Kind::Error) => ("出错".to_string(), message.clone()),
        (_, Kind::Done) => ("完成".to_string(), s.last_turn.map(|d| format!("用时 {}", duration_label(d))).unwrap_or_default()),
        (_, Kind::Context) => {
            let pct = s.context_ratio().map_or(90, |r| (r * 100.0).floor() as u32);
            ("上下文快满了".to_string(), format!("已用 {pct}%"))
        }
        (_, k) => (k.word().to_string(), String::new()),
    };
    let body = match detail.trim() {
        "" => what.clone(),
        d => truncate_chars(d, BODY_MAX),
    };
    Post {
        id: format!("{}.{}", thread(s), kind.word()),
        thread: thread(s),
        title: format!("{agent} {what} · {project}"),
        subtitle: s.name.clone(),
        body,
        sound: kind.sound(),
        pane: s.pane,
        session: Some(s.key.clone()),
    }
}

/// An agent's own OSC notification, attributed to its session.
pub fn osc_post(s: &Session, project: &str, title: Option<&str>, body: &str, seq: u64) -> Post {
    let head = format!("{} · {project}", agent_label(s.agent()));
    Post {
        id: format!("{}.osc.{seq}", thread(s)),
        thread: thread(s),
        title: title.filter(|t| !t.trim().is_empty()).map_or(head, str::to_string),
        subtitle: s.name.clone(),
        body: truncate_chars(body, BODY_MAX),
        sound: false,
        pane: s.pane,
        session: Some(s.key.clone()),
    }
}

/// A plain shell pane's OSC notification (title falls back to the pane title).
pub fn plain_post(pane: PaneId, title: Option<&str>, fallback_title: &str, body: &str, seq: u64) -> Post {
    Post {
        id: format!("gilvt.pane.{pane}.{seq}"),
        thread: format!("gilvt.pane.{pane}"),
        title: title.unwrap_or(fallback_title).to_string(),
        subtitle: String::new(),
        body: body.to_string(),
        sound: false,
        pane: Some(pane),
        session: None,
    }
}

/// Where a click on a notification for `pane` / `session` goes. A session's notification goes to its
/// pane while the pane still runs that session, else to the session's current pane if it is live
/// (pane ids restart with gilvt, so an old notification may name some other pane); a plain pane's
/// goes to its pane. `at_pane` is the live session in `pane`, `by_key` the session's record.
pub fn click_target(pane: PaneId, session: Option<&SessionKey>, at_pane: Option<&Session>, by_key: Option<&Session>) -> Option<PaneId> {
    let Some(key) = session else { return Some(pane) };
    if at_pane.is_some_and(|s| &s.key == key) {
        return Some(pane);
    }
    by_key.filter(|s| s.is_live()).and_then(|s| s.pane)
}

#[cfg(test)]
#[path = "decide_tests.rs"]
mod tests;
