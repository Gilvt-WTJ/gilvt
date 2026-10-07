//! The editor pane's chrome (E2a §3): the 26px header, the one notice bar under it, and the 22px status bar.
//! Plain elements outside the text area; the pure text helpers are unit-tested.

use std::path::Path;

use gpui::{
    anchored, canvas, deferred, div, prelude::*, px, AnyElement, ClickEvent, Context, Corner, CursorStyle, Div, FontWeight, Hsla,
    MouseButton, MouseDownEvent, Pixels, SharedString, Stateful,
};
use gilvt_editor::LineEnding;
use gilvt_term::Palette;
use gilvt_viewer::diff::DiffLine;

use super::compare::{self, CompareRows, LineKind, Row, SplitRow};
use super::popup::MenuKind;
use super::view::{compare_line_h, Bar, BarAction, EditorView};
use crate::debug_state::rects::{self, RectId};
use crate::launcher::{label_home, shorten_dir};
use crate::theme::{hsla, mix, AppSettings};

pub(crate) const HEADER_H: f32 = 26.;
pub(crate) const STATUS_H: f32 = 22.;
/// Below this many text columns only the header and 「窗口太窄」 are shown.
pub(crate) const MIN_TEXT_COLS: usize = 10;
pub(crate) const TOO_NARROW: &str = "窗口太窄";
pub(crate) const MODIFIED_TEXT: &str = "⚠ 文件已在磁盘上被修改，而你有未保存的改动。";
pub(crate) const DELETED_TEXT: &str = "⚠ 文件已被删除或移走。";
pub(crate) const CONFIRM_TEXT: &str = "重新打开会放弃未保存的改动。";
pub(crate) const JUMP_LABEL: &str = "跳到出错位置";
const CHROME_TEXT: f32 = 12.;
/// Positions of the clickable segments in `status_segments`.
const STATUS_ENCODING: usize = 2;
const STATUS_LINE_ENDING: usize = 3;
/// Rough width of one header character at `CHROME_TEXT` (for the path budget; the path also clips).
const APPROX_CHAR_W: f32 = 7.;
/// Header cells taken by the 「只读」 marker, ●, both buttons and the paddings.
const HEADER_FIXED_CHARS: usize = 22;

/// `第 N 行，第 M 列` (1-based) plus ` · 已选 K 字符` when something is selected.
pub(crate) fn cursor_label(line: usize, col: usize, selected_chars: usize) -> String {
    let mut s = format!("第 {} 行，第 {} 列", line + 1, col + 1);
    if selected_chars > 0 {
        s.push_str(&format!(" · 已选 {selected_chars} 字符"));
    }
    s
}

/// The status bar's first segment: a live flash (「已更新」) instead of the cursor label.
pub(crate) fn status_left(cursor: String, flash: Option<&str>) -> String {
    flash.map_or(cursor, str::to_owned)
}

/// The status bar's line-ending segment: the ending the next save writes, marked when the file had several.
pub(crate) fn line_ending_label(le: LineEnding, mixed: bool) -> String {
    if mixed { format!("{} · 混合", le.name()) } else { le.name().to_owned() }
}

/// The hover text of a mixed line-ending segment.
pub(crate) fn mixed_hint(le: LineEnding) -> String {
    format!("文件里同时有多种换行符；保存会统一成 {}。", le.name())
}

/// The status bar's segments, left to right; drawn with 「｜」 between them.
pub(crate) fn status_segments(cursor: String, language: &str, encoding: &str, line_ending: &str, lines: usize) -> Vec<String> {
    vec![cursor, language.into(), encoding.into(), line_ending.into(), format!("{lines} 行")]
}

pub(crate) fn close_prompt(name: &str) -> String {
    format!("要保存对 {name} 的修改吗？")
}

/// How many characters the header's directory may take in a pane `width_px` wide, next to a file name of
/// `name_chars` characters (counted double: CJK names are twice as wide).
pub(crate) fn path_budget(width_px: f32, name_chars: usize) -> usize {
    let total = (width_px.max(0.) / APPROX_CHAR_W) as usize;
    total.saturating_sub(name_chars * 2 + HEADER_FIXED_CHARS).max(4)
}

/// The header's directory: `~`-abbreviated, cut from the left on component boundaries to fit `budget`.
pub(crate) fn header_dir(path: Option<&Path>, home: Option<&Path>, budget: usize) -> String {
    match path.and_then(Path::parent).filter(|d| !d.as_os_str().is_empty()) {
        Some(dir) => shorten_dir(dir, home, budget),
        None => String::new(),
    }
}

/// The notice bar's buttons, left to right: label, action, primary.
pub(crate) fn bar_buttons(bar: &Bar) -> Vec<(&'static str, BarAction, bool)> {
    match bar {
        Bar::None => vec![],
        Bar::Close => vec![("取消", BarAction::Cancel, false), ("不保存", BarAction::Discard, false), ("保存 ⏎", BarAction::SaveAndClose, true)],
        Bar::Modified { .. } => vec![("重新载入", BarAction::Reload, false), ("对比", BarAction::Compare, true), ("仍然覆盖", BarAction::Overwrite, false)],
        Bar::Deleted => vec![("保存（重新创建）", BarAction::Recreate, true), ("关闭", BarAction::CloseNow, false), ("知道了", BarAction::Cancel, false)],
        Bar::Confirm(_) => vec![("取消", BarAction::Cancel, false), ("放弃改动并重新打开", BarAction::ConfirmReopen, true)],
        Bar::SaveError { jump: Some(_), .. } => vec![(JUMP_LABEL, BarAction::Jump, false), ("知道了", BarAction::Cancel, false)],
        Bar::SaveError { jump: None, .. } => vec![("知道了", BarAction::Cancel, false)],
    }
}

/// The header's read-only marker: 「只读 · 部分字符已替换」 for a lossy decoding, 「只读」 for any other
/// read-only buffer (manual, file permissions, over-long line).
pub(crate) fn read_only_badge(read_only: bool, lossy: bool) -> Option<&'static str> {
    match (read_only, lossy) {
        (_, true) => Some("只读 · 部分字符已替换"),
        (true, false) => Some("只读"),
        (false, false) => None,
    }
}

/// `muted` text and the header / status background.
fn tones(p: &Palette) -> (Hsla, Hsla) {
    (hsla(mix(p.foreground, p.background, 0.45)), hsla(mix(p.background, p.foreground, 0.06)))
}

fn button(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>, primary: bool, p: &Palette) -> Stateful<Div> {
    let el = div()
        .id(id)
        .flex_none()
        .px(px(8.))
        .rounded(px(4.))
        .cursor(CursorStyle::PointingHand)
        .child(label.into());
    if primary {
        el.bg(hsla(p.ansi[4])).text_color(hsla(p.background))
    } else {
        el.bg(hsla(mix(p.background, p.foreground, 0.14)))
    }
}

pub(crate) fn header(view: &EditorView, p: &Palette, cx: &mut Context<EditorView>) -> impl IntoElement {
    let (muted, bg) = tones(p);
    let name = view.title();
    let width = view.layout.map_or(600., |l| l.bounds.size.width / px(1.));
    let dir = header_dir(view.path(), label_home(), path_budget(width, name.chars().count()));
    let mut left = div()
        .flex()
        .flex_1()
        .min_w(px(0.))
        .items_center()
        .gap_2()
        .overflow_hidden()
        .whitespace_nowrap()
        .child(div().flex_none().font_weight(FontWeight::BOLD).child(name));
    if view.is_dirty() {
        left = left.child(div().flex_none().text_color(hsla(p.ansi[3])).child("●"));
    }
    if let Some(badge) = read_only_badge(view.model.is_read_only(), view.model.buf.lossy()) {
        left = left.child(div().flex_none().px(px(6.)).rounded(px(4.)).bg(hsla(mix(p.background, p.foreground, 0.14))).child(badge));
    }
    left = left.child(div().flex_1().min_w(px(0.)).overflow_hidden().text_color(muted).child(dir));
    let header = div()
        .id("editor-header")
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .h(px(HEADER_H))
        .px_2()
        .gap_2()
        .text_size(px(CHROME_TEXT))
        .bg(bg);
    // Dragged onto the tab bar, the pane leaves its split for a tab of its own (`workspace/pane_drag.rs`).
    let header = match view.pane {
        Some(pane) => header
            .children(rects::recorder(RectId::PaneHeader(pane)))
            .cursor(CursorStyle::OpenHand)
            .on_drag(crate::workspace::DraggedPane { pane, title: view.title() }, |d, _, _, cx| cx.new(|_| d.clone())),
        None => header.cursor(CursorStyle::Arrow),
    };
    header
        .child(left)
        .child(
            button("editor-preview", preview_label(view.live), view.live.is_some(), p)
                .relative()
                .children(view.pane.and_then(|id| rects::recorder(RectId::EditorPreview(id))))
                .on_click(cx.listener(|v, _: &ClickEvent, _, cx| v.click_preview(cx))),
        )
        .child(
            button("editor-save", "保存 ⌘S", true, p)
                .relative()
                .children(view.pane.and_then(|id| rects::recorder(RectId::EditorSave(id))))
                .on_click(cx.listener(|v, _: &ClickEvent, _, cx| {
                    v.save(cx);
                })),
        )
        .child(
            button("editor-close", "✕", false, p)
                .relative()
                .children(view.pane.and_then(|id| rects::recorder(RectId::EditorClose(id))))
                .on_click(cx.listener(|v, _: &ClickEvent, _, cx| v.request_close(cx))),
        )
}

pub(crate) fn bar(view: &EditorView, p: &Palette, cx: &mut Context<EditorView>) -> Option<impl IntoElement> {
    let (text, tint): (Vec<String>, _) = match &view.bar {
        Bar::None => return None,
        Bar::Close => (vec![close_prompt(&view.title())], p.ansi[3]),
        Bar::Modified { .. } => (vec![MODIFIED_TEXT.into()], p.ansi[3]),
        Bar::SaveError { message, .. } => (vec![message.clone()], p.ansi[1]),
        Bar::Deleted => (vec![DELETED_TEXT.into()], p.ansi[3]),
        Bar::Confirm(_) => (vec![CONFIRM_TEXT.into()], p.ansi[3]),
    };
    let mut row = div()
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .px_2()
        .py(px(3.))
        .text_size(px(CHROME_TEXT))
        .bg(hsla(mix(p.background, tint, 0.25)))
        .cursor(CursorStyle::Arrow)
        .child(div().flex().flex_1().min_w(px(0.)).gap_1().overflow_hidden().whitespace_nowrap().children(text));
    for (i, (label, action, primary)) in bar_buttons(&view.bar).into_iter().enumerate() {
        row = row.child(
            button(("editor-bar", i), label, primary, p)
                .relative()
                .children(view.pane.and_then(|id| rects::recorder(RectId::EditorBarButton(id, i))))
                .on_click(cx.listener(move |v, _: &ClickEvent, _, cx| v.bar_action(action, cx))),
        );
    }
    Some(row)
}

pub(crate) fn status(view: &EditorView, p: &Palette, cx: &mut Context<EditorView>) -> impl IntoElement {
    let (muted, bg) = tones(p);
    let m = &view.model;
    let pos = m.caret_position();
    let selected = m.buf.selection().range().len();
    let mixed = m.buf.mixed_line_endings();
    let segs = status_segments(
        status_left(cursor_label(pos.line, pos.col, selected), view.flash_text()),
        &m.language_label(),
        m.buf.encoding().name(),
        &line_ending_label(m.buf.line_ending(), mixed),
        m.buf.line_count(),
    );
    let mut row = div()
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .h(px(STATUS_H))
        .px_2()
        .text_size(px(11.5))
        .text_color(muted)
        .bg(bg)
        .overflow_hidden()
        .whitespace_nowrap()
        .cursor(CursorStyle::Arrow);
    let hover = hsla(mix(p.background, p.foreground, 0.14));
    for (i, s) in segs.into_iter().enumerate() {
        if i > 0 {
            row = row.child("｜");
        }
        row = match i {
            STATUS_ENCODING => row.child(
                div()
                    .id("editor-status-encoding")
                    .relative()
                    .px(px(3.))
                    .rounded(px(3.))
                    .cursor(CursorStyle::PointingHand)
                    .hover(|st| st.bg(hover))
                    .child(format!("{s} ▾"))
                    .child(encoding_recorder(cx))
                    .children(view.pane.and_then(|id| rects::recorder(RectId::EditorEncoding(id))))
                    .on_click(cx.listener(|v, e: &ClickEvent, _, cx| v.open_menu_at(MenuKind::Encoding, e.position(), cx))),
            ),
            // Amber and a hover hint while the file has mixed endings; a click opens the line-ending menu.
            STATUS_LINE_ENDING => {
                let mut seg = div()
                    .id("editor-status-line-ending")
                    .relative()
                    .px(px(3.))
                    .rounded(px(3.))
                    .cursor(CursorStyle::PointingHand)
                    .hover(|st| st.bg(hover))
                    .child(s)
                    .children(view.pane.and_then(|id| rects::recorder(RectId::EditorLineEnding(id))))
                    .on_click(cx.listener(|v, e: &ClickEvent, _, cx| v.open_menu_at(MenuKind::LineEnding, e.position(), cx)));
                if mixed {
                    let hint = mixed_hint(m.buf.line_ending());
                    seg = seg
                        .text_color(hsla(p.ansi[3]))
                        .tooltip(move |_, cx| cx.new(|_| crate::sidebar::tooltip::SidebarTooltip::new(vec![hint.clone()])).into());
                }
                row.child(seg)
            }
            _ => row.child(s),
        };
        // Read-only edits are refused quietly; this is where the reason shows.
        if i == 0 {
            if let Some(reason) = m.read_only_reason() {
                row = row.child(div().text_color(hsla(p.ansi[3])).child(reason));
            }
        }
    }
    row
}

/// An invisible child of the encoding segment (which is `relative()`) that tells the view where the segment
/// was laid out, so a menu opened without a click (`EditorView::open_menu`) is anchored there.
fn encoding_recorder(cx: &mut Context<EditorView>) -> impl IntoElement {
    let view = cx.entity();
    canvas(
        move |bounds, window, cx| {
            if view.update(cx, |v, _| v.status_encoding_laid_out(bounds)) {
                // A menu was waiting for this layout: draw again so it shows (a notify here schedules no frame).
                let view = view.clone();
                window.defer(cx, move |_, cx| view.update(cx, |_, cx| cx.notify()));
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The open status-bar menu (E2b-1 §3), drawn above everything (`deferred`), anchored by its bottom-left
/// corner above the status bar. Looks like the sidebar's row menu (`sidebar/menu.rs`). Clicks inside never
/// reach the text (`occlude`); a mouse-down outside closes it (`EditorView::menu_mouse_down_out`).
pub(crate) fn popup(view: &EditorView, ui: &gilvt_theme::UiColors, cx: &mut Context<EditorView>) -> Option<AnyElement> {
    let menu = view.placed_menu()?;
    let h = crate::theme::hsla;
    let (bg, border, text, hover, hover_text) = (h(ui.raised), h(ui.border_strong), h(ui.text), h(ui.accent), h(ui.on_accent));
    let (rule, disabled) = (h(ui.border), h(ui.text_4));
    let mut list = div()
        .id("editor-menu")
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
        .cursor(CursorStyle::Arrow)
        // Keeps the keyboard on the editor (for Esc) when the click lands between items.
        .on_mouse_down(MouseButton::Left, cx.listener(|v, _: &MouseDownEvent, window, _| window.focus(&v.focus_handle)))
        .on_mouse_down_out(cx.listener(EditorView::menu_mouse_down_out));
    for (i, item) in menu.items.iter().enumerate() {
        if item.separator_before {
            list = list.child(div().my(px(3.)).h(px(1.)).bg(rule));
        }
        let mut row = div()
            .id(("editor-menu-item", i))
            .relative()
            .flex()
            .gap(px(6.))
            .mx(px(4.))
            .px(px(8.))
            .py(px(3.))
            .rounded(px(4.))
            .child(div().w(px(10.)).child(if item.checked { "✓" } else { "" }))
            .child(SharedString::from(item.label.clone()))
            .children(view.pane.and_then(|id| rects::recorder(RectId::EditorMenuItem(id, i))));
        row = if item.enabled {
            let action = item.action.clone();
            row.hover(|s| s.bg(hover).text_color(hover_text))
                .on_click(cx.listener(move |v, _: &ClickEvent, _, cx| v.menu_action(action.clone(), cx)))
        } else {
            row.text_color(disabled)
        };
        list = list.child(row);
    }
    Some(deferred(anchored().anchor(Corner::BottomLeft).position(menu.at).snap_to_window_with_margin(px(8.)).child(list)).with_priority(1).into_any_element())
}

/// What the pane shows instead of the bar, text and status bar when it is too narrow.
pub(crate) fn too_narrow(p: &Palette) -> impl IntoElement {
    let (muted, _) = tones(p);
    div()
        .absolute()
        .top(px(HEADER_H))
        .left_0()
        .right_0()
        .bottom_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(hsla(p.background))
        .text_size(px(CHROME_TEXT))
        .text_color(muted)
        .child(TOO_NARROW)
}

/// The header's preview button: the shortcut while closed, the provider shown while open.
pub(crate) fn preview_label(live: Option<crate::live_preview::Provider>) -> String {
    match live {
        None => "预览 ⌘⇧V".to_string(),
        Some(p) => format!("预览：{}", p.label()),
    }
}

/// How the compare overlay draws its rows.
struct RowStyle {
    line_h: Pixels,
    /// Width of a line number, in digits.
    digits: usize,
    muted: Hsla,
    added: Hsla,
    removed: Hsla,
    added_mark: Hsla,
    removed_mark: Hsla,
    /// The empty side of a split row (a line only one version has).
    blank: Hsla,
}

/// One line of one side: line number(s), `+` / `-` marker, text, on its tint. `None` is an empty cell.
fn compare_cell(nos: &[Option<u32>], line: Option<&DiffLine>, s: &RowStyle) -> Div {
    let el = div().flex().flex_1().min_w(px(0.)).h(s.line_h).items_center().overflow_hidden().whitespace_nowrap();
    let Some(line) = line else { return el.bg(s.blank) };
    let (bg, marker, mark) = match line.kind {
        LineKind::Added => (Some(s.added), "+", s.added_mark),
        LineKind::Removed => (Some(s.removed), "-", s.removed_mark),
        LineKind::Context => (None, " ", s.muted),
    };
    let mut el = match bg {
        Some(bg) => el.bg(bg),
        None => el,
    };
    for no in nos {
        let n = no.map_or(String::new(), |n| n.to_string());
        el = el.child(div().flex_none().text_color(s.muted).child(format!("{n:>w$} ", w = s.digits)));
    }
    el.child(div().flex_none().text_color(mark).child(format!("{marker} ")))
        .child(div().flex_1().min_w(px(0.)).overflow_hidden().child(line.text.replace('\t', "    ")))
}

fn fold_row(start: usize, len: usize, s: &RowStyle, p: &Palette, cx: &mut Context<EditorView>) -> AnyElement {
    div()
        .id(("editor-compare-fold", start))
        .flex()
        .flex_none()
        .w_full()
        .h(s.line_h)
        .items_center()
        .justify_center()
        .bg(hsla(mix(p.background, p.foreground, 0.06)))
        .text_color(s.muted)
        .cursor(CursorStyle::PointingHand)
        .child(compare::fold_label(len))
        .on_click(cx.listener(move |v, _: &ClickEvent, _, cx| v.compare_toggle_fold(start, cx)))
        .into_any_element()
}

/// The 「对比」 overlay (E2b-1 §3): covers the whole pane and blocks the mouse for everything under it.
/// Rows are plain `div`s from `scroll` on; the body clips what does not fit.
pub(crate) fn compare_overlay(view: &EditorView, p: &Palette, cx: &mut Context<EditorView>) -> Option<impl IntoElement> {
    let c = view.compare.as_ref()?;
    let (muted, bg) = tones(p);
    let settings = &cx.global::<AppSettings>().0;
    let (family, font_size, line_h) = (settings.font_family.clone(), px(settings.font_size), compare_line_h(settings));
    let s = RowStyle {
        line_h,
        digits: c.diff.lines.len().max(1).to_string().len(),
        muted,
        added: hsla(mix(p.background, p.ansi[2], 0.2)),
        removed: hsla(mix(p.background, p.ansi[1], 0.2)),
        added_mark: hsla(p.ansi[2]),
        removed_mark: hsla(p.ansi[1]),
        blank: hsla(mix(p.background, p.foreground, 0.04)),
    };
    // Rows past the bottom are clipped anyway: draw at most a screenful past the text area's height.
    let shown = view.layout.map_or(80, |l| l.rows + 8);
    let window = |len: usize| c.scroll.min(len)..(c.scroll + shown).min(len);
    let mut body = div()
        .id("editor-compare-body")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .overflow_hidden()
        .font_family(family)
        .text_size(font_size)
        .on_scroll_wheel(cx.listener(EditorView::compare_wheel));
    let note = |t: &'static str| div().flex().flex_1().items_center().justify_center().text_color(muted).child(t);
    body = match &c.rows {
        _ if c.disk_missing => body.child(note(compare::MISSING_TEXT)),
        CompareRows::Same => body.child(note(c.note)),
        CompareRows::Unified(rows) => {
            for row in &rows[window(rows.len())] {
                body = body.child(match *row {
                    Row::Line(i) => {
                        let l = &c.diff.lines[i];
                        compare_cell(&[l.old_no, l.new_no], Some(l), &s).flex_none().w_full().into_any_element()
                    }
                    Row::Fold { start, len } => fold_row(start, len, &s, p, cx),
                });
            }
            body
        }
        CompareRows::Split(rows) => {
            for row in &rows[window(rows.len())] {
                body = body.child(match *row {
                    SplitRow::Pair { left, right } => {
                        let (l, r) = (left.map(|i| &c.diff.lines[i]), right.map(|i| &c.diff.lines[i]));
                        div()
                            .flex()
                            .flex_none()
                            .w_full()
                            .child(compare_cell(&[l.and_then(|l| l.old_no)], l, &s))
                            .child(div().flex_none().w(px(1.)).h(line_h).bg(bg))
                            .child(compare_cell(&[r.and_then(|r| r.new_no)], r, &s))
                            .into_any_element()
                    }
                    SplitRow::Fold { start, len } => fold_row(start, len, &s, p, cx),
                });
            }
            body
        }
    };
    let mut footer = div().flex().flex_none().items_center().gap_2().px_2().py(px(6.)).bg(bg);
    for (i, (label, action, danger)) in compare::buttons(c.disk_missing).into_iter().enumerate() {
        let mut b = button(("editor-compare-btn", i), label, false, p)
            .relative()
            .children(view.pane.and_then(|id| rects::recorder(RectId::EditorCompareButton(id, i))));
        if danger {
            // 仍然覆盖磁盘 sits alone on the right, in red.
            footer = footer.child(div().flex_1());
            b = b.bg(hsla(p.ansi[1])).text_color(hsla(p.background));
        }
        footer = footer.child(b.on_click(cx.listener(move |v, _: &ClickEvent, _, cx| v.compare_action(action, cx))));
    }
    let header = div()
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .h(px(HEADER_H))
        .px_2()
        .bg(bg)
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .font_weight(FontWeight::BOLD)
                .child(compare::header_text(&view.title())),
        )
        .child(div().flex_none().text_color(muted).child(compare::ESC_HINT));
    let panel = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .rounded(px(6.))
        .border_1()
        .border_color(hsla(mix(p.background, p.foreground, 0.2)))
        .bg(hsla(p.background))
        .overflow_hidden()
        .child(header)
        .child(body)
        .child(footer);
    Some(
        div()
            .id("editor-compare")
            .occlude()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .flex_col()
            .py(px(18.))
            .px(px(34.))
            .bg(hsla(mix(p.background, p.foreground, 0.1)))
            .text_size(px(CHROME_TEXT))
            .cursor(CursorStyle::Arrow)
            // Clicks here never reach the text (`occlude`); they keep the keyboard on the editor for Esc.
            .on_mouse_down(MouseButton::Left, cx.listener(|v, _: &MouseDownEvent, window, _| window.focus(&v.focus_handle)))
            .child(panel)
            // Already `absolute()`, which positions the recorder too (`relative()` here would undo it).
            .children(view.pane.and_then(|id| rects::recorder(RectId::EditorCompare(id)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_label_is_one_based_and_shows_the_selection_only_when_present() {
        assert_eq!(cursor_label(0, 0, 0), "第 1 行，第 1 列");
        assert_eq!(cursor_label(9, 4, 12), "第 10 行，第 5 列 · 已选 12 字符");
    }

    #[test]
    fn status_segments_in_order() {
        assert_eq!(
            status_segments(cursor_label(1, 2, 0), "Rust", "UTF-8", "LF", 42),
            vec!["第 2 行，第 3 列", "Rust", "UTF-8", "LF", "42 行"]
        );
        // The clickable segments are found by position.
        let segs = status_segments("c".into(), "Rust", "GBK", "CRLF", 1);
        assert_eq!((segs[STATUS_ENCODING].as_str(), segs[STATUS_LINE_ENDING].as_str()), ("GBK", "CRLF"));
    }

    #[test]
    fn the_label_marks_a_mixed_file() {
        assert_eq!(line_ending_label(LineEnding::Lf, false), "LF");
        assert_eq!(line_ending_label(LineEnding::CrLf, true), "CRLF · 混合");
        assert_eq!(mixed_hint(LineEnding::Lf), "文件里同时有多种换行符；保存会统一成 LF。");
        assert_eq!(mixed_hint(LineEnding::CrLf), "文件里同时有多种换行符；保存会统一成 CRLF。");
        assert_eq!(mixed_hint(LineEnding::Cr), "文件里同时有多种换行符；保存会统一成 CR。");
    }

    #[test]
    fn a_forced_encoding_buffer_with_mixed_endings_reports_mixed() {
        use gilvt_editor::{Buffer, Encoding, OpenOptions};
        let dir = std::env::temp_dir().join(format!("gilvt-chrome-mixed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("m.txt");
        std::fs::write(&f, b"a\r\nb\nc\r\n").unwrap();
        let opts = OpenOptions { encoding: Some(Encoding::Utf8), read_only: false };
        let buf = Buffer::open_with(&f, opts).unwrap();
        assert!(buf.mixed_line_endings());
        assert_eq!(line_ending_label(buf.line_ending(), buf.mixed_line_endings()), format!("{} · 混合", buf.line_ending().name()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn flash_text_replaces_the_cursor_label_while_active() {
        assert_eq!(status_left("第 1 行，第 1 列".into(), Some("已更新")), "已更新");
        assert_eq!(status_left("第 1 行，第 1 列".into(), None), "第 1 行，第 1 列");
    }

    #[test]
    fn the_header_says_why_a_buffer_is_read_only() {
        assert_eq!(read_only_badge(false, false), None);
        assert_eq!(read_only_badge(true, false), Some("只读"));
        assert_eq!(read_only_badge(true, true), Some("只读 · 部分字符已替换"));
    }

    #[test]
    fn close_prompt_names_the_file() {
        assert_eq!(close_prompt("SKILL.md"), "要保存对 SKILL.md 的修改吗？");
    }

    #[test]
    fn path_budget_shrinks_with_the_pane_and_never_vanishes() {
        assert!(path_budget(800., 8) > path_budget(400., 8));
        assert!(path_budget(800., 8) > path_budget(800., 20));
        assert_eq!(path_budget(50., 8), 4);
        assert_eq!(path_budget(-10., 0), 4);
    }

    #[test]
    fn header_dir_abbreviates_home_and_keeps_the_tail() {
        let home = Some(Path::new("/Users/u"));
        let p = Path::new("/Users/u/Workplace/utils/acme_web_monorepo/gilvt/SKILL.md");
        assert_eq!(header_dir(Some(p), home, 200), "~/Workplace/utils/acme_web_monorepo/gilvt");
        assert_eq!(header_dir(Some(p), home, 30), "~/…/acme_web_monorepo/gilvt");
        assert_eq!(header_dir(Some(Path::new("/opt/a.md")), home, 40), "/opt");
        assert_eq!(header_dir(Some(Path::new("a.md")), home, 40), "");
        assert_eq!(header_dir(None, home, 40), "");
    }

    #[test]
    fn bar_buttons_match_the_spec() {
        let labels = |b: &Bar| bar_buttons(b).into_iter().map(|(l, a, _)| (l, a)).collect::<Vec<_>>();
        assert_eq!(
            labels(&Bar::Close),
            vec![("取消", BarAction::Cancel), ("不保存", BarAction::Discard), ("保存 ⏎", BarAction::SaveAndClose)]
        );
        assert_eq!(
            labels(&Bar::Modified { closing: false }),
            vec![("重新载入", BarAction::Reload), ("对比", BarAction::Compare), ("仍然覆盖", BarAction::Overwrite)]
        );
        assert_eq!(
            labels(&Bar::Deleted),
            vec![("保存（重新创建）", BarAction::Recreate), ("关闭", BarAction::CloseNow), ("知道了", BarAction::Cancel)]
        );
        assert_eq!(
            labels(&Bar::Confirm(super::super::view::ReopenIntent(Default::default()))),
            vec![("取消", BarAction::Cancel), ("放弃改动并重新打开", BarAction::ConfirmReopen)]
        );
        assert_eq!(
            labels(&Bar::SaveError { message: "x".into(), jump: Some((1, 2)) }),
            vec![(JUMP_LABEL, BarAction::Jump), ("知道了", BarAction::Cancel)]
        );
        assert_eq!(labels(&Bar::SaveError { message: "x".into(), jump: None }), vec![("知道了", BarAction::Cancel)]);
        assert!(labels(&Bar::None).is_empty());
        // The default (primary) button of the close bar is 保存.
        assert!(bar_buttons(&Bar::Close)[2].2);
    }

    #[test]
    fn preview_button_names_the_current_provider() {
        use crate::live_preview::Provider;
        assert_eq!(preview_label(None), "预览 ⌘⇧V");
        assert_eq!(preview_label(Some(Provider::Rendered)), "预览：渲染");
        assert_eq!(preview_label(Some(Provider::Changes)), "预览：改动");
    }
}
