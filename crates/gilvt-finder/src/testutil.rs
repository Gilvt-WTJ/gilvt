//! Temp git repositories for tests.

use std::path::Path;
use std::process::Command;

/// Runs `git -C dir <args>`, panicking on failure.
pub fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap().status.success();
    assert!(ok, "git {args:?}");
}

/// An empty repository with a configured committer.
pub fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.email", "t@example.com"]);
    git(dir.path(), &["config", "user.name", "t"]);
    dir
}

/// Writes `rel` under `dir` (creating parents).
pub fn write(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}
