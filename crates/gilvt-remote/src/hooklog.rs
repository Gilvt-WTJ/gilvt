//! Which hooks reach the app (spec R2a §4.1) and the in-memory replay buffer for reconnecting bridges (§4.3).
//! Pure: the daemon does the IO.

use std::collections::VecDeque;

use gilvt_ipc::remote::RemoteEvent;

use crate::links::Links;

/// Hook events kept for replay.
pub const HOOK_LOG_MAX: usize = 2000;

/// The link a hook belongs to, or None to drop it: no link, an unknown link, or tmux (R2b handles tmux).
pub fn attribute(links: &Links, link: Option<&str>, tmux: bool) -> Option<String> {
    let link = link?;
    (links.contains(link) && !tmux).then(|| link.to_string())
}

pub struct HookLog {
    cap: usize,
    events: VecDeque<(u64, RemoteEvent)>,
    /// The highest seq dropped to stay within `cap` (0: none).
    dropped_through: u64,
}

#[derive(Debug, PartialEq)]
pub struct Replay {
    pub events: Vec<(u64, RemoteEvent)>,
    /// Hooks the app had not seen are gone (another daemon, or older than the buffer).
    pub gap: bool,
}

impl HookLog {
    pub fn new(cap: usize) -> HookLog {
        HookLog { cap, events: VecDeque::new(), dropped_through: 0 }
    }

    pub fn push(&mut self, seq: u64, event: RemoteEvent) {
        self.events.push_back((seq, event));
        while self.events.len() > self.cap {
            if let Some((s, _)) = self.events.pop_front() {
                self.dropped_through = s;
            }
        }
    }

    /// What a bridge saying `Hello { cursor, epoch: hello_epoch }` gets after its Welcome.
    pub fn replay(&self, daemon_epoch: u64, hello_epoch: Option<u64>, cursor: u64) -> Replay {
        let all = || self.events.iter().cloned().collect::<Vec<_>>();
        match hello_epoch {
            // The app's first connection this run: it has nothing to lose.
            None => Replay { events: all(), gap: false },
            Some(e) if e != daemon_epoch => Replay { events: all(), gap: true },
            Some(_) if cursor < self.dropped_through => Replay { events: all(), gap: true },
            Some(_) => Replay { events: self.events.iter().filter(|(s, _)| *s > cursor).cloned().collect(), gap: false },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_ipc::remote::LinkState;

    fn hook(n: u64) -> RemoteEvent {
        RemoteEvent::Hook { link: "i-1".into(), agent: "claude".into(), event: format!("E{n}"), payload: serde_json::json!({}) }
    }
    fn seqs(r: &Replay) -> Vec<u64> {
        r.events.iter().map(|(s, _)| *s).collect()
    }

    #[test]
    fn attribution_rules() {
        let mut links = Links::default();
        links.up(LinkState { link: "i-1".into(), hostname: "h".into(), tty: None, pid: 1 });
        assert_eq!(attribute(&links, None, false), None, "no link");
        assert_eq!(attribute(&links, Some("i-9"), false), None, "link not registered");
        assert_eq!(attribute(&links, Some("i-1"), true), None, "tmux: R2b");
        assert_eq!(attribute(&links, Some("i-1"), false), Some("i-1".into()));
    }

    #[test]
    fn same_epoch_replays_what_the_app_missed() {
        let mut log = HookLog::new(10);
        for s in 1..=5 { log.push(s, hook(s)); }
        let r = log.replay(77, Some(77), 3);
        assert_eq!((seqs(&r), r.gap), (vec![4, 5], false));
        assert!(log.replay(77, Some(77), 5).events.is_empty());
    }

    #[test]
    fn another_epoch_replays_everything_with_a_gap() {
        let mut log = HookLog::new(10);
        for s in 1..=3 { log.push(s, hook(s)); }
        let r = log.replay(77, Some(76), 9);
        assert_eq!((seqs(&r), r.gap), (vec![1, 2, 3], true));
    }

    #[test]
    fn a_first_connection_replays_everything_without_a_gap() {
        let mut log = HookLog::new(10);
        for s in 1..=3 { log.push(s, hook(s)); }
        let r = log.replay(77, None, 0);
        assert_eq!((seqs(&r), r.gap), (vec![1, 2, 3], false));
    }

    #[test]
    fn a_cursor_older_than_the_buffer_is_a_gap_and_the_cap_holds() {
        let mut log = HookLog::new(3);
        for s in 1..=5 { log.push(s, hook(s)); }
        assert_eq!(seqs(&log.replay(77, Some(77), 0)), vec![3, 4, 5], "only the newest 3 kept");
        let r = log.replay(77, Some(77), 1);
        assert_eq!((seqs(&r), r.gap), (vec![3, 4, 5], true), "seq 2 was dropped and the app never saw it");
        let r = log.replay(77, Some(77), 2);
        assert_eq!((seqs(&r), r.gap), (vec![3, 4, 5], false), "the app saw 2: nothing lost");
    }

    #[test]
    fn the_default_cap_is_2000() {
        let mut log = HookLog::new(HOOK_LOG_MAX);
        for s in 1..=2500 { log.push(s, hook(s)); }
        let r = log.replay(1, Some(1), 0);
        assert_eq!((r.events.len(), r.events[0].0, r.gap), (2000, 501, true));
    }
}
