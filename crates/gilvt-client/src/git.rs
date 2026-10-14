//! The few git queries gilvt needs, via the system `git`: the Mac's viewer and CLI and the remote daemon (R2a §8.1 `GitChanges`) share them.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use gilvt_ipc::remote::{ChangeEntry, ChangeStatus};

fn git(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(dir).stdin(Stdio::null()).stderr(Stdio::null());
    c
}

/// Whether `git` can run here. On macOS without the Command Line Tools `/usr/bin/git` is only a stub that opens an
/// install dialog on that Mac's screen, so it counts as no git (spec macOS §4.4). Checked once per process.
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let found = std::env::var_os("PATH").and_then(|p| {
            std::env::split_paths(&p).map(|d| d.join("git")).find(|g| g.is_file())
        });
        usable(found.as_deref(), cfg!(target_os = "macos"), || {
            Command::new("xcode-select").arg("-p").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
        })
    })
}

fn usable(git: Option<&Path>, macos: bool, clt_installed: impl FnOnce() -> bool) -> bool {
    match git {
        None => false,
        Some(g) if macos && g == Path::new("/usr/bin/git") => clt_installed(),
        Some(_) => true,
    }
}

/// Top-level directory of the repository containing `dir` (a directory or a file's parent).
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    if !available() {
        return None;
    }
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

/// Existing files that differ from `rev`: tracked changes, then untracked files; absolute, sorted, without duplicates.
/// Deleted files are left out (they cannot be shown).
pub fn changed_entries(root: &Path, rev: &str) -> io::Result<Vec<ChangeEntry>> {
    if is_option(rev) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("invalid revision: {rev}")));
    }
    let mut files: Vec<ChangeEntry> = Vec::new();
    let runs = [(ChangeStatus::Modified, vec!["diff", "--name-only", "-z", rev, "--"]), (ChangeStatus::Untracked, vec!["ls-files", "--others", "--exclude-standard", "-z"])];
    for (status, args) in runs {
        let out = git(root).args(&args).output()?;
        if !out.status.success() {
            return Err(io::Error::other(format!("git {} failed", args[0])));
        }
        for name in out.stdout.split(|&b| b == 0).filter(|n| !n.is_empty()) {
            let path = root.join(String::from_utf8_lossy(name).as_ref());
            if path.is_file() {
                files.push(ChangeEntry { status, path });
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);
    Ok(files)
}

/// Existing files that differ from `rev` (tracked changes plus untracked files), absolute, sorted.
pub fn changed_files(root: &Path, rev: &str) -> io::Result<Vec<PathBuf>> {
    Ok(changed_entries(root, rev)?.into_iter().map(|e| e.path).collect())
}

/// The repository containing `path` and the file's contents at `rev` (None: not there, a new file). None outside a
/// repository. `git` reports paths relative to the canonical root (e.g. /private/tmp on macOS), so both are
/// canonicalized before taking the relative path.
pub fn preview_base(path: &Path, rev: &str) -> io::Result<Option<(PathBuf, Option<String>)>> {
    let Some(root) = path.parent().and_then(repo_root) else { return Ok(None) };
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.clone());
    let canon_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let rel = canon_path.strip_prefix(&canon_root).unwrap_or(path).to_path_buf();
    let old = show_file(&canon_root, rev, &rel)?;
    Ok(Some((root, old)))
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
    fn the_command_line_tools_stub_is_no_git() {
        let stub = Path::new("/usr/bin/git");
        let brew = Path::new("/opt/homebrew/bin/git");
        // macOS: /usr/bin/git without the Command Line Tools would pop up an install dialog on that Mac's screen.
        assert!(!usable(None, true, || true), "no git on PATH");
        assert!(!usable(Some(stub), true, || false));
        assert!(usable(Some(stub), true, || true), "CLT installed");
        assert!(usable(Some(brew), true, || panic!("not asked for another git")));
        // Elsewhere /usr/bin/git is a real git.
        assert!(usable(Some(stub), false, || panic!("not asked on Linux")));
    }

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

    #[test]
    fn entries_carry_a_status_and_agree_with_changed_files() {
        let repo = testrepo::make();
        let root = repo_root(repo.path()).unwrap();
        std::fs::write(repo.path().join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(repo.path().join("new.txt"), "x\n").unwrap();
        let entries = changed_entries(&root, "HEAD").unwrap();
        let named: Vec<(ChangeStatus, String)> = entries.iter().map(|e| (e.status, e.path.file_name().unwrap().to_string_lossy().into_owned())).collect();
        assert_eq!(named, vec![(ChangeStatus::Modified, "a.txt".into()), (ChangeStatus::Untracked, "new.txt".into())]);
        let paths: Vec<PathBuf> = entries.into_iter().map(|e| e.path).collect();
        assert_eq!(paths, changed_files(&root, "HEAD").unwrap());
    }

    #[test]
    fn a_deleted_file_is_not_listed() {
        let repo = testrepo::make();
        std::fs::remove_file(repo.path().join("a.txt")).unwrap();
        assert!(changed_entries(&repo_root(repo.path()).unwrap(), "HEAD").unwrap().is_empty());
    }

    #[test]
    fn preview_base_finds_the_root_and_the_old_text() {
        let repo = testrepo::make();
        let file = repo.path().join("a.txt");
        std::fs::write(&file, "changed\n").unwrap();
        let (root, base) = preview_base(&file, "HEAD").unwrap().unwrap();
        assert_eq!(root.canonicalize().unwrap(), repo.path().canonicalize().unwrap());
        assert_eq!(base.as_deref(), Some("one\ntwo\n"));
        std::fs::write(repo.path().join("n.txt"), "x\n").unwrap();
        assert_eq!(preview_base(&repo.path().join("n.txt"), "HEAD").unwrap().unwrap().1, None, "new at HEAD");
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("x"), "x").unwrap();
        assert_eq!(preview_base(&outside.path().join("x"), "HEAD").unwrap(), None);
    }
}
