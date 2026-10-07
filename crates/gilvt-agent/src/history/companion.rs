//! The data an agent keeps per session next to its transcript (checkpoints, env, tasks, shell snapshots):
//! found only by the session id, only below the agent's own root, never through a symlink.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{AgentKind, HistoryEntry};

/// `8-4-4-4-12` lowercase hex: what both agents name sessions with. Anything else never reaches a path.
pub fn is_session_uuid(id: &str) -> bool {
    let parts: Vec<&str> = id.split('-').collect();
    let lens = [8, 4, 4, 4, 12];
    parts.len() == 5
        && parts.iter().zip(lens).all(|(p, n)| {
            p.len() == n
                && p.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

fn root_of(transcript: &Path, name: &str) -> Option<PathBuf> {
    transcript
        .ancestors()
        .find(|a| a.file_name().is_some_and(|n| n == name))
        .map(Path::to_path_buf)
}

/// A path that exists, is not a symlink, and (after resolving) is still inside `root`.
fn inside(root: &Path, path: PathBuf) -> Option<PathBuf> {
    let meta = path.symlink_metadata().ok()?;
    if meta.file_type().is_symlink() {
        return None;
    }
    let root = root.canonicalize().ok()?;
    path.canonicalize().ok()?.starts_with(&root).then_some(path)
}

/// The per-session data of `e` outside its transcript. Empty for an id that is not a UUID or a transcript
/// outside the agent's root.
pub fn companion_files(e: &HistoryEntry) -> Vec<PathBuf> {
    if !is_session_uuid(&e.session_id) {
        return Vec::new();
    }
    let id = &e.session_id;
    let mut found = Vec::new();
    match e.agent {
        AgentKind::Claude => {
            let Some(root) = root_of(&e.transcript, ".claude") else {
                return found;
            };
            for dir in ["file-history", "session-env", "tasks"] {
                found.extend(inside(&root, root.join(dir).join(id)));
            }
            if let Ok(read) = fs::read_dir(root.join("todos")) {
                let prefix = format!("{id}-");
                let mut todos: Vec<PathBuf> = read
                    .flatten()
                    .filter(|f| f.file_name().to_string_lossy().starts_with(&prefix))
                    .filter_map(|f| inside(&root, f.path()))
                    .collect();
                todos.sort();
                found.extend(todos);
            }
        }
        AgentKind::Codex => {
            let Some(root) = root_of(&e.transcript, ".codex") else {
                return found;
            };
            if let Ok(read) = fs::read_dir(root.join("shell_snapshots")) {
                let prefix = format!("{id}.");
                let mut snaps: Vec<PathBuf> = read
                    .flatten()
                    .filter(|f| f.file_name().to_string_lossy().starts_with(&prefix))
                    .filter_map(|f| inside(&root, f.path()))
                    .collect();
                snaps.sort();
                found.extend(snaps);
            }
        }
    }
    found
}

/// Bytes of a file, or of a directory tree; symlinks count 0 and are not followed.
pub fn path_size(path: &Path) -> u64 {
    let Ok(meta) = path.symlink_metadata() else {
        return 0;
    };
    if meta.file_type().is_symlink() {
        0
    } else if meta.is_dir() {
        fs::read_dir(path).map_or(0, |d| d.flatten().map(|f| path_size(&f.path())).sum())
    } else {
        meta.len()
    }
}

pub fn companion_size(e: &HistoryEntry) -> u64 {
    companion_files(e).iter().map(|p| path_size(p)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    const ID: &str = "0f282162-b358-47c9-9314-2b637ec43051";

    fn entry(agent: AgentKind, transcript: PathBuf, id: &str) -> HistoryEntry {
        HistoryEntry {
            agent,
            session_id: id.into(),
            cwd: "/Users/u/proj".into(),
            transcript,
            first_prompt: String::new(),
            topic_prompt: String::new(),
            custom_title: None,
            ai_title: None,
            started: None,
            last_active: SystemTime::UNIX_EPOCH,
            turns: 1,
            model: None,
            size: 0,
        }
    }

    fn touch(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    #[test]
    fn claude_companions_are_the_four_id_named_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".claude");
        let transcript = root.join(format!("projects/-p/{ID}.jsonl"));
        touch(&transcript, 1);
        touch(&root.join(format!("file-history/{ID}/a@v1")), 10);
        fs::create_dir_all(root.join(format!("session-env/{ID}"))).unwrap();
        touch(&root.join(format!("tasks/{ID}/1.json")), 5);
        touch(&root.join(format!("todos/{ID}-agent-{ID}.json")), 2);
        touch(&root.join("file-history/other/x"), 99);
        let e = entry(AgentKind::Claude, transcript, ID);
        let mut found = companion_files(&e);
        found.sort();
        let names: Vec<String> = found
            .iter()
            .map(|p| {
                p.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            names,
            [
                format!("file-history/{ID}"),
                format!("session-env/{ID}"),
                format!("tasks/{ID}"),
                format!("todos/{ID}-agent-{ID}.json"),
            ]
        );
        assert_eq!(companion_size(&e), 10 + 5 + 2);
    }

    #[test]
    fn codex_companions_are_the_shell_snapshots_of_the_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".codex");
        let transcript = root.join(format!("sessions/2026/09/28/rollout-x-{ID}.jsonl"));
        touch(&transcript, 1);
        touch(
            &root.join(format!("shell_snapshots/{ID}.1790651744388103000.sh")),
            7,
        );
        touch(
            &root.join("shell_snapshots/11111111-2222-4333-8444-555555555555.1.sh"),
            99,
        );
        let e = entry(AgentKind::Codex, transcript, ID);
        assert_eq!(companion_files(&e).len(), 1);
        assert_eq!(companion_size(&e), 7);
    }

    #[test]
    fn a_session_id_that_is_not_a_uuid_collects_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".claude");
        let transcript = root.join("projects/-p/x.jsonl");
        touch(&transcript, 1);
        touch(&root.join("file-history/x/a"), 1);
        for bad in [
            "x",
            "..",
            "../projects",
            "a/b",
            "",
            "0F282162-B358-47C9-9314-2B637EC43051/..",
        ] {
            let e = entry(AgentKind::Claude, transcript.clone(), bad);
            assert!(companion_files(&e).is_empty(), "{bad:?}");
        }
        assert!(!is_session_uuid("0f282162-b358-47c9-9314-2b637ec4305")); // one char short
        assert!(is_session_uuid(ID));
    }

    #[test]
    fn a_symlinked_companion_is_not_followed_and_missing_ones_are_fine() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".claude");
        let transcript = root.join(format!("projects/-p/{ID}.jsonl"));
        touch(&transcript, 1);
        let elsewhere = tmp.path().join("elsewhere");
        touch(&elsewhere.join("keep"), 50);
        fs::create_dir_all(root.join("file-history")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join(format!("file-history/{ID}"))).unwrap();
        let e = entry(AgentKind::Claude, transcript, ID);
        assert!(
            companion_files(&e).is_empty(),
            "a link out of the root is skipped"
        );
        assert_eq!(companion_size(&e), 0);
        // A transcript outside any `.claude` / `.codex` root has no companions.
        let stray = entry(
            AgentKind::Claude,
            tmp.path().join(format!("{ID}.jsonl")),
            ID,
        );
        assert!(companion_files(&stray).is_empty());
    }
}
