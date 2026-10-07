//! What runs in the foreground of each terminal pane, polled about once a second: an agent that sends
//! no hooks is bound to its transcript after a grace period (lite mode), and a pane whose foreground
//! went back to the shell (or that closed) ends its sessions. Pure: observations and time come in.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use gilvt_agent::{AgentKind, PaneId};

/// Hooks normally arrive within milliseconds of an agent starting; without one by then, fall back.
pub const LITE_DELAY: Duration = Duration::from_secs(3);
/// Claude writes its transcript only after the first prompt: look again this often.
pub const RETRY: Duration = Duration::from_secs(3);
/// A transcript counts if it was written after the agent started, less this much clock slack.
pub const SINCE_SLACK: Duration = Duration::from_secs(2);
/// Consecutive polls with the shell in the foreground before the pane's sessions end
/// (the shell is briefly in front while a wrapper function starts the agent).
pub const SHELL_POLLS: u32 = 2;
/// After the launcher typed into a pane it stays busy this long, or until a poll sees a non-shell foreground
/// (the agent started): a shell reading (stale, or `cd` / a wrapper function being in front) never clears it.
pub const TYPED_HOLD: Duration = Duration::from_secs(5);

/// The foreground program of a pane, classified on a background thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Foreground {
    Shell,
    Agent { kind: AgentKind, pid: u32, cwd: Option<PathBuf> },
    /// Some other program (an editor, a build).
    Other,
    /// The lookup failed (the process group leader already exited).
    Unknown,
}

/// What to do after a poll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// The pane's live sessions end.
    Exited(PaneId),
    /// Look for the newest transcript of `agent` for `cwd` written at or after `since`, then bind it.
    Discover { pane: PaneId, agent: AgentKind, cwd: PathBuf, since: SystemTime },
}

#[derive(Default)]
struct Track {
    /// The agent in the foreground: kind, pid, when first seen (monotonic and wall clock).
    agent: Option<(AgentKind, u32, Instant, SystemTime)>,
    shell_polls: u32,
    last_try: Option<Instant>,
    /// The latest poll saw the shell in the foreground (and nothing was typed there by the launcher since).
    at_shell: bool,
    /// The launcher typed here: shell readings keep the pane busy until this instant.
    hold_until: Option<Instant>,
}

#[derive(Default)]
pub struct Tracker {
    panes: HashMap<PaneId, Track>,
}

impl Tracker {
    /// One poll over every terminal pane that exists now. `has_live(pane)`: the registry has a live
    /// session bound to the pane.
    pub fn observe(
        &mut self,
        panes: &[(PaneId, Foreground)],
        has_live: impl Fn(PaneId) -> bool,
        now: Instant,
        wall: SystemTime,
    ) -> Vec<Action> {
        let mut actions: Vec<Action> = Vec::new();
        let gone: Vec<PaneId> = self.panes.keys().copied().filter(|p| !panes.iter().any(|(q, _)| q == p)).collect();
        for pane in gone {
            self.panes.remove(&pane);
            if has_live(pane) {
                actions.push(Action::Exited(pane));
            }
        }
        for (pane, fg) in panes {
            let track = self.panes.entry(*pane).or_default();
            track.at_shell = match fg {
                Foreground::Shell => match track.hold_until {
                    Some(until) if now < until => false,
                    _ => {
                        track.hold_until = None;
                        true
                    }
                },
                Foreground::Agent { .. } | Foreground::Other => {
                    track.hold_until = None;
                    false
                }
                Foreground::Unknown => false,
            };
            match fg {
                Foreground::Shell => {
                    track.agent = None;
                    track.last_try = None;
                    track.shell_polls = track.shell_polls.saturating_add(1);
                    if track.shell_polls >= SHELL_POLLS && has_live(*pane) {
                        actions.push(Action::Exited(*pane));
                    }
                }
                Foreground::Agent { kind, pid, cwd } => {
                    track.shell_polls = 0;
                    if !matches!(track.agent, Some((k, p, ..)) if k == *kind && p == *pid) {
                        track.agent = Some((*kind, *pid, now, wall));
                        track.last_try = None;
                    }
                    let (_, _, since, since_wall) = track.agent.expect("set above");
                    let due = now.saturating_duration_since(since) >= LITE_DELAY
                        && track.last_try.is_none_or(|t| now.saturating_duration_since(t) >= RETRY);
                    if let (true, false, Some(cwd)) = (due, has_live(*pane), cwd) {
                        track.last_try = Some(now);
                        let since = since_wall.checked_sub(SINCE_SLACK).unwrap_or(since_wall);
                        actions.push(Action::Discover { pane: *pane, agent: *kind, cwd: cwd.clone(), since });
                    }
                }
                Foreground::Other => {
                    track.agent = None;
                    track.shell_polls = 0;
                }
                Foreground::Unknown => {}
            }
        }
        actions
    }

    /// The latest poll of `pane` saw its shell in the foreground. False before the first poll, after an
    /// `Unknown` lookup and after [`Tracker::typed`].
    pub fn at_shell(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).is_some_and(|t| t.at_shell)
    }

    /// The launcher typed a command into `pane`: it no longer counts as sitting at the shell until the next
    /// poll says so again (a second launch within the poll interval must not type into it too).
    pub fn typed(&mut self, pane: PaneId) {
        self.typed_for(pane, Instant::now(), TYPED_HOLD);
    }

    /// [`Tracker::typed`] with an explicit start and hold. Also works for a pane no poll has seen yet (a new
    /// pane): its track is created so the hold survives until the first poll.
    pub fn typed_for(&mut self, pane: PaneId, now: Instant, hold: Duration) {
        let t = self.panes.entry(pane).or_default();
        t.at_shell = false;
        t.hold_until = Some(now + hold);
    }
}

/// Shells whose presence in the foreground means nothing else runs in the pane.
pub fn is_shell(name: &str) -> bool {
    matches!(name, "zsh" | "bash" | "fish" | "sh" | "dash" | "ksh" | "tcsh" | "csh" | "login")
}

/// Agent kind of a foreground program `name` (argv only looked at for interpreters): gilvt-agent's
/// rules plus the command names configured under `[agent]` (matched as given or without `.exe`).
pub fn classify(name: &str, argv: Option<&str>, claude: &[String], codex: &[String]) -> Option<AgentKind> {
    let base = gilvt_agent::process_basename(name);
    let named = |c: &String| c == name || c == base;
    if claude.iter().any(named) {
        return Some(AgentKind::Claude);
    }
    if codex.iter().any(named) {
        return Some(AgentKind::Codex);
    }
    gilvt_agent::agent_of_process(name, argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(pid: u32) -> Foreground {
        Foreground::Agent { kind: AgentKind::Claude, pid, cwd: Some("/r".into()) }
    }

    #[test]
    fn lite_discovery_after_the_grace_period_and_retries() {
        let (t0, w0) = (Instant::now(), SystemTime::UNIX_EPOCH + Duration::from_secs(1000));
        let mut tr = Tracker::default();
        let at = |s: u64| (t0 + Duration::from_secs(s), w0 + Duration::from_secs(s));
        let obs = |tr: &mut Tracker, s: u64, live: bool| {
            let (now, wall) = at(s);
            tr.observe(&[(1, agent(50))], |_| live, now, wall)
        };
        assert!(obs(&mut tr, 0, false).is_empty());
        assert!(obs(&mut tr, 2, false).is_empty(), "within the grace period");
        let since = w0 - SINCE_SLACK;
        assert_eq!(obs(&mut tr, 3, false), vec![Action::Discover { pane: 1, agent: AgentKind::Claude, cwd: "/r".into(), since }]);
        assert!(obs(&mut tr, 4, false).is_empty(), "not again before RETRY");
        assert_eq!(obs(&mut tr, 6, false).len(), 1);
        assert!(obs(&mut tr, 9, true).is_empty(), "a hook (or the lite bind) made the pane live");
    }

    #[test]
    fn a_new_agent_process_restarts_the_grace_period() {
        let t0 = Instant::now();
        let w = SystemTime::UNIX_EPOCH;
        let mut tr = Tracker::default();
        tr.observe(&[(1, agent(50))], |_| false, t0, w);
        let a = tr.observe(&[(1, agent(51))], |_| false, t0 + Duration::from_secs(4), w);
        assert!(a.is_empty());
        let a = tr.observe(&[(1, agent(51))], |_| false, t0 + Duration::from_secs(7), w);
        assert_eq!(a.len(), 1);
    }

    #[test]
    fn shell_in_front_twice_ends_live_sessions() {
        let t0 = Instant::now();
        let w = SystemTime::UNIX_EPOCH;
        let mut tr = Tracker::default();
        assert!(tr.observe(&[(1, Foreground::Shell)], |_| true, t0, w).is_empty(), "one poll is not enough");
        assert_eq!(tr.observe(&[(1, Foreground::Shell)], |_| true, t0, w), vec![Action::Exited(1)]);
        // Nothing live: nothing to end.
        assert!(tr.observe(&[(1, Foreground::Shell)], |_| false, t0, w).is_empty());
        // Another program in between resets the count; unknown lookups change nothing.
        let mut tr = Tracker::default();
        tr.observe(&[(1, Foreground::Shell)], |_| true, t0, w);
        tr.observe(&[(1, Foreground::Other)], |_| true, t0, w);
        assert!(tr.observe(&[(1, Foreground::Shell)], |_| true, t0, w).is_empty());
        assert!(tr.observe(&[(1, Foreground::Unknown)], |_| true, t0, w).is_empty());
        assert_eq!(tr.observe(&[(1, Foreground::Shell)], |_| true, t0, w), vec![Action::Exited(1)]);
    }

    #[test]
    fn closed_panes_end_their_sessions() {
        let t0 = Instant::now();
        let w = SystemTime::UNIX_EPOCH;
        let mut tr = Tracker::default();
        tr.observe(&[(1, agent(50)), (2, agent(60))], |_| true, t0, w);
        assert_eq!(tr.observe(&[(2, agent(60))], |p| p == 1, t0, w), vec![Action::Exited(1)]);
        assert!(tr.observe(&[], |_| false, t0, w).is_empty());
    }

    #[test]
    fn the_latest_poll_tells_whether_a_pane_sits_at_its_shell() {
        let t0 = Instant::now();
        let w = SystemTime::UNIX_EPOCH;
        let mut tr = Tracker::default();
        assert!(!tr.at_shell(1), "never polled");
        tr.observe(&[(1, Foreground::Shell), (2, agent(50)), (3, Foreground::Other)], |_| false, t0, w);
        assert!(tr.at_shell(1) && !tr.at_shell(2) && !tr.at_shell(3));
        tr.observe(&[(1, Foreground::Unknown)], |_| false, t0, w);
        assert!(!tr.at_shell(1), "an unknown lookup is not idle");
        tr.observe(&[(1, Foreground::Shell)], |_| false, t0, w);
        tr.typed(1);
        assert!(!tr.at_shell(1), "typed into: busy");
        tr.observe(&[(1, Foreground::Shell)], |_| false, t0, w);
        assert!(!tr.at_shell(1), "a (stale or early) shell reading does not clear it");
        tr.observe(&[(1, Foreground::Other)], |_| false, t0, w);
        tr.observe(&[(1, Foreground::Shell)], |_| false, t0, w);
        assert!(tr.at_shell(1), "the program ran and ended");
        tr.typed(1);
        tr.observe(&[(1, Foreground::Shell)], |_| false, t0 + TYPED_HOLD + Duration::from_secs(1), w);
        assert!(tr.at_shell(1), "hold elapsed");
        tr.typed(1);
        tr.observe(&[(1, Foreground::Unknown)], |_| false, t0, w);
        tr.observe(&[(1, Foreground::Shell)], |_| false, t0, w);
        assert!(!tr.at_shell(1), "unknown does not release the hold");
    }

    #[test]
    fn typed_before_the_first_poll_and_long_holds() {
        let t0 = Instant::now();
        let w = SystemTime::UNIX_EPOCH;
        let mut tr = Tracker::default();
        tr.typed_for(7, t0, Duration::from_secs(8));
        assert!(!tr.at_shell(7));
        tr.observe(&[(7, Foreground::Shell)], |_| false, t0 + Duration::from_secs(7), w);
        assert!(!tr.at_shell(7), "still inside the 8 s hold");
        tr.observe(&[(7, Foreground::Shell)], |_| false, t0 + Duration::from_secs(8), w);
        assert!(tr.at_shell(7), "hold over");
        let mut tr = Tracker::default();
        tr.typed(8);
        tr.observe(&[(8, Foreground::Shell)], |_| false, t0, w);
        assert!(!tr.at_shell(8), "typed() on an unpolled pane");
        tr.observe(&[], |_| false, t0, w);
        assert!(!tr.at_shell(1), "closed");
    }

    #[test]
    fn classification() {
        let claude = vec!["claude".to_string(), "cc".to_string()];
        let codex = vec!["codex".to_string(), "codex-w".to_string()];
        assert_eq!(classify("cc", None, &claude, &codex), Some(AgentKind::Claude));
        assert_eq!(classify("claude.exe", None, &["claude".to_string()], &[]), Some(AgentKind::Claude));
        assert_eq!(classify("claude.exe", None, &[], &[]), Some(AgentKind::Claude));
        assert_eq!(classify("cc.exe", None, &claude, &codex), Some(AgentKind::Claude));
        assert_eq!(classify("codex-w", None, &claude, &codex), Some(AgentKind::Codex));
        assert_eq!(classify("node", Some("node /opt/bin/codex --model x"), &claude, &codex), Some(AgentKind::Codex));
        assert_eq!(classify("node", Some("node server.js"), &claude, &codex), None);
        assert_eq!(classify("vim", None, &[], &[]), None);
        assert!(is_shell("zsh") && is_shell("login") && !is_shell("claude"));
    }
}
