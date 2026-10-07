//! Draws the timeline's list items (mockup m3b-part3.html): the title with the filters, tool rows (▸ on
//! hover, icon, summary, time; error lines under a failure; the expanded detail), thinking rows, subagent
//! children behind a purple rule, 「↩ …」, 「另有 N 条」 and the history turns. Items are built inside the
//! list's render callback (only visible ones), so every handler talks to the `Workspace` through a weak
//! handle. Nothing here writes to a PTY.

use std::path::PathBuf;
use std::time::SystemTime;

use gpui::{
    div, prelude::*, px, relative, AnyElement, ClickEvent, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, HighlightStyle,
    InteractiveText, SharedString, StyledText, UnderlineStyle, WeakEntity,
};

use super::colors::Colors;
use super::timeline_model::{detail_text, resolve_path, summary, DetailLine, Entry, Filter, Ink, Row, RowKind, ToolRow};
use crate::debug_state::rects::{self, RectId};
use crate::workspace::Workspace;

/// Monospace text (error lines, details). Explicit fallbacks, the terminal's (`fallback_fonts`): without them
/// gpui 0.2.2 shapes Chinese with a system fallback but draws the glyph ids from Menlo (「复制」 came out
/// as 「突推」).
pub(super) fn mono_font(fallbacks: &[String]) -> Font {
    Font {
        family: "Menlo".into(),
        features: FontFeatures::default(),
        fallbacks: Some(FontFallbacks::from_fonts(fallbacks.to_vec())),
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

/// The proportional UI font, for labels inside monospace blocks.
const UI_FONT: &str = ".SystemUIFont";

/// What every item needs.
pub(super) struct Ctx {
    pub k: Colors,
    pub ws: WeakEntity<Workspace>,
    pub now: SystemTime,
    /// The session's cwd (relative file names in rows resolve against it).
    pub cwd: Option<PathBuf>,
    /// See [`mono_font`].
    pub mono: Font,
}

impl Ctx {
    fn ink(&self, ink: Ink) -> gpui::Hsla {
        match ink {
            Ink::Text => self.k.text,
            Ink::Running => self.k.blue,
            Ink::Failed => self.k.red,
            Ink::Warn => self.k.yellow,
            Ink::Subagent => self.k.purple,
            Ink::Muted => self.k.muted,
        }
    }

    /// A click handler running `f` on the workspace.
    fn on_ws(&self, f: impl Fn(&mut Workspace, &ClickEvent, &mut gpui::Window, &mut gpui::Context<Workspace>) + 'static) -> impl Fn(&ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static {
        let ws = self.ws.clone();
        move |e, window, cx| {
            let _ = ws.update(cx, |ws, cx| f(ws, e, window, cx));
        }
    }
}

/// Item `ix` (not the head).
pub(super) fn entry(e: &Entry, ix: usize, cx: &Ctx) -> AnyElement {
    match e {
        Entry::Head => div().into_any_element(),
        Entry::Title { turn, filter, started } => title(*turn, *filter, started.as_deref(), ix, cx),
        Entry::Row(row) => row_item(row, ix, cx),
        Entry::Note(note) => {
            div().relative().children(rects::recorder(RectId::TimelineRow(ix))).px(px(6.)).py(px(6.)).text_color(cx.k.muted).child(*note).into_any_element()
        }
        Entry::History { index, label, open, .. } => {
            let index = *index;
            div()
                .id(("tl-hist", ix))
                .relative()
                .children(rects::recorder(RectId::TimelineRow(ix)))
                .mt(px(4.))
                .px(px(4.))
                .py(px(4.))
                .border_t_1()
                .border_dashed()
                .border_color(cx.k.dash)
                .text_color(cx.k.hist)
                .cursor_pointer()
                .hover(|s| s.bg(cx.k.row_hover))
                .truncate()
                .child(format!("{} {label}", if *open { "▾" } else { "▸" }))
                .on_click(cx.on_ws(move |ws, _, _, cx| ws.toggle_timeline_turn(index, cx)))
                .into_any_element()
        }
    }
}

/// 「时间线 · 第 N 轮 · 14:30:12」 (the start muted) and 全部 / Bash / 编辑 / 失败.
fn title(turn: u32, on: Filter, started: Option<&str>, ix: usize, cx: &Ctx) -> AnyElement {
    let k = &cx.k;
    let chips = Filter::ALL.into_iter().enumerate().map(|(n, f)| {
        let active = f == on;
        div()
            .id(f.label())
            .relative()
            .children(rects::recorder(RectId::TimelineChip(n)))
            .px(px(8.))
            .py(px(1.))
            .rounded(px(10.))
            .text_size(px(11.))
            .bg(if active { k.chip_on } else { k.chip })
            .text_color(if active { k.chip_text_on } else { k.chip_text })
            .cursor_pointer()
            .child(f.label())
            .on_click(cx.on_ws(move |ws, _, _, cx| ws.set_timeline_filter(f, cx)))
    });
    div()
        .child(
            div()
                .relative()
                .children(rects::recorder(RectId::TimelineRow(ix)))
                .mt(px(10.))
                .mb(px(4.))
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .flex()
                .items_baseline()
                .text_color(k.head)
                .child(
                    if crate::i18n::current() == crate::i18n::Language::English {
                        format!("Timeline · Turn {turn}")
                    } else {
                        format!("时间线 · 第 {turn} 轮")
                    },
                )
                .children(started.map(|t| {
                    div()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(k.dur)
                        .child(format!("\u{a0}·\u{a0}{t}"))
                })),
        )
        .child(
            div()
                .flex()
                .gap(px(4.))
                .mt(px(2.))
                .mb(px(6.))
                .children(chips),
        )
        .into_any_element()
}

/// A row, indented behind a purple rule inside a subagent, or under an expanded history turn.
fn row_item(row: &Row, ix: usize, cx: &Ctx) -> AnyElement {
    let body = match &row.kind {
        RowKind::Tool(t) => tool(row, t, ix, cx),
        RowKind::Thinking { secs, expandable, text } => thinking(row, secs.as_deref(), *expandable, text.as_deref(), ix, cx),
        RowKind::Returned(result) => line("↩", result.clone(), cx.k.muted, ix),
        RowKind::Truncated(n) => line(
            "…",
            if crate::i18n::current() == crate::i18n::Language::English {
                format!("{n} more")
            } else {
                format!("另有 {n} 条")
            },
            cx.k.muted,
            ix,
        ),
        RowKind::Empty(note) => line("", (*note).to_string(), cx.k.muted, ix),
    };
    let body = if row.sub { div().ml(px(10.)).pl(px(6.)).border_l_2().border_color(cx.k.rule).child(body).into_any_element() } else { body };
    if row.history {
        div().ml(px(12.)).child(body).into_any_element()
    } else {
        body
    }
}

/// A plain row (no chevron): icon + text, both in `color`.
fn line(icon: &'static str, text: String, color: gpui::Hsla, ix: usize) -> AnyElement {
    div()
        .relative()
        .children(rects::recorder(RectId::TimelineRow(ix)))
        .flex()
        .gap(px(6.))
        .px(px(4.))
        .py(px(3.))
        .child(div().w(px(12.)).flex_none())
        .child(div().w(px(16.)).flex_none().flex().justify_center().child(icon))
        .child(div().flex_1().min_w(px(0.)).truncate().child(text))
        .text_color(color)
        .into_any_element()
}

/// ▸ (on hover; ▾ while open): opens / closes the detail without jumping.
fn chevron(key: &str, open: bool, ix: usize, cx: &Ctx) -> impl IntoElement {
    let key = key.to_string();
    div()
        .id(("tl-chev", ix))
        .relative()
        .children(rects::recorder(RectId::TimelineToggle(ix)))
        .w(px(12.))
        .flex_none()
        .text_color(cx.k.hint)
        .child(if open { "▾" } else { "▸" })
        .when(!open, |d| d.invisible().group_hover("tl-row", |s| s.visible()))
        .on_click(cx.on_ws(move |ws, _, _, cx| {
            cx.stop_propagation();
            ws.toggle_timeline_row(&key, cx);
        }))
}

fn tool(row: &Row, t: &ToolRow, ix: usize, cx: &Ctx) -> AnyElement {
    let k = &cx.k;
    let s = summary(t);
    let status = s.status.clone();
    let mut highlights = Vec::new();
    let file_range = s.file.clone();
    if let Some(r) = s.file.clone() {
        let underline = UnderlineStyle { thickness: px(1.), color: Some(k.hint), wavy: false };
        highlights.push((r, HighlightStyle { underline: Some(underline), ..HighlightStyle::default() }));
    }
    if let Some(r) = s.added.clone() {
        highlights.push((r, HighlightStyle { color: Some(k.green), ..HighlightStyle::default() }));
    }
    if let Some(r) = s.removed.clone() {
        highlights.push((r, HighlightStyle { color: Some(k.red), ..HighlightStyle::default() }));
    }
    let styled = StyledText::new(SharedString::from(s.text)).with_highlights(highlights);
    // Where the file name was laid out (`gilvt debug state`), read after the text is prepainted.
    let file_rect = file_range.clone().and_then(|r| rects::text_range_recorder(RectId::TimelineFile(ix), styled.layout().clone(), r));
    let mut text = InteractiveText::new(("tl-text", ix), styled);
    // ⌘+click on the file name: Quick Look (the row's own click ignores ⌘).
    if let (Some(range), Some(raw)) = (file_range, t.file.clone()) {
        let ws = cx.ws.clone();
        let cwd = cx.cwd.clone();
        text = text.on_click(vec![range], move |_, window, cx| {
            if !window.modifiers().platform {
                return;
            }
            let home = std::env::var_os("HOME").map(PathBuf::from);
            if let Some(path) = resolve_path(&raw, cwd.as_deref(), home.as_deref()) {
                let _ = ws.update(cx, |ws, cx| ws.timeline_quick_look(path, window, cx));
            }
        });
    }
    let key = row.key.clone();
    let anchor = t.anchor;
    let needles = t.needles.clone();
    let open = t.detail.is_some();
    let head = div()
        .id(("tl-row", ix))
        .relative()
        .children(rects::recorder(RectId::TimelineRow(ix)))
        .group("tl-row")
        .flex()
        .items_baseline()
        .gap(px(6.))
        .px(px(4.))
        .py(px(3.))
        .rounded(px(5.))
        .cursor_pointer()
        .hover(|s| s.bg(k.row_hover))
        .child(chevron(&row.key, open, ix, cx))
        .child(div().w(px(16.)).flex_none().flex().justify_center().text_color(cx.ink(t.icon_ink)).child(t.icon))
        .child(div().flex_1().min_w(px(0.)).truncate().text_color(cx.ink(t.ink)).child(text).children(file_rect))
        .children(status.map(|x| div().flex_none().text_color(cx.ink(t.ink)).child(x)))
        .child(div().flex_none().text_size(px(11.)).text_color(k.dur).child(t.timing.label(cx.now)))
        .on_click(cx.on_ws(move |ws, e, window, cx| {
            if !e.modifiers().platform {
                ws.timeline_click(&key, anchor, &needles, window, cx);
            }
        }));
    let error = (!t.error.is_empty()).then(|| {
        div()
            .ml(px(38.))
            .mb(px(3.))
            .px(px(6.))
            .py(px(3.))
            .rounded(px(4.))
            .bg(k.err_bg)
            .text_color(k.err)
            .font(cx.mono.clone())
            .text_size(px(10.5))
            .line_height(relative(1.45))
            .children(t.error.iter().map(|l| div().child(l.clone())))
    });
    let detail = t.detail.as_ref().map(|lines| detail(lines, ix, cx));
    div().child(head).children(error).children(detail).into_any_element()
}

/// The expanded detail: arguments key by key, then the output; monospace, with a copy button (gpui has no
/// text selection for plain elements).
fn detail(lines: &[DetailLine], ix: usize, cx: &Ctx) -> AnyElement {
    let k = &cx.k;
    let text = detail_text(lines);
    let copy = div()
        .id(("tl-copy", ix))
        .children(rects::recorder(RectId::TimelineCopy(ix)))
        .absolute()
        .top(px(3.))
        .right(px(5.))
        .px(px(5.))
        .rounded(px(4.))
        .bg(k.chip)
        .text_color(k.chip_text)
        .font_family(UI_FONT)
        .text_size(px(10.))
        .cursor_pointer()
        .child(crate::i18n::text("复制", "Copy"))
        .on_click(cx.on_ws(move |ws, _, _, cx| {
            cx.stop_propagation();
            ws.copy_timeline_detail(text.clone(), cx);
        }));
    let body = lines.iter().map(|l| match l {
        DetailLine::Input { key, value } => div().child(format!("{key}: {value}")),
        DetailLine::More(s) => div().pl(px(12.)).child(s.clone()),
        DetailLine::Output(s) => div().text_color(k.meta).child(if s.is_empty() { " ".to_string() } else { s.clone() }),
    });
    let empty = lines.is_empty().then(|| {
        div().text_color(k.muted).child(crate::i18n::text(
            "（没有记录参数或输出）",
            "(No recorded arguments or output)",
        ))
    });
    div()
        .relative()
        .ml(px(38.))
        .mt(px(2.))
        .mb(px(6.))
        .px(px(7.))
        .py(px(5.))
        .pr(px(36.))
        .rounded(px(5.))
        .bg(k.detail_bg)
        .text_color(k.detail)
        .font(cx.mono.clone())
        .text_size(px(10.5))
        .line_height(relative(1.45))
        .children(body)
        .children(empty)
        .child(copy)
        .into_any_element()
}

/// 「✻ 思考 · 4s ▸」: collapsed by default; open shows its first 20 lines.
fn thinking(row: &Row, secs: Option<&str>, expandable: bool, text: Option<&[String]>, ix: usize, cx: &Ctx) -> AnyElement {
    let k = &cx.k;
    let label = match secs {
        Some(s) if crate::i18n::current() == crate::i18n::Language::English => {
            format!("Thinking · {s}")
        }
        Some(s) => format!("思考 · {s}"),
        None => crate::i18n::text("思考", "Thinking").to_string(),
    };
    let label = if expandable { format!("{label} {}", if text.is_some() { "▾" } else { "▸" }) } else { label };
    let key = row.key.clone();
    let head = div()
        .id(("tl-think", ix))
        .relative()
        .children(rects::recorder(RectId::TimelineRow(ix)))
        .group("tl-row")
        .flex()
        .gap(px(6.))
        .px(px(4.))
        .py(px(3.))
        .rounded(px(5.))
        .text_color(k.muted)
        .italic()
        .child(div().w(px(12.)).flex_none())
        .child(div().w(px(16.)).flex_none().flex().justify_center().child("✻"))
        .child(div().flex_1().min_w(px(0.)).truncate().child(label))
        .when(expandable, |d| {
            d.cursor_pointer().hover(|s| s.bg(k.row_hover)).on_click(cx.on_ws(move |ws, _, _, cx| ws.toggle_timeline_row(&key, cx)))
        });
    let body = text.map(|lines| {
        div()
            .ml(px(38.))
            .mb(px(6.))
            .text_size(px(11.))
            .text_color(k.meta)
            .italic()
            .children(lines.iter().map(|l| div().child(l.clone())))
    });
    div().child(head).children(body).into_any_element()
}
