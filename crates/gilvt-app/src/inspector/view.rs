//! Draws the inspector inside a `Workspace` (mockup m3b-layout.html; card colors from m3b-part3.html): the tab
//! strip, then in 「过程」 one virtualized list: the waiting banner, the status card and the TODO block as its
//! first item, then the timeline (`timeline_view`). Without a session: the empty state.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use gpui::{div, list, prelude::*, px, relative, AnyElement, AnyWindowHandle, App, Context, CursorStyle, FontWeight, MouseButton, Window};
use gilvt_agent::PlanState;

use super::colors::Colors;
use super::model::{self, Banner, Card, Clock, Plain, Plan};
use super::timeline_model::{rows_need_tick, Entry};
use super::{timeline_view, InspectorTab};
use crate::actions::NextNeedsYou;
use crate::agents::Agents;
use crate::debug_state::rects::{self, RectId};
use crate::sidebar::model::Tone;
use crate::theme::AppSettings;
use crate::workspace::{workspaces, Workspace};

/// Whether a pane is open in some window. `ws` is the window being drawn (`me`), never read by handle.
fn reachable(ws: &Workspace, me: AnyWindowHandle, cx: &App) -> impl Fn(u64) -> bool {
    let mut panes: Vec<u64> = Vec::new();
    for w in workspaces(cx).into_iter().filter(|w| AnyWindowHandle::from(*w) != me) {
        if let Ok(other) = w.read(cx) {
            panes.extend(other.pane_ids());
        }
    }
    let mine: Vec<u64> = ws.pane_ids().collect();
    move |p| mine.contains(&p) || panes.contains(&p)
}

/// What the list's first item shows: banner, status card, TODO and the 「无法读取」 note.
struct Head {
    banner: Option<Banner>,
    card: Card,
    plan: Option<Plan>,
    failed: bool,
}

impl Head {
    /// Changes whenever the item's *layout* can change (a block appears / disappears, the tag list or
    /// TODO rows change, …), so the list only re-measures the head then. Deliberately excludes every
    /// time-derived string — `Card.turn`'s elapsed label, `Card.tokens`' counts, `Banner.detail` /
    /// `waited` — which would otherwise change every second a turn runs or a banner shows and force a
    /// re-splice; gpui resets the scroll offset within an item whenever it is respliced
    /// (`ListState::splice_focusable`), which pulled a user scrolled partway into the head back to its
    /// top every tick (M3b fix round 1). The head still repaints its current text every tick via the
    /// normal `notify` redraw — only the list's bookkeeping must not churn.
    fn signature(&self) -> u64 {
        let mut h = DefaultHasher::new();
        layout_key(self).hash(&mut h);
        h.finish()
    }

    fn render(&self, k: &Colors) -> AnyElement {
        div()
            .children(self.banner.as_ref().map(|b| render_banner(b, k)))
            .child(render_card(&self.card, k))
            .children(self.plan.as_ref().map(|p| render_plan(p, k)))
            .when(self.failed, |d| {
                d.child(
                    div()
                        .mt(px(10.))
                        .text_size(px(11.))
                        .text_color(k.muted)
                        .child(crate::i18n::text(
                            "无法读取该会话的过程",
                            "Could not read this session's activity",
                        )),
                )
            })
            .into_any_element()
    }
}

/// The head's layout-affecting shape (see `Head::signature`): every field here is either a discrete
/// state (only changes on an actual event, not on a tick) or a presence flag for a block whose exact
/// text does not change how many lines it takes (all of the card's / banner's text is `.truncate()`d
/// to one line). Everything time-derived — elapsed / wait strings and durations — is left out.
fn layout_key(head: &Head) -> String {
    let c = &head.card;
    format!(
        "{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
        head.banner.is_some(),
        c.tone,
        c.turn.is_some(),
        c.context.is_some(),
        c.model.is_some(),
        c.tokens.is_some(),
        c.tags,
        head.plan,
        head.failed,
    )
}

/// The inspector column, and whether it needs a redraw every second (a running turn's elapsed time or a
/// running row's, or a banner wait still counted in seconds).
pub fn render(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> (AnyElement, bool) {
    let theme = crate::theme::current(cx);
    let k = Colors::new(&theme.ui, theme.dark);
    if ws.inspector().tab == InspectorTab::Config {
        ws.sync_config_summary(cx);
    }
    let agents = cx.global::<Agents>();
    let clock = Clock::now();
    let focused = ws.focused_pane();
    let session = focused.and_then(|p| model::follow(agents.registry().sessions(), p));
    let view = session.and_then(|s| agents.timeline(&s.key));
    let card = session.map(|s| model::card(s, view.as_deref(), agents.permission_mode(&s.key), clock));
    let banner = model::banner(agents.registry().sessions(), session, reachable(ws, window.window_handle(), cx), |s| agents.project(s), clock.now);
    let mut tick = model::needs_tick(card.as_ref(), banner.as_ref());
    let plain = focused.map_or(Plain::Shell, |p| ws.plain_kind(p));
    let followed = session.map(|s| (s.key.clone(), s.cwd.clone()));
    let session_live = session.is_some_and(|s| s.is_live());
    let tab = ws.inspector().tab;

    let body = match tab {
        InspectorTab::Artifacts => {
            let (body, wants_tick) = super::artifacts_view::render(ws, window, cx);
            tick |= wants_tick;
            body
        }
        InspectorTab::Config => super::config_view::render(ws, k, cx),
        InspectorTab::Process => match (card, followed) {
            (Some(card), Some((key, cwd))) => {
                let plan = view.as_ref().and_then(|v| model::plan(&v.plan));
                let head = Head { banner, card, plan, failed: view.as_ref().is_some_and(|v| v.failed) };
                let turns = view.as_ref().map(|v| v.turns.clone()).unwrap_or_default();
                let ui = &mut ws.inspector_mut().timeline;
                let entries = ui.sync(&key, &turns, head.signature());
                tick |= rows_need_tick(session_live, &entries);
                let mono = timeline_view::mono_font(&cx.global::<AppSettings>().0.fallback_fonts);
                let ctx = timeline_view::Ctx { k, ws: cx.entity().downgrade(), now: clock.wall, cwd, mono };
                list(ui.list.clone(), move |ix, _, _| {
                    let item = match entries.get(ix) {
                        Some(Entry::Head) => head.render(&ctx.k),
                        Some(e) => timeline_view::entry(e, ix, &ctx),
                        None => div().into_any_element(),
                    };
                    div().px(px(10.)).child(item).into_any_element()
                })
                .flex_1()
                .pt(px(8.))
                .pb(px(14.))
                .min_h(px(0.))
                .into_any_element()
            }
            _ => div()
                .id("inspector-scroll")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .px(px(10.))
                .pt(px(8.))
                .pb(px(14.))
                .children(banner.as_ref().map(|b| render_banner(b, &k)))
                .child(render_empty(plain, &k))
                .into_any_element(),
        },
    };
    let state = ws.inspector();

    let column = div()
        .relative()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(state.width))
        .h_full()
        .overflow_hidden()
        .bg(k.bg)
        .border_l_1()
        .border_color(k.border)
        .text_size(px(12.))
        .text_color(k.text)
        // Clicks here must not move the keyboard from the terminal to the window root (gpui focuses the
        // nearest focusable ancestor on mouse down unless prevented; children run first in the bubble phase).
        .on_any_mouse_down(|_, window, _| window.prevent_default())
        .child(render_tabs(&k, state.tab, cx))
        .children(state.note.map(|(note, _)| div().flex_none().px(px(10.)).py(px(4.)).text_size(px(11.)).text_color(k.meta).child(note)))
        .child(body)
        // The boundary with the pane area: drag to resize (240–560 px).
        .child(
            div()
                .id("inspector-edge")
                .children(rects::recorder(RectId::InspectorEdge))
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(px(4.))
                .cursor(CursorStyle::ResizeLeftRight)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|ws, _, _, cx| {
                        ws.start_inspector_drag();
                        cx.stop_propagation();
                    }),
                ),
        );
    (column.into_any_element(), tick)
}

fn render_tabs(k: &Colors, current: InspectorTab, cx: &mut Context<Workspace>) -> impl IntoElement {
    let mut strip = div().flex().flex_none().items_end().gap(px(2.)).px(px(8.)).pt(px(6.)).border_b_1().border_color(k.border);
    for tab in InspectorTab::ALL {
        let on = tab == current;
        strip = strip.child(
            div()
                .id(tab.label())
                .px(px(10.))
                .py(px(5.))
                .rounded_t(px(6.))
                .text_color(if on { k.tab_on } else if tab.unavailable_note().is_some() { k.tab_off } else { k.tab })
                .when(on, |d| d.bg(k.card).border_1().border_color(k.border).border_b_0().mb(px(-1.)))
                .child(tab.label())
                .on_click(cx.listener(move |ws, _, window, cx| ws.choose_inspector_tab(tab, window, cx))),
        );
    }
    strip.child(div().ml_auto().px(px(2.)).py(px(5.)).text_size(px(11.)).text_color(k.hint).child("⌘I"))
}

/// 「⏳ codex · web 在等审批」; a click is ⌘⇧J.
fn render_banner(b: &Banner, k: &Colors) -> impl IntoElement {
    div()
        .id("inspector-banner")
        .relative()
        .children(rects::recorder(RectId::InspectorBanner))
        .mb(px(8.))
        .px(px(9.))
        .py(px(7.))
        .rounded(px(8.))
        .bg(k.banner)
        .border_1()
        .border_color(k.card_y_border)
        .cursor_pointer()
        .hover(|s| s.bg(k.banner_hover))
        .child(
            div()
                .flex()
                .justify_between()
                .gap(px(6.))
                .child(
                    div()
                        .min_w(px(0.))
                        .truncate()
                        .font_weight(FontWeight::BOLD)
                        .text_color(k.yellow)
                        .child(b.title.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(k.yellow)
                        .child(crate::i18n::text("⌘⇧J 跳过去", "⇧⌘J Jump there")),
                ),
        )
        .child(
            div()
                .truncate()
                .text_size(px(11.))
                .line_height(relative(1.6))
                .text_color(k.meta)
                .child(b.detail.clone()),
        )
        .child(div().truncate().text_size(px(11.)).line_height(relative(1.6)).text_color(k.meta).child(b.detail.clone()))
        .on_click(|_, window, cx| window.dispatch_action(Box::new(NextNeedsYou), cx))
}

fn render_card(c: &Card, k: &Colors) -> impl IntoElement {
    let (bg, border) = match c.tone {
        Tone::Waiting => (k.card_y, k.card_y_border),
        Tone::Error => (k.card_r, k.card_r_border),
        _ => (k.card, k.card_border),
    };
    let meta = [c.context.as_ref().map(|b| b.label.clone()), c.model.clone(), c.tokens.clone()];
    div()
        .mb(px(8.))
        .px(px(10.))
        .py(px(8.))
        .rounded(px(8.))
        .bg(bg)
        .border_1()
        .border_color(border)
        .child(
            div()
                .flex()
                .justify_between()
                .gap(px(8.))
                .child(div().min_w(px(0.)).truncate().font_weight(FontWeight::SEMIBOLD).text_color(k.tone(c.tone)).child(c.status.clone()))
                .children(c.turn.clone().map(|t| div().flex_none().text_color(k.muted).child(t))),
        )
        .children(c.context.as_ref().map(|b| {
            div()
                .mt(px(6.))
                .mb(px(3.))
                .h(px(5.))
                .rounded(px(3.))
                .overflow_hidden()
                .bg(k.track)
                .child(div().h_full().w(relative(b.ratio)).bg(k.meter(b.meter)))
        }))
        .child(
            div()
                .text_size(px(11.))
                .line_height(relative(1.6))
                .text_color(k.meta)
                .children(meta.into_iter().flatten().map(|line| div().truncate().child(line))),
        )
        .when(!c.tags.is_empty(), |d| {
            d.child(div().flex().flex_wrap().gap(px(4.)).mt(px(3.)).children(c.tags.iter().map(|t| {
                div().px(px(6.)).rounded(px(8.)).bg(k.tag).text_color(k.tag_text).text_size(px(10.5)).child(*t)
            })))
        })
}

/// 「TODO · 3/5」: ☑ done (struck through), ◐ active (bold), ☐ to do.
fn render_plan(p: &Plan, k: &Colors) -> impl IntoElement {
    div()
        .child(div().mt(px(10.)).mb(px(4.)).text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(k.head).child(p.title.clone()))
        .children(p.rows.iter().map(|r| {
            let row = div().py(px(1.5)).child(format!("{} {}", r.glyph, r.text));
            match r.state {
                PlanState::Done => row.text_color(k.done).line_through(),
                PlanState::Active => row.font_weight(FontWeight::BOLD),
                PlanState::Todo => row,
            }
        }))
}

/// 「当前 pane 是普通 shell / 运行 claude 或 codex 后，这里显示它的执行过程」.
fn render_empty(plain: Plain, k: &Colors) -> impl IntoElement {
    let bold = |t: &'static str| div().font_weight(FontWeight::BOLD).child(t);
    div()
        .px(px(8.))
        .py(px(40.))
        .flex()
        .flex_col()
        .items_center()
        .line_height(relative(1.8))
        .text_color(k.empty)
        .child(plain.title())
        .child(
            if crate::i18n::current() == crate::i18n::Language::English {
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .child("Run ")
                    .child(bold("claude"))
                    .child(" or ")
                    .child(bold("codex"))
                    .child(" to see its activity here")
            } else {
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .child(crate::i18n::text("运行 ", "Run "))
                    .child(bold("claude"))
                    .child(crate::i18n::text(" 或 ", " or "))
                    .child(bold("codex"))
                    .child(crate::i18n::text(" 后，这里显示它的执行过程", " to see its activity here"))
            },
        )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::model::PlanRow;
    use super::*;

    fn card(tone: Tone, turn: Option<&str>, tokens: Option<&str>, tags: Vec<&'static str>) -> Card {
        Card { tone, status: "status".into(), turn: turn.map(String::from), context: None, model: None, tokens: tokens.map(String::from), tags, running: true }
    }

    fn banner(waited_secs: u64) -> Banner {
        Banner { title: "title".into(), detail: format!("等了 {waited_secs}s"), waited: Duration::from_secs(waited_secs) }
    }

    #[test]
    fn signature_ignores_elapsed_and_wait_strings() {
        let a = Head { banner: Some(banner(1)), card: card(Tone::Running, Some("第 1 轮 · 1s"), Some("本轮 1 · 会话 2 tokens"), vec![]), plan: None, failed: false };
        let b = Head { banner: Some(banner(90)), card: card(Tone::Running, Some("第 1 轮 · 1m30s"), Some("本轮 9 · 会话 20 tokens"), vec![]), plan: None, failed: false };
        assert_eq!(a.signature(), b.signature());
    }

    #[test]
    fn signature_changes_when_a_block_or_tag_appears() {
        let base = Head { banner: None, card: card(Tone::Running, None, None, vec![]), plan: None, failed: false };
        let base_sig = base.signature();

        let with_banner = Head { banner: Some(banner(0)), card: card(Tone::Running, None, None, vec![]), plan: None, failed: false };
        assert_ne!(with_banner.signature(), base_sig);

        let with_tag = Head { card: card(Tone::Running, None, None, vec!["精简模式"]), banner: None, plan: None, failed: false };
        assert_ne!(with_tag.signature(), base_sig);

        let with_plan = Head {
            banner: None,
            card: card(Tone::Running, None, None, vec![]),
            plan: Some(Plan { title: "TODO · 0/1".into(), rows: vec![PlanRow { glyph: "☐", text: "a".into(), state: PlanState::Todo }] }),
            failed: false,
        };
        assert_ne!(with_plan.signature(), base_sig);

        let failed = Head { banner: None, card: card(Tone::Running, None, None, vec![]), plan: None, failed: true };
        assert_ne!(failed.signature(), base_sig);
    }
}
