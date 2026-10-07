//! The inline 重命名… field of a sidebar row: ⏎ saves, Esc (or clicking elsewhere) cancels, an empty
//! name brings the automatic one back. Typing goes through the input handler (IME works).

use std::ops::Range;

use gpui::{
    canvas, div, prelude::*, px, Bounds, Context, ElementInputHandler, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, Pixels, Point, SharedString, Subscription, UTF16Selection, Window,
};

use crate::actions::{Paste, RENAME_CONTEXT};

/// Longest name kept (the row truncates what does not fit anyway).
pub const MAX_CHARS: usize = 80;

/// The field's text. It starts with the current name selected, so typing replaces it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameText {
    text: String,
    all_selected: bool,
}

impl RenameText {
    pub fn new(current: &str) -> Self {
        RenameText { text: current.to_string(), all_selected: !current.is_empty() }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn all_selected(&self) -> bool {
        self.all_selected
    }

    /// Typed or pasted text: its first line without control characters, up to [`MAX_CHARS`].
    pub fn insert(&mut self, typed: &str) {
        let clean: String = typed.lines().next().unwrap_or("").chars().filter(|c| !c.is_control()).collect();
        if self.all_selected {
            self.text.clear();
            self.all_selected = false;
        }
        let room = MAX_CHARS.saturating_sub(self.text.chars().count());
        self.text.extend(clean.chars().take(room));
    }

    pub fn backspace(&mut self) {
        if std::mem::take(&mut self.all_selected) {
            self.text.clear();
        } else {
            self.text.pop();
        }
    }

    /// ← / → / End: the caret goes to the end, keeping the text.
    pub fn deselect(&mut self) {
        self.all_selected = false;
    }

    /// What is saved: trimmed; "" means "back to the automatic name".
    pub fn value(&self) -> String {
        self.text.trim().to_string()
    }
}

pub enum RenameEvent {
    Commit(String),
    /// Esc inside the field: the user explicitly wants out, back to the terminal.
    Cancel,
    /// The field lost focus without an explicit Esc — e.g. a right-click opened the row menu, or
    /// another row started renaming. Something else now owns the keyboard on purpose, so closing
    /// the field here must not steal it back.
    Blurred,
}

impl RenameEvent {
    /// Whether closing the field for this event should hand focus back to the terminal pane.
    /// Only an explicit Commit or an explicit Esc-cancel do; a blur must never move focus, or
    /// whatever the blur was caused by (e.g. the row menu) loses the keyboard it just took.
    pub fn refocus_terminal(&self) -> bool {
        !matches!(self, RenameEvent::Blurred)
    }
}

pub struct RenameField {
    focus_handle: FocusHandle,
    text: RenameText,
    /// IME composition, shown after the text.
    marked: Option<String>,
    /// The grey hint while the field is empty.
    placeholder: SharedString,
    _blur: Subscription,
}

impl EventEmitter<RenameEvent> for RenameField {}

impl Focusable for RenameField {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl RenameField {
    pub fn new(current: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_placeholder(
            current,
            crate::i18n::text("留空 = 自动名称", "Leave empty for automatic name"),
            window,
            cx,
        )
    }

    /// The same field with another grey hint while it is empty (the settings window's model and path boxes).
    pub fn with_placeholder(current: &str, placeholder: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let blur = cx.on_blur(&focus_handle, window, |_, _, cx| cx.emit(RenameEvent::Blurred));
        RenameField { focus_handle, text: RenameText::new(current), marked: None, placeholder: placeholder.to_string().into(), _blur: blur }
    }

    /// What is typed so far (untrimmed).
    pub fn text(&self) -> &str {
        self.text.text()
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return; // IME composition owns the keyboard.
        }
        let m = event.keystroke.modifiers;
        if m.platform || m.control {
            return; // ⌘V and the app's shortcuts.
        }
        match event.keystroke.key.as_str() {
            "enter" => cx.emit(RenameEvent::Commit(self.text.value())),
            "escape" => cx.emit(RenameEvent::Cancel),
            "backspace" => self.text.backspace(),
            "left" | "right" | "end" | "home" | "up" | "down" => self.text.deselect(),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.text.insert(&text);
            cx.notify();
        }
    }
}

impl Render for RenameField {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = crate::theme::current(cx);
        let (ui, h) = (&theme.ui, crate::theme::hsla);
        let (accent, selection, placeholder, bg) = (h(ui.accent), h(ui.selected), h(ui.text_4), h(ui.raised));
        let focus = self.focus_handle.clone();
        let entity = cx.entity();
        let caret = div().flex_none().w(px(1.5)).h(px(13.)).bg(accent);
        let row = div()
            .id("rename-field")
            .key_context(RENAME_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::paste))
            .relative()
            .flex()
            .items_center()
            .w_full()
            .h(px(18.))
            .px(px(3.))
            .rounded(px(3.))
            .border_1()
            .border_color(accent)
            .bg(bg)
            .overflow_hidden()
            .whitespace_nowrap()
            // IME: typed text reaches `replace_text_in_range` through this input handler.
            .child(
                canvas(|_, _, _| {}, move |bounds, _, window, cx| {
                    window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                })
                .absolute()
                .size_full(),
            );
        let text = self.text.text().to_string();
        if text.is_empty() && self.marked.is_none() {
            row.child(caret).child(div().ml(px(2.)).text_color(placeholder).child(self.placeholder.clone()))
        } else if self.text.all_selected() {
            row.child(div().bg(selection).child(text))
        } else {
            row.child(text).children(self.marked.clone().map(|m| div().underline().child(m))).child(caret)
        }
    }
}

impl gpui::EntityInputHandler for RenameField {
    fn text_for_range(&mut self, _: Range<usize>, _: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        None
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let len = self.marked.as_ref().map_or(0, |t| t.encode_utf16().count());
        Some(UTF16Selection { range: len..len, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|t| 0..t.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        self.text.insert(text);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        if !text.is_empty() && self.text.all_selected() {
            // Composing replaces the selected name like typing does.
            self.text.backspace();
        }
        self.marked = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    fn bounds_for_range(&mut self, _: Range<usize>, element: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        Some(element)
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_replaces_the_selected_name() {
        let mut t = RenameText::new("实现用户列表分页");
        assert!(t.all_selected());
        t.insert("修");
        assert_eq!((t.text(), t.all_selected()), ("修", false));
        t.insert("复 bug");
        assert_eq!(t.value(), "修复 bug");
        t.backspace();
        assert_eq!(t.text(), "修复 bu");
    }

    #[test]
    fn backspace_on_the_selection_clears_it() {
        let mut t = RenameText::new("old");
        t.backspace();
        assert_eq!((t.text(), t.value().as_str()), ("", ""), "empty = automatic name");
        let mut t = RenameText::new("old");
        t.deselect();
        t.insert(" name");
        assert_eq!(t.text(), "old name");
        let mut empty = RenameText::new("");
        assert!(!empty.all_selected());
        empty.backspace();
        assert_eq!(empty.text(), "");
    }

    #[test]
    fn pasted_text_is_one_clean_line() {
        let mut t = RenameText::new("");
        t.insert("  first\tline  \nsecond");
        assert_eq!(t.text(), "  firstline  ");
        assert_eq!(t.value(), "firstline", "saved trimmed");
        t.insert("\r\n");
        assert_eq!(t.text(), "  firstline  ");
    }

    #[test]
    fn only_blur_skips_refocus() {
        assert!(RenameEvent::Commit("x".into()).refocus_terminal());
        assert!(RenameEvent::Cancel.refocus_terminal());
        assert!(!RenameEvent::Blurred.refocus_terminal());
    }

    #[test]
    fn names_are_capped() {
        let mut t = RenameText::new("");
        t.insert(&"名".repeat(MAX_CHARS + 5));
        assert_eq!(t.text().chars().count(), MAX_CHARS);
        t.insert("x");
        assert_eq!(t.text().chars().count(), MAX_CHARS);
    }
}
