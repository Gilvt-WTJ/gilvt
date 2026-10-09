//! Turn snapshots: a tree of the whole working directory, written into gilvt's own object directory so the
//! user's repository is only ever read (never its index, refs or object database). Uses the system `git`.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

pub mod ledger;

/// Files larger than this are left out of a snapshot (reported in [`Snapshot::skipped_large`]).
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// How long a repository's snapshot store is kept after its last snapshot.
pub const KEEP: Duration = Duration::from_secs(30 * 24 * 3600);

/// A git tree object id (40 or 64 hex digits).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TreeId(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeStatus {
    M,
    A,
    D,
    R,
}

/// One file that differs between two trees; paths are relative to the repository root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    /// The path before a rename.
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    pub added: u32,
    pub removed: u32,
    pub binary: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub tree: TreeId,
    /// Files over [`MAX_FILE_BYTES`] that were left out (relative paths).
    pub skipped_large: Vec<String>,
    pub took: Duration,
}

#[derive(Debug)]
pub enum SnapshotError {
    /// The directory is not inside a git repository.
    NotARepo,
    /// The tree is not in the store any more (pruned) or never was.
    Missing,
    Git(String),
    Io(io::Error),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SnapshotError::NotARepo => f.write_str(gilvt_i18n::text("不是 git 仓库", "Not a git repository")),
            SnapshotError::Missing => f.write_str(gilvt_i18n::text("快照已清理", "Snapshot was cleaned up")),
            SnapshotError::Git(m) => write!(f, "git: {m}"),
            SnapshotError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

impl From<io::Error> for SnapshotError {
    fn from(e: io::Error) -> Self {
        SnapshotError::Io(e)
    }
}

/// The snapshot store of one repository: `<state>/snapshots/<hash of the repo root>/objects`, reading the
/// repository's own objects through `GIT_ALTERNATE_OBJECT_DIRECTORIES`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectStore {
    repo_root: PathBuf,
    repo_objects: PathBuf,
    repo_index: PathBuf,
    dir: PathBuf,
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn run(mut c: Command) -> Result<Vec<u8>, SnapshotError> {
    let out = c.output()?;
    if !out.status.success() {
        return Err(SnapshotError::Git(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    Ok(out.stdout)
}

/// What a failed `git rev-parse --show-toplevel` means: `NotARepo` only when git itself says so.
fn classify_rev_parse_failure(stderr: &str) -> SnapshotError {
    if stderr.to_lowercase().contains("not a git repository") {
        SnapshotError::NotARepo
    } else {
        SnapshotError::Git(stderr.trim().to_string())
    }
}

fn plain_git(root: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(root).stdin(Stdio::null());
    c
}

fn locks() -> &'static Mutex<HashMap<PathBuf, Arc<Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(Default::default)
}

impl ObjectStore {
    /// The store for the repository containing `dir`; `NotARepo` outside one.
    pub fn open(dir: &Path, state_dir: &Path) -> Result<ObjectStore, SnapshotError> {
        let top = run({
            let mut c = plain_git(dir);
            c.args(["rev-parse", "--show-toplevel"]);
            c
        })
        .map_err(|e| match e {
            SnapshotError::Git(stderr) => classify_rev_parse_failure(&stderr),
            other => other,
        })?;
        let repo_root = PathBuf::from(String::from_utf8_lossy(&top).trim_end());
        let git_path = |what: &str| -> Result<PathBuf, SnapshotError> {
            let out = run({
                let mut c = plain_git(&repo_root);
                c.args(["rev-parse", "--git-path", what]);
                c
            })?;
            let p = PathBuf::from(String::from_utf8_lossy(&out).trim_end());
            Ok(if p.is_absolute() { p } else { repo_root.join(p) })
        };
        let repo_objects = git_path("objects")?;
        let repo_index = git_path("index")?;
        let canon = repo_root.canonicalize().unwrap_or_else(|_| repo_root.clone());
        let dir = state_dir.join("snapshots").join(format!("{:016x}", fnv1a(&canon.to_string_lossy())));
        Ok(ObjectStore { repo_root, repo_objects, repo_index, dir })
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// `git` run against the snapshot store (objects written here, repository objects readable).
    fn git(&self) -> Command {
        let mut c = plain_git(&self.repo_root);
        // A copied split index would otherwise write `sharedindex.*` files into the user's `$GIT_DIR`.
        c.args(["-c", "core.splitIndex=false"]);
        c.env("GIT_OBJECT_DIRECTORY", self.dir.join("objects")).env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &self.repo_objects);
        c
    }

    /// A tree of the working directory as it is now (tracked + untracked files, honoring `.gitignore`).
    /// Never touches the repository's index, refs or objects: a copy of its index is the starting point,
    /// so unchanged files are not hashed again.
    pub fn take(&self) -> Result<Snapshot, SnapshotError> {
        let started = std::time::Instant::now();
        let lock = locks().lock().unwrap().entry(self.dir.clone()).or_default().clone();
        let _guard = lock.lock().unwrap();
        fs::create_dir_all(self.dir.join("objects"))?;
        let idx = self.dir.join(format!("index.{}", std::process::id()));
        if fs::copy(&self.repo_index, &idx).is_err() {
            let _ = fs::remove_file(&idx);
        }
        let result = self.take_with(&idx);
        let _ = fs::remove_file(&idx);
        fs::write(self.dir.join("stamp"), b"")?;
        result.map(|(tree, skipped_large)| Snapshot { tree, skipped_large, took: started.elapsed() })
    }

    fn take_with(&self, idx: &Path) -> Result<(TreeId, Vec<String>), SnapshotError> {
        let git = || {
            let mut c = self.git();
            c.env("GIT_INDEX_FILE", idx);
            c
        };
        // Files that would be hashed anew: modified tracked ones and untracked ones.
        let listed = run({
            let mut c = git();
            c.args(["ls-files", "-m", "-o", "--exclude-standard", "-z"]);
            c
        })?;
        let mut skipped_large = Vec::new();
        for name in listed.split(|&b| b == 0).filter(|n| !n.is_empty()) {
            let rel = String::from_utf8_lossy(name).into_owned();
            if fs::metadata(self.repo_root.join(&rel)).is_ok_and(|m| m.is_file() && m.len() > MAX_FILE_BYTES) {
                skipped_large.push(rel);
            }
        }
        skipped_large.sort();
        skipped_large.dedup();
        // Untracked directories that are repositories of their own: `git add` refuses them while they have no
        // commit, and a snapshot of their insides is not this repository's business.
        let nested: Vec<String> = listed
            .split(|&b| b == 0)
            .filter(|n| n.ends_with(b"/"))
            .map(|n| String::from_utf8_lossy(n).trim_end_matches('/').to_string())
            .filter(|rel| self.repo_root.join(rel).join(".git").exists())
            .collect();
        let mut add = git();
        add.args(["add", "-A", "--", "."]);
        for rel in skipped_large.iter().chain(&nested) {
            add.arg(format!(":(exclude,literal){rel}"));
        }
        run(add)?;
        let tree = run({
            let mut c = git();
            c.arg("write-tree");
            c
        })?;
        Ok((TreeId(String::from_utf8_lossy(&tree).trim().to_string()), skipped_large))
    }

    /// Whether `tree` is in the store (or the repository).
    pub fn has_tree(&self, tree: &TreeId) -> bool {
        let mut c = self.git();
        c.args(["cat-file", "-e", &format!("{}^{{tree}}", tree.0)]);
        c.output().is_ok_and(|o| o.status.success())
    }

    /// The files that differ between two trees, sorted by path.
    pub fn diff_trees(&self, before: &TreeId, after: &TreeId) -> Result<Vec<FileChange>, SnapshotError> {
        if !self.has_tree(before) || !self.has_tree(after) {
            return Err(SnapshotError::Missing);
        }
        let names = run({
            let mut c = self.git();
            c.args(["diff-tree", "-r", "-M", "-z", "--name-status", &before.0, &after.0]);
            c
        })?;
        let nums = run({
            let mut c = self.git();
            c.args(["diff-tree", "-r", "-M", "-z", "--numstat", &before.0, &after.0]);
            c
        })?;
        let counts = parse_numstat(&nums);
        let mut out = parse_name_status(&names)
            .into_iter()
            .map(|(status, path, old_path)| {
                let (added, removed, binary) = counts.get(&path).copied().unwrap_or((0, 0, false));
                FileChange { path, old_path, status, added, removed, binary }
            })
            .collect::<Vec<_>>();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// The content of `path` in `tree`; `Ok(None)` when the file is not in it, `Missing` when the tree is gone.
    pub fn blob_at(&self, tree: &TreeId, path: &str) -> Result<Option<Vec<u8>>, SnapshotError> {
        if !self.has_tree(tree) {
            return Err(SnapshotError::Missing);
        }
        let mut c = self.git();
        c.args(["cat-file", "blob", &format!("{}:{path}", tree.0)]);
        let out = c.output()?;
        Ok(out.status.success().then_some(out.stdout))
    }
}

fn parse_name_status(raw: &[u8]) -> Vec<(ChangeStatus, String, Option<String>)> {
    let mut parts = raw.split(|&b| b == 0).filter(|p| !p.is_empty());
    let mut out = Vec::new();
    while let Some(code) = parts.next() {
        let code = String::from_utf8_lossy(code).into_owned();
        let take = |parts: &mut dyn Iterator<Item = &[u8]>| parts.next().map(|p| String::from_utf8_lossy(p).into_owned());
        match code.chars().next() {
            Some('R' | 'C') => {
                let (Some(old), Some(new)) = (take(&mut parts), take(&mut parts)) else { break };
                out.push((ChangeStatus::R, new, Some(old)));
            }
            Some(c @ ('M' | 'A' | 'D' | 'T')) => {
                let Some(path) = take(&mut parts) else { break };
                let status = match c {
                    'A' => ChangeStatus::A,
                    'D' => ChangeStatus::D,
                    _ => ChangeStatus::M,
                };
                out.push((status, path, None));
            }
            _ => {}
        }
    }
    out
}

/// `added\tremoved\tpath` records, or `added\tremoved\t` followed by the old and new path for a rename; `-`
/// counts mean a binary file.
fn parse_numstat(raw: &[u8]) -> BTreeMap<String, (u32, u32, bool)> {
    let mut parts = raw.split(|&b| b == 0);
    let mut out = BTreeMap::new();
    while let Some(head) = parts.next() {
        if head.is_empty() {
            continue;
        }
        let head = String::from_utf8_lossy(head).into_owned();
        let mut fields = head.splitn(3, '\t');
        let (Some(a), Some(r), Some(path)) = (fields.next(), fields.next(), fields.next()) else { continue };
        let binary = a == "-" && r == "-";
        let counts = (a.parse().unwrap_or(0), r.parse().unwrap_or(0), binary);
        if path.is_empty() {
            let (_old, new) = (parts.next(), parts.next());
            if let Some(new) = new {
                out.insert(String::from_utf8_lossy(new).into_owned(), counts);
            }
        } else {
            out.insert(path.to_string(), counts);
        }
    }
    out
}

/// Removes the stores of repositories that had no snapshot for `older_than`; returns how many.
pub fn prune(state_dir: &Path, older_than: Duration) -> io::Result<usize> {
    let root = state_dir.join("snapshots");
    let Ok(entries) = fs::read_dir(&root) else { return Ok(0) };
    let cutoff = SystemTime::now().checked_sub(older_than).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut removed = 0;
    for entry in entries.flatten() {
        let dir = entry.path();
        let stamp = fs::metadata(dir.join("stamp")).or_else(|_| fs::metadata(&dir)).and_then(|m| m.modified());
        if stamp.is_ok_and(|t| t < cutoff) {
            fs::remove_dir_all(&dir)?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    #[test]
    fn messages_follow_the_language() {
        use super::SnapshotError;
        use gilvt_i18n::{has_chinese, with_language, Language};
        let english = with_language(Language::English, || [SnapshotError::NotARepo, SnapshotError::Missing].map(|e| e.to_string()));
        assert_eq!(english, ["Not a git repository", "Snapshot was cleaned up"]);
        assert!(english.iter().all(|s| !has_chinese(s)));
        assert_eq!(SnapshotError::Missing.to_string(), "快照已清理");
    }

    #[test]
    fn only_gits_own_verdict_means_not_a_repo() {
        use super::*;
        assert!(matches!(
            classify_rev_parse_failure("fatal: not a git repository (or any of the parent directories): .git"),
            SnapshotError::NotARepo
        ));
        assert!(matches!(classify_rev_parse_failure("fatal: Not a Git Repository"), SnapshotError::NotARepo));
        assert!(matches!(classify_rev_parse_failure("fatal: detected dubious ownership"), SnapshotError::Git(_)));
        assert!(matches!(classify_rev_parse_failure("error: cannot open /x: Permission denied"), SnapshotError::Git(_)));
    }

    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap().status.success();
        assert!(ok, "git {args:?}");
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "t@example.com"]);
        git(dir.path(), &["config", "user.name", "t"]);
        fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
        fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        fs::write(dir.path().join("keep.txt"), "keep\nkeep\nkeep\nkeep\n").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        dir
    }

    fn files_under(root: &Path) -> Vec<String> {
        fn walk(dir: &Path, out: &mut Vec<String>, root: &Path) {
            for e in fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out, root);
                } else {
                    out.push(p.strip_prefix(root).unwrap().display().to_string());
                }
            }
        }
        let mut out = Vec::new();
        walk(root, &mut out, root);
        out.sort();
        out
    }

    #[test]
    fn take_captures_the_working_tree() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        fs::write(r.path().join("a.txt"), "one\nTWO\n").unwrap();
        fs::write(r.path().join("new.txt"), "n\n").unwrap();
        fs::write(r.path().join("ignored.log"), "x\n").unwrap();
        let store = ObjectStore::open(r.path(), state.path()).unwrap();
        let snap = store.take().unwrap();
        assert_eq!(store.blob_at(&snap.tree, "a.txt").unwrap().unwrap(), b"one\nTWO\n");
        assert_eq!(store.blob_at(&snap.tree, "new.txt").unwrap().unwrap(), b"n\n");
        assert_eq!(store.blob_at(&snap.tree, "keep.txt").unwrap().unwrap(), b"keep\nkeep\nkeep\nkeep\n");
        assert_eq!(store.blob_at(&snap.tree, "ignored.log").unwrap(), None, ".gitignore is honored");
        assert!(snap.skipped_large.is_empty());
    }

    #[test]
    fn same_state_same_tree() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        let store = ObjectStore::open(r.path(), state.path()).unwrap();
        assert_eq!(store.take().unwrap().tree, store.take().unwrap().tree);
    }

    #[test]
    fn diff_has_statuses_counts_and_renames() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        let store = ObjectStore::open(r.path(), state.path()).unwrap();
        let before = store.take().unwrap().tree;
        fs::write(r.path().join("a.txt"), "one\nTWO\nthree\n").unwrap();
        fs::write(r.path().join("new.txt"), "n1\nn2\n").unwrap();
        fs::rename(r.path().join("keep.txt"), r.path().join("moved.txt")).unwrap();
        fs::write(r.path().join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
        let after = store.take().unwrap().tree;
        let changes = store.diff_trees(&before, &after).unwrap();
        let by = |p: &str| changes.iter().find(|c| c.path == p).unwrap_or_else(|| panic!("{p} in {changes:?}")).clone();
        let a = by("a.txt");
        assert_eq!((a.status, a.added, a.removed), (ChangeStatus::M, 2, 1));
        let n = by("new.txt");
        assert_eq!((n.status, n.added, n.removed), (ChangeStatus::A, 2, 0));
        let m = by("moved.txt");
        assert_eq!((m.status, m.old_path.as_deref()), (ChangeStatus::R, Some("keep.txt")));
        assert!(by("bin.dat").binary);
        assert_eq!(changes.len(), 4);
        fs::remove_file(r.path().join("new.txt")).unwrap();
        let gone = store.take().unwrap().tree;
        let d = store.diff_trees(&after, &gone).unwrap();
        assert_eq!((d[0].path.as_str(), d[0].status), ("new.txt", ChangeStatus::D));
    }

    #[test]
    fn the_users_repository_is_not_touched() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        fs::write(r.path().join("a.txt"), "changed\n").unwrap();
        fs::write(r.path().join("untracked.txt"), "u\n").unwrap();
        let dot_git = r.path().join(".git");
        let status = |_: ()| String::from_utf8(plain_git(r.path()).args(["status", "--porcelain"]).output().unwrap().stdout).unwrap();
        // `git status` refreshes the index's stat cache itself, so it runs before the bytes are read.
        let status_before = status(());
        let (files_before, index_before) = (files_under(&dot_git), fs::read(dot_git.join("index")).unwrap());
        let store = ObjectStore::open(r.path(), state.path()).unwrap();
        store.take().unwrap();
        store.take().unwrap();
        assert_eq!(files_under(&dot_git), files_before, "no file added to or removed from .git");
        assert_eq!(fs::read(dot_git.join("index")).unwrap(), index_before, "the index is byte-identical");
        assert_eq!(status(()), status_before);
    }

    #[test]
    fn a_split_index_repository_gets_no_new_files() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        git(r.path(), &["update-index", "--split-index"]);
        fs::write(r.path().join("untracked.txt"), "u\n").unwrap();
        let dot_git = r.path().join(".git");
        let files_before = files_under(&dot_git);
        let store = ObjectStore::open(r.path(), state.path()).unwrap();
        store.take().unwrap();
        assert_eq!(files_under(&dot_git), files_before, "no sharedindex.* or other file appears in .git");
    }

    #[test]
    fn large_files_are_left_out() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        fs::write(r.path().join("big.bin"), vec![b'x'; (MAX_FILE_BYTES + 1) as usize]).unwrap();
        fs::write(r.path().join("small.txt"), "s\n").unwrap();
        let store = ObjectStore::open(r.path(), state.path()).unwrap();
        let snap = store.take().unwrap();
        assert_eq!(snap.skipped_large, vec!["big.bin".to_string()]);
        assert_eq!(store.blob_at(&snap.tree, "big.bin").unwrap(), None);
        assert!(store.blob_at(&snap.tree, "small.txt").unwrap().is_some());
    }

    #[test]
    fn outside_a_repository() {
        let (dir, state) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        assert!(matches!(ObjectStore::open(dir.path(), state.path()), Err(SnapshotError::NotARepo)));
    }

    #[test]
    fn prune_removes_only_stale_stores() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        fs::write(r.path().join("a.txt"), "only in the store\n").unwrap();
        let store = ObjectStore::open(r.path(), state.path()).unwrap();
        let tree = store.take().unwrap().tree;
        assert_eq!(prune(state.path(), KEEP).unwrap(), 0);
        assert!(store.has_tree(&tree));
        assert_eq!(prune(state.path(), Duration::ZERO).unwrap(), 1);
        assert!(!store.has_tree(&tree));
        assert!(matches!(store.blob_at(&tree, "a.txt"), Err(SnapshotError::Missing)));
    }

    #[test]
    fn a_repository_without_commits_works() {
        let (dir, state) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        git(dir.path(), &["init", "-q", "-b", "main"]);
        fs::write(dir.path().join("first.txt"), "hello\n").unwrap();
        let store = ObjectStore::open(dir.path(), state.path()).unwrap();
        let snap = store.take().unwrap();
        assert_eq!(store.blob_at(&snap.tree, "first.txt").unwrap().unwrap(), b"hello\n");
        let empty = tempfile::tempdir().unwrap();
        git(empty.path(), &["init", "-q", "-b", "main"]);
        let nothing = ObjectStore::open(empty.path(), state.path()).unwrap().take().unwrap();
        assert!(!nothing.tree.0.is_empty(), "an empty working tree is the empty tree");
    }

    #[test]
    fn awkward_file_names_survive() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        let store = ObjectStore::open(r.path(), state.path()).unwrap();
        let before = store.take().unwrap().tree;
        let names = ["with space.txt", "中文文件.md", "tab\there.txt", "new\nline.txt", "quote\"s.txt"];
        for n in names {
            fs::write(r.path().join(n), "x\n").unwrap();
        }
        let after = store.take().unwrap().tree;
        let changes = store.diff_trees(&before, &after).unwrap();
        let mut got: Vec<String> = changes.iter().map(|c| c.path.clone()).collect();
        got.sort();
        let mut want: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        want.sort();
        assert_eq!(got, want);
        for n in names {
            assert_eq!(store.blob_at(&after, n).unwrap().unwrap(), b"x\n", "{n}");
        }
    }

    #[test]
    fn a_nested_repository_does_not_fail_the_snapshot() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        let inner = r.path().join("vendor/inner");
        fs::create_dir_all(&inner).unwrap();
        git(&inner, &["init", "-q", "-b", "main"]);
        fs::write(inner.join("f.txt"), "f\n").unwrap();
        let store = ObjectStore::open(r.path(), state.path()).unwrap();
        let snap = store.take().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(store.blob_at(&snap.tree, "a.txt").unwrap().unwrap(), b"one\ntwo\n");
    }

    #[test]
    fn concurrent_snapshots_of_one_repository_do_not_collide() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        fs::write(r.path().join("a.txt"), "changing\n").unwrap();
        let store = ObjectStore::open(r.path(), state.path()).unwrap();
        let handles: Vec<_> = (0..6)
            .map(|_| {
                let s = store.clone();
                std::thread::spawn(move || s.take().map(|t| t.tree))
            })
            .collect();
        let trees: Vec<_> = handles.into_iter().map(|h| h.join().unwrap().unwrap()).collect();
        assert!(trees.windows(2).all(|w| w[0] == w[1]));
    }

    #[test]
    fn works_from_a_linked_worktree() {
        let (r, state) = (repo(), tempfile::tempdir().unwrap());
        let wt = tempfile::tempdir().unwrap();
        let wt_path = wt.path().join("wt");
        git(r.path(), &["worktree", "add", "-q", "-b", "side", wt_path.to_str().unwrap()]);
        fs::write(wt_path.join("w.txt"), "w\n").unwrap();
        let store = ObjectStore::open(&wt_path, state.path()).unwrap();
        let snap = store.take().unwrap();
        assert!(store.blob_at(&snap.tree, "w.txt").unwrap().is_some());
        assert!(store.blob_at(&snap.tree, "a.txt").unwrap().is_some());
    }
}
