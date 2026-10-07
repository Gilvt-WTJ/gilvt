//! The mermaid service's decisions, without gpui or WebKit: which requests share a render,
//! the order diagrams reach the engine and which no one waits for any more, what a timeout
//! costs, what a failure means, which diagrams not to retry, when the engine has been idle long
//! enough to drop, and the pixel format gpui draws.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gilvt_mermaid::IMAGE_LOAD_ERROR;

/// How long the engine lives after its last render finished.
pub const IDLE: Duration = Duration::from_secs(60);
/// Longest a diagram may take once the engine is running it.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// Longest creating the engine and loading its page may take; not counted against a diagram.
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

/// Why a diagram has no picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// mermaid rejected the source; its message.
    Syntax(String),
    /// An image node points at an http(s) URL, which the page may not load.
    RemoteImage,
    /// The engine could not be created, died or timed out.
    Engine,
}

/// SVG markup, or why there is none.
pub type Outcome = Result<Arc<str>, Failure>;

/// Requests by cache key: waiters of renders in flight, and the outcomes not worth retrying
/// this session (mermaid is deterministic; engine failures wait for an explicit reload).
pub struct Requests<W> {
    waiting: HashMap<String, Vec<W>>,
    failed: HashMap<String, Failure>,
}

impl<W> Default for Requests<W> {
    fn default() -> Self {
        Requests { waiting: HashMap::new(), failed: HashMap::new() }
    }
}

impl<W> Requests<W> {
    /// How `key` failed before, if it did.
    pub fn failure(&self, key: &str) -> Option<Failure> {
        self.failed.get(key).cloned()
    }

    /// Queues `waiter` for `key`. True when no render of `key` is in flight, so the caller
    /// starts one; later waiters share it.
    pub fn wait(&mut self, key: &str, waiter: W) -> bool {
        match self.waiting.get_mut(key) {
            Some(waiters) => {
                waiters.push(waiter);
                false
            }
            None => {
                self.waiting.insert(key.to_string(), vec![waiter]);
                true
            }
        }
    }

    /// Ends the render of `key`, returning its waiters. Failures are remembered.
    pub fn complete(&mut self, key: &str, outcome: &Outcome) -> Vec<W> {
        if let Err(failure) = outcome {
            self.failed.insert(key.to_string(), failure.clone());
        }
        self.waiting.remove(key).unwrap_or_default()
    }

    /// Drops the waiters of `key` that went away. True while some remain; otherwise forgets
    /// `key`, so its next request starts a new render.
    pub fn still_wanted(&mut self, key: &str, alive: impl Fn(&W) -> bool) -> bool {
        let Some(waiters) = self.waiting.get_mut(key) else { return false };
        waiters.retain(alive);
        if waiters.is_empty() {
            self.waiting.remove(key);
            return false;
        }
        true
    }

    /// Forgets every failure (`R`: the user asks to try again).
    pub fn clear_failures(&mut self) {
        self.failed.clear();
    }
}

/// Diagrams for the engine, which is handed one at a time, so a diagram's timeout only counts
/// the time the engine spends on it.
pub struct Queue<J> {
    waiting: VecDeque<J>,
    running: bool,
}

impl<J> Default for Queue<J> {
    fn default() -> Self {
        Queue { waiting: VecDeque::new(), running: false }
    }
}

impl<J> Queue<J> {
    pub fn push(&mut self, job: J) {
        self.waiting.push_back(job);
    }

    /// The job to start now: the oldest waiting one still `wanted`, if none is running. The
    /// unwanted ones before it are dropped.
    pub fn next(&mut self, mut wanted: impl FnMut(&J) -> bool) -> Option<J> {
        if self.running {
            return None;
        }
        while let Some(job) = self.waiting.pop_front() {
            if wanted(&job) {
                self.running = true;
                return Some(job);
            }
        }
        None
    }

    /// The running job ended.
    pub fn finish(&mut self) {
        self.running = false;
    }

    /// Every waiting job (the engine could not start, so none of them can run).
    pub fn drain(&mut self) -> Vec<J> {
        self.waiting.drain(..).collect()
    }
}

/// What became of the diagram the engine was running.
#[derive(Debug)]
pub enum Answer {
    Svg(String),
    /// An error message: mermaid's, or the page's if it died.
    Rejected(String),
    TimedOut,
}

/// The outcome of the diagram `source`, and whether to keep the engine. A hung or dead page is
/// replaced before the next diagram runs, so only the diagram it failed on counts as failed.
pub fn classify(answer: Answer, engine_alive: bool, source: &str) -> (Outcome, bool) {
    match answer {
        Answer::Svg(svg) => (Ok(svg.into()), true),
        Answer::Rejected(message) if engine_alive && message == IMAGE_LOAD_ERROR && has_remote_image(source) => {
            (Err(Failure::RemoteImage), true)
        }
        Answer::Rejected(message) if engine_alive => (Err(Failure::Syntax(message)), true),
        Answer::Rejected(_) | Answer::TimedOut => (Err(Failure::Engine), false),
    }
}

/// Whether an image node (`img: "…"`) in `source` points at an http(s) URL.
fn has_remote_image(source: &str) -> bool {
    source.match_indices("img").any(|(at, _)| {
        let Some(value) = source[at + 3..].trim_start().strip_prefix(':') else { return false };
        let url = value.trim_start().trim_start_matches(['"', '\'']).as_bytes();
        let starts = |scheme: &str| url.get(..scheme.len()).is_some_and(|s| s.eq_ignore_ascii_case(scheme.as_bytes()));
        starts("http://") || starts("https://")
    })
}

/// Engine idleness: renders running, and when the last one finished.
#[derive(Default)]
pub struct IdleClock {
    running: usize,
    last: Option<Instant>,
}

impl IdleClock {
    pub fn begin(&mut self) {
        self.running += 1;
    }

    pub fn end(&mut self, now: Instant) {
        self.running = self.running.saturating_sub(1);
        self.last = Some(now);
    }

    /// No render running (so an idle countdown should start).
    pub fn is_idle(&self) -> bool {
        self.running == 0
    }

    /// Whether the engine has been idle for `IDLE` at `now`.
    pub fn expired(&self, now: Instant) -> bool {
        self.is_idle() && self.last.is_some_and(|last| now.saturating_duration_since(last) >= IDLE)
    }
}

/// resvg's premultiplied RGBA → the straight-alpha BGRA gpui's image sprites expect (its
/// blend is `src * alpha + dst * (1 - alpha)`).
pub fn to_straight_bgra(pixels: &mut [u8]) {
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
        let alpha = u16::from(px[3]);
        if alpha != 0 && alpha != 255 {
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_key_shares_one_render() {
        let mut r = Requests::default();
        assert!(r.wait("k", 1));
        assert!(!r.wait("k", 2));
        assert!(r.wait("other", 3));
        assert_eq!(r.complete("k", &Ok("<svg/>".into())), [1, 2]);
        assert_eq!(r.failure("k"), None, "successes are the SVG cache's business");
        assert!(r.wait("k", 4), "a finished render is not joined");
        assert_eq!(r.complete("missing", &Ok("<svg/>".into())), Vec::<i32>::new());
    }

    #[test]
    fn failures_stick_until_cleared() {
        let mut r = Requests::default();
        r.wait("bad", ());
        r.wait("dead", ());
        r.complete("bad", &Err(Failure::Syntax("Parse error".into())));
        r.complete("dead", &Err(Failure::Engine));
        assert_eq!(r.failure("bad"), Some(Failure::Syntax("Parse error".into())));
        assert_eq!(r.failure("dead"), Some(Failure::Engine));
        r.clear_failures();
        assert_eq!(r.failure("bad"), None);
        assert_eq!(r.failure("dead"), None);
    }

    #[test]
    fn engine_gets_one_diagram_at_a_time() {
        let mut q = Queue::default();
        assert_eq!(q.next(|_| true), None::<u8>);
        q.push(1);
        q.push(2);
        q.push(3);
        assert_eq!(q.next(|_| true), Some(1));
        assert_eq!(q.next(|_| true), None, "1 is still running");
        q.finish();
        assert_eq!(q.next(|_| true), Some(2));
        q.finish();
        assert_eq!(q.next(|_| true), Some(3));
        q.finish();
        assert_eq!(q.next(|_| true), None);
    }

    #[test]
    fn a_timeout_fails_only_the_running_diagram() {
        let mut q = Queue::default();
        let mut r = Requests::default();
        for key in ["slow", "a", "b"] {
            assert!(r.wait(key, key));
            q.push(key);
        }
        let running = q.next(|_| true).unwrap();
        let (outcome, keep) = classify(Answer::TimedOut, true, "slow");
        assert!(!keep, "a hung page is replaced");
        assert_eq!(r.complete(running, &outcome), ["slow"]);
        q.finish();
        // The rest still wait for an engine instead of failing with the dropped one.
        assert_eq!(q.next(|_| true), Some("a"));
        assert_eq!(r.failure("slow"), Some(Failure::Engine));
        assert_eq!(r.failure("a"), None);
        assert_eq!(r.failure("b"), None);
    }

    #[test]
    fn answers_decide_about_the_engine() {
        let (outcome, keep) = classify(Answer::Svg("<svg/>".into()), true, "pie");
        assert_eq!((outcome, keep), (Ok("<svg/>".into()), true));
        let (outcome, keep) = classify(Answer::Rejected("Parse error".into()), true, "pie");
        assert_eq!((outcome, keep), (Err(Failure::Syntax("Parse error".into())), true));
        // The same answer from a page that died is the engine's fault, not the diagram's.
        let (outcome, keep) = classify(Answer::Rejected("web process terminated".into()), false, "pie");
        assert_eq!((outcome, keep), (Err(Failure::Engine), false));
    }

    #[test]
    fn a_remote_image_is_told_apart() {
        let remote = "flowchart LR\n  A@{ img: \"https://example.com/a.png\", h: 60 } --> B";
        let failed = |source: &str| classify(Answer::Rejected(IMAGE_LOAD_ERROR.into()), true, source);
        assert_eq!(failed(remote), (Err(Failure::RemoteImage), true));
        assert_eq!(failed("flowchart LR\n  A@{ img:'HTTP://example.com/a.png' }").0, Err(Failure::RemoteImage));
        // Local pictures fail the same way, but not for being remote.
        let locals = ["A@{ img: \"a.png\" }", "A@{ img: \"data:image/png;base64,AAAA\" }", "A[\"http://example.com\"]"];
        for local in locals {
            assert_eq!(failed(local).0, Err(Failure::Syntax(IMAGE_LOAD_ERROR.into())), "{local}");
        }
        // A syntax error comes first: mermaid parses before it loads pictures.
        let parse = classify(Answer::Rejected("Parse error on line 2".into()), true, remote);
        assert_eq!(parse.0, Err(Failure::Syntax("Parse error on line 2".into())));
        assert_eq!(classify(Answer::Rejected(IMAGE_LOAD_ERROR.into()), false, remote), (Err(Failure::Engine), false));
    }

    #[test]
    fn diagrams_no_one_waits_for_are_skipped() {
        let mut q = Queue::default();
        let mut r = Requests::default();
        // Waiters are (view, alive). View 1 closed; view 2 switched to the dark theme, so it
        // gave up its light key and waits for the dark one.
        let asked = [("closed", (1, false)), ("light", (2, false)), ("shared", (1, false)), ("dark", (2, true))];
        for (key, waiter) in asked {
            if r.wait(key, waiter) {
                q.push(key);
            }
        }
        assert!(!r.wait("shared", (3, true)), "joins the queued render");
        let alive = |w: &(i32, bool)| w.1;
        assert_eq!(q.next(|key| r.still_wanted(key, alive)), Some("shared"));
        assert_eq!(r.complete("shared", &Ok("<svg/>".into())), [(3, true)], "the closed view's waiter is gone");
        q.finish();
        assert_eq!(q.next(|key| r.still_wanted(key, alive)), Some("dark"));
        q.finish();
        assert_eq!(q.next(|key| r.still_wanted(key, alive)), None);
        // Skipped keys are forgotten, not failed: asking again starts a render.
        assert_eq!(r.failure("light"), None);
        assert!(r.wait("light", (4, true)));
        assert!(!r.still_wanted("missing", alive));
    }

    #[test]
    fn an_engine_that_cannot_start_fails_everyone_waiting() {
        let mut q = Queue::default();
        q.push(1);
        q.push(2);
        q.push(3);
        assert_eq!(q.next(|_| true), Some(1));
        assert_eq!(q.drain(), [2, 3]);
        q.finish();
        assert_eq!(q.next(|_| true), None);
    }

    #[test]
    fn idle_counts_from_the_last_render() {
        let t0 = Instant::now();
        let mut clock = IdleClock::default();
        assert!(!clock.expired(t0 + IDLE * 2), "never used: nothing to drop");
        clock.begin();
        clock.begin();
        clock.end(t0);
        assert!(!clock.is_idle());
        assert!(!clock.expired(t0 + IDLE * 2), "a render is still running");
        clock.end(t0 + Duration::from_secs(5));
        assert!(clock.is_idle());
        assert!(!clock.expired(t0 + IDLE));
        assert!(clock.expired(t0 + Duration::from_secs(5) + IDLE));
    }

    #[test]
    fn pixels_become_straight_bgra() {
        let mut px = [
            10, 20, 30, 255, // opaque: only swapped
            0, 0, 0, 0, // transparent: untouched
            64, 32, 0, 128, // half alpha: divided back out
            200, 200, 200, 100, // rounding can overshoot 255
        ];
        to_straight_bgra(&mut px);
        assert_eq!(px, [30, 20, 10, 255, 0, 0, 0, 0, 0, 64, 128, 128, 255, 255, 255, 100]);
    }
}
