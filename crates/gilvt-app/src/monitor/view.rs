//! Draws the monitor tab's card wall inside a `Workspace` (mockups: specs/2026-10-05-gilvt-monitor-mockups/card-v2.html,
//! ui-detail-v1.html 图 1; the ✦ block: s2-card-states.html — without the chat column and the ◎ marks, which come with S2–S4).

use gpui::{div, prelude::*, px, relative, AnyElement, App, Context, Div, FontWeight, Hsla, KeyDownEvent, MouseButton, SharedString, Stateful, Window};

use gilvt_theme::color::mix;
use gilvt_theme::UiColors;

use super::model::{AgentCard, BlockLine, Card, Group, GroupView, Mark, MonitorModel, SummaryLine, TerminalCard, TurnLine};
use super::{Filter, MonitorPane, MonitorUi};
use crate::debug_state::rects::{self, RectId};
use crate::pane_tree::PaneId;
use crate::sidebar::model::Tone;
use crate::theme::{hsla, AppSettings};
use crate::workspace::{monitor_jump, Workspace};

struct Colors {
    bg: Hsla,
    card: Hsla,
    card_selected: Hsla,
    text: Hsla,
    faint: Hsla,
    chip: Hsla,
    chip_on: Hsla,
    need: Hsla,
    blue: Hsla,
    red: Hsla,
    green: Hsla,
    grey: Hsla,
    track: Hsla,
    button: Hsla,
    /// The ✦ AI summary's purples (s2-card-states.html).
    ai: Hsla,
    ai_bg: Hsla,
    ai_text: Hsla,
    ai_faint: Hsla,
    ai_skeleton: Hsla,
}

impl Colors {
    // Status colours match the sidebar's (`sidebar/view.rs` Colors).
    fn new(ui: &UiColors) -> Colors {
        let h = hsla;
        Colors {
            bg: h(ui.panel),
            card: h(ui.card),
            card_selected: h(ui.selected),
            text: h(ui.text),
            faint: h(ui.text_3),
            chip: h(ui.fill),
            chip_on: h(ui.fill_on),
            need: h(ui.attention.fg),
            blue: h(ui.running.fg),
            red: h(ui.error.fg),
            green: h(ui.done.fg),
            grey: h(ui.text_4),
            track: h(ui.fill),
            button: h(ui.fill),
            ai: h(ui.purple),
            ai_bg: h(mix(ui.panel, ui.purple, 0.12)),
            ai_text: h(mix(ui.text, ui.purple, 0.3)),
            ai_faint: h(mix(ui.purple, ui.text_3, 0.4)),
            ai_skeleton: h(mix(ui.panel, ui.purple, 0.2)),
        }
    }

    fn tone(&self, t: Tone) -> Hsla {
        match t {
            Tone::Waiting => self.need,
            Tone::Running => self.blue,
            Tone::Error => self.red,
            Tone::Done => self.green,
            Tone::Muted => self.grey,
        }
    }

    fn mark(&self, m: Mark) -> Hsla {
        match m {
            Mark::Ok => self.green,
            Mark::Failed => self.red,
            Mark::Running => self.blue,
            Mark::Interrupted | Mark::Unknown => self.faint,
        }
    }
}

/// The filter chips in drawing order (`RectId::MonitorFilter(i)`): 全部, then each non-empty group but 已结束
/// (spec §3.4; 已结束 still counts in 全部), with their labels as drawn.
pub(crate) fn chips(model: &MonitorModel) -> Vec<(Filter, String)> {
    let all_total: usize = model.counts.iter().map(|(_, n)| n).sum();
    std::iter::once((
        Filter::All,
        format!("{} {all_total}", crate::i18n::text("全部", "All")),
    ))
    .chain(
        model
            .counts
            .iter()
            .filter(|(g, _)| *g != Group::Ended)
            .map(|(g, n)| (Filter::Only(*g), format!("{} {n}", g.title()))),
    )
    .collect()
}

/// A group header as drawn: `index` is its `RectId::MonitorGroup`, `cards` the cards drawn under it (none
/// when collapsed) with their `RectId::MonitorCard` numbers.
pub(crate) struct DrawnGroup<'a> {
    pub index: usize,
    pub view: &'a GroupView,
    pub cards: Vec<(usize, &'a Card)>,
}

/// The wall's drawing order, shared by `render` and DebugState so their `RectId` numbers agree.
pub(crate) fn drawn(model: &MonitorModel) -> Vec<DrawnGroup<'_>> {
    let mut n = 0;
    model
        .groups
        .iter()
        .enumerate()
        .map(|(index, view)| {
            let cards = if view.collapsed {
                Vec::new()
            } else {
                view.cards
                    .iter()
                    .map(|c| {
                        n += 1;
                        (n - 1, c)
                    })
                    .collect()
            };
            DrawnGroup { index, view, cards }
        })
        .collect()
}

/// A card's (title, status line, meta line) as drawn: agent — 「C name」, its status, the meta row (git · 第 N 轮
/// · …); terminal — 「name · cwd」, the running / 最后一条 line (else 前台：… / 空闲), the failed command's error line.
pub(crate) fn card_lines(c: &Card) -> (String, String, String) {
    match c {
        Card::Agent(a) => {
            let mut meta: Vec<String> = Vec::new();
            if !a.git.is_empty() {
                meta.push(a.git.clone());
            }
            if let Some(t) = a.turn {
                meta.push(
                    if crate::i18n::current() == crate::i18n::Language::English {
                        format!("Turn {t}")
                    } else {
                        format!("第 {t} 轮")
                    },
                );
            }
            if let Some(e) = &a.elapsed {
                meta.push(
                    if crate::i18n::current() == crate::i18n::Language::English {
                        if e.starts_with("took") {
                            e.clone()
                        } else {
                            format!("This turn {e}")
                        }
                    } else if e.starts_with("用时") {
                        e.clone()
                    } else {
                        format!("本轮 {e}")
                    },
                );
            }
            if let Some(ch) = a.changes {
                meta.push(match (ch.added, ch.removed) {
                    (Some(added), Some(removed))
                        if crate::i18n::current() == crate::i18n::Language::English =>
                    {
                        format!("+{added} −{removed} · {} files", ch.files)
                    }
                    (Some(added), Some(removed)) => {
                        format!("+{added} −{removed} · {} 文件", ch.files)
                    }
                    _ if crate::i18n::current() == crate::i18n::Language::English => {
                        format!("{} files", ch.files)
                    }
                    _ => format!("{} 文件", ch.files),
                });
            }
            (format!("{} {}", a.letter, a.name), a.status.clone(), meta.join(" · "))
        }
        Card::Terminal(t) => {
            let title = if t.cwd.is_empty() { t.name.clone() } else { format!("{} · {}", t.name, t.cwd) };
            let status = if let Some((cmd, elapsed)) = &t.running {
                format!("● {cmd} · {elapsed}")
            } else if let Some(last) = &t.last {
                block_text(last, true)
            } else if let Some(fg) = &t.foreground {
                if crate::i18n::current() == crate::i18n::Language::English {
                    format!("Foreground: {fg}")
                } else {
                    format!("前台：{fg}")
                }
            } else {
                crate::i18n::text("空闲", "Idle").to_string()
            };
            (title, status, t.error_line.clone().unwrap_or_default())
        }
    }
}

/// The 补课 rows' texts (`RectId::MonitorCatchupRow(n, j)`), newest first.
pub(crate) fn catchup_texts(c: &Card) -> Vec<String> {
    match c {
        Card::Agent(a) => a.catchup.iter().map(turn_text).collect(),
        Card::Terminal(t) => t.catchup.iter().map(|b| block_text(b, false)).collect(),
    }
}

pub fn render(ws: &Workspace, pane: PaneId, m: &MonitorPane, window: &Window, cx: &mut Context<Workspace>) -> AnyElement {
    let theme = crate::theme::current(cx);
    let k = Colors::new(&theme.ui);
    let model = ws.monitor_model().cloned().unwrap_or_default();
    let ui = &m.ui;
    let chat_on = cx.global::<AppSettings>().0.monitor.enabled;

    let mut filters = div().flex().flex_wrap().gap_1().items_center();
    for (i, (filter, label)) in chips(&model).into_iter().enumerate() {
        let on = model.filter == filter;
        filters = filters.child(
            div()
                .id(("monitor-filter", i))
                .relative()
                .children(rects::recorder(RectId::MonitorFilter(i)))
                .px_2()
                .rounded_full()
                .cursor_pointer()
                .bg(if on { k.chip_on } else { k.chip })
                .text_color(k.text)
                .child(label)
                .on_click(cx.listener(move |ws, _, _, cx| ws.monitor_set_filter(pane, filter, cx))),
        );
    }

    let mut body = div().flex().flex_col().gap_3();
    if model.counts.is_empty() {
        body = body.child(div().text_color(k.faint).child(crate::i18n::text(
            "还没有会话。在任意标签运行 claude / codex，或者执行命令，这里就会出现。",
            "No sessions yet. Run claude / codex or execute a command in any tab to see it here.",
        )));
    }
    for d in drawn(&model) {
        let (gi, g) = (d.index, d.view);
        let mut head = div()
            .id(("monitor-group", gi))
            .relative()
            .children(rects::recorder(RectId::MonitorGroup(gi)))
            .text_color(k.faint)
            .text_size(px(11.))
            .child(format!("{} · {}{}", g.group.title(), g.cards.len(), if g.collapsed { " ▸" } else { "" }));
        if g.group == Group::Ended {
            head = head.cursor_pointer().on_click(cx.listener(move |ws, _, _, cx| ws.monitor_toggle_ended(pane, cx)));
        }
        body = body.child(head);
        if g.collapsed {
            continue;
        }
        let mut grid = div().flex().flex_wrap().gap_2();
        for (n, c) in d.cards {
            grid = grid.child(card(c, n, pane, ui, chat_on, &k, cx));
        }
        body = body.child(grid);
    }

    // The wall's key handler covers the wall only: the chat panel is drawn beside it (`chat_view::render`), so
    // what is typed in the chat's input never reaches these keys.
    let wall = div()
        .id(("monitor", pane as usize))
        .track_focus(&m.focus)
        .size_full()
        .overflow_y_scroll()
        .bg(k.bg)
        .text_color(k.text)
        .text_size(px(12.))
        .p_3()
        .flex()
        .flex_col()
        .gap_3()
        .on_key_down(cx.listener(move |ws, e: &KeyDownEvent, window, cx| {
            let mods = &e.keystroke.modifiers;
            if mods.platform || mods.control || mods.alt {
                return;
            }
            if e.keystroke.key == "a" && !e.is_held && ws.monitor_ask_selected(pane, window, cx) {
                cx.stop_propagation();
                return;
            }
            if ws.monitor_key(pane, &e.keystroke.key, cx) {
                cx.stop_propagation();
            }
        }))
        .child(filters)
        .child(body)
        .into_any_element();
    if !chat_on {
        return wall;
    }
    crate::monitor::chat_view::render(ws, pane, m, wall, window, cx)
}

fn card(c: &Card, n: usize, pane: PaneId, ui: &MonitorUi, chat_on: bool, k: &Colors, cx: &mut Context<Workspace>) -> AnyElement {
    let key = c.key();
    let selected = ui.selected.as_deref() == Some(key.as_str());
    let expanded = ui.expanded.as_deref() == Some(key.as_str());
    let bar = match c {
        Card::Agent(a) => k.tone(a.tone),
        Card::Terminal(t) if t.running.is_some() => k.blue,
        Card::Terminal(_) => k.grey,
    };
    let mut el = div()
        .id(("monitor-card", n))
        .relative()
        .children(rects::recorder(RectId::MonitorCard(n)))
        // 两列宽（补课展开）或一列；最小 260px，见 spec §3.5。
        .w(if expanded { relative(1.) } else { px(300.).into() })
        .min_w(px(260.))
        .p_2()
        .rounded_md()
        .border_l(px(3.))
        .border_color(bar)
        .bg(if selected { k.card_selected } else { k.card })
        .flex()
        .flex_col()
        .gap_1();
    {
        let key = key.clone();
        el = el.on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| ws.monitor_select(pane, key.clone(), cx)));
    }
    let lines = card_lines(c);
    el = match c {
        Card::Agent(a) => agent_body(el, a, lines, n, key.clone(), k, cx),
        Card::Terminal(t) => terminal_body(el, t, lines, n, key.clone(), k, cx),
    };
    // Buttons.
    let target = c.pane();
    let mut buttons = div().flex().gap_1().mt_1();
    buttons = buttons.child(
        div()
            .id(("monitor-jump", n))
            .relative()
            .children(rects::recorder(RectId::MonitorJump(n)))
            .px_2()
            .rounded_sm()
            .bg(k.button)
            .child(crate::i18n::text("跳过去", "Jump there"))
            .when(target.is_none(), |d| d.opacity(0.4))
            .when(target.is_some(), |d| d.cursor_pointer())
            .on_click(cx.listener(move |_, _, _, cx| {
                if let Some(p) = target {
                    App::defer(cx, move |cx| monitor_jump(p, None, None, cx));
                }
            })),
    );
    {
        let key = key.clone();
        buttons = buttons.child(
            div()
                .id(("monitor-catchup", n))
                .relative()
                .children(rects::recorder(RectId::MonitorCatchup(n)))
                .px_2()
                .rounded_sm()
                .cursor_pointer()
                .bg(k.button)
                .child(if expanded {
                    crate::i18n::text("收起", "Collapse")
                } else {
                    crate::i18n::text("补课", "Catch up")
                })
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.monitor_toggle_catchup(pane, key.clone(), cx)
                })),
        );
    }
    if c.summary().is_some_and(|s| s.actionable && s.state != "none") {
        let key = key.clone();
        buttons = buttons.child(
            div()
                .id(("monitor-resummarize", n))
                .relative()
                .children(rects::recorder(RectId::MonitorResummarize(n)))
                .px_2()
                .rounded_sm()
                .cursor_pointer()
                .bg(k.ai_bg)
                .text_color(k.ai_text)
                .child(crate::i18n::text("✦ 重新总结", "✦ Summarize Again"))
                .on_click(cx.listener(move |_, _, _, cx| {
                    let key = key.clone();
                    App::defer(cx, move |cx| crate::monitor::summaries::request(key, cx));
                })),
        );
    }
    if chat_on && !c.excluded() {
        let key = key.clone();
        buttons = buttons.child(
            div()
                .id(("monitor-ask", n))
                .relative()
                .children(rects::recorder(RectId::MonitorAsk(n)))
                .px_2()
                .rounded_sm()
                .cursor_pointer()
                .bg(k.ai_bg)
                .text_color(k.ai_text)
                .child(crate::i18n::text("◎ 问它", "◎ Ask"))
                .on_click(cx.listener(move |ws, _, window, cx| {
                    ws.monitor_ask(pane, key.clone(), window, cx)
                })),
        );
    }
    el = el.child(buttons);
    if expanded {
        el = el.child(catchup(c, n, k, cx));
    }
    el.into_any_element()
}

fn agent_body(el: Stateful<Div>, a: &AgentCard, (title, status, meta): (String, String, String), n: usize, key: String, k: &Colors, cx: &mut Context<Workspace>) -> Stateful<Div> {
    let mut bottom: Vec<String> = Vec::new();
    if let Some((done, total)) = a.todo {
        bottom.push(format!("TODO {done}/{total}"));
    }
    if let Some(ctx) = a.context {
        bottom.push(format!(
            "{} {}%",
            crate::i18n::text("上下文", "Context"),
            (ctx * 100.).round() as u32
        ));
    }
    el.child(
        div()
            .flex()
            .justify_between()
            .gap_2()
            .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(title))
            .child(div().text_color(k.faint).flex_none().child(a.location.clone())),
    )
    .when_some(a.summary.clone(), |d, s| d.child(summary_block(&s, n, key, k, cx)))
    .child(div().text_color(k.tone(a.tone)).truncate().child(status))
    .when(!meta.is_empty(), |d| d.child(div().text_color(k.faint).text_size(px(11.)).truncate().child(meta)))
    .when_some(a.todo, |d, (done, total)| {
        let ratio = if total == 0 { 0. } else { done as f32 / total as f32 };
        d.child(div().h(px(4.)).w_full().rounded_sm().bg(k.track).child(div().h_full().rounded_sm().bg(k.blue).w(relative(ratio))))
    })
    .when(!bottom.is_empty(), |d| d.child(div().text_color(k.faint).text_size(px(11.)).child(bottom.join(" · "))))
    .when(!a.quote.is_empty(), |d| d.child(div().text_color(k.faint).italic().truncate().child(SharedString::from(format!("「{}」", a.quote)))))
}

fn terminal_body(el: Stateful<Div>, t: &TerminalCard, (title, status, error): (String, String, String), n: usize, key: String, k: &Colors, cx: &mut Context<Workspace>) -> Stateful<Div> {
    let color = match (&t.running, &t.last) {
        (Some(_), _) => k.blue,
        (None, Some(last)) => k.mark(last.mark),
        (None, None) => k.faint,
    };
    el.child(
        div()
            .flex()
            .justify_between()
            .gap_2()
            .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(title))
            .child(div().text_color(k.faint).flex_none().child(t.location.clone())),
    )
    .when_some(t.summary.clone(), |d, s| d.child(summary_block(&s, n, key, k, cx)))
    .child(div().text_color(color).truncate().child(status))
    .when(!error.is_empty(), |d| d.child(div().text_color(k.red).text_size(px(11.)).truncate().child(error)))
}

/// The ✦ block's texts as DebugState lists them: 「目标：…」 and 「近期：…」 ("" when absent).
pub(crate) fn summary_texts(s: &SummaryLine) -> (String, String) {
    (
        s.goal
            .as_ref()
            .map(|g| {
                if crate::i18n::current() == crate::i18n::Language::English {
                    format!("Goal: {g}")
                } else {
                    format!("目标：{g}")
                }
            })
            .unwrap_or_default(),
        s.recent
            .as_ref()
            .map(|r| {
                if crate::i18n::current() == crate::i18n::Language::English {
                    format!("Recent: {r}")
                } else {
                    format!("近期：{r}")
                }
            })
            .unwrap_or_default(),
    )
}

fn summary_block(s: &SummaryLine, n: usize, key: String, k: &Colors, cx: &mut Context<Workspace>) -> AnyElement {
    let (goal, recent) = summary_texts(s);
    let header_color = match s.state {
        "failed" | "paused" => k.red,
        "stale" => k.need,
        _ => k.ai_faint,
    };
    let mut header = div().id(("monitor-summary-head", n)).text_size(px(10.5)).text_color(header_color).truncate().child(s.header.clone());
    if s.clickable {
        header = header.cursor_pointer().on_click(cx.listener(move |_, _, _, cx| {
            let key = key.clone();
            App::defer(cx, move |cx| crate::monitor::summaries::request(key, cx));
        }));
    }
    let none = s.state == "none";
    div()
        .id(("monitor-summary", n))
        .relative()
        .children(rects::recorder(RectId::MonitorSummary(n)))
        .my_1()
        .px_2()
        .py_1()
        .rounded_md()
        .when(!none, |d| d.bg(k.ai_bg).border_l(px(2.)).border_color(if matches!(s.state, "failed" | "paused") { k.red } else { k.ai }))
        .when(none, |d| d.border_1().border_dashed().border_color(k.ai))
        .flex()
        .flex_col()
        .gap(px(1.))
        .child(header)
        .when(s.state == "pending" && s.recent.is_none(), |d| {
            d.child(div().h(px(7.)).w(relative(0.7)).rounded_sm().bg(k.ai_skeleton)).child(div().h(px(7.)).w(relative(0.92)).rounded_sm().bg(k.ai_skeleton))
        })
        .when(!goal.is_empty(), |d| d.child(div().text_color(k.ai_text).truncate().child(goal)))
        .when(!recent.is_empty(), |d| d.child(div().text_color(k.ai_text).line_clamp(3).child(recent)))
        .into_any_element()
}

/// 「✗ make test · exit 2 · 3 分钟前」; `last` adds 最后一条：.
pub(crate) fn block_text(b: &BlockLine, last: bool) -> String {
    let mut parts = vec![format!("{}{} {}", if last { "最后一条：" } else { "" }, b.mark.glyph(), b.command)];
    if let Some(code) = b.exit {
        parts.push(format!("exit {code}"));
    }
    if let Some(took) = &b.took {
        parts.push(took.clone());
    }
    parts.push(b.ago.clone());
    parts.join(" · ")
}

pub(crate) fn turn_text(l: &TurnLine) -> String {
    let mut parts = vec![format!("T{} 「{}」 {}", l.turn, l.prompt, l.mark.glyph())];
    if let Some(took) = &l.took {
        parts.push(took.clone());
    }
    if let Some(ch) = l.changes {
        parts.push(match (ch.added, ch.removed) {
            (Some(added), Some(removed)) => format!("+{added} −{removed}"),
            // Not the task's net yet: no line counts to show.
            _ => format!("{} 文件", ch.files),
        });
    }
    parts.join(" · ")
}

fn catchup(c: &Card, n: usize, k: &Colors, cx: &mut Context<Workspace>) -> AnyElement {
    let mut list = div().flex().flex_col().gap_1().mt_1().pt_1().border_t_1().border_color(k.track);
    match c {
        Card::Agent(a) => {
            if a.catchup.is_empty() {
                list = list.child(
                    div()
                        .text_color(k.faint)
                        .child(crate::i18n::text("还没有轮次", "No turns yet")),
                );
            }
            for (j, l) in a.catchup.iter().enumerate() {
                let (pane, turn) = (a.pane, l.turn);
                list = list.child(
                    div()
                        .id(("monitor-turn", n * 1000 + j))
                        .relative()
                        .children(rects::recorder(RectId::MonitorCatchupRow(n, j)))
                        .text_color(k.mark(l.mark))
                        .truncate()
                        .when(pane.is_some(), |d| d.cursor_pointer())
                        .child(turn_text(l))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if let Some(p) = pane {
                                App::defer(cx, move |cx| monitor_jump(p, Some(turn), None, cx));
                            }
                        })),
                );
            }
        }
        Card::Terminal(t) => {
            if t.catchup.is_empty() {
                list = list.child(div().text_color(k.faint).child(crate::i18n::text(
                    "还没有记录到命令",
                    "No commands recorded",
                )));
            }
            for (j, b) in t.catchup.iter().enumerate() {
                let (pane, line) = (t.pane, b.line);
                list = list.child(
                    div()
                        .id(("monitor-block", n * 1000 + j))
                        .relative()
                        .children(rects::recorder(RectId::MonitorCatchupRow(n, j)))
                        .text_color(k.mark(b.mark))
                        .truncate()
                        .cursor_pointer()
                        .child(block_text(b, false))
                        .on_click(cx.listener(move |_, _, _, cx| App::defer(cx, move |cx| monitor_jump(pane, None, line, cx)))),
                );
            }
        }
    }
    list.into_any_element()
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
