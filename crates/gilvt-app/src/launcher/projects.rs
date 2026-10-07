//! Projects of past sessions' directories for the 会话 palette (the current-project filter and the rows'
//! project names), the same rule as the sidebar's groups: the main git repository (every worktree of one
//! repository is one project), else the nearest `.git` ancestor, else the directory itself.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui::{App, Global};

use super::sessions_model::Project;
use crate::agents::{fallback_name, project_root};
use crate::workspace;

/// Resolved on the background executor (it stats `.git` upwards). One for all windows; `generation` changes
/// whenever some were added.
#[derive(Default)]
pub struct Projects {
    map: HashMap<PathBuf, Project>,
    pending: HashSet<PathBuf>,
    generation: u64,
}

impl Global for Projects {}

impl Projects {
    /// The resolved project, else the directory itself (until resolved).
    pub fn get(&self, cwd: &Path) -> Project {
        self.map.get(cwd).cloned().unwrap_or_else(|| Project { root: cwd.to_path_buf(), name: fallback_name(cwd) })
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Resolves the ones of `dirs` not known or being resolved yet, then repaints the windows.
    pub fn resolve(dirs: HashSet<PathBuf>, cx: &mut App) {
        let projects = cx.default_global::<Projects>();
        let dirs: Vec<PathBuf> = dirs.into_iter().filter(|d| !projects.map.contains_key(d) && !projects.pending.contains(d)).collect();
        if dirs.is_empty() {
            return;
        }
        projects.pending.extend(dirs.iter().cloned());
        let job = cx.background_executor().spawn(async move {
            dirs.into_iter()
                .map(|d| {
                    // The main repository, so the worktrees of one repository are one project (as in the sidebar);
                    // outside git (or without it) the nearest `.git` ancestor, else the directory itself.
                    let project = match gilvt_agent::main_repo_of(&d) {
                        Some((root, name)) => Project { root, name },
                        None => {
                            let root = project_root(&d).to_path_buf();
                            Project { name: fallback_name(&root), root }
                        }
                    };
                    (d, project)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |cx| {
            let found = job.await;
            let _ = cx.update(|cx| {
                let projects = cx.default_global::<Projects>();
                for (dir, project) in found {
                    projects.pending.remove(&dir);
                    projects.map.insert(dir, project);
                }
                projects.generation += 1;
                cx.defer(workspace::notify_all);
            });
        })
        .detach();
    }
}
