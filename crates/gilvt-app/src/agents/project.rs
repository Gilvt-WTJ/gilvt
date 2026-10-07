//! A session's project for the sidebar and the 会话 palette: its git root's name, else the cwd's last component.

use std::path::Path;

/// Stats `.git` in each ancestor (blocking: call off the main thread).
pub fn project_name(cwd: &Path) -> String {
    fallback_name(project_root(cwd))
}

/// The nearest ancestor holding a `.git` (directory, or a worktree's file), else `cwd` itself. Blocking, like
/// [`project_name`].
pub fn project_root(cwd: &Path) -> &Path {
    cwd.ancestors().find(|d| d.join(".git").exists()).unwrap_or(cwd)
}

/// The last path component, or the whole path for `/`.
pub fn fallback_name(path: &Path) -> String {
    match path.file_name() {
        Some(n) => n.to_string_lossy().into_owned(),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_root_else_last_component() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("myrepo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("crates/a")).unwrap();
        assert_eq!(project_name(&repo.join("crates/a")), "myrepo");
        assert_eq!(project_root(&repo.join("crates/a")), repo);
        // A worktree's `.git` is a file.
        let wt = dir.path().join("wt");
        std::fs::create_dir_all(wt.join("src")).unwrap();
        std::fs::write(wt.join(".git"), "gitdir: /elsewhere").unwrap();
        assert_eq!(project_name(&wt.join("src")), "wt");
        let plain = dir.path().join("plain/sub");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(project_name(&plain), "sub");
        assert_eq!(fallback_name(Path::new("/")), "/");
    }
}
