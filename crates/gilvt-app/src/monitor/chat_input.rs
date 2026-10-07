//! The chat panel's input, also the bottom command bar's (S2 §6.3–6.4): ⏎ sends, ⇧⏎ breaks the line, `@` picks
//! sessions as chips (↑ ↓ ⏎, Esc), Backspace on empty text removes the last chip. The caret is always at the end
//! (like the sidebar's rename field); typing goes through the input handler, so IMEs work. [`Draft`] is the pure
//! part.

use std::ops::Range;

use gpui::{
    canvas, div, prelude::*, px, Bounds, Context, ElementInputHandler, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Pixels, Point,
    ScrollHandle, UTF16Selection, Window,
};

use super::chat_model::Outgoing;
use crate::actions::{Paste, CHAT_INPUT_CONTEXT};
use crate::debug_state::rects::{self, RectId};

pub const MAX_CHARS: usize = 4000;
pub const MAX_CANDIDATES: usize = 8;
pub const PLACEHOLDER: &str = "继续问…（@ 选会话 · ⏎ 发送 · ⇧⏎ 换行）";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub key: String,
    /// 「web-login · web · 右」: name and location, as the picker lists it.
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picker {
    pub query: String,
    pub selected: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    text: String,
    chips: Vec<(String, String)>,
    picker: Option<Picker>,
}

impl Draft {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn chips(&self) -> &[(String, String)] {
        &self.chips
    }

    pub fn picker(&self) -> Option<&Picker> {
        self.picker.as_ref()
    }

    /// Typed or pasted text. `@` at the start or after whitespace opens the picker; while it is open, text goes into
    /// its query. Control characters other than newline and tab are dropped; `\r\n` becomes `\n`.
    pub fn insert(&mut self, typed: &str) {
        for ch in typed.replace("\r\n", "\n").chars() {
            if let Some(p) = &mut self.picker {
                if ch.is_whitespace() {
                    // A stray @: it was plain text after all.
                    self.dismiss_picker();
                } else {
                    if !ch.is_control() && p.query.chars().count() < MAX_CHARS {
                        p.query.push(ch);
                        p.selected = 0;
                    }
                    continue;
                }
            }
            if (ch == '@' || ch == '＠') && self.text.chars().last().is_none_or(opens_picker_after) {
                self.picker = Some(Picker::default());
                continue;
            }
            if (ch.is_control() && ch != '\n' && ch != '\t') || self.text.chars().count() >= MAX_CHARS {
                continue;
            }
            self.text.push(ch);
        }
    }

    /// Closes the picker, keeping what was typed: `@` and the query go back into the text.
    pub fn dismiss_picker(&mut self) {
        if let Some(p) = self.picker.take() {
            let room = MAX_CHARS.saturating_sub(self.text.chars().count());
            self.text.extend(std::iter::once('@').chain(p.query.chars()).take(room));
        }
    }

    /// Esc in the picker: with candidates it just closes; with none the typed text is restored.
    pub fn cancel_picker(&mut self, all: &[Candidate]) {
        if self.matches(all).is_empty() {
            self.dismiss_picker();
        } else {
            self.picker = None;
        }
    }

    pub fn newline(&mut self) {
        self.insert("\n");
    }

    pub fn backspace(&mut self) {
        if let Some(p) = &mut self.picker {
            if p.query.pop().is_none() {
                self.picker = None;
            }
            return;
        }
        if self.text.pop().is_none() {
            self.chips.pop();
        }
    }

    pub fn add_chip(&mut self, key: String, label: String) {
        if !self.chips.iter().any(|(k, _)| *k == key) {
            self.chips.push((key, label));
        }
    }

    pub fn remove_chip(&mut self, i: usize) {
        if i < self.chips.len() {
            self.chips.remove(i);
        }
    }

    /// The picker's candidates: not chosen yet, matching the query (label or key, case-insensitive), in wall order.
    pub fn matches(&self, all: &[Candidate]) -> Vec<Candidate> {
        let Some(p) = &self.picker else { return Vec::new() };
        let q = p.query.to_lowercase();
        all.iter()
            .filter(|c| !self.chips.iter().any(|(k, _)| *k == c.key))
            .filter(|c| q.is_empty() || c.label.to_lowercase().contains(&q) || c.key.to_lowercase().contains(&q))
            .take(MAX_CANDIDATES)
            .cloned()
            .collect()
    }

    pub fn move_selection(&mut self, delta: i32, n: usize) {
        if let Some(p) = &mut self.picker {
            if n > 0 {
                p.selected = (p.selected as i64 + delta as i64).rem_euclid(n as i64) as usize;
            }
        }
    }

    /// ⏎ in the picker: the selected candidate becomes a chip. False when nothing matches (the picker closes and
    /// `@` + query become text).
    pub fn choose(&mut self, all: &[Candidate]) -> bool {
        let m = self.matches(all);
        let Some(c) = self.picker.as_ref().and_then(|p| m.get(p.selected.min(m.len().saturating_sub(1)))).cloned() else {
            self.dismiss_picker(); // nothing to choose: the typed text stays
            return false;
        };
        self.picker = None;
        self.add_chip(c.key, c.label);
        true
    }

    /// ⏎: the message to send (trimmed text, the chips); the text is cleared, the chips stay for follow-ups.
    pub fn take(&mut self) -> Option<Outgoing> {
        let text = self.text.trim().to_string();
        if text.is_empty() {
            return None;
        }
        self.text.clear();
        Some(Outgoing { text, chips: self.chips.clone() })
    }
}

/// Whether `@` right after `prev` starts a mention: after whitespace, CJK text or full-width punctuation
/// (an e-mail address like `a@b` is just text).
fn opens_picker_after(prev: char) -> bool {
    prev.is_whitespace()
        || matches!(prev as u32, 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xFF00..=0xFFEF | 0x2014 | 0x2018..=0x201D | 0x2026)
}

/// Where an input is drawn: the chat panel or the bottom command bar. They differ only in the placeholder and the
/// `RectId`s they record (both can be on screen in one window).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputPlace {
    Panel,
    CommandBar,
}

impl InputPlace {
    pub fn placeholder(self) -> &'static str {
        match self {
            InputPlace::Panel => crate::i18n::text(
                PLACEHOLDER,
                "Ask another question… (@ choose sessions · ⏎ send · ⇧⏎ newline)",
            ),
            InputPlace::CommandBar => crate::i18n::text(
                super::command_bar::PLACEHOLDER,
                "Ask Monitor… (@ choose sessions · ⏎ send · Esc collapse)",
            ),
        }
    }

    pub fn input_rect(self) -> RectId {
        match self {
            InputPlace::Panel => RectId::ChatInput,
            InputPlace::CommandBar => RectId::CommandBarInput,
        }
    }

    pub fn chip_remove_rect(self, i: usize) -> RectId {
        match self {
            InputPlace::Panel => RectId::ChatChipRemove(i),
            InputPlace::CommandBar => RectId::CommandBarChipRemove(i),
        }
    }

    pub fn element_id(self) -> &'static str {
        match self {
            InputPlace::Panel => "chat-input",
            InputPlace::CommandBar => "command-bar-input",
        }
    }
}

/// Tells the OS input method to drop its composition (gpui never sends `discardMarkedText`). Clearing our own
/// marked text is not enough: the IME keeps composing, and the next keystroke would commit the pinyin into whatever
/// input handler has the keyboard then (a terminal's PTY). A callback it makes into gpui meanwhile is dropped
/// (gpui's async window update does not re-enter a busy app).
pub fn discard_os_composition() {
    let Some(mtm) = objc2::MainThreadMarker::new() else { return };
    if let Some(ctx) = objc2_app_kit::NSTextInputContext::currentInputContext(mtm) {
        ctx.discardMarkedText();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EnterKey {
    Send,
    Newline,
}

/// Only plain ⏎ sends and ⇧⏎ breaks the line; ⌥⏎ and fn-⏎ are not ours.
fn classify_enter(m: &gpui::Modifiers) -> Option<EnterKey> {
    if m.alt || m.function {
        None
    } else if m.shift {
        Some(EnterKey::Newline)
    } else {
        Some(EnterKey::Send)
    }
}

pub enum ChatInputEvent {
    Send(Outgoing),
    /// Esc with the picker closed: back to the card wall.
    Escape,
}

pub struct ChatInput {
    focus_handle: FocusHandle,
    draft: Draft,
    candidates: Vec<Candidate>,
    /// IME composition, shown after the text.
    marked: Option<String>,
    place: InputPlace,
    /// The `@` picker's list (drawn by `chat_view::picker_list`, which caps its height).
    picker_scroll: ScrollHandle,
}

impl EventEmitter<ChatInputEvent> for ChatInput {}

impl Focusable for ChatInput {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ChatInput {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        ChatInput {
            focus_handle: cx.focus_handle(),
            draft: Draft::default(),
            candidates: Vec::new(),
            marked: None,
            place: InputPlace::Panel,
            picker_scroll: ScrollHandle::new(),
        }
    }

    /// The bottom command bar's input (S2 §6.4).
    pub fn for_command_bar(window: &mut Window, cx: &mut Context<Self>) -> Self {
        ChatInput { place: InputPlace::CommandBar, ..ChatInput::new(window, cx) }
    }

    /// Drops an IME composition that was not committed (the command bar closed under it): ours, and the OS input
    /// method's, so it cannot commit into a hidden field or the pane that gets the keyboard next.
    pub fn cancel_composition(&mut self, cx: &mut Context<Self>) {
        if self.marked.take().is_some() {
            discard_os_composition();
            cx.notify();
        }
    }

    /// Closes the `@` picker, keeping what was typed (`@` and the query go back into the text).
    pub fn close_picker(&mut self, cx: &mut Context<Self>) {
        if self.draft.picker().is_some() {
            self.draft.dismiss_picker();
            cx.notify();
        }
    }

    pub fn draft(&self) -> &Draft {
        &self.draft
    }

    /// The `@` candidates in wall order (the workspace sets them every frame the wall is built).
    pub fn set_candidates(&mut self, candidates: Vec<Candidate>) {
        self.candidates = candidates;
    }

    pub fn matches(&self) -> Vec<Candidate> {
        self.draft.matches(&self.candidates)
    }

    pub fn add_chip(&mut self, key: String, label: String, cx: &mut Context<Self>) {
        self.draft.add_chip(key, label);
        cx.notify();
    }

    pub fn remove_chip(&mut self, i: usize, cx: &mut Context<Self>) {
        self.draft.remove_chip(i);
        cx.notify();
    }

    /// A click on the picker's row `i`.
    pub fn choose_index(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.draft.picker {
            p.selected = i;
        }
        self.draft.choose(&self.candidates);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return; // IME composition owns the keyboard.
        }
        let m = event.keystroke.modifiers;
        if m.platform || m.control {
            return;
        }
        let picking = self.draft.picker.is_some();
        match event.keystroke.key.as_str() {
            "enter" => match classify_enter(&m) {
                None => return,
                Some(_) if picking => {
                    self.draft.choose(&self.candidates);
                }
                Some(EnterKey::Newline) => self.draft.newline(),
                Some(EnterKey::Send) => {
                    if let Some(out) = self.draft.take() {
                        cx.emit(ChatInputEvent::Send(out));
                    }
                }
            },
            "escape" if picking => self.draft.cancel_picker(&self.candidates),
            "escape" => cx.emit(ChatInputEvent::Escape),
            "up" if picking => {
                let n = self.matches().len();
                self.draft.move_selection(-1, n);
            }
            "down" if picking => {
                let n = self.matches().len();
                self.draft.move_selection(1, n);
            }
            "backspace" => self.draft.backspace(),
            _ => return,
        }
        self.reveal_selection();
        cx.stop_propagation();
        cx.notify();
    }

    /// The `@` picker's list scrolls by itself (its height is capped): ↑ ↓ and typing keep the highlighted row in
    /// view.
    pub fn picker_scroll(&self) -> &ScrollHandle {
        &self.picker_scroll
    }

    fn reveal_selection(&self) {
        if let Some(i) = self.draft.picker().and_then(|p| super::chat_view::picker_highlight(p.selected, self.matches().len())) {
            self.picker_scroll.scroll_to_item(i);
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.draft.insert(&text);
            cx.notify();
        }
    }
}

impl Render for ChatInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The ✦ purple and its tints match the card wall's (`monitor/view.rs` Colors).
        let theme = crate::theme::current(cx);
        let ui = &theme.ui;
        let h = crate::theme::hsla;
        let (accent, border, placeholder, bg, chip_bg, chip_text) = (
            h(ui.purple),
            h(ui.border_strong),
            h(ui.text_4),
            h(ui.raised),
            h(gilvt_theme::color::mix(ui.panel, ui.purple, 0.12)),
            h(gilvt_theme::color::mix(ui.text, ui.purple, 0.3)),
        );
        let focused = self.focus_handle.is_focused(window);
        let focus = self.focus_handle.clone();
        let entity = cx.entity();
        let place = self.place;
        let mut chips = div().flex().flex_wrap().gap_1();
        for (i, (_, label)) in self.draft.chips.iter().enumerate() {
            chips = chips.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .rounded_sm()
                    .bg(chip_bg)
                    .text_color(chip_text)
                    .child(format!("@{label}"))
                    .child(
                        div()
                            .id(("chat-chip-remove", i))
                            .relative()
                            .children(rects::recorder(place.chip_remove_rect(i)))
                            .cursor_pointer()
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_chip(i, cx))),
                    ),
            );
        }
        let caret = || div().flex_none().w(px(1.5)).h(px(13.)).bg(accent);
        let lines: Vec<String> = self.draft.text.split('\n').map(str::to_string).collect();
        let last = lines.len() - 1;
        let mut body = div().flex().flex_col().min_h(px(18.));
        if self.draft.text.is_empty() && self.marked.is_none() {
            body = body.child(div().flex().items_center().children(focused.then(caret)).child(div().ml(px(2.)).text_color(placeholder).child(place.placeholder())));
        } else {
            for (i, l) in lines.into_iter().enumerate() {
                let mut row = div().flex().items_center().min_h(px(16.)).child(l);
                if i == last {
                    row = row.children(self.marked.clone().map(|m| div().underline().child(m))).children(focused.then(caret));
                }
                body = body.child(row);
            }
        }
        if let Some(p) = &self.draft.picker {
            body = body.child(div().text_color(accent).child(format!("@{}", p.query)));
        }
        div()
            .id(place.element_id())
            .key_context(CHAT_INPUT_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::paste))
            .relative()
            .children(rects::recorder(place.input_rect()))
            .w_full()
            .p_1()
            .rounded(px(6.))
            .border_1()
            .border_color(if focused { accent } else { border })
            .bg(bg)
            .flex()
            .flex_col()
            .gap_1()
            .child(chips)
            .child(body)
            .child(
                canvas(|_, _, _| {}, move |bounds, _, window, cx| {
                    window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                })
                .absolute()
                .size_full(),
            )
    }
}

impl gpui::EntityInputHandler for ChatInput {
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
        self.draft.insert(text);
        self.reveal_selection();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
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

    fn cands() -> Vec<Candidate> {
        vec![
            Candidate { key: "agent:codex:w1".into(), label: "web-login · web · 右".into() },
            Candidate { key: "pane:7".into(), label: "zsh · ~/work/web".into() },
            Candidate { key: "agent:claude:a1".into(), label: "api-refactor · api".into() },
        ]
    }

    #[test]
    fn at_opens_the_picker_only_at_a_word_start() {
        let mut d = Draft::default();
        d.insert("看看 ");
        d.insert("@");
        assert_eq!(d.picker().map(|p| p.query.as_str()), Some(""));
        assert_eq!(d.text(), "看看 ", "the @ itself is not text");
        let mut e = Draft::default();
        e.insert("a@b");
        assert!(e.picker().is_none(), "an e-mail is just text");
        assert_eq!(e.text(), "a@b");
    }

    #[test]
    fn typing_filters_and_enter_makes_a_chip() {
        let mut d = Draft::default();
        d.insert("@");
        d.insert("WE");
        assert_eq!(d.matches(&cands()).iter().map(|c| c.key.as_str()).collect::<Vec<_>>(), ["agent:codex:w1", "pane:7"], "case-insensitive, label or key");
        d.move_selection(1, 2);
        assert!(d.choose(&cands()));
        assert_eq!(d.chips(), [("pane:7".to_string(), "zsh · ~/work/web".to_string())]);
        assert!(d.picker().is_none());
        d.insert("@");
        assert_eq!(d.matches(&cands()).len(), 2, "chosen sessions are not offered again");
        d.backspace();
        assert!(d.picker().is_none(), "backspace on an empty query closes the picker");
    }

    #[test]
    fn backspace_on_empty_text_removes_the_last_chip() {
        let mut d = Draft::default();
        d.add_chip("pane:7".into(), "zsh".into());
        d.add_chip("pane:7".into(), "zsh".into());
        assert_eq!(d.chips().len(), 1, "no duplicates");
        d.insert("x");
        d.backspace();
        d.backspace();
        assert!(d.chips().is_empty());
    }

    #[test]
    fn take_sends_the_text_and_keeps_the_chips() {
        let mut d = Draft::default();
        assert_eq!(d.take(), None);
        d.add_chip("agent:codex:w1".into(), "web-login".into());
        d.insert("  为什么");
        d.newline();
        d.insert("失败？  ");
        assert_eq!(d.take(), Some(Outgoing { text: "为什么\n失败？".into(), chips: vec![("agent:codex:w1".into(), "web-login".into())] }));
        assert_eq!(d.text(), "");
        assert_eq!(d.chips().len(), 1, "follow-up questions keep the scope");
        d.insert("   ");
        assert_eq!(d.take(), None, "blank text is not sent");
    }

    #[test]
    fn pasted_text_keeps_newlines_and_is_capped() {
        let mut d = Draft::default();
        d.insert("一\r\n二\t三\u{7}");
        assert_eq!(d.text(), "一\n二\t三");
        d.insert(&"长".repeat(MAX_CHARS));
        assert_eq!(d.text().chars().count(), MAX_CHARS);
    }

    #[test]
    fn whitespace_closes_a_stray_picker_and_keeps_the_text() {
        let mut d = Draft::default();
        d.insert("@ab cd");
        assert!(d.picker().is_none());
        assert_eq!(d.text(), "@ab cd");
    }

    #[test]
    fn enter_without_a_match_restores_the_text() {
        let mut d = Draft::default();
        d.insert("@zzz");
        assert!(!d.choose(&cands()));
        assert_eq!((d.text(), d.picker().is_none()), ("@zzz", true));
    }

    #[test]
    fn escape_restores_only_when_nothing_matches() {
        let mut d = Draft::default();
        d.insert("@zzz");
        d.cancel_picker(&cands());
        assert_eq!(d.text(), "@zzz");
        let mut d = Draft::default();
        d.insert("@web");
        d.cancel_picker(&cands());
        assert_eq!((d.text(), d.picker().is_none()), ("", true));
    }

    #[test]
    fn cjk_query_filters_and_backspaces_by_character() {
        let all = vec![Candidate { key: "pane:1".into(), label: "登录页 · web".into() }, Candidate { key: "pane:2".into(), label: "api".into() }];
        let mut d = Draft::default();
        d.insert("@登录");
        assert_eq!(d.matches(&all).len(), 1);
        d.backspace();
        assert_eq!(d.picker().map(|p| p.query.as_str()), Some("登"));
    }

    #[test]
    fn selection_wraps_both_ways_and_chips_can_be_removed() {
        let mut d = Draft::default();
        d.insert("@");
        d.move_selection(-1, 3);
        assert_eq!(d.picker().map(|p| p.selected), Some(2));
        d.move_selection(1, 3);
        assert_eq!(d.picker().map(|p| p.selected), Some(0));
        d.add_chip("a".into(), "A".into());
        d.add_chip("b".into(), "B".into());
        d.remove_chip(0);
        d.remove_chip(9);
        assert_eq!(d.chips(), [("b".to_string(), "B".to_string())]);
    }

    #[test]
    fn the_query_is_capped() {
        let mut d = Draft::default();
        d.insert("@");
        d.insert(&"x".repeat(MAX_CHARS + 50));
        assert_eq!(d.picker().unwrap().query.chars().count(), MAX_CHARS);
    }

    #[test]
    fn at_opens_after_cjk_and_full_width_forms() {
        for typed in ["你好@", "你好，@", "你好＠", "@"] {
            let mut d = Draft::default();
            d.insert(typed);
            assert!(d.picker().is_some(), "{typed}");
        }
        let mut d = Draft::default();
        d.insert("a＠");
        assert!(d.picker().is_none());
    }

    #[test]
    fn only_plain_and_shift_enter_count() {
        let m = |shift, alt, function| gpui::Modifiers { shift, alt, function, ..Default::default() };
        assert_eq!(classify_enter(&m(false, false, false)), Some(EnterKey::Send));
        assert_eq!(classify_enter(&m(true, false, false)), Some(EnterKey::Newline));
        assert_eq!(classify_enter(&m(false, true, false)), None);
        assert_eq!(classify_enter(&m(false, false, true)), None);
    }

    #[test]
    fn places_have_their_own_placeholder_and_rects() {
        assert_eq!(InputPlace::Panel.placeholder(), PLACEHOLDER);
        assert_eq!(InputPlace::CommandBar.placeholder(), "问监控官…（@ 选会话 · ⏎ 发送 · Esc 收起）");
        assert_eq!(InputPlace::Panel.input_rect(), RectId::ChatInput);
        assert_eq!(InputPlace::CommandBar.input_rect(), RectId::CommandBarInput);
        assert_eq!(InputPlace::CommandBar.chip_remove_rect(2), RectId::CommandBarChipRemove(2));
        assert_ne!(InputPlace::Panel.chip_remove_rect(0), InputPlace::CommandBar.chip_remove_rect(0), "both can be drawn in one window");
        assert_ne!(InputPlace::Panel.element_id(), InputPlace::CommandBar.element_id());
    }
}
