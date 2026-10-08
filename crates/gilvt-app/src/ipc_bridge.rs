//! Receives `gilvt` CLI requests on the app's socket and opens previews in the right window; answers
//! `gilvt debug state` queries on the main thread when gilvt was started with `GILVT_DEBUG_STATE=1`
//! (otherwise the socket refuses them without involving the app), and the 监控官's tool calls (`gilvt mcp`) always.

use std::path::PathBuf;

use gpui::App;
use gilvt_ipc::{remove_stale_sockets, socket_path_for, DebugQueries, Query, Request, Server, DEBUG_STATE_DISABLED, QUERY_QUEUE};

use crate::debug_state;
use crate::preview_view::{OpenRequest, Source};
use crate::workspace::Workspace;

/// A `view` request as a preview to open: (pane it came from, request, pinned?).
pub fn view_request(req: Request) -> Option<(Option<u64>, OpenRequest, bool)> {
    let Request::View { pane, path, content, as_type, line, col: _, pin } = req else { return None };
    let source = match (path, content) {
        (Some(path), _) => Source::File(path),
        (None, Some(content)) => Source::Inline { content, as_type },
        (None, None) => return None,
    };
    Some((pane, OpenRequest { sources: vec![source], index: 0, line, annotation: None, rev: None, turn: None }, pin))
}

/// A `diff` request's preview: every changed file in the repository containing `cwd`.
pub fn diff_request(files: Vec<PathBuf>, rev: Option<String>) -> OpenRequest {
    OpenRequest { rev, ..OpenRequest::files(files) }
}

/// Opens `req` in the window that owns `pane` (the terminal `gilvt` ran in), else the active window,
/// else the first one.
fn route(pane: Option<u64>, req: OpenRequest, pin: bool, cx: &mut App) {
    let windows: Vec<_> = cx.windows().into_iter().filter_map(|w| w.downcast::<Workspace>()).collect();
    let owner = pane.and_then(|p| windows.iter().copied().find(|w| w.read(cx).is_ok_and(|ws| ws.has_pane(p))));
    let active = || cx.active_window().and_then(|w| w.downcast::<Workspace>());
    if let Some(window) = owner.or_else(active).or_else(|| windows.first().copied()) {
        let _ = window.update(cx, |ws, window, cx| {
            window.activate_window();
            ws.open_preview(req, pin, pane, window, cx);
        });
    }
}

/// Binds this process's socket and handles requests until the app exits; `Hook` requests go to
/// `hooks` (the agents bridge). Returns the socket path.
pub fn start(hooks: async_channel::Sender<Request>, cx: &mut App) -> Option<PathBuf> {
    let path = socket_path_for(std::process::id());
    if let Some(dir) = path.parent() {
        remove_stale_sockets(dir);
    }
    let (tx, rx) = async_channel::unbounded();
    let debug = debug_state::enabled();
    let (query_tx, query_rx) = async_channel::bounded::<Query>(QUERY_QUEUE);
    let (monitor_tx, monitor_rx) = async_channel::bounded::<Query>(QUERY_QUEUE);
    let (remote_tx, remote_rx) = async_channel::bounded::<Query>(QUERY_QUEUE);
    let debug_queries = if debug { DebugQueries::Answered(query_tx) } else { DebugQueries::Refused(DEBUG_STATE_DISABLED) };
    let started = Server::start_with_monitor(&path, tx, debug_queries, monitor_tx, remote_tx);
    let server = match started {
        Ok(s) => s,
        Err(e) => {
            eprintln!("gilvt: IPC disabled ({}): {e}", path.display());
            return None;
        }
    };
    cx.spawn(async move |cx| {
        // The server lives as long as this task: its Drop removes the socket file.
        let _server = server;
        while let Ok(req) = rx.recv().await {
            match req {
                Request::Diff { pane, cwd, rev } => {
                    let base = rev.clone().unwrap_or_else(|| "HEAD".into());
                    let files = cx
                        .background_executor()
                        .spawn(async move {
                            gilvt_viewer::git::repo_root(&cwd)
                                .and_then(|root| gilvt_viewer::git::changed_files(&root, &base).ok())
                                .unwrap_or_default()
                        })
                        .await;
                    let _ = cx.update(|cx| route(pane, diff_request(files, rev), false, cx));
                }
                hook @ Request::Hook { .. } => {
                    let _ = hooks.try_send(hook);
                }
                other => {
                    if let Some((pane, req, pin)) = view_request(other) {
                        let _ = cx.update(|cx| route(pane, req, pin, cx));
                    }
                }
            }
        }
    })
    .detach();
    // Separate from the loop above, so a slow `diff` never delays an answer.
    if debug {
        cx.spawn(async move |cx| {
            while let Ok(first) = query_rx.recv().await {
                // Everything queued meanwhile is answered from the same snapshot.
                let mut batch = vec![first];
                while let Ok(q) = query_rx.try_recv() {
                    batch.push(q);
                }
                let batch = debug_state::live_queries(batch, std::time::Instant::now());
                let Some(tail) = batch.iter().map(|(_, t)| *t).max() else { continue };
                debug_state::rects::start_recording();
                // Draw every window once first (outside `update`), so covered windows have current rects.
                if let Ok(views) = cx.update(debug_state::views_to_draw) {
                    views.into_iter().for_each(debug_state::draw_now);
                }
                match cx.update(|cx| debug_state::collect(tail, cx)) {
                    Ok(state) => batch.into_iter().for_each(|(q, tail)| q.respond(debug_state::answer(&state, tail))),
                    // The app is going away; dropping the queries answers their clients.
                    Err(_) => break,
                }
                // Let the monitor loop (and everything else on the main thread) run before the next batch.
                YieldNow(false).await;
            }
        })
        .detach();
    }
    // 监控官 tool calls (`gilvt mcp`): answered one by one on the main thread, debug state or not. Expired ones are
    // skipped by `answer_query`. Never log a query: it carries the token.
    cx.spawn(async move |cx| {
        while let Ok(q) = monitor_rx.recv().await {
            if cx.update(|cx| crate::monitor::tools::answer_query(q, cx)).is_err() {
                break;
            }
            // One at a time: a full queue never holds back debug state queries or drawing.
            YieldNow(false).await;
        }
    })
    .detach();
    // `gilvt ssh` link starts: answered on the main thread.
    cx.spawn(async move |cx| {
        while let Ok(q) = remote_rx.recv().await {
            let _ = cx.update(|cx| crate::remote::answer_begin(q, cx));
        }
    })
    .detach();
    Some(path)
}

/// Resolves on its second poll: the task goes to the back of the main thread's queue once (a channel with items
/// waiting never suspends `recv`, so a loop draining it would not let other tasks in).
struct YieldNow(bool);

impl std::future::Future for YieldNow {
    type Output = ();

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        if self.0 {
            return std::task::Poll::Ready(());
        }
        self.0 = true;
        cx.waker().wake_by_ref();
        std::task::Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_requests_become_previews() {
        let req = Request::View { pane: Some(2), path: Some("/r/a.rs".into()), content: None, as_type: None, line: Some(9), col: Some(1), pin: true };
        let (pane, open, pin) = view_request(req).unwrap();
        assert_eq!((pane, pin), (Some(2), true));
        assert_eq!(open.sources, vec![Source::File("/r/a.rs".into())]);
        assert_eq!(open.line, Some(9));

        let req = Request::View { pane: None, path: None, content: Some("x".into()), as_type: Some("md".into()), line: None, col: None, pin: false };
        let (_, open, _) = view_request(req).unwrap();
        assert_eq!(open.sources, vec![Source::Inline { content: "x".into(), as_type: Some("md".into()) }]);

        let empty = Request::View { pane: None, path: None, content: None, as_type: None, line: None, col: None, pin: false };
        assert!(view_request(empty).is_none());
    }

    #[test]
    fn diff_requests_list_files() {
        let open = diff_request(vec!["/r/a".into(), "/r/b".into()], Some("main".into()));
        assert_eq!(open.sources.len(), 2);
        assert_eq!(open.rev.as_deref(), Some("main"));
    }
}
