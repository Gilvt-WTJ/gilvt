//! `gilvt debug state`: a read-only snapshot of the UI for GUI acceptance tests, taken on the main
//! thread when the socket asks for it. Fields only ever get added; renaming or removing one bumps
//! [`VERSION`] (docs/debug-state.md).
//!
//! Opt-in: the snapshot holds every pane's screen text and cwd, and every pane's programs can reach the
//! socket, so gilvt answers only when it was started with `GILVT_DEBUG_STATE=1` ([`init_from_env`]).

mod collect;
pub mod map;
pub mod rects;
pub mod timeline;
mod untranslated;

use std::ffi::OsStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use gilvt_ipc::{Query, Request, Response};
use gpui::App;
use serde::Serialize;

use crate::agents::Agents;
use crate::notify;
use crate::persist::snapshot::WindowSnap;
pub use collect::{draw_now, views_to_draw};
use rects::Rect4;

/// The snapshot format's version.
pub const VERSION: u32 = 1;

/// Set to `1` in gilvt's own environment at launch to answer debug state queries. Never passed on to panes.
pub const ENV_DEBUG_STATE: &str = "GILVT_DEBUG_STATE";

/// Screen lines per pane at most (as `gilvt debug state --tail` clamps them).
pub const MAX_TAIL: u16 = 200;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Does this value of [`ENV_DEBUG_STATE`] switch debug state on? Only exactly `1` does.
pub fn switched_on(value: Option<&OsStr>) -> bool {
    value.is_some_and(|v| v == "1")
}

/// Reads [`ENV_DEBUG_STATE`] once and removes it from this process's environment, so no pane, agent or
/// helper inherits it (a pane's programs cannot tell). Call first thing in `main`, before any thread starts.
pub fn init_from_env() {
    ENABLED.store(switched_on(std::env::var_os(ENV_DEBUG_STATE).as_deref()), Ordering::Relaxed);
    std::env::remove_var(ENV_DEBUG_STATE);
}

/// Was gilvt started with `GILVT_DEBUG_STATE=1`?
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DebugState {
    pub version: u32,
    pub pid: u32,
    /// Is gilvt the active app (the same test the notifications use)?
    pub front: bool,
    /// The Dock badge label gilvt last set; null = no badge.
    pub dock_badge: Option<String>,
    /// How many times this process called `requestUserAttention`.
    pub dock_bounces: u64,
    /// The sessions of the saved layout nobody resumed yet (the sidebar's 待恢复), in the order of the list.
    pub pending: Vec<Pending>,
    /// The theme in use (a ⌘, preview counts).
    pub theme: Option<ThemeInfo>,
    pub windows: Vec<WindowState>,
    /// The settings window, while it is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<SettingsState>,
    /// The 监控官's chat process (S2 §6.2).
    pub chat_process: ChatProcessState,
    /// Automatic updates (Sparkle).
    pub update: UpdateState,
    /// ssh hosts seen this run.
    pub hosts: Vec<HostState>,
}

/// Top-level `update`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UpdateState {
    /// This build carries Sparkle and started it (release builds with `[update] mode` other than `off`).
    pub available: bool,
    /// `[update] mode`: `off`, `check` or `download`.
    pub mode: &'static str,
    /// The version downloaded and waiting to be installed when gilvt quits.
    pub ready: Option<String>,
}

/// The chat process of the 监控官 (`chat_process`); always present.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatProcessState {
    /// The CLI process is alive.
    pub running: bool,
    /// idle | starting | answering | stopping | error (the same as `monitor.chat.status`).
    pub status: &'static str,
    /// "claude" | "codex" while a process runs; null otherwise.
    pub provider: Option<&'static str>,
    /// The process id while it runs.
    pub pid: Option<u32>,
    /// Turns the running process has finished.
    pub turns: u64,
    /// Chat processes started in this gilvt run (lazy: 0 before the first message).
    pub starts: u64,
}

impl Default for ChatProcessState {
    fn default() -> Self {
        ChatProcessState { running: false, status: "idle", provider: None, pid: None, turns: 0, starts: 0 }
    }
}

/// The theme in use.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ThemeInfo {
    pub name: String,
    pub dark: bool,
    /// "builtin" | "user" | "fallback".
    pub source: &'static str,
    /// How many `[colors]` overrides apply.
    pub colors_overrides: usize,
    pub error: Option<String>,
}

/// A row of the settings window's 「外观」 page.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ThemeRow {
    pub name: String,
    pub user: bool,
    pub current: bool,
    pub selected: bool,
    pub rect: Option<Rect4>,
}

/// A filter / mode / slot control of the 「外观」 page.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ThemeChip {
    pub id: &'static str,
    pub on: bool,
    pub rect: Option<Rect4>,
}

/// A pending resume.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pending {
    /// "<agent>:<session_id>".
    pub session: String,
    /// The pane the session ran in (now a shell).
    pub pane: u64,
    pub cwd: Option<String>,
    pub name: String,
    /// The status it had when the layout was saved, as saved ("思考中", "空闲", …).
    pub last_status: String,
}

/// The git facts of a session's directory.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Git {
    /// The line as the sidebar draws it ("⎇ main ●2 ↑1↓0", a linked worktree's with its directory in front).
    pub line: String,
    /// null when HEAD is detached.
    pub branch: Option<String>,
    /// The short commit while HEAD is detached.
    pub detached: Option<String>,
    /// Changed + untracked paths.
    pub dirty: usize,
    pub ahead: u32,
    pub behind: u32,
    /// The directory is a linked worktree, not the main checkout.
    pub linked_worktree: bool,
    /// The worktree's top directory.
    pub repo_root: String,
    /// The main repository's directory name (what groups the sessions of one project).
    pub repo: String,
}

/// The close confirm bar of a window (⌘W / ⌘⇧W / the red button / ⌘Q).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CloseConfirm {
    /// "pane" | "tab" | "window" | "quit".
    pub action: &'static str,
    /// The sessions it lists, in order.
    pub items: Vec<CloseItem>,
    /// The unsaved editor files it lists (file names only, no directories), in order; empty when none.
    pub dirty: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CloseItem {
    /// "<agent>:<session_id>".
    pub session: String,
    pub pane: Option<u64>,
    pub name: String,
    /// The status as the bar shows it: 思考中 / 执行 <工具> / 等待授权 / 在问你.
    pub status: String,
}

/// One workspace window.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindowState {
    /// The CGWindowID (NSWindow `windowNumber`, Peekaboo's `--window-id`); null when unknown.
    pub id: Option<u64>,
    /// Is it the key window?
    pub key: bool,
    pub title: String,
    pub sidebar: Sidebar,
    pub tabs: Vec<Tab>,
    /// The tab bar: dropping a dragged pane header (`panes[].header`) on it moves the pane into a tab of its own.
    pub tab_bar: Option<Rect4>,
    /// The gpui-drawn right-click menu that is open, if any.
    pub context_menu: Option<Menu>,
    pub overlay: Option<Overlay>,
    pub inspector: Inspector,
    /// The error banner over the pane area (e.g. 「无法启动 shell：…」), without its 「（点击关闭）」.
    pub error_banner: Option<String>,
    /// The banner offering to move Gilvt.app to Applications (running translocated or from the dmg).
    pub install_banner: Option<InstallBanner>,
    /// The column edges that can be dragged.
    pub dividers: Vec<Divider>,
    /// What `workspace.json` would hold for this window right now: frame, active tab, each tab's split tree
    /// with pane ids, cwds and the agents to offer 恢复 for. Previews are not in it.
    pub layout: WindowSnap,
    /// The close confirm bar, when it shows.
    pub close_confirm: Option<CloseConfirm>,
    /// Every editor pane of the window, in tab then layout order.
    pub editors: Vec<EditorState>,
    /// The 「◎ 监控官」 card wall, when this window has a monitor pane drawn in the latest frame.
    pub monitor: Option<MonitorState>,
    /// The 监控官's bottom command bar (S2 §6.4); null while the 监控官 is off.
    pub command_bar: Option<CommandBarState>,
}

/// The monitor tab's card wall (`windows[].monitor`), in drawing order (the `n` of its `RectId`s).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorState {
    /// The monitor pane's id.
    pub pane: u64,
    /// The filter applied: "all" or a group id (the stored one falls back to "all" when its group emptied).
    pub filter: String,
    /// Cards in 需要你 (whatever the filter).
    pub needs_you: usize,
    /// Key of the selected card (`cards[].key`).
    pub selected: Option<String>,
    /// Key of the card whose 补课 list is open.
    pub expanded: Option<String>,
    /// The filter chips as drawn: 全部 first, then each non-empty group except 已结束.
    pub chips: Vec<MonitorChip>,
    /// The group headers drawn (after the filter).
    pub groups: Vec<MonitorGroup>,
    /// The cards drawn (collapsed groups excluded).
    pub cards: Vec<MonitorCard>,
    /// The chat panel (S2 §6.4) as drawn in the latest frame; null while the 监控官 is off.
    pub chat: Option<ChatState>,
}

/// The monitor tab's chat panel (`windows[].monitor.chat`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatState {
    /// Drawn as the 26px rail (tab narrower than 760px, or collapsed with ⇥).
    pub collapsed: bool,
    /// The panel is open over the wall (narrow tab).
    pub open: bool,
    /// idle | starting | answering | stopping | error.
    pub status: &'static str,
    /// The configured channel: "claude" | "codex".
    pub provider: &'static str,
    /// The configured chat model; "" = the CLI's default.
    pub model: String,
    /// An answer finished since the panel was last looked at.
    pub unread: bool,
    /// Keys of the chips in the input (the scope of the next message).
    pub scope: Vec<String>,
    /// The conversation, oldest first.
    pub messages: Vec<ChatMessageState>,
    /// The quick-question buttons.
    pub quick: Vec<ChatQuick>,
    /// The input box.
    pub input: ChatInputState,
    /// The `@` list while it is open; null otherwise.
    pub picker: Option<ChatPicker>,
    /// 「停止」, while an answer is running.
    pub stop: Option<Rect4>,
    /// 「新对话」.
    pub new: Option<Rect4>,
    /// The channel / model label in the header.
    pub model_button: Option<Rect4>,
    /// 「⇥」.
    pub collapse: Option<Rect4>,
    /// The rail.
    pub rail: Option<Rect4>,
    /// The panel.
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatMessageState {
    /// user | assistant | notice | error.
    pub role: &'static str,
    /// The user's text, or the answer's Markdown source so far.
    pub text: String,
    /// Keys of the chips a user message carries.
    pub chips: Vec<String>,
    /// The tool rows of an answer.
    pub tools: Vec<ChatTool>,
    /// The session links of an answer, in reading order.
    pub links: Vec<ChatLinkState>,
    /// The red card of a chat that could not start.
    pub error: Option<ChatErrorState>,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatTool {
    /// The MCP tool's name.
    pub name: String,
    /// As drawn: 「… 正在读取 …」, 「✓ 已读取 …」, 「✗ …」.
    pub label: String,
    pub done: bool,
    pub ok: bool,
}

/// `windows[].install_banner`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InstallBanner {
    /// `translocated` or `disk_image`.
    pub kind: &'static str,
    /// The running bundle.
    pub bundle: String,
    /// Where 移到「应用程序」 copies it.
    pub target: String,
    /// A bundle is already at `target` (it goes to the Trash first).
    pub replaces: bool,
    /// `offer`, `moving`, `moved` (copied; reopens after this process quits) or `failed`.
    pub stage: &'static str,
    /// The sentence shown.
    pub text: String,
    pub error: Option<String>,
    /// 移到「应用程序」 / 重试; null while moved.
    pub move_button: Option<Rect4>,
    /// 以后再说.
    pub dismiss_button: Option<Rect4>,
}

/// `windows[].command_bar`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommandBarState {
    /// The popup and the input are open (⌘⇧M).
    pub expanded: bool,
    /// The bar's input has the keyboard (the window's real focus, not the logical `panes[].focused`).
    pub focused: bool,
    /// The chat's status: idle / starting / answering / stopping / error (same as `monitor.chat.status`).
    pub status: &'static str,
    /// The line's text: 「◎ 监控官」, 「◎ 监控官 · 回答中…」, 「◎ 监控官 · <first sentence of the last answer>」 …
    pub line: String,
    /// The line's right-hand hint: 「⌘⇧M 提问」, or 「Esc 收起」 while open.
    pub hint: &'static str,
    /// An answer finished that no panel or open bar showed yet (the red dot on the line).
    pub unread: bool,
    /// What the popup shows (also given while closed): 「你：…」, then the replies as plain text.
    pub popup_text: String,
    /// The popup's session links in reading order; `rect` only while open and live.
    pub links: Vec<ChatLinkState>,
    pub input_text: String,
    pub chips: Vec<ChatChip>,
    /// The `@` picker, while open.
    pub picker: Option<ChatPicker>,
    /// 「在监控官中查看 ↗」.
    pub open_monitor: Option<Rect4>,
    pub popup_rect: Option<Rect4>,
    pub input_rect: Option<Rect4>,
    /// The 20px line.
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatLinkState {
    /// The key in `gilvt://session/<key>`.
    pub key: String,
    /// The link text.
    pub text: String,
    /// null when the link is drawn as plain text (its card is gone or excluded).
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatErrorState {
    pub title: String,
    pub text: String,
    /// 「打开设置」.
    pub settings: Option<Rect4>,
    /// 「查看日志」.
    pub log: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatQuick {
    pub label: String,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatInputState {
    /// The text typed (without the `@` being picked).
    pub text: String,
    pub chips: Vec<ChatChip>,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatChip {
    pub key: String,
    pub label: String,
    /// The chip's ✕.
    pub remove: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatPicker {
    pub query: String,
    /// The highlighted candidate's index.
    pub selected: usize,
    pub items: Vec<ChatPickerItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatPickerItem {
    pub key: String,
    pub label: String,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorChip {
    /// As drawn: 「全部 3」, 「需要你 1」 ….
    pub label: String,
    /// The applied filter's chip.
    pub active: bool,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorGroup {
    /// needs_you | error | running | done | idle | terminals | ended
    pub name: &'static str,
    pub count: usize,
    /// Its cards are not drawn (已结束 until opened).
    pub collapsed: bool,
    /// The header.
    pub rect: Option<Rect4>,
}

/// One 补课 row (a turn of an agent card, a command of a terminal card).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorRow {
    pub text: String,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorCard {
    /// `agent:<claude|codex>:<session id>` or `pane:<pane id>`.
    pub key: String,
    /// "agent" | "terminal"
    pub kind: &'static str,
    /// Its group's name (`groups[].name`).
    pub group: &'static str,
    /// The title line as drawn.
    pub title: String,
    /// The status line as drawn (terminal: the running / 最后一条 line, 前台：… or 空闲).
    pub status_line: String,
    /// Agent: the meta row (git · 第 N 轮 · …); terminal: the error line of a failed last command; else "".
    pub meta_line: String,
    pub selected: bool,
    pub expanded: bool,
    pub rect: Option<Rect4>,
    /// The 跳过去 button.
    pub jump: Option<Rect4>,
    /// The 补课 / 收起 button.
    pub catchup: Option<Rect4>,
    /// The 补课 rows, newest first; [] unless expanded.
    pub rows: Vec<MonitorRow>,
    /// The ✦ block; null when the monitor is off or the directory is excluded.
    pub summary: Option<MonitorSummary>,
    /// 「✦ 重新总结」, when drawn.
    pub resummarize: Option<Rect4>,
    /// 「◎ 问它」, when drawn.
    pub ask: Option<Rect4>,
}

/// A monitor card's ✦ block (S2 §4.7).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorSummary {
    /// none | pending | ready | stale | failed | paused
    pub state: &'static str,
    /// As drawn: 「✦ AI 总结 · 刚刚 · 覆盖第 1 轮」, 「✦ 生成总结」, 「✦ 总结失败：… · 重试」.
    pub header: String,
    /// 「目标：…」; "" when absent (terminals, pending).
    pub goal: String,
    /// 「近期：…」; "" when absent.
    pub recent: String,
    pub rect: Option<Rect4>,
}

/// The editor pane's caret as the status bar shows it (1-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EditorCursor {
    pub line: usize,
    pub col: usize,
}

/// One editor pane (`windows[].editors[]`). Never carries the file's content.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditorState {
    pub pane: u64,
    /// Index of the tab the pane sits in.
    pub tab: usize,
    /// The absolute path of the file; "" for an unsaved buffer without one.
    pub path: String,
    pub dirty: bool,
    pub readonly: bool,
    pub cursor: EditorCursor,
    /// Characters in the selection (0 when none).
    pub selection_chars: usize,
    pub scroll_row: usize,
    /// Text columns of the wrapped layout (0 before the first paint).
    pub wrap_cols: usize,
    /// Visible text rows (0 before the first paint).
    pub rows: usize,
    /// "none" | "close" | "modified" | "save_error" | "deleted" | "confirm".
    pub bar: &'static str,
    /// The pane's text area; null when not drawn in the latest frame.
    pub rect: Option<Rect4>,
    /// The header's 「保存 ⌘S」.
    pub save_rect: Option<Rect4>,
    /// The header's 「✕」.
    pub close_rect: Option<Rect4>,
    /// Syntax highlighting: language, on/off and class counts of the visible rows.
    pub highlight: EditorHighlight,
    /// The live preview following this editor; null when none is open. Counts and the banner only, never text.
    pub preview: Option<LivePreviewState>,
    /// The header's 「预览」 button.
    pub preview_rect: Option<Rect4>,
    /// The buffer's encoding as the status bar names it ("UTF-8", "GBK", …).
    pub encoding: String,
    /// The line ending the next save writes: "LF" | "CRLF" | "CR".
    pub line_ending: &'static str,
    /// The file had more than one kind of line ending when it was read.
    pub mixed_line_endings: bool,
    /// The decoding replaced bytes it could not decode (the buffer is read-only).
    pub lossy: bool,
    /// The encoding was picked by hand (the encoding menu), not detected.
    pub explicit_encoding: bool,
    /// The flash shown instead of the cursor label (「已更新」), while it is live; null otherwise.
    pub status_flash: Option<String>,
    /// The 「对比」 overlay; null while closed.
    pub compare: Option<CompareState>,
    /// The open status-bar menu's items; null while none is drawn.
    pub menu: Option<Menu>,
    /// "encoding" | "line_ending": which menu `menu` is; null while none is drawn.
    pub menu_kind: Option<&'static str>,
    /// The notice bar's buttons, left to right ([] without a bar).
    pub bar_buttons: Vec<Button>,
    /// The status bar's encoding segment (a click opens the encoding menu).
    pub status_encoding_rect: Option<Rect4>,
    /// The status bar's line-ending segment (a click opens the line-ending menu).
    pub status_line_ending_rect: Option<Rect4>,
}

/// The editor pane's 「对比」 overlay (`editors[].compare`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompareState {
    /// Display rows after folding (a fold row counts once); 0 when the texts are the same or the file is gone.
    pub rows: usize,
    /// Side by side (the pane is at least 80 columns wide); false when unified or with no rows.
    pub split: bool,
    /// The file is gone from disk (「文件已被删除。」, only 「保留我的，稍后再说」).
    pub disk_missing: bool,
    /// The texts are the same (the 「内容相同…」 note instead of rows).
    pub same: bool,
    /// The overlay (it covers the whole pane).
    pub rect: Option<Rect4>,
    /// The bottom buttons, left to right.
    pub buttons: Vec<Button>,
}

/// `editors[].preview`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LivePreviewState {
    /// The preview pane's PaneId.
    pub pane: u64,
    /// "rendered" | "diagram" | "image" | "changes".
    pub provider: &'static str,
    /// Successful rebuilds so far (the first load counts).
    pub refreshes: u64,
    /// The banner on the preview (an error or 「文件太大…」), if any.
    pub banner: Option<String>,
}

/// `editors[].highlight` — counts only, never text.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditorHighlight {
    /// The status bar's language name without the 「（未高亮）」 suffix; "纯文本" for plain text.
    pub language: String,
    pub enabled: bool,
    /// "纯文本" | "文件较大" | null.
    pub disabled_reason: Option<&'static str>,
    /// Class name -> runs among the visible rows at the latest paint (zero classes omitted).
    pub visible_classes: std::collections::BTreeMap<&'static str, usize>,
}

/// A draggable column edge.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Divider {
    /// "center|inspector" (the pane area's edge with the inspector).
    pub between: &'static str,
    pub rect: Option<Rect4>,
}

/// A labelled button (confirm bars).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Button {
    pub label: String,
    pub rect: Option<Rect4>,
}

/// A filter chip (the timeline's, the 会话 palette's).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Chip {
    pub label: String,
    pub active: bool,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sidebar {
    pub visible: bool,
    /// "project" | "status".
    pub group_by: &'static str,
    /// Every section, collapsed ones included (empty while the sidebar is hidden).
    pub sections: Vec<Section>,
    /// The rows drawn, in visual order (a waiting session twice: under 需要你 and in its group).
    pub rows: Vec<Row>,
    pub trash_confirm: Option<TrashConfirm>,
    /// Terminal rows listed (the 终端 M of the header).
    pub terminals: usize,
    /// The header as drawn ("会话 · 3 · 终端 4").
    pub header: String,
    /// The hover tooltip on screen, if any.
    pub tooltip: Option<Tooltip>,
    /// The 按项目 / 按状态 buttons.
    pub group_buttons: Vec<GroupButton>,
    /// The 待 Review entry under the header; null while the sidebar is hidden.
    pub review_entry: Option<ReviewEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tooltip {
    /// The lines joined by `\n`.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroupButton {
    /// "project" | "status".
    pub name: &'static str,
    pub active: bool,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Section {
    /// "needs_you" | "project:<name>" | "status:<group>" | "ended", as in `Row::section`.
    pub name: String,
    /// The header as drawn ("需要你 · 1", "gilvt-lab", "已结束 · 2").
    pub title: String,
    pub collapsed: bool,
    /// Its sessions, collapsed or not.
    pub count: usize,
    /// The header (a click folds / unfolds it; 需要你's does nothing).
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    /// "agent" | "terminal" (a terminal pane no live agent sits in).
    pub kind: &'static str,
    /// The row's directory as the tooltip shows it; "" when unknown.
    pub cwd: String,
    /// "<agent>:<session_id>"; "" for terminal rows.
    pub session: String,
    /// "" for terminal rows.
    pub agent: &'static str,
    /// The name shown (the rename, else the first prompt, else 新会话).
    pub name: String,
    /// thinking | running_tool | awaiting_approval | awaiting_answer | idle | errored | ended | terminal (terminal rows).
    pub status: &'static str,
    /// The question / action / tool / error of the status line; "" when it has none.
    pub detail: String,
    /// The whole status line as drawn ("? 在问你 · Cats or dogs?", "✓ 完成未看 · 用时 4 分钟").
    pub line: String,
    pub muted: bool,
    /// 精简模式.
    pub lite: bool,
    /// The pane a click goes to; null for ended sessions and panes no window has.
    pub pane: Option<u64>,
    /// The session of this window's focused pane.
    pub focused: bool,
    pub section: String,
    /// Drawn: false for the rows of a collapsed section (listed so cases can find idle sessions), which
    /// then have no rect.
    pub visible: bool,
    pub rect: Option<Rect4>,
    /// Branch / dirty / ahead-behind of the session's directory; null without git, and for ended sessions
    /// and pending ones (their row draws no git line).
    pub git: Option<Git>,
    /// The ✦ line as drawn without 「✦ 」; null when not drawn.
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrashConfirm {
    pub sessions: Vec<String>,
    /// ［取消］［移到废纸篓］.
    pub buttons: Vec<Button>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tab {
    pub title: String,
    pub active: bool,
    /// The status dot: "amber" | "red" | "blue" | "green"; null for none.
    pub dot: Option<&'static str>,
    pub panes: Vec<Pane>,
    /// The tab in the tab bar.
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pane {
    pub id: u64,
    /// "terminal" | "preview" (a pinned preview) | "editor" (the built-in editor) | "monitor" (the 监控官 tab).
    pub kind: &'static str,
    /// The window's focused pane (active tab's focused pane).
    pub focused: bool,
    /// Null when not drawn in the latest frame (another tab, zoomed away).
    pub rect: Option<Rect4>,
    /// shell | agent:claude | agent:codex | other:<process> | unknown; null for previews.
    pub foreground: Option<String>,
    pub session: Option<String>,
    /// The outline's color: a status color ("amber" …) or "focus" (the blue focus border of a split tab).
    pub border: Option<&'static str>,
    pub cwd: Option<String>,
    /// The last `tail_lines` logical lines of the visible screen (soft-wrapped rows joined); [] for previews.
    pub screen_tail: Vec<String>,
    /// An input method's uncommitted composition (keys go to the IME, not the pty, while it is set).
    pub marked_text: Option<String>,
    /// Previews: where the code / diff text (the rendered document for Markdown) is drawn; null for terminals and before load.
    pub code: Option<Rect4>,
    /// Previews: the text `⌘C` would copy from the mouse selection; null without one and for terminals.
    pub selection: Option<String>,
    /// Pinned previews and editors: the header, dragged onto the tab bar to move the pane into a tab of its own;
    /// null for terminals / the monitor and when not drawn.
    pub header: Option<Rect4>,
    /// Terminals: the cell of the terminal cursor in the latest frame (also while a TUI hides it); null for
    /// previews / editors and before the first frame.
    pub cursor: Option<TermCursor>,
    /// Terminals: the newest 10 command blocks (OSC 133), newest first; [] for other panes.
    pub commands: Vec<CommandRow>,
    /// Terminals: the command running now ("" when the shell did not send its text); null otherwise.
    pub running_command: Option<String>,
    /// Terminals: "local" or the ssh host id (`user@hostname:port`); null for other panes.
    pub host: Option<String>,
    /// Terminals in an ssh link; null otherwise.
    pub remote: Option<PaneRemoteState>,
}

/// `panes[].remote`: the ssh link a terminal pane is in.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PaneRemoteState {
    pub link: String,
    pub host: String,
    pub display: String,
    pub hostname: Option<String>,
    pub enhanced: bool,
    pub cwd: Option<String>,
}

/// Top-level `hosts[]`: ssh hosts this run has seen.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HostState {
    pub id: String,
    pub display: String,
    pub hostname: Option<String>,
    pub install: &'static str,
    pub installed: Option<String>,
    pub bridge: &'static str,
    pub links: Vec<String>,
}

/// The `hosts` list from the app-wide remote state (BTreeMap order = sorted by id).
fn host_states(r: &crate::remote::RemoteHosts) -> Vec<HostState> {
    r.hosts
        .iter()
        .map(|(id, h)| {
            let prefs = r.prefs.hosts.get(id);
            let install = match prefs.and_then(|p| p.install.as_deref()) {
                Some("allowed") => "allowed",
                Some("never") => "never",
                _ => "ask",
            };
            let mut links = r.links_of(id);
            links.sort();
            HostState { id: id.clone(), display: h.display.clone(), hostname: h.hostname.clone(), install, installed: prefs.and_then(|p| p.installed.clone()), bridge: h.bridge.id(), links }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TermCursor {
    /// Viewport row and column, from 0.
    pub row: usize,
    pub col: usize,
    /// The cell.
    pub rect: Rect4,
}

/// One command block of a terminal pane.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommandRow {
    /// Increasing within the pane.
    pub id: u64,
    /// As typed; null when the shell did not send it.
    pub command: Option<String>,
    /// Null while it runs or when its end was not seen.
    pub exit: Option<i32>,
    pub running: bool,
    /// Its output tail was captured.
    pub has_output: bool,
    /// The last non-empty output line when the command failed; null otherwise.
    pub error_line: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Menu {
    pub items: Vec<MenuItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MenuItem {
    pub label: String,
    pub checked: bool,
    pub enabled: bool,
    pub rect: Option<Rect4>,
}

/// The overlay over the pane area.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Overlay {
    Quicklook {
        /// The file shown; null for stdin / nothing.
        path: Option<String>,
        /// The diff base label as drawn ("与 HEAD 相比", "仅文件"); null until loaded.
        base: Option<String>,
        /// "unified" | "split" | "rendered" (Markdown).
        mode: &'static str,
        /// Where the code / diff text is drawn (null in the rendered Markdown view and before load).
        code: Option<Rect4>,
        /// The text `⌘C` would copy from the mouse selection in the code view; null without one.
        selection: Option<String>,
        /// A turn's diff (`base` starts with 「本轮」): the file on disk differs from the turn's second snapshot.
        changed_since: bool,
    },
    Finder {
        query: String,
        selected: usize,
        /// Hits listed.
        items: usize,
    },
    Sessions {
        query: String,
        /// The cursor row.
        selected: usize,
        /// Rows listed (headers not counted).
        items: usize,
        filter: SessionsFilter,
        /// The rows listed, in order (a row scrolled out of the list has `rect: null`).
        rows: Vec<SessionsRow>,
        /// 「<project>」(only with a current project), 「全部项目」, 「≥ 7 天未活动」.
        chips: Vec<Chip>,
        /// The confirm bar of 移到废纸篓, in place of the key hints.
        confirm: Option<Confirm>,
        /// The red banner on top (「已复制会话 ID」, 「运行中的会话不能移到废纸篓」…).
        banner: Option<String>,
        /// The 「清理… ⌘⇧K」 entry of the key hints (null while the confirm bar replaces them).
        cleanup: Option<Button>,
    },
    /// The cleanup wizard (⌘⇧K) in place of the 会话 palette.
    Cleanup {
        /// The open preset, an index into `presets`.
        preset: usize,
        /// "presets" | "preview": the column ↑ / ↓ move in.
        column: &'static str,
        /// The cursor row of the preview.
        selected: usize,
        /// The sizes are still being computed: every count reads 「计算中…」 and both buttons are off.
        computing: bool,
        presets: Vec<CleanupPreset>,
        /// The open preset's hits, in order (a row scrolled out of the list has `rect: null`).
        rows: Vec<CleanupRow>,
        /// 「已选 N / M · X MB」.
        summary: String,
        /// 「归档 N 个」, 「移到废纸篓 N 个（…）」.
        buttons: Vec<CleanupButton>,
        /// The confirm bar of 移到废纸篓, in place of the summary and buttons.
        confirm: Option<Confirm>,
        /// The banner on top (「已归档 N 个会话」…).
        banner: Option<String>,
    },
    SessionCenter {
        generation: u64,
        /// "needs_you" | "review" | "running". The All tab keeps the legacy `sessions` shape.
        tab: &'static str,
        refreshing: bool,
        /// "smart" | "recent" | "project" | "oldest".
        sort: &'static str,
        query: String,
        store_initialized: bool,
        store_last_error: Option<String>,
        /// Sessions in the 待 Review queue (what the tab and the sidebar entry count).
        store_pending_count: usize,
        tabs: Vec<SessionCenterTab>,
        rows: Vec<SessionCenterRow>,
        /// The open read-only review; null while none is open.
        session_review: Option<SessionReviewState>,
    },
    NewAgent {
        /// "claude" | "codex".
        agent: &'static str,
        dir: String,
        task: String,
        preview: NewAgentPreview,
        /// The 更多 rows (model, permission mode) are open.
        more: bool,
        fields: Vec<Field>,
        /// 「在新 worktree 中运行」 ticked; null while the row is not drawn (the directory is not in a git repository).
        worktree: Option<bool>,
        /// Why the new worktree could not be made (the red line; the panel stays open); null without a failure.
        error: Option<String>,
    },
}

/// A preset line of the cleanup wizard.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CleanupPreset {
    pub label: String,
    pub hint: String,
    /// Sessions it hits; null while computing.
    pub count: Option<usize>,
    /// Their bytes (companion data included); null while computing.
    pub bytes: Option<u64>,
    /// The open preset.
    pub active: bool,
    pub rect: Option<Rect4>,
}

/// A preview row of the cleanup wizard.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CleanupRow {
    /// "<agent>:<session_id>".
    pub session: String,
    pub title: String,
    /// The session's directory as shown (`~/…`).
    pub dir: String,
    pub dir_missing: bool,
    /// Checked (acted on by the buttons).
    pub picked: bool,
    /// Pinned (📌; starts unchecked).
    pub pinned: bool,
    /// Tagged 「尚未 Review」.
    pub unreviewed: bool,
    /// The cursor row.
    pub selected: bool,
    pub rect: Option<Rect4>,
}

/// A button of the cleanup wizard's action bar.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CleanupButton {
    pub label: String,
    /// "archive" | "trash".
    pub action: &'static str,
    /// The preset's default action (drawn emphasised).
    pub default: bool,
    pub enabled: bool,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionCenterTab {
    pub name: &'static str,
    pub count: usize,
    pub active: bool,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionCenterRow {
    pub session_key: String,
    pub priority: &'static str,
    pub unreviewed_count: u32,
    pub selected: bool,
    /// Pinned (P in the review): sorts before the unpinned rows of its priority.
    pub pinned: bool,
    /// The session's directory as the row shows it (`~/…`, the palette's shortening); "" when unknown.
    pub dir: String,
    /// The directory no longer exists (struck through, tagged 「目录已不存在」).
    pub dir_missing: bool,
    /// "none" | "exact" | "inferred" | "unresolved".
    pub binding: &'static str,
    pub pid: Option<u32>,
    pub tty: Option<String>,
    pub rect: Option<Rect4>,
}

/// The 待 Review N entry of the sidebar (a click opens the Session Center on that tab).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewEntry {
    pub count: usize,
    pub rect: Option<Rect4>,
}

/// The read-only review of one session, as drawn in the latest frame.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionReviewState {
    /// "<agent>:<session_id>".
    pub session_key: String,
    /// "incremental" (only turns after the review cursor) | "full".
    pub mode: &'static str,
    /// "wide" (queue beside the review) | "narrow" (the review replaces the queue).
    pub layout: &'static str,
    pub loading: bool,
    /// Why the transcript could not be read; null without a failure.
    pub error: Option<String>,
    /// The red line after a failed save (nothing was marked reviewed / snoozed / pinned); null otherwise.
    pub notice: Option<String>,
    /// The last completed turn when the shown page was loaded, "<agent>:<turn id>"; what 「已 Review，下一个」 saves.
    pub snapshot_through: Option<String>,
    pub has_earlier: bool,
    pub has_later: bool,
    pub snooze_menu: bool,
    pub pinned: bool,
    /// The saved review position is no longer in the transcript: the page shows the newest turns, 「已 Review，
    /// 下一个」 is off, and the two recovery buttons are drawn.
    pub stale: bool,
    /// The turns on the shown page, in order.
    pub turns: Vec<ReviewTurnState>,
    pub actions: ReviewActions,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewTurnState {
    /// "<agent>:<turn id>" (a byte offset for the fallback cursor).
    pub cursor: String,
    pub ordinal: u32,
    /// "done" | "interrupted" | "failed".
    pub outcome: &'static str,
    /// The agent's final reply is shown (false: the card says 「未产生最终回复」).
    pub has_reply: bool,
    /// The 过程 section is unfolded.
    pub expanded: bool,
    pub rect: Option<Rect4>,
}

/// A button of the review: `enabled` is whether a click does anything; `rect` is null when it is not drawn.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewAction {
    pub enabled: bool,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewActions {
    pub review_next: ReviewAction,
    /// 「标记已 Review 并归档」 (⇧⌘E): off while the session runs in gilvt.
    pub review_next_archive: ReviewAction,
    pub skip: ReviewAction,
    pub snooze: ReviewAction,
    pub pin: ReviewAction,
    pub back_to_agent: ReviewAction,
    pub interrupt: ReviewAction,
    pub terminate: ReviewAction,
    pub copy_diagnostics: ReviewAction,
    pub terminate_confirming: bool,
    pub full_history: ReviewAction,
    /// Only drawn in the narrow layout.
    pub back: ReviewAction,
    /// Only drawn when the page has earlier / later turns.
    pub earlier: ReviewAction,
    pub later: ReviewAction,
    /// Only drawn while the snooze menu is open.
    pub snooze_hour: ReviewAction,
    pub snooze_later: ReviewAction,
    pub snooze_tomorrow: ReviewAction,
    pub snooze_cancel: ReviewAction,
    /// Only drawn (and enabled) while the saved position is stale: 「从当前开始」.
    pub baseline_here: ReviewAction,
    /// Only drawn (and enabled) while the saved position is stale: 「Review 全部可见历史」.
    pub review_all: ReviewAction,
}

/// A row of the 会话 palette.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionsRow {
    /// "<agent>:<session_id>".
    pub session: String,
    pub title: String,
    /// Where `title` comes from: "saved" (renamed in gilvt) | "custom" (the user's title in the agent) | "ai"
    /// (the agent's generated title) | "prompt" (derived from a prompt).
    pub title_source: &'static str,
    pub agent: &'static str,
    /// The grey line under the title (`Row::subtitle`, 「dir · branch · N 轮」): 「~/Workplace/auth · feature/pay · 3 轮」.
    pub meta: String,
    /// The session's directory as shown (`~/…`), the first part of `meta`.
    pub dir: String,
    /// The directory no longer exists (the row is struck through and tagged 「目录已不存在」).
    pub dir_missing: bool,
    /// The right side as drawn: 「● 运行中 · 标签 2」 or 「10 分钟前」.
    pub right: String,
    /// Running (it cannot be moved to the Trash).
    pub live: bool,
    /// "none" | "exact" | "inferred" | "unresolved".
    pub binding: &'static str,
    pub pid: Option<u32>,
    pub tty: Option<String>,
    /// The cursor row.
    pub selected: bool,
    /// Picked for the Trash (⌘ / ⇧ click, the stale filter's boxes).
    pub marked: bool,
    pub rect: Option<Rect4>,
}

/// A confirm bar: its text and buttons.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Confirm {
    pub text: String,
    pub buttons: Vec<Button>,
}

/// A field of the ⌘⇧N panel.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Field {
    /// "dir" | "prompt" | "more" | "model" | "permission" (model / permission only while 更多 is open).
    pub name: &'static str,
    /// The text (dir, prompt), the line (more), the chosen chip (model, permission).
    pub value: String,
    /// Has the keys.
    pub focused: bool,
    pub rect: Option<Rect4>,
}

/// The ⌘⇧N panel's preview lines.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NewAgentPreview {
    /// 「将在当前 pane 执行：」, or 目录不存在.
    pub head: String,
    pub command: String,
    /// The directory does not exist (drawn red).
    pub missing: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionsFilter {
    /// "current" | "all" (the scope applied).
    pub scope: &'static str,
    /// 「≥ 7 天未活动」.
    pub stale: bool,
    /// 「已归档」: only the archived sessions are listed.
    pub archived: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Inspector {
    pub visible: bool,
    /// "process" | "artifacts" | "config".
    pub tab: &'static str,
    /// The status card of the followed session.
    pub card: Option<Card>,
    /// The waiting banner's title without its ⏳ ("claude · gilvt-lab 在问你").
    pub banner: Option<String>,
    /// Timeline rows listed (turn titles and history turns included), 0 without a session.
    pub timeline_rows: usize,
    /// The waiting banner (a click is ⌘⇧J).
    pub banner_rect: Option<Rect4>,
    /// The column's width in points (kept while hidden).
    pub width: f32,
    /// The timeline filter: "全部" | "Bash" | "编辑" | "失败" (as the chips' labels).
    pub filter: &'static str,
    /// The filter chips over the current turn ([] without a turn).
    pub chips: Vec<Chip>,
    /// The note under the tab strip (「已复制」, 「已超出回滚范围」, a greyed tab's note).
    pub toast: Option<String>,
    /// Every timeline item listed, in order (`timeline_rows` of them); `rect: null` when not drawn.
    pub rows: Vec<TimelineRow>,
    /// The 「产物」 tab's content while it is the current tab; else null.
    pub artifacts: Option<Artifacts>,
    /// The M5a read-only configuration summary while 「配置」 is the current tab; else null.
    pub config: Option<ConfigSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigSummary {
    pub loading: bool,
    /// "shell" | "no_session" while the focused pane has no Agent; null otherwise.
    pub empty: Option<&'static str>,
    /// "claude" | "codex".
    pub agent: Option<&'static str>,
    pub cwd: Option<String>,
    pub model: Option<ConfigValue>,
    pub permission: Option<ConfigValue>,
    pub mcp: Vec<ConfigItem>,
    pub hooks: Vec<ConfigItem>,
    pub skills: usize,
    pub commands: usize,
    pub subagents: usize,
    /// The 「扩展」 openers (Skills, 命令, 子 Agent) as drawn.
    pub group_rects: Vec<Option<Rect4>>,
    /// Expanded MCP / hook / memory rows as `section/name`.
    pub expanded: Vec<String>,
    /// The Skills / commands / sub-agents dialog while it is open.
    pub dialog: Option<ConfigDialog>,
    pub memory: Vec<ConfigItem>,
    pub sources: Vec<ConfigSource>,
    pub warnings: Vec<String>,
    pub refresh_rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigValue {
    pub text: String,
    /// "runtime" | "user" | "project" | "local".
    pub source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigItem {
    pub name: String,
    pub detail: Option<String>,
    pub source: &'static str,
    /// The redacted facts shown when the row is expanded.
    pub lines: Vec<String>,
    /// The Markdown file 「打开」 shows; null for JSON / TOML entries.
    pub path: Option<String>,
    pub expanded: bool,
    pub row_rect: Option<Rect4>,
    /// Only while expanded and `path` is set.
    pub open_rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigDialog {
    /// "skills" | "commands" | "subagents".
    pub group: &'static str,
    pub title: String,
    pub rows: Vec<ConfigDialogRow>,
    pub close_rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigDialogRow {
    pub name: String,
    pub description: Option<String>,
    pub source: &'static str,
    pub path: String,
    pub rect: Option<Rect4>,
    /// The hover-only 「编辑」: its bounds whenever the row is drawn, even while the button is hidden (hover the row before clicking it); null only when the row is not drawn.
    pub edit_rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigSource {
    pub path: String,
    pub source: &'static str,
}

/// The 「产物」 tab's content (`inspector.artifacts`; null while another tab shows).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Artifacts {
    pub summary: Option<ArtifactsSummary>,
    /// The 「本会话净改动」 card; null when the model has no net.
    pub net: Option<ArtifactsNet>,
    /// The folded 「N 轮无文件改动」 rows, in drawing order.
    pub quiet_groups: Vec<ArtifactQuietGroup>,
    /// Why there are no cards: "shell" (not an agent pane) | "no_session" | "no_turns"; null with cards.
    pub empty: Option<&'static str>,
    /// Newest first, as drawn.
    pub cards: Vec<ArtifactCard>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArtifactsSummary {
    pub files: usize,
    /// Net lines; null while computing or when no range can be diffed.
    pub added: Option<u32>,
    pub removed: Option<u32>,
    pub computing: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArtifactsNet {
    pub open: bool,
    pub selected: bool,
    pub computing: bool,
    pub range_label: String,
    /// Listed only while the net is open (and has not failed).
    pub files: Vec<ArtifactFile>,
    pub excluded: usize,
    pub only_repo: Option<String>,
    pub failed: Option<String>,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArtifactQuietGroup {
    pub key: u32,
    pub count: usize,
    pub titles: Vec<String>,
    pub open: bool,
    pub selected: bool,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArtifactCard {
    /// Turn numbers (timeline) the task spans.
    pub turns: Vec<u32>,
    /// Heads of the follow-up prompts folded into the task.
    pub follow_ups: Vec<String>,
    /// Start time as drawn, e.g. "14:22".
    pub started: String,
    pub computing: bool,
    /// The per-file +/− are not net values and are not drawn.
    pub counts_hidden: bool,
    /// Key of the quiet group the card belongs to; null otherwise.
    pub quiet_group: Option<u32>,
    pub touched_later_turn: Option<u32>,
    /// The small line under the title, as drawn.
    pub sub_line: String,
    /// The turn's number in the session's timeline; null for a turn the timeline no longer has.
    pub turn: Option<u32>,
    pub title: String,
    /// done | running | quiet | degraded.
    pub state: &'static str,
    /// The degraded / large-file notes under the card, as drawn.
    pub notices: Vec<String>,
    pub expanded: bool,
    pub selected: bool,
    /// 「后被改动」: a later turn changed one of its files.
    pub touched_later: bool,
    pub test: Option<ArtifactTest>,
    /// The agent's closing sentence, without the 「」.
    pub quote: String,
    /// Files the card has but does not list (past the first 8).
    pub hidden_files: usize,
    pub files: Vec<ArtifactFile>,
    pub rect: Option<Rect4>,
    /// The 「另有 N 个文件 · 按目录分组查看全部」 link (null when the card has no hidden files).
    pub more_rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArtifactTest {
    pub ok: bool,
    pub command: String,
    pub exit: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArtifactFile {
    pub path: String,
    /// M | A | D | R.
    pub status: &'static str,
    pub added: u32,
    pub removed: u32,
    pub binary: bool,
    pub selected: bool,
    pub rect: Option<Rect4>,
}

/// An item of the inspector's timeline.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineRow {
    /// title | tool | subagent | thinking | returned | truncated | empty | note | history_turn.
    pub kind: &'static str,
    /// The text drawn: 「Bash echo early」, 「Update README.md +2 −1」, 「思考 · 4s」, 「时间线 · 第 2 轮」,
    /// 「第 1 轮 · fix it · 3 步 · 2s ✓」 (without ▸ / ▾).
    pub label: String,
    /// Tool rows: running | ok | failed | denied | interrupted | pending; null for other kinds.
    pub status: Option<&'static str>,
    /// A click jumps to the call's line in the terminal (tool rows with an anchor).
    pub anchored: bool,
    /// The detail (tool, thinking) or the turn (history_turn) is open.
    pub expanded: bool,
    /// An edit's line counts.
    pub lines: Option<LineCounts>,
    /// Inside a subagent (behind the purple rule).
    pub nested: bool,
    /// Inside an expanded history turn.
    pub history: bool,
    /// The error lines under a failed call.
    pub error: Vec<String>,
    /// The row's line (tool rows: without the error lines and the detail below it).
    pub rect: Option<Rect4>,
    /// The ▸ / ▾ in front of a tool row (drawn only while hovered or open).
    pub toggle: Option<Rect4>,
    /// The underlined file name (⌘+click: Quick Look).
    pub file: Option<Rect4>,
    /// The 「复制」 button of an open detail.
    pub copy: Option<Rect4>,
    /// title / history_turn: when the turn started, as drawn (「14:30:12」 for the title, 「14:30」 /
    /// 「昨天 14:30」 for a history turn); null for other kinds or an unknown start.
    pub started: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LineCounts {
    pub added: u32,
    pub removed: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Card {
    /// As `Row::status`.
    pub status: &'static str,
    /// 第 N 轮; null before the first turn.
    pub turn: Option<u32>,
}

/// The ⌘, settings window (top level; absent while it is closed).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingsState {
    /// The CGWindowID; null when unknown.
    pub id: Option<u64>,
    /// Is it the key window?
    pub key: bool,
    /// The page shown: "appearance", "language", or "monitor".
    pub page: &'static str,
    /// The active application language: "zh-CN" or "en".
    pub language: &'static str,
    /// Where `language` comes from: "config" (config.toml sets `language`) or "system" (macOS's preferred language).
    pub language_source: &'static str,
    /// Language choices, including their click rectangles while the language page is shown.
    pub languages: Vec<SettingsLanguage>,
    /// config.toml does not parse: every control is disabled.
    pub readonly: bool,
    /// Why (the parser's message, starting with the file's path).
    pub error: Option<String>,
    /// The last write-back failed.
    pub write_error: Option<String>,
    pub config_path: String,
    pub fields: Vec<SettingsField>,
    /// The 「其他…」 / CLI path text box while it is open.
    pub other: Option<SettingsOther>,
    /// The line under 测试连接 (trial runs, refusals).
    pub notice: Option<String>,
    pub notice_error: bool,
    pub test: SettingsTest,
    /// The nav column's pages, in order.
    pub pages: Vec<SettingsPage>,
    /// The 「外观」 page (its rects only while it is the page shown).
    pub appearance: AppearanceState,
}

/// A page in the settings window's nav column.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingsPage {
    /// "appearance" | "language" | "monitor".
    pub id: &'static str,
    pub label: &'static str,
    /// The page shown.
    pub selected: bool,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingsLanguage {
    /// The config value: "zh-CN" or "en".
    pub id: &'static str,
    pub label: &'static str,
    pub selected: bool,
    pub rect: Option<Rect4>,
}

/// The settings window's 「外观」 page.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppearanceState {
    /// The search box's text.
    pub query: String,
    /// "all" | "dark" | "light".
    pub filter: &'static str,
    /// "fixed" | "system".
    pub mode: &'static str,
    /// The slot being edited in system mode ("light" | "dark"); null in fixed mode.
    pub slot: Option<&'static str>,
    /// The highlighted row's theme.
    pub selected: Option<String>,
    /// The page's choice: the fixed theme, or the light / dark slots in system mode.
    pub fixed: Option<String>,
    pub light: Option<String>,
    pub dark: Option<String>,
    /// The first 50 rows shown.
    pub rows: Vec<ThemeRow>,
    /// filter-all | filter-dark | filter-light | mode-fixed | mode-system, and slot-light | slot-dark in system mode.
    pub chips: Vec<ThemeChip>,
    /// The search box.
    pub search_rect: Option<Rect4>,
    /// How many `[colors]` overrides apply (the page says so when there are any).
    pub colors_overrides: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingsField {
    /// enabled | provider | model | summary_model | refresh_models | command | choose_command | test | auto_summary |
    /// summary_interval | sidebar_summary | exclude_paths | add_exclude | open_config.
    pub id: &'static str,
    /// What the control shows (「CLI 默认」, 「gpt-x ⚠ 不在列表中」, 「自定义：90s」).
    pub label: String,
    /// The config value: bool, string or list; null for buttons.
    pub value: serde_json::Value,
    pub hint: Option<String>,
    /// A dropdown that is open.
    pub open: bool,
    pub options: Vec<SettingsOption>,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingsOption {
    pub label: String,
    pub selected: bool,
    /// Segments and exclude ×: always drawn; dropdown items: only while it is open.
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingsOther {
    pub field: &'static str,
    pub text: String,
    /// The trial run is going.
    pub trying: bool,
    pub rect: Option<Rect4>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingsTest {
    /// idle | running | ok | failed.
    pub state: &'static str,
    pub text: String,
}

/// The top-level fields, from plain data; `windows` is filled in by the caller.
pub fn top_level(pid: u32, front: bool, badge: Option<&str>, bounces: u64) -> DebugState {
    DebugState { version: VERSION, pid, front, dock_badge: badge.map(str::to_string), dock_bounces: bounces, pending: Vec::new(), theme: None, windows: Vec::new(), settings: None, chat_process: ChatProcessState::default(), update: UpdateState::default(), hosts: Vec::new() }
}

/// Takes the snapshot. `tail_lines` is how much of each pane's screen to include.
pub fn collect(tail_lines: u16, cx: &mut App) -> DebugState {
    rects::forget_closed_windows(cx);
    let tail_lines = tail_lines.min(MAX_TAIL);
    let front = notify::frontmost(cx);
    let (badge, bounces) = notify::dock_shown(cx);
    let mut state = top_level(std::process::id(), front, badge.as_deref(), bounces);
    state.pending = cx.try_global::<Agents>().map(|a| map::pending(a.pending())).unwrap_or_default();
    let theme = crate::theme::current(cx);
    state.theme = Some(ThemeInfo {
        name: theme.name.clone(),
        dark: theme.dark,
        source: theme.source.as_str(),
        colors_overrides: theme.overrides,
        error: theme.error.clone(),
    });
    state.windows = collect::windows(tail_lines as usize, cx);
    state.settings = collect::settings(cx);
    state.chat_process = crate::monitor::chat::process_state(cx);
    state.update = crate::updater::debug(cx);
    state.hosts = cx.try_global::<crate::remote::RemoteHosts>().map(host_states).unwrap_or_default();
    state
}

/// The debug state queries of one batch still worth answering at `now`, with their tail (clamped to
/// [`MAX_TAIL`]). Expired ones are dropped unanswered (their client has given up); anything but
/// `DebugState` is answered with an error.
pub fn live_queries(batch: Vec<Query>, now: Instant) -> Vec<(Query, u16)> {
    let mut out = Vec::with_capacity(batch.len());
    for query in batch {
        if query.expired_at(now) {
            continue;
        }
        match query.request {
            Request::DebugState { tail_lines } => out.push((query, tail_lines.min(MAX_TAIL))),
            _ => query.respond(Response::Error { message: "not a query".into() }),
        }
    }
    out
}

/// `state` with each pane's `screen_tail` cut to its last `tail` lines: one snapshot, taken with the largest
/// tail of a batch, answers every query of it. (`screen_tail(n)` is the last n lines of `screen_tail(m)`
/// for m ≥ n.)
pub fn with_tail(state: &DebugState, tail: usize) -> DebugState {
    let mut state = state.clone();
    for pane in state.windows.iter_mut().flat_map(|w| w.tabs.iter_mut()).flat_map(|t| t.panes.iter_mut()) {
        let extra = pane.screen_tail.len().saturating_sub(tail);
        pane.screen_tail.drain(..extra);
    }
    state
}

/// The answer to one query of a batch whose snapshot is `state`.
pub fn answer(state: &DebugState, tail_lines: u16) -> Response {
    to_response(&with_tail(state, tail_lines as usize))
}

fn to_response(state: &DebugState) -> Response {
    match serde_json::to_value(state) {
        Ok(mut state) => {
            untranslated::add(&mut state);
            Response::DebugState { state }
        }
        Err(e) => Response::Error { message: format!("debug state: {e}") },
    }
}

#[cfg(test)]
mod tests;
