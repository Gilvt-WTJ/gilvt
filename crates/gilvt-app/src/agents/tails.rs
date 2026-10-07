//! Transcript tailing on one background thread: a `notify` watcher over the directories of the
//! watched transcripts (and Claude subagent transcripts) wakes it, and it also polls every [`POLL`].
//! New lines are parsed there and sent to the main thread as event batches; the same thread keeps the
//! session timelines (hooks are forwarded to it) and sends their snapshots (see [`super::worker`]).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::{Duration, SystemTime};

use gilvt_agent::{parse_transcript_line, Event, SessionKey};
use notify::{RecursiveMode, Watcher};

use super::timelines::{HookFeed, TimelineView};
use super::worker::Worker;

pub const POLL: Duration = Duration::from_secs(2);

/// Events from one transcript.
#[derive(Clone, Debug, PartialEq)]
pub enum Batch {
    /// What the file held when watching began: history, reduced to the latest usage and model, plus the
    /// session's first real prompt (a resumed session sends no prompt hook for it, so it names the session).
    Catchup { events: Vec<Event>, first_prompt: Option<String> },
    /// Lines written since.
    Live(Vec<Event>),
}

/// What the tail thread sends to the main thread.
#[derive(Clone, Debug)]
pub enum Out {
    /// Registry events of one transcript.
    Events(SessionKey, Batch),
    /// A session timeline changed.
    Timeline(SessionKey, Arc<TimelineView>),
}

enum Msg {
    Watch(Vec<(SessionKey, PathBuf)>),
    Hook(SessionKey, HookFeed),
    Answered(SessionKey, SystemTime),
    Changed,
}

/// Handle to the tail thread; the thread exits when this is dropped.
pub struct Tails {
    tx: mpsc::Sender<Msg>,
}

impl Tails {
    pub fn spawn(out: async_channel::Sender<Out>) -> Tails {
        let (tx, rx) = mpsc::channel();
        let wake = tx.clone();
        std::thread::Builder::new()
            .name("gilvt-transcripts".into())
            .spawn(move || run(rx, wake, out))
            .expect("spawn transcript thread");
        Tails { tx }
    }

    /// Replaces the set of watched transcripts.
    pub fn watch(&self, set: Vec<(SessionKey, PathBuf)>) {
        let _ = self.tx.send(Msg::Watch(set));
    }

    /// The user answered an approval dialog of session `key` at `at` (see `Timeline::approval_answered`).
    pub fn approval_answered(&self, key: SessionKey, at: SystemTime) {
        let _ = self.tx.send(Msg::Answered(key, at));
    }

    /// Feeds a hook request to the timeline of session `key`.
    pub fn hook(&self, key: SessionKey, feed: HookFeed) {
        let _ = self.tx.send(Msg::Hook(key, feed));
    }
}

/// History keeps only what a current reading needs: the last usage and the last model, and the first real
/// prompt (the parsers emit `PromptSubmit` for real prompts only: no `!` commands, slash-command echoes, task
/// notifications or interrupts).
pub fn catchup(events: Vec<Event>) -> Batch {
    let usage = events.iter().rposition(|e| matches!(e, Event::Usage { .. }));
    let model = events.iter().rposition(|e| matches!(e, Event::Model { .. }));
    let mut keep: Vec<usize> = usage.into_iter().chain(model).collect();
    keep.sort_unstable();
    let first_prompt = events.iter().find_map(|e| match e {
        Event::PromptSubmit { text } if !text.trim().is_empty() => Some(text.clone()),
        _ => None,
    });
    Batch::Catchup { events: keep.into_iter().map(|i| events[i].clone()).collect(), first_prompt }
}

impl Batch {
    /// Nothing for the registry.
    pub fn is_empty(&self) -> bool {
        match self {
            Batch::Catchup { events, first_prompt } => events.is_empty() && first_prompt.is_none(),
            Batch::Live(events) => events.is_empty(),
        }
    }
}

pub fn parse(key: &SessionKey, lines: &[String]) -> Vec<Event> {
    lines.iter().flat_map(|l| parse_transcript_line(key.0, l)).collect()
}

fn run(rx: mpsc::Receiver<Msg>, wake: mpsc::Sender<Msg>, out: async_channel::Sender<Out>) {
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = wake.send(Msg::Changed);
        }
    })
    .map_err(|e| eprintln!("gilvt: transcript watcher unavailable, polling only: {e}"))
    .ok();
    let mut worker = Worker::default();
    let mut dirs: HashSet<PathBuf> = HashSet::new();
    loop {
        let mut msgs = match rx.recv_timeout(POLL) {
            Ok(m) => vec![m],
            Err(mpsc::RecvTimeoutError::Timeout) => Vec::new(),
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        msgs.extend(rx.try_iter());
        for msg in msgs {
            match msg {
                Msg::Watch(set) => worker.watch(set),
                Msg::Hook(key, feed) => worker.hook(key, feed),
                Msg::Answered(key, at) => worker.approval_answered(&key, at),
                Msg::Changed => {}
            }
        }
        let mut open = true;
        worker.read(&mut |key, batch| open &= out.send_blocking(Out::Events(key, batch)).is_ok());
        for (key, view) in worker.publish() {
            open &= out.send_blocking(Out::Timeline(key, view)).is_ok();
        }
        if !open {
            return;
        }
        let now = worker.dirs();
        if let Some(w) = watcher.as_mut() {
            for d in dirs.difference(&now) {
                let _ = w.unwatch(d);
            }
            for d in now.difference(&dirs) {
                // A directory that does not exist yet is covered by the poll.
                let _ = w.watch(d, RecursiveMode::NonRecursive);
            }
        }
        dirs = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_agent::AgentKind;
    use std::io::Write;

    fn next_events(rx: &async_channel::Receiver<Out>) -> (SessionKey, Batch) {
        loop {
            if let Out::Events(key, batch) = rx.recv_blocking().unwrap() {
                return (key, batch);
            }
        }
    }

    fn usage(n: u64) -> Event {
        Event::Usage { context_tokens: n, context_window: None, message_id: None }
    }

    #[test]
    fn history_keeps_the_latest_usage_and_model() {
        let model = |n: &str| Event::Model { name: n.into() };
        let prompt = |t: &str| Event::PromptSubmit { text: t.into() };
        let history = vec![
            model("a"),
            prompt("  "),
            prompt("first"),
            usage(1),
            Event::TurnEnd { background_tasks: 0 },
            prompt("second"),
            usage(2),
            model("b"),
            Event::Interrupted,
        ];
        assert_eq!(catchup(history), Batch::Catchup { events: vec![usage(2), model("b")], first_prompt: Some("first".into()) });
        assert!(catchup(vec![Event::SessionEnd]).is_empty());
    }

    #[test]
    fn tails_new_lines_after_catching_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let line = |n: u64| {
            format!(r#"{{"timestamp":"2026-01-01T00:00:00Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"total_tokens":{n}}},"model_context_window":1000}}}}}}"#)
        };
        std::fs::write(&path, format!("{}\n{}\n", line(10), line(20))).unwrap();
        let (tx, rx) = async_channel::unbounded();
        let tails = Tails::spawn(tx);
        let key: SessionKey = (AgentKind::Codex, "s".into());
        tails.watch(vec![(key.clone(), path.clone())]);
        let (k, batch) = next_events(&rx);
        assert_eq!(k, key);
        let Batch::Catchup { events, first_prompt: None } = batch else { panic!("{batch:?}") };
        assert!(matches!(events.as_slice(), [Event::Usage { context_tokens: 20, .. }]), "{events:?}");
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{}", line(30)).unwrap();
        let (_, batch) = next_events(&rx);
        let Batch::Live(events) = batch else { panic!("{batch:?}") };
        assert!(matches!(events.as_slice(), [Event::Usage { context_tokens: 30, .. }]), "{events:?}");
    }
}
