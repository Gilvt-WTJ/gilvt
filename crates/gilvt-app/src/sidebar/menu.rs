//! The right-click menu of a sidebar row (spec §4.1): 重命名… / 静音这个会话的通知 / 复制会话 ID; an
//! 已结束 row adds 恢复 / 在右侧恢复 / 归档 / 移到废纸篓… (M3c §6). A small popover anchored at the click; a click
//! elsewhere or Esc closes it.

use gilvt_agent::SessionKey;
use gpui::{anchored, deferred, div, prelude::*, px, AnyElement, Context, MouseDownEvent, Pixels, Point, SharedString, Window};

use crate::agents::Agents;
use crate::debug_state::rects::{self, RectId};
use crate::launcher::Location;
use crate::workspace::Workspace;

#[derive(Clone, Debug, PartialEq)]
pub struct SessionMenu {
    pub key: SessionKey,
    /// Window coordinates of the right click.
    pub at: Point<Pixels>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    /// 已结束 only: resume it (恢复 = the smart location of ↩, 在右侧恢复 = ⌘↩).
    Resume(Location),
    Rename,
    ToggleMute,
    CopyId,
    /// 已结束 only: 归档.
    Archive,
    /// 已结束 only: 移到废纸篓…, after the confirm bar.
    Trash,
}

/// Items and their labels; the mute item is checked while the session is muted. A live session's menu is
/// the M3a one; an ended one's adds the resume items first and 移到废纸篓… last.
pub fn items(muted: bool, ended: bool) -> Vec<(MenuCommand, &'static str, bool)> {
    let mut items = Vec::new();
    if ended {
        items.push((MenuCommand::Resume(Location::Smart), "恢复", false));
        items.push((MenuCommand::Resume(Location::Right), "在右侧恢复", false));
    }
    items.push((MenuCommand::Rename, "重命名…", false));
    items.push((MenuCommand::ToggleMute, "静音这个会话的通知", muted));
    items.push((MenuCommand::CopyId, "复制会话 ID", false));
    if ended {
        items.push((MenuCommand::Archive, "归档", false));
        items.push((MenuCommand::Trash, "移到废纸篓…", false));
    }
    items
}

/// The groups a separator sets apart: resume, the M3a items, the Trash.
fn group(cmd: MenuCommand) -> u8 {
    match cmd {
        MenuCommand::Resume(_) => 0,
        MenuCommand::Rename | MenuCommand::ToggleMute | MenuCommand::CopyId => 1,
        MenuCommand::Archive | MenuCommand::Trash => 2,
    }
}

/// A separator goes above item `i` when it starts a new group.
pub fn separator_before(items: &[(MenuCommand, &'static str, bool)], i: usize) -> bool {
    i > 0 && group(items[i].0) != group(items[i - 1].0)
}

pub fn render(menu: &SessionMenu, _window: &Window, cx: &mut Context<Workspace>) -> AnyElement {
    let theme = crate::theme::current(cx);
    let (ui, h) = (&theme.ui, crate::theme::hsla);
    let (bg, border, text, hover, hover_text) = (h(ui.raised), h(ui.border_strong), h(ui.text), h(ui.accent), h(ui.on_accent));
    let (rule, danger) = (h(ui.border), h(ui.error.fg));
    let session = cx.global::<Agents>().registry().get(&menu.key);
    let (muted, ended) = (session.is_some_and(|s| s.muted), session.is_some_and(|s| !s.is_live()));
    let mut list = div()
        .id("session-menu")
        .occlude()
        .min_w(px(190.))
        .py(px(4.))
        .rounded(px(8.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .shadow_lg()
        .text_size(px(12.5))
        .text_color(text)
        .on_mouse_down_out(cx.listener(|ws, e: &MouseDownEvent, window, cx| {
            // The right click that opens a menu also reaches the one it replaces.
            if ws.session_menu().is_some_and(|m| m.at != e.position) {
                ws.close_session_menu(window, cx);
            }
        }));
    let items = items(muted, ended);
    for (i, &(cmd, label, checked)) in items.iter().enumerate() {
        if separator_before(&items, i) {
            list = list.child(div().my(px(3.)).h(px(1.)).bg(rule));
        }
        let key = menu.key.clone();
        let label: SharedString = label.into();
        list = list.child(
            div()
                .id(("session-menu-item", i))
                .relative()
                .children(rects::recorder(RectId::SessionMenuItem(i)))
                .flex()
                .gap(px(6.))
                .mx(px(4.))
                .px(px(8.))
                .py(px(3.))
                .rounded(px(4.))
                .when(cmd == MenuCommand::Trash, |d| d.text_color(danger))
                .hover(|s| s.bg(hover).text_color(hover_text))
                .child(div().w(px(10.)).child(if checked { "✓" } else { "" }))
                .child(label)
                .on_click(cx.listener(move |ws, _, window, cx| ws.menu_command(cmd, key.clone(), window, cx))),
        );
    }
    deferred(anchored().position(menu.at).snap_to_window_with_margin(px(8.)).child(list)).with_priority(1).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_items_follow_the_spec() {
        let labels: Vec<_> = items(false, false).iter().map(|i| i.1).collect();
        assert_eq!(labels, ["重命名…", "静音这个会话的通知", "复制会话 ID"], "live sessions keep the M3a menu");
        assert!(!items(false, false)[1].2);
        assert!(items(true, false)[1].2, "checked while muted");
        assert!((0..3).all(|i| !separator_before(&items(false, false), i)));
    }

    #[test]
    fn ended_sessions_add_resume_archive_and_trash() {
        let ended = items(true, true);
        let labels: Vec<_> = ended.iter().map(|i| i.1).collect();
        assert_eq!(labels, ["恢复", "在右侧恢复", "重命名…", "静音这个会话的通知", "复制会话 ID", "归档", "移到废纸篓…"]);
        assert_eq!(ended[0].0, MenuCommand::Resume(Location::Smart));
        assert_eq!(ended[1].0, MenuCommand::Resume(Location::Right));
        assert_eq!(ended[5].0, MenuCommand::Archive);
        assert_eq!(ended[6].0, MenuCommand::Trash);
        assert!(ended[3].2, "checked while muted");
        let separated: Vec<usize> = (0..ended.len()).filter(|&i| separator_before(&ended, i)).collect();
        assert_eq!(separated, [2, 5], "above 重命名… and 归档 (which sits right before 移到废纸篓…)");
    }
}
