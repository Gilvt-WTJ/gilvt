//! Moving past sessions to the Trash (spec §4.3): every file of the session (`session_files`), after
//! checking once more that it is not running; then out of the history list, its cache and the sidebar.

use std::path::{Path, PathBuf};

use gilvt_agent::{companion_files, session_files, HistoryEntry, Session, SessionKey};
use gpui::App;

use super::history::{title, History};
use super::trash::move_to_trash;
use crate::agents::Agents;

/// Why a running session was skipped.
pub const LIVE_REASON: &str = "正在运行";

/// The outcome of [`trash_sessions`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrashReport {
    /// Sessions whose files all went to the Trash (or were already gone).
    pub moved: usize,
    /// (title, reason) of the sessions that stayed, in the order given.
    pub failed: Vec<(String, String)>,
}

impl TrashReport {
    /// The toast after a partial failure: 「已移走 N 个，M 个失败：<首个原因>」; None when all were moved.
    pub fn toast(&self) -> Option<String> {
        let (_, reason) = self.failed.first()?;
        Some(if crate::i18n::english() {
            format!("Moved {}, {} failed: {reason}", self.moved, self.failed.len())
        } else {
            format!("已移走 {} 个，{} 个失败：{reason}", self.moved, self.failed.len())
        })
    }

    /// The banner after the confirm bar's 移到废纸篓: the partial failure, else how many went.
    pub fn banner(&self) -> String {
        self.toast().unwrap_or_else(|| {
            if crate::i18n::english() {
                format!("Moved {} sessions to Trash", self.moved)
            } else {
                format!("已移到废纸篓 {} 个会话", self.moved)
            }
        })
    }
}

fn key(e: &HistoryEntry) -> SessionKey {
    (e.agent, e.session_id.clone())
}

/// The pure part: `live` sessions are skipped, every other session's files go to `trash`: the companions
/// first, the transcript last (a missing file counts as moved), so a session whose move failed keeps its
/// transcript and stays listed (spec §4.3). Returns the report and the sessions whose transcript is gone now,
/// which leave the lists.
pub fn trash_with<'a>(
    entries: &'a [HistoryEntry],
    title: impl Fn(&HistoryEntry) -> String,
    live: impl Fn(&HistoryEntry) -> bool,
    mut trash: impl FnMut(&Path) -> Result<(), String>,
) -> (TrashReport, Vec<&'a HistoryEntry>) {
    let mut report = TrashReport::default();
    let mut gone = Vec::new();
    for e in entries {
        if live(e) {
            report.failed.push((title(e), crate::i18n::text(LIVE_REASON, "running").into()));
            continue;
        }
        let mut error = None;
        let mut files: Vec<PathBuf> = companion_files(e);
        files.extend(session_files(e).into_iter().rev());
        for file in files {
            if file.symlink_metadata().is_err() {
                continue;
            }
            if let Err(reason) = trash(&file) {
                error = Some(reason);
                break;
            }
        }
        if e.transcript.symlink_metadata().is_err() {
            gone.push(e);
        }
        match error {
            Some(reason) => report.failed.push((title(e), reason)),
            None => report.moved += 1,
        }
    }
    (report, gone)
}

/// Moves `entries` to the Trash (⌘⌫ in the 会话 palette, the sidebar's 移到废纸篓…). Runs on the main thread;
/// the liveness check reads the registry right before each session.
pub fn trash_sessions(entries: Vec<HistoryEntry>, cx: &mut App) -> TrashReport {
    let registry = cx.try_global::<Agents>().map(Agents::registry);
    let (report, gone) = trash_with(
        &entries,
        |e| title(e, registry.and_then(|r| r.saved_name(&key(e)))),
        |e| registry.and_then(|r| r.get(&key(e))).is_some_and(Session::is_live),
        move_to_trash,
    );
    let keys: Vec<SessionKey> = gone.iter().map(|e| key(e)).collect();
    let transcripts = gone.iter().map(|e| e.transcript.clone()).collect();
    Agents::forget(&keys, cx);
    History::forget(transcripts, cx);
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_agent::AgentKind;
    use std::fs;
    use std::path::PathBuf;
    use std::time::SystemTime;

    #[test]
    fn banners_read_in_english() {
        let failed = vec![("t".to_string(), LIVE_REASON.to_string())];
        assert_eq!(TrashReport { moved: 2, failed: failed.clone() }.banner(), "已移走 2 个，1 个失败：正在运行");
        assert_eq!(TrashReport { moved: 2, failed: Vec::new() }.banner(), "已移到废纸篓 2 个会话");
        crate::i18n::with_language(crate::i18n::Language::English, || {
            let failed = vec![("t".to_string(), "running".to_string())];
            assert_eq!(TrashReport { moved: 2, failed }.banner(), "Moved 2, 1 failed: running");
            assert_eq!(TrashReport { moved: 2, failed: Vec::new() }.banner(), "Moved 2 sessions to Trash");
        });
    }

    /// A fake Trash: moves into `<tempdir>/Trash`, fails for paths containing `fail`.
    fn fake_trash(bin: &Path) -> impl FnMut(&Path) -> Result<(), String> + '_ {
        move |p| {
            if p.to_string_lossy().contains("fail") {
                return Err("权限不足".into());
            }
            fs::create_dir_all(bin).unwrap();
            let mut dest = bin.join(p.file_name().unwrap());
            if dest.exists() {
                // Two companions can share a basename (`file-history/<id>`, `tasks/<id>`).
                dest = bin.join(format!("{}-{}", p.parent().unwrap().file_name().unwrap().to_string_lossy(), p.file_name().unwrap().to_string_lossy()));
            }
            fs::rename(p, dest).map_err(|e| e.to_string())
        }
    }

    fn entry(agent: AgentKind, transcript: PathBuf, first_prompt: &str) -> HistoryEntry {
        HistoryEntry {
            agent,
            session_id: transcript.file_stem().unwrap().to_string_lossy().into(),
            cwd: "/Users/u/proj".into(),
            transcript,
            first_prompt: first_prompt.into(),
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

    fn write(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "{}\n").unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into()).collect();
        names.sort();
        names
    }

    #[test]
    fn every_file_of_a_session_goes_to_the_trash() {
        let tmp = tempfile::tempdir().unwrap();
        let (home, bin) = (tmp.path().join("home"), tmp.path().join("Trash"));
        let claude = home.join(".claude/projects/-Users-u-proj/c1.jsonl");
        write(&claude);
        write(&home.join(".claude/projects/-Users-u-proj/c1/subagents/agent-1.jsonl"));
        let codex = home.join(".codex/sessions/2026/09/28/rollout-x.jsonl");
        write(&codex);
        write(&home.join(".codex/sessions/2026/09/28/rollout-x.jsonl.langsmith"));
        write(&home.join(".codex/sessions/2026/09/28/rollout-y.jsonl"));
        let entries = [entry(AgentKind::Claude, claude.clone(), "a"), entry(AgentKind::Codex, codex.clone(), "b")];

        let (report, gone) = trash_with(&entries, |e| e.first_prompt.clone(), |_| false, fake_trash(&bin));
        assert_eq!(report, TrashReport { moved: 2, failed: vec![] });
        assert_eq!(report.toast(), None);
        assert_eq!(gone.len(), 2);
        assert_eq!(names(&bin), ["c1", "c1.jsonl", "rollout-x.jsonl", "rollout-x.jsonl.langsmith"]);
        assert_eq!(names(&home.join(".claude/projects/-Users-u-proj")), Vec::<String>::new());
        // Another session's rollout in the same day directory stays.
        assert_eq!(names(&home.join(".codex/sessions/2026/09/28")), ["rollout-y.jsonl"]);
    }

    #[test]
    fn running_and_failing_sessions_stay() {
        let tmp = tempfile::tempdir().unwrap();
        let (home, bin) = (tmp.path().join("home"), tmp.path().join("Trash"));
        let dir = home.join(".claude/projects/-Users-u-proj");
        let (live, ok, fail) = (dir.join("live.jsonl"), dir.join("ok.jsonl"), dir.join("fail.jsonl"));
        [&live, &ok, &fail].into_iter().for_each(|p| write(p));
        let entries = [
            entry(AgentKind::Claude, live.clone(), "还在跑的"),
            entry(AgentKind::Claude, ok.clone(), "可以删的"),
            entry(AgentKind::Claude, fail.clone(), "删不掉的"),
        ];
        let (report, gone) =
            trash_with(&entries, |e| e.first_prompt.clone(), |e| e.session_id == "live", fake_trash(&bin));
        let failed = vec![("还在跑的".to_string(), LIVE_REASON.to_string()), ("删不掉的".into(), "权限不足".into())];
        assert_eq!(report, TrashReport { moved: 1, failed });
        assert_eq!(report.toast().as_deref(), Some("已移走 1 个，2 个失败：正在运行"));
        assert_eq!(gone.iter().map(|e| e.session_id.as_str()).collect::<Vec<_>>(), ["ok"]);
        assert!(live.exists() && fail.exists() && !ok.exists());
    }

    #[test]
    fn a_failing_companion_keeps_the_session_and_its_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let (home, bin) = (tmp.path().join("home"), tmp.path().join("Trash"));
        let rollout = home.join(".codex/sessions/2026/09/28/rollout-z.jsonl");
        write(&rollout);
        write(&home.join(".codex/sessions/2026/09/28/rollout-z.jsonl.fail"));
        // Already gone (the cache was stale): nothing to move, and it leaves the lists.
        let vanished = home.join(".codex/sessions/2026/09/27/rollout-v.jsonl");
        let entries = [entry(AgentKind::Codex, rollout.clone(), "z"), entry(AgentKind::Codex, vanished, "v")];
        let (report, gone) = trash_with(&entries, |e| e.first_prompt.clone(), |_| false, fake_trash(&bin));
        assert_eq!(report, TrashReport { moved: 1, failed: vec![("z".into(), "权限不足".into())] });
        // The transcript is moved last, so it is still there and the session stays listed.
        assert_eq!(gone.iter().map(|e| e.first_prompt.as_str()).collect::<Vec<_>>(), ["v"]);
        assert!(rollout.exists());
        assert!(!bin.exists(), "nothing was moved");
    }

    #[test]
    fn companion_data_goes_first_and_the_transcript_last() {
        let tmp = tempfile::tempdir().unwrap();
        let (home, bin) = (tmp.path().join("home"), tmp.path().join("Trash"));
        let id = "0f282162-b358-47c9-9314-2b637ec43051";
        let claude = home.join(format!(".claude/projects/-Users-u-proj/{id}.jsonl"));
        write(&claude);
        write(&home.join(format!(".claude/file-history/{id}/a@v1")));
        write(&home.join(format!(".claude/tasks/{id}/1.json")));
        write(&home.join(".claude/file-history/other/keep"));
        let entries = [entry(AgentKind::Claude, claude.clone(), "a")];
        let mut order = Vec::new();
        let (report, gone) = trash_with(&entries, |e| e.first_prompt.clone(), |_| false, |p| {
            order.push(p.file_name().unwrap().to_string_lossy().into_owned());
            fake_trash(&bin)(p)
        });
        assert_eq!(report, TrashReport { moved: 1, failed: vec![] });
        assert_eq!(gone.len(), 1);
        assert_eq!(order.last().map(String::as_str), Some(format!("{id}.jsonl").as_str()));
        assert!(order.iter().take(order.len() - 1).all(|n| n == id), "{order:?}");
        assert!(home.join(".claude/file-history/other/keep").exists(), "another session's data stays");
        assert!(!home.join(format!(".claude/file-history/{id}")).exists());
    }

    #[test]
    fn a_failing_companion_keeps_the_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let (home, bin) = (tmp.path().join("home"), tmp.path().join("Trash"));
        let id = "0f282162-b358-47c9-9314-2b637ec43051";
        let claude = home.join(format!(".claude/projects/-Users-u-proj/{id}.jsonl"));
        write(&claude);
        write(&home.join(format!(".claude/tasks/{id}/1.json")));
        let entries = [entry(AgentKind::Claude, claude.clone(), "a")];
        let (report, gone) = trash_with(&entries, |e| e.first_prompt.clone(), |_| false, |p| {
            if p.to_string_lossy().contains("/tasks/") { Err("权限不足".into()) } else { fake_trash(&bin)(p) }
        });
        assert_eq!(report.failed.len(), 1);
        assert!(gone.is_empty());
        assert!(claude.exists(), "the transcript is still there, the session stays listed");
    }
}
