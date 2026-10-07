use std::ops::Range;

use gilvt_agent::ReviewOutcome;
use gpui::{
    canvas, div, prelude::*, px, AnyElement, Context, ElementInputHandler, FontWeight, Hsla,
    SharedString, Window,
};

use super::model::{Row, Tab};
use super::view::SessionCenterView;
use crate::actions::SESSIONS_CONTEXT;
use crate::debug_state::rects::{self, RectId};
use crate::review::{ReviewPriority, ReviewService, ReviewSort};
use crate::session_review::model::{layout as model_layout, Layout as ReviewLayout};
use crate::terminal_view::TerminalView;
use crate::theme::{hsla, mix};

const ROW_HEIGHT: f32 = 58.;
const VISIBLE_ROWS: usize = 9;

#[derive(Clone, Copy)]
pub(crate) struct Colors {
    pub(crate) dark: bool,
    pub(crate) background: Hsla,
    pub(crate) text: Hsla,
    pub(crate) muted: Hsla,
    pub(crate) rule: Hsla,
    pub(crate) hover: Hsla,
    pub(crate) selected: Hsla,
    pub(crate) accent: Hsla,
    pub(crate) danger: Hsla,
    pub(crate) warning: Hsla,
}

impl Colors {
    pub(crate) fn new(window: &Window, cx: &gpui::App) -> Self {
        let palette = TerminalView::palette(window, cx);
        let theme = crate::theme::current(cx);
        let dark = theme.dark;
        Colors {
            dark,
            background: hsla(palette.background),
            text: hsla(palette.foreground),
            muted: hsla(mix(palette.foreground, palette.background, 0.48)),
            rule: hsla(mix(palette.background, palette.foreground, 0.16)),
            hover: hsla(mix(palette.background, palette.foreground, 0.08)),
            selected: hsla(mix(
                palette.background,
                palette.ansi[4],
                if dark { 0.28 } else { 0.16 },
            )),
            accent: hsla(palette.ansi[4]),
            danger: hsla(palette.ansi[1]),
            warning: hsla(theme.ui.attention.fg),
        }
    }
}

fn sort_label(sort: ReviewSort) -> &'static str {
    match sort {
        ReviewSort::Smart => crate::i18n::text("智能排序", "Smart order"),
        ReviewSort::Recent => crate::i18n::text("最近完成", "Recently completed"),
        ReviewSort::Project => crate::i18n::text("按项目", "By project"),
        ReviewSort::Oldest => crate::i18n::text("最久未 Review", "Oldest unreviewed"),
    }
}

fn next_sort(sort: ReviewSort) -> ReviewSort {
    match sort {
        ReviewSort::Smart => ReviewSort::Recent,
        ReviewSort::Recent => ReviewSort::Project,
        ReviewSort::Project => ReviewSort::Oldest,
        ReviewSort::Oldest => ReviewSort::Smart,
    }
}

fn status(row: &Row) -> (&'static str, &'static str) {
    if row.needs_you {
        return (crate::i18n::text("需要你", "Needs you"), "warning");
    }
    match row.priority {
        Some(ReviewPriority::Failed) => (crate::i18n::text("失败", "Failed"), "danger"),
        Some(ReviewPriority::RunningWithResults) => {
            (crate::i18n::text("运行中", "Running"), "accent")
        }
        _ if row.running => (crate::i18n::text("运行中", "Running"), "accent"),
        _ => match row.latest_outcome {
            Some(ReviewOutcome::Interrupted) => {
                (crate::i18n::text("已中断", "Interrupted"), "warning")
            }
            Some(ReviewOutcome::Failed { .. }) => (crate::i18n::text("失败", "Failed"), "danger"),
            _ => (crate::i18n::text("已完成", "Completed"), "muted"),
        },
    }
}

impl SessionCenterView {
    fn tab_bar(&self, colors: Colors, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_end()
            .gap_1()
            .px_3()
            .pt_2()
            .children(Tab::ALL.into_iter().enumerate().map(|(index, tab)| {
                let active = self.tab == tab;
                let label = format!("{} {}", tab.label(), self.list.counts.for_tab(tab));
                div()
                    .id(SharedString::from(format!("session-center-tab-{index}")))
                    .relative()
                    .children(rects::recorder(RectId::SessionCenterTab(index)))
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .border_b_2()
                    .border_color(if active {
                        colors.accent
                    } else {
                        colors.background
                    })
                    .text_color(if active { colors.text } else { colors.muted })
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .hover(|style| style.bg(colors.hover))
                    .on_click(cx.listener(move |view, _, window, cx| view.set_tab(tab, window, cx)))
                    .child(label)
            }))
            .into_any_element()
    }

    fn toolbar(&self, colors: Colors, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let focus = self.focus_handle.clone();
        let query = if self.query.is_empty() && self.marked.is_none() {
            div().text_color(colors.muted).child(crate::i18n::text(
                "搜索标题、提示词、项目或 ID",
                "Search titles, prompts, projects, or IDs",
            ))
        } else {
            div().child(self.query.clone()).children(
                self.marked
                    .clone()
                    .map(|text| div().border_b_1().border_color(colors.accent).child(text)),
            )
        };
        div()
            .relative()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(colors.rule)
                    .child(query),
            )
            .child(
                div()
                    .id("session-center-sort")
                    .relative()
                    .children(rects::recorder(RectId::SessionCenterSort))
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(colors.rule)
                    .text_color(colors.muted)
                    .hover(|style| style.bg(colors.hover))
                    .on_click(cx.listener(|view, _, _, cx| view.set_sort(next_sort(view.sort), cx)))
                    .child(sort_label(self.sort)),
            )
            .into_any_element()
    }

    fn render_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = Colors::new(window, cx);
        range.map(|index| self.row(index, colors, cx)).collect()
    }

    fn row(&self, index: usize, colors: Colors, cx: &mut Context<Self>) -> AnyElement {
        let row = &self.list.rows[index];
        let selected = self.selection.cursor() == index;
        let (status, tone) = status(row);
        let tone = match tone {
            "danger" => colors.danger,
            "warning" => colors.warning,
            "accent" => colors.accent,
            _ => colors.muted,
        };
        let dir = SessionCenterView::row_dir(row, cx);
        let subtitle = super::model::queue_subtitle(row, &dir);
        let pending = (row.unreviewed_count > 0).then(|| {
            if crate::i18n::current() == crate::i18n::Language::English {
                format!("{} turns to review", row.unreviewed_count)
            } else {
                format!("{} 轮待 Review", row.unreviewed_count)
            }
        });
        div()
            .id(SharedString::from(format!("session-center-row-{index}")))
            .relative()
            .children(rects::recorder(RectId::SessionCenterRow(index)))
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .bg(if selected {
                colors.selected
            } else {
                colors.background
            })
            .hover(|style| style.bg(colors.hover))
            .on_click(cx.listener(move |view, _, _, cx| view.select(index, cx)))
            .child(div().w(px(5.)).h(px(28.)).rounded_full().bg(tone))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(
                        div().flex().gap_2().child(row.title.clone()).children(
                            row.pinned
                                .then(|| div().text_color(colors.warning).child("PIN")),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .text_size(px(11.))
                            .text_color(colors.muted)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .when(dir.missing, |d| d.line_through())
                                    .child(subtitle),
                            )
                            .children(dir.missing.then(|| {
                                div()
                                    .flex_none()
                                    .text_size(px(10.))
                                    .text_color(colors.warning)
                                    .child(crate::i18n::text("目录已不存在", "Directory missing"))
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .text_size(px(11.))
                    .child(
                        div()
                            .text_color(tone)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(status),
                    )
                    .children(pending.map(|text| div().text_color(colors.muted).child(text))),
            )
            .into_any_element()
    }

    /// The queue. `fill` stretches the list to the height of its parent (the two-column review);
    /// otherwise it is as tall as its rows, up to `VISIBLE_ROWS`.
    fn queue(&mut self, colors: Colors, fill: bool, cx: &mut Context<Self>) -> AnyElement {
        let body = if self.list.rows.is_empty() {
            let text = if self.snapshot.refreshing {
                crate::i18n::text("正在刷新会话…", "Refreshing sessions…")
            } else if self.query.is_empty() && self.tab == Tab::Review {
                crate::i18n::text(
                    "已全部 Review，没有待处理的结果",
                    "Everything is reviewed; nothing is pending",
                )
            } else if self.query.is_empty() {
                crate::i18n::text("这里暂时没有会话", "No sessions here yet")
            } else {
                crate::i18n::text("无匹配", "No matches")
            };
            div()
                .h(px(ROW_HEIGHT * 2.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.muted)
                .child(text)
                .into_any_element()
        } else {
            let list = gpui::uniform_list(
                "session-center-rows",
                self.list.rows.len(),
                cx.processor(Self::render_rows),
            )
            .track_scroll(self.scroll.clone());
            if fill {
                list.flex_1().min_h(px(0.)).into_any_element()
            } else {
                list.h(px(
                    ROW_HEIGHT * self.list.rows.len().min(VISIBLE_ROWS) as f32
                ))
                .into_any_element()
            }
        };
        let hints = if self.surface.is_some() {
            crate::i18n::text(
                "点击左侧会话切换 · Esc 返回列表",
                "Click a session on the left to switch · Esc returns to the list",
            )
        } else {
            crate::i18n::text(
                "↑↓ 选择 · Space 只读 Review · ↩ 回到 Agent / 恢复 · 全局外部运行发现将在 M3e 提供",
                "↑↓ select · Space opens read-only review · ↩ returns to Agent / resumes · external runtime discovery is planned for M3e",
            )
        };
        // A review state file that could not be read or saved: say so instead of silently losing progress.
        let store_error = cx.global::<ReviewService>().last_error().map(|error| {
            if crate::i18n::current() == crate::i18n::Language::English {
                format!("The review state file has a problem; progress may not be saved: {error}")
            } else {
                format!("Review 状态文件有问题，进度可能不会保存：{error}")
            }
        });
        let queue = div().flex().flex_col();
        let queue = if fill { queue.h_full() } else { queue };
        queue
            .child(self.toolbar(colors, cx))
            .child(div().h(px(1.)).bg(colors.rule))
            .child(body)
            .child(div().h(px(1.)).bg(colors.rule))
            .children(store_error.map(|text| {
                div()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .text_size(px(11.))
                    .text_color(colors.danger)
                    .child(text)
            }))
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child(hints),
            )
            .into_any_element()
    }

    /// Height of the whole center while a review is open (wide or narrow).
    pub(crate) fn review_height(window: &Window) -> f32 {
        (f32::from(window.viewport_size().height) - 96.).clamp(420., 720.)
    }
}

impl Render for SessionCenterView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(cx);
        let colors = Colors::new(window, cx);
        let root = div()
            .id("session-center")
            .key_context(SESSIONS_CONTEXT)
            .track_focus(&self.focus_handle);
        self.layout = model_layout(f32::from(window.viewport_size().width));
        let wide = self.layout == ReviewLayout::Wide;
        let reviewing = self.surface.is_some() && self.tab != Tab::All;
        let root = self
            .listeners(root, cx)
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.text)
            .text_size(px(13.));
        let root = if reviewing {
            root.h(px(Self::review_height(window)))
        } else {
            root
        };
        root.child(self.tab_bar(colors, cx))
            .child(div().h(px(1.)).bg(colors.rule))
            .child(match (self.tab, reviewing, wide) {
                (Tab::All, _, _) => match &self.cleanup {
                    Some((wizard, _)) => wizard.clone().into_any_element(),
                    None => self.all_sessions.clone().into_any_element(),
                },
                (_, false, _) => self.queue(colors, false, cx),
                // Narrow: the review replaces the queue until the user goes back.
                (_, true, false) => div()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.review_pane(colors, false, window, cx))
                    .into_any_element(),
                (_, true, true) => div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .child(
                        div()
                            .w(px(340.))
                            .flex_none()
                            .h_full()
                            .border_r_1()
                            .border_color(colors.rule)
                            .child(self.queue(colors, true, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .h_full()
                            .child(self.review_pane(colors, true, window, cx)),
                    )
                    .into_any_element(),
            })
    }
}
