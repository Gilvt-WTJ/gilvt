//! Where a search runs, and paths relative to a pane's cwd.

use std::path::{Component, Path, PathBuf};

use crate::git;

/// Where a search runs: the git repository containing `cwd`, else `cwd` itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    pub dir: PathBuf,
    pub is_repo: bool,
}

/// Blocking (runs `git`). A repository root is spelled as an ancestor of `cwd` when one names it
/// (`/tmp/r`, not git's `/private/tmp/r`), so paths under it stay comparable with the pane's cwd.
pub fn search_root(cwd: &Path) -> Root {
    match git::toplevel(cwd) {
        Some(top) => Root { dir: as_ancestor_of(cwd, top), is_repo: true },
        None => Root { dir: cwd.to_path_buf(), is_repo: false },
    }
}

fn as_ancestor_of(cwd: &Path, top: PathBuf) -> PathBuf {
    let canonical = top.canonicalize().unwrap_or_else(|_| top.clone());
    cwd.ancestors().find(|a| a.canonicalize().is_ok_and(|c| c == canonical)).map_or(top, Path::to_path_buf)
}

/// Path of `target` (absolute) relative to `from` (absolute directory), using `..` as needed; `.` when
/// they are the same. Purely lexical: symlinks are not resolved.
pub fn relative_to(target: &Path, from: &Path) -> PathBuf {
    let target: Vec<Component> = target.components().collect();
    let from: Vec<Component> = from.components().collect();
    let common = target.iter().zip(&from).take_while(|(t, f)| t == f).count();
    let mut rel: PathBuf = from[common..].iter().map(|_| Component::ParentDir).collect();
    rel.extend(&target[common..]);
    if rel.as_os_str().is_empty() {
        rel.push(".");
    }
    rel
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    #[test]
    fn root_of_a_repository_subdirectory() {
        let repo = testutil::repo();
        let sub = repo.path().join("sub/dir");
        std::fs::create_dir_all(&sub).unwrap();
        // The temp dir lives under a symlink on macOS (/var → /private/var); git reports the resolved path.
        assert_eq!(search_root(&sub), Root { dir: repo.path().to_path_buf(), is_repo: true });
        assert_eq!(search_root(repo.path()), Root { dir: repo.path().to_path_buf(), is_repo: true });
    }

    #[test]
    fn root_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(search_root(dir.path()), Root { dir: dir.path().to_path_buf(), is_repo: false });
    }

    #[test]
    fn relative_paths() {
        let rel = |t: &str, f: &str| relative_to(Path::new(t), Path::new(f));
        assert_eq!(rel("/r/a/x.rs", "/r/a"), Path::new("x.rs"));
        assert_eq!(rel("/r/a/b/c/x.rs", "/r/a"), Path::new("b/c/x.rs"));
        assert_eq!(rel("/r/b/x.rs", "/r/a"), Path::new("../b/x.rs"));
        assert_eq!(rel("/r/x.rs", "/r/a/b"), Path::new("../../x.rs"));
        assert_eq!(rel("/x.rs", "/r/a"), Path::new("../../x.rs"));
        assert_eq!(rel("/r/a", "/r/a"), Path::new("."));
        assert_eq!(rel("/r/a/", "/r/a"), Path::new("."));
        assert_eq!(rel("/r/中文 目录/x.md", "/r/a"), Path::new("../中文 目录/x.md"));
    }
}
