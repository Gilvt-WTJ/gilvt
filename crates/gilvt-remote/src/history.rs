//! The remote session history for the app's sessions palette (spec R2a §4.5): `HistoryIndex` over this host's
//! `~/.claude` and `~/.codex`, cached in `~/.gilvt-server/state/history.json`, one scan at a time on a background
//! thread; requests made during a scan get that scan's result.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gilvt_agent::{HistoryEntry, HistoryIndex};

pub type Scan = Arc<dyn Fn() -> Result<Vec<HistoryEntry>, String> + Send + Sync>;
type Waiter = Box<dyn FnOnce(Result<Arc<Vec<HistoryEntry>>, String>) + Send>;

/// The serialized entries a `HistoryList` may carry (half a frame: `write_frame` refuses more than 16 MB, and a
/// refused frame ends the bridge's writer).
pub const HISTORY_BYTES: usize = 8 * 1024 * 1024;

/// The real scan: loads the cache once, then refreshes it from `home`.
pub fn scanner(state_dir: PathBuf, home: PathBuf) -> Scan {
    let index: Arc<Mutex<Option<HistoryIndex>>> = Arc::default();
    Arc::new(move || {
        let _ = std::fs::create_dir_all(&state_dir);
        let mut slot = index.lock().unwrap_or_else(|e| e.into_inner());
        let idx = slot.get_or_insert_with(|| HistoryIndex::load(Some(&state_dir)));
        Ok(idx.refresh(&home))
    })
}

#[derive(Default)]
struct Inner {
    running: bool,
    waiters: Vec<Waiter>,
}

pub struct HistoryWorker {
    scan: Scan,
    inner: Arc<Mutex<Inner>>,
}

impl HistoryWorker {
    pub fn new(scan: Scan) -> HistoryWorker {
        HistoryWorker { scan, inner: Arc::default() }
    }

    pub fn request(&self, reply: impl FnOnce(Result<Arc<Vec<HistoryEntry>>, String>) + Send + 'static) {
        let mut inner = self.inner.lock().unwrap();
        inner.waiters.push(Box::new(reply));
        if inner.running {
            return;
        }
        inner.running = true;
        let (scan, shared) = (self.scan.clone(), self.inner.clone());
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scan()))
                .unwrap_or_else(|_| Err("the history scan failed".into()))
                .map(Arc::new);
            let waiters = {
                let mut i = shared.lock().unwrap();
                i.running = false;
                std::mem::take(&mut i.waiters)
            };
            for w in waiters {
                w(result.clone());
            }
        });
    }
}

/// The newest entries (by last activity) whose JSON fits in `max` bytes.
pub fn fit_entries(mut entries: Vec<HistoryEntry>, max: usize) -> Vec<HistoryEntry> {
    entries.sort_by(|a, b| b.last_active.cmp(&a.last_active));
    let mut used = 0usize;
    let keep = entries
        .iter()
        .take_while(|e| {
            used = used.saturating_add(serde_json::to_vec(e).map_or(usize::MAX / 2, |v| v.len() + 1));
            used <= max
        })
        .count();
    entries.truncate(keep);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    fn entry(n: u64) -> HistoryEntry {
        HistoryEntry {
            agent: gilvt_agent::AgentKind::Claude,
            session_id: format!("s{n}"),
            cwd: "/home/u/r".into(),
            transcript: format!("/home/u/.claude/projects/-home-u-r/s{n}.jsonl").into(),
            first_prompt: "x".repeat(100),
            topic_prompt: String::new(),
            custom_title: None,
            ai_title: None,
            started: None,
            last_active: UNIX_EPOCH + Duration::from_secs(n),
            turns: 1,
            model: None,
            size: 1,
        }
    }

    #[test]
    fn history_list_is_trimmed_to_fit_a_frame() {
        let all: Vec<HistoryEntry> = (0..50_000).map(entry).collect();
        let kept = fit_entries(all, HISTORY_BYTES);
        assert!(!kept.is_empty() && kept.len() < 50_000);
        assert_eq!(kept[0].session_id, "s49999", "newest first");
        let resp = gilvt_ipc::remote::DaemonMsg::Response { id: 1, resp: gilvt_ipc::remote::RemoteResponse::HistoryList { entries: kept } };
        let mut buf = Vec::new();
        gilvt_ipc::remote::write_frame(&mut buf, &resp).expect("fits in one frame");
    }

    #[test]
    fn concurrent_requests_share_one_scan() {
        let scans = Arc::new(AtomicUsize::new(0));
        let counted = scans.clone();
        let worker = HistoryWorker::new(Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(300));
            Ok(vec![entry(1)])
        }));
        let (tx, rx) = std::sync::mpsc::channel();
        for _ in 0..3 {
            let tx = tx.clone();
            worker.request(move |r| tx.send(r.map(|e| e.len())).unwrap());
        }
        for _ in 0..3 {
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(1));
        }
        assert_eq!(scans.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failing_scan_answers_an_error() {
        let worker = HistoryWorker::new(Arc::new(|| panic!("boom")));
        let (tx, rx) = std::sync::mpsc::channel();
        worker.request(move |r| tx.send(r.is_err()).unwrap());
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
    }

    #[test]
    fn the_real_scanner_answers_for_an_empty_home() {
        let home = tempfile::tempdir().unwrap();
        let scan = scanner(home.path().join(".gilvt-server/state"), home.path().to_path_buf());
        assert_eq!(scan().unwrap(), Vec::new());
    }
}
