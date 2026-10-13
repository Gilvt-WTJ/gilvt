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
    // A reading the host could not look at (EACCES…) is not "missing": when nothing opens, say why.
    let failure = stats.iter().find_map(|s| s.error.clone());
    let files: std::collections::HashSet<std::path::PathBuf> = probe.paths().into_iter().zip(stats).filter(|(_, s)| s.exists && s.is_file).map(|(p, _)| p).collect();
    match probe.pick(|p| files.contains(p)) {
        Some(hit) => ClickOutcome::Open(hit),
        None => failure.map_or(ClickOutcome::Missing, ClickOutcome::Failed),
    }
}

/// Numbers the ⌘-clicks of a window: only the answer to the latest one acts (an earlier one answered late would open
/// a second file or put up a stale error). Holds the latest click's Quick Look placeholder until it is answered.
pub(crate) struct ClickGen<P = gpui::Entity<crate::preview_view::PreviewView>> {
    latest: u64,
    pending: Option<P>,
}

impl<P> Default for ClickGen<P> {
    fn default() -> Self {
        ClickGen { latest: 0, pending: None }
    }
}

impl<P> ClickGen<P> {
    /// A new click with its placeholder (None for ⇧⌘-click). Returns its number and the placeholder of an earlier
    /// click still unanswered: that answer will be dropped, so its placeholder must close.
    pub(crate) fn start(&mut self, pending: Option<P>) -> (u64, Option<P>) {
        self.latest += 1;
        (self.latest, std::mem::replace(&mut self.pending, pending))
    }

    /// `click` was answered: whether it is still the latest (only then does its answer act).
    pub(crate) fn finish(&mut self, click: u64) -> bool {
        let current = click == self.latest;
        if current {
            self.pending = None;
        }
        current
    }
}

impl Workspace {
    /// `view` is on screen: Quick Look holds it, or it was pinned into a pane.
    pub(crate) fn shows_preview(&self, view: &gpui::Entity<crate::preview_view::PreviewView>) -> bool {
        self.quicklook.as_ref().is_some_and(|(v, _)| v == view) || self.panes.values().any(|p| matches!(p, super::PaneView::Preview(v) if v == view))
    }

    pub(crate) fn open_remote_path(&mut self, origin: PaneId, probe: PathProbe, in_editor: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.pane_remote(origin, cx) else { return };
        // A plain login has no daemon to ask (R2a plan deviation 11).
        if !r.enhanced {
            self.refuse_remote(origin, cx);
            return;
        }
        let fs = RemoteFs::new(&r.host, cx);
        // ⇧⌘-click opens the editor, which shows its own 「正在读取…」: no Quick Look placeholder for it.
        let pending = (!in_editor).then(|| self.open_preview_pending(Some(origin), window, cx));
        let (click, stale) = self.remote_clicks.start(pending.clone());
        if let Some(stale) = stale {
            self.close_quicklook_if(&stale, window, cx);
        }
        let stat = fs.stat(probe.paths(), cx);
        cx.spawn_in(window, async move |ws, cx| {
            let outcome = decide_remote_click(&probe, stat.await, &fs.display);
            let _ = ws.update_in(cx, |ws, window, cx| {
                // A newer click owns Quick Look and the banner now (its own placeholder replaced this one's).
                if !ws.remote_clicks.finish(click) {
                    return;
                }
                match (outcome, pending) {
                    // Only into a placeholder that is still shown (Quick Look, or pinned as a pane): one dismissed
                    // while the Stat ran is gone, and loading it would only send a Watch and its Unwatch.
                    (ClickOutcome::Open(hit), Some(pending)) => {
                        if ws.shows_preview(&pending) {
                            pending.update(cx, |v, cx| v.replace_request(OpenRequest::at(HostPath::remote(fs.host.clone(), hit.path), hit.line, hit.annotation), cx))
                        }
                    }
                    (ClickOutcome::Open(hit), None) => ws.open_editor_at(HostPath::remote(fs.host.clone(), hit.path), hit.line, Some(origin), false, window, cx),
                    (ClickOutcome::Missing, pending) => {
                        if let Some(p) = pending {
                            ws.close_quicklook_if(&p, window, cx);
                        }
                        ws.show_error(no_such_file(&fs.display), cx);
                    }
                    (ClickOutcome::Failed(message), pending) => {
                        if let Some(p) = pending {
                            ws.close_quicklook_if(&p, window, cx);
                        }
                        ws.show_error(message, cx);
                    }
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
        StatEntry { exists, is_file: exists, size: 1, mtime_ns: 1, error: None }
    }

    #[test]
    fn a_missing_remote_file_is_missing_whatever_the_mac_has() {
        // `decide_remote_click` only reads the host's answer. That the click never looks at this Mac's disk is the
        // job of `path_click` (terminal_view.rs, `a_remote_pane_never_asks_the_local_disk`).
        let probe = probe_for("decoy.txt", std::path::Path::new("/home/dev"));
        let missing = vec![entry(false); probe.paths().len()];
        assert_eq!(decide_remote_click(&probe, Ok(missing), "devbox"), ClickOutcome::Missing);
        let a_directory = vec![StatEntry { exists: true, is_file: false, size: 0, mtime_ns: 1, error: None }; probe.paths().len()];
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
    fn only_the_latest_click_acts() {
        let mut clicks = ClickGen::default();
        let (first, _) = clicks.start(Some("A"));
        let (second, replaced) = clicks.start(None); // a ⇧⌘-click: no placeholder of its own
        assert_eq!(replaced, Some("A"), "the unanswered placeholder is handed back to be closed");
        assert!(!clicks.finish(first), "a slow first answer is dropped: no banner, no editor");
        assert!(clicks.finish(second));
        let (_, replaced) = clicks.start(Some("C"));
        assert_eq!(replaced, None, "an answered click's view is not closed by the next one");
    }

    #[test]
    fn a_stat_failure_is_shown_not_called_missing() {
        let probe = probe_for("a.rs", std::path::Path::new("/home/dev"));
        let denied: Vec<StatEntry> = probe.paths().iter().map(|p| StatEntry { error: Some(format!("{}: Permission denied (os error 13)", p.display())), ..StatEntry::default() }).collect();
        let ClickOutcome::Failed(m) = decide_remote_click(&probe, Ok(denied), "devbox") else { panic!("EACCES is not 「devbox 上没有这个文件」") };
        assert!(m.contains("Permission denied"), "{m}");
    }

    #[test]
    fn an_unreachable_host_says_so() {
        let probe = probe_for("a.rs", std::path::Path::new("/home/dev"));
        assert_eq!(decide_remote_click(&probe, Err(FsFailure::NotConnected), "devbox"), ClickOutcome::Failed("devbox 未连接".into()));
        assert_eq!(decide_remote_click(&probe, Ok(Vec::new()), "devbox"), ClickOutcome::Missing, "a short answer finds nothing");
    }
}
