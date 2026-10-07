//! Pure state, key map and time arithmetic of the read-only session review.

use std::collections::HashSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gilvt_agent::{ReviewPage, SessionKey, TurnCursor};

/// Window width from which the review sits beside the queue instead of replacing it.
const WIDE_FROM: f32 = 960.;
/// The palette width the Session Center uses while no review is open.
pub const BASE_WIDTH: f32 = 640.;
const WIDE_MAX: f32 = 1040.;
/// The palette frame keeps this much window on both sides together.
const WINDOW_MARGIN: f32 = 48.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Queue and review side by side.
    Wide,
    /// One page at a time: the review replaces the queue until the user goes back.
    Narrow,
}

pub fn layout(viewport_width: f32) -> Layout {
    if viewport_width >= WIDE_FROM {
        Layout::Wide
    } else {
        Layout::Narrow
    }
}

pub fn frame_width(viewport_width: f32, reviewing: bool) -> f32 {
    if reviewing && layout(viewport_width) == Layout::Wide {
        (viewport_width - WINDOW_MARGIN).min(WIDE_MAX)
    } else {
        BASE_WIDTH
    }
}

/// Which part of the Session Center owns the keyboard. Letters only drive the review while one is open,
/// because in the queue they are typed into the search box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    List,
    Detail,
    SnoozeMenu,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnoozeChoice {
    Hour,
    Later,
    Tomorrow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paging {
    Earlier,
    Later,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Open the selected session's read-only review.
    Open,
    ReviewNext,
    /// 「标记已 Review 并归档」: like `ReviewNext`, then the session is archived.
    ReviewNextAndArchive,
    Skip,
    OpenSnoozeMenu,
    CloseSnoozeMenu,
    Snooze(SnoozeChoice),
    Pin,
    ToggleFullHistory,
    /// Stale position: start reviewing from the last turn that exists now (older turns count as seen).
    BaselineHere,
    /// Stale position: review every turn that is still in the transcript.
    ReviewAllVisible,
    Page(Paging),
    /// Scroll the review by a direction (-1 up, 1 down).
    Scroll(i8),
    BackToAgent,
    Interrupt,
    Terminate,
    CopyDiagnostics,
    /// Leave the review and show the queue.
    Back,
    /// Close the Session Center.
    Close,
}

/// The modifier keys held with a keystroke. Shifted letters still arrive as their base key, so `shift` only
/// matters for ⇧⌘E.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub command: bool,
    pub alt: bool,
    pub control: bool,
    pub shift: bool,
}

#[cfg(test)]
impl Mods {
    pub const NONE: Mods = Mods {
        command: false,
        alt: false,
        control: false,
        shift: false,
    };
    pub const CMD: Mods = Mods {
        command: true,
        alt: false,
        control: false,
        shift: false,
    };
}

pub fn key_action(scope: Scope, key: &str, mods: Mods) -> Option<Action> {
    // ⌘S / ⌘Z / ⌘P and friends belong to the app (or the user's muscle memory), never to the review's
    // letters; only ⌘↩ is a modified review key.
    let archive_chord = mods.command && mods.shift && key == "e";
    if mods.alt || mods.control || (mods.command && key != "enter" && !archive_chord) {
        return None;
    }
    let command = mods.command;
    if scope == Scope::SnoozeMenu {
        return match key {
            "1" => Some(Action::Snooze(SnoozeChoice::Hour)),
            "2" => Some(Action::Snooze(SnoozeChoice::Later)),
            "3" => Some(Action::Snooze(SnoozeChoice::Tomorrow)),
            "escape" => Some(Action::CloseSnoozeMenu),
            _ => None,
        };
    }
    if archive_chord {
        return (scope == Scope::Detail).then_some(Action::ReviewNextAndArchive);
    }
    if key == "enter" {
        return Some(if command {
            Action::ReviewNext
        } else {
            Action::BackToAgent
        });
    }
    match (scope, key) {
        (Scope::List, "space") => Some(Action::Open),
        (Scope::List, "escape") => Some(Action::Close),
        (Scope::Detail, "escape") => Some(Action::Back),
        (Scope::Detail, "s") => Some(Action::Skip),
        (Scope::Detail, "z") => Some(Action::OpenSnoozeMenu),
        (Scope::Detail, "p") => Some(Action::Pin),
        (Scope::Detail, "f") => Some(Action::ToggleFullHistory),
        (Scope::Detail, "b") => Some(Action::BaselineHere),
        (Scope::Detail, "a") => Some(Action::ReviewAllVisible),
        // E / L work in every input source; with the Pinyin input method the bracket keys arrive as the
        // fullwidth 【 】, so those count too.
        (Scope::Detail, "e" | "[" | "【") => Some(Action::Page(Paging::Earlier)),
        (Scope::Detail, "l" | "]" | "】") => Some(Action::Page(Paging::Later)),
        (Scope::Detail, "up") => Some(Action::Scroll(-1)),
        (Scope::Detail, "down") => Some(Action::Scroll(1)),
        _ => None,
    }
}

/// 「标记已 Review 并归档」 acts: "reviewed, next" would act (`ready`) and the session is not running in gilvt
/// (archiving a running session is refused everywhere).
pub fn can_review_and_archive(ready: bool, running_in_gilvt: bool) -> bool {
    ready && !running_in_gilvt
}

/// The session to show after the current one was reviewed or snoozed: the row after it in queue order,
/// wrapping to the first, never the current session itself (it may re-enter the queue with newer turns).
pub fn next_key(keys: &[SessionKey], current: &SessionKey) -> Option<SessionKey> {
    match keys.iter().position(|key| key == current) {
        Some(position) if keys.len() > 1 => Some(keys[(position + 1) % keys.len()].clone()),
        Some(_) => None,
        None => keys.first().cloned(),
    }
}

/// `utc_offset_secs` is the local time zone's offset from UTC.
pub fn snooze_until(choice: SnoozeChoice, now: SystemTime, utc_offset_secs: i64) -> SystemTime {
    const HOUR: i64 = 3600;
    const DAY: i64 = 24 * HOUR;
    let now_secs = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    let local_now = now_secs + utc_offset_secs;
    let local_midnight = local_now - local_now.rem_euclid(DAY);
    let to_system = |local: i64| {
        let utc = (local - utc_offset_secs).max(0) as u64;
        UNIX_EPOCH + Duration::from_secs(utc)
    };
    match choice {
        SnoozeChoice::Hour => now + Duration::from_secs(HOUR as u64),
        SnoozeChoice::Later => {
            let evening = local_midnight + 18 * HOUR;
            if evening - local_now >= HOUR {
                to_system(evening)
            } else {
                now + Duration::from_secs(2 * HOUR as u64)
            }
        }
        SnoozeChoice::Tomorrow => to_system(local_midnight + DAY + 9 * HOUR),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Only turns after the review cursor.
    #[default]
    Incremental,
    /// The whole transcript, newest page first.
    Full,
}

pub fn open_page(mode: Mode, reviewed_through: Option<&TurnCursor>) -> ReviewPage {
    match mode {
        Mode::Incremental => ReviewPage::After {
            cursor: reviewed_through.cloned(),
        },
        Mode::Full => ReviewPage::Latest,
    }
}

/// Like [`open_page`], but a saved position that is no longer in the transcript (`stale`) cannot anchor an
/// "after" page: show the newest turns instead so the recovery actions have something to act on.
pub fn open_page_for(mode: Mode, reviewed_through: Option<&TurnCursor>, stale: bool) -> ReviewPage {
    if stale {
        ReviewPage::Latest
    } else {
        open_page(mode, reviewed_through)
    }
}

/// What to tell the user when saving a review change failed. Only a real I/O error is a "save failure";
/// the store reports a rewritten transcript as `InvalidInput` and a stale saved position as `InvalidData`.
pub fn save_notice(kind: std::io::ErrorKind, what: &str, detail: &str) -> String {
    if crate::i18n::current() == crate::i18n::Language::English {
        return match kind {
            std::io::ErrorKind::InvalidInput => format!("Did not {what}: the session record changed during review. Reopen it and try again."),
            std::io::ErrorKind::InvalidData => format!("Did not {what}: the saved review position is stale. Choose \"Start from Here\" or \"Review All Visible History\"."),
            _ => format!("Save failed; did not {what}: {detail}"),
        };
    }
    match kind {
        std::io::ErrorKind::InvalidInput => format!(
            "没有{what}：这个会话的记录在 Review 期间被改写了，请重新打开它"
        ),
        std::io::ErrorKind::InvalidData => format!(
            "没有{what}：这个会话的 Review 位置已失效，请选择「从当前开始」或「Review 全部可见历史」"
        ),
        _ => format!("保存失败，没有{what}：{detail}"),
    }
}

/// The first `max_lines` lines of `text` as one string (so it lays out as one element), and how many lines
/// were left out.
pub fn capped_text(text: &str, max_lines: usize) -> (String, usize) {
    let total = text.lines().count();
    let kept = text.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    (kept, total.saturating_sub(max_lines))
}

pub fn turn_page(direction: Paging, first: &TurnCursor, last: &TurnCursor) -> ReviewPage {
    match direction {
        Paging::Earlier => ReviewPage::Before {
            cursor: first.clone(),
        },
        Paging::Later => ReviewPage::After {
            cursor: Some(last.clone()),
        },
    }
}

/// UI state of the open review. Opening, scrolling or closing never touches the persisted cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Surface {
    pub key: SessionKey,
    pub mode: Mode,
    pub snooze_menu: bool,
    /// The session's last completed turn when the review was opened (from the queue row the user opened).
    /// "Reviewed, next" saves this, so a turn that finished afterwards, or a page reloaded from a newer
    /// history refresh, stays unreviewed.
    pub opened_through: Option<TurnCursor>,
    expanded: HashSet<TurnCursor>,
}

impl Surface {
    pub fn new(key: SessionKey) -> Self {
        Surface {
            key,
            mode: Mode::Incremental,
            snooze_menu: false,
            opened_through: None,
            expanded: HashSet::new(),
        }
    }

    /// The turn that "reviewed, next" advances to, or None when it must not: the shown document is another
    /// session's, more turns follow the shown page (confirming would mark turns that were never shown), or
    /// the saved position is stale (that needs the recovery actions).
    pub fn review_target(
        &self,
        document: &SessionKey,
        document_snapshot_through: &TurnCursor,
        has_later: bool,
        stale: bool,
    ) -> Option<TurnCursor> {
        if &self.key != document || has_later || stale {
            return None;
        }
        Some(
            self.opened_through
                .clone()
                .unwrap_or_else(|| document_snapshot_through.clone()),
        )
    }

    pub fn scope(&self) -> Scope {
        if self.snooze_menu {
            Scope::SnoozeMenu
        } else {
            Scope::Detail
        }
    }

    pub fn is_expanded(&self, turn: &TurnCursor) -> bool {
        self.expanded.contains(turn)
    }

    pub fn toggle_expanded(&mut self, turn: TurnCursor) {
        if !self.expanded.remove(&turn) {
            self.expanded.insert(turn);
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.expanded.clear();
    }

    /// Switches to another session with a clean slate.
    pub fn reopen(&mut self, key: SessionKey) {
        *self = Surface::new(key);
    }
}
