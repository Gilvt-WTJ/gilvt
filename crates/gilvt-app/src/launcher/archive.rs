//! Archiving past sessions (spec §2) as one action shared by the 会话 palette, the sidebar's menu and the
//! review: every session is checked once more for running right before it is archived, and a running one is
//! skipped (it is counted, so the caller can say so).

use std::time::SystemTime;

use gilvt_agent::{HistoryEntry, Session, SessionKey};
use gpui::App;

use crate::agents::Agents;
use crate::review::ReviewService;

/// The outcome of [`archive_sessions`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArchiveReport {
    pub archived: usize,
    /// Sessions left alone because they are running.
    pub skipped_running: usize,
    /// The last save error, if any session could not be saved.
    pub failed: Option<String>,
}

impl ArchiveReport {
    /// The banner for the action: None when there was nothing to say (nothing was given).
    pub fn toast(&self) -> Option<String> {
        let english = crate::i18n::english();
        if let Some(err) = &self.failed {
            return Some(save_failed(err));
        }
        if self.archived == 0 {
            return (self.skipped_running > 0).then(|| running_not_archived().to_string());
        }
        let mut text = if english {
            format!("Archived {} sessions", self.archived)
        } else {
            format!("已归档 {} 个会话", self.archived)
        };
        if self.skipped_running > 0 {
            text.push_str(&if english {
                format!(", skipped {} running", self.skipped_running)
            } else {
                format!("，跳过 {} 个运行中的", self.skipped_running)
            });
        }
        Some(text)
    }
}

/// The banner when an archive state could not be saved.
pub fn save_failed(err: impl std::fmt::Display) -> String {
    if crate::i18n::english() { format!("Could not save: {err}") } else { format!("无法保存：{err}") }
}

/// The banner when every target is running.
pub fn running_not_archived() -> &'static str {
    crate::i18n::text("运行中的会话不能归档", "Running sessions cannot be archived")
}

/// Why nothing could be saved: no state directory.
pub fn state_dir_unavailable() -> &'static str {
    crate::i18n::text("状态目录不可用", "State directory unavailable")
}

/// The pure part: running sessions are skipped, every other one is handed to `save`.
pub fn archive_with(
    entries: &[HistoryEntry],
    is_live: impl Fn(&HistoryEntry) -> bool,
    mut save: impl FnMut(&HistoryEntry) -> Result<(), String>,
) -> ArchiveReport {
    let mut report = ArchiveReport::default();
    for entry in entries {
        if is_live(entry) {
            report.skipped_running += 1;
            continue;
        }
        match save(entry) {
            Ok(()) => report.archived += 1,
            Err(err) => report.failed = Some(err),
        }
    }
    report
}

/// Archives `entries` as of their current turn count; a later turn brings a session back (spec §2).
pub fn archive_sessions(entries: &[HistoryEntry], cx: &mut App) -> ArchiveReport {
    if !cx.has_global::<ReviewService>() {
        return ArchiveReport {
            failed: Some(state_dir_unavailable().into()),
            ..ArchiveReport::default()
        };
    }
    let live: Vec<bool> = {
        let registry = cx.try_global::<Agents>().map(Agents::registry);
        entries
            .iter()
            .map(|e| {
                let key: SessionKey = (e.agent, e.session_id.clone());
                registry
                    .and_then(|r| r.get(&key))
                    .is_some_and(Session::is_live)
            })
            .collect()
    };
    let now = SystemTime::now();
    let by_ptr = |e: &HistoryEntry| {
        entries
            .iter()
            .position(|other| std::ptr::eq(other, e))
            .is_some_and(|i| live[i])
    };
    let service = cx.global_mut::<ReviewService>();
    archive_with(entries, by_ptr, |e| {
        service
            .set_archived(&(e.agent, e.session_id.clone()), e.turns, now)
            .map_err(|err| err.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_agent::AgentKind;
    use std::path::PathBuf;

    #[test]
    fn toasts_read_in_english() {
        let report = |archived, skipped_running, failed: Option<&str>| ArchiveReport { archived, skipped_running, failed: failed.map(Into::into) };
        assert_eq!(report(2, 1, None).toast().as_deref(), Some("已归档 2 个会话，跳过 1 个运行中的"));
        crate::i18n::with_language(crate::i18n::Language::English, || {
            assert_eq!(report(2, 1, None).toast().as_deref(), Some("Archived 2 sessions, skipped 1 running"));
            assert_eq!(report(0, 1, None).toast().as_deref(), Some("Running sessions cannot be archived"));
            assert_eq!(report(0, 0, Some(state_dir_unavailable())).toast().as_deref(), Some("Could not save: State directory unavailable"));
        });
    }

    fn entry(id: &str) -> HistoryEntry {
        HistoryEntry {
            agent: AgentKind::Claude,
            session_id: id.into(),
            cwd: PathBuf::new(),
            transcript: PathBuf::new(),
            first_prompt: String::new(),
            topic_prompt: String::new(),
            custom_title: None,
            ai_title: None,
            started: None,
            last_active: SystemTime::UNIX_EPOCH,
            turns: 2,
            model: None,
            size: 0,
        }
    }

    #[test]
    fn running_sessions_are_skipped_and_counted() {
        let entries = [entry("a"), entry("b"), entry("c")];
        let mut saved = vec![];
        let report = archive_with(
            &entries,
            |e| e.session_id == "b",
            |e| {
                saved.push(e.session_id.clone());
                Ok(())
            },
        );
        assert_eq!(saved, ["a", "c"]);
        assert_eq!(
            report,
            ArchiveReport {
                archived: 2,
                skipped_running: 1,
                failed: None
            }
        );
        assert_eq!(
            report.toast().as_deref(),
            Some("已归档 2 个会话，跳过 1 个运行中的")
        );
    }

    #[test]
    fn toasts_for_nothing_archived_and_for_failures() {
        let only_running = ArchiveReport {
            archived: 0,
            skipped_running: 1,
            failed: None,
        };
        assert_eq!(
            only_running.toast().as_deref(),
            Some("运行中的会话不能归档")
        );
        assert_eq!(ArchiveReport::default().toast(), None);
        let failed = archive_with(&[entry("a")], |_| false, |_| Err("磁盘已满".into()));
        assert_eq!(failed.toast().as_deref(), Some("无法保存：磁盘已满"));
        assert_eq!(
            archive_with(&[entry("a")], |_| false, |_| Ok(()))
                .toast()
                .as_deref(),
            Some("已归档 1 个会话")
        );
    }
}
