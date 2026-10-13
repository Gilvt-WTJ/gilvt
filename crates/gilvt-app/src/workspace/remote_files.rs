//! Cmd+click on a path in an ssh pane (R2a §8.3): one `Stat` of every reading of the token, then the first that exists
//! on the host opens (Quick Look, or the editor with ⇧). Never a file of this Mac.

use gilvt_agent::host::HostPath;
use gilvt_ipc::remote::StatEntry;
use gilvt_term::paths::{PathHit, PathProbe};
use gpui::{Context, Window};

use super::Workspace;
use crate::pane_tree::PaneId;
use crate::preview_view::OpenRequest;
use crate::remote::fs::{no_such_file, FsFailure, RemoteFs};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ClickOutcome {
    Open(PathHit),
    Missing,
    Failed(String),
}

/// `stats` answer `probe.paths()` in order; only a regular file counts (as `is_file` does locally).
pub(crate) fn decide_remote_click(probe: &PathProbe, stats: Result<Vec<StatEntry>, FsFailure>, display: &str) -> ClickOutcome {
    let stats = match stats {
        Ok(s) => s,
        Err(e) => return ClickOutcome::Failed(e.message(display)),
    };
    let files: std::collections::HashSet<std::path::PathBuf> = probe.paths().into_iter().zip(stats).filter(|(_, s)| s.exists && s.is_file).map(|(p, _)| p).collect();
    match probe.pick(|p| files.contains(p)) {
        Some(hit) => ClickOutcome::Open(hit),
        None => ClickOutcome::Missing,
    }
}

impl Workspace {
    pub(crate) fn open_remote_path(&mut self, origin: PaneId, probe: PathProbe, in_editor: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.pane_remote(origin, cx) else { return };
        // A plain login has no daemon to ask (R2a plan deviation 11).
        if !r.enhanced {
            self.refuse_remote(origin, cx);
            return;
        }
        let fs = RemoteFs::new(&r.host, cx);
        let pending = self.open_preview_pending(Some(origin), window, cx);
        let stat = fs.stat(probe.paths(), cx);
        cx.spawn_in(window, async move |ws, cx| {
            let outcome = decide_remote_click(&probe, stat.await, &fs.display);
            let _ = ws.update_in(cx, |ws, window, cx| match outcome {
                ClickOutcome::Open(hit) if in_editor => {
                    ws.close_quicklook_if(&pending, window, cx);
                    ws.open_editor_at(HostPath::remote(fs.host.clone(), hit.path), hit.line, Some(origin), false, window, cx);
                }
                ClickOutcome::Open(hit) => pending.update(cx, |v, cx| v.replace_request(OpenRequest::at(HostPath::remote(fs.host.clone(), hit.path), hit.line, hit.annotation), cx)),
                ClickOutcome::Missing => {
                    ws.close_quicklook_if(&pending, window, cx);
                    ws.show_error(no_such_file(&fs.display), cx);
                }
                ClickOutcome::Failed(message) => {
                    ws.close_quicklook_if(&pending, window, cx);
                    ws.show_error(message, cx);
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_term::paths::path_probe;

    fn probe_for(text: &str, cwd: &std::path::Path) -> gilvt_term::paths::PathProbe {
        let snapshot = gilvt_term::snapshot::snapshot_of(text, 80, 2);
        path_probe(&snapshot, 0, 1, Some(cwd), None).unwrap()
    }

    fn entry(exists: bool) -> StatEntry {
        StatEntry { exists, is_file: exists, size: 1, mtime_ns: 1 }
    }

    #[test]
    fn a_missing_remote_file_is_missing_whatever_the_mac_has() {
        // `decide_remote_click` only reads the host's answer. That the click never looks at this Mac's disk is the
        // job of `path_click` (terminal_view.rs, `a_remote_pane_never_asks_the_local_disk`).
        let probe = probe_for("decoy.txt", std::path::Path::new("/home/dev"));
        let missing = vec![entry(false); probe.paths().len()];
        assert_eq!(decide_remote_click(&probe, Ok(missing), "devbox"), ClickOutcome::Missing);
        let a_directory = vec![StatEntry { exists: true, is_file: false, size: 0, mtime_ns: 1 }; probe.paths().len()];
        assert_eq!(decide_remote_click(&probe, Ok(a_directory), "devbox"), ClickOutcome::Missing, "only a regular file opens");
    }

    #[test]
    fn the_first_existing_candidate_opens() {
        let probe = probe_for("b/x.rs:3", std::path::Path::new("/home/dev"));
        let stats: Vec<StatEntry> = probe.paths().iter().map(|p| entry(p == std::path::Path::new("/home/dev/x.rs"))).collect();
        let ClickOutcome::Open(hit) = decide_remote_click(&probe, Ok(stats), "devbox") else { panic!() };
        assert_eq!((hit.path, hit.line), (std::path::PathBuf::from("/home/dev/x.rs"), Some(3)));
    }

    #[test]
    fn an_unreachable_host_says_so() {
        let probe = probe_for("a.rs", std::path::Path::new("/home/dev"));
        assert_eq!(decide_remote_click(&probe, Err(FsFailure::NotConnected), "devbox"), ClickOutcome::Failed("devbox 未连接".into()));
        assert_eq!(decide_remote_click(&probe, Ok(Vec::new()), "devbox"), ClickOutcome::Missing, "a short answer finds nothing");
    }
}
