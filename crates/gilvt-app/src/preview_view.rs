//! Quick Look: a code / diff preview shown as an overlay or pinned as a pane.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui::{
    div, prelude::*, px, Bounds, Context, EventEmitter, ExternalPaths, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseUpEvent, Pixels, ScrollDelta, ScrollWheelEvent, Subscription, Task, Window,
};
use gilvt_viewer::diff::{split_rows, unified_rows};
use gilvt_viewer::{highlight, nav, Appearance, Content, DiffBase, Preview, Row, Span, SplitRow, TurnRange};

use crate::actions::OpenExternalEditor;
use crate::markdown::mermaid::Diagrams;
use crate::markdown::{self, History, MdDoc, MdState};
use crate::preview_element::PreviewElement;
use crate::preview_select::{CellHit, Selection};
use crate::theme::{hsla, mix};

/// Unchanged lines kept around each change before folding.
pub const CONTEXT_LINES: usize = 3;
/// Views at least this wide show diffs side by side.
pub const SPLIT_MIN_WIDTH: f32 = 1100.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    Inline { content: String, as_type: Option<String> },
    /// The editor's buffer, shown by one of the live-preview providers (never read from disk by the view).
    Live(crate::live_preview::LiveInput),
}

/// The pane title of a live source: 「预览 · <file>」, 「预览 · 未命名」 for an unsaved buffer.
fn live_title(source: &Source) -> Option<String> {
    let Source::Live(input) = source else { return None };
    let name = input.path.as_deref().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "未命名".into());
    Some(format!("预览 · {name}"))
}

/// What to show: one or more sources (←/→ switches), where to start, what to compare against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenRequest {
    pub sources: Vec<Source>,
    pub index: usize,
    pub line: Option<u32>,
    /// Message shown next to `line` (e.g. the compiler error that was Cmd+clicked).
    pub annotation: Option<String>,
    /// Revision to diff against first (from `gilvt diff <rev>`); HEAD otherwise.
    pub rev: Option<String>,
    /// Show the file as one agent turn left it (the artifacts tab); the only diff base then.
    pub turn: Option<TurnRange>,
}

impl OpenRequest {
    pub fn file(path: PathBuf, line: Option<u32>, annotation: Option<String>) -> OpenRequest {
        OpenRequest { sources: vec![Source::File(path)], index: 0, line, annotation, rev: None, turn: None }
    }

    /// A group of files (←/→ switches), starting at the first.
    pub fn files(paths: Vec<PathBuf>) -> OpenRequest {
        OpenRequest { sources: paths.into_iter().map(Source::File).collect(), index: 0, line: None, annotation: None, rev: None, turn: None }
    }
}

pub enum PreviewEvent {
    Close,
    Pin,
    /// `T`: Quick Look opens in a tab of its own; a pinned pane leaves its split for one.
    NewTab,
    /// Esc in a pinned pane: give the keyboard back to the terminal it was opened from.
    Leave,
    /// Files dropped on the preview replaced what it shows; `path` is the one now shown (recorded
    /// as recent, like any Quick Look open).
    Opened(PathBuf),
    /// `E` / `⌘O`: edit the shown file in the built-in editor (`flip`: ⌥ was held, split ↔ new tab).
    Edit { path: PathBuf, line: Option<u32>, flip: bool },
    /// `⌘⌥O`: open the shown file in the external editor.
    OpenExternal { path: PathBuf, line: Option<u32> },
}

/// Keys that open the shown file in the built-in editor: `E` (`⌥E` flips the placement) and `⌘O`.
/// Returns the flip. `⌘⌥O` is not one of them: it is the `OpenExternalEditor` action. Matches on the
/// key, not `key_char`, because ⌥E is a dead key on macOS.
pub(crate) fn edit_key(key: &str, m: gpui::Modifiers) -> Option<bool> {
    match (key, m.control, m.shift, m.platform, m.alt) {
        ("e", false, false, false, alt) => Some(alt),
        ("o", false, false, true, false) => Some(false),
        _ => None,
    }
}

/// `D` cycles the diff base of a file; a live preview has no base to cycle (its diff is always the buffer
/// against the disk version).
fn base_key_applies(source: Option<&Source>) -> bool {
    !matches!(source, Some(Source::Live(_)))
}

/// Dropped files replace the shown file; a live preview is fed by its provider and must not turn into a File view.
fn accepts_drop(source: Option<&Source>) -> bool {
    !matches!(source, Some(Source::Live(_)))
}

/// `S` flips rendered / source; for a live preview the provider decides the view and the next refresh would undo a toggle.
pub(crate) fn source_toggle_applies(source: Option<&Source>) -> bool {
    !matches!(source, Some(Source::Live(_)))
}

/// Following a Markdown link to a file swaps the source for a `File`; a live preview never navigates.
pub(crate) fn link_navigation_applies(source: Option<&Source>) -> bool {
    !matches!(source, Some(Source::Live(_)))
}

/// The line an edit or external open starts at: the rendered document's top line, else the annotated line.
pub(crate) fn edit_line(rendered_top: Option<u32>, annotation: Option<u32>) -> Option<u32> {
    rendered_top.or(annotation)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Unified,
    Split,
}

pub struct Loaded {
    pub preview: Preview,
    /// Highlight spans per diff line.
    pub spans: Vec<Vec<Span>>,
}

pub enum Rows {
    Unified(Vec<Row>),
    Split(Vec<SplitRow>),
}

impl Rows {
    pub fn len(&self) -> usize {
        match self {
            Rows::Unified(r) => r.len(),
            Rows::Split(r) => r.len(),
        }
    }
}

/// Geometry of the last paint, for scrolling limits and mouse hit-testing.
#[derive(Clone, Copy, Debug)]
pub struct PreviewLayout {
    pub bounds: Bounds<Pixels>,
    pub row_height: Pixels,
    pub visible_rows: usize,
    pub total_rows: usize,
    pub rail: Bounds<Pixels>,
    pub mode: Mode,
}

pub struct PreviewView {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) sources: Vec<Source>,
    pub(crate) index: usize,
    bases: Vec<DiffBase>,
    base_ix: usize,
    pub(crate) loaded: Option<Loaded>,
    pub(crate) error: Option<String>,
    pub(crate) mode_override: Option<Mode>,
    pub(crate) expanded: HashSet<usize>,
    /// First visible row (fractional for smooth trackpad scrolling).
    pub(crate) scroll_top: f32,
    /// New-text line to scroll to once rows are laid out.
    pub(crate) pending_line: Option<u32>,
    pub(crate) annotation: Option<(u32, String)>,
    pub(crate) layout: Option<PreviewLayout>,
    /// Mouse selection in the code view, and the cells of the last paint it is hit-tested against.
    pub(crate) selection: Option<Selection>,
    pub(crate) dragging: bool,
    pub(crate) hits: Vec<CellHit>,
    /// Rendered Markdown: the selection and the texts it is hit-tested against.
    pub(crate) md_sel: Option<crate::markdown::select::MdSelection>,
    pub(crate) md_texts: crate::markdown::select::Texts,
    /// The pane this view is pinned in (for `debug state`'s rects); None for the Quick Look overlay.
    pub(crate) pane: Option<crate::pane_tree::PaneId>,
    /// Markdown files: the rendered document (shown unless `show_source`).
    pub(crate) md: Option<MdState>,
    pub(crate) show_source: bool,
    /// Documents left by following links (`⌘[` returns).
    pub(crate) history: History,
    /// Source line a rendered document should open at (history, no highlight).
    pub(crate) restore_line: Option<u32>,
    /// Heading a followed link points at, applied once the document loads.
    pub(crate) pending_anchor: Option<String>,
    /// Transient message at the top (e.g. a link target that does not exist).
    pub(crate) banner: Option<String>,
    pub(crate) _banner: Option<Task<()>>,
    /// Mermaid diagrams of the rendered document.
    pub(crate) diagrams: Diagrams,
    appearance: Appearance,
    stale: bool,
    pinned: bool,
    live_refreshes: u64,
    /// A live preview of a buffer past the size limits keeps its 「文件太大」 banner across builds (decided by
    /// the editor, which owns the line count).
    live_keep_banner: bool,
    _watcher: Option<notify::RecommendedWatcher>,
    _load: Option<Task<()>>,
    _highlight: Option<Task<()>>,
    _watch: Option<Task<()>>,
    _release: Subscription,
}

impl EventEmitter<PreviewEvent> for PreviewView {}

impl Focusable for PreviewView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// What `b` cycles through: one agent turn's range alone when given, else `rev` (from `gilvt diff <rev>`)
/// first when given, then HEAD, then no diff.
fn diff_bases(rev: Option<String>, turn: Option<TurnRange>) -> Vec<DiffBase> {
    if let Some(range) = turn {
        return vec![DiffBase::Turn(range)];
    }
    let default = [DiffBase::Head, DiffBase::None];
    rev.map(DiffBase::Rev).into_iter().chain(default).collect()
}

/// `path` with its directory resolved (FSEvents reports real paths, e.g. /private/tmp for /tmp);
/// the file name is kept so a file that was just removed or renamed still matches.
pub(crate) fn canonical_target(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    Some(dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()).join(name))
}

fn find_in_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    // GUI apps get a minimal PATH, so also look where installers usually put CLIs.
    let found = path
        .split(':')
        .chain(["/usr/local/bin", "/opt/homebrew/bin"])
        .map(|d| Path::new(d).join(program))
        .find(|p| p.is_file());
    found
}

/// `code -g file:line` when VS Code's CLI is installed, else the default text editor.
pub fn open_in_editor(path: &Path, line: Option<u32>) {
    let mut cmd = match find_in_path("code") {
        Some(code) => {
            let mut c = std::process::Command::new(code);
            c.arg("-g").arg(format!("{}:{}", path.display(), line.unwrap_or(1)));
            c
        }
        None => {
            let mut c = std::process::Command::new("/usr/bin/open");
            c.arg("-t").arg(path);
            c
        }
    };
    let spawned = cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    match spawned {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => eprintln!("gilvt: failed to open editor for {}: {e}", path.display()),
    }
}

impl PreviewView {
    pub fn new(req: OpenRequest, pinned: bool, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let bases = diff_bases(req.rev, req.turn);
        let appearance = if crate::theme::current(cx).dark { Appearance::Dark } else { Appearance::Light };
        let mut view = PreviewView {
            focus_handle: cx.focus_handle(),
            index: req.index.min(req.sources.len().saturating_sub(1)),
            sources: req.sources,
            bases,
            base_ix: 0,
            loaded: None,
            error: None,
            mode_override: None,
            expanded: HashSet::new(),
            scroll_top: 0.0,
            pending_line: req.line,
            annotation: req.line.zip(req.annotation),
            layout: None,
            selection: None,
            dragging: false,
            hits: Vec::new(),
            md_sel: None,
            md_texts: Default::default(),
            pane: None,
            md: None,
            show_source: false,
            history: History::default(),
            restore_line: None,
            pending_anchor: None,
            banner: None,
            _banner: None,
            diagrams: Diagrams::default(),
            appearance,
            stale: false,
            pinned,
            live_refreshes: 0,
            live_keep_banner: false,
            _watcher: None,
            _load: None,
            _highlight: None,
            _watch: None,
            _release: cx.on_release(|view, cx| view.diagrams.release(cx)),
        };
        view.load(cx);
        view
    }

    pub fn set_pinned(&mut self, pinned: bool, cx: &mut Context<Self>) {
        self.pinned = pinned;
        if pinned && self.stale {
            self.reload(cx);
        }
        cx.notify();
    }

    /// A pane that follows an editor buffer: pinned (no Quick Look keys that close or pin it), showing the diff
    /// for the 「改动」 provider and the typeset document otherwise.
    pub fn new_live(input: crate::live_preview::LiveInput, too_large: bool, window: &mut Window, cx: &mut Context<Self>) -> PreviewView {
        let changes = input.provider == crate::live_preview::Provider::Changes;
        let req = OpenRequest { sources: vec![Source::Live(input)], index: 0, line: None, annotation: None, rev: None, turn: None };
        let mut view = PreviewView::new(req, true, window, cx);
        view.show_source = changes;
        view.live_keep_banner = too_large;
        view
    }

    /// The buffer (or the provider) changed: rebuild in the background. The picture stays until the new one is
    /// ready; the scroll position is kept (`set_markdown` restores the top line).
    pub fn set_live(&mut self, input: crate::live_preview::LiveInput, too_large: bool, cx: &mut Context<Self>) {
        self.live_keep_banner = too_large;
        self.show_source = input.provider == crate::live_preview::Provider::Changes;
        match self.sources.get_mut(self.index) {
            Some(Source::Live(current)) => *current = input,
            _ => {
                self.sources = vec![Source::Live(input)];
                self.index = 0;
            }
        }
        if !too_large {
            self.banner = None;
        }
        self.spawn_load(cx);
    }

    pub fn set_banner_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.banner = Some(text);
        cx.notify();
    }

    /// `gilvt debug state`: `(provider, successful refreshes, banner)` of a live preview.
    pub fn debug_live(&self) -> Option<(&'static str, u64, Option<String>)> {
        match self.source()? {
            Source::Live(input) => Some((input.provider.id(), self.live_refreshes, self.banner.clone())),
            _ => None,
        }
    }

    pub(crate) fn source(&self) -> Option<&Source> {
        self.sources.get(self.index)
    }

    /// `gilvt debug state`: the file, the diff base as drawn and the mode of the last layout.
    pub fn debug_overlay(&self, rect: &dyn Fn(crate::debug_state::rects::RectId) -> Option<crate::debug_state::rects::Rect4>) -> crate::debug_state::Overlay {
        let base = self.loaded.as_ref().map(|l| if l.preview.repo_root.is_some() { l.preview.base.label() } else { DiffBase::None.label() });
        let mode = match (self.has_markdown() && !self.show_source, self.layout.map(|l| l.mode)) {
            (true, _) => "rendered",
            (false, Some(Mode::Split)) => "split",
            (false, _) => "unified",
        };
        crate::debug_state::Overlay::Quicklook {
            path: self.file_path().map(|p| p.display().to_string()),
            base,
            mode,
            code: rect(crate::debug_state::rects::RectId::PreviewCode(self.pane)),
            selection: self.selection_text(),
            changed_since: self.loaded.as_ref().is_some_and(|l| l.preview.changed_since),
        }
    }

    pub fn file_path(&self) -> Option<&Path> {
        match self.source()? {
            Source::File(p) => Some(p),
            Source::Inline { .. } | Source::Live(_) => None,
        }
    }

    pub fn title(&self) -> String {
        match self.source() {
            Some(Source::File(p)) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            Some(s @ Source::Live(_)) => live_title(s).unwrap_or_default(),
            Some(Source::Inline { .. }) => "stdin".into(),
            None => "没有改动".into(),
        }
    }

    /// Loads the current source and starts watching it.
    pub(crate) fn load(&mut self, cx: &mut Context<Self>) {
        self.spawn_load(cx);
        self.watch(cx);
    }

    /// Reads + diffs (and parses Markdown) on the background executor and shows it right away,
    /// then highlights it.
    fn spawn_load(&mut self, cx: &mut Context<Self>) {
        self.stale = false;
        let Some(source) = self.source().cloned() else { return };
        let base = self.bases[self.base_ix].clone();
        let task = cx.background_executor().spawn(async move {
            let preview = match source {
                Source::File(p) => Preview::load(&p, &base).map_err(|e| format!("{}: {e}", p.display()))?,
                Source::Inline { content, as_type } => Preview::from_content(content, as_type),
                Source::Live(input) => crate::live_preview::build_preview(&input, &crate::live_preview::cache_dir())?,
            };
            let md = MdDoc::build(&preview);
            Ok::<_, String>((preview, md))
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok((preview, md)) => {
                        view.clear_selection();
                        view.loaded = Some(Loaded { preview, spans: Vec::new() });
                        view.error = None;
                        if matches!(view.source(), Some(Source::Live(_))) {
                            view.live_refreshes += 1;
                            if !view.live_keep_banner {
                                view.banner = None;
                            }
                        }
                        view.set_markdown(md, cx);
                        view.highlight(cx);
                    }
                    Err(e) if matches!(view.source(), Some(Source::Live(_))) => view.banner = Some(e),
                    Err(e) => view.error = Some(e),
                }
                cx.notify();
            });
        }));
    }

    /// Highlights what is loaded in the current appearance (the slow part) and applies the
    /// colors when done, replacing highlighting still running for an older load or theme.
    fn highlight(&mut self, cx: &mut Context<Self>) {
        let Some(loaded) = &self.loaded else { return };
        let job = loaded.preview.diff.clone().map(|d| (d, loaded.preview.doc.syntax_token()));
        let code = self.md.as_ref().map(|md| (md.doc.clone(), md.doc.code_blocks()));
        let appearance = self.appearance;
        let background = cx.background_executor().clone();
        self._highlight = Some(cx.spawn(async move |this, cx| {
            // Code blocks of a rendered document first: they are what is on screen.
            if let Some((doc, blocks)) = code {
                let spans = background.spawn(async move { markdown::highlight_code(blocks, appearance) }).await;
                let _ = this.update(cx, |view, cx| view.set_code_spans(&doc, spans, cx));
            }
            let Some((diff, token)) = job else { return };
            let spans = background.spawn(async move { highlight::highlight_diff(&diff, &token, appearance) }).await;
            let _ = this.update(cx, |view, cx| {
                if let Some(loaded) = view.loaded.as_mut() {
                    loaded.spans = spans;
                    cx.notify();
                }
            });
        }));
    }

    /// Watches the file's directory (editors often replace files by rename) for changes to it.
    fn watch(&mut self, cx: &mut Context<Self>) {
        use notify::Watcher;
        self._watcher = None;
        self._watch = None;
        let Some(target) = self.file_path().and_then(canonical_target) else { return };
        let Some(dir) = target.parent().map(Path::to_path_buf) else { return };
        let (tx, rx) = async_channel::unbounded::<()>();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok_and(|e| e.paths.iter().any(|p| p == &target || canonical_target(p).as_ref() == Some(&target))) {
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
                while rx.try_recv().is_ok() {}
                if this.update(cx, |view, cx| view.file_changed(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    fn file_changed(&mut self, cx: &mut Context<Self>) {
        if self.pinned {
            self.reload(cx);
        } else {
            self.stale = true;
            cx.notify();
        }
    }

    /// Re-reads the same file keeping scroll position and folds (the file changed on disk).
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.spawn_load(cx);
    }

    /// Rows for `mode`, or `None` when nothing text-like is loaded.
    pub(crate) fn rows(&self, mode: Mode) -> Option<Rows> {
        let diff = self.loaded.as_ref()?.preview.diff.as_ref()?;
        Some(match mode {
            Mode::Split if diff.has_changes() => Rows::Split(split_rows(diff, CONTEXT_LINES, &self.expanded)),
            _ => Rows::Unified(unified_rows(diff, CONTEXT_LINES, &self.expanded)),
        })
    }

    /// Split when wide enough (or forced with `U`), unless there is nothing to compare.
    pub(crate) fn mode_for_width(&self, width: Pixels) -> Mode {
        self.mode_override.unwrap_or(if width / px(1.) >= SPLIT_MIN_WIDTH { Mode::Split } else { Mode::Unified })
    }

    fn max_scroll(&self) -> f32 {
        self.layout.map_or(0.0, |l| l.total_rows.saturating_sub(l.visible_rows.saturating_sub(1)) as f32)
    }

    /// Expands the fold hiding the pending "scroll to line" target, so the jump lands on the line.
    pub(crate) fn unfold_pending_line(&mut self) {
        let Some(line) = self.pending_line else { return };
        let Some(diff) = self.loaded.as_ref().and_then(|l| l.preview.diff.as_ref()) else { return };
        let rows = unified_rows(diff, CONTEXT_LINES, &self.expanded);
        if let Some(start) = nav::fold_containing(&rows, diff.index_of_new_line(line)) {
            self.expanded.insert(start);
        }
    }

    fn scroll_by(&mut self, rows: f32, cx: &mut Context<Self>) {
        self.scroll_top = (self.scroll_top + rows).clamp(0.0, self.max_scroll());
        cx.notify();
    }

    fn scroll_to_row(&mut self, row: usize, cx: &mut Context<Self>) {
        // Keep a few rows of context above the target.
        self.scroll_top = (row as f32 - 3.0).clamp(0.0, self.max_scroll());
        cx.notify();
    }

    fn jump_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(layout) = self.layout else { return };
        let Some(diff) = self.loaded.as_ref().and_then(|l| l.preview.diff.as_ref()) else { return };
        let starts = match self.rows(layout.mode) {
            Some(Rows::Unified(r)) => nav::change_starts(r.len(), |i| nav::unified_changed(diff, &r[i])),
            Some(Rows::Split(r)) => nav::change_starts(r.len(), |i| nav::split_changed(diff, &r[i])),
            None => return,
        };
        // "Current" is the row shown just below the 3 context rows kept by scroll_to_row.
        let current = self.scroll_top as usize + 3;
        let target = if forward { nav::next_change(&starts, current) } else { nav::prev_change(&starts, current) };
        if let Some(row) = target {
            self.scroll_to_row(row, cx);
        }
    }

    fn switch_file(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.sources.len() as isize;
        if n <= 1 {
            return;
        }
        self.index = (self.index as isize + delta).rem_euclid(n) as usize;
        self.reset_view_state();
        self.load(cx);
        cx.notify();
    }

    /// Files dropped from Finder replace what is shown, as a fresh open would: no link history, diffed
    /// against HEAD (directories are skipped; nothing changes when only directories, or no paths,
    /// are dropped).
    fn drop_files(&mut self, paths: &ExternalPaths, _: &mut Window, cx: &mut Context<Self>) {
        if !accepts_drop(self.source()) {
            return;
        }
        let files = crate::drop::preview_files(paths.paths());
        if files.is_empty() {
            return;
        }
        cx.emit(PreviewEvent::Opened(files[0].clone()));
        self.sources = files.into_iter().map(Source::File).collect();
        self.index = 0;
        self.history = History::default();
        self.bases = diff_bases(None, None);
        self.base_ix = 0;
        self.reset_view_state();
        self.load(cx);
        cx.notify();
    }

    fn cycle_base(&mut self, cx: &mut Context<Self>) {
        self.base_ix = (self.base_ix + 1) % self.bases.len();
        self.reset_view_state();
        self.load(cx);
    }

    pub(crate) fn reset_view_state(&mut self) {
        self.clear_selection();
        self.loaded = None;
        self.md = None;
        self.expanded.clear();
        self.scroll_top = 0.0;
        self.pending_line = None;
        self.restore_line = None;
        self.pending_anchor = None;
        self.annotation = None;
        self.banner = None;
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.markdown_key(event, cx) {
            cx.stop_propagation();
            return;
        }
        let ks = &event.keystroke;
        let m = ks.modifiers;
        if let Some(flip) = edit_key(&ks.key, m) {
            // An inline source has no file to edit: the key does nothing.
            if let Some((path, line)) = self.edit_target() {
                cx.emit(PreviewEvent::Edit { path, line, flip });
                cx.stop_propagation();
            }
            return;
        }
        let half = self.layout.map_or(10.0, |l| (l.visible_rows / 2).max(1) as f32);
        let page = self.layout.map_or(20.0, |l| l.visible_rows.saturating_sub(2).max(1) as f32);
        match (ks.key.as_str(), m.control, m.shift, m.platform) {
            ("escape", false, false, false) | ("space", false, false, false) if !self.pinned => cx.emit(PreviewEvent::Close),
            ("enter", false, false, false) if !self.pinned => cx.emit(PreviewEvent::Pin),
            ("t", false, false, false) if !m.alt => cx.emit(PreviewEvent::NewTab),
            ("escape", false, false, false) => cx.emit(PreviewEvent::Leave),
            ("j", false, false, false) | ("down", false, false, false) => self.scroll_by(1.0, cx),
            ("k", false, false, false) | ("up", false, false, false) => self.scroll_by(-1.0, cx),
            ("d", true, false, false) => self.scroll_by(half, cx),
            ("u", true, false, false) => self.scroll_by(-half, cx),
            ("pagedown", false, false, false) => self.scroll_by(page, cx),
            ("pageup", false, false, false) => self.scroll_by(-page, cx),
            ("g", false, false, false) => self.scroll_by(-f32::MAX / 2.0, cx),
            ("g", false, true, false) => self.scroll_by(f32::MAX / 2.0, cx),
            ("n", false, false, false) => self.jump_change(true, cx),
            ("p", false, false, false) => self.jump_change(false, cx),
            ("right", false, false, false) => self.switch_file(1, cx),
            ("left", false, false, false) => self.switch_file(-1, cx),
            ("u", false, _, false) => {
                let current = self.layout.map_or(Mode::Unified, |l| l.mode);
                self.clear_selection();
                self.mode_override = Some(if current == Mode::Split { Mode::Unified } else { Mode::Split });
                self.scroll_top = 0.0;
                cx.notify();
            }
            ("d", false, _, false) if base_key_applies(self.source()) => self.cycle_base(cx),
            ("r", false, _, false) => {
                self.retry_diagrams(cx);
                self.reload(cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Where `E` / `⌘O` / `⌘⌥O` open the shown file: None for an inline source.
    fn edit_target(&self) -> Option<(PathBuf, Option<u32>)> {
        let path = self.file_path()?.to_path_buf();
        let top = if self.rendered() { self.md.as_ref().map(|md| md.top_line()) } else { None };
        Some((path, edit_line(top, self.annotation.as_ref().map(|(l, _)| *l))))
    }

    fn open_external(&mut self, _: &OpenExternalEditor, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((path, line)) = self.edit_target() {
            cx.emit(PreviewEvent::OpenExternal { path, line });
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        // The rendered view's list scrolls itself.
        if self.rendered() {
            return;
        }
        let Some(layout) = self.layout else { return };
        let rows = match event.delta {
            ScrollDelta::Pixels(p) => -(p.y / layout.row_height),
            ScrollDelta::Lines(l) => -l.y * 3.0,
        };
        self.scroll_by(rows, cx);
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        window.focus(&self.focus_handle);
        if self.rendered() {
            self.md_begin_select(event.position);
            cx.notify();
            return;
        }
        let Some(layout) = self.layout else { return };
        let pos = event.position;
        if layout.rail.contains(&pos) {
            let frac = (pos.y - layout.rail.origin.y) / layout.rail.size.height;
            let row = (frac * layout.total_rows as f32) as usize;
            self.scroll_to_row(row, cx);
            return;
        }
        if !layout.bounds.contains(&pos) {
            return;
        }
        let row = (self.scroll_top + (pos.y - layout.bounds.origin.y) / layout.row_height) as usize;
        let fold = match self.rows(layout.mode) {
            Some(Rows::Unified(r)) => r.get(row).and_then(|r| match r {
                Row::Fold { start, .. } => Some(*start),
                _ => None,
            }),
            Some(Rows::Split(r)) => r.get(row).and_then(|r| match r {
                SplitRow::Fold { start, .. } => Some(*start),
                _ => None,
            }),
            None => None,
        };
        if let Some(start) = fold {
            self.clear_selection();
            self.expanded.insert(start);
            cx.notify();
        } else {
            self.begin_select(pos);
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.end_select(cx);
    }

    fn header(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let p = crate::terminal_view::TerminalView::palette(window, cx);
        let muted = hsla(mix(p.foreground, p.background, 0.45));
        let mut left = div().flex().flex_none().items_center().gap_2().whitespace_nowrap().child(div().font_weight(gpui::FontWeight::BOLD).child(self.title()));
        if self.sources.len() > 1 {
            left = left.child(div().text_color(muted).child(format!("{}/{}", self.index + 1, self.sources.len())));
        }
        if let Some(loaded) = &self.loaded {
            if let Some(diff) = loaded.preview.diff.as_ref().filter(|d| d.has_changes()) {
                let (a, r) = diff.stats();
                left = left.child(div().text_color(hsla(p.ansi[2])).child(format!("+{a}"))).child(div().text_color(hsla(p.ansi[1])).child(format!("−{r}")));
            }
            let base = match self.source() {
                Some(Source::Live(input)) => input.provider.label().to_string(),
                _ if loaded.preview.repo_root.is_some() => loaded.preview.base.label().to_string(),
                _ => DiffBase::None.label().to_string(),
            };
            left = left.child(div().px_2().rounded_md().bg(hsla(mix(p.background, p.foreground, 0.1))).child(base));
            if loaded.preview.is_new {
                left = left.child(div().text_color(hsla(p.ansi[2])).child("新文件"));
            }
            if loaded.preview.changed_since {
                let scope = match self.bases.get(self.base_ix) {
                    Some(DiffBase::Turn(r)) => r.scope,
                    _ => "这一轮",
                };
                left = left.child(div().text_color(muted).child(changed_since_note(scope)));
            }
        }
        if self.has_markdown() {
            let segment = |label: &'static str, on: bool| {
                let el = div().px_2().rounded_sm().child(label);
                if on { el.bg(hsla(mix(p.background, p.foreground, 0.2))) } else { el.text_color(muted) }
            };
            left = left.child(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded_md()
                    .bg(hsla(mix(p.background, p.foreground, 0.1)))
                    .child(segment("渲染", !self.show_source))
                    .child(segment("源码 diff", self.show_source)),
            );
        }
        let view = match (self.has_markdown(), self.show_source) {
            (false, _) => "U 视图",
            (true, false) => "S 源码",
            (true, true) => "S 渲染 · U 视图",
        };
        let back = if self.history.is_empty() { "" } else { " · ⌘[ 返回" };
        let rest = if self.pinned { "D 对比 · R 刷新 · E 编辑 · T 新标签 · Esc 回到终端" } else { "D 对比 · ⏎ 固定 · T 新标签 · E 编辑 · Esc" };
        let hints = if matches!(self.source(), Some(Source::Live(_))) { live_hint().to_string() } else { format!("n p 改动 · {view} · {rest}{back}") };
        let header = div()
            .id("preview-header")
            .relative()
            .flex()
            .flex_none()
            .justify_between()
            .items_center()
            .h(px(32.))
            .px_3()
            .gap_4()
            .text_size(px(12.))
            .bg(hsla(mix(p.background, p.foreground, 0.06)));
        // A pinned pane's header drags onto the tab bar to give the pane a tab of its own (`workspace/pane_drag.rs`).
        let header = match self.pane {
            Some(pane) => header
                .children(crate::debug_state::rects::recorder(crate::debug_state::rects::RectId::PaneHeader(pane)))
                .cursor(gpui::CursorStyle::OpenHand)
                .on_drag(crate::workspace::DraggedPane { pane, title: self.title() }, |d, _, _, cx| cx.new(|_| d.clone())),
            None => header,
        };
        header
            .child(left)
            // Hints give way first when the pane is narrow.
            .child(div().flex_shrink().min_w(px(0.)).overflow_hidden().whitespace_nowrap().text_color(muted).child(hints))
    }

    fn message(&self, text: String, window: &Window, cx: &Context<Self>) -> gpui::AnyElement {
        let p = crate::terminal_view::TerminalView::palette(window, cx);
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(hsla(mix(p.foreground, p.background, 0.4)))
            .child(text)
            .into_any_element()
    }
}

impl Render for PreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = crate::terminal_view::TerminalView::palette(window, cx);
        // Theme switched: re-highlight what is shown (a load still running picks it up).
        // Not a reload, which would show changes the stale banner is waiting to be asked for.
        let appearance = if crate::theme::current(cx).dark { Appearance::Dark } else { Appearance::Light };
        if appearance != self.appearance {
            self.appearance = appearance;
            self.highlight(cx);
        }
        let body = if self.sources.is_empty() {
            self.message("没有改动".into(), window, cx)
        } else if let Some(e) = &self.error {
            self.message(e.clone(), window, cx)
        } else if self.rendered() {
            self.render_markdown(window, cx)
        } else {
            match self.loaded.as_ref().map(|l| &l.preview.doc.content) {
                None => self.message("加载中…".into(), window, cx),
                Some(Content::Binary) => self.message(format!("二进制文件 · {} 字节", self.loaded.as_ref().unwrap().preview.doc.size), window, cx),
                Some(Content::TooLarge(size)) => self.message(format!("文件过大（{size} 字节），不预览 · ⌘⌥O 用外部编辑器打开"), window, cx),
                Some(Content::Text(_)) => div().flex_1().overflow_hidden().relative().child(PreviewElement::new(cx.entity())).children(crate::debug_state::rects::recorder(crate::debug_state::rects::RectId::PreviewCode(self.pane))).into_any_element(),
            }
        };
        let stale = self.stale.then(|| {
            div()
                .flex_none()
                .px_3()
                .py_1()
                .text_size(px(12.))
                .bg(hsla(mix(p.background, p.ansi[3], 0.25)))
                .child("文件已更新 · R 刷新")
        });
        let banner = self.banner.clone().map(|text| {
            div().flex_none().px_3().py_1().text_size(px(12.)).bg(hsla(mix(p.background, p.ansi[1], 0.25))).child(text)
        });
        div()
            .id("preview")
            .key_context("Preview")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(p.background))
            .text_color(hsla(p.foreground))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(|this, e, _, cx| this.drag_select(e, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::open_external))
            .on_drop(cx.listener(Self::drop_files))
            .child(self.header(window, cx))
            .children(stale)
            .children(banner)
            .child(body)
    }
}

/// The note on a file that changed after the diff's 「后」 snapshot (the session net's 「本会话」 reads 「在这之后」).
fn changed_since_note(scope: &str) -> String {
    match scope {
        "本会话" => "文件在这之后又被改动".into(),
        scope => format!("文件在{scope}之后又被改动"),
    }
}

/// The hint line of a live preview: it is read-only, so only the keys it honours.
fn live_hint() -> &'static str {
    "Esc 回到编辑器"
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_changed_since_note_names_the_scope() {
        use super::changed_since_note;
        assert_eq!(changed_since_note("这一轮"), "文件在这一轮之后又被改动");
        assert_eq!(changed_since_note("这个任务"), "文件在这个任务之后又被改动");
        assert_eq!(changed_since_note("本会话"), "文件在这之后又被改动");
    }

    #[test]
    fn a_live_preview_has_its_own_hint() {
        assert_eq!(super::live_hint(), "Esc 回到编辑器");
    }

    #[test]
    fn a_live_source_has_no_file_to_edit_or_watch() {
        use super::{live_title, Source};
        use crate::live_preview::{LiveInput, Provider};
        let src = Source::Live(LiveInput { provider: Provider::Rendered, path: Some("/tmp/a.md".into()), text: "x".into(), slot: 1 });
        assert_eq!(live_title(&src), Some("预览 · a.md".to_string()));
        let unsaved = Source::Live(LiveInput { provider: Provider::Changes, path: None, text: String::new(), slot: 1 });
        assert_eq!(live_title(&unsaved), Some("预览 · 未命名".to_string()));
        assert_eq!(live_title(&Source::File("/tmp/a.md".into())), None);
    }

    use gilvt_viewer::DiffBase;

    use super::{accepts_drop, base_key_applies, canonical_target, diff_bases, edit_key, edit_line, link_navigation_applies, source_toggle_applies, Source};

    #[test]
    fn a_live_source_ignores_quick_look_keys_drops_and_links() {
        use crate::live_preview::{LiveInput, Provider};
        let live = Source::Live(LiveInput { provider: Provider::Changes, path: None, text: String::new(), slot: 1 });
        let file = Source::File("/tmp/a".into());
        assert!(!base_key_applies(Some(&live)));
        assert!(base_key_applies(Some(&file)));
        assert!(base_key_applies(None));
        assert!(!accepts_drop(Some(&live)));
        assert!(accepts_drop(Some(&file)));
        assert!(accepts_drop(None));
        assert!(!source_toggle_applies(Some(&live)));
        assert!(source_toggle_applies(Some(&file)));
        assert!(!link_navigation_applies(Some(&live)));
        assert!(link_navigation_applies(Some(&file)));
    }

    #[test]
    fn e_and_cmd_o_edit_alt_e_flips_cmd_alt_o_is_left_to_the_action() {
        let m = |control, shift, platform, alt| gpui::Modifiers { control, shift, platform, alt, function: false };
        let none = m(false, false, false, false);
        let cases = [
            ("e", none, Some(false)),
            ("e", m(false, false, false, true), Some(true)),
            ("e", m(false, true, false, false), None),
            ("e", m(true, false, false, false), None),
            ("e", m(false, false, true, false), None),
            ("o", m(false, false, true, false), Some(false)),
            // ⌘⌥O: the OpenExternalEditor binding, not an edit.
            ("o", m(false, false, true, true), None),
            ("o", m(false, true, true, false), None),
            ("o", none, None),
            ("j", none, None),
        ];
        for (key, mods, want) in cases {
            assert_eq!(edit_key(key, mods), want, "{key} {mods:?}");
        }
    }

    #[test]
    fn edits_start_at_the_rendered_top_line_else_the_annotated_line() {
        assert_eq!(edit_line(Some(40), Some(7)), Some(40));
        assert_eq!(edit_line(None, Some(7)), Some(7));
        assert_eq!(edit_line(None, None), None);
    }

    #[test]
    fn a_revision_comes_first_then_head_then_no_diff() {
        assert_eq!(diff_bases(None, None), [DiffBase::Head, DiffBase::None]);
        assert_eq!(diff_bases(Some("main".into()), None),[DiffBase::Rev("main".into()), DiffBase::Head, DiffBase::None]);
    }

    #[test]
    fn watch_targets_resolve_symlinked_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, tmp.path().join("link")).unwrap();
        let want = real.canonicalize().unwrap().join("a.rs");
        // Resolved whether or not the file exists yet.
        assert_eq!(canonical_target(&tmp.path().join("link/a.rs")), Some(want.clone()));
        std::fs::write(real.join("a.rs"), "x").unwrap();
        assert_eq!(canonical_target(&tmp.path().join("link/a.rs")), Some(want.clone()));
        assert_eq!(canonical_target(&want), Some(want));
    }
}
