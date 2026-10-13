//! The daemon's file workers (R2a §8.1): reads, writes and git run here, never on a bridge's read loop or on the hook
//! socket, so a slow disk or a big repository holds up neither hooks nor other requests. A panicking job is
//! contained; the worker goes on.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

pub const FILE_WORKERS: usize = 4;

type Job = Box<dyn FnOnce() + Send>;

pub struct FileWorkers {
    tx: Mutex<Sender<Job>>,
}

impl FileWorkers {
    pub fn new(n: usize) -> FileWorkers {
        let (tx, rx) = channel::<Job>();
        let rx: Arc<Mutex<Receiver<Job>>> = Arc::new(Mutex::new(rx));
        for _ in 0..n.max(1) {
            let rx = rx.clone();
            std::thread::spawn(move || loop {
                let job = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                match job {
                    Ok(job) => {
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                    }
                    Err(_) => return,
                }
            });
        }
        FileWorkers { tx: Mutex::new(tx) }
    }

    pub fn submit(&self, job: impl FnOnce() + Send + 'static) {
        let _ = self.tx.lock().unwrap_or_else(|e| e.into_inner()).send(Box::new(job));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn slow_jobs_run_side_by_side() {
        let w = FileWorkers::new(FILE_WORKERS);
        let (tx, rx) = std::sync::mpsc::channel();
        let start = Instant::now();
        for _ in 0..FILE_WORKERS {
            let tx = tx.clone();
            w.submit(move || {
                std::thread::sleep(Duration::from_millis(300));
                tx.send(()).unwrap();
            });
        }
        for _ in 0..FILE_WORKERS {
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        assert!(start.elapsed() < Duration::from_millis(900), "four 300 ms jobs took {:?}", start.elapsed());
    }

    #[test]
    fn a_panicking_job_does_not_take_a_worker_down() {
        let w = FileWorkers::new(1);
        w.submit(|| panic!("boom"));
        let (tx, rx) = std::sync::mpsc::channel();
        w.submit(move || tx.send(7).unwrap());
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 7);
    }
}
