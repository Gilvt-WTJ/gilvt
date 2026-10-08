//! Where things were drawn in each window's last frame, for `gilvt debug state`: sidebar rows and headers,
//! tabs, panes, menu items, timeline rows, palette rows, chips and buttons record their bounds from an
//! invisible `canvas` child while they are prepainted. `Workspace` clears its window's entries when it
//! renders, so an element not drawn in the latest frame has no rect.
//!
//! Recording is off until the first query arrives in a gilvt started with `GILVT_DEBUG_STATE=1`
//! ([`start_recording`]); until then [`recorder`] adds no element and [`begin_frame`] does nothing, so
//! the only cost for everyone else is one atomic load per recorder per frame. The first query draws every
//! window before it collects, so its rects are current.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{canvas, point, prelude::*, px, size, App, Bounds, Global, Pixels, TextLayout, Window, WindowId};

use crate::pane_tree::PaneId;

/// What a rect belongs to (indexes are in drawing order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RectId {
    /// The n-th row drawn in the sidebar.
    SidebarRow(usize),
    /// The install banner's buttons: 0 移到「应用程序」, 1 以后再说.
    InstallButton(usize),
    /// The n-th grouping button of the sidebar header (0 按项目, 1 按状态).
    SidebarGrouping(usize),
    Pane(PaneId),
    /// The n-th item of the sidebar row menu.
    SessionMenuItem(usize),
    /// The n-th item of the 会话 palette's row menu.
    PaletteMenuItem(usize),
    /// The header of the n-th sidebar section.
    SidebarSection(usize),
    /// The n-th button of the sidebar's trash confirm bar.
    SidebarTrashButton(usize),
    /// The n-th tab of the tab bar.
    Tab(usize),
    /// The whole tab bar (where a dragged pane header drops to get a tab of its own).
    TabBar,
    /// The header of a pinned preview or editor pane: the handle dragged onto the tab bar.
    PaneHeader(PaneId),
    /// The pane area's edge with the inspector.
    InspectorEdge,
    /// The inspector's waiting banner.
    InspectorBanner,
    /// Item n of the inspector's timeline list (its line).
    TimelineRow(usize),
    /// The ▸ / ▾ of timeline item n.
    TimelineToggle(usize),
    /// The file name in timeline item n.
    TimelineFile(usize),
    /// The 「复制」 button of timeline item n's detail.
    TimelineCopy(usize),
    /// The n-th timeline filter chip.
    TimelineChip(usize),
    /// Card `cards[n]` of the 「产物」 tab (its index in the model's cards, newest first; a card folded in a
    /// closed quiet group is not drawn).
    ArtifactCard(usize),
    /// File `j` of card `cards[n]` of the 「产物」 tab.
    ArtifactFile(usize, usize),
    /// The 「另有 N 个文件 · 按目录分组查看全部」 link of card `cards[n]`.
    ArtifactMore(usize),
    /// The 「本会话净改动」 bar / card of the 「产物」 tab.
    ArtifactNet,
    /// File `j` of the open 「本会话净改动」 card.
    ArtifactNetFile(usize),
    /// The folded 「N 轮无文件改动」 row `quiet_groups[n]`.
    ArtifactQuiet(usize),
    /// The 「刷新」 link in the inspector's configuration tab.
    InspectorConfigRefresh,
    /// The header line of the n-th expandable MCP / hook / memory row of the 「配置」 tab (drawing order).
    ConfigRow(usize),
    /// The 「打开」 link inside the n-th expanded row.
    ConfigOpen(usize),
    /// The n-th opener (0 Skills, 1 命令, 2 子 Agent) in the 「扩展」 card.
    ConfigGroup(usize),
    /// The n-th row of the resource dialog.
    ConfigDialogRow(usize),
    /// The ✕ of the resource dialog.
    ConfigDialogClose,
    /// Row n of the 会话 palette (an index into its rows, headers not counted).
    PaletteRow(usize),
    /// The 会话 palette's chips: 0 the project, 1 全部项目, 2 ≥ 7 天未活动, 3 已归档.
    PaletteChip(usize),
    /// The 「清理… ⌘⇧K」 entry of the 会话 palette's key hints.
    PaletteCleanup,
    /// The n-th button of the 会话 palette's confirm bar.
    PaletteConfirmButton(usize),
    /// The cleanup wizard's preset lines (`Preset::ALL` order).
    CleanupPreset(usize),
    /// Row n of the cleanup wizard's preview.
    CleanupRow(usize),
    /// The cleanup wizard's buttons: 0 归档, 1 移到废纸篓.
    CleanupButton(usize),
    /// The n-th button of the cleanup wizard's confirm bar.
    CleanupConfirmButton(usize),
    /// The four Session Center tabs.
    SessionCenterTab(usize),
    /// A visible row in the Session Center review queues.
    SessionCenterRow(usize),
    /// The Session Center sort control.
    SessionCenterSort,
    /// A button of the Session Center's read-only review.
    ReviewButton(ReviewButton),
    /// The n-th turn card of the shown review page.
    ReviewTurn(usize),
    /// The 待 Review N entry of the sidebar.
    SidebarReviewEntry,
    /// The ⌘⇧N panel's fields: 0 目录, 1 初始任务, 2 更多, 3 模型, 4 权限模式.
    NewAgentField(usize),
    /// The code / diff text area (the rendered document in a Markdown preview) of Quick Look (`None`) or of the pinned preview in a pane.
    PreviewCode(Option<PaneId>),
    /// The editor pane's text area.
    EditorBody(PaneId),
    /// The 「保存 ⌘S」 button of the editor pane's header.
    EditorSave(PaneId),
    /// The 「预览」 button of the editor pane's header.
    EditorPreview(PaneId),
    /// The 「✕」 button of the editor pane's header.
    EditorClose(PaneId),
    /// Button n of the editor pane's notice bar (left to right).
    EditorBarButton(PaneId, usize),
    /// The encoding segment of the editor pane's status bar.
    EditorEncoding(PaneId),
    /// The line-ending segment of the editor pane's status bar.
    EditorLineEnding(PaneId),
    /// Item n of the editor pane's open status-bar menu (separators not counted).
    EditorMenuItem(PaneId, usize),
    /// The editor pane's 「对比」 overlay.
    EditorCompare(PaneId),
    /// Button n of the 「对比」 overlay's bottom row (left to right).
    EditorCompareButton(PaneId, usize),
    /// The hover-only 「编辑」 of row n of the resource dialog (recorded whenever the row is drawn, even while the button is hidden: gpui prepaints hidden elements; a hidden button takes no click, so hover the row first).
    ConfigDialogEdit(usize),
    /// The n-th filter chip of the monitor tab (0 = 全部).
    MonitorFilter(usize),
    /// The header of the n-th group drawn in the monitor tab.
    MonitorGroup(usize),
    /// The n-th card drawn in the monitor tab.
    MonitorCard(usize),
    /// The 「跳过去」 button of card n.
    MonitorJump(usize),
    /// The 「补课」 button of card n.
    MonitorCatchup(usize),
    /// Row j of card n's 补课 list.
    MonitorCatchupRow(usize, usize),
    /// The ✦ block of the n-th drawn monitor card.
    MonitorSummary(usize),
    /// Its 「✦ 重新总结」 button.
    MonitorResummarize(usize),
    /// The control of field n of the settings page (`settings_window::form::FieldId::index`).
    SettingsField(usize),
    /// Option j of field n: a segment, an exclude tag's ×, or an item of an open dropdown.
    SettingsOption(usize, usize),
    /// The 「其他…」 / CLI path text box.
    SettingsOther,
    /// The chat panel's input box.
    ChatInput,
    /// The × of chip n in the chat input.
    ChatChipRemove(usize),
    /// The 「◎ 问它」 button of card n.
    MonitorAsk(usize),
    /// The chat panel (beside the wall, or over it when the tab is narrow).
    ChatPanel,
    /// The 「◎」 rail at the tab's right edge (narrow tab, or collapsed with ⇥).
    ChatRail,
    /// The chat header's model button (opens the settings).
    ChatModel,
    /// The chat header's ⇥ (back to the rail).
    ChatCollapse,
    /// Chat message n.
    ChatMessage(usize),
    /// Session link j (reading order) of chat message n: where its text was laid out, on the line where it starts
    /// (`text_range_bounds`). Only clickable links (on the wall, not excluded) have one.
    ChatLink(usize, usize),
    /// Quick question i (「✦ 生成站会简报」, 「哪些需要我？」, 「有什么出错了？」).
    ChatQuick(usize),
    /// The chat header's 「停止」 (while answering).
    ChatStop,
    /// The chat header's 「新对话」.
    ChatNew,
    /// Row i of the `@` picker.
    ChatPickerItem(usize),
    /// The 「打开设置」 button of the red card in chat message n.
    ChatErrorSettings(usize),
    /// The 「查看日志」 button of the red card in chat message n.
    ChatErrorLog(usize),
    /// The command bar's input (while open).
    CommandBarInput,
    /// The 「×」 of chip n in the command bar's input.
    CommandBarChipRemove(usize),
    /// The bottom command bar's 20px line (S2 §6.4).
    CommandBar,
    /// The command bar's popup (last question and answer, picker, input).
    CommandBarPopup,
    /// Its 「在监控官中查看 ↗」.
    CommandBarOpenMonitor,
    /// Session link j of conversation message n, as drawn in the command bar's popup.
    CommandBarLink(usize, usize),
    /// Row n of the command bar's `@` picker.
    CommandBarPickerItem(usize),
    /// Settings window, 「外观」 page: a theme row (index among the rows shown).
    ThemeRow(usize),
    /// Settings window, 「外观」 page: a filter / mode / slot control, by its id ("filter-dark", "mode-system", …).
    ThemeChip(&'static str),
    /// Settings window, 「外观」 page: the search box.
    ThemeSearch,
    /// Settings window: page n of the nav column (`settings_window::Page::index`).
    SettingsNav(usize),
    /// Settings window: language choice n (`i18n::Language::ALL` order).
    SettingsLanguage(usize),
}

/// The buttons of the Session Center's review (header, page edges, action bar and snooze menu).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReviewButton {
    Back,
    FullHistory,
    BackToAgent,
    Interrupt,
    Terminate,
    CopyDiagnostics,
    Earlier,
    Later,
    Skip,
    Snooze,
    Pin,
    Next,
    NextArchive,
    SnoozeHour,
    SnoozeLater,
    SnoozeTomorrow,
    SnoozeCancel,
    /// 「从当前开始」 of the stale-position banner.
    BaselineHere,
    /// 「Review 全部可见历史」 of the stale-position banner.
    ReviewAll,
}

/// `[x, y, w, h]` in window-local logical points, rounded to 0.1.
pub type Rect4 = [f32; 4];

#[derive(Default)]
pub struct DebugRects(HashMap<WindowId, HashMap<RectId, Rect4>>);

impl Global for DebugRects {}

fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

/// `bounds`, rounded.
pub fn rect_of(b: Bounds<Pixels>) -> Rect4 {
    [round1(b.origin.x / px(1.)), round1(b.origin.y / px(1.)), round1(b.size.width / px(1.)), round1(b.size.height / px(1.))]
}

/// The visible part of `bounds` inside the clip `mask`, rounded; None when none of it shows.
pub fn visible_rect(bounds: Bounds<Pixels>, mask: Bounds<Pixels>) -> Option<Rect4> {
    let b = bounds.intersect(&mask);
    (b.size.width > px(0.) && b.size.height > px(0.)).then(|| rect_of(b))
}

/// `rect` (gpui's coordinates, relative to the window's content view) moved into the window frame's
/// coordinates: `titlebar` is how far the content view starts below the frame's top edge.
pub fn in_frame(rect: Rect4, titlebar: f32) -> Rect4 {
    let [x, y, w, h] = rect;
    [x, round1(y + titlebar), w, h]
}

/// The part of `layout`'s text between two byte offsets, on the line where `range` starts: from the start
/// to the end offset (or the text's right edge when the end is cut off), one line high. None when the start
/// is not laid out (cut off by truncation).
pub fn text_range_bounds(layout: &TextLayout, range: &Range<usize>) -> Option<Bounds<Pixels>> {
    let start = layout.position_for_index(range.start)?;
    let bounds = layout.bounds();
    let end_x = layout.position_for_index(range.end).filter(|e| e.y == start.y).map_or(bounds.right(), |e| e.x);
    Some(Bounds { origin: point(start.x, start.y), size: size(end_x - start.x, layout.line_height()) })
}

static RECORDING: AtomicBool = AtomicBool::new(false);

/// Starts recording rects (the first debug state query calls this; only queries of a gilvt started with
/// `GILVT_DEBUG_STATE=1` get that far). Stays on for the rest of the process.
pub fn start_recording() {
    RECORDING.store(true, Ordering::Relaxed);
}

/// Are rects recorded?
pub fn recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

fn record(id: RectId, bounds: Bounds<Pixels>, window: &Window, cx: &mut App) {
    if !recording() {
        return;
    }
    if let Some(rect) = visible_rect(bounds, window.content_mask().bounds) {
        let window_id = window.window_handle().window_id();
        cx.default_global::<DebugRects>().0.entry(window_id).or_default().insert(id, rect);
    }
}

/// Forgets the window's rects of the previous frame (called when its root view renders).
pub fn begin_frame(window: &Window, cx: &mut App) {
    if !recording() {
        return;
    }
    if let Some(rects) = cx.default_global::<DebugRects>().0.get_mut(&window.window_handle().window_id()) {
        rects.clear();
    }
}

/// An invisible child covering its parent (which must be `relative()`) that records the parent's
/// visible bounds under `id`; None (no element: add it with `.children(…)`) while not [`recording`].
pub fn recorder(id: RectId) -> Option<impl IntoElement> {
    recording().then(|| canvas(move |bounds, window, cx| record(id, bounds, window, cx), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
}

/// An invisible, empty child placed after a text element (a later sibling is prepainted after it) that
/// records where `range` of the text was laid out (see [`text_range_bounds`]); None while not [`recording`].
pub fn text_range_recorder(id: RectId, layout: TextLayout, range: Range<usize>) -> Option<impl IntoElement> {
    recording().then(|| canvas(
        move |_, window, cx| {
            if let Some(bounds) = text_range_bounds(&layout, &range) {
                record(id, bounds, window, cx);
            }
        },
        |_, _, _, _| {},
    )
    .absolute())
}

/// Forgets the rects of windows that no longer exist (called when a window closes).
pub fn forget_closed_windows(cx: &mut App) {
    if !cx.has_global::<DebugRects>() {
        return;
    }
    let live: HashSet<WindowId> = cx.windows().iter().map(|w| w.window_id()).collect();
    cx.global_mut::<DebugRects>().0.retain(|id, _| live.contains(id));
}

/// The rects window `id` recorded in its latest frame.
pub fn of_window(id: WindowId, cx: &App) -> HashMap<RectId, Rect4> {
    cx.try_global::<DebugRects>().and_then(|r| r.0.get(&id)).cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use gpui::{point, size};

    use super::*;

    fn b(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) }
    }

    #[test]
    fn rects_are_clipped_and_rounded() {
        let window = b(0., 0., 1280., 800.);
        assert_eq!(visible_rect(b(8.04, 96.06, 224.0, 75.96), window), Some([8.0, 96.1, 224.0, 76.0]));
        // Scrolled half out of the sidebar list: only the visible part.
        assert_eq!(visible_rect(b(8., 90., 224., 40.), b(0., 100., 240., 600.)), Some([8.0, 100.0, 224.0, 30.0]));
        assert_eq!(visible_rect(b(8., 20., 224., 40.), b(0., 100., 240., 600.)), None, "scrolled out");
        assert_eq!(visible_rect(b(0., 0., 0., 10.), window), None, "empty");
    }

    #[test]
    fn rects_move_below_the_titlebar() {
        assert_eq!(in_frame([240.0, 30.0, 720.0, 590.0], 28.0), [240.0, 58.0, 720.0, 590.0]);
        assert_eq!(in_frame([8.0, 96.1, 224.0, 76.0], 27.96), [8.0, 124.1, 224.0, 76.0], "rounded again");
        assert_eq!(in_frame([8.0, 96.0, 224.0, 76.0], 0.0), [8.0, 96.0, 224.0, 76.0], "no titlebar (full screen)");
    }
}
