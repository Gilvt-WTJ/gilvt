//! Everything needed to show one file: the document plus its diff against a git base.

use std::io;
use std::path::{Path, PathBuf};

use crate::diff::{diff_texts, Diff};
use crate::document::{Content, Document};
use crate::git;
use gilvt_snapshot::{ObjectStore, SnapshotError, TreeId};

/// The two snapshots of one agent turn (`gilvt-snapshot`): a file is shown as the turn's second snapshot has
/// it, compared with the first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnRange {
    pub store: ObjectStore,
    pub before: TreeId,
    pub after: TreeId,
    /// The base's label, e.g. 「本轮（第 3 轮前 → 后）」 / 「本任务（第 5 轮前 → 第 7 轮后）」.
    pub label: String,
    /// What the range covers, for 「文件在{scope}之后又被改动」: 这一轮 / 这个任务 / 本会话.
    pub scope: &'static str,
}

/// What the file is compared against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffBase {
    /// The committed version (HEAD).
    Head,
    /// Another revision (branch, tag, commit).
    Rev(String),
    /// Before / after one agent turn.
    Turn(TurnRange),
    /// No comparison: just the file.
    None,
}

impl DiffBase {
    pub fn rev(&self) -> Option<&str> {
        match self {
            DiffBase::Head => Some("HEAD"),
            DiffBase::Rev(r) => Some(r),
            DiffBase::Turn(_) => None,
            DiffBase::None => None,
        }
    }

    /// Label for the UI, e.g. "与 HEAD 相比".
    pub fn label(&self) -> String {
        match self {
            DiffBase::Head => "与 HEAD 相比".into(),
            DiffBase::Rev(r) => format!("与 {r} 相比"),
            DiffBase::Turn(r) => r.label.clone(),
            DiffBase::None => "仅文件".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Preview {
    pub doc: Document,
    /// Repository containing the file, if any.
    pub repo_root: Option<PathBuf>,
    /// The base actually used (falls back to `None` outside a repo).
    pub base: DiffBase,
    /// For text documents: the diff against `base` (all-context when `base` is `None`).
    pub diff: Option<Diff>,
    /// The file does not exist at `base` (new / untracked file).
    pub is_new: bool,
    /// `DiffBase::Turn`: the file on disk differs from the turn's second snapshot (it changed after the turn).
    pub changed_since: bool,
}

impl Preview {
    /// Loads `path` and diffs it against `base`. Blocking (reads the file, runs git).
    pub fn load(path: &Path, base: &DiffBase) -> io::Result<Preview> {
        if let DiffBase::Turn(range) = base {
            return Preview::load_turn(path, range);
        }
        let doc = Document::load(path)?;
        let repo_root = path.parent().and_then(git::repo_root);
        let mut preview = Preview { doc, repo_root, base: DiffBase::None, diff: None, is_new: false, changed_since: false };
        let Some(text) = preview.doc.text().map(str::to_string) else { return Ok(preview) };

        let old = match (&preview.repo_root, base.rev()) {
            (Some(root), Some(rev)) => {
                // `git` reports paths relative to the canonical root (e.g. /private/tmp on macOS).
                let canon_root = root.canonicalize().unwrap_or_else(|_| root.clone());
                let canon_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
                let rel = canon_path.strip_prefix(&canon_root).unwrap_or(path).to_path_buf();
                preview.base = base.clone();
                let old = git::show_file(&canon_root, rev, &rel)?;
                preview.is_new = old.is_none();
                Some(old.unwrap_or_default())
            }
            _ => None,
        };
        preview.diff = Some(match old {
            Some(old) => diff_texts(&old, &text),
            None => Diff::unchanged(&text),
        });
        Ok(preview)
    }

    /// Inline content (`gilvt view -`): no path, no git.
    pub fn from_content(content: String, type_hint: Option<String>) -> Preview {
        let doc = Document::from_content(content, type_hint);
        let diff = doc.text().map(Diff::unchanged);
        Preview { doc, repo_root: None, base: DiffBase::None, diff, is_new: false, changed_since: false }
    }

    /// Two texts compared: `new` is what is shown (as a document that has `path`, if any), `old` the base of
    /// the diff. No path on disk is read and no git is run: the editor's live preview builds it from its buffer.
    pub fn from_texts(path: Option<&Path>, old: &str, new: String, type_hint: Option<String>) -> Preview {
        let name = path.and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "stdin".to_string());
        let diff = diff_texts(old, &new);
        let size = new.len() as u64;
        let doc = Document { path: path.map(Path::to_path_buf), name, type_hint, content: Content::Text(new), size };
        Preview { doc, repo_root: None, base: DiffBase::None, diff: Some(diff), is_new: false, changed_since: false }
    }

    /// `path` as the turn's second snapshot has it, diffed against the first. The disk is only read to tell
    /// whether the file changed since (a file the turn deleted need not exist any more).
    fn load_turn(path: &Path, range: &TurnRange) -> io::Result<Preview> {
        let root = range.store.repo_root();
        let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        // The file may be gone (the turn deleted it): resolve its directory and keep the name.
        let canon_path = path.canonicalize().unwrap_or_else(|_| match (path.parent().and_then(|d| d.canonicalize().ok()), path.file_name()) {
            (Some(dir), Some(name)) => dir.join(name),
            _ => path.to_path_buf(),
        });
        let rel = canon_path
            .strip_prefix(&canon_root)
            .map_err(|_| io::Error::other(format!("{}: 不在这个仓库里", path.display())))?
            .to_string_lossy()
            .into_owned();
        let gone = |e: SnapshotError| match e {
            SnapshotError::Missing => io::Error::other("快照已清理"),
            other => io::Error::other(other.to_string()),
        };
        let old = range.store.blob_at(&range.before, &rel).map_err(gone)?;
        let new = range.store.blob_at(&range.after, &rel).map_err(gone)?;
        let changed_since = std::fs::read(path).ok() != new;
        let doc = Document::from_bytes(path, new.unwrap_or_default());
        let mut preview = Preview {
            doc,
            repo_root: Some(root.to_path_buf()),
            base: DiffBase::Turn(range.clone()),
            diff: None,
            is_new: old.is_none(),
            changed_since,
        };
        if let Some(text) = preview.doc.text().map(str::to_string) {
            let old_text = old.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            preview.diff = Some(diff_texts(&old_text, &text));
        }
        Ok(preview)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::LineKind;
    use crate::git::testrepo;
    use gilvt_snapshot::ObjectStore;

    #[test]
    fn diff_against_head() {
        let repo = testrepo::make();
        let file = repo.path().join("a.txt");
        std::fs::write(&file, "one\nTWO\n").unwrap();
        let p = Preview::load(&file, &DiffBase::Head).unwrap();
        assert_eq!(p.base, DiffBase::Head);
        assert!(!p.is_new);
        assert_eq!(p.diff.unwrap().stats(), (1, 1));
    }

    #[test]
    fn untracked_file_is_new() {
        let repo = testrepo::make();
        let file = repo.path().join("n.txt");
        std::fs::write(&file, "x\n").unwrap();
        let p = Preview::load(&file, &DiffBase::Head).unwrap();
        assert!(p.is_new);
        assert_eq!(p.diff.unwrap().lines[0].kind, LineKind::Added);
    }

    #[test]
    fn file_only_and_outside_repo() {
        let repo = testrepo::make();
        let file = repo.path().join("a.txt");
        std::fs::write(&file, "changed\n").unwrap();
        let p = Preview::load(&file, &DiffBase::None).unwrap();
        assert!(!p.diff.unwrap().has_changes());

        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.rs");
        std::fs::write(&f, "fn x() {}\n").unwrap();
        let p = Preview::load(&f, &DiffBase::Head).unwrap();
        assert_eq!(p.base, DiffBase::None, "no repo: falls back to file only");
        assert!(p.repo_root.is_none());
    }

    #[test]
    fn inline_content() {
        let p = Preview::from_content("a\nb\n".into(), Some("txt".into()));
        assert_eq!(p.diff.unwrap().lines.len(), 2);
        assert_eq!(DiffBase::Rev("main".into()).label(), "与 main 相比");
    }

    #[test]
    fn from_texts_diffs_old_against_new_and_keeps_the_path() {
        let path = Path::new("/tmp/notes/a.md");
        let p = Preview::from_texts(Some(path), "one\ntwo\n", "one\nTWO\n".to_string(), None);
        assert_eq!(p.doc.path.as_deref(), Some(path));
        assert_eq!(p.doc.name, "a.md");
        assert_eq!(p.doc.text(), Some("one\nTWO\n"));
        let diff = p.diff.as_ref().unwrap();
        assert!(diff.has_changes());
        assert_eq!(diff.stats(), (1, 1));
        assert_eq!(p.repo_root, None);
        assert_eq!(p.base, DiffBase::None);
    }

    #[test]
    fn from_texts_without_a_path_is_named_stdin_and_honours_the_hint() {
        let p = Preview::from_texts(None, "", "# hi\n".to_string(), Some("md".into()));
        assert_eq!(p.doc.path, None);
        assert_eq!(p.doc.name, "stdin");
        assert_eq!(p.doc.syntax_token(), "md");
    }

    fn two_snapshots(repo: &std::path::Path, state: &std::path::Path, edit: impl FnOnce(&std::path::Path)) -> TurnRange {
        let store = ObjectStore::open(repo, state).unwrap();
        let before = store.take().unwrap().tree;
        edit(repo);
        let after = store.take().unwrap().tree;
        TurnRange { store, before, after, label: "本轮（第 3 轮前 → 后）".into(), scope: "这一轮" }
    }

    #[test]
    fn turn_diff_reads_both_sides_from_the_snapshots() {
        let (repo, state) = (testrepo::make(), tempfile::tempdir().unwrap());
        let range = two_snapshots(repo.path(), state.path(), |r| std::fs::write(r.join("a.txt"), "one\nTWO\nthree\n").unwrap());
        std::fs::write(repo.path().join("a.txt"), "the disk moved on\n").unwrap();
        let p = Preview::load(&repo.path().join("a.txt"), &DiffBase::Turn(range)).unwrap();
        assert_eq!(p.doc.text(), Some("one\nTWO\nthree\n"), "the new side is the `after` snapshot, not the disk");
        assert_eq!(p.diff.as_ref().unwrap().stats(), (2, 1));
        assert!(p.changed_since);
        assert_eq!(p.base.label(), "本轮（第 3 轮前 → 后）");
    }

    #[test]
    fn an_unchanged_disk_file_is_not_changed_since() {
        let (repo, state) = (testrepo::make(), tempfile::tempdir().unwrap());
        let range = two_snapshots(repo.path(), state.path(), |r| std::fs::write(r.join("a.txt"), "one\nTWO\n").unwrap());
        let p = Preview::load(&repo.path().join("a.txt"), &DiffBase::Turn(range)).unwrap();
        assert!(!p.changed_since);
    }

    #[test]
    fn a_new_file_in_the_turn_is_new() {
        let (repo, state) = (testrepo::make(), tempfile::tempdir().unwrap());
        let range = two_snapshots(repo.path(), state.path(), |r| std::fs::write(r.join("n.txt"), "n1\nn2\n").unwrap());
        let p = Preview::load(&repo.path().join("n.txt"), &DiffBase::Turn(range)).unwrap();
        assert!(p.is_new);
        assert_eq!(p.diff.unwrap().stats(), (2, 0));
    }

    #[test]
    fn a_file_deleted_in_the_turn_shows_as_removed() {
        let (repo, state) = (testrepo::make(), tempfile::tempdir().unwrap());
        let range = two_snapshots(repo.path(), state.path(), |r| std::fs::remove_file(r.join("a.txt")).unwrap());
        let p = Preview::load(&repo.path().join("a.txt"), &DiffBase::Turn(range)).unwrap();
        assert_eq!(p.doc.text(), Some(""), "nothing to read from the disk: the file is gone");
        assert_eq!(p.diff.unwrap().stats(), (0, 2));
        assert!(!p.changed_since, "still gone");
    }

    #[test]
    fn a_binary_file_shows_the_binary_message() {
        let (repo, state) = (testrepo::make(), tempfile::tempdir().unwrap());
        let range = two_snapshots(repo.path(), state.path(), |r| std::fs::write(r.join("b.dat"), [0u8, 1, 2]).unwrap());
        let p = Preview::load(&repo.path().join("b.dat"), &DiffBase::Turn(range)).unwrap();
        assert_eq!(p.doc.content, crate::Content::Binary);
        assert!(p.diff.is_none());
    }

    #[test]
    fn a_pruned_snapshot_is_reported() {
        let (repo, state) = (testrepo::make(), tempfile::tempdir().unwrap());
        let range = two_snapshots(repo.path(), state.path(), |r| std::fs::write(r.join("a.txt"), "only in the store\n").unwrap());
        gilvt_snapshot::prune(state.path(), std::time::Duration::ZERO).unwrap();
        let err = Preview::load(&repo.path().join("a.txt"), &DiffBase::Turn(range)).unwrap_err();
        assert!(err.to_string().contains("快照已清理"), "{err}");
    }

    #[test]
    fn a_file_outside_the_repository_is_refused() {
        let (repo, state) = (testrepo::make(), tempfile::tempdir().unwrap());
        let range = two_snapshots(repo.path(), state.path(), |_| {});
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("x.txt"), "x\n").unwrap();
        assert!(Preview::load(&other.path().join("x.txt"), &DiffBase::Turn(range)).is_err());
    }
}
