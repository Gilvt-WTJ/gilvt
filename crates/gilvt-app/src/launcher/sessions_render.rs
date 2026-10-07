//! Draws the 会话 palette (mockup m3c-sessions.html, option A): the query box with its chips, the pick line,
//! the list (section headers and rows), the confirm bar or the key hints, and the row menu. Colors derive
//! from the terminal palette like the ⌘P palette's. Also the query box's text input (IME).

use std::ops::Range;

use gpui::{
    anchored, canvas, deferred, div, prelude::*, px, AnyElement, Bounds, ClickEvent, Context, ElementInputHandler, FontWeight,
    Hsla, MouseButton, MouseDownEvent, Pixels, Point, SharedString, UTF16Selection, Window,
};

use super::sessions_model::{effective_scope, empty_message, menu_items, picks_text, Click, Line, Row, Scope};
use super::sessions_view::{SessionsEvent, SessionsView, CLEANUP_LABEL};
use super::History;
use crate::debug_state::rects::{self, RectId};
use crate::actions::SESSIONS_CONTEXT;
use crate::terminal_view::{search_paste_text, TerminalView};
use crate::theme::{hsla, mix};

const ROW_HEIGHT: f32 = 42.;
/// Lines visible before the list scrolls.
const VISIBLE_LINES: usize = 12;

/// The palette's colors (also the 新建 Agent panel's).
#[derive(Clone, Copy)]
pub(super) struct Colors {
    pub(super) text: Hsla,
    pub(super) muted: Hsla,
    pub(super) rule: Hsla,
    pub(super) bar: Hsla,
    pub(super) cursor: Hsla,
    pub(super) picked: Hsla,
    pub(super) accent: Hsla,
    pub(super) chip: Hsla,
    pub(super) chip_on: Hsla,
    pub(super) danger: Hsla,
    pub(super) warn: Hsla,
    pub(super) claude: Hsla,
    pub(super) codex: Hsla,
    pub(super) codex_text: Hsla,
    pub(super) menu_bg: Hsla,
    pub(super) menu_border: Hsla,
}

impl Colors {
    pub(super) fn new(window: &Window, cx: &gpui::App) -> Colors {
        let p = TerminalView::palette(window, cx);
        let theme = crate::theme::current(cx);
        let ui = &theme.ui;
        Colors {
            text: hsla(p.foreground),
            muted: hsla(mix(p.foreground, p.background, 0.45)),
            rule: hsla(mix(p.background, p.foreground, 0.15)),
            bar: hsla(mix(p.background, p.foreground, 0.06)),
            cursor: hsla(mix(p.background, p.foreground, 0.12)),
            picked: hsla(mix(p.background, p.ansi[4], 0.14)),
            accent: hsla(p.ansi[4]),
            chip: hsla(mix(p.background, p.foreground, 0.08)),
            chip_on: hsla(mix(p.background, p.ansi[4], 0.22)),
            danger: hsla(p.ansi[1]),
            warn: hsla(p.ansi[1]),
            claude: hsla(ui.claude),
            codex: hsla(ui.codex),
            codex_text: hsla(ui.codex_on),
            menu_bg: hsla(ui.raised),
            menu_border: hsla(ui.border_strong),
        }
    }
}

impl SessionsView {
    fn query_box(&self, k: Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let focus = self.focus_handle.clone();
        let caret = div().flex_none().w(px(1.5)).h(px(16.)).bg(k.accent);
        let text = if self.query.is_empty() && self.marked.is_none() {
            div().flex().items_center().child(caret.mr(px(4.))).child(div().text_color(k.muted).child("搜索会话（标题、首条提示词、项目）"))
        } else {
            div().flex().items_center().child(self.query.clone()).children(self.marked.clone().map(|m| div().underline().child(m))).child(caret)
        };
        let scope = effective_scope(self.filters, self.current.as_ref());
        let chip = |id: &'static str, n: usize, label: SharedString, on: bool| {
            div()
                .id(id)
                .relative()
                .children(rects::recorder(RectId::PaletteChip(n)))
                .flex_none()
                .px(px(7.))
                .py(px(1.))
                .rounded(px(10.))
                .text_size(px(11.))
                .bg(if on { k.chip_on } else { k.chip })
                .text_color(if on { k.accent } else { k.muted })
                .child(label)
        };
        let project = self.current.as_ref().map(|c| {
            chip("chip-project", 0, c.name.clone().into(), scope == Scope::Current)
                .on_click(cx.listener(|v, _, _, cx| v.set_filters(|f| f.scope = Scope::Current, cx)))
        });
        let refreshing = cx.try_global::<History>().is_some_and(History::refreshing);
        div()
            .relative()
            .flex_none()
            .h(px(38.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .text_size(px(14.))
            // IME: typed text reaches `replace_text_in_range` through this input handler.
            .child(
                canvas(|_, _, _| {}, move |bounds, _, window, cx| {
                    window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                })
                .absolute()
                .size_full(),
            )
            .child(div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().child(text))
            .when(refreshing, |d| d.child(div().flex_none().text_size(px(11.)).text_color(k.muted).child("刷新中…")))
            .children(project)
            .child(
                chip("chip-all", 1, "全部项目".into(), scope == Scope::All)
                    .on_click(cx.listener(|v, _, _, cx| v.set_filters(|f| f.scope = Scope::All, cx))),
            )
            .child(
                chip("chip-stale", 2, "≥ 7 天未活动".into(), self.filters.stale)
                    .on_click(cx.listener(|v, _, _, cx| v.set_filters(|f| f.stale = !f.stale, cx))),
            )
            .child(
                chip("chip-archived", 3, "已归档".into(), self.filters.archived)
                    .on_click(cx.listener(|v, _, _, cx| v.set_filters(|f| f.archived = !f.archived, cx))),
            )
    }

    fn render_lines(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let k = Colors::new(window, cx);
        let boxes = self.filters.stale || self.selection.picking();
        range
            .filter_map(|ix| self.list.lines.get(ix).cloned())
            .map(|line| match line {
                Line::Header(title) => div()
                    .h(px(ROW_HEIGHT))
                    .px_3()
                    .flex()
                    .items_end()
                    .pb(px(4.))
                    .text_size(px(11.))
                    .text_color(k.muted)
                    .child(title)
                    .into_any_element(),
                Line::Row(i) => self.render_row(i, boxes, k, cx),
            })
            .collect()
    }

    fn render_row(&self, i: usize, boxes: bool, k: Colors, cx: &mut Context<Self>) -> AnyElement {
        let r: &Row = &self.list.rows[i];
        let picked = self.selection.is_picked(&r.key);
        let bg = if i == self.selection.cursor() { Some(k.cursor) } else if picked { Some(k.picked) } else { None };
        let (icon_bg, icon_text) = if r.letter == 'C' { (k.claude, gpui::white()) } else { (k.codex, k.codex_text) };
        let checkbox = boxes.then(|| {
            let b = div().id(("session-pick", i)).flex_none().size(px(12.)).rounded(px(3.));
            match (r.selectable(), picked) {
                (false, _) => b,
                (true, true) => b.bg(k.accent).border_1().border_color(k.accent),
                (true, false) => b.border_1().border_color(k.muted),
            }
            .on_click(cx.listener(move |v, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                v.click(i, Click::Toggle, 1, cx);
            }))
        });
        let renaming = self.renaming.as_ref().filter(|rn| rn.key == r.key);
        let name = match renaming {
            Some(rn) => div().flex_1().min_w(px(0.)).child(rn.field.clone()),
            None => div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(div().truncate().child(r.title.clone()))
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(k.muted)
                        .when(r.dir_missing, |d| d.line_through())
                        .child(r.subtitle()),
                ),
        };
        let missing = r.dir_missing.then(|| div().flex_none().text_size(px(10.)).text_color(k.warn).child("目录已不存在"));
        let archived = r.archived.then(|| div().flex_none().text_size(px(10.)).text_color(k.muted).child("已归档"));
        let right = div().text_color(if r.live.is_some() { k.accent } else { k.muted }).child(r.right());
        div()
            .id(("session-row", i))
            .relative()
            .children(rects::recorder(RectId::PaletteRow(i)))
            .w_full()
            .h(px(ROW_HEIGHT))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .when_some(bg, |d, bg| d.bg(bg))
            .children(checkbox)
            .child(
                div()
                    .flex_none()
                    .size(px(16.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(icon_bg)
                    .text_color(icon_text)
                    .text_size(px(9.))
                    .font_weight(FontWeight::BOLD)
                    .child(r.letter.to_string()),
            )
            .child(name)
            .children(missing)
            .children(archived)
            .child(right.flex_none().text_size(px(11.)).whitespace_nowrap())
            // Clicks in the rename field stay there.
            .when(renaming.is_none(), |d| {
                d.on_click(cx.listener(move |v, e: &ClickEvent, _, cx| {
                    v.click(i, Click::from_modifiers(e.modifiers()), e.click_count(), cx);
                }))
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    v.open_menu(i, e.position, cx);
                }),
            )
            .into_any_element()
    }

    fn render_menu(&self, k: Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let (row, at) = (menu.row, menu.at);
        let mut list = div()
            .id("sessions-menu")
            .occlude()
            .min_w(px(170.))
            .py(px(4.))
            .rounded(px(7.))
            .border_1()
            .border_color(k.menu_border)
            .bg(k.menu_bg)
            .shadow_lg()
            .text_size(px(12.5))
            .text_color(k.text)
            .on_mouse_down_out(cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                // The right click that opens a menu also reaches the one it replaces.
                if v.menu.as_ref().is_some_and(|m| m.at != e.position) {
                    v.menu = None;
                    cx.notify();
                }
            }));
        for (n, (item, label, enabled)) in menu_items(self.menu_trashable(row), self.filters.archived).into_iter().enumerate() {
            let last = n == 7;
            if last {
                list = list.child(div().my(px(3.)).h(px(1.)).bg(k.rule));
            }
            let color = match (enabled, last) {
                (false, _) => k.muted,
                (true, true) => k.danger,
                (true, false) => k.text,
            };
            list = list.child(
                div()
                    .id(("sessions-menu-item", n))
                    .relative()
                    .children(rects::recorder(RectId::PaletteMenuItem(n)))
                    .mx(px(4.))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(4.))
                    .text_color(color)
                    .child(label)
                    .when(enabled, |d| {
                        d.hover(|s| s.bg(k.accent).text_color(gpui::white()))
                            .on_click(cx.listener(move |v, _, window, cx| v.menu_command(item, row, window, cx)))
                    }),
            );
        }
        Some(deferred(anchored().position(at).snap_to_window_with_margin(px(8.)).child(list)).with_priority(1).into_any_element())
    }

    fn bottom(&self, k: Colors, cx: &mut Context<Self>) -> AnyElement {
        let base = div().flex().flex_none().items_center().px_3().py(px(6.)).bg(k.bar).text_size(px(12.));
        if let Some(confirm) = &self.confirm {
            let button = |id: &'static str, n: usize, label: &'static str| {
                div()
                    .id(id)
                    .relative()
                    .children(rects::recorder(RectId::PaletteConfirmButton(n)))
                    .flex_none()
                    .px(px(10.))
                    .py(px(2.))
                    .rounded(px(6.))
                    .border_1()
                    .bg(k.menu_bg)
                    .child(label)
            };
            return base
                .gap_2()
                .child(div().flex_1().min_w(px(0.)).child(confirm.text.clone()))
                .child(button("confirm-cancel", 0, "取消").border_color(k.rule).on_click(cx.listener(|v, _, _, cx| v.cancel_confirm(cx))))
                .child(
                    button("confirm-trash", 1, "移到废纸篓")
                        .border_color(k.danger)
                        .text_color(k.danger)
                        .on_click(cx.listener(|v, _, _, cx| v.trash_confirmed(cx))),
                )
                .into_any_element();
        }
        let hint = |key: &'static str, what: &'static str| {
            div().flex().gap(px(3.)).child(div().text_color(k.text).font_weight(FontWeight::SEMIBOLD).child(key)).child(what)
        };
        base.flex_wrap()
            .gap_x_3()
            .text_size(px(11.))
            .text_color(k.muted)
            .child(hint("↩", "恢复（空闲 shell 里就地，否则新标签）"))
            .child(hint("⌘↩", "右侧"))
            .child(hint("⌘⇧↩", "下方"))
            .child(hint("⌘R", "重命名"))
            .child(hint("⌘⌫", "移到废纸篓"))
            .child(div().child("右键：更多"))
            .child(
                div()
                    .id("sessions-cleanup")
                    .relative()
                    .children(rects::recorder(RectId::PaletteCleanup))
                    .ml_auto()
                    .text_color(k.accent)
                    .child(CLEANUP_LABEL)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SessionsEvent::OpenCleanup))),
            )
            .into_any_element()
    }
}

impl Render for SessionsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(window, cx);
        let k = Colors::new(window, cx);
        let body = if self.list.lines.is_empty() {
            let (total, refreshing) = cx.try_global::<History>().map_or((0, false), |h| (h.entries().len(), h.refreshing()));
            let text = empty_message(total, refreshing, &self.query, self.filters, self.current.as_ref());
            div().h(px(ROW_HEIGHT * 2.)).flex().items_center().justify_center().text_color(k.muted).child(text).into_any_element()
        } else {
            gpui::uniform_list("sessions-lines", self.list.lines.len(), cx.processor(Self::render_lines))
                .track_scroll(self.scroll.clone())
                .h(px(ROW_HEIGHT * self.list.lines.len().min(VISIBLE_LINES) as f32))
                .into_any_element()
        };
        let picks = (self.filters.stale || self.selection.picking()).then(|| {
            let (n, bytes) = self.picks();
            div().flex_none().px_3().pt(px(6.)).text_size(px(11.)).text_color(k.muted).child(picks_text(n, bytes))
        });
        let banner = self.banner.clone().map(|text| {
            let p = TerminalView::palette(window, cx);
            div().flex_none().px_3().py_1().text_size(px(12.)).bg(hsla(mix(p.background, p.ansi[1], 0.25))).child(text)
        });
        let root = div().id("sessions").key_context(SESSIONS_CONTEXT).track_focus(&self.focus_handle);
        self.listeners(root, cx)
            .flex()
            .flex_col()
            .bg(hsla(TerminalView::palette(window, cx).background))
            .text_color(k.text)
            .text_size(px(13.))
            .children(banner)
            .child(self.query_box(k, cx))
            .child(div().h(px(1.)).bg(k.rule))
            .children(picks)
            .child(div().py_1().child(body))
            .child(div().h(px(1.)).bg(k.rule))
            .child(self.bottom(k, cx))
            .children(self.render_menu(k, cx))
    }
}

impl gpui::EntityInputHandler for SessionsView {
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
        let typed = search_paste_text(text);
        if typed.is_empty() {
            cx.notify();
        } else {
            let query = self.query.clone() + &typed;
            self.set_query(query, cx);
        }
    }

    fn replace_and_mark_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    fn bounds_for_range(&mut self, _: Range<usize>, element: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        // The candidate window opens below the query box.
        Some(element)
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}
