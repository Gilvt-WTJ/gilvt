//! The read-only review pane: header, one card per turn (prompt, final reply, outcome, collapsible
//! process), paging and the action bar. Colors for tones come from the inspector; the frame's own
//! palette colors keep it blended with the Session Center.

use std::time::SystemTime;

use gilvt_agent::{
    ReviewCompatibility, ReviewItem, ReviewOutcome, ReviewTool, ReviewToolStatus, ReviewTurn,
};
use gpui::{div, prelude::*, px, AnyElement, Context, FontWeight, SharedString, Window};

use super::model::{capped_text, Action, Mode, Paging, SnoozeChoice};
use crate::debug_state::rects::{self, RectId, ReviewButton};
use crate::inspector::artifacts_model::relative;
use crate::inspector::colors::Colors as Tones;
use crate::inspector::model::token_label;
use crate::review::ReviewService;
use crate::session_center::render::Colors;
use crate::session_center::view::SessionCenterView;

/// A long reply or prompt is cut here so one huge turn cannot make every frame lay out a huge text.
const MAX_LINES: usize = 600;

/// The text as ONE element (gpui lays a multi-line string out as one shaped text), not one element per line:
/// 20 turns of 600-line replies would otherwise be ~24k elements laid out on every frame.
fn lines_block(text: &str, color: gpui::Hsla) -> AnyElement {
    let (shown, omitted) = capped_text(text, MAX_LINES);
    div()
        .flex()
        .flex_col()
        .text_color(color)
        .child(shown)
        .children((omitted > 0).then(|| {
            div().opacity(0.6).child(
                if crate::i18n::current() == crate::i18n::Language::English {
                    format!("… {omitted} more lines not shown")
                } else {
                    format!("… 还有 {omitted} 行没有显示")
                },
            )
        }))
        .into_any_element()
}

fn outcome_label(outcome: &ReviewOutcome, tones: &Tones) -> (String, gpui::Hsla) {
    match outcome {
        ReviewOutcome::Done => (crate::i18n::text("已完成", "Completed").into(), tones.green),
        ReviewOutcome::Interrupted => (
            crate::i18n::text("已中断", "Interrupted").into(),
            tones.yellow,
        ),
        ReviewOutcome::Failed { message } if message.is_empty() => {
            (crate::i18n::text("失败", "Failed").into(), tones.red)
        }
        ReviewOutcome::Failed { message }
            if crate::i18n::current() == crate::i18n::Language::English =>
        {
            (format!("Failed: {message}"), tones.red)
        }
        ReviewOutcome::Failed { message } => (format!("失败：{message}"), tones.red),
    }
}

fn tool_status(status: &ReviewToolStatus, tones: &Tones) -> (&'static str, gpui::Hsla) {
    match status {
        ReviewToolStatus::Running => (crate::i18n::text("运行中", "Running"), tones.blue),
        ReviewToolStatus::Ok => (crate::i18n::text("成功", "Succeeded"), tones.green),
        ReviewToolStatus::Failed { .. } => (crate::i18n::text("失败", "Failed"), tones.red),
        ReviewToolStatus::Denied => (crate::i18n::text("已拒绝", "Denied"), tones.yellow),
        ReviewToolStatus::Interrupted => (crate::i18n::text("已中断", "Interrupted"), tones.yellow),
    }
}

fn is_failed(item: &ReviewItem) -> bool {
    matches!(
        item,
        ReviewItem::Tool(ReviewTool {
            status: ReviewToolStatus::Failed { .. },
            ..
        })
    )
}

impl SessionCenterView {
    #[allow(clippy::too_many_arguments)]
    fn review_button(
        &self,
        which: ReviewButton,
        label: impl Into<SharedString>,
        action: Action,
        enabled: bool,
        primary: bool,
        colors: Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let button = div()
            .id(SharedString::from(format!("review-button-{which:?}")))
            .relative()
            .children(rects::recorder(RectId::ReviewButton(which)))
            .flex_none()
            .px_2()
            .py_1()
            .rounded(px(6.))
            .border_1()
            .border_color(if primary { colors.accent } else { colors.rule })
            .text_color(if primary { colors.accent } else { colors.text })
            .when(primary, |button| button.font_weight(FontWeight::SEMIBOLD))
            .child(label.into());
        if enabled {
            button
                .cursor_pointer()
                .hover(|style| style.bg(colors.hover))
                .on_click(cx.listener(move |view, _, window, cx| view.dispatch(action, window, cx)))
                .into_any_element()
        } else {
            button.opacity(0.4).into_any_element()
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn turn_card(
        &self,
        index: usize,
        turn: &ReviewTurn,
        expanded: bool,
        colors: Colors,
        tones: &Tones,
        now: SystemTime,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (outcome, outcome_color) = outcome_label(&turn.outcome, tones);
        let english = crate::i18n::current() == crate::i18n::Language::English;
        let mut facts = vec![if english {
            format!("Turn {}", turn.ordinal)
        } else {
            format!("第 {} 轮", turn.ordinal)
        }];
        if turn.tokens > 0 {
            facts.push(format!("{} tokens", token_label(turn.tokens)));
        }
        if turn.lines_added + turn.lines_removed > 0 {
            facts.push(format!("+{} -{}", turn.lines_added, turn.lines_removed));
        }
        if let Some(done) = turn.completed_at {
            facts.push(relative(now, done));
        }
        let failed = turn.items.iter().filter(|item| is_failed(item)).count();
        let process = if english {
            format!(
                "{} Process · {} items{}",
                if expanded { "▾" } else { "▸" },
                turn.items.len(),
                if failed > 0 {
                    format!(" · {failed} failed")
                } else {
                    String::new()
                }
            )
        } else {
            format!(
                "{} 过程 · {} 项{}",
                if expanded { "▾" } else { "▸" },
                turn.items.len(),
                if failed > 0 {
                    format!(" · {failed} 个失败")
                } else {
                    String::new()
                }
            )
        };
        let cursor = turn.cursor.clone();
        div()
            .relative()
            .children(rects::recorder(RectId::ReviewTurn(index)))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded(px(8.))
            .border_1()
            .border_color(colors.rule)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child(crate::i18n::text("你", "You"))
                    .into_any_element(),
            )
            .child(lines_block(&turn.prompt, colors.muted))
            .child(div().h(px(1.)).bg(colors.rule))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child(crate::i18n::text(
                        "Agent 最终回复",
                        "Agent's final response",
                    )),
            )
            .child(if turn.final_reply.trim().is_empty() {
                div()
                    .text_color(colors.muted)
                    .child(crate::i18n::text("未产生最终回复", "No final response"))
                    .into_any_element()
            } else {
                lines_block(&turn.final_reply, colors.text)
            })
            .children(turn.truncation.as_ref().map(|truncation| {
                div()
                    .text_size(px(11.))
                    .text_color(tones.yellow)
                    .child(if english {
                        format!("This turn is {} bytes, above the {} byte read limit, so only part is shown", truncation.bytes, truncation.limit)
                    } else {
                        format!("这一轮有 {} 字节，超过 {} 字节的读取上限，只显示了一部分", truncation.bytes, truncation.limit)
                    })
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child(div().text_color(outcome_color).child(outcome))
                    .children(facts.into_iter().map(|fact| div().child(fact))),
            )
            .children((!turn.items.is_empty()).then(|| {
                div()
                    .id(SharedString::from(format!(
                        "review-process-{}",
                        turn.ordinal
                    )))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(colors.accent)
                    .on_click(cx.listener(move |view, _, _, cx| {
                        if let Some(surface) = &mut view.surface {
                            surface.toggle_expanded(cursor.clone());
                        }
                        cx.notify();
                    }))
                    .child(process)
            }))
            .children(expanded.then(|| self.process_items(&turn.items, colors, tones)))
            .into_any_element()
    }

    fn process_items(&self, items: &[ReviewItem], colors: Colors, tones: &Tones) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .text_size(px(12.))
            .children(items.iter().map(|item| {
                match item {
                    ReviewItem::Thinking { secs, text } => div()
                        .flex()
                        .flex_col()
                        .p_2()
                        .rounded(px(6.))
                        .bg(tones.detail_bg)
                        .text_color(colors.muted)
                        .child(match secs {
                            Some(secs)
                                if crate::i18n::current() == crate::i18n::Language::English =>
                            {
                                format!("Thinking {secs:.0}s")
                            }
                            Some(secs) => format!("思考 {secs:.0}s"),
                            None => crate::i18n::text("思考", "Thinking").to_string(),
                        })
                        .child(text.iter().take(20).cloned().collect::<Vec<_>>().join("\n"))
                        .into_any_element(),
                    ReviewItem::Tool(tool) => {
                        let (status, tone) = tool_status(&tool.status, tones);
                        div()
                            .flex()
                            .flex_col()
                            .p_2()
                            .rounded(px(6.))
                            .bg(if matches!(tool.status, ReviewToolStatus::Failed { .. }) {
                                tones.err_bg
                            } else {
                                tones.detail_bg
                            })
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(div().text_color(tone).child(status))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(tool.tool.clone()),
                                    )
                                    .child(
                                        div().flex_1().min_w(px(0.)).child(tool.summary.clone()),
                                    ),
                            )
                            .children((!tool.detail.input.is_empty()).then(|| {
                                div().text_color(colors.muted).child(
                                    tool.detail
                                        .input
                                        .iter()
                                        .map(|(key, value)| format!("{key}: {value}"))
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                )
                            }))
                            .children(
                                (!tool.detail.output.is_empty() || !tool.error_excerpt.is_empty())
                                    .then(|| {
                                        div().text_color(tones.detail).child(
                                            tool.detail
                                                .output
                                                .iter()
                                                .chain(tool.error_excerpt.iter())
                                                .take(24)
                                                .cloned()
                                                .collect::<Vec<_>>()
                                                .join("\n"),
                                        )
                                    }),
                            )
                            .into_any_element()
                    }
                }
            }))
            .into_any_element()
    }

    pub(crate) fn review_pane(
        &mut self,
        colors: Colors,
        wide: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(surface) = self.surface.clone() else {
            return div().into_any_element();
        };
        let tones = Tones::new(&crate::theme::current(cx).ui, colors.dark);
        let (document, loading, error) = {
            let service = cx.global::<ReviewService>();
            (
                service.detail(),
                service.detail_loading(),
                service.detail_error().map(str::to_string),
            )
        };
        let document = document.filter(|document| document.key == surface.key);
        let row = self.list.rows.iter().find(|row| row.key == surface.key);
        let title = row.map(|row| row.title.clone()).unwrap_or_default();
        let dir = row
            .map(|row| SessionCenterView::row_dir(row, cx))
            .unwrap_or_default();
        let subtitle = row
            .map(|row| crate::session_center::model::header_subtitle(row, &dir))
            .unwrap_or_default();
        let pinned = row.is_some_and(|row| row.pinned);
        let has_runtime = row.is_some_and(|row| row.runtime.is_some());
        let can_control = self.can_control_runtime();
        let terminate_confirm = self.terminate_confirm.as_ref().is_some_and(|key| key == &surface.key);
        let range = match (&document, surface.mode) {
            (_, Mode::Full) => crate::i18n::text("完整历史", "Full history").to_string(),
            (Some(document), Mode::Incremental)
                if crate::i18n::current() == crate::i18n::Language::English =>
            {
                format!(
                    "Unreviewed · {} turns{}",
                    document.turns.len(),
                    if document.has_later {
                        " (more available)"
                    } else {
                        ""
                    }
                )
            }
            (Some(document), Mode::Incremental) => format!(
                "未 Review · {} 轮{}",
                document.turns.len(),
                if document.has_later {
                    "（还有更多）"
                } else {
                    ""
                }
            ),
            (None, Mode::Incremental) => crate::i18n::text("未 Review", "Unreviewed").to_string(),
        };
        let ready = self.can_review_next(cx);
        let stale = self.surface_stale();
        let has_later = document.as_ref().is_some_and(|document| document.has_later);
        let now = SystemTime::now();

        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .children((!wide).then(|| {
                self.review_button(
                    ReviewButton::Back,
                    crate::i18n::text("← 返回", "← Back"),
                    Action::Back,
                    true,
                    false,
                    colors,
                    cx,
                )
            }))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
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
                                    .child(format!("{subtitle} · {range}")),
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
            .child(self.review_button(
                ReviewButton::FullHistory,
                match surface.mode {
                    Mode::Incremental => crate::i18n::text("完整历史 F", "Full History F"),
                    Mode::Full => crate::i18n::text("只看未 Review F", "Unreviewed Only F"),
                },
                Action::ToggleFullHistory,
                true,
                false,
                colors,
                cx,
            ))
            .child(self.review_button(
                ReviewButton::BackToAgent,
                crate::i18n::text("回到 Agent ↩", "Back to Agent ↩"),
                Action::BackToAgent,
                true,
                false,
                colors,
                cx,
            ))
            .children(has_runtime.then(|| {
                self.review_button(
                    ReviewButton::CopyDiagnostics,
                    crate::i18n::text("复制诊断", "Copy Diagnostics"),
                    Action::CopyDiagnostics,
                    true,
                    false,
                    colors,
                    cx,
                )
            }));

        let mut body = div()
            .id("session-review-body")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.detail_scroll)
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_3();
        if let Some(notice) = &self.notice {
            body = body.child(
                div()
                    .p_2()
                    .rounded(px(6.))
                    .bg(tones.err_bg)
                    .text_color(tones.err)
                    .child(notice.clone()),
            );
        }
        if stale {
            // The saved review position is not in the transcript (it was truncated or replaced): the user
            // chooses what counts as seen; "reviewed, next" stays off until then.
            let baseline = self.review_button(
                ReviewButton::BaselineHere,
                crate::i18n::text("从当前开始 B", "Start from Here B"),
                Action::BaselineHere,
                true,
                false,
                colors,
                cx,
            );
            let all = self.review_button(
                ReviewButton::ReviewAll,
                crate::i18n::text("Review 全部可见历史 A", "Review All Visible History A"),
                Action::ReviewAllVisible,
                true,
                false,
                colors,
                cx,
            );
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_2()
                    .rounded(px(6.))
                    .bg(tones.banner)
                    .text_color(tones.yellow)
                    .child(crate::i18n::text(
                        "这个会话的 Review 位置在记录里找不到了（文件可能被截断或被替换）。下面是最新的几轮；请选择怎么处理：",
                        "The review position for this session is no longer available (the file may have been truncated or replaced). These are the latest turns; choose how to proceed:",
                    ))
                    .child(div().flex().flex_wrap().gap_2().child(baseline).child(all)),
            );
        }
        if let Some(error) = error {
            body = body.child(div().text_color(tones.err).child(
                if crate::i18n::current() == crate::i18n::Language::English {
                    format!("Could not read this session: {error}")
                } else {
                    format!("无法读取这个会话：{error}")
                },
            ));
        } else if loading {
            body = body.child(
                div()
                    .text_color(colors.muted)
                    .child(crate::i18n::text("正在加载…", "Loading…")),
            );
        }
        if let Some(document) = &document {
            if document.has_earlier {
                body = body.child(self.review_button(
                    ReviewButton::Earlier,
                    crate::i18n::text("更早的 turn E", "Earlier Turns E"),
                    Action::Page(Paging::Earlier),
                    true,
                    false,
                    colors,
                    cx,
                ));
            }
            if document.turns.is_empty() {
                body = body.child(
                    div()
                        .text_color(colors.muted)
                        .child(crate::i18n::text("没有需要 Review 的新内容，可以切换到完整历史查看。", "There is no new content to review. Switch to full history to browse older turns.")),
                );
            }
            for (index, turn) in document.turns.iter().enumerate() {
                let expanded = surface.is_expanded(&turn.cursor);
                body = body.child(self.turn_card(index, turn, expanded, colors, &tones, now, cx));
            }
            if document.has_later {
                body = body.child(self.review_button(
                    ReviewButton::Later,
                    crate::i18n::text("更新的 turn L", "Later Turns L"),
                    Action::Page(Paging::Later),
                    true,
                    false,
                    colors,
                    cx,
                ));
            }
            let ReviewCompatibility {
                unknown_records,
                unknown_blocks,
            } = document.compatibility;
            if unknown_records + unknown_blocks > 0 {
                body = body.child(div().text_size(px(11.)).text_color(colors.muted).child(
                    if crate::i18n::current() == crate::i18n::Language::English {
                        format!("Skipped {unknown_records} unrecognized records and {unknown_blocks} content blocks; the rest is shown normally")
                    } else {
                        format!("有 {} 条记录、{} 个内容块无法识别，已跳过；其余内容照常显示", unknown_records, unknown_blocks)
                    },
                ));
            }
        }

        let snooze_menu = surface.snooze_menu.then(|| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(colors.rule)
                .text_size(px(12.))
                .child(
                    div()
                        .text_color(colors.muted)
                        .child(crate::i18n::text("稍后提醒：", "Remind me later:")),
                )
                .child(self.review_button(
                    ReviewButton::SnoozeHour,
                    crate::i18n::text("1 小时后 1", "In 1 Hour 1"),
                    Action::Snooze(SnoozeChoice::Hour),
                    true,
                    false,
                    colors,
                    cx,
                ))
                .child(self.review_button(
                    ReviewButton::SnoozeLater,
                    crate::i18n::text("今天晚些时候 2", "Later Today 2"),
                    Action::Snooze(SnoozeChoice::Later),
                    true,
                    false,
                    colors,
                    cx,
                ))
                .child(self.review_button(
                    ReviewButton::SnoozeTomorrow,
                    crate::i18n::text("明天 3", "Tomorrow 3"),
                    Action::Snooze(SnoozeChoice::Tomorrow),
                    true,
                    false,
                    colors,
                    cx,
                ))
                .child(self.review_button(
                    ReviewButton::SnoozeCancel,
                    crate::i18n::text("取消 Esc", "Cancel Esc"),
                    Action::CloseSnoozeMenu,
                    true,
                    false,
                    colors,
                    cx,
                ))
        });

        // Wraps: in the wide layout the Review column is narrow, and six buttons do not fit on one line
        // (without wrapping the primary 「已 Review，下一个」 was pushed out of sight).
        let footer = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(colors.rule)
            .child(self.review_button(
                ReviewButton::Skip,
                crate::i18n::text("跳过 S", "Skip S"),
                Action::Skip,
                true,
                false,
                colors,
                cx,
            ))
            .child(self.review_button(
                ReviewButton::Snooze,
                crate::i18n::text("稍后提醒 Z", "Remind Later Z"),
                Action::OpenSnoozeMenu,
                true,
                false,
                colors,
                cx,
            ))
            .child(self.review_button(
                ReviewButton::Pin,
                if pinned {
                    crate::i18n::text("取消置顶 P", "Unpin P")
                } else {
                    crate::i18n::text("置顶 P", "Pin P")
                },
                Action::Pin,
                true,
                false,
                colors,
                cx,
            ))
            .children(has_runtime.then(|| {
                self.review_button(
                    ReviewButton::Interrupt,
                    crate::i18n::text("中断", "Interrupt"),
                    Action::Interrupt,
                    can_control,
                    false,
                    colors,
                    cx,
                )
            }))
            .children(has_runtime.then(|| {
                self.review_button(
                    ReviewButton::Terminate,
                    if terminate_confirm {
                        crate::i18n::text("确认终止", "Confirm Terminate")
                    } else {
                        crate::i18n::text("终止…", "Terminate…")
                    },
                    Action::Terminate,
                    can_control,
                    false,
                    colors,
                    cx,
                )
            }))
            .child(div().flex_1())
            // Why the button is off: confirming now would mark turns that were never shown.
            .children((has_later && !stale).then(|| {
                div()
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child(crate::i18n::text(
                        "还有更晚的 turn：翻到最后一页（L）后才能确认",
                        "There are later turns. Go to the last page (L) before confirming.",
                    ))
            }))
            .child(self.review_button(
                ReviewButton::NextArchive,
                crate::i18n::text("标记已 Review 并归档 ⇧⌘E", "Mark Reviewed and Archive ⇧⌘E"),
                Action::ReviewNextAndArchive,
                self.can_review_and_archive(cx),
                false,
                colors,
                cx,
            ))
            .child(self.review_button(
                ReviewButton::Next,
                crate::i18n::text("已 Review，下一个 ⌘↩", "Reviewed, Next ⌘↩"),
                Action::ReviewNext,
                ready,
                true,
                colors,
                cx,
            ));

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(div().h(px(1.)).bg(colors.rule))
            .child(body)
            .children(snooze_menu)
            .child(footer)
            .into_any_element()
    }
}
