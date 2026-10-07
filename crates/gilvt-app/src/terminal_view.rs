//! One terminal pane: owns a `TermSession`, routes keyboard / IME / mouse input to it and
//! renders it with `TerminalElement`.

use std::cell::Cell;
use std::ops::{Range, RangeInclusive};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    canvas, div, prelude::*, px, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase, DragMoveEvent,
    EventEmitter, ExternalPaths, FileDropEvent, FocusHandle, Focusable, HitboxBehavior, HitboxId, KeyDownEvent, ModifiersChangedEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent, Subscription, Task,
    UTF16Selection, Window,
};
use gilvt_term::keys::{encode_key, KeyInput, KeyOptions, Mods};
use gilvt_term::mouse::{self, Action as MouseAction, Button};
use gilvt_term::paste::encode_paste;
use gilvt_term::snapshot::{take_snapshot, Snapshot};
use gilvt_term::paths::PathHit;
use gilvt_term::{Direction, Palette, Scroll, ScrollOutcome, SelectionType, Side, TermEvent, TermMode, TermSession};

use crate::actions::*;
use crate::drop::{drop_action, DropAction};
use crate::agents::PaneKey;
use crate::launcher::place::{self, Queue};
use crate::launch::{session_options, ShellEnv};
use crate::pane_tree::PaneId;
use crate::settings::Settings;
use gilvt_term::CommandLog;

use crate::terminal_element::TerminalElement;
use crate::theme::{hsla, AppSettings, CellMetrics};

pub enum TerminalViewEvent {
    TitleChanged,
    Exited,
    /// Cmd+click on a file path; `in_editor` for Cmd+Shift+click.
    OpenPath { hit: PathHit, in_editor: bool },
    /// ⌘P: open the file palette for this pane.
    FindFile,
    /// Files dropped from Finder, to preview as one Quick Look group.
    Preview(Vec<PathBuf>),
    /// A key the user typed that could answer an agent's approval dialog (written to the PTY as usual).
    Key(PaneKey),
    /// OSC 9 / 777; `focused` = this pane has the keyboard in the key window.
    Notify { note: gilvt_term::Notification, title: String, focused: bool },
    /// A command started or finished (the monitor tab redraws).
    CommandsChanged,
}

/// How long a timeline jump highlights its rows.
const JUMP_HIGHLIGHT: Duration = Duration::from_secs(1);
/// How far above a call's anchor its line is looked for: the agent's spinner, tips and input box (and a TODO
/// list) sit between them.
const CALL_SEARCH_ROWS: usize = 60;
/// Rows a timeline jump highlights.
pub(crate) const JUMP_ROWS: usize = 3;

/// Geometry of the last painted frame, used to map mouse positions to cells.
pub struct LayoutInfo {
    pub bounds: Bounds<Pixels>,
    pub metrics: CellMetrics,
    pub snapshot: Snapshot,
}

pub struct TerminalView {
    pub(crate) session: TermSession,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) marked_text: Option<String>,
    pub(crate) layout: Option<LayoutInfo>,
    /// The link (row + column span) under the mouse while Cmd is held; drawn underlined and
    /// paired with a pointing-hand cursor.
    pub(crate) hovered_link: Option<(usize, RangeInclusive<usize>)>,
    /// The absolute line a timeline jump highlights (3 rows from it, for `JUMP_HIGHLIGHT`), and which jump.
    pub(crate) jump: Option<(u64, u64)>,
    jump_seq: u64,
    last_mouse: Option<Point<Pixels>>,
    title: Option<String>,
    auto_title: String,
    auto_title_checked: Option<Instant>,
    selecting: bool,
    held_button: Option<Button>,
    /// The cell a plain single left click went down on; releasing it there moves the cursor to it.
    click_cell: Option<(usize, usize)>,
    scroll_accum: f32,
    search: Option<String>,
    /// Working directory reported by shell integration (OSC 7); preferred over process lookup.
    reported_cwd: Option<PathBuf>,
    /// Commands run at this pane's prompt (OSC 133).
    commands: CommandLog,
    /// What releasing the files dragged over this pane would do.
    drop_hint: Option<String>,
    /// The pane's hitbox in the last frame: not hovered while an overlay (Quick Look, the palette)
    /// covers the pointer, so no hint shows for a drop that would land there.
    hitbox: Rc<Cell<Option<HitboxId>>>,
    /// Launcher commands waiting for this new pane's shell (M3c §2.1).
    launch_queue: Queue,
    _event_task: Task<()>,
    _focus_subs: Vec<Subscription>,
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Opens a web / mail link with the system handler; the child is reaped on a helper thread.
pub(crate) fn open_link(url: &str) {
    match std::process::Command::new("/usr/bin/open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => eprintln!("gilvt: failed to open {url}: {e}"),
    }
}

/// The [`PaneKey`] of a key press: Enter / Esc / one character, without ⌘ / ⌃ / ⌥ (⇧ is fine).
fn pane_key(key: &str, text: Option<&str>, mods: Mods) -> Option<PaneKey> {
    if mods.cmd || mods.ctrl || mods.alt {
        return None;
    }
    match key {
        "enter" => Some(PaneKey::Enter),
        "escape" => Some(PaneKey::Escape),
        "tab" if !mods.shift => Some(PaneKey::Tab),
        _ => typed_char(text?),
    }
}

/// One printable character typed (IME commits and plain keys alike).
fn typed_char(text: &str) -> Option<PaneKey> {
    let mut chars = text.chars();
    let c = chars.next()?;
    (chars.next().is_none() && !c.is_control()).then_some(PaneKey::Char(c))
}

/// Starts a shell for pane `pane`. The real size is applied on first layout.
pub fn spawn_session(settings: &Settings, env: &ShellEnv, pane: PaneId, cwd: Option<PathBuf>) -> std::io::Result<TermSession> {
    let opts = session_options(settings, env, pane, cwd.filter(|p| p.is_dir()), |k| std::env::var(k).ok());
    TermSession::spawn(opts)
}

impl TerminalView {
    pub fn new(session: TermSession, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let events = session.events();
        let event_task = cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this.update_in(cx, |view, window, cx| view.handle_event(event, window, cx)).is_err() {
                    break;
                }
            }
        });
        let focus_handle = cx.focus_handle();
        let focus_subs = vec![
            cx.on_focus(&focus_handle, window, |view, _, cx| view.report_focus(true, cx)),
            cx.on_blur(&focus_handle, window, |view, _, cx| view.report_focus(false, cx)),
        ];
        Self {
            session,
            focus_handle,
            marked_text: None,
            layout: None,
            hovered_link: None,
            jump: None,
            jump_seq: 0,
            last_mouse: None,
            title: None,
            auto_title: "shell".into(),
            auto_title_checked: None,
            selecting: false,
            held_button: None,
            click_cell: None,
            scroll_accum: 0.0,
            search: None,
            reported_cwd: None,
            commands: CommandLog::default(),
            drop_hint: None,
            hitbox: Rc::new(Cell::new(None)),
            launch_queue: Queue::default(),
            _event_task: event_task,
            _focus_subs: focus_subs,
        }
    }

    /// Title set by the program (OSC 0/2), else "foreground-process — dir".
    pub fn title(&self) -> String {
        self.title.clone().unwrap_or_else(|| self.auto_title.clone())
    }

    /// Recomputes the fallback title at most twice a second; emits TitleChanged when it changes.
    fn refresh_auto_title(&mut self, cx: &mut Context<Self>) {
        if self.auto_title_checked.is_some_and(|t| t.elapsed() < Duration::from_millis(500)) {
            return;
        }
        self.auto_title_checked = Some(Instant::now());
        let next = gilvt_term::procinfo::auto_title(self.session.foreground_name().as_deref(), self.cwd().as_deref());
        if next != self.auto_title {
            self.auto_title = next;
            if self.title.is_none() {
                cx.emit(TerminalViewEvent::TitleChanged);
            }
        }
    }

    /// Commands run at this pane's prompt (newest last), for the monitor tab.
    pub fn commands(&self) -> &CommandLog {
        &self.commands
    }

    /// The shell's working directory: the last OSC 7 report when it is local, else the
    /// foreground process's cwd.
    pub fn cwd(&self) -> Option<PathBuf> {
        self.reported_cwd.clone().or_else(|| self.session.cwd())
    }


    /// Inspector timeline jump: scrolls so absolute `line` sits about a third from the top and highlights it
    /// and the 2 rows below for a second. Only the view scrolls; nothing is written to the PTY.
    /// Jumps to a timeline call anchored at `anchor` (the cursor line when the call started, usually the
    /// agent's input box below it): to the nearest line above showing all of the first set of `needles` that
    /// matches within [`CALL_SEARCH_ROWS`], else to the anchor itself.
    pub fn jump_to_call(&mut self, anchor: u64, needles: &[Vec<String>], cx: &mut Context<Self>) -> ScrollOutcome {
        let found = needles.iter().find_map(|set| {
            let set: Vec<&str> = set.iter().map(String::as_str).collect();
            self.session.find_line_above(anchor, CALL_SEARCH_ROWS, &set)
        });
        let line = found.unwrap_or(anchor);
        self.jump_to_line(line, cx)
    }

    pub fn jump_to_line(&mut self, line: u64, cx: &mut Context<Self>) -> ScrollOutcome {
        let outcome = self.session.scroll_to_absolute(line);
        if let ScrollOutcome::Shown { .. } = outcome {
            self.hovered_link = None;
            self.jump_seq += 1;
            let seq = self.jump_seq;
            self.jump = Some((line, seq));
            cx.spawn(async move |view, cx| {
                cx.background_executor().timer(JUMP_HIGHLIGHT).await;
                let _ = view.update(cx, |view, cx| {
                    if view.jump.is_some_and(|(_, s)| s == seq) {
                        view.jump = None;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
        outcome
    }

    pub fn searching(&self) -> bool {
        self.search.is_some()
    }

    pub fn palette(_window: &Window, cx: &gpui::App) -> Palette {
        crate::theme::current(cx).palette.clone()
    }

    pub(crate) fn snapshot(&self, palette: &Palette) -> Snapshot {
        let term = self.session.term().lock();
        take_snapshot(&term, palette, self.session.search_match())
    }

    fn mode(&self) -> TermMode {
        *self.session.term().lock().mode()
    }

    fn handle_event(&mut self, event: TermEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Before the match moves `event`; reads the grid, so outside any `term().lock()`.
        if self.commands.observe(&event, &self.session, self.reported_cwd.clone()) {
            cx.emit(TerminalViewEvent::CommandsChanged);
        }
        if self.session.handle_reply(&event, &Self::palette(window, cx)) {
            return;
        }
        match event {
            TermEvent::Wakeup => {
                self.refresh_auto_title(cx);
                cx.notify();
            }
            TermEvent::Title(t) => {
                self.title = Some(t);
                cx.emit(TerminalViewEvent::TitleChanged);
            }
            TermEvent::ResetTitle => {
                self.title = None;
                cx.emit(TerminalViewEvent::TitleChanged);
            }
            TermEvent::ClipboardStore(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            TermEvent::Notification(note) => {
                let focused = window.is_window_active() && self.focus_handle.is_focused(window);
                cx.emit(TerminalViewEvent::Notify { note, title: self.title(), focused });
            }
            TermEvent::ChildExit(_) => cx.emit(TerminalViewEvent::Exited),
            // A remote report (ssh) says nothing about local paths: fall back to the process cwd.
            TermEvent::Cwd(c) => {
                self.reported_cwd = c.is_local().then_some(c.path);
                self.auto_title_checked = None;
                self.refresh_auto_title(cx);
            }
            // The first prompt of a new pane releases a launcher command; command blocks come later.
            TermEvent::Prompt(mark) => {
                if let Some(text) = self.launch_queue.on_prompt(mark) {
                    self.type_text(text, cx);
                }
            }
            TermEvent::Bell | TermEvent::CommandLine(_) | TermEvent::CommandOutput(_) | TermEvent::PtyWrite(_) | TermEvent::ColorRequest(..) | TermEvent::TextAreaSizeRequest(_) => {}
        }
    }

    fn report_focus(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self.mode().contains(TermMode::FOCUS_IN_OUT) {
            self.session.write(if focused { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() });
        }
        cx.notify();
    }

    // ---------- keyboard ----------

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let mods = Mods { shift: ks.modifiers.shift, alt: ks.modifiers.alt, ctrl: ks.modifiers.control, cmd: ks.modifiers.platform };
        if self.search.is_some() {
            self.search_key(&ks.key, ks.key_char.as_deref(), mods, cx);
            cx.stop_propagation();
            return;
        }
        if self.marked_text.is_some() {
            return; // IME composition owns the keyboard.
        }
        let input = KeyInput { key: &ks.key, text: ks.key_char.as_deref(), mods, repeat: event.is_held };
        let opts = KeyOptions { option_as_meta: cx.global::<AppSettings>().0.option_as_meta };
        if let Some(bytes) = encode_key(&input, self.mode(), &opts) {
            self.session.clear_selection();
            self.session.write_input(bytes);
            if let Some(key) = pane_key(&ks.key, ks.key_char.as_deref(), mods) {
                cx.emit(TerminalViewEvent::Key(key));
            }
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn search_key(&mut self, key: &str, text: Option<&str>, mods: Mods, cx: &mut Context<Self>) {
        let Some(query) = self.search.as_mut() else { return };
        match key {
            "escape" => {
                self.search = None;
                self.session.end_search();
            }
            "enter" => {
                let dir = if mods.shift { Direction::Right } else { Direction::Left };
                self.session.search_step(dir);
            }
            "backspace" => {
                query.pop();
                let q = query.clone();
                self.session.search(&q);
            }
            _ if !mods.cmd && !mods.ctrl => {
                if let Some(t) = text.filter(|t| !t.chars().any(char::is_control)) {
                    query.push_str(t);
                    let q = query.clone();
                    self.session.search(&q);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    // ---------- actions ----------

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.session.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return };
        if let Some(query) = self.search.as_mut() {
            query.push_str(&search_paste_text(&text));
            let q = query.clone();
            self.session.search(&q);
            cx.notify();
            return;
        }
        self.paste_text(&text);
    }

    /// Writes `text` the way ⌘V does (bracketed when the program asks for it). Only for text the
    /// user explicitly puts there: pastes, ⌥⏎ in the file palette, files dropped from Finder.
    pub fn paste_text(&mut self, text: &str) {
        self.session.clear_selection();
        self.session.write_input(encode_paste(text, self.mode()));
    }

    /// A launcher command still waits for this pane's first prompt.
    pub fn launch_pending(&self) -> bool {
        self.launch_queue.is_pending()
    }

    /// Types a launcher command plus Return (M3c §2.1): now (`wait` None, an idle shell), else at the shell's
    /// first prompt mark or after `wait`, whichever comes first. Only for an explicit user action.
    pub fn type_command(&mut self, command: String, wait: Option<Duration>, cx: &mut Context<Self>) {
        let Some(wait) = wait else {
            self.type_text(place::typed(&command), cx);
            return;
        };
        let seq = self.launch_queue.push(command);
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(wait).await;
            let _ = view.update(cx, |view, cx| {
                if let Some(text) = view.launch_queue.on_timeout(seq) {
                    view.type_text(text, cx);
                }
            });
        })
        .detach();
    }

    /// Writes `text` like typed keys (unbracketed; the shell sees each line's Return).
    fn type_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.session.clear_selection();
        self.session.write_input(text.into_bytes());
        cx.notify();
    }

    fn find_file(&mut self, _: &FindFile, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TerminalViewEvent::FindFile);
    }

    fn find(&mut self, _: &Find, _: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_none() {
            self.search = Some(String::new());
        }
        cx.notify();
    }

    fn clear_scrollback(&mut self, _: &ClearScrollback, _: &mut Window, cx: &mut Context<Self>) {
        self.session.clear_history();
        self.search = None;
        cx.notify();
    }

    fn scroll_page_up(&mut self, _: &ScrollPageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.session.scroll(Scroll::PageUp);
        self.hovered_link = None;
        cx.notify();
    }

    fn scroll_page_down(&mut self, _: &ScrollPageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.session.scroll(Scroll::PageDown);
        self.hovered_link = None;
        cx.notify();
    }

    fn scroll_to_top(&mut self, _: &ScrollToTop, _: &mut Window, cx: &mut Context<Self>) {
        self.session.scroll(Scroll::Top);
        self.hovered_link = None;
        cx.notify();
    }

    fn scroll_to_bottom(&mut self, _: &ScrollToBottom, _: &mut Window, cx: &mut Context<Self>) {
        self.session.scroll(Scroll::Bottom);
        self.hovered_link = None;
        cx.notify();
    }

    // ---------- mouse ----------

    /// Viewport cell under `pos`, plus which half of the cell was hit.
    fn cell_at(&self, pos: Point<Pixels>) -> Option<(usize, usize, Side)> {
        let l = self.layout.as_ref()?;
        let x = (pos.x - l.bounds.origin.x) / l.metrics.cell_width;
        let y = (pos.y - l.bounds.origin.y) / l.metrics.line_height;
        let cols = l.snapshot.cols;
        let rows = l.snapshot.rows.len();
        let col = (x.max(0.0) as usize).min(cols.saturating_sub(1));
        let row = (y.max(0.0) as usize).min(rows.saturating_sub(1));
        let side = if x.fract() < 0.5 { Side::Left } else { Side::Right };
        Some((row, col, side))
    }

    fn mouse_down(&mut self, button: Button, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.click_cell = None;
        let Some((row, col, side)) = self.cell_at(event.position) else { return };
        let m = event.modifiers;
        let mods = Mods { shift: m.shift, alt: m.alt, ctrl: m.control, cmd: m.platform };

        if button == Button::Left && m.platform {
            if let Some(url) = self
                .layout
                .as_ref()
                .and_then(|l| gilvt_term::links::link_at(&l.snapshot, row, col))
                .filter(|u| gilvt_term::links::is_openable(u))
            {
                open_link(&url);
            } else if let Some(hit) = self.path_at(row, col) {
                cx.emit(TerminalViewEvent::OpenPath { hit, in_editor: m.shift });
            }
            return;
        }
        let mode = self.mode();
        if mouse::reporting_enabled(mode) && !m.shift {
            if let Some(bytes) = mouse::encode_mouse(button, MouseAction::Press, col, row, mods, mode) {
                self.session.write(bytes);
            }
            self.held_button = Some(button);
            return;
        }
        if button != Button::Left {
            return;
        }
        let point = match &self.layout {
            Some(l) => l.snapshot.grid_point(row, col),
            None => return,
        };
        if m.shift {
            self.session.update_selection(point, side);
        } else {
            let ty = match event.click_count {
                2 => SelectionType::Semantic,
                n if n >= 3 => SelectionType::Lines,
                _ => SelectionType::Simple,
            };
            self.session.start_selection(point, side, ty);
            if event.click_count == 1 && !m.alt && !m.control {
                self.click_cell = Some((row, col));
            }
        }
        self.selecting = true;
        cx.notify();
    }

    /// The existing file named by the text at (row, col), resolved against this pane's cwd.
    fn path_at(&self, row: usize, col: usize) -> Option<PathHit> {
        let layout = self.layout.as_ref()?;
        gilvt_term::paths::path_at(&layout.snapshot, row, col, self.cwd().as_deref(), |p| p.is_file())
    }

    /// Recomputes `hovered_link` for a mouse at `position` with the current Cmd (platform
    /// modifier) state, notifying only when the hovered span actually changes. Called from both
    /// mouse-move (position changes) and modifiers-changed (Cmd pressed/released in place).
    fn update_hovered_link(&mut self, position: Point<Pixels>, cmd: bool, cx: &mut Context<Self>) {
        self.last_mouse = Some(position);
        let next = cmd
            .then(|| self.cell_at(position))
            .flatten()
            .and_then(|(row, col, _)| {
                self.layout
                    .as_ref()
                    .and_then(|l| gilvt_term::links::link_span_at(&l.snapshot, row, col))
                    .map(|(_, span)| (row, span))
                    .or_else(|| self.path_at(row, col).map(|hit| (row, hit.cols)))
            });
        if next != self.hovered_link {
            self.hovered_link = next;
            cx.notify();
        }
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        // Files dragged from Finder arrive as mouse moves; they are not the program's to see.
        if cx.has_active_drag() {
            return;
        }
        self.update_hovered_link(event.position, event.modifiers.platform, cx);
        let Some((row, col, side)) = self.cell_at(event.position) else { return };
        let m = event.modifiers;
        let mods = Mods { shift: m.shift, alt: m.alt, ctrl: m.control, cmd: m.platform };
        let mode = self.mode();
        if mouse::reporting_enabled(mode) && !self.selecting {
            let action = MouseAction::Motion(self.held_button);
            if let Some(bytes) = mouse::encode_mouse(self.held_button.unwrap_or(Button::Left), action, col, row, mods, mode) {
                self.session.write(bytes);
            }
            return;
        }
        if self.selecting && event.pressed_button == Some(MouseButton::Left) {
            if let Some(point) = self.layout.as_ref().map(|l| l.snapshot.grid_point(row, col)) {
                self.session.update_selection(point, side);
                cx.notify();
            }
        }
    }

    fn mouse_up(&mut self, button: Button, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        if let Some(down) = self.click_cell.take() {
            if button == Button::Left && self.cell_at(event.position).is_some_and(|(row, col, _)| (row, col) == down) {
                self.click_to_move(down, cx);
            }
        }
        if let (Some(held), Some((row, col, _))) = (self.held_button.take(), self.cell_at(event.position)) {
            let m = event.modifiers;
            let mods = Mods { shift: m.shift, alt: m.alt, ctrl: m.control, cmd: m.platform };
            if held == button {
                if let Some(bytes) = mouse::encode_mouse(button, MouseAction::Release, col, row, mods, self.mode()) {
                    self.session.write(bytes);
                }
            }
        }
    }

    /// The terminal cursor's viewport cell in the latest frame (`ime_anchor` while hidden) and its bounds.
    pub(crate) fn debug_cursor(&self) -> Option<(usize, usize, Bounds<Pixels>)> {
        let l = self.layout.as_ref()?;
        let (row, col) = l.snapshot.cursor.map_or(l.snapshot.ime_anchor, |c| (c.row, c.col));
        let origin = l.bounds.origin + gpui::point(l.metrics.cell_width * col as f32, l.metrics.line_height * row as f32);
        Some((row, col, Bounds { origin, size: gpui::size(l.metrics.cell_width, l.metrics.line_height) }))
    }

    /// Moves the cursor of the line being edited to `cell` with arrow keys ([`gilvt_term::click_move`]).
    /// Only while the foreground program reads raw keys: a plain command would echo them.
    fn click_to_move(&mut self, cell: (usize, usize), cx: &mut Context<Self>) {
        let Some(l) = self.layout.as_ref() else { return };
        let cursor = l.snapshot.cursor.map_or(l.snapshot.ime_anchor, |c| (c.row, c.col));
        let Some(n) = gilvt_term::click_move::arrow_presses(&l.snapshot, cursor, cell).filter(|&n| n != 0) else { return };
        if !self.session.reads_raw_keys() {
            return;
        }
        let key = KeyInput { key: if n > 0 { "right" } else { "left" }, text: None, mods: Mods::default(), repeat: false };
        let Some(press) = encode_key(&key, self.mode(), &KeyOptions::default()) else { return };
        self.session.clear_selection();
        self.session.write_input(press.repeat(n.unsigned_abs()));
        cx.notify();
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(line_height) = self.layout.as_ref().map(|l| l.metrics.line_height) else { return };
        self.scroll_accum += match event.delta {
            ScrollDelta::Pixels(p) => p.y / line_height,
            ScrollDelta::Lines(l) => l.y,
        };
        let lines = self.scroll_accum.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_accum -= lines as f32;
        let mode = self.mode();
        if mouse::reporting_enabled(mode) {
            if let Some((row, col, _)) = self.cell_at(event.position) {
                let button = if lines > 0 { Button::WheelUp } else { Button::WheelDown };
                let m = event.modifiers;
                let mods = Mods { shift: m.shift, alt: m.alt, ctrl: m.control, cmd: m.platform };
                for _ in 0..lines.unsigned_abs() {
                    if let Some(bytes) = mouse::encode_mouse(button, MouseAction::Press, col, row, mods, mode) {
                        self.session.write(bytes);
                    }
                }
            }
        } else if let Some(bytes) = mouse::alternate_scroll(lines, mode) {
            self.session.write(bytes);
        } else {
            self.session.scroll(Scroll::Delta(lines));
            self.hovered_link = None;
            cx.notify();
        }
    }

    fn drop_paths(&mut self, paths: &ExternalPaths, _: &mut Window, cx: &mut Context<Self>) {
        self.drop_hint = None;
        let foreground = self.session.foreground_name();
        match drop_action(foreground.as_deref(), crate::drop::alt_held(), paths.paths()) {
            Some(DropAction::Insert(text)) => self.paste_text(&text),
            Some(DropAction::Preview(files)) => cx.emit(TerminalViewEvent::Preview(files)),
            None => {}
        }
        cx.notify();
    }

    /// Shows the drop hint while dragged files are over this pane (the listener sees every move in
    /// the window) and keeps it current as ⌥ changes.
    fn drag_move(&mut self, event: &DragMoveEvent<ExternalPaths>, window: &mut Window, cx: &mut Context<Self>) {
        let over = match self.hitbox.get() {
            Some(hitbox) => hitbox.is_hovered(window),
            None => event.bounds.contains(&event.event.position),
        };
        let next = over.then(|| {
            let foreground = self.session.foreground_name();
            crate::drop::hint(foreground.as_deref(), crate::drop::alt_held(), event.drag(cx).paths())
        });
        if next != self.drop_hint {
            self.drop_hint = next;
            cx.notify();
        }
    }

    fn drop_hint_layer(&self, window: &Window, cx: &Context<Self>) -> Option<impl IntoElement> {
        let text = self.drop_hint.clone()?;
        let p = Self::palette(window, cx);
        let view = cx.entity().downgrade();
        // No hitbox: the drop must still land on the pane.
        Some(
            div()
                .absolute()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(p.background).opacity(0.6))
                // The hint goes when the drag ends anywhere: dragged out of the window (no drop), or
                // released (a drop arrives as a mouse up) even where an overlay takes the drop. gpui
                // clears the drag after the mouse up, so a hint still painted without one is stale.
                .child(
                    canvas(|_, _, _| {}, move |_, _, window, cx| {
                        let clear = move |view: &gpui::WeakEntity<TerminalView>, cx: &mut gpui::App| {
                            let _ = view.update(cx, |v, cx| {
                                if v.drop_hint.take().is_some() {
                                    cx.notify();
                                }
                            });
                        };
                        if !cx.has_active_drag() {
                            let view = view.clone();
                            cx.defer(move |cx| clear(&view, cx));
                        }
                        let exited = view.clone();
                        window.on_mouse_event(move |event: &FileDropEvent, _, _, cx| {
                            if matches!(event, FileDropEvent::Exited) {
                                clear(&exited, cx);
                            }
                        });
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                clear(&view, cx);
                            }
                        });
                    })
                    .absolute()
                    .size_full(),
                )
                .child(
                    div()
                        .px_4()
                        .py_2()
                        .rounded_lg()
                        .bg(hsla(crate::theme::mix(p.background, p.foreground, 0.15)))
                        .text_color(hsla(p.foreground))
                        .text_size(px(14.))
                        .child(text),
                ),
        )
    }

    fn search_bar(&self, window: &Window, cx: &Context<Self>) -> Option<impl IntoElement> {
        let query = self.search.as_ref()?;
        let p = Self::palette(window, cx);
        let status = if query.is_empty() {
            String::new()
        } else if self.session.search_match().is_some() {
            "⏎ 上一个 · ⇧⏎ 下一个".to_string()
        } else {
            "无匹配".to_string()
        };
        Some(
            div()
                .absolute()
                .top(px(6.))
                .right(px(10.))
                .px_2()
                .py_1()
                .rounded_md()
                .bg(hsla(crate::theme::mix(p.background, p.foreground, 0.12)))
                .text_color(hsla(p.foreground))
                .text_size(px(12.))
                .flex()
                .gap_2()
                .child(format!("查找：{query}▏"))
                .child(div().text_color(hsla(crate::theme::mix(p.foreground, p.background, 0.4))).child(status)),
        )
    }
}

/// Clipboard text to append to a search query: the first line (up to the first `\n` or `\r`),
/// with control characters removed.
pub(crate) fn search_paste_text(clip: &str) -> String {
    clip.split(['\n', '\r']).next().unwrap_or("").chars().filter(|c| !c.is_control()).collect()
}

impl gpui::EntityInputHandler for TerminalView {
    fn text_for_range(&mut self, _: Range<usize>, _: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        None
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let len = self.marked_text.as_ref().map_or(0, |t| t.encode_utf16().count());
        Some(UTF16Selection { range: len..len, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_text.as_ref().map(|t| 0..t.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = None;
        if !text.is_empty() {
            self.session.clear_selection();
            self.session.write_input(text.as_bytes().to_vec());
            if let Some(key) = typed_char(text) {
                cx.emit(TerminalViewEvent::Key(key));
            }
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    fn bounds_for_range(&mut self, _: Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        // Places the IME candidate window at the cursor.
        // Never `None` once laid out: macOS turns that into a zero rect, i.e. the screen's bottom-left
        // corner. A hidden cursor (TUIs) still has a position.
        let l = self.layout.as_ref()?;
        let (row, col) = l.snapshot.cursor.map_or(l.snapshot.ime_anchor, |c| (c.row, c.col));
        let origin = Point {
            x: l.bounds.origin.x + l.metrics.cell_width * col as f32,
            y: l.bounds.origin.y + l.metrics.line_height * row as f32,
        };
        Some(Bounds::new(origin, gpui::size(l.metrics.cell_width, l.metrics.line_height)))
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("terminal")
            .key_context(crate::actions::TERMINAL_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .relative()
            .overflow_hidden()
            .cursor(if self.hovered_link.is_some() { CursorStyle::PointingHand } else { CursorStyle::IBeam })
            .on_key_down(cx.listener(Self::on_key_down))
            .on_modifiers_changed(cx.listener(|view, event: &ModifiersChangedEvent, _window, cx| {
                if let Some(pos) = view.last_mouse {
                    view.update_hovered_link(pos, event.modifiers.platform, cx);
                }
            }))
            .on_hover(cx.listener(|view, hovered: &bool, _window, cx| {
                if !hovered && (view.hovered_link.is_some() || view.last_mouse.is_some()) {
                    view.hovered_link = None;
                    view.last_mouse = None;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::find_file))
            .on_action(cx.listener(Self::clear_scrollback))
            .on_action(cx.listener(Self::scroll_page_up))
            .on_action(cx.listener(Self::scroll_page_down))
            .on_action(cx.listener(Self::scroll_to_top))
            .on_action(cx.listener(Self::scroll_to_bottom))
            .on_mouse_down(MouseButton::Left, cx.listener(|v, e, w, cx| v.mouse_down(Button::Left, e, w, cx)))
            .on_mouse_down(MouseButton::Middle, cx.listener(|v, e, w, cx| v.mouse_down(Button::Middle, e, w, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|v, e, w, cx| v.mouse_down(Button::Right, e, w, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|v, e, w, cx| v.mouse_up(Button::Left, e, w, cx)))
            .on_mouse_up(MouseButton::Middle, cx.listener(|v, e, w, cx| v.mouse_up(Button::Middle, e, w, cx)))
            .on_mouse_up(MouseButton::Right, cx.listener(|v, e, w, cx| v.mouse_up(Button::Right, e, w, cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|v, e, w, cx| v.mouse_up(Button::Left, e, w, cx)))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(|v, e, w, cx| v.mouse_up(Button::Middle, e, w, cx)))
            .on_mouse_up_out(MouseButton::Right, cx.listener(|v, e, w, cx| v.mouse_up(Button::Right, e, w, cx)))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .on_drop(cx.listener(Self::drop_paths))
            .on_drag_move(cx.listener(Self::drag_move))
            .child(TerminalElement::new(cx.entity()))
            .child({
                // A hitbox that blocks nothing, only to learn whether an overlay covers the pointer.
                let cell = self.hitbox.clone();
                canvas(move |bounds, window, _| cell.set(Some(window.insert_hitbox(bounds, HitboxBehavior::Normal).id)), |_, _, _, _| {})
                    .absolute()
                    .size_full()
            })
            .children(self.search_bar(window, cx))
            .children(self.drop_hint_layer(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::{pane_key, search_paste_text, typed_char, Mods, PaneKey};

    #[test]
    fn takes_first_line() {
        assert_eq!(search_paste_text("abc\ndef"), "abc");
    }

    #[test]
    fn strips_control_characters() {
        assert_eq!(search_paste_text("a\tb"), "ab");
    }

    #[test]
    fn empty_first_line() {
        assert_eq!(search_paste_text("\nx"), "");
    }

    #[test]
    fn approval_keys_of_key_presses() {
        let plain = Mods::default();
        assert_eq!(pane_key("enter", None, plain), Some(PaneKey::Enter));
        assert_eq!(pane_key("escape", None, plain), Some(PaneKey::Escape));
        assert_eq!(pane_key("1", Some("1"), plain), Some(PaneKey::Char('1')));
        assert_eq!(pane_key("y", Some("Y"), Mods { shift: true, ..plain }), Some(PaneKey::Char('Y')));
        assert_eq!(pane_key("enter", None, Mods { cmd: true, ..plain }), None);
        assert_eq!(pane_key("1", Some("1"), Mods { ctrl: true, ..plain }), None);
        assert_eq!(pane_key("up", None, plain), None);
        assert_eq!(pane_key("tab", None, plain), Some(PaneKey::Tab));
        assert_eq!(pane_key("tab", None, Mods { shift: true, ..plain }), None);
        assert_eq!(typed_char("12"), None);
        assert_eq!(typed_char("\t"), None);
        assert_eq!(typed_char("修"), Some(PaneKey::Char('修')));
    }
}
