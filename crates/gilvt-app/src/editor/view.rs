//! `EditorView`: the gpui entity of one editor pane. Owns the `EditorModel`, focus, keyboard, IME, mouse and
//! scroll handling and the notice bar; painting of the text is `EditorElement`, the header / bar / status bar
//! are `chrome`.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{
    div, prelude::*, px, Bounds, ClipboardItem, Context, CursorStyle, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent, Subscription, Task,
    UTF16Selection, Window,
};
use gilvt_editor::{Buffer, EditorError, ExternalState, OpenOptions, Position, Selection};

use super::chrome;
use super::compare::{self, Compare, CompareAction, CompareKey};
use super::element::{EditorElement, TEXT_PAD};
use super::external::{self, ExternalAction, DEBOUNCE_MS, FLASH_MS, FLASH_UPDATED};
use super::model::{EditorModel, HMove};
use super::popup::{self, MenuAction, MenuItem, MenuKind, OpenMenu, PopupKey};
use crate::actions::{Copy, EditorCut, EditorRedo, EditorSave, EditorSelectAll, EditorUndo, OpenExternalEditor, Paste, EDITOR_CONTEXT};
use crate::pane_tree::PaneId;
use crate::theme::{hsla, AppSettings};

pub enum EditorEvent {
    /// Title-relevant state (dirty flag) changed.
    Changed,
    /// The pane asks to be closed (after any confirmation).
    CloseRequested,
    /// Focus should leave the editor (e.g. Esc).
    Leave,
    /// The buffer text changed (undo/redo included).
    TextChanged,
    /// A save succeeded.
    Saved,
    /// The first shown row now belongs to this logical line (1-based).
    Scrolled(usize),
    /// Header button / shortcut: open or close the live preview.
    TogglePreview,
    /// Header button: switch the live preview's provider.
    CyclePreview,
}

/// The one bar shown under the header (Task 7 draws it).
#[derive(Clone, Debug, PartialEq)]
pub enum Bar {
    None,
    /// 要保存对 X 的修改吗？
    Close,
    /// 文件已在磁盘上被修改，而你有未保存的改动。: 重新载入 / 对比 / 仍然覆盖. `closing`: reached from the close
    /// bar's 保存, so a successful 仍然覆盖 closes the pane.
    Modified { closing: bool },
    SaveError { message: String, jump: Option<(usize, usize)> },
    /// 文件已被删除或移走: 保存（重新创建） / 关闭 / 知道了.
    Deleted,
    /// Confirm reopening with other options (discards unsaved edits): 取消 / 放弃改动并重新打开.
    Confirm(ReopenIntent),
}

/// What a reopen would do once confirmed (E2b-1 §6): the options to reopen the file with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReopenIntent(pub gilvt_editor::OpenOptions);

/// Geometry of the last painted text area, used to map mouse positions to display rows / cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LayoutInfo {
    pub bounds: Bounds<Pixels>,
    /// Top-left of text column 0 (right of the gutter and its padding).
    pub text_origin: Point<Pixels>,
    pub cell_w: Pixels,
    pub line_h: Pixels,
    pub cols: usize,
    pub rows: usize,
}

/// A pointer position in text-area terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Hit {
    /// View row, clamped to `-1..=rows`: one row past either edge means "drag-scroll that way".
    pub row: i64,
    /// Cells from text column 0 (fractional, never negative).
    pub x: f32,
    /// In the line-number gutter (left of the text padding).
    pub gutter: bool,
}

pub(crate) fn pixel_to_cell(pos: Point<Pixels>, l: &LayoutInfo) -> Hit {
    let x = (pos.x - l.text_origin.x) / l.cell_w;
    let y = (pos.y - l.bounds.origin.y) / l.line_h;
    Hit { row: (y.floor() as i64).clamp(-1, l.rows as i64), x: x.max(0.), gutter: pos.x < l.text_origin.x - px(TEXT_PAD) }
}

/// Result of [`EditorView::save`]; the close / quit flows continue only on `Saved`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved,
    /// The file changed on disk: the 「重新载入 / 对比 / 仍然覆盖」 bar is up.
    Modified,
    /// The red save-error bar is up (or the buffer is read-only and nothing happened).
    Failed,
}

/// A non-text key the editor handles itself. Plain characters (and space) go through the input handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyCmd {
    Move(HMove, bool),
    /// Display rows, extend.
    Rows(i64, bool),
    /// Pages (view height − 1 rows), extend.
    Page(i64, bool),
    Home(bool),
    End(bool),
    Enter,
    /// Shift (outdent).
    Tab(bool),
    Backspace,
    DeleteWordBack,
    Delete,
    Escape,
}

/// The editor's own keys (E2a §5.2): ⇧ extends; ⌥ ←/→ by word; ⌘ ←/→ line start / end; ⌘ ↑/↓ document start / end.
/// `None` lets the key go on to the input handler. Known gap: ⌘⇧↑ / ⌘⇧↓ remain global session navigation
/// (PrevSession / NextSession) because spec §5.5 keeps the terminal-pane shortcuts identical in the editor pane,
/// so ⇧-extending to the document start / end is not available (spec §5.2/§5.5; recorded in the SDD ledger).
/// gpui dispatches key bindings before key-down listeners, so they would never reach here anyway; they map to `None`.
pub(crate) fn key_command(key: &str, ctrl: bool, shift: bool, alt: bool, platform: bool) -> Option<KeyCmd> {
    use HMove::*;
    use KeyCmd::*;
    if ctrl {
        return None;
    }
    Some(match (key, alt, platform) {
        ("left", false, false) => Move(Left, shift),
        ("right", false, false) => Move(Right, shift),
        ("left", true, false) => Move(WordLeft, shift),
        ("right", true, false) => Move(WordRight, shift),
        ("left", false, true) => Move(LineStart, shift),
        ("right", false, true) => Move(LineEnd, shift),
        ("up", false, false) => Rows(-1, shift),
        ("down", false, false) => Rows(1, shift),
        ("up", false, true) if !shift => Move(DocStart, false),
        ("down", false, true) if !shift => Move(DocEnd, false),
        ("home", false, false) => Home(shift),
        ("end", false, false) => End(shift),
        ("pageup", false, false) => Page(-1, shift),
        ("pagedown", false, false) => Page(1, shift),
        ("enter", false, false) => Enter,
        ("tab", false, false) => Tab(shift),
        ("backspace", false, false) => Backspace,
        ("backspace", true, false) => DeleteWordBack,
        ("delete", false, false) => Delete,
        ("escape", false, false) => Escape,
        _ => return None,
    })
}

/// A notice-bar button (or its key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BarAction {
    /// 取消 / Esc: hide the bar, keep the buffer.
    Cancel,
    /// 不保存: close without saving.
    Discard,
    /// 保存 ⏎ on the close bar: save, then close if that worked.
    SaveAndClose,
    /// 仍然覆盖.
    Overwrite,
    /// 跳到出错位置.
    Jump,
    /// 重新载入: take the disk version as one undoable edit.
    Reload,
    /// 对比: open the compare overlay (Task 6).
    Compare,
    /// 保存（重新创建） on the deleted bar: a plain save.
    Recreate,
    /// 关闭 on the deleted bar: the normal close request (close bar when dirty).
    CloseNow,
    /// 放弃改动并重新打开.
    ConfirmReopen,
}

/// What `cmd` does while `bar` is shown; `None` lets it reach the text (Esc with no bar leaves the editor).
pub(crate) fn bar_key(bar: &Bar, cmd: KeyCmd) -> Option<BarAction> {
    match (bar, cmd) {
        (Bar::None, _) => None,
        (Bar::Close, KeyCmd::Enter) => Some(BarAction::SaveAndClose),
        // Enter does nothing on Modified / Deleted / Confirm: no accidental overwrite or discard.
        (_, KeyCmd::Escape) => Some(BarAction::Cancel),
        _ => None,
    }
}

/// The bar a failed save leaves up (E2a §6).
pub(crate) fn bar_for_save_error(e: &EditorError) -> Bar {
    match e {
        EditorError::ModifiedOnDisk => Bar::Modified { closing: false },
        EditorError::Unrepresentable { line, col } => Bar::SaveError { message: e.to_string(), jump: Some((*line, *col)) },
        e => Bar::SaveError { message: e.to_string(), jump: None },
    }
}

/// `request_close`: the close bar when something would be lost, else `None` (close at once).
pub(crate) fn close_bar(dirty: bool) -> Option<Bar> {
    dirty.then_some(Bar::Close)
}

/// What a save leaves behind: the new bar, its outcome, and whether the pane closes.
/// `closing`: the save was started by the close bar (保存) or by 仍然覆盖 on a `Modified { closing: true }` bar.
/// The flag lives in the bar, so it cannot outlive the bar that carries it.
pub(crate) fn save_step(closing: bool, r: Result<(), &EditorError>) -> (Bar, SaveOutcome, bool) {
    match r {
        Ok(()) => (Bar::None, SaveOutcome::Saved, closing),
        Err(EditorError::ModifiedOnDisk) => (Bar::Modified { closing }, SaveOutcome::Modified, false),
        Err(e) => (bar_for_save_error(e), SaveOutcome::Failed, false),
    }
}

/// What a bar button does, before any I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Plan {
    /// Hide the bar; `close` also closes the pane.
    Hide { close: bool },
    /// Save (or overwrite) via [`save_step`] with this `closing` flag.
    Save { overwrite: bool, closing: bool },
    Jump,
    /// 重新载入.
    Reload,
    /// 对比.
    Compare,
    /// `request_close`.
    CloseRequest,
    /// Reopen with these options (discards unsaved edits; the confirm bar was clicked).
    Reopen(gilvt_editor::OpenOptions),
}

pub(crate) fn plan(bar: &Bar, action: BarAction) -> Plan {
    match action {
        BarAction::Cancel => Plan::Hide { close: false },
        BarAction::Discard => Plan::Hide { close: true },
        BarAction::SaveAndClose => Plan::Save { overwrite: false, closing: true },
        BarAction::Overwrite => Plan::Save { overwrite: true, closing: matches!(bar, Bar::Modified { closing: true }) },
        BarAction::Jump => Plan::Jump,
        BarAction::Reload => Plan::Reload,
        BarAction::Compare => Plan::Compare,
        BarAction::Recreate => Plan::Save { overwrite: false, closing: false },
        BarAction::CloseNow => Plan::CloseRequest,
        BarAction::ConfirmReopen => match bar {
            Bar::Confirm(i) => Plan::Reopen(i.0),
            // Only the confirm bar carries an intent; anything else just hides the bar.
            _ => Plan::Hide { close: false },
        },
    }
}

/// What picking an encoding / read-only item does (E2b-1 §6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReopenStep {
    /// Unsaved edits would be lost: put up the 「放弃改动并重新打开」 bar.
    Confirm(ReopenIntent),
    /// Nothing to lose: reopen now.
    Now(OpenOptions),
}

pub(crate) fn reopen_plan(dirty: bool, opts: OpenOptions) -> ReopenStep {
    if dirty { ReopenStep::Confirm(ReopenIntent(opts)) } else { ReopenStep::Now(opts) }
}

/// A UTF-16 range from the input handler as an ordered, clamped char range of `buf`.
pub(crate) fn utf16_to_chars(buf: &Buffer, r: &Range<usize>) -> Range<usize> {
    let (a, b) = (buf.utf16_to_char(r.start), buf.utf16_to_char(r.end));
    a.min(b)..a.max(b)
}

pub(crate) fn chars_to_utf16(buf: &Buffer, r: Range<usize>) -> Range<usize> {
    buf.char_to_utf16(r.start)..buf.char_to_utf16(r.end)
}

/// IME preedit as stored for drawing: line breaks become spaces (the element shapes it as one line).
pub(crate) fn preedit(text: &str) -> Option<String> {
    let t: String = text.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    (!t.is_empty()).then_some(t)
}

/// Committed IME / typed text with stray control characters dropped (Enter and Tab arrive as keys).
pub(crate) fn typed_text(text: &str) -> String {
    text.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect()
}

/// Char offset of an `Unrepresentable { line, col }` (col in chars), clamped to the line.
pub(crate) fn jump_index(buf: &Buffer, line: usize, col: usize) -> usize {
    let line = line.min(buf.line_count().saturating_sub(1));
    let start = buf.char_index(Position { line, col: 0 });
    start + col.min(buf.line_len_chars(line))
}

/// What to watch for `path`: its directory (resolved, as FSEvents reports real paths) and the file in it.
/// Same rule as the preview: the directory, not the file, because editors and agents replace files by rename.
pub(crate) fn watch_target(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let file = crate::preview_view::canonical_target(path)?;
    Some((file.parent()?.to_path_buf(), file))
}

/// Whether a watcher event is about `target` (a canonical path from [`watch_target`]). Access events are
/// ignored; the cheap file-name check runs first so unrelated files in the directory cost no canonicalize.
pub(crate) fn event_hits(event: &notify::Event, target: &Path) -> bool {
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return false;
    }
    event.paths.iter().any(|p| {
        p.file_name() == target.file_name()
            && (p == target || crate::preview_view::canonical_target(p).as_deref() == Some(target))
    })
}

/// A silent reload waits while an IME composition is open (its preedit would be dropped); the next watcher
/// event or focus check tries again.
pub(crate) fn may_reload_now(composing: bool) -> bool {
    !composing
}

/// Empties the watcher queue after the debounce: one check covers the whole burst. Returns how many it dropped.
pub(crate) fn drain_burst(rx: &async_channel::Receiver<()>) -> usize {
    std::iter::from_fn(|| rx.try_recv().ok()).count()
}

/// The flash text while it is younger than `FLASH_MS` (the timer task clears it; this also guards a late paint).
pub(crate) fn live_flash(flash: Option<&(String, Instant)>, now: Instant) -> Option<&str> {
    flash.filter(|(_, at)| now.saturating_duration_since(*at) < Duration::from_millis(FLASH_MS)).map(|(t, _)| t.as_str())
}

/// What changes the remembered dismissal (`acked`).
#[derive(Clone, Copy, Debug)]
pub(crate) enum AckEvent<'a> {
    /// The user hid this bar (取消 / Esc / 知道了).
    Dismissed(&'a Bar),
    /// `check_external` returned this.
    Checked(ExternalState),
    /// A successful save / reload / reopen: buffer and disk agree again.
    Resynced,
}

/// The disk state the user dismissed a bar for, so the next watcher event or focus check does not undo the
/// dismissal (E2b-1 §4: Esc hides the banner, the save stays blocked). Forgotten once the disk state changes.
pub(crate) fn next_acked(acked: Option<ExternalState>, event: AckEvent) -> Option<ExternalState> {
    match event {
        AckEvent::Dismissed(Bar::Modified { .. }) => Some(ExternalState::Modified),
        AckEvent::Dismissed(Bar::Deleted) => Some(ExternalState::Deleted),
        AckEvent::Dismissed(_) => acked,
        AckEvent::Checked(state) => acked.filter(|a| *a == state),
        AckEvent::Resynced => None,
    }
}

/// The bar after the disk version was taken in: the external-change bars are answered, others stay.
pub(crate) fn bar_after_reload(bar: &Bar) -> Bar {
    match bar {
        Bar::Modified { .. } | Bar::Deleted => Bar::None,
        b => b.clone(),
    }
}

/// Adds a wheel `delta` to `accum` (in rows of `line_h`) and takes out the whole rows; the fraction stays for
/// the next event. Positive = towards the top of the document.
pub(crate) fn wheel_lines(accum: &mut f32, delta: ScrollDelta, line_h: Pixels) -> i64 {
    *accum += match delta {
        ScrollDelta::Pixels(p) => p.y / line_h,
        ScrollDelta::Lines(l) => l.y,
    };
    let lines = accum.trunc();
    *accum -= lines;
    lines as i64
}

/// Whether the text is covered (inert to keys, mouse, IME, edits and ⌘S): whenever a compare overlay is open,
/// whatever it shows, or a status-bar menu is open.
pub(crate) fn covered_by(compare: Option<&Compare>, menu_open: bool) -> bool {
    compare.is_some() || menu_open
}

/// The whole pane width in cells (gutter included): what the compare overlay's split / unified choice uses.
pub(crate) fn pane_cols(l: &LayoutInfo) -> usize {
    (l.bounds.size.width / l.cell_w).max(0.) as usize
}

/// Row height of the compare overlay: the editor's own line height.
pub(crate) fn compare_line_h(s: &crate::settings::Settings) -> Pixels {
    px((s.font_size * s.line_height).round())
}

/// Opens `path` into a model sized 80×24; the first paint applies the real viewport.
// The default-options wrapper; the Workspace goes through `load_model_with` (it always has options).
#[cfg_attr(not(test), allow(dead_code))]
pub fn load_model(path: &Path) -> Result<EditorModel, EditorError> {
    load_model_with(path, OpenOptions::default())
}

/// [`load_model`] with an encoding / read-only choice (the refused-file banner's reopen buttons).
pub fn load_model_with(path: &Path, opts: OpenOptions) -> Result<EditorModel, EditorError> {
    let buf = Buffer::open_with(path, opts)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(EditorModel::new(buf, &name, 80, 24))
}

pub struct EditorView {
    pub(crate) model: EditorModel,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) layout: Option<LayoutInfo>,
    /// IME preedit text; drawn at the caret, never in the buffer.
    pub(crate) marked: Option<String>,
    pub(crate) bar: Bar,
    /// Set by the Workspace.
    pub pane: Option<PaneId>,
    scroll_accum: f32,
    selecting: bool,
    last_dirty: bool,
    /// Which live preview follows this buffer (written by the Workspace).
    pub live: Option<crate::live_preview::Provider>,
    last_revision: u64,
    last_top_line: usize,
    /// Class-run counts of the rows visible at the last paint (for DebugState).
    pub(crate) hl_stats: std::collections::BTreeMap<&'static str, usize>,
    /// Transient status text (「已更新」) and when it was set; shown instead of the cursor label for `FLASH_MS`.
    pub(crate) status_flash: Option<(String, Instant)>,
    /// Clears `status_flash`; replaced (and so cancelled) by the next flash.
    _flash_task: Option<Task<()>>,
    /// The disk state whose bar the user dismissed; see [`next_acked`].
    acked: Option<ExternalState>,
    /// Watches the file's directory; `None` when it could not be created (focus / save checks still work).
    _watcher: Option<notify::RecommendedWatcher>,
    /// Debounces watcher events into `on_external_change`.
    _watch: Option<Task<()>>,
    /// Checks the disk when the editor gains focus.
    _focus_sub: Option<Subscription>,
    /// The 「对比」 overlay; while it is open it owns the keyboard and the mouse (see `on_key_down`).
    pub(crate) compare: Option<Compare>,
    /// The status-bar menu (encoding / line ending); while it is open it owns the keyboard (see `on_key_down`).
    pub(crate) menu: Option<OpenMenu>,
    /// `menu` was opened before the status bar was ever laid out (the refused-file banner's 「选择编码打开…」
    /// on a new pane): its `at` is placed, and the menu drawn, once the encoding segment has been laid out.
    menu_unplaced: bool,
    /// The status bar's encoding segment as last laid out (window coordinates).
    pub(crate) status_encoding: Option<Bounds<Pixels>>,
    /// Where the left click that closed the menu (by clicking outside it) went down: that click must not also
    /// move the caret. Taken by the next mouse-down on the pane.
    menu_closed_at: Option<Point<Pixels>>,
}

impl EventEmitter<EditorEvent> for EditorView {}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EditorView {
    pub fn new(model: EditorModel, cx: &mut Context<Self>) -> Self {
        let last_dirty = model.buf.dirty();
        let last_revision = model.buf.revision();
        let last_top_line = model.top_line();
        Self {
            model,
            focus_handle: cx.focus_handle(),
            layout: None,
            marked: None,
            bar: Bar::None,
            pane: None,
            scroll_accum: 0.0,
            selecting: false,
            last_dirty,
            live: None,
            last_revision,
            last_top_line,
            hl_stats: Default::default(),
            status_flash: None,
            _flash_task: None,
            acked: None,
            _watcher: None,
            _watch: None,
            _focus_sub: None,
            compare: None,
            menu: None,
            menu_unplaced: false,
            status_encoding: None,
            menu_closed_at: None,
        }
    }

    /// Starts the file watcher and the focus check. Call once after `cx.new` (needs a Window).
    pub fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._focus_sub = Some(cx.on_focus(&self.focus_handle, window, |v, _, cx| v.on_external_change(cx)));
        self.watch(cx);
    }

    /// Same chain as the preview (`notify` callback → channel → task), plus a real debounce: after the first
    /// event of a burst wait `DEBOUNCE_MS`, drop the rest of the burst, then check once. A watcher that cannot
    /// be created leaves only the focus / save checks.
    fn watch(&mut self, cx: &mut Context<Self>) {
        use notify::Watcher;
        self._watcher = None;
        self._watch = None;
        let Some((dir, target)) = self.path().and_then(watch_target) else { return };
        let (tx, rx) = async_channel::unbounded::<()>();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok_and(|e| event_hits(&e, &target)) {
                let _ = tx.try_send(());
            }
        });
        let Ok(mut watcher) = watcher else { return };
        if watcher.watch(&dir, notify::RecursiveMode::NonRecursive).is_err() {
            return;
        }
        self._watcher = Some(watcher);
        self._watch = Some(cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                cx.background_executor().timer(Duration::from_millis(DEBOUNCE_MS)).await;
                drain_burst(&rx);
                if this.update(cx, |v, cx| v.on_external_change(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// `check_external` → [`external::decide`] → act (E2b-1 §4). Our own saves read `Unchanged` here because E1
    /// refreshes the disk fingerprint inside `save`.
    pub(crate) fn on_external_change(&mut self, cx: &mut Context<Self>) {
        let state = self.model.buf.check_external();
        self.acked = next_acked(self.acked, AckEvent::Checked(state));
        match external::decide(state, self.model.buf.dirty(), &self.bar, self.acked) {
            ExternalAction::Nothing => {}
            // Only reached when nothing is unsaved: E1 `reload` (history cleared, caret kept, options kept).
            // Deferred while an IME composition is open; the next event or focus check tries again.
            ExternalAction::SilentReload => {
                if !may_reload_now(self.marked.is_some()) {
                    return;
                }
                // A file that outgrew the limit is never read here (every event would stall the UI): one red bar.
                if let Some(len) = self.model.buf.path().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()) {
                    if let Some(bar) = external::too_large_notice(len, &self.bar) {
                        self.set_bar(bar, cx);
                    }
                    if len > gilvt_editor::MAX_FILE_SIZE {
                        return;
                    }
                }
                // Any other failed reload is swallowed: the data stays safe, a later save is blocked by `ModifiedOnDisk`.
                if self.model.reload().is_ok() {
                    self.close_menu_on_model_change();
                    self.selecting = false;
                    self.bar = bar_after_reload(&self.bar);
                    self.acked = next_acked(self.acked, AckEvent::Resynced);
                    self.flash(FLASH_UPDATED, cx);
                    self.after_edit(cx);
                }
            }
            ExternalAction::ShowModified => self.set_bar(Bar::Modified { closing: false }, cx),
            ExternalAction::ShowDeleted => self.set_bar(Bar::Deleted, cx),
        }
    }

    /// Shows `text` at the left of the status bar for `FLASH_MS`; a new flash replaces the old one.
    pub(crate) fn flash(&mut self, text: &str, cx: &mut Context<Self>) {
        self.status_flash = Some((text.into(), Instant::now()));
        self._flash_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(FLASH_MS)).await;
            let _ = this.update(cx, |v, cx| {
                v.status_flash = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The flash text to draw now, if any.
    pub(crate) fn flash_text(&self) -> Option<&str> {
        live_flash(self.status_flash.as_ref(), Instant::now())
    }

    pub fn path(&self) -> Option<&Path> {
        self.model.buf.path()
    }

    pub fn is_dirty(&self) -> bool {
        self.model.buf.dirty()
    }

    /// The file name (shown on the tab and in the header).
    pub fn title(&self) -> String {
        self.path()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "未命名".into())
    }

    /// The last painted text area is under `MIN_TEXT_COLS` columns: only the header and 「窗口太窄」 show.
    pub(crate) fn too_narrow(&self) -> bool {
        self.layout.is_some_and(|l| l.cols < chrome::MIN_TEXT_COLS)
    }

    /// After any command: tells the Workspace when the dirty flag flipped (tab title ●), then redraws.
    fn after_edit(&mut self, cx: &mut Context<Self>) {
        let dirty = self.model.buf.dirty();
        if dirty != self.last_dirty {
            self.last_dirty = dirty;
            cx.emit(EditorEvent::Changed);
        }
        let revision = self.model.buf.revision();
        if revision != self.last_revision {
            self.last_revision = revision;
            cx.emit(EditorEvent::TextChanged);
        }
        self.emit_scrolled(cx);
        cx.notify();
    }

    /// Tells the Workspace (and so a live preview) when the first shown row belongs to another logical line.
    fn emit_scrolled(&mut self, cx: &mut Context<Self>) {
        // Viewport-resize rewrap happens during paint and is intentionally not covered here.
        let top = self.model.top_line();
        if top != self.last_top_line {
            self.last_top_line = top;
            cx.emit(EditorEvent::Scrolled(top));
        }
    }

    /// The buffer is past the live preview's size limits (it then only follows saves).
    pub fn live_too_large(&self) -> bool {
        crate::live_preview::too_large(self.model.buf.len_bytes(), self.model.buf.line_count())
    }

    /// What a live preview of this buffer shows, unsaved edits included.
    pub fn live_input(&self, provider: crate::live_preview::Provider, slot: u64) -> crate::live_preview::LiveInput {
        crate::live_preview::LiveInput { provider, path: self.path().map(Path::to_path_buf), text: self.model.buf.text(), slot }
    }

    /// The header button: open the preview, switch provider, or close it (`live_preview::click_action`).
    pub(crate) fn click_preview(&mut self, cx: &mut Context<Self>) {
        use crate::live_preview::{click_action, providers_for, PreviewClick};
        match click_action(self.live.is_some(), providers_for(&self.title()).len()) {
            PreviewClick::Cycle => cx.emit(EditorEvent::CyclePreview),
            PreviewClick::Open | PreviewClick::Close => cx.emit(EditorEvent::TogglePreview),
        }
    }

    fn on_toggle_preview(&mut self, _: &crate::actions::EditorTogglePreview, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::TogglePreview);
    }

    /// Saves; on failure puts up the matching bar (E2a §6). A read-only buffer is left alone (⌘S does nothing).
    pub fn save(&mut self, cx: &mut Context<Self>) -> SaveOutcome {
        self.save_with(false, false, cx)
    }

    /// Runs a save (or overwrite), applies [`save_step`], and closes the pane when it says so.
    fn save_with(&mut self, overwrite: bool, closing: bool, cx: &mut Context<Self>) -> SaveOutcome {
        if self.model.buf.read_only() {
            return SaveOutcome::Failed;
        }
        let r = if overwrite { self.model.save_overwrite() } else { self.model.save() };
        let (bar, outcome, close) = save_step(closing, r.as_ref().map(|_| ()));
        self.bar = bar;
        if outcome == SaveOutcome::Saved {
            self.acked = next_acked(self.acked, AckEvent::Resynced);
            self.after_edit(cx);
            cx.emit(EditorEvent::Saved);
        } else {
            cx.notify();
        }
        if close {
            cx.emit(EditorEvent::CloseRequested);
        }
        outcome
    }

    /// ✕ / ⌘W: asks first when there are unsaved changes, else asks the Workspace to close the pane.
    pub fn request_close(&mut self, cx: &mut Context<Self>) {
        match close_bar(self.is_dirty()) {
            Some(bar) => self.set_bar(bar, cx),
            None => cx.emit(EditorEvent::CloseRequested),
        }
    }

    pub fn set_bar(&mut self, bar: Bar, cx: &mut Context<Self>) {
        self.bar = bar;
        cx.notify();
    }

    /// Puts the caret at the start of 1-based `line` (clamped to the document) and scrolls it into view.
    pub fn goto_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let buf = &self.model.buf;
        let line = line.clamp(1, buf.line_count().max(1)) - 1;
        let idx = buf.char_index(Position { line, col: 0 });
        self.model.set_caret(idx);
        self.emit_scrolled(cx);
        cx.notify();
    }

    pub(crate) fn bar_action(&mut self, action: BarAction, cx: &mut Context<Self>) {
        self.run_plan(plan(&self.bar, action), cx);
    }

    /// Carries out a [`Plan`]: from a bar button / key, or `Plan::Reopen` from [`Self::begin_reopen`].
    fn run_plan(&mut self, plan: Plan, cx: &mut Context<Self>) {
        match plan {
            Plan::Hide { close } => {
                self.acked = next_acked(self.acked, AckEvent::Dismissed(&self.bar));
                let recheck = external::recheck_after_hide(&self.bar);
                self.set_bar(Bar::None, cx);
                if close {
                    cx.emit(EditorEvent::CloseRequested);
                } else if recheck {
                    // The confirmation had suppressed every decision: look at the disk again now.
                    self.on_external_change(cx);
                }
            }
            // A failed save leaves its own bar up and the pane open (E2a §6).
            Plan::Save { overwrite, closing } => {
                self.save_with(overwrite, closing, cx);
            }
            Plan::Jump => {
                if let Bar::SaveError { jump: Some((line, col)), .. } = self.bar {
                    let idx = jump_index(&self.model.buf, line, col);
                    self.model.set_caret(idx);
                    self.emit_scrolled(cx);
                    cx.notify();
                }
            }
            Plan::Reload => match self.model.reload_as_edit() {
                Ok(()) => {
                    self.close_menu_on_model_change();
                    self.bar = Bar::None;
                    self.acked = next_acked(self.acked, AckEvent::Resynced);
                    self.flash(FLASH_UPDATED, cx);
                    self.after_edit(cx);
                }
                Err(e) => self.set_bar(bar_for_save_error(&e), cx),
            },
            Plan::Compare => self.open_compare(cx),
            Plan::CloseRequest => self.request_close(cx),
            Plan::Reopen(opts) => match self.model.reopen(opts) {
                Ok(()) => {
                    self.close_menu_on_model_change();
                    self.marked = None;
                    self.selecting = false;
                    self.bar = Bar::None;
                    self.acked = next_acked(self.acked, AckEvent::Resynced);
                    self.after_edit(cx);
                }
                Err(e) => self.set_bar(bar_for_save_error(&e), cx),
            },
        }
    }

    // ---------- status-bar menus ----------

    /// A menu cannot open over the compare overlay, nor while only 「窗口太窄」 shows.
    fn can_open_menu(&self, kind: &MenuKind) -> bool {
        popup::can_open_menu(kind, self.model.is_read_only(), self.too_narrow(), self.compare.is_some())
    }

    /// The model was swapped or reloaded under an open menu: its frozen items (✓, explicit, writable) may be
    /// stale, so it closes.
    fn close_menu_on_model_change(&mut self) {
        self.menu = None;
        self.menu_unplaced = false;
    }

    fn menu_items(&self, kind: &MenuKind) -> Vec<MenuItem> {
        let buf = &self.model.buf;
        match kind {
            MenuKind::Encoding => {
                let writable = self.path().is_some_and(popup::path_is_writable);
                popup::encoding_menu(buf.encoding(), buf.explicit_encoding().is_some(), buf.forced_read_only(), buf.lossy(), writable)
            }
            MenuKind::LineEnding => popup::line_ending_menu(buf.line_ending()),
        }
    }

    fn set_menu(&mut self, kind: MenuKind, at: Point<Pixels>, unplaced: bool, cx: &mut Context<Self>) {
        let items = self.menu_items(&kind);
        self.marked = None;
        self.selecting = false;
        self.menu_unplaced = unplaced;
        self.menu = Some(OpenMenu { kind, at, items });
        cx.notify();
    }

    /// A click on a status-bar segment at `click`: the menu opens above the status bar at the click's x.
    pub(crate) fn open_menu_at(&mut self, kind: MenuKind, click: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.can_open_menu(&kind) {
            return;
        }
        let status_top = self.status_encoding.map(|b| b.top()).or(self.layout.map(|l| l.bounds.bottom())).unwrap_or(click.y);
        self.set_menu(kind, popup::menu_anchor(click, status_top), false, cx);
    }

    /// Opens a menu without a click (the refused-file banner's 「选择编码打开…」): anchored at the encoding
    /// segment; before the pane was ever laid out, placed by the status bar's first layout.
    pub fn open_menu(&mut self, kind: MenuKind, cx: &mut Context<Self>) {
        if !self.can_open_menu(&kind) {
            return;
        }
        let at = self.status_encoding.map(|b| b.origin).or(self.layout.map(|l| l.bounds.bottom_right()));
        self.set_menu(kind, at.unwrap_or_default(), at.is_none(), cx);
    }

    /// Called by the status bar's encoding segment each time it is laid out: remembers where it is and places
    /// a menu still waiting for it (then draws again so the menu shows).
    pub(crate) fn status_encoding_laid_out(&mut self, bounds: Bounds<Pixels>) -> bool {
        self.status_encoding = Some(bounds);
        match self.menu.as_mut() {
            Some(m) if self.menu_unplaced => {
                m.at = bounds.origin;
                self.menu_unplaced = false;
                true
            }
            _ => false,
        }
    }

    /// The menu to draw: not yet while it waits for the status bar's layout.
    pub(crate) fn placed_menu(&self) -> Option<&OpenMenu> {
        self.menu.as_ref().filter(|_| !self.menu_unplaced)
    }

    pub(crate) fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        self.menu_unplaced = false;
        cx.notify();
    }

    /// A mouse-down outside the open menu closes it; a left click there must not also move the caret.
    pub(crate) fn menu_mouse_down_out(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_none() {
            return;
        }
        self.menu_closed_at = (event.button == MouseButton::Left).then_some(event.position);
        self.close_menu(cx);
    }

    /// A menu item was clicked: close the menu, then act.
    pub(crate) fn menu_action(&mut self, action: MenuAction, cx: &mut Context<Self>) {
        self.close_menu(cx);
        match action {
            MenuAction::Encoding(opts) => self.begin_reopen(opts, cx),
            // Refused quietly on a read-only model (it could never be saved); the status bar says why.
            MenuAction::LineEnding(le) => {
                let buf = &self.model.buf;
                let changes = popup::line_ending_choice_changes_anything(buf.line_ending(), buf.mixed_line_endings(), le);
                // Cannot fail when the menu could be opened (read-only never opens it); ignored if it does.
                if self.model.set_line_ending(le).is_ok() {
                    if changes {
                        self.flash(&popup::line_ending_flash(le), cx);
                    }
                    self.after_edit(cx);
                }
            }
        }
    }

    /// Reopen with `opts`: asks first when unsaved edits would be lost, else reopens now (the confirm bar's
    /// own `Plan::Reopen` path).
    pub(crate) fn begin_reopen(&mut self, opts: OpenOptions, cx: &mut Context<Self>) {
        match reopen_plan(self.is_dirty(), opts) {
            ReopenStep::Confirm(intent) => self.set_bar(Bar::Confirm(intent), cx),
            ReopenStep::Now(opts) => self.run_plan(Plan::Reopen(opts), cx),
        }
    }

    /// 对比: reads the disk afresh and opens the overlay over the pane. A read error (other than the file
    /// being gone, which the overlay shows) puts up the error bar instead.
    fn open_compare(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path().map(Path::to_path_buf) else { return };
        let width = self.layout.map_or(compare::SPLIT_MIN_COLS, |l| pane_cols(&l));
        let buf = &self.model.buf;
        match Compare::open(&path, buf.explicit_encoding(), &buf.text(), buf.line_ending(), buf.encoding().name(), width) {
            Ok(c) => {
                self.marked = None;
                self.selecting = false;
                self.scroll_accum = 0.0;
                self.compare = Some(c);
                cx.notify();
            }
            // The Modified banner comes back on the next watcher event or focus check.
            Err(e) => self.set_bar(Bar::SaveError { message: compare::open_error_message(&e), jump: None }, cx),
        }
    }

    /// A compare-overlay button (or Esc): close the overlay, then run the banner's own action, if any.
    pub(crate) fn compare_action(&mut self, action: CompareAction, cx: &mut Context<Self>) {
        self.compare = None;
        match compare::apply_action(action) {
            Some(a) => self.bar_action(a, cx),
            None => cx.notify(),
        }
    }

    /// Clicking a fold row of the overlay opens it.
    pub(crate) fn compare_toggle_fold(&mut self, start: usize, cx: &mut Context<Self>) {
        if let Some(c) = self.compare.as_mut() {
            c.toggle_fold(start);
            cx.notify();
        }
    }

    /// Re-lays out the overlay when the pane crossed `SPLIT_MIN_COLS` since it opened (called from `render`).
    fn compare_fit_width(&mut self) {
        if let (Some(c), Some(l)) = (self.compare.as_mut(), self.layout) {
            c.set_width(pane_cols(&l));
        }
    }

    /// Overlay rows per page: the text area's height in rows, less the overlay's own chrome.
    fn compare_page(&self) -> i64 {
        self.layout.map_or(10, |l| l.rows.saturating_sub(6).max(1)) as i64
    }

    /// The overlay's keys: Esc closes, ↑/↓/PageUp/PageDown scroll, everything else is swallowed.
    fn compare_key_down(&mut self, cmd: Option<KeyCmd>, cx: &mut Context<Self>) {
        let page = self.compare_page();
        let Some(c) = self.compare.as_mut() else { return };
        match compare::compare_key(cmd) {
            CompareKey::Action(a) => return self.compare_action(a, cx),
            CompareKey::Scroll(d) => c.scroll_by(d),
            CompareKey::Page(d) => c.scroll_by(d * page),
            CompareKey::Swallow => return,
        }
        cx.notify();
    }

    /// The mouse wheel over the overlay scrolls its rows (never the text under it).
    pub(crate) fn compare_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let line_h = compare_line_h(&cx.global::<AppSettings>().0);
        let lines = wheel_lines(&mut self.scroll_accum, event.delta, line_h);
        if let (Some(c), true) = (self.compare.as_mut(), lines != 0) {
            c.scroll_by(-lines);
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let m = ks.modifiers;
        // The compare overlay comes first: before the IME guard, the bar's Esc and the text. Editor keys are
        // consumed here; typed characters reach the input handler, which is inert while the overlay is open.
        if self.compare.is_some() {
            let cmd = key_command(&ks.key, m.control, m.shift, m.alt, m.platform);
            self.compare_key_down(cmd, cx);
            if cmd.is_some() {
                cx.stop_propagation();
            }
            return;
        }
        // Then an open menu: Esc closes it, every other key is eaten here (nothing reaches the text or a bar;
        // the input handler is inert too, see `covered`).
        if self.menu.is_some() {
            if popup::popup_key(key_command(&ks.key, m.control, m.shift, m.alt, m.platform)) == PopupKey::Close {
                self.close_menu(cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.marked.is_some() {
            return; // IME composition owns the keyboard.
        }
        let Some(cmd) = key_command(&ks.key, m.control, m.shift, m.alt, m.platform) else { return };
        if self.too_narrow() && cmd != KeyCmd::Escape {
            return;
        }
        if let Some(action) = bar_key(&self.bar, cmd) {
            self.bar_action(action, cx);
        } else {
            self.run_key(cmd, cx);
        }
        cx.stop_propagation();
    }

    fn run_key(&mut self, cmd: KeyCmd, cx: &mut Context<Self>) {
        let m = &mut self.model;
        // Edits on a read-only model return `Err(ReadOnly)`; the status bar already says why.
        match cmd {
            KeyCmd::Move(kind, extend) => m.move_h(kind, extend),
            KeyCmd::Rows(d, extend) => m.move_rows(d, extend),
            KeyCmd::Page(d, extend) => {
                let page = m.view_rows().saturating_sub(1).max(1) as i64;
                m.move_rows(d * page, extend);
            }
            KeyCmd::Home(extend) => m.home(extend),
            KeyCmd::End(extend) => m.end(extend),
            KeyCmd::Enter => {
                let _ = m.enter();
            }
            KeyCmd::Tab(shift) => {
                let _ = m.tab(shift);
            }
            KeyCmd::Backspace => {
                let _ = m.backspace();
            }
            KeyCmd::DeleteWordBack => {
                let _ = m.delete_word_back();
            }
            KeyCmd::Delete => {
                let _ = m.delete_forward();
            }
            KeyCmd::Escape => {
                cx.emit(EditorEvent::Leave);
                return;
            }
        }
        self.after_edit(cx);
    }

    // ---------- actions ----------

    /// The compare overlay or a status-bar menu is open: the text takes no edits, no save and no IME.
    fn covered(&self) -> bool {
        covered_by(self.compare.as_ref(), self.menu.is_some())
    }

    fn on_save(&mut self, _: &EditorSave, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_none() && !self.covered() {
            self.save(cx);
        }
    }

    /// ⌘⌥O: the file in the external editor at the caret's line (what is on disk; unsaved edits stay here).
    fn on_open_external(&mut self, _: &OpenExternalEditor, _: &mut Window, _: &mut Context<Self>) {
        if let Some(path) = self.path() {
            let line = self.model.caret_position().line as u32 + 1;
            crate::preview_view::open_in_editor(path, Some(line));
        }
    }

    fn on_undo(&mut self, _: &EditorUndo, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_none() && !self.covered() {
            self.model.undo();
            self.after_edit(cx);
        }
    }

    fn on_redo(&mut self, _: &EditorRedo, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_none() && !self.covered() {
            self.model.redo();
            self.after_edit(cx);
        }
    }

    fn on_cut(&mut self, _: &EditorCut, _: &mut Window, cx: &mut Context<Self>) {
        if self.covered() {
            return;
        }
        if let Ok(Some(text)) = self.model.cut() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.after_edit(cx);
        }
    }

    fn on_copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.model.copy_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn on_paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.covered() {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return };
        // One replace, one undo step (E2a §6); a read-only model refuses it quietly.
        let _ = self.model.insert(&text);
        self.after_edit(cx);
    }

    fn on_select_all(&mut self, _: &EditorSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if self.covered() {
            return;
        }
        self.model.select_all();
        cx.notify();
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        // The click that just closed the menu (its mouse-down-out ran first, in the capture phase) only focuses.
        if self.menu_closed_at.take() == Some(event.position) {
            return;
        }
        let Some(l) = self.layout else { return };
        // Clicks on the header / bar / status bar (or the 「窗口太窄」 cover, or the compare overlay) only focus.
        if self.too_narrow() || self.covered() || !l.bounds.contains(&event.position) {
            return;
        }
        let hit = pixel_to_cell(event.position, &l);
        let row = hit.row.clamp(0, l.rows.saturating_sub(1) as i64) as usize;
        if hit.gutter {
            self.model.triple_click(row);
        } else if event.modifiers.shift {
            self.model.click(row, hit.x, true);
        } else {
            match event.click_count {
                2 => self.model.double_click(row, hit.x),
                n if n >= 3 => self.model.triple_click(row),
                _ => self.model.click(row, hit.x, false),
            }
        }
        self.selecting = true;
        self.emit_scrolled(cx);
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || self.covered() || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(l) = self.layout else { return };
        let hit = pixel_to_cell(event.position, &l);
        // Above the text area: scroll up a row and extend to the new top row. Below it, the model's
        // `drag_to` → `ensure_cursor_visible` scrolls down by itself.
        if hit.row < 0 {
            self.model.scroll_by(-1);
            self.model.drag_to(0, hit.x);
        } else {
            self.model.drag_to(hit.row as usize, hit.x);
        }
        self.emit_scrolled(cx);
        cx.notify();
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        cx.notify();
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(line_h) = self.layout.map(|l| l.line_h) else { return };
        if self.covered() {
            return;
        }
        let lines = wheel_lines(&mut self.scroll_accum, event.delta, line_h);
        if lines == 0 {
            return;
        }
        self.model.scroll_by(-lines);
        self.emit_scrolled(cx);
        cx.notify();
    }

    /// Screen bounds of the caret cell, for the IME candidate window.
    fn caret_bounds(&self) -> Option<Bounds<Pixels>> {
        let l = self.layout?;
        let row = self.model.caret_row().checked_sub(self.model.scroll_row())?;
        let origin = Point {
            x: l.text_origin.x + l.cell_w * self.model.caret_visual_col() as f32,
            y: l.bounds.origin.y + l.line_h * row as f32,
        };
        Some(Bounds::new(origin, gpui::size(l.cell_w, l.line_h)))
    }
}

/// Text input and IME. Offsets are UTF-16 code units over the whole buffer. The preedit lives in `marked`,
/// never in the buffer; its range is reported at the selection start so an IME that echoes it back as the
/// replacement range cannot hit other text (a commit while composing ignores the range).
impl gpui::EntityInputHandler for EditorView {
    fn text_for_range(&mut self, range: Range<usize>, adjusted: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let buf = &self.model.buf;
        let r = utf16_to_chars(buf, &range);
        *adjusted = Some(chars_to_utf16(buf, r.clone()));
        Some(buf.slice(r))
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let buf = &self.model.buf;
        let sel = buf.selection();
        Some(UTF16Selection { range: chars_to_utf16(buf, sel.range()), reversed: sel.head < sel.anchor })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let buf = &self.model.buf;
        let at = buf.char_to_utf16(buf.selection().start());
        self.marked.as_ref().map(|t| at..at + t.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        if self.covered() {
            return;
        }
        let composing = self.marked.take().is_some();
        let text = typed_text(text);
        let range = range.filter(|_| !composing);
        // An empty commit only deletes when the IME names what to delete.
        if self.too_narrow() || (text.is_empty() && range.is_none()) {
            cx.notify();
            return;
        }
        if let Some(r) = range {
            let r = utf16_to_chars(&self.model.buf, &r);
            self.model.buf.set_selection(Selection { anchor: r.start, head: r.end });
        }
        let _ = self.model.insert(&text);
        self.after_edit(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.covered() {
            return;
        }
        self.marked = preedit(text);
        cx.notify();
    }

    fn bounds_for_range(&mut self, _: Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        self.caret_bounds()
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = crate::theme::current(cx).palette.clone();
        let narrow = self.too_narrow();
        if narrow {
            // The status bar it belongs to is gone.
            self.menu = None;
            self.menu_unplaced = false;
        }
        let header = chrome::header(self, &p, cx);
        let bar = (!narrow).then(|| chrome::bar(self, &p, cx)).flatten();
        let status = (!narrow).then(|| chrome::status(self, &p, cx));
        let cover = narrow.then(|| chrome::too_narrow(&p));
        self.compare_fit_width();
        let overlay = chrome::compare_overlay(self, &p, cx);
        let theme = crate::theme::current(cx);
        let menu = chrome::popup(self, &theme.ui, cx);
        div()
            .id("editor")
            .key_context(EDITOR_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(hsla(p.background))
            .text_color(hsla(p.foreground))
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_toggle_preview))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_cut))
            .on_action(cx.listener(Self::on_copy))
            .on_action(cx.listener(Self::on_paste))
            .on_action(cx.listener(Self::on_select_all))
            .on_action(cx.listener(Self::on_open_external))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .child(header)
            .children(bar)
            // Kept even when too narrow: it keeps measuring the width, so the pane comes back when widened.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    .relative()
                    .child(EditorElement::new(cx.entity()))
                    .children(self.pane.and_then(|id| crate::debug_state::rects::recorder(crate::debug_state::rects::RectId::EditorBody(id)))),
            )
            .children(status)
            .children(cover)
            .children(overlay)
            .children(menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    // Element at (100, 50); gutter 5 cells of 10px + 8px padding → text column 0 at x = 158.
    fn layout() -> LayoutInfo {
        LayoutInfo {
            bounds: Bounds::new(point(px(100.), px(50.)), size(px(600.), px(400.))),
            text_origin: point(px(158.), px(50.)),
            cell_w: px(10.),
            line_h: px(20.),
            cols: 44,
            rows: 20,
        }
    }

    #[test]
    fn pixels_map_to_view_rows_and_fractional_cells() {
        let h = pixel_to_cell(point(px(183.), px(95.)), &layout());
        assert_eq!(h, Hit { row: 2, x: 2.5, gutter: false });
    }

    #[test]
    fn the_text_padding_is_column_zero_and_left_of_it_is_the_gutter() {
        let pad = pixel_to_cell(point(px(152.), px(50.)), &layout());
        assert_eq!((pad.x, pad.gutter), (0.0, false));
        assert!(pixel_to_cell(point(px(149.), px(50.)), &layout()).gutter);
        assert!(pixel_to_cell(point(px(101.), px(50.)), &layout()).gutter);
    }

    #[test]
    fn rows_outside_the_area_clamp_to_one_past_each_edge() {
        assert_eq!(pixel_to_cell(point(px(200.), px(49.)), &layout()).row, -1);
        assert_eq!(pixel_to_cell(point(px(200.), px(-500.)), &layout()).row, -1);
        assert_eq!(pixel_to_cell(point(px(200.), px(449.)), &layout()).row, 19);
        assert_eq!(pixel_to_cell(point(px(200.), px(450.)), &layout()).row, 20);
        assert_eq!(pixel_to_cell(point(px(200.), px(5000.)), &layout()).row, 20);
    }

    #[test]
    fn key_table() {
        use HMove::*;
        use KeyCmd::*;
        let k = |key: &str, shift, alt, cmd| key_command(key, false, shift, alt, cmd);
        assert_eq!(k("left", false, false, false), Some(Move(Left, false)));
        assert_eq!(k("right", true, false, false), Some(Move(Right, true)));
        assert_eq!(k("left", false, true, false), Some(Move(WordLeft, false)));
        assert_eq!(k("right", true, true, false), Some(Move(WordRight, true)));
        assert_eq!(k("left", false, false, true), Some(Move(LineStart, false)));
        assert_eq!(k("right", true, false, true), Some(Move(LineEnd, true)));
        assert_eq!(k("up", false, false, false), Some(Rows(-1, false)));
        assert_eq!(k("down", true, false, false), Some(Rows(1, true)));
        assert_eq!(k("up", false, false, true), Some(Move(DocStart, false)));
        assert_eq!(k("down", false, false, true), Some(Move(DocEnd, false)));
        // ⌘⇧↑ / ⌘⇧↓ stay global session navigation (spec §5.5): a known gap, not an editor key.
        assert_eq!(k("up", true, false, true), None);
        assert_eq!(k("down", true, false, true), None);
        assert_eq!(k("home", true, false, false), Some(Home(true)));
        assert_eq!(k("end", false, false, false), Some(End(false)));
        assert_eq!(k("pageup", false, false, false), Some(Page(-1, false)));
        assert_eq!(k("pagedown", true, false, false), Some(Page(1, true)));
        assert_eq!(k("enter", false, false, false), Some(Enter));
        assert_eq!(k("enter", true, false, false), Some(Enter));
        assert_eq!(k("tab", false, false, false), Some(Tab(false)));
        assert_eq!(k("tab", true, false, false), Some(Tab(true)));
        assert_eq!(k("backspace", false, false, false), Some(Backspace));
        assert_eq!(k("backspace", false, true, false), Some(DeleteWordBack));
        assert_eq!(k("delete", false, false, false), Some(Delete));
        assert_eq!(k("escape", false, false, false), Some(Escape));
    }

    #[test]
    fn text_and_shortcut_keys_are_not_the_editors() {
        // Characters go through the input handler; ⌘ / ⌃ combinations belong to key bindings.
        for key in ["a", "space", "1", "z"] {
            assert_eq!(key_command(key, false, false, false, false), None, "{key}");
        }
        assert_eq!(key_command("left", false, false, true, true), None); // ⌥⌘← is FocusLeft
        assert_eq!(key_command("enter", false, true, false, true), None); // ⌘⇧↩ is Zoom Pane
        assert_eq!(key_command("w", false, false, false, true), None);
        assert_eq!(key_command("left", true, false, false, false), None);
        assert_eq!(key_command("escape", false, false, false, true), None);
    }

    #[test]
    fn bar_keys() {
        let err = Bar::SaveError { message: "x".into(), jump: None };
        assert_eq!(bar_key(&Bar::Close, KeyCmd::Enter), Some(BarAction::SaveAndClose));
        assert_eq!(bar_key(&Bar::Close, KeyCmd::Escape), Some(BarAction::Cancel));
        assert_eq!(bar_key(&Bar::Modified { closing: false }, KeyCmd::Escape), Some(BarAction::Cancel));
        assert_eq!(bar_key(&err, KeyCmd::Escape), Some(BarAction::Cancel));
        // Enter only answers the close bar; other keys keep editing.
        assert_eq!(bar_key(&Bar::Modified { closing: false }, KeyCmd::Enter), None);
        assert_eq!(bar_key(&Bar::Close, KeyCmd::Backspace), None);
        // No bar: Esc leaves the editor (handled by the caller).
        assert_eq!(bar_key(&Bar::None, KeyCmd::Escape), None);
        assert_eq!(bar_key(&Bar::None, KeyCmd::Enter), None);
    }

    #[test]
    fn save_errors_pick_their_bar() {
        assert_eq!(bar_for_save_error(&EditorError::ModifiedOnDisk), Bar::Modified { closing: false });
        assert_eq!(
            bar_for_save_error(&EditorError::Unrepresentable { line: 2, col: 5 }),
            Bar::SaveError { message: "第 3 行第 6 列的字符无法用文件原来的编码保存".into(), jump: Some((2, 5)) }
        );
        assert_eq!(
            bar_for_save_error(&EditorError::PermissionDenied),
            Bar::SaveError { message: "没有权限写入这个文件".into(), jump: None }
        );
        let io = EditorError::Io(std::io::Error::other("disk full"));
        assert!(matches!(bar_for_save_error(&io), Bar::SaveError { jump: None, .. }));
    }

    /// Drives `bar_action`'s pure half: returns (new bar, closed).
    fn press(bar: &Bar, action: BarAction, r: Result<(), EditorError>) -> (Bar, bool) {
        match plan(bar, action) {
            Plan::Hide { close } => (Bar::None, close),
            Plan::Save { closing, .. } => {
                let (b, _, close) = save_step(closing, r.as_ref().map(|_| ()));
                (b, close)
            }
            Plan::Jump | Plan::Reload | Plan::Compare | Plan::Reopen(_) => (bar.clone(), false),
            Plan::CloseRequest => (bar.clone(), false),
        }
    }

    #[test]
    fn the_modified_banner_has_reload_compare_overwrite() {
        let b = chrome::bar_buttons(&Bar::Modified { closing: false });
        assert_eq!(b.iter().map(|(l, _, _)| *l).collect::<Vec<_>>(), ["重新载入", "对比", "仍然覆盖"]);
        assert_eq!(b.iter().filter(|(_, _, p)| *p).count(), 1, "only 对比 is primary");
        assert!(b[1].2);
    }
    #[test]
    fn modified_actions_plan() {
        let m = Bar::Modified { closing: true };
        assert_eq!(plan(&m, BarAction::Reload), Plan::Reload);
        assert_eq!(plan(&m, BarAction::Compare), Plan::Compare);
        assert_eq!(plan(&m, BarAction::Overwrite), Plan::Save { overwrite: true, closing: true });
        assert_eq!(plan(&m, BarAction::Cancel), Plan::Hide { close: false });
    }
    #[test]
    fn deleted_banner() {
        let b = chrome::bar_buttons(&Bar::Deleted);
        assert_eq!(b.iter().map(|(l, _, _)| *l).collect::<Vec<_>>(), ["保存（重新创建）", "关闭", "知道了"]);
        assert_eq!(b.iter().map(|(_, _, p)| *p).collect::<Vec<_>>(), [true, false, false]);
        assert_eq!(plan(&Bar::Deleted, BarAction::Recreate), Plan::Save { overwrite: false, closing: false });
        assert_eq!(plan(&Bar::Deleted, BarAction::CloseNow), Plan::CloseRequest);
    }
    #[test]
    fn reopen_confirmation_never_defaults_to_discarding() {
        let i = ReopenIntent(gilvt_editor::OpenOptions { encoding: None, read_only: true });
        let b = chrome::bar_buttons(&Bar::Confirm(i));
        assert_eq!(b.iter().map(|(l, _, p)| (*l, *p)).collect::<Vec<_>>(), [("取消", false), ("放弃改动并重新打开", true)]);
        assert_eq!(bar_key(&Bar::Confirm(i), KeyCmd::Enter), None);
        assert_eq!(bar_key(&Bar::Confirm(i), KeyCmd::Escape), Some(BarAction::Cancel));
        assert_eq!(plan(&Bar::Confirm(i), BarAction::ConfirmReopen), Plan::Reopen(i.0));
    }
    #[test]
    fn reopening_a_dirty_buffer_asks_first_and_a_clean_one_reopens_now() {
        let opts = gilvt_editor::OpenOptions { encoding: gilvt_editor::encoding_by_label("gbk"), read_only: false };
        assert_eq!(reopen_plan(true, opts), ReopenStep::Confirm(ReopenIntent(opts)));
        assert_eq!(reopen_plan(false, opts), ReopenStep::Now(opts));
        // The confirm bar's 放弃改动并重新打开 then reopens with the same options.
        assert_eq!(plan(&Bar::Confirm(ReopenIntent(opts)), BarAction::ConfirmReopen), Plan::Reopen(opts));
    }
    #[test]
    fn escape_closes_every_new_bar_without_discarding_and_enter_does_nothing() {
        for bar in [Bar::Modified { closing: false }, Bar::Modified { closing: true }, Bar::Deleted] {
            assert_eq!(bar_key(&bar, KeyCmd::Escape), Some(BarAction::Cancel));
            assert_eq!(bar_key(&bar, KeyCmd::Enter), None);
        }
    }
    #[test]
    fn reload_and_compare_plan_from_any_bar() {
        assert_eq!(plan(&Bar::None, BarAction::Reload), Plan::Reload);
        assert_eq!(plan(&Bar::Deleted, BarAction::Compare), Plan::Compare);
    }

    #[test]
    fn close_flow_state_machine() {
        let md = || Err(EditorError::ModifiedOnDisk);
        // close -> 保存 -> Modified -> 仍然覆盖 closes.
        let (b, closed) = press(&Bar::Close, BarAction::SaveAndClose, md());
        assert_eq!((&b, closed), (&Bar::Modified { closing: true }, false));
        assert_eq!(press(&b, BarAction::Overwrite, Ok(())), (Bar::None, true));
        // close -> 保存 -> Modified -> ⌘S succeeds (bar replaced) -> later plain conflict -> 仍然覆盖 does NOT close.
        let (b, _) = press(&Bar::Close, BarAction::SaveAndClose, md());
        assert_eq!(save_step(false, Ok(())), (Bar::None, SaveOutcome::Saved, false));
        let _ = b;
        let (b, closed) = (save_step(false, Err(&EditorError::ModifiedOnDisk)).0, false);
        assert_eq!((&b, closed), (&Bar::Modified { closing: false }, false));
        assert_eq!(press(&b, BarAction::Overwrite, Ok(())), (Bar::None, false));
        // A plain ⌘S conflict over a closing bar also drops the flow.
        assert_eq!(save_step(false, Err(&EditorError::ModifiedOnDisk)).0, Bar::Modified { closing: false });
        // Cancel clears; Discard closes.
        assert_eq!(press(&Bar::Modified { closing: true }, BarAction::Cancel, Ok(())), (Bar::None, false));
        assert_eq!(press(&Bar::Close, BarAction::Discard, Ok(())), (Bar::None, true));
        // Saved from the close bar closes; Failed drops the flow and keeps the pane.
        assert_eq!(press(&Bar::Close, BarAction::SaveAndClose, Ok(())), (Bar::None, true));
        let (b, closed) = press(&Bar::Close, BarAction::SaveAndClose, Err(EditorError::ReadOnly));
        assert!(!closed && matches!(b, Bar::SaveError { .. }));
        // Overwrite failing keeps the pane and drops the flow.
        let (b, closed) = press(&Bar::Modified { closing: true }, BarAction::Overwrite, Err(EditorError::ReadOnly));
        assert!(!closed && matches!(b, Bar::SaveError { .. }));
        // 知道了 on a save-error bar (with or without a jump) dismisses it.
        let err = |jump| Bar::SaveError { message: "x".into(), jump };
        assert_eq!(press(&err(None), BarAction::Cancel, Ok(())), (Bar::None, false));
        assert_eq!(press(&err(Some((1, 2))), BarAction::Cancel, Ok(())), (Bar::None, false));
        // Esc in Modified is Cancel.
        assert_eq!(bar_key(&Bar::Modified { closing: true }, KeyCmd::Escape), Some(BarAction::Cancel));
    }

    #[test]
    fn closing_asks_only_when_dirty() {
        assert_eq!(close_bar(true), Some(Bar::Close));
        assert_eq!(close_bar(false), None);
    }

    #[test]
    fn utf16_ranges_convert_over_cjk_and_surrogate_pairs() {
        // "a你😀b": chars a=0 你=1 😀=2 b=3; UTF-16 a=0 你=1 😀=2..4 b=4.
        let buf = Buffer::from_text("a你😀b");
        assert_eq!(utf16_to_chars(&buf, &(1..4)), 1..3);
        assert_eq!(utf16_to_chars(&buf, &(4..5)), 3..4);
        assert_eq!(utf16_to_chars(&buf, &Range { start: 5, end: 1 }), 1..4); // reversed → ordered
        assert_eq!(utf16_to_chars(&buf, &(0..99)), 0..4); // clamped
        assert_eq!(chars_to_utf16(&buf, 2..4), 2..5);
        assert_eq!(buf.slice(utf16_to_chars(&buf, &(2..4))), "😀");
    }

    #[test]
    fn preedit_never_holds_a_line_break() {
        assert_eq!(preedit("ni hao"), Some("ni hao".into()));
        assert_eq!(preedit("a\nb\r"), Some("a b ".into()));
        assert_eq!(preedit(""), None);
    }

    #[test]
    fn typed_text_drops_control_characters() {
        assert_eq!(typed_text("你好"), "你好");
        assert_eq!(typed_text("a\u{1b}b\u{7f}"), "ab");
        assert_eq!(typed_text("a\tb\nc"), "a\tb\nc");
    }

    #[test]
    fn jump_index_counts_chars_from_the_line_start_and_clamps() {
        let buf = Buffer::from_text("ab\ncde😀f\n");
        assert_eq!(jump_index(&buf, 1, 3), 6);
        assert_eq!(buf.slice(6..7), "😀");
        assert_eq!(jump_index(&buf, 1, 99), 3 + 5);
        assert_eq!(jump_index(&buf, 99, 0), buf.len_chars());
    }

    // ---------- Task 5: watcher, focus check, flash ----------

    #[test]
    fn watch_target_resolves_a_symlinked_directory() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("a.md"), "x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let (d, f) = watch_target(&link.join("a.md")).unwrap();
        assert_eq!(d, real.canonicalize().unwrap());
        assert_eq!(f, real.canonicalize().unwrap().join("a.md"));
    }

    #[test]
    fn our_own_save_is_unchanged_so_it_never_raises_a_banner() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.md");
        std::fs::write(&p, "a\n").unwrap();
        let mut m = EditorModel::new(Buffer::open(&p).unwrap(), "a.md", 40, 5);
        m.insert("b").unwrap();
        m.save().unwrap();
        assert_eq!(m.buf.check_external(), ExternalState::Unchanged);
        assert_eq!(external::decide(m.buf.check_external(), m.buf.dirty(), &Bar::None, None), ExternalAction::Nothing);
    }

    #[test]
    fn an_agent_write_after_my_edit_is_a_conflict_banner() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.md");
        std::fs::write(&p, "a\n").unwrap();
        let mut m = EditorModel::new(Buffer::open(&p).unwrap(), "a.md", 40, 5);
        m.insert("mine").unwrap();
        std::fs::write(&p, "agent\n").unwrap();
        assert_eq!(external::decide(m.buf.check_external(), m.buf.dirty(), &Bar::None, None), ExternalAction::ShowModified);
    }

    #[test]
    fn a_clean_buffer_reload_keeps_a_clean_history_and_the_caret() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.md");
        std::fs::write(&p, "one\ntwo\n").unwrap();
        let mut m = EditorModel::new(Buffer::open(&p).unwrap(), "a.md", 40, 5);
        m.set_caret(5);
        std::fs::write(&p, "one\ntwo\nthree\n").unwrap();
        assert_eq!(external::decide(m.buf.check_external(), m.buf.dirty(), &Bar::None, None), ExternalAction::SilentReload);
        m.reload().unwrap();
        assert_eq!(m.buf.line(2), "three");
        assert!(!m.buf.dirty());
        assert_eq!(m.buf.selection().head, 5);
        // E1 `reload` clears the history: nothing to undo back to.
        m.undo();
        assert_eq!(m.buf.line(2), "three");
        assert_eq!(m.buf.check_external(), ExternalState::Unchanged);
    }

    #[test]
    fn drain_burst_empties_the_queue_in_one_call() {
        let (tx, rx) = async_channel::unbounded::<()>();
        for _ in 0..5 {
            tx.try_send(()).unwrap();
        }
        assert_eq!(drain_burst(&rx), 5);
        assert!(rx.is_empty());
        assert_eq!(drain_burst(&rx), 0);
    }

    #[test]
    fn a_flash_lives_for_flash_ms() {
        let t = Instant::now();
        let f = (external::FLASH_UPDATED.to_string(), t);
        assert_eq!(live_flash(Some(&f), t), Some("已更新"));
        assert_eq!(live_flash(Some(&f), t + Duration::from_millis(external::FLASH_MS - 1)), Some("已更新"));
        assert_eq!(live_flash(Some(&f), t + Duration::from_millis(external::FLASH_MS)), None);
        assert_eq!(live_flash(None, t), None);
    }

    #[test]
    fn dismissing_a_modified_or_deleted_bar_remembers_its_state() {
        use AckEvent::Dismissed;
        use ExternalState::*;
        assert_eq!(next_acked(None, Dismissed(&Bar::Modified { closing: false })), Some(Modified));
        assert_eq!(next_acked(None, Dismissed(&Bar::Modified { closing: true })), Some(Modified));
        assert_eq!(next_acked(None, Dismissed(&Bar::Deleted)), Some(Deleted));
        assert_eq!(next_acked(Some(Modified), Dismissed(&Bar::Deleted)), Some(Deleted));
        // Other bars carry no external state: the memory is left as it was.
        for bar in [Bar::None, Bar::Close, Bar::SaveError { message: "x".into(), jump: None }, Bar::Confirm(ReopenIntent(Default::default()))] {
            assert_eq!(next_acked(None, Dismissed(&bar)), None, "{bar:?}");
            assert_eq!(next_acked(Some(Modified), Dismissed(&bar)), Some(Modified), "{bar:?}");
        }
    }

    #[test]
    fn a_check_keeps_the_memory_only_while_the_state_is_the_same() {
        use AckEvent::Checked;
        use ExternalState::*;
        assert_eq!(next_acked(Some(Modified), Checked(Modified)), Some(Modified));
        assert_eq!(next_acked(Some(Deleted), Checked(Deleted)), Some(Deleted));
        assert_eq!(next_acked(Some(Modified), Checked(Unchanged)), None);
        assert_eq!(next_acked(Some(Deleted), Checked(Unchanged)), None);
        assert_eq!(next_acked(Some(Modified), Checked(Deleted)), None);
        assert_eq!(next_acked(Some(Deleted), Checked(Modified)), None);
        // A check never creates a memory.
        for s in [Unchanged, Modified, Deleted] {
            assert_eq!(next_acked(None, Checked(s)), None);
        }
    }

    #[test]
    fn a_save_or_reload_forgets_the_memory() {
        use ExternalState::*;
        for a in [None, Some(Modified), Some(Deleted)] {
            assert_eq!(next_acked(a, AckEvent::Resynced), None);
        }
    }

    /// The sequence the controller ruled on: Esc on the banner, the next watcher event stays quiet, a save is
    /// still refused, and only a different disk state (or a resync) brings banners back.
    #[test]
    fn esc_on_the_banner_is_not_undone_by_the_next_event() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.md");
        std::fs::write(&p, "a\n").unwrap();
        let mut m = EditorModel::new(Buffer::open(&p).unwrap(), "a.md", 40, 5);
        m.insert("mine").unwrap();
        std::fs::write(&p, "agent\n").unwrap();
        let mut acked = None;
        let check = |m: &mut EditorModel, acked: &mut Option<ExternalState>, bar: &Bar| {
            let s = m.buf.check_external();
            *acked = next_acked(*acked, AckEvent::Checked(s));
            external::decide(s, m.buf.dirty(), bar, *acked)
        };
        assert_eq!(check(&mut m, &mut acked, &Bar::None), ExternalAction::ShowModified);
        // Esc.
        acked = next_acked(acked, AckEvent::Dismissed(&Bar::Modified { closing: false }));
        assert_eq!(check(&mut m, &mut acked, &Bar::None), ExternalAction::Nothing);
        assert!(matches!(m.save(), Err(EditorError::ModifiedOnDisk)));
        // The file disappears: a different state, so the deleted bar shows.
        std::fs::remove_file(&p).unwrap();
        assert_eq!(check(&mut m, &mut acked, &Bar::None), ExternalAction::ShowDeleted);
        assert_eq!(acked, None);
    }

    /// Esc on the banner, then undo back to the saved point: the clean buffer must still take the disk version.
    #[test]
    fn undoing_to_clean_after_esc_still_reloads_silently() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.md");
        std::fs::write(&p, "a\n").unwrap();
        let mut m = EditorModel::new(Buffer::open(&p).unwrap(), "a.md", 40, 5);
        m.insert("mine").unwrap();
        std::fs::write(&p, "agent\n").unwrap();
        let s = m.buf.check_external();
        assert_eq!(external::decide(s, m.buf.dirty(), &Bar::None, None), ExternalAction::ShowModified);
        let acked = next_acked(None, AckEvent::Dismissed(&Bar::Modified { closing: false }));
        assert_eq!(acked, Some(ExternalState::Modified));
        assert!(m.undo());
        assert!(!m.buf.dirty());
        let s = m.buf.check_external();
        let acked = next_acked(acked, AckEvent::Checked(s));
        assert_eq!(external::decide(s, m.buf.dirty(), &Bar::None, acked), ExternalAction::SilentReload);
    }

    #[test]
    fn no_silent_reload_during_an_ime_composition() {
        assert!(may_reload_now(false));
        assert!(!may_reload_now(true));
    }

    #[test]
    fn watcher_events_match_by_name_then_canonical_path_and_skip_access() {
        use notify::event::{AccessKind, CreateKind, ModifyKind};
        use notify::{Event, EventKind};
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let (_, target) = watch_target(&link.join("a.md")).unwrap();
        let ev = |kind, p: PathBuf| Event::new(kind).add_path(p);
        let modify = EventKind::Modify(ModifyKind::Any);
        assert!(event_hits(&ev(modify, target.clone()), &target));
        assert!(event_hits(&ev(modify, link.join("a.md")), &target)); // same file through the symlink
        assert!(event_hits(&ev(EventKind::Create(CreateKind::File), target.clone()), &target));
        assert!(!event_hits(&ev(modify, real.join("b.md")), &target)); // other file name
        assert!(!event_hits(&ev(modify, dir.path().join("a.md")), &target)); // same name, other dir
        assert!(!event_hits(&ev(EventKind::Access(AccessKind::Any), target.clone()), &target));
    }

    #[test]
    fn a_reload_clears_only_the_external_bars() {
        assert_eq!(bar_after_reload(&Bar::Modified { closing: true }), Bar::None);
        assert_eq!(bar_after_reload(&Bar::Deleted), Bar::None);
        assert_eq!(bar_after_reload(&Bar::None), Bar::None);
        let e = Bar::SaveError { message: "x".into(), jump: None };
        assert_eq!(bar_after_reload(&e), e);
    }

    // ---------- Task 6: compare overlay glue ----------

    #[test]
    fn wheel_lines_take_whole_rows_and_keep_the_fraction() {
        let mut acc = 0.0;
        assert_eq!(wheel_lines(&mut acc, ScrollDelta::Pixels(point(px(0.), px(30.))), px(20.)), 1);
        assert!((acc - 0.5).abs() < 1e-6);
        assert_eq!(wheel_lines(&mut acc, ScrollDelta::Pixels(point(px(0.), px(10.))), px(20.)), 1);
        assert!(acc.abs() < 1e-6);
        assert_eq!(wheel_lines(&mut acc, ScrollDelta::Lines(point(0., -2.5)), px(20.)), -2);
        assert!((acc + 0.5).abs() < 1e-6);
    }

    #[test]
    fn an_open_compare_covers_the_text() {
        assert!(!covered_by(None, false));
        let (diff, rows) = compare::build("a\n", "b\n", 100, &Default::default());
        let c = Compare { diff, rows, expanded: Default::default(), disk_missing: false, scroll: 0, note: "", width_cols: 100 };
        assert!(covered_by(Some(&c), false));
        let missing = Compare { disk_missing: true, ..c };
        assert!(covered_by(Some(&missing), false));
    }

    #[test]
    fn an_open_menu_covers_the_text() {
        assert!(covered_by(None, true));
    }

    #[test]
    fn the_compare_width_is_the_whole_pane_in_cells() {
        // 600px / 10px cells: 60 columns, gutter included → unified.
        assert_eq!(pane_cols(&layout()), 60);
        let wide = LayoutInfo { bounds: Bounds::new(point(px(0.), px(0.)), size(px(800.), px(400.))), ..layout() };
        assert_eq!(pane_cols(&wide), compare::SPLIT_MIN_COLS);
    }

    #[test]
    fn load_model_reads_the_file_and_rejects_a_directory() {
        let dir = std::env::temp_dir().join(format!("gilvt-editor-view-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.md");
        std::fs::write(&file, "a\nb\n").unwrap();
        let m = load_model(&file).unwrap();
        assert_eq!(m.buf.line(1), "b");
        assert_eq!(m.view_rows(), 24);
        assert!(load_model(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
