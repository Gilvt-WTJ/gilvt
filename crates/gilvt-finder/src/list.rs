//! The files a search runs over: git's view of a repository, or a directory walk elsewhere.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::{git, Root};

/// Up to this many files are listed outside git repositories.
pub const MAX_FILES: usize = 100_000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// Paths relative to the root, '/'-separated, sorted, unique.
    pub files: Vec<String>,
    /// Subset of `files` with uncommitted changes (git status), for ranking + the dot marker.
    pub changed: HashSet<String>,
    /// True when the walk stopped at MAX_FILES.
    pub truncated: bool,
    /// True when the root is the home directory or `/`, which are not listed (see `not_listed`).
    pub skipped: bool,
}

/// Blocking. In a repository: indexed files still in the working tree plus untracked files that are
/// not ignored, `changed` from `git status`; falls back to the walk when git fails. Elsewhere: a walk
/// with ripgrep's defaults (honors `.gitignore` / `.ignore`, skips hidden entries, does not follow
/// symlinks, stays on the root's file system, skips `~/Library`), stopping at MAX_FILES; unreadable
/// directories are skipped. Names that are not UTF-8 are left out. The home directory and `/` are
/// not listed at all, repository or not.
pub fn list(root: &Root) -> Listing {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if not_listed(&root.dir, home.as_deref()) {
        return Listing { skipped: true, ..Listing::default() };
    }
    if root.is_repo {
        if let Some(listing) = git_listing(&root.dir) {
            return listing;
        }
    }
    let library = home.map(|h| h.join("Library"));
    let (mut files, truncated) = walk(&root.dir, MAX_FILES, library.as_deref());
    files.sort_unstable();
    Listing { files, changed: HashSet::new(), truncated, skipped: false }
}

/// Whether `dir` is too broad to list: the home directory (usually not a repository; walking it
/// reaches Desktop / Documents / Downloads, which raise macOS privacy prompts) or `/` (network
/// mounts under /Volumes can hang a walk).
fn not_listed(dir: &Path, home: Option<&Path>) -> bool {
    dir == Path::new("/") || home.is_some_and(|h| dir == h)
}

fn git_listing(dir: &Path) -> Option<Listing> {
    // The three git commands are independent; run them side by side (together ~2x faster).
    let (indexed, untracked, status) = std::thread::scope(|s| {
        let untracked = s.spawn(|| git::untracked(dir));
        let status = s.spawn(|| git::status(dir));
        (git::indexed(dir), untracked.join().ok().flatten(), status.join().ok().flatten())
    });
    let untracked = untracked?;
    let mut files = indexed?;
    // Without status the file list is still right; only the change markers are lost.
    let status = status.unwrap_or_default();
    let deleted: HashSet<&str> = status.deleted.iter().map(String::as_str).collect();
    files.retain(|f| !deleted.contains(f.as_str()));
    files.extend(untracked.iter().cloned());
    files.sort_unstable();
    // Unmerged files appear once per conflict stage.
    files.dedup();
    let changed = status.changed.into_iter().filter(|c| files.binary_search(c).is_ok()).chain(untracked).collect();
    Some(Listing { files, changed, truncated: false, skipped: false })
}

/// Files under `dir` in walk order (sorted by name per directory, so truncation is deterministic),
/// at most `limit`, without crossing into other file systems or into `skip`; true when more were left.
fn walk(dir: &Path, limit: usize, skip: Option<&Path>) -> (Vec<String>, bool) {
    let skip = skip.map(Path::to_path_buf);
    let walker = WalkBuilder::new(dir)
        .require_git(false)
        .same_file_system(true)
        .sort_by_file_name(Ord::cmp)
        .filter_entry(move |e| skip.as_deref() != Some(e.path()))
        .build();
    let mut files = Vec::new();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(rel) = entry.path().strip_prefix(dir).ok().and_then(Path::to_str) else { continue };
        if files.len() == limit {
            return (files, true);
        }
        files.push(rel.to_owned());
    }
    (files, false)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    use super::*;
    use crate::testutil::{self, git, write};

    fn set(paths: &[&str]) -> HashSet<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn repository_listing() {
        let repo = testutil::repo();
        let dir = repo.path();
        write(dir, ".gitignore", "build/\n*.log\n");
        for f in ["a.txt", "gone.txt", "old.rs", "has space.txt", "中文.md", "sub/dir/deep.rs"] {
            write(dir, f, "x\n");
        }
        // Committed symlinks are listed, to a file or not (the palette tells them apart on ⏎).
        std::os::unix::fs::symlink("a.txt", dir.join("link.txt")).unwrap();
        std::os::unix::fs::symlink("sub", dir.join("sublink")).unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "init"]);
        // A submodule entry (gitlink) is a directory, not a file.
        let head = Command::new("git").arg("-C").arg(dir).args(["rev-parse", "HEAD"]).output().unwrap().stdout;
        let gitlink = format!("160000,{},vendor/lib", String::from_utf8(head).unwrap().trim());
        git(dir, &["update-index", "--add", "--cacheinfo", &gitlink]);

        write(dir, "a.txt", "changed\n");
        write(dir, "new.txt", "x\n");
        write(dir, "newdir/x.rs", "x\n");
        write(dir, "build/out.o", "x\n");
        write(dir, "debug.log", "x\n");
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        git(dir, &["mv", "old.rs", "renamed.rs"]);

        let listing = list(&Root { dir: dir.to_path_buf(), is_repo: true });
        let expected = [
            ".gitignore",
            "a.txt",
            "has space.txt",
            "link.txt",
            "new.txt",
            "newdir/x.rs",
            "renamed.rs",
            "sub/dir/deep.rs",
            "sublink",
            "中文.md",
        ];
        assert_eq!(listing.files, expected);
        assert_eq!(listing.changed, set(&["a.txt", "new.txt", "newdir/x.rs", "renamed.rs"]));
        assert!(!listing.skipped);
        assert!(!listing.truncated);
    }

    #[test]
    fn falls_back_to_the_walk_when_git_fails() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", "x\n");
        let listing = list(&Root { dir: dir.path().to_path_buf(), is_repo: true });
        assert_eq!(listing, Listing { files: vec!["a.txt".into()], ..Listing::default() });
    }

    #[test]
    fn walk_honors_gitignore_and_skips_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        write(d, ".gitignore", "ignored/\n*.tmp\n");
        for f in ["a.txt", "x.tmp", "ignored/y.txt", ".hidden/z.txt", ".dotfile", "sub/b.txt", "sub/中文 名.md"] {
            write(d, f, "x\n");
        }
        std::os::unix::fs::symlink(d.join("a.txt"), d.join("link.txt")).unwrap();

        let listing = list(&Root { dir: d.to_path_buf(), is_repo: false });
        assert_eq!(listing.files, ["a.txt", "sub/b.txt", "sub/中文 名.md"]);
        assert!(listing.changed.is_empty());
        assert!(!listing.truncated);
    }

    #[test]
    fn walk_skips_unreadable_directories() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        write(d, "a.txt", "x\n");
        write(d, "locked/secret.txt", "x\n");
        let locked = d.join("locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let (files, truncated) = walk(d, MAX_FILES, None);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(files, ["a.txt"]);
        assert!(!truncated);
    }

    #[test]
    fn home_and_root_are_not_listed() {
        let home = Path::new("/Users/me");
        assert!(not_listed(Path::new("/"), Some(home)));
        assert!(not_listed(Path::new("/Users/me"), Some(home)));
        assert!(not_listed(Path::new("/Users/me/"), Some(home)));
        assert!(!not_listed(Path::new("/Users/me/code"), Some(home)));
        assert!(!not_listed(Path::new("/Users"), Some(home)));
        assert!(!not_listed(Path::new("/Users/me"), None));
        let listing = list(&Root { dir: PathBuf::from("/"), is_repo: false });
        assert_eq!(listing, Listing { skipped: true, ..Listing::default() });
    }

    #[test]
    fn walk_skips_the_given_directory() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for f in ["me/Library/prefs.plist", "me/code/a.rs", "other/Library/b.rs"] {
            write(d, f, "x\n");
        }
        let (files, _) = walk(d, MAX_FILES, Some(&d.join("me/Library")));
        assert_eq!(files, ["me/code/a.rs", "other/Library/b.rs"]);
    }

    #[test]
    fn walk_stops_at_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["a", "b/c", "d"] {
            write(dir.path(), f, "x\n");
        }
        assert_eq!(walk(dir.path(), 2, None), (vec!["a".to_string(), "b/c".to_string()], true));
        assert_eq!(walk(dir.path(), 3, None), (vec!["a".to_string(), "b/c".to_string(), "d".to_string()], false));
    }
}
