//! The bottom command bar (S2 §6.4; mockup s2-chat.html 图 3): a 20px line at the bottom of every workspace window
//! that shows what the 监控官 is doing or the first sentence of its last answer; ⌘⇧M opens an input with the last
//! question and answer above it. Same conversation as the chat panel (`monitor::chat`). This part is pure: the line's
//! text, the popup's content and the open / close state machine. (Where a session link goes is decided by
//! `chat_view::link_target`, the same rule as the panel.)

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{div, prelude::*, px, AnyElement, App, Context, Entity, Font, FocusHandle, Focusable, MouseButton, ScrollHandle, Subscription, WeakEntity, Window};

use super::chat;
use super::chat_input::{ChatInput, ChatInputEvent};
use super::chat_md::{self, LineKind, MdLine};
use super::chat_model::{Conversation, ErrorCard, Message, Role, Status};
use super::chat_view::{self, Colors, LinkStyle};
use super::model::MonitorModel;
use crate::debug_state::rects::{self, RectId};
use crate::theme::AppSettings;
use crate::workspace::Workspace;

pub const BAR_HEIGHT: f32 = 20.0;
pub const POPUP_MAX_HEIGHT: f32 = 260.0;
pub const LINE_MAX_CHARS: usize = 80;
pub const TITLE: &str = "◎ 监控官";
pub const HINT: &str = "⌘⇧M 提问";
pub const HINT_OPEN: &str = "Esc 收起";
pub const PLACEHOLDER: &str = "问监控官…（@ 选会话 · ⏎ 发送 · Esc 收起）";
pub const EMPTY_POPUP: &str = "还没有对话。可以问「哪些需要我？」";
pub const OPEN_MONITOR: &str = "在监控官中查看 ↗";

/// At most `max` chars; a longer text keeps `max - 1` and ends in 「…」.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// A Markdown line as plain text: link text, code without backticks, whitespace runs folded into one space.
fn plain(line: &MdLine) -> String {
    let text: String = line.spans.iter().map(|s| s.text.as_str()).collect();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A Markdown line as the popup draws it: prose as [`plain`], a code line verbatim (its indentation is content).
fn as_drawn(line: &MdLine) -> String {
    match line.kind {
        LineKind::Code => line.spans.iter().map(|s| s.text.as_str()).collect(),
        _ => plain(line),
    }
}

/// The first sentence of an answer: the first non-empty paragraph / list item (quoted or not; a heading only when
/// there is nothing else), cut after the first 。！？ or an ASCII . ! ? followed by whitespace or the end (plus any closing quotes / brackets), at most [`LINE_MAX_CHARS`].
pub fn first_sentence(md: &str) -> Option<String> {
    let lines = chat_md::lines(md);
    let body = lines.iter().find(|l| matches!(l.kind, LineKind::Paragraph | LineKind::Item { .. }) && !plain(l).is_empty());
    let line = body.or_else(|| lines.iter().find(|l| matches!(l.kind, LineKind::Heading(_)) && !plain(l).is_empty()))?;
    let chars: Vec<char> = plain(line).chars().collect();
    let closer = |c: char| "」』）】》〉”’)]}\"'".contains(c);
    let end = (0..chars.len())
        .find_map(|i| {
            let c = chars[i];
            if !matches!(c, '。' | '！' | '？' | '!' | '?' | '.') {
                return None;
            }
            // Closing quotes / brackets belong to the sentence: 他说「好。」 stays balanced.
            let mut j = i + 1;
            while chars.get(j).is_some_and(|&n| closer(n)) {
                j += 1;
            }
            // The ASCII marks end a sentence only before whitespace or the end (not in `v1.2`, `x/y?id=1`).
            let full_width = matches!(c, '。' | '！' | '？');
            (full_width || chars.get(j).is_none_or(|n| n.is_whitespace())).then_some(j)
        })
        .unwrap_or(chars.len());
    Some(clip(&chars[..end].iter().collect::<String>(), LINE_MAX_CHARS))
}

/// The error the last question ended in, if any: the last message after the last question, ignoring notices, is an
/// error. (A failed turn leaves the status `Idle`, so the status alone does not tell.)
fn pending_error(conv: &Conversation) -> Option<&Message> {
    let last_user = conv.messages.iter().rposition(|m| m.role == Role::User);
    let (i, m) = conv.messages.iter().enumerate().rev().find(|(_, m)| m.role != Role::Notice)?;
    (m.role == Role::Error && last_user.is_none_or(|u| i > u)).then_some(m)
}

/// The collapsed line (Decision 8).
pub fn collapsed_line(conv: &Conversation) -> String {
    let title = crate::i18n::text(TITLE, "◎ Monitor");
    let with = |s: &str| format!("{title} · {s}");
    match conv.status {
        Status::Starting => return with(crate::i18n::text("启动中…", "Starting…")),
        Status::Answering => return with(crate::i18n::text("回答中…", "Answering…")),
        Status::Stopping => return with(crate::i18n::text("停止中…", "Stopping…")),
        Status::Error | Status::Idle => {}
    }
    if let Some(m) = pending_error(conv) {
        let what = m
            .card
            .as_ref()
            .map_or(m.text.as_str(), |c| c.title.as_str());
        let line = clip(what.lines().next().unwrap_or(""), LINE_MAX_CHARS);
        return with(
            &if crate::i18n::current() == crate::i18n::Language::English {
                format!("Error: {line}")
            } else {
                format!("出错：{line}")
            },
        );
    }
    // Only the newest answer counts: when it has no sentence (code only, a stopped turn without text) show the title.
    match conv.messages.iter().rev().find(|m| m.role == Role::Assistant).and_then(|m| first_sentence(&m.text)) {
        Some(s) => with(&s),
        None => title.to_string(),
    }
}

/// The line's right-hand hint.
pub fn hint_text(expanded: bool) -> &'static str {
    if expanded {
        crate::i18n::text(HINT_OPEN, "Esc collapse")
    } else {
        crate::i18n::text(HINT, "⇧⌘M ask")
    }
}

/// The last question and what came after it (indices into the conversation's messages).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exchange {
    pub question: usize,
    pub replies: Range<usize>,
}

pub fn last_exchange(messages: &[Message]) -> Option<Exchange> {
    let question = messages.iter().rposition(|m| m.role == Role::User)?;
    Some(Exchange { question, replies: question + 1..messages.len() })
}

/// 「你：@zsh @web-login 为什么失败？」.
pub fn question_line(m: &Message) -> String {
    let chips: String = m.chips.iter().map(|(_, label)| format!("@{label} ")).collect();
    if crate::i18n::english() {
        format!("You: {chips}{}", m.text)
    } else {
        format!("你：{chips}{}", m.text)
    }
}

/// An error card on one line: 「title：text」.
fn card_line(c: &ErrorCard) -> String {
    if crate::i18n::english() {
        format!("{}: {}", c.title, c.text)
    } else {
        format!("{}：{}", c.title, c.text)
    }
}

/// The popup as plain text (DebugState `popup_text`): the question, then each reply's tool rows and Markdown lines,
/// notices and errors; [`EMPTY_POPUP`] before the first question.
pub fn popup_text(messages: &[Message]) -> String {
    let Some(x) = last_exchange(messages) else {
        return crate::i18n::text(
            EMPTY_POPUP,
            "No conversation yet. Try asking \"What needs me?\"",
        )
        .to_string();
    };
    let mut out = vec![question_line(&messages[x.question])];
    for m in &messages[x.replies] {
        match m.role {
            Role::Assistant => {
                out.extend(m.tools.iter().map(|t| t.label.clone()));
                out.extend(chat_md::lines(&m.text).iter().map(as_drawn).filter(|l| !l.trim().is_empty()));
            }
            Role::User | Role::Notice => out.push(m.text.clone()),
            Role::Error => out.push(match &m.card {
                Some(c) => card_line(c),
                None => m.text.clone(),
            }),
        }
    }
    out.join("\n")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarEvent {
    /// ⌘⇧M.
    Toggle,
    /// A click on the 20px line.
    ClickLine,
    /// Esc in the input (with the `@` picker closed).
    Escape,
    /// The input lost the keyboard to something else.
    Blur,
    /// 「在监控官中查看 ↗」.
    OpenMonitor,
    /// A session link in the popup.
    Link,
    /// Every frame (catches the 监控官 being turned off).
    Frame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// Leave the keyboard where it is.
    Keep,
    /// Give it to the bar's input.
    Input,
    /// Give it back to the window's focused pane (`Workspace::focus_active`).
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub expanded: bool,
    pub focus: Focus,
}

/// Open / close (Decision 4). `input_focused`: the bar's input has the keyboard right now.
pub fn step(expanded: bool, enabled: bool, input_focused: bool, ev: BarEvent) -> Step {
    let keep = |expanded| Step { expanded, focus: Focus::Keep };
    if !enabled {
        return Step { expanded: false, focus: if expanded && input_focused { Focus::Back } else { Focus::Keep } };
    }
    match (ev, expanded) {
        (BarEvent::Toggle | BarEvent::ClickLine, false) | (BarEvent::ClickLine, true) => Step { expanded: true, focus: Focus::Input },
        (BarEvent::Toggle | BarEvent::Escape | BarEvent::Link, true) => Step { expanded: false, focus: Focus::Back },
        (BarEvent::Blur | BarEvent::OpenMonitor, true) => keep(false),
        (_, expanded) => keep(expanded),
    }
}

// ---------- gpui: one window's bar ----------

/// One window's command bar: open or not, its input (draft and chips survive closing), the popup's scroll.
pub struct CommandBar {
    pub expanded: bool,
    pub input: Entity<ChatInput>,
    pub scroll: ScrollHandle,
    /// The chat revision the popup's scrolling was last decided for.
    pub seen: Rc<Cell<u64>>,
    /// The number of questions at that revision: a new one scrolls to the bottom even when the reader scrolled up.
    pub questions: Rc<Cell<usize>>,
    /// The wall while the bar is open (this window's when it draws one): the `@` candidates and which session links
    /// are live (`chat_view::link_target`; excluded sessions are plain text).
    pub model: Option<Rc<MonitorModel>>,
    /// What had the keyboard when the bar opened. Closing gives the keyboard back through `focus_active` (the
    /// tab's pane, or an open palette); only the 监控官 tab's chat input is restored from here.
    pub return_to: Option<FocusHandle>,
    _events: Subscription,
    _blur: Subscription,
}

impl CommandBar {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> CommandBar {
        let input = cx.new(|cx| ChatInput::for_command_bar(window, cx));
        let events = cx.subscribe_in(&input, window, |ws, _, ev: &ChatInputEvent, window, cx| match ev {
            ChatInputEvent::Send(out) => {
                let out = out.clone();
                App::defer(cx, move |cx| chat::send(out, cx));
            }
            ChatInputEvent::Escape => ws.command_bar_event(BarEvent::Escape, window, cx),
        });
        let focus = input.focus_handle(cx);
        // The keyboard went elsewhere (a click in a pane, ⌘1 …): close without taking it back.
        let blur = cx.on_blur(&focus, window, |ws, window, cx| ws.command_bar_event(BarEvent::Blur, window, cx));
        CommandBar {
            expanded: false,
            input,
            scroll: ScrollHandle::new(),
            seen: Default::default(),
            questions: Default::default(),
            model: None,
            return_to: None,
            _events: events,
            _blur: blur,
        }
    }
}

fn colors(cx: &App) -> Colors {
    Colors::new(&crate::theme::current(cx).ui)
}

fn enabled(cx: &App) -> bool {
    cx.global::<AppSettings>().0.monitor.enabled
}

/// The 20px line: status / first sentence on the left, the unread dot and the hint on the right. Takes layout space
/// (the panes end above it). A click opens the bar. A long line is cut by its drawn width (CJK is twice as wide).
pub fn line(bar: &CommandBar, _window: &Window, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    if !enabled(cx) {
        return None;
    }
    let k = colors(cx);
    let view = chat::view(cx);
    if bar.expanded && view.unread {
        App::defer(cx, chat::mark_read);
    }
    Some(
        div()
            .id("command-bar")
            .relative()
            .children(rects::recorder(RectId::CommandBar))
            .flex_none()
            .h(px(BAR_HEIGHT))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .border_t_1()
            .border_color(if bar.expanded { k.ai } else { k.border })
            .bg(k.ai_bg)
            .text_size(px(10.5))
            .text_color(k.ai)
            .cursor_pointer()
            // Keeps the root from taking the keyboard on mouse down (Decision 5): the pane keeps it until the click.
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_click(cx.listener(|ws, _, window, cx| ws.command_bar_event(BarEvent::ClickLine, window, cx)))
            .child(div().flex_1().min_w(px(0.)).truncate().child(collapsed_line(&view.conv)))
            .children((view.unread && !bar.expanded).then(|| div().flex_none().size(px(6.)).rounded_full().bg(k.red)))
            .child(div().flex_none().text_color(k.faint).child(hint_text(bar.expanded)))
            .into_any_element(),
    )
}

/// One reply of the last exchange, as the panel draws it (tool rows, Markdown with live session links).
fn reply(m: &Message, n: usize, style: &LinkStyle, base: &Font, weak: &WeakEntity<Workspace>, k: &Colors) -> AnyElement {
    match m.role {
        Role::Assistant => {
            let mut col = div().flex().flex_col().gap(px(2.)).min_w(px(0.));
            for t in &m.tools {
                col = col.child(div().text_size(px(10.5)).text_color(if t.done && !t.ok { k.red } else { k.faint }).child(t.label.clone()));
            }
            let mut link = 0;
            for (li, l) in chat_md::lines(&m.text).iter().enumerate() {
                col = col.child(chat_view::md_line(l, n, li, &mut link, style, base, weak, k));
            }
            if m.open && m.text.is_empty() {
                col = col.child(div().text_color(k.faint).child("…"));
            }
            col.into_any_element()
        }
        Role::Notice => div().text_size(px(10.5)).text_color(k.faint).child(m.text.clone()).into_any_element(),
        Role::Error => div()
            .text_color(k.red)
            .child(match &m.card {
                Some(c) => card_line(c),
                None => m.text.clone(),
            })
            .into_any_element(),
        Role::User => div().text_size(px(10.5)).text_color(k.faint).child(question_line(m)).into_any_element(),
    }
}

/// The open bar: the last question and answer, 「在监控官中查看 ↗」, the `@` picker and the input, over the bottom of
/// the pane area (no layout space: the panes keep their size).
pub fn popup(bar: &CommandBar, window: &Window, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    if !bar.expanded || !enabled(cx) {
        return None;
    }
    let k = colors(cx);
    let view = chat::view(cx);
    let msgs = &view.conv.messages;
    chat_view::follow_answer(&view, &bar.scroll, &bar.seen, &bar.questions);
    let base = window.text_style().font();
    let weak = cx.entity().downgrade();
    let no_cards = MonitorModel::default();
    let style = LinkStyle {
        prefix: "command-bar-text",
        rect: RectId::CommandBarLink,
        wall: bar.model.as_deref().unwrap_or(&no_cards),
        on_click: Rc::new(|ws: &mut Workspace, key, window: &mut Window, cx: &mut Context<Workspace>| ws.command_bar_open_link(key, window, cx)),
    };
    let mut body = div().id("command-bar-scroll").max_h(px(POPUP_MAX_HEIGHT)).overflow_y_scroll().track_scroll(&bar.scroll).flex().flex_col().gap_1();
    match last_exchange(msgs) {
        None => {
            body = body.child(div().text_color(k.faint).child(crate::i18n::text(
                EMPTY_POPUP,
                "No conversation yet. Try asking \"What needs me?\"",
            )))
        }
        Some(x) => {
            body = body.child(div().text_size(px(10.5)).text_color(k.faint).child(question_line(&msgs[x.question])));
            for n in x.replies {
                body = body.child(reply(&msgs[n], n, &style, &base, &weak, &k));
            }
        }
    }

    let input = bar.input.clone();
    let picker_el = chat_view::picker_list("command-bar-picker", RectId::CommandBarPickerItem, &input, &k, cx);

    Some(
        div()
            .id("command-bar-popup")
            .children(rects::recorder(RectId::CommandBarPopup))
            .occlude()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            // A click on the popup's background leaves the keyboard in the input.
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .bg(k.bg)
            .border_t_1()
            .border_color(k.ai)
            .shadow_lg()
            .text_size(px(12.))
            .text_color(k.text)
            .child(body)
            .child(
                div().flex().child(
                    chat_view::button(
                        "command-bar-open-monitor",
                        RectId::CommandBarOpenMonitor,
                        crate::i18n::text(OPEN_MONITOR, "View in Monitor ↗"),
                        &k,
                    )
                    .on_click(
                        cx.listener(|ws, _, window, cx| ws.command_bar_open_monitor(window, cx)),
                    ),
                ),
            )
            .children(picker_el)
            .child(input)
            .into_any_element(),
    )
}

/// DebugState's `windows[].command_bar`, walked like [`popup`] draws it (same `RectId` numbers).
pub fn debug_command_bar(
    view: &chat::ChatView,
    expanded: bool,
    focused: bool,
    draft: &super::chat_input::Draft,
    matches: &[super::chat_input::Candidate],
    rect: &dyn Fn(RectId) -> Option<crate::debug_state::rects::Rect4>,
) -> crate::debug_state::CommandBarState {
    use crate::debug_state as ds;
    let msgs = &view.conv.messages;
    let mut links = Vec::new();
    if let Some(x) = last_exchange(msgs) {
        for n in x.replies.filter(|&n| msgs[n].role == Role::Assistant) {
            for (j, (text, key)) in chat_md::session_links(&msgs[n].text).into_iter().enumerate() {
                links.push(ds::ChatLinkState { key, text, rect: rect(RectId::CommandBarLink(n, j)) });
            }
        }
    }
    ds::CommandBarState {
        expanded,
        focused,
        status: view.conv.status.id(),
        line: collapsed_line(&view.conv),
        hint: hint_text(expanded),
        unread: view.unread,
        popup_text: popup_text(msgs),
        links,
        input_text: draft.text().to_string(),
        chips: draft
            .chips()
            .iter()
            .enumerate()
            .map(|(i, (key, label))| ds::ChatChip { key: key.clone(), label: label.clone(), remove: rect(RectId::CommandBarChipRemove(i)) })
            .collect(),
        picker: draft.picker().map(|p| ds::ChatPicker {
            query: p.query.clone(),
            selected: p.selected,
            items: matches.iter().enumerate().map(|(i, c)| ds::ChatPickerItem { key: c.key.clone(), label: c.label.clone(), rect: rect(RectId::CommandBarPickerItem(i)) }).collect(),
        }),
        open_monitor: rect(RectId::CommandBarOpenMonitor),
        popup_rect: rect(RectId::CommandBarPopup),
        input_rect: rect(RectId::CommandBarInput),
        rect: rect(RectId::CommandBar),
    }
}

#[cfg(test)]
#[path = "command_bar_tests.rs"]
mod tests;
