//! Which hooks reach the app (spec R2a §4.1) and the in-memory replay buffer for reconnecting bridges (§4.3).
//! Pure: the daemon does the IO.

use std::collections::VecDeque;
use std::sync::Arc;

use gilvt_ipc::remote::RemoteEvent;

use crate::links::Links;

/// Hook events kept for replay.
pub const HOOK_LOG_MAX: usize = 2000;
/// And at most this many payload bytes (2000 hooks of up to 1 MB each would be 2 GB).
pub const HOOK_LOG_BYTES: usize = 32 << 20;

/// The link a hook belongs to, or None to drop it: no link, an unknown link, or tmux (R2b handles tmux).
pub fn attribute(links: &Links, link: Option<&str>, tmux: bool) -> Option<String> {
    let link = link?;
    (links.contains(link) && !tmux).then(|| link.to_string())
}

/// What a hook costs the buffer: its payload's JSON length (computed before taking the daemon's lock).
pub fn hook_size(payload: &serde_json::Value) -> usize {
    serde_json::to_vec(payload).map_or(0, |v| v.len())
}

pub struct HookLog {
    cap: usize,
    max_bytes: usize,
    bytes: usize,
    events: VecDeque<(u64, Arc<RemoteEvent>, usize)>,
    /// The highest seq dropped to stay within `cap` / `max_bytes` (0: none).
    dropped_through: u64,
}

#[derive(Debug, PartialEq)]
pub struct Replay {
    /// Shared with the buffer: the daemon builds the frames outside its lock.
    pub events: Vec<(u64, Arc<RemoteEvent>)>,
    /// Hooks the app had not seen are gone (another daemon, or older than the buffer).
    pub gap: bool,
}

impl HookLog {
    pub fn new(cap: usize, max_bytes: usize) -> HookLog {
        HookLog { cap, max_bytes, bytes: 0, events: VecDeque::new(), dropped_through: 0 }
    }

    /// Keeps hook `seq` (`size`: [`hook_size`]), dropping the oldest while over either cap.
    pub fn push(&mut self, seq: u64, event: Arc<RemoteEvent>, size: usize) {
        self.events.push_back((seq, event, size));
        self.bytes += size;
        while self.events.len() > self.cap || self.bytes > self.max_bytes {
            let Some((s, _, n)) = self.events.pop_front() else { break };
            self.dropped_through = s;
            self.bytes -= n;
        }
    }

    /// What a bridge saying `Hello { cursor, epoch: hello_epoch }` gets after its Welcome: only hooks of links still
    /// registered (`live`), since the hooks of an ended link have no pane left to go to. The gap is decided on the
    /// whole buffer, before that filter.
    pub fn replay(&self, daemon_epoch: u64, hello_epoch: Option<u64>, cursor: u64, live: impl Fn(&str) -> bool) -> Replay {
        let keep = |e: &RemoteEvent| match e {
            RemoteEvent::Hook { link, .. } => live(link),
            _ => true,
        };
        let from = |after: u64| self.events.iter().filter(|(s, e, _)| *s > after && keep(e)).map(|(s, e, _)| (*s, e.clone())).collect::<Vec<_>>();
        match hello_epoch {
            // The app's first connection this run: it has nothing to lose.
            None => Replay { events: from(0), gap: false },
            Some(e) if e != daemon_epoch => Replay { events: from(0), gap: true },
            Some(_) if cursor < self.dropped_through => Replay { events: from(0), gap: true },
            Some(_) => Replay { events: from(cursor), gap: false },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_ipc::remote::LinkState;

    fn hook(n: u64) -> Arc<RemoteEvent> {
        hook_on("i-1", n)
    }
    fn hook_on(link: &str, n: u64) -> Arc<RemoteEvent> {
        Arc::new(RemoteEvent::Hook { link: link.into(), agent: "claude".into(), event: format!("E{n}"), payload: serde_json::json!({}) })
    }
    fn all(_: &str) -> bool {
        true
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
        let mut log = HookLog::new(10, HOOK_LOG_BYTES);
        for s in 1..=5 { log.push(s, hook(s), 1); }
        let r = log.replay(77, Some(77), 3, all);
        assert_eq!((seqs(&r), r.gap), (vec![4, 5], false));
        assert!(log.replay(77, Some(77), 5, all).events.is_empty());
    }

    #[test]
    fn another_epoch_replays_everything_with_a_gap() {
        let mut log = HookLog::new(10, HOOK_LOG_BYTES);
        for s in 1..=3 { log.push(s, hook(s), 1); }
        let r = log.replay(77, Some(76), 9, all);
        assert_eq!((seqs(&r), r.gap), (vec![1, 2, 3], true));
    }

    #[test]
    fn a_first_connection_replays_everything_without_a_gap() {
        let mut log = HookLog::new(10, HOOK_LOG_BYTES);
        for s in 1..=3 { log.push(s, hook(s), 1); }
        let r = log.replay(77, None, 0, all);
        assert_eq!((seqs(&r), r.gap), (vec![1, 2, 3], false));
    }

    #[test]
    fn a_cursor_older_than_the_buffer_is_a_gap_and_the_cap_holds() {
        let mut log = HookLog::new(3, HOOK_LOG_BYTES);
        for s in 1..=5 { log.push(s, hook(s), 1); }
        assert_eq!(seqs(&log.replay(77, Some(77), 0, all)), vec![3, 4, 5], "only the newest 3 kept");
        let r = log.replay(77, Some(77), 1, all);
        assert_eq!((seqs(&r), r.gap), (vec![3, 4, 5], true), "seq 2 was dropped and the app never saw it");
        let r = log.replay(77, Some(77), 2, all);
        assert_eq!((seqs(&r), r.gap), (vec![3, 4, 5], false), "the app saw 2: nothing lost");
    }

    #[test]
    fn the_default_cap_is_2000() {
        let mut log = HookLog::new(HOOK_LOG_MAX, HOOK_LOG_BYTES);
        for s in 1..=2500 { log.push(s, hook(s), 1); }
        let r = log.replay(1, Some(1), 0, all);
        assert_eq!((r.events.len(), r.events[0].0, r.gap), (2000, 501, true));
    }

    #[test]
    fn the_byte_cap_drops_the_oldest_and_reports_the_gap() {
        let mut log = HookLog::new(100, 10);
        for s in 1..=4 { log.push(s, hook(s), 3); }
        // 4 × 3 = 12 > 10: seq 1 went.
        assert_eq!(seqs(&log.replay(77, Some(77), 0, all)), vec![2, 3, 4]);
        let r = log.replay(77, Some(77), 0, all);
        assert!(r.gap, "the app never saw seq 1");
        let r = log.replay(77, Some(77), 1, all);
        assert_eq!((seqs(&r), r.gap), (vec![2, 3, 4], false));
        log.push(5, hook(5), 9);
        // 3+3+3+9 → drop 2, 3, 4 until 9 ≤ 10.
        let r = log.replay(77, Some(77), 1, all);
        assert_eq!((seqs(&r), r.gap), (vec![5], true));
        assert_eq!((seqs(&log.replay(77, Some(77), 4, all)), log.replay(77, Some(77), 4, all).gap), (vec![5], false));
        // One hook larger than the whole cap is not kept at all, and counts as dropped.
        log.push(6, hook(6), 11);
        let r = log.replay(77, Some(77), 5, all);
        assert_eq!((seqs(&r), r.gap), (vec![], true));
    }

    #[test]
    fn hooks_of_ended_links_are_not_replayed() {
        let mut log = HookLog::new(10, HOOK_LOG_BYTES);
        log.push(1, hook_on("i-1", 1), 1);
        log.push(2, hook_on("gone", 2), 1);
        log.push(3, Arc::new(RemoteEvent::LinkDown { link: "gone".into() }), 1);
        log.push(4, hook_on("i-1", 4), 1);
        let live = |l: &str| l == "i-1";
        let r = log.replay(77, None, 0, live);
        assert_eq!((seqs(&r), r.gap), (vec![1, 3, 4], false));
        let r = log.replay(77, Some(77), 1, live);
        assert_eq!((seqs(&r), r.gap), (vec![3, 4], false), "filtering is not a gap");
    }
}
