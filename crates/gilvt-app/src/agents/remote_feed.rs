//! The app side of remote transcript following (spec R2a §4.4, §5.5): which remote sessions are followed, how many
//! bytes arrived, and which of them are history (the bytes before `Following { size }`) for the registry's
//! catch-up. Pure; `Agents` sends the requests and feeds the tail thread.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gilvt_agent::host::Host;
use gilvt_agent::SessionKey;

/// An ended remote session is unfollowed this long after it ended.
pub const UNFOLLOW_AFTER: Duration = Duration::from_secs(60);
pub const FOLLOW_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FollowCmd {
    /// `restart`: the tail thread must drop what it had (another transcript for the session).
    Follow { host: String, key: SessionKey, path: PathBuf, from: u64, restart: bool },
    Unfollow { host: String, key: SessionKey },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appended {
    pub lines: Vec<String>,
    /// Before the size at follow time: catch-up for the registry.
    pub history: bool,
    /// This append completed the history.
    pub history_done: bool,
}

struct Follow {
    host: String,
    path: PathBuf,
    received: u64,
    history_until: Option<u64>,
    history_done: bool,
    partial: Vec<u8>,
    ended: Option<Instant>,
    active: bool,
    /// A `TranscriptFollow` was sent and its `Following` has not come: pushes before it are the old follower's.
    awaiting: Cell<bool>,
}

impl Follow {
    fn new(host: &str, path: &PathBuf) -> Follow {
        Follow { host: host.into(), path: path.clone(), received: 0, history_until: None, history_done: false, partial: Vec::new(), ended: None, active: true, awaiting: Cell::new(true) }
    }
}

#[derive(Default)]
pub struct RemoteFollows {
    map: HashMap<SessionKey, Follow>,
}

fn host_of(key: &SessionKey) -> Option<&str> {
    match &key.2 {
        Host::Remote(h) => Some(&h.0),
        Host::Local => None,
    }
}

impl RemoteFollows {
    /// The live sessions that have a transcript (`Core::watch_set`); local ones are ignored.
    pub fn sync(&mut self, live: &[(SessionKey, PathBuf)], now: Instant) -> Vec<FollowCmd> {
        let mut cmds = Vec::new();
        for (key, path) in live {
            let Some(host) = host_of(key) else { continue };
            match self.map.get_mut(key) {
                Some(f) if &f.path != path => {
                    *f = Follow::new(host, path);
                    cmds.push(FollowCmd::Follow { host: host.into(), key: key.clone(), path: path.clone(), from: 0, restart: true });
                }
                Some(f) => {
                    f.ended = None;
                    if !f.active {
                        f.active = true;
                        f.awaiting.set(true);
                        cmds.push(FollowCmd::Follow { host: host.into(), key: key.clone(), path: path.clone(), from: f.received, restart: false });
                    }
                }
                None => {
                    self.map.insert(key.clone(), Follow::new(host, path));
                    cmds.push(FollowCmd::Follow { host: host.into(), key: key.clone(), path: path.clone(), from: 0, restart: false });
                }
            }
        }
        for (key, f) in self.map.iter_mut() {
            if f.active && f.ended.is_none() && !live.iter().any(|(k, _)| k == key) {
                f.ended = Some(now);
            }
        }
        cmds
    }

    pub fn tick(&mut self, now: Instant) -> Vec<FollowCmd> {
        let mut cmds = Vec::new();
        for (key, f) in self.map.iter_mut() {
            if f.active && f.ended.is_some_and(|t| now.saturating_duration_since(t) >= UNFOLLOW_AFTER) {
                f.active = false;
                cmds.push(FollowCmd::Unfollow { host: f.host.clone(), key: key.clone() });
            }
        }
        cmds
    }

    /// A new bridge to `host`: follow again from what arrived (R2a §5.5).
    pub fn reconnected(&self, host: &str) -> Vec<FollowCmd> {
        self.map
            .iter()
            .filter(|(_, f)| f.active && f.host == host)
            .inspect(|(_, f)| f.awaiting.set(true))
            .map(|(key, f)| FollowCmd::Follow { host: f.host.clone(), key: key.clone(), path: f.path.clone(), from: f.received, restart: false })
            .collect()
    }

    /// `Following { size }`: the bytes before `size` are history. True when that history is already complete.
    pub fn following(&mut self, key: &SessionKey, size: u64) -> bool {
        let Some(f) = self.map.get_mut(key) else { return false };
        f.awaiting.set(false);
        if f.history_done {
            return false;
        }
        f.history_until = Some(size);
        f.history_done = f.received >= size;
        f.history_done
    }

    pub fn append(&mut self, key: &SessionKey, offset: u64, bytes: &[u8]) -> Option<Appended> {
        let f = self.map.get_mut(key)?;
        if f.awaiting.get() || offset < f.received {
            return None;
        }
        if offset > f.received {
            f.partial.clear();
        }
        let history = !f.history_done;
        f.received = offset + bytes.len() as u64;
        f.partial.extend_from_slice(bytes);
        let cut = f.partial.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        let complete: Vec<u8> = f.partial.drain(..cut).collect();
        let lines = complete.split(|&b| b == b'\n').filter(|l| !l.is_empty()).map(|l| String::from_utf8_lossy(l).into_owned()).collect();
        let history_done = history && f.history_until.is_some_and(|end| f.received >= end);
        if history_done {
            f.history_done = true;
        }
        Some(Appended { lines, history, history_done })
    }

    /// `TranscriptReset { size }`: the file shrank or was replaced and the daemon pushes it again from 0. Its `size`
    /// bytes are history again (catch-up for the registry: replaying them as live events would rewrite the
    /// session's status); what is appended after them is live. None: not followed. Some(true): `size` is 0.
    pub fn reset(&mut self, key: &SessionKey, size: u64) -> Option<bool> {
        let f = self.map.get_mut(key)?;
        if f.awaiting.get() {
            return None;
        }
        f.received = 0;
        f.partial.clear();
        f.history_until = Some(size);
        f.history_done = size == 0;
        Some(f.history_done)
    }

    /// The follow request failed (timeout, error): stop dropping pushes, there may still be some.
    pub fn follow_failed(&mut self, key: &SessionKey) {
        if let Some(f) = self.map.get_mut(key) {
            f.awaiting.set(false);
        }
    }

    /// Followed and the history has not all arrived: the inspector shows its loading state.
    pub fn loading(&self, key: &SessionKey) -> bool {
        self.map.get(key).is_some_and(|f| f.active && !f.history_done)
    }

    #[allow(dead_code)] // DebugState `hosts[].follows` (R2a Task 13)
    pub fn count(&self, host: &str) -> usize {
        self.map.values().filter(|f| f.active && f.host == host).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_agent::host::{Host, HostId};
    use gilvt_agent::AgentKind;

    fn key(id: &str) -> SessionKey {
        (AgentKind::Claude, id.into(), Host::Remote(HostId("dev@h:22".into())))
    }
    fn follow(host: &str, k: &SessionKey, from: u64) -> FollowCmd {
        FollowCmd::Follow { host: host.into(), key: k.clone(), path: "/home/dev/s.jsonl".into(), from, restart: false }
    }

    #[test]
    fn history_then_live() {
        let (t0, k) = (Instant::now(), key("s"));
        let mut f = RemoteFollows::default();
        assert_eq!(f.sync(&[(k.clone(), "/home/dev/s.jsonl".into())], t0), vec![follow("dev@h:22", &k, 0)]);
        assert!(f.loading(&k));
        assert!(!f.following(&k, 8));
        let a = f.append(&k, 0, b"{\"a\":1}\n").unwrap();
        assert_eq!((a.lines, a.history, a.history_done), (vec!["{\"a\":1}".to_string()], true, true));
        assert!(!f.loading(&k));
        let b = f.append(&k, 8, b"{\"b\":2}\n{\"c\":3}\n").unwrap();
        assert_eq!((b.lines.len(), b.history, b.history_done), (2, false, false));
    }

    #[test]
    fn an_empty_transcript_is_done_at_once_and_duplicates_are_dropped() {
        let (t0, k) = (Instant::now(), key("s"));
        let mut f = RemoteFollows::default();
        f.sync(&[(k.clone(), "/home/dev/s.jsonl".into())], t0);
        assert!(f.following(&k, 0), "nothing to catch up");
        f.append(&k, 0, b"x\n").unwrap();
        assert!(f.append(&k, 0, b"x\n").is_none(), "an overlap is a duplicate");
        assert!(f.append(&k, 10, b"y\n").is_some(), "a jump is a skipped oversized line");
    }

    #[test]
    fn unfollow_sixty_seconds_after_the_end_and_refollow_on_reconnect() {
        let (t0, k) = (Instant::now(), key("s"));
        let mut f = RemoteFollows::default();
        f.sync(&[(k.clone(), "/home/dev/s.jsonl".into())], t0);
        f.following(&k, 0);
        f.append(&k, 0, b"x\n").unwrap();
        assert_eq!(f.reconnected("dev@h:22"), vec![follow("dev@h:22", &k, 2)], "from = bytes received");
        assert!(f.sync(&[], t0).is_empty(), "ended: nothing yet");
        assert!(f.tick(t0 + Duration::from_secs(59)).is_empty());
        assert_eq!(f.tick(t0 + UNFOLLOW_AFTER), vec![FollowCmd::Unfollow { host: "dev@h:22".into(), key: k.clone() }]);
        assert_eq!(f.count("dev@h:22"), 0);
        assert!(f.reconnected("dev@h:22").is_empty());
        assert_eq!(f.sync(&[(k.clone(), "/home/dev/s.jsonl".into())], t0), vec![follow("dev@h:22", &k, 2)], "live again: continue");
    }

    #[test]
    fn pushes_before_the_following_reply_are_the_old_followers() {
        let (t0, k) = (Instant::now(), key("s"));
        let mut f = RemoteFollows::default();
        f.sync(&[(k.clone(), "/home/dev/s.jsonl".into())], t0);
        assert!(f.append(&k, 0, b"x\n").is_none(), "no Following yet");
        assert_eq!(f.reset(&k, 0), None);
        f.following(&k, 0);
        f.append(&k, 0, b"x\n").unwrap();
        f.reconnected("dev@h:22");
        assert!(f.append(&k, 2, b"y\n").is_none(), "awaiting the new bridge's Following");
        f.follow_failed(&k);
        assert!(f.append(&k, 2, b"y\n").is_some());
    }

    #[test]
    fn local_keys_are_ignored() {
        let (t0, k) = (Instant::now(), key("s"));
        let mut f = RemoteFollows::default();
        f.sync(&[(k.clone(), "/home/dev/s.jsonl".into()), (gilvt_agent::local_key(AgentKind::Codex, "l"), "/l.jsonl".into())], t0);
        assert_eq!(f.count("dev@h:22"), 1);
    }

    #[test]
    fn after_a_reset_the_bytes_below_its_size_are_history_again_and_later_ones_are_live() {
        let (t0, k) = (Instant::now(), key("s"));
        let mut f = RemoteFollows::default();
        f.sync(&[(k.clone(), "/home/dev/s.jsonl".into())], t0);
        f.following(&k, 8);
        assert!(f.append(&k, 0, b"{\"a\":1}\n").unwrap().history_done);
        // The file was replaced by one of 4 bytes: the daemon pushes it from 0 again.
        assert_eq!(f.reset(&k, 4), Some(false));
        assert!(f.loading(&k), "the history is on its way again");
        let a = f.append(&k, 0, b"x\ny\n").unwrap();
        assert_eq!((a.lines.len(), a.history, a.history_done), (2, true, true), "below the reset's size: catch-up, not live events");
        let b = f.append(&k, 4, b"z\n").unwrap();
        assert_eq!((b.history, b.history_done), (false, false), "after it: live");
        // An empty replacement has nothing to catch up on.
        assert_eq!(f.reset(&k, 0), Some(true));
        assert!(!f.loading(&k));
        assert!(!f.append(&k, 0, b"w\n").unwrap().history);
        assert_eq!(f.reset(&key("other"), 0), None, "not followed");
    }
}
