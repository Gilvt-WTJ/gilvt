//! `gilvt view` / `gilvt diff` run in a remote shell (R2a §8.2), arriving as the daemon's `Open` event for the pane of
//! that shell's link. A view opens the remote file (or the piped text); a diff lists the remote changes once
//! (`GitChanges`) behind a pending Quick Look, then each file is read when shown.

use std::path::PathBuf;

use gilvt_agent::host::HostPath;
use gilvt_ipc::Request;
use gpui::App;

use crate::ipc_bridge::{diff_request, route, route_pending, view_request};
use crate::preview_view::OpenRequest;

pub enum OpenPlan {
    Preview { req: OpenRequest, pin: bool },
    Diff { cwd: PathBuf, rev: Option<String> },
    Nothing,
}

pub fn plan(host: &str, request: Request) -> OpenPlan {
    match request {
        Request::Diff { cwd, rev, .. } => OpenPlan::Diff { cwd, rev },
        view @ Request::View { .. } => match view_request(view, Some(host)) {
            Some((_, req, pin)) => OpenPlan::Preview { req, pin },
            None => OpenPlan::Nothing,
        },
        _ => OpenPlan::Nothing,
    }
}

pub fn open(host: &str, pane: u64, request: Request, cx: &mut App) {
    match plan(host, request) {
        OpenPlan::Preview { req, pin } => route(Some(pane), req, pin, cx),
        OpenPlan::Diff { cwd, rev } => {
            let Some(view) = route_pending(Some(pane), cx) else { return };
            // Weak: closing the pending Quick Look must not keep the view alive or feed it a late answer.
            let view = view.downgrade();
            let git = crate::hostfs::HostGit::Remote(super::fs::RemoteFs::new(host, cx));
            let list = git.changed_files(cwd, rev.clone(), cx);
            let host = host.to_string();
            cx.spawn(async move |cx| {
                let result = list.await;
                let _ = view.update(cx, |v, cx| match result {
                    Ok(files) => v.replace_request(diff_request(files.into_iter().map(|p| HostPath::remote(host.clone(), p)).collect(), rev), cx),
                    Err(message) => v.fail(message, cx),
                });
            })
            .detach();
        }
        OpenPlan::Nothing => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview_view::Source;

    #[test]
    fn plans_a_view_a_stdin_view_and_a_diff() {
        let view = Request::View { pane: None, path: Some("/home/dev/a.rs".into()), content: None, as_type: None, line: None, col: None, pin: false, link: Some("i-1".into()) };
        let OpenPlan::Preview { req, pin: false } = plan("dev@h:22", view) else { panic!() };
        assert_eq!(req.sources, vec![Source::File(HostPath::remote("dev@h:22", "/home/dev/a.rs"))]);
        let stdin = Request::View { pane: None, path: None, content: Some("hi\n".into()), as_type: Some("md".into()), line: None, col: None, pin: true, link: Some("i-1".into()) };
        let OpenPlan::Preview { req, pin: true } = plan("dev@h:22", stdin) else { panic!() };
        assert_eq!(req.sources, vec![Source::Inline { content: "hi\n".into(), as_type: Some("md".into()) }]);
        let diff = Request::Diff { pane: None, cwd: "/home/dev/r".into(), rev: None, link: Some("i-1".into()) };
        assert!(matches!(plan("dev@h:22", diff), OpenPlan::Diff { ref cwd, rev: None } if cwd == std::path::Path::new("/home/dev/r")));
        assert!(matches!(plan("dev@h:22", Request::DebugState { tail_lines: 1 }), OpenPlan::Nothing));
    }
}
