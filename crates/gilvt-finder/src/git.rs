//! The git queries behind a listing, via the system `git` (same behavior as the user's CLI).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn git(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(dir).stdin(Stdio::null()).stderr(Stdio::null());
    c
}

/// Stdout of a successful `git -C dir <args>`.
fn run(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = git(dir).args(args).output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// NUL-terminated records of `-z` output.
fn records(out: &[u8]) -> impl Iterator<Item = &[u8]> {
    out.split(|&b| b == 0).filter(|r| !r.is_empty())
}

/// Top-level directory of the repository containing `dir`.
pub(crate) fn toplevel(dir: &Path) -> Option<PathBuf> {
    let out = run(dir, &["rev-parse", "--show-toplevel"])?;
    let top = String::from_utf8(out).ok()?;
    Some(PathBuf::from(top.trim_end()))
}

/// Indexed files, submodules excluded, relative to `root`, unsorted. Names that are not UTF-8 are skipped.
pub(crate) fn indexed(root: &Path) -> Option<Vec<String>> {
    // `--stage` for the file mode: a submodule is a gitlink (160000), a directory rather than a file.
    // Symlinks (120000) stay: `CLAUDE.md -> AGENTS.md` is common, and untracked ones are listed too.
    let out = run(root, &["ls-files", "-z", "--stage"])?;
    let mut files = Vec::new();
    for record in records(&out) {
        // "<mode> <object> <stage>\t<path>"
        let Some(tab) = record.iter().position(|&b| b == b'\t') else { continue };
        if record.starts_with(b"160000 ") {
            continue;
        }
        files.extend(utf8(&record[tab + 1..]));
    }
    Some(files)
}

/// Untracked files that are not ignored, relative to `root`.
pub(crate) fn untracked(root: &Path) -> Option<Vec<String>> {
    let out = run(root, &["ls-files", "-z", "--others", "--exclude-standard"])?;
    // An untracked nested repository is listed as its directory ("dir/").
    Some(records(&out).filter(|r| !r.ends_with(b"/")).filter_map(utf8).collect())
}

/// Paths `git status` reports, relative to `root`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Status {
    /// Staged or modified.
    pub changed: Vec<String>,
    /// Still in the index but deleted from the working tree.
    pub deleted: Vec<String>,
}

pub(crate) fn status(root: &Path) -> Option<Status> {
    // No optional locks: a background refresh must not make the user's own git commands fail on index.lock.
    // No untracked scan: `untracked` lists those already, and the scan is most of status's time.
    let out = run(root, &["--no-optional-locks", "status", "--porcelain", "-z", "--untracked-files=no"])?;
    Some(parse_status(&out))
}

/// Parses `git status --porcelain -z`: "XY path" records; renames and copies are followed by a record
/// holding the original path.
fn parse_status(out: &[u8]) -> Status {
    let mut status = Status::default();
    let mut records = records(out);
    while let Some(record) = records.next() {
        let [x, y, b' ', path @ ..] = record else { continue };
        if matches!(*x, b'R' | b'C') {
            records.next();
        }
        let Some(path) = utf8(path) else { continue };
        if *y == b'D' {
            status.deleted.push(path);
        } else {
            status.changed.push(path);
        }
    }
    status
}

fn utf8(bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes).ok().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_records() {
        let out = b" M a.txt\0R  new.rs\0old.rs\0?? dir/x.rs\0 D gone.txt\0AD added-then-gone\0M  staged.txt\0";
        let status = parse_status(out);
        assert_eq!(status.changed, ["a.txt", "new.rs", "dir/x.rs", "staged.txt"]);
        assert_eq!(status.deleted, ["gone.txt", "added-then-gone"]);
    }
}
