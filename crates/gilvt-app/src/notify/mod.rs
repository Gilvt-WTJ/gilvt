//! System notifications: agent sessions' state changes (through the decision chain in `decide`) and
//! OSC 9 / 777 from panes. Native UNUserNotificationCenter when gilvt runs from Gilvt.app (a click
//! focuses the pane), `osascript` otherwise. Also the Dock badge and bounce (`dock`).

mod decide;
mod dock;
mod dock_tile;
mod native;
mod script;

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use gilvt_agent::{Change, PaneId, SessionKey};
use gilvt_term::Notification;
use gpui::{App, Global};

use self::decide::{click_target, osc_post, osc_route, plain_post, Chain, OscRoute, Post, Seen, OSC_HOLD};
use self::dock::Dock;
use crate::agents::Agents;
use crate::theme::AppSettings;
use crate::workspace;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Native,
    Script,
}

pub struct Notifier {
    backend: Backend,
    chain: Chain,
    /// When each pane last showed an OSC notification (rate limit).
    osc_last: HashMap<PaneId, Instant>,
    seq: u64,
    dock: Dock,
}

impl Global for Notifier {}

impl Notifier {
    fn show(&mut self, p: Post) {
        match self.backend {
            Backend::Native => native::post(p),
            Backend::Script => script::show(&p),
        }
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }
}

/// Picks the backend and, for the native one, routes clicks to `focus_pane_anywhere`.
pub fn init(cx: &mut App) {
    let backend = if native::bundled() { Backend::Native } else { Backend::Script };
    if backend == Backend::Native {
        let (tx, rx) = async_channel::unbounded::<native::Click>();
        native::install(tx);
        cx.spawn(async move |cx| {
            while let Ok((pane, session)) = rx.recv().await {
                if cx.update(|cx| clicked(pane, session, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }
    cx.set_global(Notifier { backend, chain: Chain::default(), osc_last: HashMap::new(), seq: 0, dock: Dock::default() });
}

/// A notification was clicked: focus its session's pane (only if the pane still runs that session)
/// and clear the session's 已通知 record now, as the window may not be key yet when focus lands.
fn clicked(pane: PaneId, session: Option<SessionKey>, cx: &mut App) {
    let target = cx.try_global::<Agents>().map_or(Some(pane), |a| {
        let r = a.registry();
        click_target(pane, session.as_ref(), r.by_pane(pane), session.as_ref().and_then(|k| r.get(k)))
    });
    if let Some(target) = target {
        workspace::focus_pane_anywhere(target, cx);
        Agents::pane_focused(target, cx);
    }
}

/// Is gilvt the active app (one of its windows is key)?
pub fn frontmost(cx: &mut App) -> bool {
    workspace::workspaces(cx).iter().any(|w| w.is_active(cx) == Some(true))
}

/// After a registry batch: runs the chain for every changed session. Deferred, because reading the
/// windows is not possible while one of them is being updated.
pub fn on_changes(changes: &[Change], cx: &mut App) {
    let mut seen = HashSet::new();
    let keys: Vec<(SessionKey, bool)> = changes
        .iter()
        .map(|c| match c {
            Change::Added(k) | Change::Updated(k) => (k.clone(), false),
            Change::Ended(k) => (k.clone(), true),
        })
        .filter(|(k, _)| seen.insert(k.clone()))
        .collect();
    cx.defer(move |cx| {
        if cx.try_global::<Notifier>().is_none() {
            return;
        }
        let visible = workspace::visible_panes(cx);
        let front = frontmost(cx);
        let now = Instant::now();
        let mut posts = Vec::new();
        for (key, ended) in keys {
            let agents = cx.global::<Agents>();
            let session = agents.registry().get(&key).filter(|s| !ended && s.is_live());
            let Some(s) = session else {
                cx.global_mut::<Notifier>().chain.ended(&key);
                continue;
            };
            let project = agents.project(s);
            let at = Seen { frontmost: front, visible: s.pane.is_some_and(|p| visible.contains(&p)) };
            let s = s.clone();
            if let Some(p) = cx.global_mut::<Notifier>().chain.observe(&s, &project, at, now) {
                posts.push(p);
            }
        }
        let notifier = cx.global_mut::<Notifier>();
        posts.into_iter().for_each(|p| notifier.show(p));
        update_dock(front, cx);
    });
}

/// Something the Dock badge counts changed outside a registry batch (a mute). Deferred like [`on_changes`].
pub fn dock_changed(cx: &mut App) {
    cx.defer(|cx| {
        let front = frontmost(cx);
        update_dock(front, cx);
    });
}

/// Brings the Dock badge and bounce up to date with the registry (spec §7).
fn update_dock(front: bool, cx: &mut App) {
    if cx.try_global::<Notifier>().is_none() {
        return;
    }
    let Some(agents) = cx.try_global::<Agents>() else { return };
    let now = dock::waiting(agents.registry().sessions());
    let enabled = cx.try_global::<AppSettings>().is_none_or(|s| s.0.notify.dock_bounce);
    let update = cx.global_mut::<Notifier>().dock.observe(now, front, enabled);
    if let Some(label) = update.badge {
        dock_tile::set_badge(label.as_deref());
    }
    if update.bounce {
        dock_tile::bounce();
    }
}

/// The Dock badge label gilvt last set (None = no badge) and how many times it bounced the Dock.
pub fn dock_shown(cx: &App) -> (Option<String>, u64) {
    cx.try_global::<Notifier>().map_or((None, 0), |n| (n.dock.label().map(str::to_string), n.dock.bounces()))
}

/// Pane `pane` closed: forget its OSC rate limit.
pub fn pane_closed(pane: PaneId, cx: &mut App) {
    if cx.has_global::<Notifier>() {
        cx.global_mut::<Notifier>().osc_last.remove(&pane);
    }
}

/// The user focused terminal pane `pane`: its session may notify again.
pub fn pane_focused(pane: PaneId, cx: &mut App) {
    let Some(key) = cx.try_global::<Agents>().and_then(|a| a.registry().by_pane(pane)).map(|s| s.key.clone()) else {
        return;
    };
    if cx.has_global::<Notifier>() {
        cx.global_mut::<Notifier>().chain.focused(&key);
    }
}

/// An OSC 9 / 777 notification from pane `pane`; `focused` = it has the keyboard in the key window.
/// Call from anywhere (deferred like [`on_changes`]).
pub fn osc(pane: PaneId, n: Notification, pane_title: String, focused: bool, cx: &mut App) {
    cx.defer(move |cx| {
        if cx.try_global::<Notifier>().is_none() {
            return;
        }
        let now = Instant::now();
        let visible = workspace::visible_panes(cx).contains(&pane);
        // The state first (a lite Codex approval), so gilvt's own post dedupes this OSC.
        Agents::osc(pane, &n.body, visible, cx);
        let seen = Seen { frontmost: frontmost(cx), visible };
        let session = cx.try_global::<Agents>().and_then(|a| a.registry().by_pane(pane));
        let last = cx.global::<Notifier>().osc_last.get(&pane).copied();
        match osc_route(session, focused, seen, last, now) {
            OscRoute::Drop => {}
            OscRoute::Plain => {
                let notifier = cx.global_mut::<Notifier>();
                notifier.osc_last.insert(pane, now);
                let p = plain_post(pane, n.title.as_deref(), &pane_title, &n.body, notifier.next_seq());
                notifier.show(p);
            }
            OscRoute::Agent(key) => {
                cx.global_mut::<Notifier>().osc_last.insert(pane, now);
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(OSC_HOLD).await;
                    let _ = cx.update(|cx| osc_after_hold(&key, &n, now, cx));
                })
                .detach();
            }
        }
    });
}

fn osc_after_hold(key: &SessionKey, n: &Notification, received: Instant, cx: &mut App) {
    let Some(s) = cx.global::<Agents>().registry().get(key).filter(|s| s.is_live()).cloned() else { return };
    // Muted, or the user came to the pane during the hold: the chain's steps again, as of now.
    let seen = Seen { frontmost: frontmost(cx), visible: s.pane.is_some_and(|p| workspace::visible_panes(cx).contains(&p)) };
    if osc_route(Some(&s), false, seen, None, Instant::now()) == OscRoute::Drop {
        return;
    }
    let project = cx.global::<Agents>().project(&s);
    let notifier = cx.global_mut::<Notifier>();
    if !notifier.chain.osc_allowed(key, received) {
        return;
    }
    notifier.chain.osc_shown(key, Instant::now());
    let p = osc_post(&s, &project, n.title.as_deref(), &n.body, notifier.next_seq());
    notifier.show(p);
}
