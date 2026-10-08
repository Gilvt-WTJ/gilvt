//! The 「◎ 监控官」 tab's chat panel (S2 §6.4; mockups s2-chat.html 图 1, 2, 4): header, messages (Markdown, tool
//! rows, session links), quick questions, the `@` picker and the input; a 26px rail when the tab is narrower than
//! 760px (an overlay when opened) or collapsed with ⇥; a red card when the chat cannot start. Not drawn while the
//! 监控官 is off.

use std::cell::Cell;
use std::ops::Range;
use std::process::Stdio;
use std::rc::Rc;

use gpui::{
    canvas, div, prelude::*, px, AnyElement, App, Context, Div, Entity, Font, FontStyle, FontWeight, Hsla, InteractiveText, ScrollHandle, SharedString, StyledText,
    TextRun, UnderlineStyle, WeakEntity, Window,
};

use gilvt_theme::color::mix;
use gilvt_theme::UiColors;

use super::chat::{self, ChatView};
use super::chat_input::{Candidate, ChatInput, Draft};
use super::chat_md::{self, LineKind};
use super::chat_model::{Outgoing, Role, Status};
use super::model::MonitorModel;
use super::MonitorPane;
use crate::debug_state::rects::{self, Rect4, RectId};
use crate::debug_state as ds;
use crate::pane_tree::PaneId;
use crate::theme::hsla;
use crate::workspace::Workspace;

pub const PANEL_WIDTH: f32 = 360.0;
pub const NARROW_WIDTH: f32 = 760.0;
pub const RAIL_WIDTH: f32 = 26.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelMode {
    Panel,
    Rail,
    /// Rail plus the panel over the wall (narrow tab, opened).
    Overlay,
}

/// The tab is narrower than `NARROW_WIDTH` (0 = not measured yet: wide).
pub fn is_narrow(width: f32) -> bool {
    width > 0.0 && width < NARROW_WIDTH
}

pub fn panel_mode(width: f32, collapsed: bool, open: bool) -> PanelMode {
    match (is_narrow(width), collapsed, open) {
        (true, _, true) => PanelMode::Overlay,
        (true, _, false) | (false, true, _) => PanelMode::Rail,
        (false, false, _) => PanelMode::Panel,
    }
}

/// The `@` list: every card of the wall in order (已结束 last), whatever the filter, minus excluded ones;
/// 「name · location」.
pub fn candidates(model: &MonitorModel) -> Vec<Candidate> {
    model.askable.iter().map(|e| Candidate { key: e.key.clone(), label: e.label.clone() }).collect()
}

/// DebugState's `windows[].monitor.chat`, walked like [`panel`] draws it (same `RectId` numbers).
pub fn debug_chat(view: &ChatView, mode: PanelMode, draft: &Draft, matches: &[Candidate], rect: &dyn Fn(RectId) -> Option<Rect4>) -> ds::ChatState {
    let messages = view
        .conv
        .messages
        .iter()
        .enumerate()
        .map(|(n, m)| ds::ChatMessageState {
            role: m.role.id(),
            text: m.text.clone(),
            chips: m.chips.iter().map(|(k, _)| k.clone()).collect(),
            tools: m.tools.iter().map(|t| ds::ChatTool { name: t.name.clone(), label: t.label.clone(), done: t.done, ok: t.ok }).collect(),
            links: match m.role {
                Role::Assistant => chat_md::session_links(&m.text).into_iter().enumerate().map(|(j, (text, key))| ds::ChatLinkState { key, text, rect: rect(RectId::ChatLink(n, j)) }).collect(),
                _ => Vec::new(),
            },
            error: m.card.as_ref().map(|c| ds::ChatErrorState { title: c.title.clone(), text: c.text.clone(), settings: rect(RectId::ChatErrorSettings(n)), log: rect(RectId::ChatErrorLog(n)) }),
            rect: rect(RectId::ChatMessage(n)),
        })
        .collect();
    ds::ChatState {
        collapsed: mode != PanelMode::Panel,
        open: mode == PanelMode::Overlay,
        status: view.conv.status.id(),
        provider: view.provider,
        model: view.model.clone(),
        unread: view.unread,
        scope: draft.chips().iter().map(|(k, _)| k.clone()).collect(),
        messages,
        quick: super::chat_model::quick()
            .iter()
            .enumerate()
            .map(|(i, (label, _))| {
                ds::ChatQuick {
                    label: label.to_string(),
                    rect: rect(RectId::ChatQuick(i)),
                }
            })
            .collect(),
        input: ds::ChatInputState {
            text: draft.text().to_string(),
            chips: draft.chips().iter().enumerate().map(|(i, (key, label))| ds::ChatChip { key: key.clone(), label: label.clone(), remove: rect(RectId::ChatChipRemove(i)) }).collect(),
            rect: rect(RectId::ChatInput),
        },
        picker: draft.picker().map(|p| ds::ChatPicker {
            query: p.query.clone(),
            selected: p.selected,
            items: matches.iter().enumerate().map(|(i, c)| ds::ChatPickerItem { key: c.key.clone(), label: c.label.clone(), rect: rect(RectId::ChatPickerItem(i)) }).collect(),
        }),
        stop: rect(RectId::ChatStop),
        new: rect(RectId::ChatNew),
        model_button: rect(RectId::ChatModel),
        collapse: rect(RectId::ChatCollapse),
        rail: rect(RectId::ChatRail),
        rect: rect(RectId::ChatPanel),
    }
}

pub(crate) struct Colors {
    pub(crate) bg: Hsla,
    pub(crate) border: Hsla,
    pub(crate) text: Hsla,
    pub(crate) faint: Hsla,
    pub(crate) user: Hsla,
    pub(crate) ai: Hsla,
    pub(crate) ai_bg: Hsla,
    pub(crate) red: Hsla,
    pub(crate) red_bg: Hsla,
    pub(crate) chip: Hsla,
    pub(crate) code_bg: Hsla,
}

impl Colors {
    // The ✦ purple and its tints match the card wall's (`monitor/view.rs` Colors).
    pub(crate) fn new(ui: &UiColors) -> Colors {
        let h = hsla;
        Colors {
            bg: h(ui.panel),
            border: h(ui.border),
            text: h(ui.text),
            faint: h(ui.text_3),
            user: h(ui.selected),
            ai: h(ui.purple),
            ai_bg: h(mix(ui.panel, ui.purple, 0.12)),
            red: h(ui.error.fg),
            red_bg: h(ui.error.bg),
            chip: h(ui.fill),
            code_bg: h(ui.inset),
        }
    }
}

pub(crate) fn button(id: &'static str, rect: RectId, label: &str, k: &Colors) -> gpui::Stateful<gpui::Div> {
    div().id(id).relative().children(rects::recorder(rect)).px_2().rounded_sm().cursor_pointer().bg(k.chip).text_color(k.text).child(label.to_string())
}

/// Where a `gilvt://session/<key>` link goes: the card's pane (None: a card without one, e.g. 已结束). None when no
/// card of the wall has the key (whatever the filter), or its session is excluded: such a link is plain text.
pub fn link_target(model: &MonitorModel, key: &str) -> Option<Option<PaneId>> {
    model.askable.iter().find(|e| e.key == key).map(|e| e.pane)
}

/// The highlighted row of the `@` picker: the input's `selected` is not clamped when the filter shrinks the list.
pub fn picker_highlight(selected: usize, matches: usize) -> Option<usize> {
    (matches > 0).then(|| selected.min(matches - 1))
}

/// Follow the answer down only when the list was already at its bottom (within a few px), or when a new question
/// was just asked; otherwise the user's scroll position stays. `offset_y` ≤ 0 (gpui), `max_y` ≥ 0.
pub fn stick_to_bottom(offset_y: f32, max_y: f32, new_question: bool) -> bool {
    new_question || -offset_y >= max_y - 4.0
}

/// Before a message list is drawn: when the conversation changed, follow it down only from the bottom or after a
/// new question ([`stick_to_bottom`]). `seen` / `questions` remember the revision and question count last handled.
pub(crate) fn follow_answer(view: &ChatView, scroll: &ScrollHandle, seen: &Cell<u64>, questions: &Cell<usize>) {
    if view.revision == seen.get() {
        return;
    }
    seen.set(view.revision);
    let now = view.conv.messages.iter().filter(|msg| msg.role == Role::User).count();
    let new_question = now != questions.replace(now);
    let (offset, max) = (scroll.offset().y, scroll.max_offset().height);
    if stick_to_bottom(f32::from(offset), f32::from(max), new_question) {
        scroll.scroll_to_bottom();
    }
}

/// The most the `@` picker's rows take before they scroll (about six rows): a long session list neither pushes the
/// input away nor runs off the panel or the command bar's popup.
pub const PICKER_MAX_HEIGHT: f32 = 120.0;

/// The `@` picker above an input (None while it is closed): matching sessions, the highlighted one, 「⏎ 选中 · Esc
/// 取消」. `id` prefixes the rows' element ids, `rect(i)` is row i's `RectId` (the panel and the command bar each
/// have their own).
pub(crate) fn picker_list(id: &'static str, rect: fn(usize) -> RectId, input: &Entity<ChatInput>, k: &Colors, cx: &mut Context<Workspace>) -> Option<Div> {
    let selected = input.read(cx).draft().picker()?.selected;
    let matches = input.read(cx).matches();
    let highlight = picker_highlight(selected, matches.len());
    let mut rows = div().id(SharedString::from(format!("{id}-list"))).max_h(px(PICKER_MAX_HEIGHT)).overflow_y_scroll().track_scroll(input.read(cx).picker_scroll()).flex().flex_col();
    for (i, c) in matches.iter().enumerate() {
        let input = input.clone();
        rows = rows.child(
            div()
                .id((id, i))
                .relative()
                .children(rects::recorder(rect(i)))
                .flex_none()
                .px_1()
                .rounded_sm()
                .cursor_pointer()
                .truncate()
                .when(highlight == Some(i), |d| d.bg(k.ai_bg))
                .child(c.label.clone())
                .on_click(cx.listener(move |_, _, _, cx| input.update(cx, |i_, cx| i_.choose_index(i, cx)))),
        );
    }
    let list = div()
        .p_1()
        .rounded_md()
        .border_1()
        .border_color(k.border)
        .bg(k.bg)
        .flex()
        .flex_col()
        .child(rows);
    let list = if matches.is_empty() {
        list.child(
            div()
                .text_color(k.faint)
                .child(crate::i18n::text("没有匹配的会话", "No matching sessions")),
        )
    } else {
        list
    };
    Some(
        list.child(
            div()
                .text_size(px(10.))
                .text_color(k.faint)
                .child(crate::i18n::text(
                    "⏎ 选中 · Esc 取消",
                    "⏎ select · Esc cancel",
                )),
        ),
    )
}

/// What a click on a session link in an answer does (the key is the link's session).
pub(crate) type LinkClick = Rc<dyn Fn(&mut Workspace, String, &mut Window, &mut Context<Workspace>)>;

/// How answers' session links are drawn and recorded: the chat panel and the bottom command bar draw the same
/// Markdown, each with its own element ids and `RectId`s.
pub(crate) struct LinkStyle<'a> {
    /// Element id prefix of a line's text: `<prefix>-<n>-<li>`.
    pub prefix: &'static str,
    /// The rect of session link `j` of message `n`.
    pub rect: fn(usize, usize) -> RectId,
    /// Decides which links are clickable ([`link_target`]).
    pub wall: &'a MonitorModel,
    pub on_click: LinkClick,
}

/// A line's spans as one wrapping text (prose wraps as words, not as boxes). Session links that `link_target`
/// accepts are clickable ranges; each records its laid-out rect as `(style.rect)(n, j)` (the part on the line
/// where the link starts). `j` counts every session link in reading order, as `chat_md::session_links` does,
/// also the ones drawn as plain text.
#[allow(clippy::too_many_arguments)]
fn inline_text(line: &chat_md::MdLine, n: usize, li: usize, link: &mut usize, bold: bool, color: Hsla, style: &LinkStyle, base: &Font, weak: &WeakEntity<Workspace>, k: &Colors) -> AnyElement {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut clicks: Vec<(Range<usize>, String, usize)> = Vec::new();
    for s in &line.spans {
        let session = s.link.as_deref().and_then(chat_md::session_key);
        let j = session.map(|_| {
            *link += 1;
            *link - 1
        });
        if s.text.is_empty() {
            continue;
        }
        let start = text.len();
        text.push_str(&s.text);
        let clickable = session.filter(|key| link_target(style.wall, key).is_some());
        let mut font = base.clone();
        if s.bold || bold {
            font.weight = FontWeight::BOLD;
        }
        if s.italic {
            font.style = FontStyle::Italic;
        }
        if s.code {
            font.family = "Menlo".into();
        }
        runs.push(TextRun {
            len: s.text.len(),
            font,
            color: if clickable.is_some() { k.ai } else { color },
            background_color: s.code.then_some(k.code_bg),
            underline: clickable.map(|_| UnderlineStyle { thickness: px(1.), color: None, wavy: false }),
            strikethrough: None,
        });
        if let (Some(key), Some(j)) = (clickable, j) {
            clicks.push((start..text.len(), key.to_string(), j));
        }
    }
    if text.is_empty() {
        return div().h(px(16.)).into_any_element();
    }
    let styled = StyledText::new(text).with_runs(runs);
    let layout = styled.layout().clone();
    let recorders: Vec<_> = clicks.iter().filter_map(|(r, _, j)| rects::text_range_recorder((style.rect)(n, *j), layout.clone(), r.clone())).collect();
    if clicks.is_empty() {
        return div().min_w(px(0.)).child(styled).into_any_element();
    }
    let ranges: Vec<Range<usize>> = clicks.iter().map(|(r, _, _)| r.clone()).collect();
    let keys: Vec<String> = clicks.into_iter().map(|(_, key, _)| key).collect();
    let weak = weak.clone();
    let on_click = style.on_click.clone();
    let interactive = InteractiveText::new(SharedString::from(format!("{}-{n}-{li}", style.prefix)), styled).on_click(ranges, move |i, window, cx| {
        if let Some(key) = keys.get(i).cloned() {
            let _ = weak.update(cx, |ws, cx| on_click(ws, key, window, cx));
        }
    });
    div().min_w(px(0.)).child(interactive).children(recorders).into_any_element()
}

/// One Markdown line of an answer: indented by its list depth, with a bar when quoted.
#[allow(clippy::too_many_arguments)]
pub(crate) fn md_line(line: &chat_md::MdLine, n: usize, li: usize, link: &mut usize, style: &LinkStyle, base: &Font, weak: &WeakEntity<Workspace>, k: &Colors) -> AnyElement {
    let mut outer = div().ml(px(10.0 * line.indent as f32)).min_w(px(0.));
    if line.quoted {
        outer = outer.pl_2().border_l_2().border_color(k.border);
    }
    let color = if line.quoted { k.faint } else { k.text };
    let el = match &line.kind {
        LineKind::Rule => div().h(px(1.)).my_1().bg(k.border).into_any_element(),
        // One physical line of a fence: wraps inside the panel instead of widening it.
        LineKind::Code => {
            let text = line.spans.iter().map(|s| s.text.as_str()).collect::<String>();
            div().min_h(px(15.)).px_1().bg(k.code_bg).font_family("Menlo").text_size(px(11.)).text_color(color).child(text).into_any_element()
        }
        LineKind::Heading(_) => div().mt_1().child(inline_text(line, n, li, link, true, color, style, base, weak, k)).into_any_element(),
        LineKind::Item { marker, .. } => div()
            .flex()
            .child(div().flex_none().w(px(16.)).text_color(k.faint).child(marker.clone()))
            .child(div().flex_1().min_w(px(0.)).child(inline_text(line, n, li, link, false, color, style, base, weak, k)))
            .into_any_element(),
        LineKind::Paragraph => inline_text(line, n, li, link, false, color, style, base, weak, k),
    };
    outer.child(el).into_any_element()
}

fn messages(view: &ChatView, pane: PaneId, wall: &MonitorModel, base: &Font, k: &Colors, cx: &mut Context<Workspace>) -> Vec<AnyElement> {
    let weak = cx.entity().downgrade();
    let style = LinkStyle { prefix: "chat-text", rect: RectId::ChatLink, wall, on_click: Rc::new(move |ws: &mut Workspace, key, _: &mut Window, cx: &mut Context<Workspace>| ws.monitor_open_link(pane, key, cx)) };
    let mut out = Vec::new();
    for (n, msg) in view.conv.messages.iter().enumerate() {
        let el = div().id(("chat-message", n)).relative().children(rects::recorder(RectId::ChatMessage(n)));
        let el = match msg.role {
            Role::User => {
                let chips: Vec<String> = msg.chips.iter().map(|(_, label)| format!("@{label}")).collect();
                el.flex().justify_end().child(
                    div()
                        .max_w(px(280.))
                        .p_2()
                        .rounded_md()
                        .bg(k.user)
                        .children((!chips.is_empty()).then(|| div().text_size(px(10.5)).text_color(k.ai).child(chips.join(" "))))
                        .child(msg.text.clone()),
                )
            }
            Role::Assistant => {
                let mut body = div().p_2().rounded_md().bg(k.ai_bg).flex().flex_col().gap(px(2.)).min_w(px(0.));
                for t in &msg.tools {
                    body = body.child(div().text_size(px(10.5)).text_color(if t.done && !t.ok { k.red } else { k.faint }).child(t.label.clone()));
                }
                let mut link = 0;
                for (li, line) in chat_md::lines(&msg.text).iter().enumerate() {
                    body = body.child(md_line(line, n, li, &mut link, &style, base, &weak, k));
                }
                if msg.open && msg.text.is_empty() {
                    body = body.child(div().text_color(k.faint).child("…"));
                }
                el.child(body)
            }
            Role::Notice => el.flex().justify_center().text_size(px(10.5)).text_color(k.faint).child(msg.text.clone()),
            Role::Error => match &msg.card {
                Some(card) => el
                    .p_2()
                    .rounded_md()
                    .bg(k.red_bg)
                    .border_1()
                    .border_color(k.red)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().font_weight(FontWeight::BOLD).text_color(k.red).child(card.title.clone()))
                    .child(card.text.clone())
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                button(
                                    "chat-error-settings",
                                    RectId::ChatErrorSettings(n),
                                    crate::i18n::text("打开设置", "Open Settings"),
                                    k,
                                )
                                .on_click(cx.listener(
                                    |_, _, _, cx| App::defer(cx, crate::settings_window::open),
                                )),
                            )
                            .child(
                                button(
                                    "chat-error-log",
                                    RectId::ChatErrorLog(n),
                                    crate::i18n::text("查看日志", "View Log"),
                                    k,
                                )
                                .on_click(cx.listener(|_, _, _, cx| App::defer(cx, open_log))),
                            ),
                    ),
                None => el.text_color(k.red).child(msg.text.clone()),
            },
        };
        out.push(el.into_any_element());
    }
    out
}

/// 「查看日志」: `chat.log` in the default text editor (made empty first when it does not exist); a failure is a
/// grey line in the chat.
fn open_log(cx: &mut App) {
    let Some(path) = chat::log_path() else {
        chat::notice(
            crate::i18n::text("无法打开日志：找不到 gilvt 的状态目录", "Could not open the log: gilvt's state folder was not found"),
            cx,
        );
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::OpenOptions::new().create(true).append(true).open(&path);
    let spawned = std::process::Command::new("/usr/bin/open").arg("-t").arg(&path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    match spawned {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) if crate::i18n::english() => chat::notice(&format!("Could not open the log: {e}"), cx),
        Err(e) => chat::notice(&format!("无法打开日志：{e}"), cx),
    }
}

#[allow(clippy::too_many_arguments)]
fn panel(
    pane: PaneId,
    m: &MonitorPane,
    view: &ChatView,
    wall: &MonitorModel,
    base: &Font,
    narrow: bool,
    k: &Colors,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let model = if view.model.is_empty() {
        crate::i18n::text("CLI 默认", "CLI default")
    } else {
        view.model.as_str()
    };
    let status = view.conv.status;
    let mut header = div()
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .border_b_1()
        .border_color(k.border)
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .child(crate::i18n::text("◎ 监控官", "◎ Monitor")),
        )
        .child(
            div()
                .id("chat-model")
                .relative()
                .children(rects::recorder(RectId::ChatModel))
                .cursor_pointer()
                .text_color(k.faint)
                .child(format!("{} · {model} ▾", view.provider_label))
                .on_click(cx.listener(|_, _, _, cx| App::defer(cx, crate::settings_window::open))),
        );
    if let Some(pill) = status.pill() {
        let error = status == Status::Error;
        header = header.child(div().px_1().rounded_full().text_size(px(10.5)).bg(if error { k.red_bg } else { k.ai_bg }).text_color(if error { k.red } else { k.ai }).child(pill));
    }
    header = header.child(div().flex_1());
    if status == Status::Answering {
        header = header.child(
            button(
                "chat-stop",
                RectId::ChatStop,
                crate::i18n::text("停止", "Stop"),
                k,
            )
            .on_click(cx.listener(|_, _, _, cx| App::defer(cx, chat::stop))),
        );
    }
    header = header
        .child(
            button(
                "chat-new",
                RectId::ChatNew,
                crate::i18n::text("新对话", "New Chat"),
                k,
            )
            .on_click(cx.listener(|_, _, _, cx| App::defer(cx, chat::new_conversation))),
        )
        .child(
            button("chat-collapse", RectId::ChatCollapse, "⇥", k).on_click(cx.listener(
                move |ws, _, window, cx| ws.monitor_chat_collapse(pane, narrow, window, cx),
            )),
        );

    follow_answer(view, &m.chat_scroll, &m.chat_seen, &m.chat_questions);
    let list = div()
        .id("chat-messages")
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .track_scroll(&m.chat_scroll)
        .p_2()
        .flex()
        .flex_col()
        .gap_2()
        .children(messages(view, pane, wall, base, k, cx));

    let mut quick = div().flex().flex_wrap().gap_1().px_2();
    for (i, (label, text)) in super::chat_model::quick().into_iter().enumerate() {
        let text = text.to_string();
        quick = quick.child(
            div()
                .id(("chat-quick", i))
                .relative()
                .children(rects::recorder(RectId::ChatQuick(i)))
                .px_2()
                .rounded_full()
                .cursor_pointer()
                .bg(k.ai_bg)
                .text_color(k.ai)
                .child(label)
                .on_click(cx.listener(move |_, _, _, cx| {
                    let out = Outgoing { text: text.clone(), chips: Vec::new() };
                    App::defer(cx, move |cx| chat::send(out, cx));
                })),
        );
    }

    let input = m.chat_input.clone();
    let picker_el = picker_list("chat-picker", RectId::ChatPickerItem, &input, k, cx).map(|list| list.mx_2());

    div()
        .id("chat-panel")
        .relative()
        .children(rects::recorder(RectId::ChatPanel))
        .w(px(PANEL_WIDTH))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .gap_1()
        .bg(k.bg)
        .text_color(k.text)
        .text_size(px(12.))
        .border_l_1()
        .border_color(k.border)
        .child(header)
        .child(list)
        .child(quick)
        .children(picker_el)
        .child(div().px_2().pb_2().child(input))
        .into_any_element()
}

fn rail(pane: PaneId, unread: bool, k: &Colors, cx: &mut Context<Workspace>) -> AnyElement {
    div()
        .id("chat-rail")
        .relative()
        .children(rects::recorder(RectId::ChatRail))
        .w(px(RAIL_WIDTH))
        .h_full()
        .flex_none()
        .border_l_1()
        .border_color(k.border)
        .bg(k.bg)
        .flex()
        .flex_col()
        .items_center()
        .pt_2()
        .cursor_pointer()
        .text_color(k.ai)
        .child("◎")
        .children(unread.then(|| div().mt_1().size(px(7.)).rounded_full().bg(k.red)))
        .on_click(cx.listener(move |ws, _, _, cx| ws.monitor_chat_expand(pane, cx)))
        .into_any_element()
}

/// The wall with the chat beside it (or a rail, or an overlay). The panel and the overlay are siblings of the wall,
/// never inside it: the wall's key handler (arrows, space, ⏎, `s`, `a`) must not see what is typed in the input.
pub fn render(ws: &Workspace, pane: PaneId, m: &MonitorPane, wall: AnyElement, window: &Window, cx: &mut Context<Workspace>) -> AnyElement {
    let k = Colors::new(&crate::theme::current(cx).ui);
    let view = chat::view(cx);
    let base = window.text_style().font();
    let no_cards = MonitorModel::default();
    let wall_model = ws.monitor_model().unwrap_or(&no_cards);
    let width = m.width.clone();
    let weak = cx.entity().downgrade();
    // A refresh asked for while drawing is dropped by gpui: the crossing is handled on the next frame, which also
    // redraws (one extra frame per crossing; a frame drawn as wide before the first measurement crosses only when
    // the tab is narrow).
    let measure = canvas(
        move |bounds, window, _| {
            let w = f32::from(bounds.size.width);
            let was = width.replace(w);
            if is_narrow(was) != is_narrow(w) {
                window.on_next_frame(move |window, cx| {
                    let _ = weak.update(cx, |ws, cx| ws.monitor_chat_width_crossed(pane, is_narrow(w), window, cx));
                });
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full();
    let mode = panel_mode(m.width.get(), m.ui.chat_collapsed, m.ui.chat_open);
    let narrow = is_narrow(m.width.get());
    if view.unread && mode != PanelMode::Rail {
        App::defer(cx, chat::mark_read);
    }
    let side = match mode {
        PanelMode::Panel => panel(pane, m, &view, wall_model, &base, narrow, &k, cx),
        PanelMode::Rail | PanelMode::Overlay => rail(pane, view.unread, &k, cx),
    };
    let overlay = (mode == PanelMode::Overlay)
        .then(|| div().absolute().top_0().bottom_0().right(px(RAIL_WIDTH)).occlude().shadow_lg().child(panel(pane, m, &view, wall_model, &base, narrow, &k, cx)).into_any_element());
    div()
        .size_full()
        .relative()
        .flex()
        .flex_row()
        .child(measure)
        .child(div().flex_1().min_w(px(0.)).h_full().child(wall))
        .child(side)
        .children(overlay)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_tabs_get_a_rail_and_an_overlay() {
        assert_eq!(panel_mode(0.0, false, false), PanelMode::Panel, "before the first frame: wide");
        assert_eq!(panel_mode(1200.0, false, false), PanelMode::Panel);
        assert_eq!(panel_mode(1200.0, true, false), PanelMode::Rail, "⇥");
        assert_eq!(panel_mode(759.0, false, false), PanelMode::Rail);
        assert_eq!(panel_mode(759.0, false, true), PanelMode::Overlay);
        assert_eq!(panel_mode(760.0, false, true), PanelMode::Panel, "760 is wide");
        assert!(!is_narrow(0.0) && is_narrow(759.0) && !is_narrow(760.0));
    }

    #[test]
    fn answers_follow_down_only_from_the_bottom() {
        assert!(stick_to_bottom(0.0, 0.0, false), "nothing to scroll yet");
        assert!(stick_to_bottom(-300.0, 300.0, false), "at the bottom");
        assert!(stick_to_bottom(-297.0, 300.0, false), "within a few px");
        assert!(!stick_to_bottom(-120.0, 300.0, false), "scrolled up to read: stays");
        assert!(stick_to_bottom(-120.0, 300.0, true), "a new question goes down");
    }

    #[test]
    fn picker_highlight_clamps_to_the_filtered_list() {
        assert_eq!(picker_highlight(0, 0), None, "nothing matches: no row");
        assert_eq!(picker_highlight(1, 3), Some(1));
        assert_eq!(picker_highlight(4, 2), Some(1), "the filter shrank the list under the selection");
    }

    #[test]
    fn debug_chat_lists_messages_tools_links_and_chips() {
        use crate::monitor::chat::{ChatView, ProcessInfo};
        use crate::monitor::chat_model::Conversation;
        use gilvt_monitor::chat::{ChatEvent, TurnEnd};
        use serde_json::json;

        let mut conv = Conversation::default();
        conv.push_user(&Outgoing { text: "生成站会简报".into(), chips: vec![("pane:3".into(), "zsh".into())] });
        let none = |_: &str| None;
        for e in [
            ChatEvent::TurnStarted,
            ChatEvent::ToolStarted { id: "t1".into(), tool: "list_sessions".into(), args: json!({}) },
            ChatEvent::ToolDone { id: "t1".into(), ok: true, text: "{\"gilvt_label\":\"已列出 2 个会话\"}".into() },
            ChatEvent::Text { message: "m".into(), delta: "### 要你处理\n- [zsh](gilvt://session/pane:3)：看一下\n### 整体\n还好。".into() },
            ChatEvent::TurnEnded(TurnEnd::Done),
        ] {
            conv.apply(&e, &none);
        }
        let view = ChatView { revision: 1, conv, provider: "claude", provider_label: "Claude", model: String::new(), unread: false, process: ProcessInfo::default() };
        let mut draft = Draft::default();
        draft.add_chip("agent:codex:w1".into(), "web-login".into());
        draft.insert("继续");
        let rect = |id: RectId| match id {
            RectId::ChatLink(1, 0) => Some([1.0, 2.0, 3.0, 4.0]),
            _ => None,
        };
        let s = debug_chat(&view, PanelMode::Panel, &draft, &[], &rect);
        assert_eq!((s.status, s.provider, s.collapsed), ("idle", "claude", false));
        assert_eq!(s.scope, ["agent:codex:w1"]);
        assert_eq!(s.input.text, "继续");
        assert_eq!(s.input.chips[0].label, "web-login");
        assert_eq!(s.messages[0].role, "user");
        assert_eq!(s.messages[0].chips, ["pane:3"]);
        assert_eq!(s.messages[1].tools[0].label, "✓ 已列出 2 个会话");
        assert_eq!(s.messages[1].links[0].key, "pane:3");
        assert_eq!(s.messages[1].links[0].rect, Some([1.0, 2.0, 3.0, 4.0]));
        assert_eq!(s.quick.iter().map(|q| q.label.as_str()).collect::<Vec<_>>(), ["✦ 生成站会简报", "哪些需要我？", "有什么出错了？"]);
        assert!(s.picker.is_none());
        let rail = debug_chat(&view, PanelMode::Rail, &draft, &[], &rect);
        assert!(rail.collapsed && !rail.open);
        let overlay = debug_chat(&view, PanelMode::Overlay, &draft, &[], &rect);
        assert!(overlay.collapsed && overlay.open);
    }
}
