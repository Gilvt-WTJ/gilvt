//! Files and git on the machine a path is on (spec §6, R2a §8.3). `Local` runs the existing blocking functions on the
//! background executor (unchanged behaviour) and is the only implementation of local preview loading and of the local
//! changed-file list (`PreviewView::spawn_load` and the local `Diff` request go through it, Task 8; plan ruling 27);
//! `Remote` asks the host's daemon through its bridge. Both return a gpui
//! `Task`, so callers never block the main thread and never look at the Mac's disk for a remote path.

use std::path::PathBuf;

use gilvt_agent::host::HostPath;
use gilvt_ipc::remote::Fingerprint;
use gilvt_viewer::{DiffBase, Preview};
use gpui::{App, Task};

use crate::remote::fs::RemoteFs;

pub enum HostFs {
    Local,
    Remote(RemoteFs),
}

impl HostFs {
    pub fn of(path: &HostPath, cx: &App) -> HostFs {
        match path.remote_host() {
            Some(host) => HostFs::Remote(RemoteFs::new(host, cx)),
            None => HostFs::Local,
        }
    }

    /// The file and its diff against `base`, as Quick Look shows it, and for a remote file the fingerprint it was read
    /// at (what its watch compares with after a reconnect). Errors are the text to show. `base.rev()` None (a plain
    /// view) asks the host for no base, as the local `Preview::load` runs no `git show` for it.
    pub fn load_preview(&self, path: PathBuf, base: DiffBase, cx: &mut App) -> Task<Result<(Preview, Option<Fingerprint>), String>> {
        match self {
            HostFs::Local => cx.background_executor().spawn(async move { Preview::load(&path, &base).map(|p| (p, None)).map_err(|e| format!("{}: {e}", path.display())) }),
            HostFs::Remote(fs) => {
                let display = fs.display.clone();
                let read = fs.read_for_preview(path.clone(), base.rev().map(str::to_string), cx);
                cx.spawn(async move |cx| match read.await {
                    Ok(r) => {
                        let fp = r.fingerprint;
                        Ok((cx.background_executor().spawn(async move { Preview::from_remote(&path, r.bytes, r.git_root, &base, r.base_text, r.base_skipped) }).await, Some(fp)))
                    }
                    Err(crate::remote::fs::FsFailure::TooLarge { size }) if size > 0 => Ok((Preview::too_large(&path, size), None)),
                    Err(e) => Err(e.message(&display)),
                })
            }
        }
    }
}

pub enum HostGit {
    Local,
    Remote(RemoteFs),
}

impl HostGit {
    /// The files `gilvt diff` lists for `cwd`'s repository against `rev` (None: HEAD), computed once. Local errors give
    /// an empty list, as before; a remote error is the text to show.
    pub fn changed_files(&self, cwd: PathBuf, rev: Option<String>, cx: &mut App) -> Task<Result<Vec<PathBuf>, String>> {
        match self {
            HostGit::Local => cx.background_executor().spawn(async move {
                let base = rev.unwrap_or_else(|| "HEAD".into());
                Ok(gilvt_viewer::git::repo_root(&cwd).and_then(|root| gilvt_viewer::git::changed_files(&root, &base).ok()).unwrap_or_default())
            }),
            HostGit::Remote(fs) => {
                let display = fs.display.clone();
                let read = fs.git_changes(cwd, rev, cx);
                cx.spawn(async move |_| read.await.map_err(|e| e.message(&display)))
            }
        }
    }
}
