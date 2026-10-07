//! The few git queries the viewer needs, via the system `git` (same behavior as the user's CLI).

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn git(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(dir).stdin(Stdio::null()).stderr(Stdio::null());
    c
}

/// Top-level directory of the repository containing `dir` (a directory or a file's parent).
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    let out = git(dir).args(["rev-parse", "--show-toplevel"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let root = String::from_utf8(out.stdout).ok()?;
    Some(PathBuf::from(root.trim_end()))
}

/// Revisions starting with `-` would be parsed as options (e.g. `--output=<file>`), so they are refused.
fn is_option(rev: &str) -> bool {
    rev.starts_with('-')
}

/// Contents of `rel` (relative to `root`) at `rev`; `Ok(None)` when the file does not exist there.
pub fn show_file(root: &Path, rev: &str, rel: &Path) -> io::Result<Option<String>> {
    if is_option(rev) {
        return Ok(None);
    }
    let spec = format!("{rev}:{}", rel.display());
    let out = git(root).args(["show", "--no-textconv", &spec]).output()?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// Whether `rev` names a commit in the repository at `root`.
pub fn is_commit(root: &Path, rev: &str) -> bool {
    if is_option(rev) {
        return false;
    }
    let spec = format!("{rev}^{{commit}}");
    git(root).args(["rev-parse", "--verify", "--quiet", &spec]).stdout(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// Existing files that differ from `rev` (tracked changes plus untracked files), absolute, sorted.
pub fn changed_files(root: &Path, rev: &str) -> io::Result<Vec<PathBuf>> {
    if is_option(rev) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("invalid revision: {rev}")));
    }
    let mut files = Vec::new();
    for args in [vec!["diff", "--name-only", "-z", rev, "--"], vec!["ls-files", "--others", "--exclude-standard", "-z"]] {
        let out = git(root).args(&args).output()?;
        if !out.status.success() {
            return Err(io::Error::other(format!("git {} failed", args[0])));
        }
        for name in out.stdout.split(|&b| b == 0).filter(|n| !n.is_empty()) {
            let path = root.join(String::from_utf8_lossy(name).as_ref());
            if path.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

#[cfg(test)]
pub(crate) mod testrepo {
    use super::*;

    /// A temp repo with one commit containing `a.txt` ("one\ntwo\n").
    pub fn make() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let ok = Command::new("git").arg("-C").arg(dir.path()).args(args).output().unwrap().status.success();
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        run(&["add", "a.txt"]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_show_and_changes() {
        let repo = testrepo::make();
        let root = repo_root(repo.path()).unwrap();
        assert_eq!(root.canonicalize().unwrap(), repo.path().canonicalize().unwrap());
        assert_eq!(show_file(&root, "HEAD", Path::new("a.txt")).unwrap().as_deref(), Some("one\ntwo\n"));
        assert_eq!(show_file(&root, "HEAD", Path::new("missing.txt")).unwrap(), None);

        std::fs::write(repo.path().join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(repo.path().join("new.txt"), "x\n").unwrap();
        let changed: Vec<String> = changed_files(&root, "HEAD")
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(changed, vec!["a.txt", "new.txt"]);
    }

    #[test]
    fn recognizes_commits() {
        let repo = testrepo::make();
        assert!(is_commit(repo.path(), "HEAD"));
        assert!(is_commit(repo.path(), "main"));
        assert!(!is_commit(repo.path(), "no-such-branch"));
    }

    #[test]
    fn option_like_revisions_are_refused() {
        let repo = testrepo::make();
        let out = repo.path().join("out");
        let rev = format!("--output={}", out.display());
        assert!(changed_files(repo.path(), &rev).is_err());
        assert!(!is_commit(repo.path(), &rev));
        assert_eq!(show_file(repo.path(), &rev, Path::new("a.txt")).unwrap(), None);
        assert!(!out.exists());
    }

    #[test]
    fn outside_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(repo_root(dir.path()), None);
    }
}
