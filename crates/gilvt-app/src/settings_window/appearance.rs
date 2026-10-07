//! The settings window's 「外观」 page: a search box, the 全部 / 深色 / 浅色 filter and the 固定 / 跟随系统 mode (with
//! the 浅色 / 深色 slots) on top, the theme list on the left, a preview of the highlighted theme on the right.
//! Choosing a theme (a click, ↑↓ or ⏎) applies it at once in every window and writes it into config.toml
//! (`config_file::set_theme`).

use std::ops::Range;

use gilvt_theme::{resolve, ResolvedTheme, Selection};
use gpui::{
    canvas, div, prelude::*, px, AnyElement, ClickEvent, Context, ElementInputHandler, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Point,
    ScrollStrategy, Subscription, UTF16Selection, UniformListScrollHandle, Window,
};

use super::appearance_model::{Filter, Mode, PickerModel, Slot};
use super::view::{segment_group, segment_item, Colors};
use crate::actions::{Paste, APPEARANCE_CONTEXT};
use crate::config_file::{self, ConfigFile};
use crate::debug_state::rects::{self, Rect4, RectId};
use crate::debug_state::{AppearanceState, ThemeChip, ThemeRow};
use crate::terminal_view::search_paste_text;
use crate::theme::{hsla, AppSettings, ThemeState};

const ROW_HEIGHT: f32 = 24.;
const VISIBLE_ROWS: usize = 13;
const LIST_WIDTH: f32 = 240.;
/// DebugState lists this many rows at most.
const MAX_ROWS: usize = 50;

pub struct AppearancePage {
    focus_handle: FocusHandle,
    model: PickerModel,
    /// IME composition, shown after the query.
    marked: Option<String>,
    scroll: UniformListScrollHandle,
    /// Why a choice was refused (config.toml does not parse).
    notice: Option<String>,
    /// The highlighted row's resolved theme, for the preview.
    shown: Option<(String, ResolvedTheme)>,
    _observers: [Subscription; 2],
}

impl Focusable for AppearancePage {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl AppearancePage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let state = cx.global::<ThemeState>();
        let entries = gilvt_theme::list_themes(state.user_dir());
        let model = PickerModel::open(entries, &state.selection().clone(), crate::theme::system_dark(cx));
        let scroll = UniformListScrollHandle::new();
        scroll.scroll_to_item(model.selected(), ScrollStrategy::Center);
        let observers = [
            // config.toml changed `theme` / `[colors]` (or a choice was refused): show what is in use.
            cx.observe_global::<ThemeState>(|page: &mut Self, cx| {
                page.model.adopt(&cx.global::<ThemeState>().selection().clone());
                page.shown = None;
                cx.notify();
            }),
            cx.observe_global::<ConfigFile>(|_, cx| cx.notify()),
        ];
        AppearancePage { focus_handle: cx.focus_handle(), model, marked: None, scroll, notice: None, shown: None, _observers: observers }
    }

    #[cfg(test)]
    pub fn model(&self) -> &PickerModel {
        &self.model
    }

    #[cfg(test)]
    pub fn model_mut(&mut self) -> &mut PickerModel {
        &mut self.model
    }

    /// A click on row `row`.
    #[cfg(test)]
    pub fn choose_for_test(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.choose(row, window, cx);
    }

    /// DebugState `settings.appearance`; `at` turns a rect id into window coordinates.
    pub fn debug_state(&self, at: &dyn Fn(RectId) -> Option<Rect4>, cx: &gpui::App) -> AppearanceState {
        state(&self.model, cx.global::<ThemeState>().overrides().count(), at)
    }

    /// Applies the model's selection (the page's choice): in every window at once, into config.toml soon after.
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.scroll.scroll_to_item(self.model.selected(), ScrollStrategy::Center);
        match config_file::set_theme(self.model.selection(), cx) {
            Ok(()) => self.notice = None,
            Err(e) => {
                self.notice = Some(e);
                // Refused: the page goes back to the theme in use.
                self.model.adopt(&cx.global::<ThemeState>().selection().clone());
            }
        }
        // This window is the one being updated: `refresh_all` could not reach it.
        crate::native::apply_appearance(window, cx);
        window.refresh();
        cx.notify();
    }

    fn choose(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.model.choose(row);
        self.apply(window, cx);
    }

    fn set_query(&mut self, q: String, cx: &mut Context<Self>) {
        self.model.set_query(q);
        self.scroll.scroll_to_item(self.model.selected(), ScrollStrategy::Top);
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return };
        let query = self.model.query().to_string() + &search_paste_text(&text);
        self.set_query(query, cx);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return;
        }
        let m = event.keystroke.modifiers;
        let plain = !m.control && !m.alt && !m.platform && !m.shift;
        let readonly = config_file::readonly(cx);
        match event.keystroke.key.as_str() {
            "up" | "down" if plain && !readonly && self.model.row_count() > 0 => {
                self.model.step(event.keystroke.key == "down");
                self.apply(window, cx);
            }
            // After typing a query the highlighted row may not be the one in use yet.
            "enter" if plain && !readonly && self.model.row_count() > 0 => self.choose(self.model.selected(), window, cx),
            "escape" if !self.model.query().is_empty() => self.set_query(String::new(), cx),
            "backspace" if !m.platform && !m.control => {
                let mut q = self.model.query().to_string();
                if q.pop().is_some() {
                    self.set_query(q, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// The resolved theme of the highlighted row (cached by name).
    fn shown_theme(&mut self, cx: &gpui::App) -> Option<&ResolvedTheme> {
        let name = self.model.row(self.model.selected())?.name.clone();
        if self.shown.as_ref().is_none_or(|(n, _)| *n != name) {
            let state = cx.global::<ThemeState>();
            let t = resolve(&Selection::Fixed(name.clone()), state.overrides(), crate::theme::system_dark(cx), state.user_dir());
            self.shown = Some((name, t));
        }
        self.shown.as_ref().map(|(_, t)| t)
    }

    /// A segmented control: the selected item stands out in the accent (also in light themes).
    fn segmented<T: Copy + PartialEq + 'static>(
        &self,
        items: &[(T, &'static str, String)],
        current: T,
        disabled: bool,
        k: &Colors,
        cx: &Context<Self>,
        act: fn(&mut Self, T, &mut Window, &mut Context<Self>),
    ) -> AnyElement {
        let mut seg = segment_group(div().flex_none(), k);
        for &(value, id, ref label) in items {
            let on = value == current;
            seg = seg.child(
                div()
                    .id(id)
                    .relative()
                    .children(rects::recorder(RectId::ThemeChip(id)))
                    .px(px(10.))
                    .py(px(2.))
                    .map(|d| segment_item(d, on, !disabled, k))
                    .when(!disabled, |d| d.on_click(cx.listener(move |page, _: &ClickEvent, window, cx| act(page, value, window, cx))))
                    .child(label.clone()),
            );
        }
        seg.when(disabled, |d| d.opacity(0.5)).into_any_element()
    }

    fn render_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = crate::theme::current(cx);
        let k = Colors::new(&theme);
        let (selected_bg, hover_bg) = (hsla(theme.ui.selected), hsla(theme.ui.hover));
        let readonly = config_file::readonly(cx);
        range
            .filter_map(|ix| self.model.row(ix).cloned().map(|e| (ix, e)))
            .map(|(ix, e)| {
                let current = self.model.is_current(&e.name);
                div()
                    .id(("theme-row", ix))
                    .relative()
                    .children(rects::recorder(RectId::ThemeRow(ix)))
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded(px(4.))
                    .when(ix == self.model.selected(), |row| row.bg(selected_bg))
                    .when(ix != self.model.selected() && !readonly, |row| row.hover(|row| row.bg(hover_bg)))
                    .when(!readonly, |row| row.cursor_pointer().on_click(cx.listener(move |page, _: &ClickEvent, window, cx| page.choose(ix, window, cx))))
                    .when(readonly, |row| row.opacity(0.5))
                    .child(
                        div()
                            .flex_none()
                            .size(px(10.))
                            .rounded_full()
                            .border_1()
                            .border_color(k.field_border)
                            .bg(hsla(e.background)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(e.name.clone()),
                    )
                    .children(e.user.then(|| {
                        div()
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(k.muted)
                            .child(crate::i18n::text("用户", "User"))
                    }))
                    .children(current.then(|| div().flex_none().text_color(k.accent).child("✓")))
                    .into_any_element()
            })
            .collect()
    }

    fn preview(&mut self, k: &Colors, cx: &gpui::App) -> AnyElement {
        let mono: gpui::SharedString = cx.global::<AppSettings>().0.font_family.clone().into();
        let Some(t) = self.shown_theme(cx) else { return div().flex_1().into_any_element() };
        let p = &t.palette;
        let ui = &t.ui;
        let swatch = |c| div().flex_1().h(px(16.)).bg(hsla(c));
        let status = |label: &'static str, c| div().flex().items_center().gap(px(4.)).child(div().size(px(8.)).rounded_full().bg(hsla(c))).child(label);
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.))
            .rounded(px(6.))
            .border_1()
            .border_color(k.rule)
            .bg(hsla(p.background))
            .text_color(hsla(p.foreground))
            .child(div().flex().children(p.ansi[..8].iter().map(|c| swatch(*c))))
            .child(div().flex().children(p.ansi[8..].iter().map(|c| swatch(*c))))
            .child(
                div()
                    .font_family(mono)
                    .text_size(px(12.))
                    .child(div().child("$ ls -la"))
                    .child(div().text_color(hsla(p.ansi[4])).child("drwxr-xr-x  src/"))
                    .child(div().text_color(hsla(p.ansi[2])).child("+ added line"))
                    .child(div().text_color(hsla(p.ansi[1])).child("- removed line")),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(10.))
                    .p(px(6.))
                    .rounded(px(5.))
                    .bg(hsla(ui.panel))
                    .text_size(px(11.))
                    .child(div().text_color(hsla(ui.attention.fg)).child(status(
                        crate::i18n::text("需要你", "Needs you"),
                        ui.attention.ring,
                    )))
                    .child(
                        div()
                            .text_color(hsla(ui.error.fg))
                            .child(status(crate::i18n::text("出错", "Error"), ui.error.ring)),
                    )
                    .child(div().text_color(hsla(ui.running.fg)).child(status(
                        crate::i18n::text("执行中", "Running"),
                        ui.running.ring,
                    )))
                    .child(div().text_color(hsla(ui.done.fg)).child(status(
                        crate::i18n::text("完成未看", "Done, unseen"),
                        ui.done.ring,
                    ))),
            )
            .into_any_element()
    }

    fn search_box(&self, focused: bool, k: &Colors, cx: &Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let focus = self.focus_handle.clone();
        let caret = div().flex_none().w(px(1.5)).h(px(14.)).bg(k.accent);
        let row = div()
            .id("theme-search")
            .relative()
            .children(rects::recorder(RectId::ThemeSearch))
            .flex_1()
            .min_w(px(120.))
            .h(px(24.))
            .px(px(7.))
            .flex()
            .items_center()
            .rounded(px(5.))
            .border_1()
            .border_color(if focused { k.accent } else { k.field_border })
            .bg(k.field)
            .cursor_text()
            // IME: typed text reaches `replace_text_in_range` through this input handler.
            .child(
                canvas(|_, _, _| {}, move |bounds, _, window, cx| {
                    window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                })
                .absolute()
                .size_full(),
            );
        let row = if self.model.query().is_empty() && self.marked.is_none() {
            row.child(caret.mr(px(4.))).child(
                div()
                    .text_color(k.muted)
                    .child(crate::i18n::text("搜索主题", "Search themes")),
            )
        } else {
            row.child(self.model.query().to_string()).children(self.marked.clone().map(|m| div().underline().child(m))).child(caret)
        };
        row.into_any_element()
    }
}

/// The page as DebugState data.
pub fn state(m: &PickerModel, overrides: usize, at: &dyn Fn(RectId) -> Option<Rect4>) -> AppearanceState {
    let system = m.mode() == Mode::System;
    let (light, dark) = m.slot_names();
    AppearanceState {
        query: m.query().to_string(),
        filter: m.filter().id(),
        mode: m.mode().id(),
        slot: system.then(|| m.slot().id()),
        selected: m.row(m.selected()).map(|e| e.name.clone()),
        fixed: (!system).then(|| m.fixed_name().to_string()),
        light: system.then(|| light.to_string()),
        dark: system.then(|| dark.to_string()),
        rows: (0..m.row_count().min(MAX_ROWS))
            .filter_map(|i| m.row(i).map(|e| (i, e)))
            .map(|(i, e)| ThemeRow { name: e.name.clone(), user: e.user, current: m.is_current(&e.name), selected: i == m.selected(), rect: at(RectId::ThemeRow(i)) })
            .collect(),
        chips: chips(m).into_iter().map(|(id, on)| ThemeChip { id, on, rect: at(RectId::ThemeChip(id)) }).collect(),
        search_rect: at(RectId::ThemeSearch),
        colors_overrides: overrides,
    }
}

/// The filter / mode / slot controls and which are on; the slots exist only in system mode.
fn chips(m: &PickerModel) -> Vec<(&'static str, bool)> {
    let mut chips = vec![
        ("filter-all", m.filter() == Filter::All),
        ("filter-dark", m.filter() == Filter::Dark),
        ("filter-light", m.filter() == Filter::Light),
        ("mode-fixed", m.mode() == Mode::Fixed),
        ("mode-system", m.mode() == Mode::System),
    ];
    if m.mode() == Mode::System {
        chips.push(("slot-light", m.slot() == Slot::Light));
        chips.push(("slot-dark", m.slot() == Slot::Dark));
    }
    chips
}

impl gpui::EntityInputHandler for AppearancePage {
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
            let query = self.model.query().to_string() + &typed;
            self.set_query(query, cx);
        }
    }

    fn replace_and_mark_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    fn bounds_for_range(&mut self, _: Range<usize>, element: gpui::Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<gpui::Bounds<Pixels>> {
        // The candidate window opens below the search box.
        Some(element)
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

fn set_filter(page: &mut AppearancePage, f: Filter, _: &mut Window, cx: &mut Context<AppearancePage>) {
    page.model.set_filter(f);
    page.scroll.scroll_to_item(page.model.selected(), ScrollStrategy::Center);
    cx.notify();
}

fn set_mode(page: &mut AppearancePage, mode: Mode, window: &mut Window, cx: &mut Context<AppearancePage>) {
    page.model.set_mode(mode);
    page.apply(window, cx);
}

fn set_slot(page: &mut AppearancePage, slot: Slot, _: &mut Window, cx: &mut Context<AppearancePage>) {
    page.model.set_slot(slot);
    page.scroll.scroll_to_item(page.model.selected(), ScrollStrategy::Center);
    cx.notify();
}

impl Render for AppearancePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = crate::theme::current(cx);
        let k = Colors::new(&theme);
        let readonly = config_file::readonly(cx);
        let path = cx.global::<ConfigFile>().path.clone();
        let filters = [
            (
                Filter::All,
                "filter-all",
                crate::i18n::text("全部", "All").to_string(),
            ),
            (
                Filter::Dark,
                "filter-dark",
                crate::i18n::text("深色", "Dark").into(),
            ),
            (
                Filter::Light,
                "filter-light",
                crate::i18n::text("浅色", "Light").into(),
            ),
        ];
        let modes = [
            (
                Mode::Fixed,
                "mode-fixed",
                crate::i18n::text("固定", "Fixed").to_string(),
            ),
            (
                Mode::System,
                "mode-system",
                crate::i18n::text("跟随系统", "Follow System").into(),
            ),
        ];
        let (light, dark) = self.model.slot_names();
        let slots = if crate::i18n::current() == crate::i18n::Language::English {
            [
                (Slot::Light, "slot-light", format!("Light: {light}")),
                (Slot::Dark, "slot-dark", format!("Dark: {dark}")),
            ]
        } else {
            [
                (Slot::Light, "slot-light", format!("浅色：{light}")),
                (Slot::Dark, "slot-dark", format!("深色：{dark}")),
            ]
        };
        let filter_seg = self.segmented(&filters, self.model.filter(), false, &k, cx, set_filter);
        let mode_seg = self.segmented(&modes, self.model.mode(), readonly, &k, cx, set_mode);
        let slot_seg = (self.model.mode() == Mode::System).then(|| self.segmented(&slots, self.model.slot(), false, &k, cx, set_slot));
        let list = if self.model.row_count() == 0 {
            div()
                .h(px(ROW_HEIGHT * 2.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(k.muted)
                .child(crate::i18n::text("无匹配", "No matches"))
                .into_any_element()
        } else {
            gpui::uniform_list("theme-rows", self.model.row_count(), cx.processor(Self::render_rows))
                .track_scroll(self.scroll.clone())
                .h(px(ROW_HEIGHT * VISIBLE_ROWS as f32))
                .into_any_element()
        };
        let preview = self.preview(&k, cx);
        let overrides = cx.global::<ThemeState>().overrides().count();
        let focused = self.focus_handle.is_focused(window);
        div()
            .id("appearance-page")
            .key_context(APPEARANCE_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::paste))
            // A click anywhere on the page (the search box included) gives it the keyboard.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|page, _: &MouseDownEvent, window, _| {
                    if !page.focus_handle.is_focused(window) {
                        window.focus(&page.focus_handle);
                    }
                }),
            )
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(div().text_size(px(10.5)).text_color(k.muted).child(crate::i18n::text("主题", "Theme")))
            .child(div().flex().flex_wrap().items_center().gap(px(8.)).child(self.search_box(focused, &k, cx)).child(filter_seg).child(mode_seg))
            .children(slot_seg.map(|s| div().flex().items_center().gap(px(8.)).child(div().text_size(px(10.5)).text_color(k.muted).child(crate::i18n::text("正在编辑", "Editing"))).child(s)))
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .child(div().flex_none().w(px(LIST_WIDTH)).p(px(3.)).rounded(px(6.)).border_1().border_color(k.rule).bg(k.field).child(list))
                    .child(preview),
            )
            .children(self.notice.clone().map(|n| div().text_size(px(11.)).text_color(k.error.2).child(n)))
            .children((overrides > 0).then(|| {
                let text = if crate::i18n::current() == crate::i18n::Language::English {
                    format!("{overrides} [colors] override(s) are applied on top of the selected theme")
                } else {
                    format!("配置中有 {overrides} 项 [colors] 颜色覆盖，已叠加在所选主题上")
                };
                div().text_size(px(10.5)).text_color(k.muted).child(text)
            }))
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(k.muted)
                    .child(if crate::i18n::current() == crate::i18n::Language::English {
                        format!("Selections apply immediately to all windows and update the theme key in {}, preserving comments and formatting", path.display())
                    } else {
                        format!("选中即生效（所有窗口），并写入 {} 的 theme 键，保留你的注释和格式", path.display())
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use gilvt_theme::{color::rgb, ThemeEntry, GILVT_DARK, GILVT_LIGHT};

    use super::*;

    #[test]
    fn rows_chips_and_slots_as_data() {
        let e = |name: &str, dark: bool, user: bool| ThemeEntry { name: name.into(), user, dark, background: rgb(0) };
        let mut m = PickerModel::open(vec![e(GILVT_LIGHT, false, false), e(GILVT_DARK, true, false), e("Mine", true, true)], &Selection::system(), true);
        let rects: HashMap<RectId, Rect4> = [(RectId::ThemeRow(1), [10.0, 20.0, 300.0, 26.0]), (RectId::ThemeChip("slot-dark"), [1.0, 2.0, 3.0, 4.0])].into();
        let at = |id: RectId| rects.get(&id).copied();
        let s = state(&m, 2, &at);
        assert_eq!(s.rows.iter().map(|r| (r.name.as_str(), r.user, r.current, r.selected)).collect::<Vec<_>>(), [(GILVT_DARK, false, true, true), ("Mine", true, false, false)]);
        assert_eq!(s.rows[1].rect, Some([10.0, 20.0, 300.0, 26.0]));
        let ids: Vec<(&str, bool)> = s.chips.iter().map(|c| (c.id, c.on)).collect();
        assert_eq!(ids, [("filter-all", false), ("filter-dark", true), ("filter-light", false), ("mode-fixed", false), ("mode-system", true), ("slot-light", false), ("slot-dark", true)]);
        assert_eq!(s.chips[6].rect, Some([1.0, 2.0, 3.0, 4.0]));
        assert_eq!((s.mode, s.slot, s.fixed.as_deref()), ("system", Some("dark"), None));
        assert_eq!((s.light.as_deref(), s.dark.as_deref(), s.colors_overrides), (Some(GILVT_LIGHT), Some(GILVT_DARK), 2));
        m.set_mode(Mode::Fixed);
        let s = state(&m, 0, &at);
        assert!(s.chips.iter().all(|c| !c.id.starts_with("slot-")), "slots only in system mode");
        assert_eq!((s.slot, s.fixed.as_deref(), s.light.as_deref()), (None, Some(GILVT_DARK), None));
    }

    /// GUI case O9 relies on this order: searching `gilvt` lists gilvt Light, then gilvt Dark.
    #[test]
    fn searching_gilvt_lists_the_two_defaults_first() {
        let mut m = PickerModel::open(gilvt_theme::list_themes(None), &Selection::Fixed(GILVT_LIGHT.into()), false);
        m.set_query("gilvt".into());
        let names: Vec<_> = (0..m.row_count()).map(|i| m.row(i).unwrap().name.clone()).collect();
        assert_eq!(names[..2], [GILVT_LIGHT.to_string(), GILVT_DARK.to_string()], "{names:?}");
        assert_eq!(m.row(m.selected()).unwrap().name, GILVT_LIGHT);
    }

    #[test]
    fn rows_are_capped() {
        let entries = (0..80).map(|i| ThemeEntry { name: format!("T{i}"), user: false, dark: true, background: rgb(0) }).collect();
        let m = PickerModel::open(entries, &Selection::Fixed("T3".into()), true);
        assert_eq!(state(&m, 0, &|_| None).rows.len(), MAX_ROWS);
    }
}
