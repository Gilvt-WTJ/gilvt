//! Draws the cleanup wizard (mockup session-rows.html, 「清理向导」): the title line, the four presets on the
//! left with 「N 个 · X MB」, the open preset's sessions on the right (box, title, directory, size, when;
//! 📌 for pinned, 「尚未 Review」 for unreviewed), and at the bottom the summary with 归档 / 移到废纸篓 or the
//! confirm bar. Colors are the 会话 palette's.

use std::ops::Range;

use gpui::{
    div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, SharedString, Window,
};

use super::model::{preset_count, Action, Column, Preset};
use super::view::CleanupWizard;
use crate::actions::SESSIONS_CONTEXT;
use crate::debug_state::rects::{self, RectId};
use crate::launcher::sessions_model::size_label;
use crate::launcher::sessions_render::Colors;
use crate::terminal_view::TerminalView;
use crate::theme::{hsla, mix};

const ROW_HEIGHT: f32 = 42.;
/// Rows visible before the preview scrolls.
const VISIBLE_ROWS: usize = 9;
const PRESETS_WIDTH: f32 = 250.;

impl CleanupWizard {
    fn presets(&self, k: Colors, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.model.column == Column::Presets;
        div()
            .flex_none()
            .w(px(PRESETS_WIDTH))
            .py_1()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(k.rule)
            .children(Preset::ALL.iter().enumerate().map(|(n, p)| {
                let active = n == self.model.preset;
                let count = preset_count(self.outcomes.as_ref().and_then(|o| o.get(n)));
                div()
                    .id(("cleanup-preset", n))
                    .relative()
                    .children(rects::recorder(RectId::CleanupPreset(n)))
                    .mx_1()
                    .px_2()
                    .py(px(5.))
                    .rounded(px(6.))
                    .when(active, |d| d.bg(if focused { k.chip_on } else { k.cursor }))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(if active {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .child(p.label()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.))
                                    .text_color(k.muted)
                                    .child(p.hint()),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(k.muted)
                            .whitespace_nowrap()
                            .child(count),
                    )
                    .on_click(cx.listener(move |w, _: &ClickEvent, _, cx| w.click_preset(n, cx)))
            }))
            .into_any_element()
    }

    fn render_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let k = Colors::new(window, cx);
        let Some(outcome) = self.current().cloned() else {
            return Vec::new();
        };
        let focused = self.model.column == Column::Preview;
        range
            .filter_map(|n| {
                let h = outcome.hits.get(n)?;
                let r = self.rows.get(h.ix)?;
                let picked = self.model.is_picked(h.ix);
                let checkbox = {
                    let b = div().flex_none().size(px(12.)).rounded(px(3.));
                    if picked {
                        b.bg(k.accent).border_1().border_color(k.accent)
                    } else {
                        b.border_1().border_color(k.muted)
                    }
                };
                let title = if h.pinned {
                    format!("📌 {}", r.title)
                } else {
                    r.title.clone()
                };
                let mut subtitle = if crate::i18n::english() {
                    format!("{} · {} turns · {}", r.dir, r.turns, size_label(h.bytes))
                } else {
                    format!("{} · {} 轮 · {}", r.dir, r.turns, size_label(h.bytes))
                };
                if h.pinned {
                    subtitle.push_str(crate::i18n::text(
                        " · 置顶，默认不选",
                        " · pinned, not selected by default",
                    ));
                }
                let tag = |text: &'static str, color| {
                    div()
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(color)
                        .child(text)
                };
                Some(
                    div()
                        .id(("cleanup-row", n))
                        .relative()
                        .children(rects::recorder(RectId::CleanupRow(n)))
                        .w_full()
                        .h(px(ROW_HEIGHT))
                        .px_3()
                        .flex()
                        .items_center()
                        .gap_2()
                        .when(focused && n == self.model.cursor, |d| d.bg(k.cursor))
                        .child(checkbox)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .child(div().truncate().child(title))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(11.))
                                        .text_color(k.muted)
                                        .when(r.dir_missing, |d| d.line_through())
                                        .child(subtitle),
                                ),
                        )
                        .children(r.dir_missing.then(|| {
                            tag(
                                crate::i18n::text("目录已不存在", "Directory missing"),
                                k.warn,
                            )
                        }))
                        .children(h.unreviewed.then(|| {
                            tag(crate::i18n::text("尚未 Review", "Not reviewed"), k.accent)
                        }))
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(k.muted)
                                .whitespace_nowrap()
                                .child(r.when.clone()),
                        )
                        .on_click(cx.listener(move |w, _: &ClickEvent, _, cx| w.click_row(n, cx)))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn preview(&self, k: Colors, cx: &mut Context<Self>) -> AnyElement {
        let empty = |text: &'static str| {
            div()
                .flex_1()
                .h(px(ROW_HEIGHT * 2.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(k.muted)
                .child(text)
                .into_any_element()
        };
        let Some(outcome) = self.current() else {
            return empty(crate::i18n::text("计算中…", "Calculating…"));
        };
        if outcome.hits.is_empty() {
            return empty(crate::i18n::text("没有命中的会话", "No matching sessions"));
        }
        let n = outcome.hits.len();
        div()
            .flex_1()
            .min_w(px(0.))
            .py_1()
            .child(
                gpui::uniform_list("cleanup-rows", n, cx.processor(Self::render_rows))
                    .track_scroll(self.scroll.clone())
                    .h(px(ROW_HEIGHT * n.min(VISIBLE_ROWS) as f32)),
            )
            .into_any_element()
    }

    fn bottom(&self, k: Colors, cx: &mut Context<Self>) -> AnyElement {
        let base = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_3()
            .py(px(6.))
            .bg(k.bar)
            .text_size(px(12.));
        let button = |id: SharedString, rect: RectId, label: String| {
            div()
                .id(id)
                .relative()
                .children(rects::recorder(rect))
                .flex_none()
                .px(px(10.))
                .py(px(2.))
                .rounded(px(6.))
                .border_1()
                .bg(k.menu_bg)
                .child(label)
        };
        if let Some(confirm) = &self.confirm {
            return base
                .child(div().flex_1().min_w(px(0.)).child(confirm.text.clone()))
                .child(
                    button(
                        "cleanup-confirm-cancel".into(),
                        RectId::CleanupConfirmButton(0),
                        crate::i18n::text("取消", "Cancel").into(),
                    )
                    .border_color(k.rule)
                    .on_click(cx.listener(|w, _, _, cx| w.cancel_confirm(cx))),
                )
                .child(
                    button(
                        "cleanup-confirm-trash".into(),
                        RectId::CleanupConfirmButton(1),
                        crate::i18n::text("移到废纸篓", "Move to Trash").into(),
                    )
                    .border_color(k.danger)
                    .text_color(k.danger)
                    .on_click(cx.listener(|w, _, _, cx| w.trash_confirmed(cx))),
                )
                .into_any_element();
        }
        let summary = self
            .model
            .summary(&self.current().cloned().unwrap_or_default());
        let mut bar = base.child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_color(k.muted)
                .child(summary),
        );
        for (n, (action, label, enabled)) in self.buttons().into_iter().enumerate() {
            let emphasis = action == self.model.action;
            let color = match (enabled, action) {
                (false, _) => k.muted,
                (true, Action::Trash) => k.danger,
                (true, Action::Archive) => k.accent,
            };
            let el = button(
                format!("cleanup-button-{n}").into(),
                RectId::CleanupButton(n),
                label,
            )
            .border_color(if enabled && emphasis { color } else { k.rule })
            .text_color(color)
            .when(emphasis, |d| d.font_weight(FontWeight::SEMIBOLD))
            .when(enabled, |d| {
                d.on_click(cx.listener(move |w, _, _, cx| w.act(action, cx)))
            });
            bar = bar.child(el);
        }
        bar.into_any_element()
    }
}

impl Render for CleanupWizard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = Colors::new(window, cx);
        let banner = self.banner.clone().map(|text| {
            let p = TerminalView::palette(window, cx);
            div()
                .flex_none()
                .px_3()
                .py_1()
                .text_size(px(12.))
                .bg(hsla(mix(p.background, p.ansi[4], 0.2)))
                .child(text)
        });
        let title = div()
            .flex_none()
            .h(px(32.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(crate::i18n::text("清理…", "Clean Up…")),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(k.muted)
                    .child(crate::i18n::text(
                        "⌘⇧K · Esc 返回 · →/Tab 预览 · Space 勾选",
                        "⇧⌘K · Esc back · →/Tab preview · Space select",
                    )),
            );
        let root = div()
            .id("cleanup-wizard")
            .key_context(SESSIONS_CONTEXT)
            .track_focus(&self.focus_handle);
        self.listeners(root, cx)
            .flex()
            .flex_col()
            .bg(hsla(TerminalView::palette(window, cx).background))
            .text_color(k.text)
            .text_size(px(13.))
            .children(banner)
            .child(title)
            .child(div().h(px(1.)).bg(k.rule))
            .child(
                div()
                    .flex()
                    .min_h(px(ROW_HEIGHT * 4.))
                    .child(self.presets(k, cx))
                    .child(self.preview(k, cx)),
            )
            .child(div().h(px(1.)).bg(k.rule))
            .child(self.bottom(k, cx))
    }
}
