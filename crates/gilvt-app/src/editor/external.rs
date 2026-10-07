//! External-change decision (E2b-1 §4): what the editor does when the file on disk changed under it.
//! Pure; the view feeds it `Buffer::check_external()`, the dirty flag, the current bar and the state the user
//! last dismissed.

use super::view::Bar;
use gilvt_editor::ExternalState;

/// What the view should do about a file-change notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalAction {
    Nothing,
    /// Clean buffer, file changed: take the disk version (E1 `reload`: history cleared, caret kept) and flash
    /// 「已更新」.
    SilentReload,
    /// Dirty buffer, file changed: put up the 「重新载入 / 对比 / 仍然覆盖」 bar.
    ShowModified,
    /// File gone: put up the deleted bar.
    ShowDeleted,
}

/// Shown once when the file on disk outgrew `MAX_FILE_SIZE` under a clean buffer (a silent reload would fail).
pub const TOO_LARGE_NOTICE: &str = "磁盘上的文件已超过 64 MB，编辑器不再跟随它的修改。";

/// The red bar to put up for a disk file of `disk_len` bytes, or `None` when it is within the limit or the
/// same notice is already showing.
pub fn too_large_notice(disk_len: u64, current_bar: &Bar) -> Option<Bar> {
    if disk_len <= gilvt_editor::MAX_FILE_SIZE {
        return None;
    }
    if matches!(current_bar, Bar::SaveError { message, jump: None } if message == TOO_LARGE_NOTICE) {
        return None;
    }
    Some(Bar::SaveError { message: TOO_LARGE_NOTICE.into(), jump: None })
}

/// After a bar is hidden with the pane staying open, the disk is looked at again when the hidden bar was one
/// that suppressed every decision (the close confirmation or a pending reopen confirmation).
pub fn recheck_after_hide(hidden: &Bar) -> bool {
    matches!(hidden, Bar::Close | Bar::Confirm(_))
}

/// Debounce between the first watcher event of a burst and the check, in milliseconds.
pub const DEBOUNCE_MS: u64 = 150;
/// How long the 「已更新」 flash stays, in milliseconds.
pub const FLASH_MS: u64 = 2000;
pub const FLASH_UPDATED: &str = "已更新";

/// E2b-1 §4: the close-confirmation bar (and a pending reopen confirmation) always wins (decided again after it
/// goes away); an existing `Modified` (重新载入 / 对比 / 仍然覆盖) / `Deleted` bar is not re-raised; a save-error
/// bar may be replaced. `acked` is the disk state whose bar the user dismissed (取消 / Esc / 知道了): while the
/// disk is still in that state the bar is not shown again (a save stays blocked by `ModifiedOnDisk`). A dismissed
/// `Modified` only holds while the buffer is dirty: a clean buffer is always reloaded silently.
pub fn decide(state: ExternalState, dirty: bool, bar: &Bar, acked: Option<ExternalState>) -> ExternalAction {
    let dismissed = acked == Some(state) && (state == ExternalState::Deleted || dirty);
    if dismissed {
        return ExternalAction::Nothing;
    }
    match state {
        ExternalState::Unchanged => ExternalAction::Nothing,
        ExternalState::Modified => match bar {
            Bar::Close | Bar::Confirm(_) => ExternalAction::Nothing,
            _ if !dirty => ExternalAction::SilentReload,
            Bar::Modified { .. } => ExternalAction::Nothing,
            _ => ExternalAction::ShowModified,
        },
        ExternalState::Deleted => match bar {
            Bar::Close | Bar::Deleted | Bar::Confirm(_) => ExternalAction::Nothing,
            _ => ExternalAction::ShowDeleted,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::view::ReopenIntent;
    use gilvt_editor::ExternalState::*;
    use gilvt_editor::OpenOptions;

    #[test]
    fn too_large_notice_decisions() {
        let limit = gilvt_editor::MAX_FILE_SIZE;
        assert_eq!(too_large_notice(limit, &Bar::None), None);
        assert_eq!(too_large_notice(0, &Bar::None), None);
        let shown = too_large_notice(limit + 1, &Bar::None).expect("above the limit");
        assert!(matches!(&shown, Bar::SaveError { message, jump: None } if message == TOO_LARGE_NOTICE));
        assert_eq!(too_large_notice(limit + 1, &shown), None, "already showing");
        let other = Bar::SaveError { message: "x".into(), jump: None };
        assert!(too_large_notice(limit + 1, &other).is_some());
    }
    #[test]
    fn recheck_after_hide_only_for_close_and_confirm() {
        assert!(recheck_after_hide(&Bar::Close));
        assert!(recheck_after_hide(&Bar::Confirm(ReopenIntent(OpenOptions::default()))));
        assert!(!recheck_after_hide(&Bar::None));
        assert!(!recheck_after_hide(&Bar::Deleted));
        assert!(!recheck_after_hide(&Bar::Modified { closing: false }));
        assert!(!recheck_after_hide(&Bar::SaveError { message: "x".into(), jump: None }));
    }
    #[test]
    fn unchanged_does_nothing() {
        assert_eq!(decide(Unchanged, true, &Bar::None, None), ExternalAction::Nothing);
    }
    #[test]
    fn a_clean_buffer_reloads_silently() {
        assert_eq!(decide(Modified, false, &Bar::None, None), ExternalAction::SilentReload);
    }
    #[test]
    fn a_dirty_buffer_shows_the_banner_once() {
        assert_eq!(decide(Modified, true, &Bar::None, None), ExternalAction::ShowModified);
        assert_eq!(decide(Modified, true, &Bar::Modified { closing: false }, None), ExternalAction::Nothing);
    }
    #[test]
    fn the_close_bar_wins_over_everything() {
        assert_eq!(decide(Modified, true, &Bar::Close, None), ExternalAction::Nothing);
        assert_eq!(decide(Deleted, true, &Bar::Close, None), ExternalAction::Nothing);
    }
    #[test]
    fn a_save_error_bar_is_replaced() {
        let e = Bar::SaveError { message: "x".into(), jump: None };
        assert_eq!(decide(Modified, true, &e, None), ExternalAction::ShowModified);
        assert_eq!(decide(Deleted, false, &e, None), ExternalAction::ShowDeleted);
    }
    #[test]
    fn deleted_shows_once_dirty_or_not() {
        assert_eq!(decide(Deleted, false, &Bar::None, None), ExternalAction::ShowDeleted);
        assert_eq!(decide(Deleted, true, &Bar::Deleted, None), ExternalAction::Nothing);
    }
    #[test]
    fn a_pending_reopen_confirmation_is_never_dropped() {
        let c = Bar::Confirm(ReopenIntent(OpenOptions::default()));
        assert_eq!(decide(Modified, true, &c, None), ExternalAction::Nothing);
        assert_eq!(decide(Modified, false, &c, None), ExternalAction::Nothing);
        assert_eq!(decide(Deleted, true, &c, None), ExternalAction::Nothing);
    }

    #[test]
    fn a_dismissed_state_stays_quiet_until_it_changes() {
        assert_eq!(decide(Modified, true, &Bar::None, Some(Modified)), ExternalAction::Nothing);
        // A clean buffer is always reloaded: the memory only keeps a dirty buffer's banner down.
        assert_eq!(decide(Modified, false, &Bar::None, Some(Modified)), ExternalAction::SilentReload);
        assert_eq!(decide(Deleted, true, &Bar::None, Some(Deleted)), ExternalAction::Nothing);
        // A different state than the dismissed one is decided as usual.
        assert_eq!(decide(Deleted, true, &Bar::None, Some(Modified)), ExternalAction::ShowDeleted);
        assert_eq!(decide(Modified, true, &Bar::None, Some(Deleted)), ExternalAction::ShowModified);
    }

    /// The whole truth table, written out by hand (rows: acked, state, dirty; columns: bar).
    #[test]
    fn the_whole_truth_table() {
        use ExternalAction::{Nothing as N, ShowDeleted as D, ShowModified as M, SilentReload as R};
        let intent = ReopenIntent(OpenOptions::default());
        let err = || Bar::SaveError { message: "x".into(), jump: None };
        // Order of bars: None, Close, Modified{false}, Modified{true}, Deleted, Confirm, SaveError.
        let bars = [Bar::None, Bar::Close, Bar::Modified { closing: false }, Bar::Modified { closing: true }, Bar::Deleted, Bar::Confirm(intent), err()];
        let table: [(Option<ExternalState>, ExternalState, bool, [ExternalAction; 7]); 18] = [
            // Nothing dismissed.
            (None, Unchanged, false, [N, N, N, N, N, N, N]),
            (None, Unchanged, true, [N, N, N, N, N, N, N]),
            (None, Modified, false, [R, N, R, R, R, N, R]),
            (None, Modified, true, [M, N, N, N, M, N, M]),
            (None, Deleted, false, [D, N, D, D, N, N, D]),
            (None, Deleted, true, [D, N, D, D, N, N, D]),
            // A Modified bar was dismissed: silent only while dirty (a clean buffer still reloads), Deleted as without memory.
            (Some(Modified), Unchanged, false, [N, N, N, N, N, N, N]),
            (Some(Modified), Unchanged, true, [N, N, N, N, N, N, N]),
            (Some(Modified), Modified, false, [R, N, R, R, R, N, R]),
            (Some(Modified), Modified, true, [N, N, N, N, N, N, N]),
            (Some(Modified), Deleted, false, [D, N, D, D, N, N, D]),
            (Some(Modified), Deleted, true, [D, N, D, D, N, N, D]),
            // A Deleted bar was dismissed: Deleted is silent, Modified as without memory.
            (Some(Deleted), Unchanged, false, [N, N, N, N, N, N, N]),
            (Some(Deleted), Unchanged, true, [N, N, N, N, N, N, N]),
            (Some(Deleted), Modified, false, [R, N, R, R, R, N, R]),
            (Some(Deleted), Modified, true, [M, N, N, N, M, N, M]),
            (Some(Deleted), Deleted, false, [N, N, N, N, N, N, N]),
            (Some(Deleted), Deleted, true, [N, N, N, N, N, N, N]),
        ];
        for (acked, state, dirty, expected) in table {
            for (bar, want) in bars.iter().zip(expected) {
                assert_eq!(decide(state, dirty, bar, acked), want, "acked={acked:?} {state:?} dirty={dirty} bar={bar:?}");
            }
        }
    }
}
