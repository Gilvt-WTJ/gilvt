//! `PaneView::Editor` wiring (E2a §4): opening a file in the built-in editor (focus an existing editor on it,
//! else split or open a file tab per `editor::route`), the window-level refusal banner (§6 打开时), and the
//! pure decisions behind them.

use std::path::{Path, PathBuf};

use gilvt_editor::{EditorError, OpenOptions};
use gpui::{div, prelude::*, px, App, Context, Entity, Window};

use super::{split_target, to_rect, PaneView, Tab, Workspace};
use crate::editor::route::{self, Placement};
use crate::editor::popup::MenuKind;
use crate::editor::view::{load_model_with, EditorEvent, EditorView};
use crate::pane_tree::{next_pane_id, Axis, PaneId, PaneTree};
use crate::preview_view::{open_in_editor, OpenRequest};
use crate::theme::{hsla, mix, CellMetrics};

/// The tab and pane (by id) that already edit `path`, comparing canonical paths.
pub(super) fn find_open(panes: &[(usize, PaneId, Option<PathBuf>)], path: &Path) -> Option<(usize, PaneId)> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let want = canon(path);
    panes.iter().find(|(_, _, p)| p.as_deref().is_some_and(|p| canon(p) == want)).map(|&(ti, id, _)| (ti, id))
}

/// Tab to activate after tab `closed` is removed: the tab it was opened from if it still exists, else its left neighbour.
/// `tab_ids` are the ids of the tabs left (non-empty).
pub(super) fn tab_after_close(tab_ids: &[u64], closed_pos: usize, origin: Option<u64>) -> usize {
    if let Some(i) = origin.and_then(|o| tab_ids.iter().position(|&id| id == o)) {
        return i;
    }
    closed_pos.saturating_sub(1).min(tab_ids.len().saturating_sub(1))
}

/// What the tab bar (and `debug state`) shows for a tab: the focused pane's title, with a leading ● when an
/// editor elsewhere in the tab has unsaved changes. `focused_marks_dirty`: the focused pane's title already
/// carries the ● (it is a dirty editor); `panes_dirty`: per pane of the tab, an editor with unsaved changes.
pub(super) fn tab_label(focused_title: String, focused_marks_dirty: bool, panes_dirty: &[bool]) -> String {
    if !focused_marks_dirty && panes_dirty.iter().any(|&d| d) {
        format!("● {focused_title}")
    } else {
        focused_title
    }
}

/// Where a newly opened editor pane goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Insert {
    /// Split `target` (in tab `tab`) with the editor on its right.
    Split { tab: usize, target: PaneId },
    /// A new file tab at index `at`, returning to tab id `origin_tab` when it closes.
    NewTab { at: usize, origin_tab: Option<u64> },
}

/// `target`: the tab index, pane and tab id `split_target` chose (None: the window has no tab).
pub(super) fn insert_for(target: Option<(usize, PaneId, u64)>, placement: Placement) -> Insert {
    match (target, placement) {
        (None, _) => Insert::NewTab { at: 0, origin_tab: None },
        (Some((tab, target, _)), Placement::Split) => Insert::Split { tab, target },
        (Some((tab, _, id)), Placement::NewTab) => Insert::NewTab { at: tab + 1, origin_tab: Some(id) },
    }
}

/// How ⌘W closes a pane: an editor decides itself (dirty: its in-pane close bar; clean: it asks to be closed);
/// any other pane goes through the Workspace's close confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CloseRoute {
    EditorRequestClose,
    Workspace,
}

pub(super) fn close_route(is_editor: bool) -> CloseRoute {
    if is_editor { CloseRoute::EditorRequestClose } else { CloseRoute::Workspace }
}

/// Which buttons the refusal banner offers besides 「知道了」.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RefusalButtons {
    /// 「在外部编辑器中打开」: not for binary or oversized files (§6), nor for a missing file (it would create one).
    pub external: bool,
    /// 「预览」: not for a missing file.
    pub preview: bool,
    /// 「选择编码打开…」 and 「以只读方式打开」: only for a legacy encoding that does not round-trip (E2b-1 §3).
    pub reopen: bool,
}

fn is_not_found(e: &EditorError) -> bool {
    matches!(e, EditorError::Io(io) if io.kind() == std::io::ErrorKind::NotFound)
}

/// The banner's reason text and buttons for a file `load_model` refused.
pub(super) fn refusal(e: &EditorError) -> (String, RefusalButtons) {
    let missing = is_not_found(e);
    let reason = if missing { "文件不存在".to_string() } else { e.to_string() };
    let external = !missing && !matches!(e, EditorError::Binary | EditorError::TooLarge { .. });
    let reopen = matches!(e, EditorError::UnsupportedEncoding);
    (reason, RefusalButtons { external, preview: !missing, reopen })
}

/// A file the editor refused to open: the window-level red banner below the tab bar (§3, §6).
pub(super) struct EditorRefusal {
    pub path: PathBuf,
    /// The reason (`EditorError`'s Display, or 「文件不存在」).
    pub message: String,
    pub buttons: RefusalButtons,
    /// What 「预览」 (and the reopen buttons) open with.
    pub line: Option<u32>,
    pub origin: Option<PaneId>,
    /// The reopen buttons (which take this refusal) place the pane like the refused open would have.
    pub flip: bool,
}

impl Workspace {
    /// ⌘W on the focused pane `id`.
    pub(super) fn close_focused_pane(&mut self, id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let editor = match self.panes.get(&id) {
            Some(PaneView::Editor(v)) => Some(v.clone()),
            _ => None,
        };
        match (close_route(editor.is_some()), editor) {
            (CloseRoute::EditorRequestClose, Some(v)) => v.update(cx, |v, cx| v.request_close(cx)),
            _ => self.request_close(super::CloseAction::Pane(id), window, cx),
        }
    }

    /// Opens `path` in the built-in editor: focuses an existing editor on it, else splits or opens a tab per `route`.
    /// A refused file shows the red banner and creates no pane.
    pub fn open_editor(&mut self, path: PathBuf, line: Option<u32>, origin: Option<PaneId>, flip: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open_editor_with(path, line, origin, flip, OpenOptions::default(), window, cx);
    }

    /// [`Self::open_editor`] with an encoding / read-only choice; returns the editor showing `path` (`None` when
    /// it was refused). An editor already open on `path` is focused as it is (`opts` do not apply to it).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn open_editor_with(
        &mut self,
        path: PathBuf,
        line: Option<u32>,
        origin: Option<PaneId>,
        flip: bool,
        opts: OpenOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<EditorView>> {
        self.ended_confirm = None;
        self.finder = None;
        self.sessions = None;
        self.new_agent = None;
        self.editor_refusal = None;
        // 1. Already open in this window: focus it (switching tab and flashing; `focus_pane` also closes Quick Look).
        if let Some((_, id)) = find_open(&self.open_editors(cx), &path) {
            let view = match self.panes.get(&id) {
                Some(PaneView::Editor(v)) => Some(v.clone()),
                _ => None,
            };
            if let (Some(v), Some(line)) = (&view, line) {
                v.update(cx, |v, cx| v.goto_line(line as usize, cx));
            }
            self.focus_pane(id, window, cx);
            return view;
        }
        // 2. Load; a refusal shows the banner and nothing else.
        let model = match load_model_with(&path, opts) {
            Ok(m) => m,
            Err(e) => {
                let (message, buttons) = refusal(&e);
                self.editor_refusal = Some(EditorRefusal { path, message, buttons, line, origin, flip });
                // Quick Look stays open (the user keeps looking at the file); a closed palette's keyboard goes back.
                self.focus_active(window, cx);
                return None;
            }
        };
        // An editor opened from a preview replaces it.
        self.quicklook = None;
        let view = cx.new(|cx| EditorView::new(model, cx));
        view.update(cx, |v, cx| v.attach(window, cx));
        if let Some(line) = line {
            view.update(cx, |v, cx| v.goto_line(line as usize, cx));
        }
        // 3. Place it.
        let target = split_target(&self.tabs, self.active, origin).map(|(ti, p)| (ti, p, self.tabs[ti].id));
        let placement = match target {
            Some((ti, pane, _)) => route::route(self.tabs[ti].tree.panes().len(), self.target_cols(ti, pane, window, cx), flip),
            None => Placement::NewTab,
        };
        let id = next_pane_id();
        view.update(cx, |v, _| v.pane = Some(id));
        let sub = cx.subscribe_in(&view, window, move |ws, _, event, window, cx| match event {
            EditorEvent::Changed => {
                ws.sync_title(window, cx);
                cx.notify();
            }
            EditorEvent::CloseRequested => ws.close_pane(id, window, cx),
            EditorEvent::Leave => ws.leave_preview(id, window, cx),
            EditorEvent::TextChanged => ws.schedule_live_refresh(id, false, cx),
            EditorEvent::Saved => ws.schedule_live_refresh(id, true, cx),
            EditorEvent::Scrolled(line) => ws.sync_live_scroll(id, *line, cx),
            EditorEvent::TogglePreview => ws.toggle_live_preview(id, window, cx),
            EditorEvent::CyclePreview => ws.cycle_live_preview(id, cx),
        });
        self.subscriptions.insert(id, sub);
        self.panes.insert(id, PaneView::Editor(view.clone()));
        match insert_for(target, placement) {
            // The keyboard stays with the pane it splits, as with a pinned preview.
            Insert::Split { tab, target } => {
                self.active = tab;
                let t = &mut self.tabs[tab];
                t.tree.split(target, id, Axis::Row);
                t.focused = target;
                t.zoomed = false;
            }
            Insert::NewTab { at, origin_tab } => {
                let mut t = Tab::new(PaneTree::new(id), id);
                t.origin_tab = origin_tab;
                self.tabs.insert(at, t);
                self.active = at;
            }
        }
        self.focus_active(window, cx);
        Some(view)
    }

    /// The refusal banner's 「以只读方式打开」 / 「选择编码打开…」: the file with the detected encoding, read-only
    /// (lossy where it does not decode faithfully); `pick` then opens the encoding menu on the new pane, with
    /// the keyboard there so Esc reaches it. A picked encoding that round-trips makes it editable.
    fn reopen_refused(&mut self, pick: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.editor_refusal.take() else { return };
        let opts = OpenOptions { encoding: None, read_only: true };
        let Some(view) = self.open_editor_with(r.path, r.line, r.origin, r.flip, opts, window, cx) else { return };
        if pick {
            if let Some(id) = view.read(cx).pane {
                self.focus_pane(id, window, cx);
            }
            view.update(cx, |v, cx| v.open_menu(MenuKind::Encoding, cx));
        }
    }

    /// Columns of pane `pane` in tab `ti` as last laid out (0 before the first frame).
    fn target_cols(&self, ti: usize, pane: PaneId, window: &Window, cx: &App) -> usize {
        let Some(bounds) = self.content_bounds.get() else { return 0 };
        let Some((_, rect)) = self.tabs[ti].tree.layout(to_rect(bounds)).into_iter().find(|(id, _)| *id == pane) else { return 0 };
        let cell = CellMetrics::measure(window, &Self::settings(cx)).cell_width / px(1.);
        route::estimate_cols(rect.w, cell, 1)
    }

    /// The tab bar's label for `tab` (see `tab_label`): a dirty editor shows its ● even when another pane has focus.
    pub(super) fn tab_title(&self, tab: &Tab, cx: &App) -> String {
        let dirty = |id: &PaneId| matches!(self.panes.get(id), Some(PaneView::Editor(v)) if v.read(cx).is_dirty());
        let title = self.panes.get(&tab.focused).map(|p| p.title(cx)).unwrap_or_default();
        let panes_dirty: Vec<bool> = tab.tree.panes().iter().map(dirty).collect();
        tab_label(title, dirty(&tab.focused), &panes_dirty)
    }

    /// Every editor pane of this window: (tab index, pane, file).
    fn open_editors(&self, cx: &App) -> Vec<(usize, PaneId, Option<PathBuf>)> {
        let mut out = Vec::new();
        for (ti, tab) in self.tabs.iter().enumerate() {
            for id in tab.tree.panes() {
                if let Some(PaneView::Editor(v)) = self.panes.get(&id) {
                    out.push((ti, id, v.read(cx).path().map(Path::to_path_buf)));
                }
            }
        }
        out
    }

    /// Editor panes with unsaved changes, in tab order (the close / quit protection, `workspace/close.rs`).
    pub fn dirty_editors(&self, cx: &App) -> Vec<(PaneId, PathBuf)> {
        self.open_editors(cx)
            .into_iter()
            .filter(|(_, id, _)| matches!(self.panes.get(id), Some(PaneView::Editor(v)) if v.read(cx).is_dirty()))
            .map(|(_, id, path)| (id, path.unwrap_or_default()))
            .collect()
    }

    /// The red 「不能编辑 X：原因」 banner below the tab bar.
    pub(super) fn render_editor_refusal(&self, r: &EditorRefusal, p: &gilvt_term::Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let name = r.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| r.path.display().to_string());
        let button = |id: &'static str, label: &'static str| {
            div().id(id).flex_none().px_2().rounded(px(4.)).border_1().border_color(hsla(mix(p.background, p.foreground, 0.4))).child(label)
        };
        let (path, line, origin) = (r.path.clone(), r.line, r.origin);
        let preview_path = path.clone();
        div()
            .id("editor-refusal")
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .px_3()
            .py_1()
            .text_size(px(12.))
            .bg(hsla(mix(p.background, p.ansi[1], 0.25)))
            .child(div().flex_1().min_w(px(0.)).truncate().child(format!("不能编辑 {name}：{}", r.message)))
            .children(r.buttons.external.then(|| {
                button("editor-refusal-external", "在外部编辑器中打开").on_click(cx.listener(move |ws, _, _, cx| {
                    open_in_editor(&path, line);
                    ws.editor_refusal = None;
                    cx.notify();
                }))
            }))
            .children(r.buttons.reopen.then(|| {
                button("editor-refusal-pick-encoding", "选择编码打开…").on_click(cx.listener(|ws, _, window, cx| ws.reopen_refused(true, window, cx)))
            }))
            .children(r.buttons.reopen.then(|| {
                button("editor-refusal-read-only", "以只读方式打开").on_click(cx.listener(|ws, _, window, cx| ws.reopen_refused(false, window, cx)))
            }))
            .children(r.buttons.preview.then(|| {
                button("editor-refusal-preview", "预览").on_click(cx.listener(move |ws, _, window, cx| {
                    ws.editor_refusal = None;
                    ws.open_preview(OpenRequest::file(preview_path.clone(), line, None), false, origin, window, cx);
                }))
            }))
            .child(button("editor-refusal-dismiss", "知道了").on_click(cx.listener(|ws, _, _, cx| {
                ws.editor_refusal = None;
                cx.notify();
            })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_routes_by_pane_kind() {
        assert_eq!(close_route(true), CloseRoute::EditorRequestClose);
        assert_eq!(close_route(false), CloseRoute::Workspace);
    }

    #[test]
    fn finds_an_existing_editor_by_canonical_path() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.md");
        std::fs::write(&p, "x").unwrap();
        let alias = dir.path().join(".").join("a.md");
        let panes = vec![(0, 5, Some(p.clone())), (1, 9, None)];
        assert_eq!(find_open(&panes, &alias), Some((0, 5)));
        assert_eq!(find_open(&panes, &dir.path().join("b.md")), None);
    }

    #[test]
    fn closing_a_file_tab_returns_to_where_it_was_opened_from() {
        // tabs [10, 11, 12(file, from 10), 13] after removing index 2 -> ids [10, 11, 13]
        assert_eq!(tab_after_close(&[10, 11, 13], 2, Some(10)), 0);
        assert_eq!(tab_after_close(&[10, 11, 13], 2, Some(99)), 1); // origin gone: left neighbour
        assert_eq!(tab_after_close(&[10], 0, None), 0);
    }

    #[test]
    fn placement_maps_to_a_split_or_a_tab_right_of_the_origin() {
        assert_eq!(insert_for(Some((1, 7, 42)), Placement::Split), Insert::Split { tab: 1, target: 7 });
        assert_eq!(insert_for(Some((1, 7, 42)), Placement::NewTab), Insert::NewTab { at: 2, origin_tab: Some(42) });
        assert_eq!(insert_for(None, Placement::Split), Insert::NewTab { at: 0, origin_tab: None });
    }

    #[test]
    fn a_dirty_editor_marks_its_tab_whichever_pane_has_focus() {
        // Split tab, focus on the terminal, the editor beside it is dirty.
        assert_eq!(tab_label("zsh".into(), false, &[false, true]), "● zsh");
        // The focused editor's own title already carries the ●: not doubled.
        assert_eq!(tab_label("● a.md".into(), true, &[false, true]), "● a.md");
        // Nothing dirty, or terminals only.
        assert_eq!(tab_label("a.md".into(), false, &[false, false]), "a.md");
        assert_eq!(tab_label("zsh".into(), false, &[false]), "zsh");
    }

    #[test]
    fn refusal_buttons_follow_the_error() {
        let both = RefusalButtons { external: true, preview: true, reopen: false };
        let preview_only = RefusalButtons { external: false, preview: true, reopen: false };
        assert_eq!(refusal(&EditorError::Binary).1, preview_only);
        assert_eq!(refusal(&EditorError::TooLarge { size: 2, limit: 1 }).1, preview_only);
        // Only a legacy encoding that does not round-trip can be reopened (read-only, or with another encoding).
        assert_eq!(refusal(&EditorError::UnsupportedEncoding).1, RefusalButtons { reopen: true, ..both });
        assert_eq!(refusal(&EditorError::NotAFile).1, both);
        assert_eq!(refusal(&EditorError::PermissionDenied).1, both);
        assert!(!refusal(&EditorError::Io(std::io::Error::other("x"))).1.reopen);
        let (msg, b) = refusal(&EditorError::Io(std::io::Error::from(std::io::ErrorKind::NotFound)));
        assert_eq!(msg, "文件不存在");
        assert_eq!(b, RefusalButtons { external: false, preview: false, reopen: false });
        assert_eq!(refusal(&EditorError::Binary).0, EditorError::Binary.to_string());
    }
}
