//! The status-bar popup menus (E2b-1 §3): the encoding menu (reopen with another encoding / read-only) and the
//! line-ending menu. Pure: the items, the key routing while a menu is open and where it is anchored; the view
//! (`EditorView::menu`) owns the open menu and `chrome::popup` draws it.

use std::path::Path;

use gilvt_editor::{common_encodings, Encoding, LineEnding, OpenOptions};
use gpui::{point, Pixels, Point};

use super::view::KeyCmd;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuKind {
    Encoding,
    LineEnding,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub label: String,
    pub checked: bool,
    pub enabled: bool,
    pub action: MenuAction,
    pub separator_before: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Encoding(OpenOptions),
    LineEnding(LineEnding),
}

/// The menu open on an editor pane: its kind, where it is anchored (its bottom-left corner, in window
/// coordinates: it opens above the status bar) and its items as built when it opened.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenMenu {
    pub kind: MenuKind,
    pub at: Point<Pixels>,
    pub items: Vec<MenuItem>,
}

/// The label of an encoding in the menu: its status-bar name, except the BOM variant of UTF-8.
fn encoding_label(e: Encoding) -> String {
    match e {
        Encoding::Utf8Bom => crate::i18n::text("UTF-8（带 BOM）", "UTF-8 (with BOM)").into(),
        e => e.name().into(),
    }
}

/// One item per `common_encodings()` (the current one checked), then, after a separator, the read-only
/// toggle. `read_only`: the buffer is read-only by choice (`forced_read_only`, which a lossy decoding also
/// sets); `explicit`: `current` was picked by hand (the toggle keeps it, a detected one is detected again);
/// `writable`: the file's real permission.
pub fn encoding_menu(current: Encoding, explicit: bool, read_only: bool, lossy: bool, writable: bool) -> Vec<MenuItem> {
    // A picked encoding keeps a manual read-only choice; a lossy buffer asks for an editable one, which E1
    // refuses (the red bar) unless that encoding round-trips.
    let keep_read_only = read_only && !lossy;
    let mut items: Vec<MenuItem> = common_encodings()
        .into_iter()
        .map(|e| MenuItem {
            label: encoding_label(e),
            checked: e == current,
            enabled: true,
            action: MenuAction::Encoding(OpenOptions { encoding: Some(e), read_only: keep_read_only }),
            separator_before: false,
        })
        .collect();
    let encoding = explicit.then_some(current);
    items.push(if read_only {
        MenuItem {
            label: crate::i18n::text("以可编辑方式打开", "Open as Editable").into(),
            checked: false,
            enabled: writable && !lossy,
            action: MenuAction::Encoding(OpenOptions { encoding, read_only: false }),
            separator_before: true,
        }
    } else {
        MenuItem {
            label: crate::i18n::text("以只读方式打开", "Open as Read Only").into(),
            checked: false,
            enabled: true,
            action: MenuAction::Encoding(OpenOptions { encoding, read_only: true }),
            separator_before: true,
        }
    });
    items
}

/// LF / CRLF / CR, the one the next save writes checked.
pub fn line_ending_menu(current: LineEnding) -> Vec<MenuItem> {
    [LineEnding::Lf, LineEnding::CrLf, LineEnding::Cr]
        .into_iter()
        .map(|le| MenuItem {
            label: le.name().into(),
            checked: le == current,
            enabled: true,
            action: MenuAction::LineEnding(le),
            separator_before: false,
        })
        .collect()
}

/// Whether a status-bar menu may open: never over the compare overlay or while only 「窗口太窄」 shows, and the
/// line-ending menu never on a read-only / lossy buffer (a line ending could never be saved there, it would only
/// leave the buffer dirty for good). The encoding menu stays: it is how a read-only buffer becomes editable.
pub(crate) fn can_open_menu(kind: &MenuKind, read_only: bool, too_narrow: bool, compare_open: bool) -> bool {
    !(too_narrow || compare_open || (*kind == MenuKind::LineEnding && read_only))
}

/// Whether choosing `chosen` in the line-ending menu changes the buffer: not when it is the current ending of a
/// uniform file (E1 leaves such a buffer clean); a mixed file is normalised either way.
pub(crate) fn line_ending_choice_changes_anything(current: LineEnding, mixed: bool, chosen: LineEnding) -> bool {
    mixed || chosen != current
}

/// The flash after a line ending was chosen.
pub(crate) fn line_ending_flash(le: LineEnding) -> String {
    if crate::i18n::current() == crate::i18n::Language::English {
        format!("Use {} when saving", le.name())
    } else {
        format!("保存时使用 {}", le.name())
    }
}

/// Whether `path` can be written, without changing it (opening for append neither truncates nor touches it).
pub(crate) fn path_is_writable(path: &Path) -> bool {
    std::fs::OpenOptions::new().append(true).open(path).is_ok()
}

/// What a key does while a menu is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PopupKey {
    /// Esc: close the menu.
    Close,
    /// Anything else: eaten (never reaches the text or a bar).
    Swallow,
}

/// `cmd`: the editor key (`key_command`), `None` for anything else (typed characters included).
pub(crate) fn popup_key(cmd: Option<KeyCmd>) -> PopupKey {
    match cmd {
        Some(KeyCmd::Escape) => PopupKey::Close,
        _ => PopupKey::Swallow,
    }
}

/// Where a menu opened by a click at `click` is anchored (its bottom-left corner): the click's x, never lower
/// than the top of the status bar (`status_top`), so the menu opens above it.
pub(crate) fn menu_anchor(click: Point<Pixels>, status_top: Pixels) -> Point<Pixels> {
    point(click.x, click.y.min(status_top))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_editor::{common_encodings, Encoding, LineEnding, OpenOptions};
    use gpui::px;

    #[test]
    fn the_encoding_menu_lists_the_common_encodings_with_the_current_one_checked() {
        let items = encoding_menu(Encoding::Utf8, false, false, false, true);
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(&labels[..2], ["UTF-8", "UTF-8（带 BOM）"]);
        assert!(labels.contains(&"GBK") && labels.contains(&"Shift_JIS"));
        assert_eq!(items.iter().filter(|i| i.checked).count(), 1);
        assert!(items[0].checked);
        assert_eq!(items.len(), common_encodings().len() + 1);
    }

    #[test]
    fn picking_an_encoding_keeps_the_manual_read_only_choice() {
        let gbk = gilvt_editor::encoding_by_label("gbk").unwrap();
        let items = encoding_menu(Encoding::Utf8, false, true, false, true);
        let gbk_item = items.iter().find(|i| i.label == "GBK").unwrap();
        assert_eq!(gbk_item.action, MenuAction::Encoding(OpenOptions { encoding: Some(gbk), read_only: true }));
    }

    #[test]
    fn the_last_item_toggles_read_only() {
        let last = encoding_menu(Encoding::Utf8, false, false, false, true).pop().unwrap();
        assert_eq!((last.label.as_str(), last.separator_before, last.enabled), ("以只读方式打开", true, true));
        let last = encoding_menu(Encoding::Utf8, false, true, false, true).pop().unwrap();
        assert_eq!(last.label, "以可编辑方式打开");
        assert!(last.enabled);
        let last = encoding_menu(Encoding::Utf8, false, true, false, false).pop().unwrap();
        assert!(!last.enabled, "the file is not writable");
        let last = encoding_menu(Encoding::Utf8, false, true, true, true).pop().unwrap();
        assert!(!last.enabled, "a lossy decoding can never be made editable");
    }

    #[test]
    fn the_line_ending_menu() {
        let items = line_ending_menu(LineEnding::CrLf);
        assert_eq!(items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["LF", "CRLF", "CR"]);
        assert_eq!(items.iter().position(|i| i.checked), Some(1));
        assert_eq!(items[2].action, MenuAction::LineEnding(LineEnding::Cr));
    }

    // ---------- beyond the brief ----------

    #[test]
    fn the_read_only_toggle_keeps_an_explicit_encoding_only() {
        let gbk = gilvt_editor::encoding_by_label("gbk").unwrap();
        let last = encoding_menu(gbk, true, false, false, true).pop().unwrap();
        assert_eq!(last.action, MenuAction::Encoding(OpenOptions { encoding: Some(gbk), read_only: true }));
        let last = encoding_menu(gbk, false, true, false, true).pop().unwrap();
        assert_eq!(last.action, MenuAction::Encoding(OpenOptions { encoding: None, read_only: false }));
        // Only the toggle has a separator above it; every encoding item is enabled.
        let items = encoding_menu(gbk, true, false, false, true);
        assert_eq!(items.iter().filter(|i| i.separator_before).count(), 1);
        assert!(items[..items.len() - 1].iter().all(|i| i.enabled && !i.separator_before));
        assert!(items.iter().find(|i| i.label == "GBK").unwrap().checked);
    }

    #[test]
    fn picking_an_encoding_of_a_lossy_buffer_asks_for_an_editable_one() {
        // The refused-file flow opens read-only + lossy; a picked encoding that round-trips makes it editable.
        let items = encoding_menu(Encoding::Utf8, false, true, true, true);
        let gbk = gilvt_editor::encoding_by_label("gbk").unwrap();
        let gbk_item = items.iter().find(|i| i.label == "GBK").unwrap();
        assert_eq!(gbk_item.action, MenuAction::Encoding(OpenOptions { encoding: Some(gbk), read_only: false }));
    }

    #[test]
    fn every_line_ending_item_is_enabled_and_has_no_separator() {
        let items = line_ending_menu(LineEnding::Lf);
        assert!(items.iter().all(|i| i.enabled && !i.separator_before));
        assert_eq!(items.iter().filter(|i| i.checked).count(), 1);
        assert!(items[0].checked);
    }

    #[test]
    fn writability_is_the_files_real_permission() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "abc").unwrap();
        assert!(path_is_writable(&p));
        assert_eq!(std::fs::read(&p).unwrap(), b"abc", "checking does not change the file");
        assert!(!path_is_writable(&dir.path().join("missing.txt")));
        // Root can write anywhere: the read-only half means nothing then.
        if unsafe { libc::geteuid() } != 0 {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o444)).unwrap();
            assert!(!path_is_writable(&p));
        }
    }

    #[test]
    fn esc_closes_a_menu_and_every_other_key_is_swallowed() {
        assert_eq!(popup_key(Some(KeyCmd::Escape)), PopupKey::Close);
        for cmd in [Some(KeyCmd::Enter), Some(KeyCmd::Backspace), Some(KeyCmd::Tab(false)), Some(KeyCmd::Rows(1, false)), None] {
            assert_eq!(popup_key(cmd), PopupKey::Swallow, "{cmd:?}");
        }
    }

    #[test]
    fn a_menu_opens_above_the_status_bar() {
        // A click on the status bar (below its top) is lifted to the top; the x stays.
        assert_eq!(menu_anchor(point(px(120.), px(590.)), px(580.)), point(px(120.), px(580.)));
        assert_eq!(menu_anchor(point(px(120.), px(500.)), px(580.)), point(px(120.), px(500.)));
    }

    #[test]
    fn a_line_ending_menu_never_opens_on_a_read_only_buffer() {
        assert!(can_open_menu(&MenuKind::LineEnding, false, false, false));
        assert!(!can_open_menu(&MenuKind::LineEnding, true, false, false));
        // The encoding menu stays: it is how a read-only buffer becomes editable.
        assert!(can_open_menu(&MenuKind::Encoding, true, false, false));
    }

    #[test]
    fn no_menu_opens_over_the_compare_overlay_or_when_too_narrow() {
        for kind in [MenuKind::Encoding, MenuKind::LineEnding] {
            assert!(!can_open_menu(&kind, false, true, false));
            assert!(!can_open_menu(&kind, false, false, true));
        }
    }

    #[test]
    fn choosing_a_line_ending_changes_something_unless_it_is_the_current_one_of_a_uniform_file() {
        assert!(!line_ending_choice_changes_anything(LineEnding::Lf, false, LineEnding::Lf));
        assert!(line_ending_choice_changes_anything(LineEnding::Lf, false, LineEnding::CrLf));
        // A mixed file is normalised even by choosing the current ending.
        assert!(line_ending_choice_changes_anything(LineEnding::Lf, true, LineEnding::Lf));
    }

    #[test]
    fn the_flash_after_choosing_names_the_ending() {
        assert_eq!(line_ending_flash(LineEnding::CrLf), "保存时使用 CRLF");
    }
}
