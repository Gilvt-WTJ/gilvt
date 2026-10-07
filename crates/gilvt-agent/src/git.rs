//! Local git facts about a session's directory, for the sidebar (P0 spec §4). Everything shells out to the
//! system `git` with a timeout; any failure is `None`, never an error the UI has to show.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Budget for the read-only queries behind the sidebar.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitInfo {
    pub repo_root: PathBuf,
    pub common_dir: PathBuf,
    pub branch: Option<String>,
    pub detached_short: Option<String>,
    pub dirty_count: usize,
    pub ahead: u32,
    pub behind: u32,
    pub is_linked_worktree: bool,
}

impl GitInfo {
    /// The main repository's directory name, shared by all its worktrees (`common_dir` is `<main>/.git`).
    pub fn main_repo_name(&self) -> String {
        main_name_of(&self.common_dir)
    }

    /// The main checkout's directory (`<main>/.git` → `<main>`); a bare repository's common dir is itself.
    /// The same for every worktree of one repository.
    pub fn main_repo_root(&self) -> PathBuf {
        main_root_of(&self.common_dir)
    }
}

/// `<main>/.git` → `<main>`; a bare repository's common dir is itself.
fn main_root_of(common_dir: &Path) -> PathBuf {
    if common_dir.file_name().is_some_and(|n| n == ".git") {
        if let Some(parent) = common_dir.parent() {
            return parent.to_path_buf();
        }
    }
    common_dir.to_path_buf()
}

fn main_name_of(common_dir: &Path) -> String {
    main_root_of(common_dir).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| common_dir.display().to_string())
}

/// The main repository's root and directory name for the repository holding `cwd`: one cheap `rev-parse`
/// (no `status`), so it suits resolving many directories, e.g. the projects of past sessions. Every worktree
/// of one repository gives the same answer. None outside a repository, without git, or on any failure.
pub fn main_repo_of(cwd: &Path) -> Option<(PathBuf, String)> {
    let out = run_git_with(cwd, &["rev-parse", "--path-format=absolute", "--git-common-dir"], QUERY_TIMEOUT).ok()?;
    let common_dir = PathBuf::from(out.lines().next()?);
    Some((main_root_of(&common_dir), main_name_of(&common_dir)))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusSummary {
    pub branch: Option<String>,
    pub detached_short: Option<String>,
    pub dirty_count: usize,
    pub ahead: u32,
    pub behind: u32,
}

/// Parses `git status --porcelain=v2 --branch`.
pub fn parse_status_v2(text: &str) -> StatusSummary {
    let mut s = StatusSummary::default();
    let (mut oid, mut head) = (None, None);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.oid ") {
            oid = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("# branch.head ") {
            head = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            let mut it = rest.split_whitespace();
            s.ahead = it.next().and_then(|a| a.strip_prefix('+')).and_then(|n| n.parse().ok()).unwrap_or(0);
            s.behind = it.next().and_then(|b| b.strip_prefix('-')).and_then(|n| n.parse().ok()).unwrap_or(0);
        } else if ["1 ", "2 ", "u ", "? "].iter().any(|p| line.starts_with(p)) {
            s.dirty_count += 1;
        }
    }
    match head.as_deref() {
        Some("(detached)") => {
            s.detached_short = oid.filter(|o| o != "(initial)").map(|o| o.chars().take(7).collect());
        }
        Some(name) => s.branch = Some(name.to_string()),
        None => {}
    }
    s
}

/// Runs `git -C cwd args…`; Ok(stdout) on success, Err(stderr or reason) otherwise. The child is killed when
/// `timeout` passes. Stdout is drained on a thread so a large status cannot fill the pipe and stall git.
pub(crate) fn run_git_with(cwd: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法运行 git：{e}"))?;
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let out_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("git 超时".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let (stdout, stderr) = (out_t.join().unwrap_or_default(), err_t.join().unwrap_or_default());
    if status.success() {
        Ok(stdout)
    } else {
        Err(stderr.trim().to_string())
    }
}

/// What a git lookup found. `Failed` (timeout, no git, a failing `status`) says nothing about the directory, so
/// callers keep what they knew; `NotRepo` is an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryOutcome {
    Info(GitInfo),
    NotRepo,
    Failed,
}

/// An error from [`run_git_with`] that is the environment's fault, not git's answer about the directory.
fn is_transient(err: &str) -> bool {
    err.starts_with("git 超时") || err.starts_with("无法运行 git")
}

/// Turns the results of `rev-parse` and (only when that worked) `status` into an outcome. A failing
/// `rev-parse` that is not a timeout or spawn error means the directory is not in a work tree.
fn classify(first: Result<String, String>, status: impl FnOnce() -> Result<String, String>) -> QueryOutcome {
    let paths = match first {
        Ok(p) => p,
        Err(e) if is_transient(&e) => return QueryOutcome::Failed,
        Err(_) => return QueryOutcome::NotRepo,
    };
    let mut lines = paths.lines();
    let (Some(root), Some(common)) = (lines.next(), lines.next()) else { return QueryOutcome::Failed };
    let Ok(status) = status() else { return QueryOutcome::Failed };
    let (repo_root, common_dir) = (PathBuf::from(root), PathBuf::from(common));
    let status = parse_status_v2(&status);
    let is_linked_worktree = common_dir != repo_root.join(".git");
    QueryOutcome::Info(GitInfo {
        repo_root,
        common_dir,
        branch: status.branch,
        detached_short: status.detached_short,
        dirty_count: status.dirty_count,
        ahead: status.ahead,
        behind: status.behind,
        is_linked_worktree,
    })
}

/// Branch, dirtiness and worktree facts for the repository holding `cwd`, telling "not a repository" from
/// "could not find out".
pub fn query_outcome(cwd: &Path) -> QueryOutcome {
    let first = run_git_with(cwd, &["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-common-dir"], QUERY_TIMEOUT);
    classify(first, || run_git_with(cwd, &["status", "--porcelain=v2", "--branch"], QUERY_TIMEOUT))
}

/// Branch, dirtiness and worktree facts for the repository holding `cwd`; None outside a work tree, without
/// git, or on any failure.
pub fn query(cwd: &Path) -> Option<GitInfo> {
    match query_outcome(cwd) {
        QueryOutcome::Info(i) => Some(i),
        QueryOutcome::NotRepo | QueryOutcome::Failed => None,
    }
}

/// "⎇ branch ●dirty ↑a↓b", dropping ahead/behind, then dirty, then shortening the branch to fit `max_chars`.
pub fn display_line(g: &GitInfo, max_chars: usize) -> String {
    let name = g.branch.clone().or_else(|| g.detached_short.clone()).unwrap_or_else(|| "?".into());
    let dirty = (g.dirty_count > 0).then(|| format!(" ●{}", g.dirty_count)).unwrap_or_default();
    let ab = (g.ahead > 0 || g.behind > 0).then(|| format!(" ↑{}↓{}", g.ahead, g.behind)).unwrap_or_default();
    for line in [format!("⎇ {name}{dirty}{ab}"), format!("⎇ {name}{dirty}"), format!("⎇ {name}")] {
        if line.chars().count() <= max_chars {
            return line;
        }
    }
    let keep = max_chars.saturating_sub(3);
    format!("⎇ {}…", name.chars().take(keep).collect::<String>())
}

/// Longest worktree slug, in chars.
pub const SLUG_MAX: usize = 40;
/// `git worktree add` checks files out, which can take a while on a big repository.
const WORKTREE_TIMEOUT: Duration = Duration::from_secs(30);

/// `[a-z0-9-]` words of `task` joined by single dashes, at most [`SLUG_MAX`] chars; "task" when nothing is left.
pub fn slug(task: &str) -> String {
    let mut out = String::new();
    for c in task.chars().flat_map(|c| c.to_lowercase()) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed: String = out.trim_matches('-').chars().take(SLUG_MAX).collect();
    let trimmed = trimmed.trim_end_matches('-').to_string();
    if trimmed.is_empty() {
        "task".into()
    } else {
        trimmed
    }
}

pub fn branch_name(task: &str, id4: &str) -> String {
    format!("gilvt/{}-{id4}", slug(task))
}

/// `<repo_root>/../<repo>.worktrees/<branch with "/" → "-">`.
pub fn worktree_path(repo_root: &Path, branch: &str) -> Option<PathBuf> {
    let name = repo_root.file_name()?.to_string_lossy().into_owned();
    Some(repo_root.parent()?.join(format!("{name}.worktrees")).join(branch.replace('/', "-")))
}

/// Four hex digits from the clock, enough to keep two worktrees of one task apart.
pub fn short_id() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    format!("{:04x}", (nanos / 1000) & 0xffff)
}

/// Where an agent starts inside `new_worktree` for a chosen directory `dir` of the checkout `repo_root`: the same
/// relative subfolder, or the worktree's root when `dir` is the root, lies outside `repo_root`, or the subfolder
/// does not exist in the new checkout.
pub fn worktree_start_dir(dir: &Path, repo_root: &Path, new_worktree: &Path) -> PathBuf {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let rel = dir.strip_prefix(repo_root).map(Path::to_path_buf).or_else(|_| canon(dir).strip_prefix(canon(repo_root)).map(Path::to_path_buf));
    match rel {
        Ok(rel) if !rel.as_os_str().is_empty() && new_worktree.join(&rel).is_dir() => new_worktree.join(rel),
        _ => new_worktree.to_path_buf(),
    }
}

/// Creates a worktree for `task` off the HEAD of `from_repo_root` (the checkout the user chose, possibly a
/// linked worktree) and returns its path, which is always next to the MAIN repository (`main_repo_root`, see
/// [`GitInfo::main_repo_root`]). Blocking. Never removes anything; fails without leaving a half-made worktree
/// (git cleans up after itself).
pub fn create_worktree(main_repo_root: &Path, from_repo_root: &Path, task: &str, id4: &str) -> Result<PathBuf, String> {
    let branch = branch_name(task, id4);
    let path = worktree_path(main_repo_root, &branch).ok_or_else(|| "无法确定 worktree 位置".to_string())?;
    if path.exists() {
        return Err(format!("目录已存在：{}", path.display()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("无法创建 {}：{e}", parent.display()))?;
    }
    let path_str = path.to_string_lossy().into_owned();
    run_git_with(from_repo_root, &["worktree", "add", "-b", &branch, &path_str], WORKTREE_TIMEOUT)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_with_upstream_and_changes() {
        let text = "# branch.oid abcdef1234567890\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n1 .M N... 100644 100644 100644 a b src/a.rs\n? new.txt\nu UU N... 1 2 3 4 a b c conflict.rs\n";
        let s = parse_status_v2(text);
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!((s.ahead, s.behind, s.dirty_count), (2, 1, 3));
        assert_eq!(s.detached_short, None);
    }

    #[test]
    fn no_upstream_means_zero_ahead_behind() {
        let s = parse_status_v2("# branch.oid abc\n# branch.head feat\n");
        assert_eq!((s.ahead, s.behind, s.dirty_count), (0, 0, 0));
        assert_eq!(s.branch.as_deref(), Some("feat"));
    }

    #[test]
    fn detached_head_shows_short_hash() {
        let s = parse_status_v2("# branch.oid 0123456789abcdef\n# branch.head (detached)\n");
        assert_eq!(s.branch, None);
        assert_eq!(s.detached_short.as_deref(), Some("0123456"));
    }

    #[test]
    fn initial_commit_has_branch_and_no_hash() {
        let s = parse_status_v2("# branch.oid (initial)\n# branch.head main\n");
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.detached_short, None);
    }

    fn info(branch: &str, dirty: usize, ahead: u32, behind: u32) -> GitInfo {
        GitInfo {
            repo_root: "/r/app".into(),
            common_dir: "/r/app/.git".into(),
            branch: Some(branch.into()),
            detached_short: None,
            dirty_count: dirty,
            ahead,
            behind,
            is_linked_worktree: false,
        }
    }

    #[test]
    fn display_line_drops_parts_as_width_shrinks() {
        let g = info("feature/login", 3, 2, 1);
        assert_eq!(display_line(&g, 100), "⎇ feature/login ●3 ↑2↓1");
        assert_eq!(display_line(&g, 18), "⎇ feature/login ●3");
        assert_eq!(display_line(&g, 15), "⎇ feature/login");
        assert_eq!(display_line(&g, 8), "⎇ featu…");
        assert_eq!(display_line(&info("main", 0, 0, 0), 100), "⎇ main");
    }

    #[test]
    fn main_repo_name_uses_common_dir_not_worktree_dir() {
        let mut g = info("x", 0, 0, 0);
        g.repo_root = "/r/app.worktrees/gilvt-x-ab12".into();
        g.is_linked_worktree = true;
        assert_eq!(g.main_repo_name(), "app");
    }

    fn git(dir: &Path, args: &[&str]) {
        let st = std::process::Command::new("git")
            .arg("-C").arg(dir).args(args)
            .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
            .status().unwrap();
        assert!(st.success(), "git {args:?}");
    }

    #[test]
    fn query_on_a_real_repo_and_its_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().canonicalize().unwrap().join("app");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "x").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        std::fs::write(repo.join("b.txt"), "y").unwrap();

        let g = query(&repo).unwrap();
        assert_eq!(g.repo_root, repo);
        assert_eq!(g.branch.as_deref(), Some("main"));
        assert_eq!(g.dirty_count, 1);
        assert!(!g.is_linked_worktree);

        let wt = dir.path().canonicalize().unwrap().join("wt");
        git(&repo, &["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()]);
        let w = query(&wt).unwrap();
        assert_eq!(w.branch.as_deref(), Some("feat"));
        assert!(w.is_linked_worktree);
        assert_eq!(w.main_repo_name(), "app");
    }

    fn ok(s: &str) -> Result<String, String> {
        Ok(s.to_string())
    }

    #[test]
    fn classify_separates_answers_from_failures() {
        let paths = "/r/app\n/r/app/.git\n";
        let st = "# branch.oid abc\n# branch.head main\n1 .M N... 1 1 1 a b c\n";
        match classify(ok(paths), || ok(st)) {
            QueryOutcome::Info(g) => assert_eq!((g.branch.as_deref(), g.dirty_count, g.is_linked_worktree), (Some("main"), 1, false)),
            other => panic!("{other:?}"),
        }
        let not_repo = Err("fatal: not a git repository (or any of the parent directories): .git".to_string());
        assert_eq!(classify(not_repo, || panic!("status must not run")), QueryOutcome::NotRepo);
        assert_eq!(classify(Err("fatal: cannot change to '/gone': No such file or directory".into()), || ok("")), QueryOutcome::NotRepo);
        assert_eq!(classify(Err("git 超时".into()), || ok("")), QueryOutcome::Failed, "timeout");
        assert_eq!(classify(Err("无法运行 git：No such file".into()), || ok("")), QueryOutcome::Failed, "spawn error");
        assert_eq!(classify(ok(paths), || Err("git 超时".into())), QueryOutcome::Failed, "status failed after rev-parse");
        assert_eq!(classify(ok("/r/app\n"), || ok(st)), QueryOutcome::Failed, "unexpected rev-parse output");
    }

    #[test]
    fn a_non_repo_directory_is_not_repo_and_a_repo_is_info() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(query_outcome(dir.path()), QueryOutcome::NotRepo);
        assert_eq!(query_outcome(&dir.path().join("does-not-exist")), QueryOutcome::NotRepo);
        let repo = dir.path().canonicalize().unwrap().join("app");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        assert!(matches!(query_outcome(&repo), QueryOutcome::Info(_)));
    }

    #[test]
    fn query_outside_a_repo_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(query(dir.path()), None);
        assert_eq!(query(&dir.path().join("does-not-exist")), None);
    }

    #[test]
    fn slug_keeps_ascii_words_and_falls_back() {
        assert_eq!(slug("Fix the Login bug!"), "fix-the-login-bug");
        assert_eq!(slug("  --a__b--  "), "a-b");
        assert_eq!(slug("修复登录"), "task");
        assert_eq!(slug(""), "task");
        assert_eq!(slug(&"a".repeat(80)).len(), 40);
        assert!(!slug(&format!("{}-b", "a".repeat(39))).ends_with('-'));
    }

    #[test]
    fn branch_and_path_naming() {
        assert_eq!(branch_name("Fix login", "ab12"), "gilvt/fix-login-ab12");
        assert_eq!(
            worktree_path(Path::new("/r/app"), "gilvt/fix-login-ab12"),
            Some(PathBuf::from("/r/app.worktrees/gilvt-fix-login-ab12"))
        );
        assert_eq!(worktree_path(Path::new("/"), "x"), None);
    }

    #[test]
    fn create_worktree_makes_a_branch_and_refuses_existing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().canonicalize().unwrap().join("app");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "x").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);

        let path = create_worktree(&repo, &repo, "Add search", "ab12").unwrap();
        assert_eq!(path, dir.path().canonicalize().unwrap().join("app.worktrees/gilvt-add-search-ab12"));
        assert_eq!(query(&path).unwrap().branch.as_deref(), Some("gilvt/add-search-ab12"));
        let again = create_worktree(&repo, &repo, "Add search", "ab12").unwrap_err();
        assert!(again.contains("已存在"), "{again}");
    }

    #[test]
    fn create_worktree_in_a_non_repo_reports_git_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(create_worktree(dir.path(), dir.path(), "x", "ab12").is_err());
    }

    #[test]
    fn main_repo_root_is_the_common_dirs_parent_or_the_bare_dir() {
        let mut g = info("x", 0, 0, 0);
        g.repo_root = "/r/app.worktrees/gilvt-x-ab12".into();
        g.common_dir = "/r/app/.git".into();
        assert_eq!(g.main_repo_root(), PathBuf::from("/r/app"));
        g.common_dir = "/r/app.git".into();
        assert_eq!(g.main_repo_root(), PathBuf::from("/r/app.git"));
        g.common_dir = "/r/app".into();
        assert_eq!(g.main_repo_root(), PathBuf::from("/r/app"), "no .git suffix: the dir itself");
    }

    #[test]
    fn main_repo_of_is_the_same_for_a_repo_a_subfolder_and_a_linked_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let repo = base.join("app");
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "x").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        let wt = base.join("app-wt");
        git(&repo, &["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()]);
        let expect = Some((repo.clone(), "app".to_string()));
        assert_eq!(main_repo_of(&repo), expect);
        assert_eq!(main_repo_of(&repo.join("sub")), expect);
        assert_eq!(main_repo_of(&wt), expect, "a linked worktree belongs to the main repository");
        assert_eq!(main_repo_of(&base.join("nope")), None);
        assert_eq!(main_repo_of(&base), None, "a plain directory is not a repository");
    }

    #[test]
    fn start_dir_keeps_the_relative_subfolder_when_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let (repo, wt) = (base.join("mono"), base.join("mono.worktrees/gilvt-x"));
        std::fs::create_dir_all(wt.join("gilvt/crates")).unwrap();
        assert_eq!(worktree_start_dir(&repo, &repo, &wt), wt, "the root stays the root");
        assert_eq!(worktree_start_dir(&repo.join("gilvt"), &repo, &wt), wt.join("gilvt"));
        assert_eq!(worktree_start_dir(&repo.join("gilvt/crates"), &repo, &wt), wt.join("gilvt/crates"));
        assert_eq!(worktree_start_dir(&repo.join("untracked/sub"), &repo, &wt), wt, "missing there: the root");
        assert_eq!(worktree_start_dir(&base.join("elsewhere"), &repo, &wt), wt, "outside the repo: the root");
    }

    #[test]
    fn start_dir_sees_through_symlinked_paths() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let (repo, wt) = (base.join("mono"), base.join("wt"));
        std::fs::create_dir_all(repo.join("gilvt")).unwrap();
        std::fs::create_dir_all(wt.join("gilvt")).unwrap();
        std::os::unix::fs::symlink(&repo, base.join("link")).unwrap();
        assert_eq!(worktree_start_dir(&base.join("link/gilvt"), &repo, &wt), wt.join("gilvt"));
    }

    #[test]
    fn worktree_from_a_linked_checkout_lands_next_to_the_main_repo_and_bases_on_its_head() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let repo = base.join("app");
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("sub/a.txt"), "x").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        // A linked worktree nested inside the repo (like .claude/worktrees/…), with one extra commit.
        let linked = repo.join(".claude/worktrees/feat");
        git(&repo, &["worktree", "add", "-q", "-b", "feat", linked.to_str().unwrap()]);
        std::fs::write(linked.join("sub/b.txt"), "y").unwrap();
        git(&linked, &["add", "."]);
        git(&linked, &["commit", "-q", "-m", "on feat"]);

        let info = query(&linked.join("sub")).unwrap();
        assert!(info.is_linked_worktree);
        assert_eq!(info.main_repo_root(), repo);
        let path = create_worktree(&info.main_repo_root(), &info.repo_root, "Try it", "cd34").unwrap();
        assert_eq!(path, base.join("app.worktrees/gilvt-try-it-cd34"), "next to the main repo, not nested in the linked one");
        assert!(path.join("sub/b.txt").exists(), "based on the HEAD of the chosen (linked) checkout");
        assert_eq!(worktree_start_dir(&linked.join("sub"), &info.repo_root, &path), path.join("sub"));
    }
}
