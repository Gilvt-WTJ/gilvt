//! Draws the sidebar model inside a `Workspace` (mockups m3a-sidebar.html / m3a-switching.html), plus the
//! confirm bar of an ended session's 移到废纸篓… (M3c §6) in place of the key hints.

use std::time::{Instant, SystemTime};

use gpui::{
    div, prelude::*, px, relative, AnyElement, AnyWindowHandle, App, ClickEvent, Context, Entity, FontWeight, Hsla, MouseButton,
    MouseDownEvent, SharedString, Window,
};

use super::model::{self, Grouping, Item, Meter, Row, RowKind, Section, SectionKind, TerminalItem, Tone};
use super::ended;
use super::rename::RenameField;
use super::tooltip::SidebarTooltip;
use gilvt_theme::UiColors;

use crate::agents::Agents;
use crate::theme::hsla;
use crate::debug_state::rects::{self, RectId};
use crate::launcher::sessions_view::Confirm;
use crate::launcher::Location;
use crate::theme::AppSettings;
use crate::workspace::{focus_pane_anywhere, resume_pending_anywhere, workspaces, Workspace};

pub const WIDTH: f32 = 240.;

struct Colors {
    bg: Hsla,
    border: Hsla,
    text: Hsla,
    group: Hsla,
    time: Hsla,
    place: Hsla,
    faint: Hsla,
    need: Hsla,
    need_bg: Hsla,
    current: Hsla,
    hover: Hsla,
    seg: Hsla,
    seg_on: Hsla,
    seg_text: Hsla,
    seg_on_text: Hsla,
    blue: Hsla,
    red: Hsla,
    green: Hsla,
    track: Hsla,
    fill: Hsla,
    warn: Hsla,
    full: Hsla,
    claude: Hsla,
    codex: Hsla,
    codex_text: Hsla,
    pill: Hsla,
    pill_text: Hsla,
    term_icon: Hsla,
    term_icon_text: Hsla,
    ai: Hsla,
}

impl Colors {
    fn new(ui: &UiColors) -> Colors {
        let h = hsla;
        Colors {
            bg: h(ui.panel),
            border: h(ui.border),
            text: h(ui.text),
            group: h(ui.text_2),
            time: h(ui.text_3),
            place: h(ui.text_4),
            faint: h(ui.text_3),
            need: h(ui.attention.fg),
            need_bg: h(ui.attention.bg),
            current: h(ui.selected),
            hover: h(ui.hover),
            seg: h(ui.fill),
            seg_on: h(ui.fill_on),
            seg_text: h(ui.text_2),
            seg_on_text: h(ui.text),
            blue: h(ui.running.fg),
            red: h(ui.error.fg),
            green: h(ui.done.fg),
            track: h(ui.fill),
            fill: h(ui.meter),
            warn: h(ui.attention.ring),
            full: h(ui.error.ring),
            claude: h(ui.claude),
            codex: h(ui.codex),
            codex_text: h(ui.codex_on),
            pill: h(ui.fill),
            pill_text: h(ui.text_3),
            term_icon: h(ui.fill),
            term_icon_text: h(ui.text_3),
            ai: h(ui.purple),
        }
    }

    fn tone(&self, t: Tone) -> Hsla {
        match t {
            Tone::Waiting => self.need,
            Tone::Running => self.blue,
            Tone::Error => self.red,
            Tone::Done => self.green,
            Tone::Muted => self.faint,
        }
    }
}

/// "12:40" in local time.
fn clock(t: Instant) -> String {
    let at = SystemTime::now() - Instant::now().saturating_duration_since(t);
    let secs = at.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return String::new();
    }
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// The sidebar model for window `ws` (`me` = its handle): sessions of all windows, located in the window
/// that owns their pane. Safe inside `ws`'s own update or render: window `me` is never read by handle.
/// The row's git line; a linked worktree is prefixed with its directory name so it can be told from the main checkout.
fn git_text(g: &gilvt_agent::GitInfo) -> String {
    let line = gilvt_agent::display_line(g, 28);
    match g.repo_root.file_name().filter(|_| g.is_linked_worktree) {
        Some(dir) => format!("{} · {line}", dir.to_string_lossy()),
        None => line,
    }
}

/// Every workspace window as (index in `workspaces(cx)`, workspace), with `ws` standing in for window `me`
/// (never read by handle: safe inside `ws`'s own update or render). Sorted by index.
pub fn all_windows<'a>(ws: &'a Workspace, me: AnyWindowHandle, cx: &'a App) -> Vec<(usize, &'a Workspace)> {
    let mine = workspaces(cx).iter().position(|w| AnyWindowHandle::from(*w) == me).unwrap_or(0);
    let mut all: Vec<(usize, &Workspace)> = workspaces(cx)
        .into_iter()
        .enumerate()
        .filter(|(_, w)| AnyWindowHandle::from(*w) != me)
        .filter_map(|(i, w)| Some((i, w.read(cx).ok()?)))
        .collect();
    all.push((mine, ws));
    all.sort_by_key(|(i, _)| *i);
    all
}

/// Sessions and plain terminal panes of every window, as the sidebar lists them.
pub struct Collected<'a> {
    pub items: Vec<Item<'a>>,
    pub terminals: Vec<TerminalItem>,
}

/// Sessions and plain terminal panes of every window as the sidebar lists them (also the monitor tab's input).
pub fn collect<'a>(ws: &'a Workspace, me: AnyWindowHandle, cx: &'a App) -> Collected<'a> {
    let agents = cx.global::<Agents>();
    let all_windows = all_windows(ws, me, cx);
    let windows = all_windows.len();
    let mine = all_windows.iter().find(|(_, w)| std::ptr::eq(*w, ws)).map_or(0, |(i, _)| *i);
    let others: Vec<(usize, &Workspace)> = all_windows.iter().copied().filter(|(_, w)| !std::ptr::eq(*w, ws)).collect();
    let focused = ws.focused_pane();
    let sessions: Vec<_> = agents.registry().sessions().collect();
    // The agents' own titles, from the history index (as of its last refresh).
    let agent_titles: std::collections::HashMap<gilvt_agent::SessionKey, String> = cx
        .try_global::<crate::launcher::History>()
        .map(|history| {
            history
                .snapshot()
                .entries
                .iter()
                .filter_map(|e| {
                    let title = e.custom_title.clone().or_else(|| e.ai_title.clone())?;
                    Some(((e.agent, e.session_id.clone()), title))
                })
                .collect()
        })
        .unwrap_or_default();
    let history = cx.try_global::<crate::launcher::History>();
    let indexed_cwd = |s: &gilvt_agent::Session| -> Option<String> {
        let dir = &history?.find(s.agent(), s.session_id())?.cwd;
        (!dir.as_os_str().is_empty()).then(|| dir.display().to_string())
    };
    let archived = |key: &gilvt_agent::SessionKey| {
        let Some(service) = cx.try_global::<crate::review::ReviewService>() else { return false };
        ended::is_archived(&service.state(key), history.and_then(|h| h.find(key.0, &key.1)))
    };
    let items: Vec<Item> = sessions
        .iter()
        .map(|s| {
            let found = s.pane.and_then(|p| {
                if ws.has_pane(p) {
                    return Some((mine, ws.pane_location(p, cx)?));
                }
                others.iter().find(|(_, o)| o.has_pane(p)).and_then(|(i, o)| Some((*i, o.pane_location(p, cx)?)))
            });
            let reachable = found.is_some();
            let location = found
                .map(|(i, l)| {
                    let window = (windows > 1).then(|| format!("窗口 {}", i + 1));
                    model::location_text(&l.tab_title, l.position, window.as_deref())
                })
                .unwrap_or_default();
            Item {
                session: s,
                project: agents.project(s),
                reachable,
                location,
                git: agents.git(s).map(git_text),
                // An ended row names the directory the history indexed (the one a resume goes to).
                cwd: indexed_cwd(s).or_else(|| s.cwd.as_ref().map(|c| c.display().to_string())).unwrap_or_default(),
                current: s.pane.is_some() && s.pane == focused,
                agent_title: agent_titles.get(&s.key).cloned(),
                archived: !s.is_live() && archived(&s.key),
            }
        })
        .collect();
    // Every terminal pane of every window that no agent (live or waiting to be resumed) occupies.
    let occupied = agents.occupied_panes();
    let terminals: Vec<TerminalItem> = all_windows
        .iter()
        .flat_map(|(i, w)| {
            w.terminal_panes(cx).into_iter().filter(|t| !occupied.contains(&t.pane)).filter_map(move |t| {
                let loc = w.pane_location(t.pane, cx)?;
                let window = (windows > 1).then(|| format!("窗口 {}", i + 1));
                let dir = t.cwd.as_deref();
                Some(TerminalItem {
                    pane: t.pane,
                    name: model::terminal_name(&t.title),
                    project: model::terminal_project(dir.map(|d| agents.project_of_dir(d))),
                    location: model::location_text(&loc.tab_title, loc.position, window.as_deref()),
                    git: dir.and_then(|d| agents.git_of_dir(d)).map(git_text),
                    cwd: dir.map(|d| d.display().to_string()).unwrap_or_default(),
                    current: Some(t.pane) == focused,
                })
            })
        })
        .collect();
    Collected { items, terminals }
}

pub fn model_for(ws: &Workspace, me: AnyWindowHandle, cx: &App) -> model::Model {
    let c = collect(ws, me, cx);
    let m = model::with_pending(model::build(&c.items, &c.terminals, ws.sidebar(), Instant::now(), &clock), cx.global::<Agents>().pending());
    let monitor = &cx.global::<AppSettings>().0.monitor;
    if !monitor.enabled || !monitor.sidebar_summary {
        return m;
    }
    use crate::monitor::summaries;
    model::with_summaries(m, |r| {
        if !r.cwd.is_empty() && summaries::is_excluded(Some(std::path::Path::new(&r.cwd)), cx) {
            return None;
        }
        let key = match (&r.key, r.pane) {
            (Some(k), _) => summaries::agent_key(k),
            (None, Some(p)) if r.kind == model::RowKind::Terminal => summaries::pane_key(p),
            _ => return None,
        };
        let recent = summaries::view(&key, cx)?.summary.as_ref()?.recent.clone();
        Some(model::first_sentence(&recent)).filter(|s| !s.is_empty())
    })
}

pub fn render(ws: &Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    let theme = crate::theme::current(cx);
    let k = Colors::new(&theme.ui);
    let m = model_for(ws, window.window_handle(), cx);
    let grouping = ws.sidebar().grouping;
    let seg = |label: &'static str, g: Grouping, cx: &mut Context<Workspace>| {
        let on = grouping == g;
        div()
            .id(label)
            .px(px(8.))
            .py(px(2.))
            .rounded(px(5.))
            .text_size(px(11.))
            .text_color(if on { k.seg_on_text } else { k.seg_text })
            .when(on, |d| d.bg(k.seg_on).shadow_sm())
            .relative()
            .children(rects::recorder(RectId::SidebarGrouping(if g == Grouping::Project { 0 } else { 1 })))
            .child(label)
            .on_click(cx.listener(move |ws, _, _, cx| ws.update_sidebar(|s| s.grouping = g, cx)))
    };
    let head = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .px(px(10.))
        .pt(px(8.))
        .pb(px(6.))
        .text_color(k.group)
        .child(model::header_text(&m))
        .child(div().flex().p(px(2.)).rounded(px(6.)).bg(k.seg).child(seg("按项目", Grouping::Project, cx)).child(seg("按状态", Grouping::Status, cx)));
    // 待 Review N: the same number as the Session Center's tab; a click opens the center on that tab.
    let pending = crate::session_center::pending::pending_review_count(cx);
    let review_entry = div()
        .id("sidebar-review-entry")
        .relative()
        .children(rects::recorder(RectId::SidebarReviewEntry))
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .mx(px(6.))
        .mb(px(4.))
        .px(px(6.))
        .py(px(4.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|d| d.bg(k.seg))
        .text_color(if pending > 0 { k.text } else { k.faint })
        .child("待 Review")
        .child(
            div()
                .px(px(7.))
                .rounded(px(9.))
                .text_size(px(11.))
                .bg(if pending > 0 { k.seg_on } else { k.seg })
                .text_color(if pending > 0 { k.seg_on_text } else { k.faint })
                .child(pending.to_string()),
        )
        .on_click(cx.listener(|ws, _, window, cx| ws.open_sessions_on(crate::session_center::model::Tab::Review, window, cx)));
    let mut list = div().id("sidebar-list").flex_1().min_h(px(0.)).overflow_y_scroll().pb(px(6.));
    if m.sections.is_empty() {
        list = list.child(
            div()
                .px(px(12.))
                .py(px(16.))
                .text_size(px(11.))
                .text_color(k.faint)
                .child("还没有会话")
                .child(div().mt(px(4.)).text_color(k.place).child("在终端里运行 claude 或 codex 后会出现在这里")),
        );
    }
    // The rename field goes in the first row of its session (需要你 repeats rows), even when collapsed.
    let mut rows = model::visible_rows(&m, ws.renaming_key()).into_iter().enumerate().peekable();
    let mut field_shown = false;
    for (si, section) in m.sections.iter().enumerate() {
        list = list.child(section_header(si, section, &k, cx));
        while let Some((n, (_, row))) = rows.next_if(|(_, (s, _))| *s == si) {
            let field = if field_shown { None } else { row.key.as_ref().and_then(|k| ws.rename_field(k)) };
            field_shown |= field.is_some();
            list = list.child(render_row(n, row, section.kind == SectionKind::Pending, field, &k, cx));
        }
    }
    let bottom = match ws.ended_confirm() {
        Some(confirm) => confirm_bar(confirm, &k, cx),
        None => div()
            .flex_none()
            .border_t_1()
            .border_color(k.track)
            .px(px(10.))
            .py(px(6.))
            .text_size(px(11.))
            .text_color(k.time)
            .line_height(relative(1.6))
            .child("点击：跳到该 pane · ⌘⇧J 需要你 · ⌘⇧↑↓ 上下一个会话 · ⌘B 折叠左栏")
            .into_any_element(),
    };
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(WIDTH))
        .h_full()
        .overflow_hidden()
        .bg(k.bg)
        .border_r_1()
        .border_color(k.border)
        .text_size(px(12.))
        .text_color(k.text)
        .child(head)
        .child(review_entry)
        .child(list)
        .child(bottom)
        .into_any_element()
}

/// 「1 个会话 · X MB 将移到废纸篓（可从废纸篓还原）」 with the session's title above, ［取消］［移到废纸篓］.
fn confirm_bar(confirm: &Confirm, k: &Colors, cx: &mut Context<Workspace>) -> AnyElement {
    let title = confirm.entries.first().map(|e| {
        let saved = cx.global::<Agents>().registry().saved_name(&(e.agent, e.session_id.clone()));
        crate::launcher::history::title(e, saved)
    });
    let button = |id: &'static str, n: usize, label: &'static str| {
        div()
            .id(id)
            .relative()
            .children(rects::recorder(RectId::SidebarTrashButton(n)))
            .flex_none()
            .px(px(10.))
            .py(px(2.))
            .rounded(px(6.))
            .border_1()
            .bg(k.seg_on)
            .child(label)
    };
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(6.))
        .border_t_1()
        .border_color(k.track)
        .bg(k.hover)
        .px(px(10.))
        .py(px(8.))
        .text_size(px(11.5))
        .line_height(relative(1.5))
        .children(title.map(|t| div().truncate().font_weight(FontWeight::SEMIBOLD).child(t)))
        .child(confirm.text.clone())
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(6.))
                .child(button("ended-confirm-cancel", 0, "取消").border_color(k.border).on_click(cx.listener(|ws, _, window, cx| ws.cancel_trash_ended(window, cx))))
                .child(
                    button("ended-confirm-trash", 1, "移到废纸篓")
                        .border_color(k.red)
                        .text_color(k.red)
                        .on_click(cx.listener(|ws, _, window, cx| ws.trash_ended_confirmed(window, cx))),
                ),
        )
        .into_any_element()
}

fn section_header(i: usize, s: &Section, k: &Colors, cx: &mut Context<Workspace>) -> AnyElement {
    if s.kind == SectionKind::NeedsYou {
        return div()
            .relative()
            .children(rects::recorder(RectId::SidebarSection(i)))
            .flex()
            .justify_between()
            .px(px(10.))
            .pt(px(6.))
            .pb(px(2.))
            .text_size(px(11.))
            .text_color(k.need)
            .font_weight(FontWeight::SEMIBOLD)
            .child(s.title.clone())
            .child("⌘⇧J")
            .into_any_element();
    }
    if s.kind == SectionKind::Pending {
        return div()
            .relative()
            .children(rects::recorder(RectId::SidebarSection(i)))
            .flex()
            .justify_between()
            .px(px(10.))
            .pt(px(8.))
            .pb(px(2.))
            .text_size(px(11.))
            .text_color(k.group)
            .child(s.title.clone())
            .child(
                div()
                    .id(("sidebar-resume-all", i))
                    .text_color(k.place)
                    .hover(|d| d.text_color(k.group))
                    .child("全部恢复")
                    .on_click(cx.listener(|ws, _, window, cx| ws.resume_all_pending(window, cx))),
            )
            .into_any_element();
    }
    let (kind, id, default) = (s.kind, s.id.clone(), s.default_collapsed);
    div()
        .id(("sidebar-section", i))
        .relative()
        .children(rects::recorder(RectId::SidebarSection(i)))
        .flex()
        .gap(px(4.))
        .px(px(10.))
        .pt(px(8.))
        .pb(px(2.))
        .text_size(px(11.))
        .text_color(k.group)
        .child(format!("{} {}", if s.collapsed { "▸" } else { "▾" }, s.title))
        .when(!s.note.is_empty(), |d| d.child(div().text_color(k.place).child(format!("· {}", s.note))))
        .on_click(cx.listener(move |ws, _, _, cx| {
            let id = id.clone();
            ws.update_sidebar(
                move |st| match kind {
                    SectionKind::Ended => st.ended_open = !st.ended_open,
                    _ => st.toggle(&id, default),
                },
                cx,
            )
        }))
        .into_any_element()
}

fn render_row(n: usize, r: &Row, pending: bool, field: Option<Entity<RenameField>>, k: &Colors, cx: &mut Context<Workspace>) -> AnyElement {
    let terminal = r.kind == RowKind::Terminal;
    let bg = if r.current { Some(k.current) } else if r.needs_you { Some(k.need_bg) } else { None };
    let (icon_bg, icon_text) = if terminal {
        (k.term_icon, k.term_icon_text)
    } else if r.letter == 'C' {
        (k.claude, gpui::white())
    } else {
        (k.codex, k.codex_text)
    };
    let icon_label: SharedString = if terminal { ">_".into() } else { r.letter.to_string().into() };
    let name: SharedString = if r.muted { format!("{} 🔕", r.name).into() } else { r.name.clone().into() };
    // Clicking the row being renamed stays in its field.
    // A 待恢复 row has no live session: a single click resumes it (which also focuses its pane).
    let pane = r.pane.filter(|_| field.is_none() && !pending);
    let resume_pending = pending && field.is_none();
    // 已结束: a double click resumes it where ↩ would (M3c §6).
    let resumable = r.ended && field.is_none();
    let renaming = field.is_some();
    let status = div()
        .flex()
        .items_center()
        .gap(px(4.))
        .mt(px(1.))
        .text_size(px(11.))
        .child(div().min_w(px(0.)).truncate().text_color(k.tone(r.tone)).child(r.status.clone()))
        .when(r.lite, |d| {
            d.child(div().flex_none().px(px(5.)).rounded(px(4.)).bg(k.pill).text_color(k.pill_text).text_size(px(10.)).child("精简模式"))
        });
    let main = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .justify_between()
                .gap(px(6.))
                .child(match field {
                    Some(field) => div().flex_1().min_w(px(0.)).child(field),
                    // An ended row: the directory's last component, grey, after the name.
                    None if !r.dir.is_empty() => div()
                        .min_w(px(0.))
                        .flex()
                        .items_baseline()
                        .gap(px(6.))
                        .child(div().min_w(px(0.)).truncate().font_weight(FontWeight::SEMIBOLD).child(name))
                        .child(div().flex_none().max_w(px(90.)).truncate().text_size(px(11.)).text_color(k.time).child(r.dir.clone())),
                    None => div()
                        .min_w(px(0.))
                        .line_clamp(2)
                        .font_weight(if terminal { FontWeight::NORMAL } else { FontWeight::SEMIBOLD })
                        .when(terminal, |d| d.text_color(k.group))
                        .child(name),
                })
                .when(!r.time.is_empty(), |d| d.child(div().flex_none().text_size(px(11.)).text_color(k.time).child(r.time.clone()))),
        )
        .when(!terminal, |d| d.child(status))
        .when_some(r.summary.clone(), |d, s| {
            d.child(div().mt(px(1.)).truncate().text_size(px(10.5)).text_color(k.ai).child(SharedString::from(format!("✦ {s}"))))
        })
        .when(!r.location.is_empty(), |d| d.child(div().mt(px(1.)).truncate().text_size(px(10.5)).text_color(k.place).child(r.location.clone())))
        .when(!r.git.is_empty(), |d| d.child(div().mt(px(1.)).truncate().text_size(px(10.5)).text_color(k.place).child(r.git.clone())))
        .when_some(r.context, |d, (ratio, meter)| {
            let fill = match meter {
                Meter::Normal => k.fill,
                Meter::Warn => k.warn,
                Meter::Full => k.full,
            };
            d.child(div().mt(px(4.)).h(px(3.)).rounded(px(2.)).bg(k.track).child(div().h_full().w(relative(ratio)).rounded(px(2.)).bg(fill)))
        });
    div()
        .id(("sidebar-row", n))
        .relative()
        .children(rects::recorder(RectId::SidebarRow(n)))
        .mx(px(6.))
        .my(px(1.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(7.))
        .flex()
        .items_start()
        .gap(px(8.))
        .when_some(bg, |d, bg| d.bg(bg))
        .when(bg.is_none() && (pane.is_some() || resumable || resume_pending), |d| d.hover(|s| s.bg(k.hover)))
        .when(r.ended, |d| d.opacity(0.55))
        .when(!renaming, |d| {
            let lines = model::tooltip_lines(r);
            d.tooltip(move |_, cx| cx.new(|_| SidebarTooltip::new(lines.clone())).into())
        })
        .child(
            div()
                .flex_none()
                .size(px(20.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .justify_center()
                .bg(icon_bg)
                .text_color(icon_text)
                .text_size(px(if terminal { 10. } else { 11. }))
                .font_weight(FontWeight::BOLD)
                .child(icon_label),
        )
        .child(main)
        .when_some(pane, |d, pane| d.on_click(cx.listener(move |_, _, _, cx| App::defer(cx, move |cx| focus_pane_anywhere(pane, cx)))))
        .when_some(r.key.clone().filter(|_| resume_pending), |d, key| {
            d.on_click(cx.listener(move |_, _, _, cx| {
                let key = key.clone();
                App::defer(cx, move |cx| resume_pending_anywhere(&key, cx))
            }))
        })
        .when_some(r.key.clone().filter(|_| resumable), |d, key| {
            d.on_click(cx.listener(move |ws, e: &ClickEvent, window, cx| {
                if e.click_count() >= 2 {
                    ws.resume_ended(key.clone(), Location::Smart, window, cx);
                }
            }))
        })
        .when_some(r.key.clone().filter(|_| !pending), |d, key| {
            d.on_mouse_down(
                MouseButton::Right,
                cx.listener(move |ws, e: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    ws.open_session_menu(key.clone(), e.position, window, cx);
                }),
            )
        })
        .into_any_element()
}
