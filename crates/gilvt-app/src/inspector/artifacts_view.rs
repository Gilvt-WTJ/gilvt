//! Draws the 「产物」 tab (M4a spec §4, tasks spec §5): the session net (or the plain summary line), then one
//! card per task — title, time, files, the test result and the agent's closing sentence — with runs of quiet
//! tasks folded into one row. `artifacts_model` decides what is shown; this only paints it and wires the
//! clicks to the `Workspace`.

use std::collections::HashSet;

use gpui::{
    canvas, div, point, prelude::*, px, relative, AnyElement, App, Bounds, ClickEvent, Context, Div, FontWeight, MouseButton, Pixels,
    ScrollHandle, SharedString, Window,
};

use super::artifacts_model::{self as m, ArtCard, Block, CardState, FileRow, Sel, SHOWN_FILES};
use super::colors::Colors;
use crate::debug_state::rects::{self, RectId};
use crate::workspace::Workspace;

/// The tab's body, and whether it needs the per-second redraw (a running turn's live file list).
pub(super) fn render(ws: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) -> (AnyElement, bool) {
    let theme = crate::theme::current(cx);
    let k = Colors::new(&theme.ui, theme.dark);
    let Some(artifacts) = ws.focused_artifacts(cx) else {
        return (
            empty(
                ws,
                &k,
                crate::i18n::text(
                    "运行 claude 或 codex 后，这里显示它每一轮改了哪些文件",
                    "Run claude or codex to see the files changed in each turn",
                ),
                cx,
            ),
            false,
        );
    };
    let running = artifacts.cards.iter().any(|c| c.state == CardState::Running);
    if artifacts.cards.is_empty() {
        return (
            empty(ws, &k, crate::i18n::text("这个会话还没有任何一轮", "This session has no turns yet"), cx),
            false,
        );
    }

    let reveal = std::mem::take(&mut ws.inspector_mut().artifacts.reveal);
    let ui = &ws.inspector().artifacts;
    let reveal = reveal.then(|| ui.scroll.clone());
    let newest = m::newest_open_by_default(&artifacts);
    let mut list = div().flex().flex_col().gap(px(8.));
    match (&artifacts.net, &artifacts.summary) {
        (Some(net), s) => list = list.child(net_block(net, s.as_ref(), ui.net_open, ui.selected, reveal.as_ref(), &k, cx)),
        (None, Some(s)) => list = list.child(summary(s, &k)),
        (None, None) => {}
    }
    let mut file_ix = 0usize;
    // `card_ix` is the card's index in `artifacts.cards` (folded quiet cards may not be drawn).
    let card = |list: Div, card_ix: usize, file_ix: &mut usize, cx: &mut Context<Workspace>| {
        let c = &artifacts.cards[card_ix];
        let open = ui.is_open(c.key, Some(c.key) == newest);
        list.child(render_card(c, card_ix, open, ui.all_files.contains(&c.key), ui.selected, reveal.as_ref(), file_ix, &k, cx))
    };
    for b in &artifacts.blocks {
        match *b {
            Block::Card(i) => list = card(list, i, &mut file_ix, cx),
            Block::Quiet(gix) => {
                let g = &artifacts.quiet_groups[gix];
                let open = ui.open_quiet.contains(&g.key);
                list = list.child(quiet_row(g, gix, open, ui.selected, reveal.as_ref(), &k, cx));
                if open {
                    for key in &g.cards {
                        if let Some(i) = artifacts.cards.iter().position(|c| c.key == *key) {
                            list = card(list, i, &mut file_ix, cx);
                        }
                    }
                }
            }
        }
    }
    let body = div()
        .id("artifacts-scroll")
        .track_scroll(&ui.scroll)
        .track_focus(&ui.focus)
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .px(px(10.))
        .pt(px(8.))
        .pb(px(14.))
        .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, _| window.focus(&ws.inspector().artifacts.focus)))
        .on_key_down(cx.listener(|ws, e, window, cx| ws.artifacts_key(e, window, cx)))
        .child(list);
    (body.into_any_element(), running)
}

/// An invisible child of the row `↑↓` just selected (its parent must be `relative()`): once the row is laid
/// out it scrolls the list the least that shows it, and the next frame draws that.
fn revealer(scroll: &ScrollHandle) -> impl IntoElement {
    let scroll = scroll.clone();
    canvas(
        move |row, window, _| {
            let (view, offset) = (scroll.bounds(), scroll.offset());
            let f = |p: Pixels| p / px(1.);
            let y = super::reveal_offset((f(view.top()), f(view.bottom())), (f(row.top()), f(row.bottom())), f(offset.y));
            if y != f(offset.y) {
                scroll.set_offset(point(offset.x, px(y)));
                window.on_next_frame(|window, _| window.refresh());
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The empty state, inside the same focusable container as the list so `Esc` still hands the keyboard back
/// when the list holds focus and the session goes away.
fn empty(ws: &Workspace, k: &Colors, text: &'static str, cx: &mut Context<Workspace>) -> AnyElement {
    div()
        .id("artifacts-scroll")
        .track_focus(&ws.inspector().artifacts.focus)
        .flex_1()
        .min_h(px(0.))
        .px(px(8.))
        .py(px(40.))
        .flex()
        .justify_center()
        .text_color(k.empty)
        .line_height(relative(1.8))
        .on_mouse_down(MouseButton::Left, cx.listener(|ws, _, window, _| window.focus(&ws.inspector().artifacts.focus)))
        .on_key_down(cx.listener(|ws, e, window, cx| ws.artifacts_key(e, window, cx)))
        .child(text)
        .into_any_element()
}

/// No session net (no snapshot pair): the plain grey line, without line counts and not clickable.
fn summary(s: &m::Summary, k: &Colors) -> impl IntoElement {
    div().text_color(k.muted).child(
        if crate::i18n::current() == crate::i18n::Language::English {
            format!("This session · {} files", s.files)
        } else {
            format!("本会话 · {} 文件", s.files)
        },
    )
}

/// `+a` green and `−b` red, side by side.
fn counts(added: u32, removed: u32, k: &Colors) -> [AnyElement; 2] {
    [div().text_color(k.green).child(format!("+{added}")).into_any_element(), div().text_color(k.red).child(format!("−{removed}")).into_any_element()]
}

/// The 「本会话净改动」 bar; open, also the range, every file grouped by directory and the notes under it.
fn net_block(
    net: &m::Net,
    summary: Option<&m::Summary>,
    open: bool,
    selected: Option<Sel>,
    reveal: Option<&ScrollHandle>,
    k: &Colors,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let files = summary.map_or(net.files.len(), |s| s.files);
    let head_text = if crate::i18n::current() == crate::i18n::Language::English {
        format!(
            "{} Net changes in this session · {files} files",
            if open { "⌄" } else { "▸" }
        )
    } else {
        format!(
            "{} 本会话净改动 · {files} 文件",
            if open { "⌄" } else { "▸" }
        )
    };
    let head = div().flex().gap(px(6.)).child(head_text).children(
        match summary.and_then(|s| s.added.zip(s.removed)) {
            Some((a, r)) => counts(a, r, k).into_iter().collect::<Vec<_>>(),
            None if net.computing => vec![div()
                .text_color(k.muted)
                .child(crate::i18n::text("· 计算中", "· Calculating"))
                .into_any_element()],
            None => vec![],
        },
    );
    let mut block = div()
        .id("artifact-net")
        .children(rects::recorder(RectId::ArtifactNet))
        .children(reveal.filter(|_| selected == Some(Sel::Net)).map(revealer))
        .relative()
        .px(px(10.))
        .py(px(6.))
        .rounded(px(8.))
        .bg(if selected == Some(Sel::Net) { k.selected } else { k.card })
        .border_1()
        .border_color(k.card_border)
        .cursor_pointer()
        .on_click(cx.listener(|ws, _: &ClickEvent, _, cx| {
            ws.select_artifact(Sel::Net, cx);
            ws.toggle_artifact_net(cx);
        }))
        .child(head);
    if !open {
        return block.into_any_element();
    }
    let note = |text: String, color| div().text_size(px(11.)).text_color(color).child(text);
    block = block.child(note(net.range_label.clone(), k.muted));
    if let Some(why) = &net.failed {
        block = block.child(note(
            if crate::i18n::current() == crate::i18n::Language::English {
                format!("Failed to calculate net changes: {why}")
            } else {
                format!("净改动计算失败：{why}")
            },
            k.red,
        ));
    } else {
        block = block.children(file_rows(&net.files, net.files.len(), true, false, Owner::Net, selected, reveal, k, cx));
    }
    if net.excluded > 0 {
        block = block.child(note(
            if crate::i18n::current() == crate::i18n::Language::English {
                format!(
                    "{} additional files changed between turns and are not included",
                    net.excluded
                )
            } else {
                format!("另有 {} 个文件在轮次之间被改动，未计入", net.excluded)
            },
            k.muted,
        ));
    }
    if let Some(name) = &net.only_repo {
        block = block.child(note(
            if crate::i18n::current() == crate::i18n::Language::English {
                format!("Only counting {name}")
            } else {
                format!("只统计 {name}")
            },
            k.muted,
        ));
    }
    block.into_any_element()
}

/// A run of quiet tasks folded into one line; `gix` is its index in `quiet_groups`.
fn quiet_row(
    g: &m::QuietGroup,
    gix: usize,
    open: bool,
    selected: Option<Sel>,
    reveal: Option<&ScrollHandle>,
    k: &Colors,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let key = g.key;
    let is_selected = selected == Some(Sel::Quiet(key));
    div()
        .id(("artifact-quiet", gix))
        .children(rects::recorder(RectId::ArtifactQuiet(gix)))
        .children(reveal.filter(|_| is_selected).map(revealer))
        .relative()
        .px(px(4.))
        .py(px(2.))
        .rounded(px(4.))
        .text_size(px(11.))
        .text_color(k.muted)
        .cursor_pointer()
        .when(is_selected, |d| d.bg(k.selected))
        .on_click(cx.listener(move |ws, _: &ClickEvent, _, cx| {
            ws.select_artifact(Sel::Quiet(key), cx);
            ws.toggle_artifact_quiet(key, cx);
        }))
        .child(truncated(
            if crate::i18n::current() == crate::i18n::Language::English {
                format!(
                    "{} {} turns without file changes · {}",
                    if open { "⌄" } else { "▸" },
                    g.cards.len(),
                    g.titles.join(", ")
                )
            } else {
                format!(
                    "{} {} 轮无文件改动 · {}",
                    if open { "⌄" } else { "▸" },
                    g.cards.len(),
                    g.titles.join("、")
                )
            },
        ))
        .into_any_element()
}

/// Whose file rows these are: the session net's, or card `cards[ix]`'s (key `key`), whose rows' element ids
/// run from `first` (card file rows are numbered across the whole list).
#[derive(Clone, Copy)]
enum Owner {
    Net,
    Card { key: u32, ix: usize, first: usize },
}

impl Owner {
    fn sel(self, i: usize) -> Sel {
        match self {
            Owner::Net => Sel::NetFile(i),
            Owner::Card { key, .. } => Sel::File(key, i),
        }
    }

    fn rect(self, i: usize) -> RectId {
        match self {
            Owner::Net => RectId::ArtifactNetFile(i),
            Owner::Card { ix, .. } => RectId::ArtifactFile(ix, i),
        }
    }
}

fn dir_of(path: &str) -> String {
    std::path::Path::new(path).parent().map(|p| p.display().to_string()).filter(|d| !d.is_empty()).unwrap_or_else(|| ".".into())
}

/// The header to draw before a file at `path` when the previous file's directory was `last`: the directory
/// when it changed; nothing in the same directory. Root files get no header while nothing else has been drawn
/// above them (no lone 「.」 line), but after another directory's files they get 「根目录」 so they don't read
/// as belonging to that directory.
fn dir_header(last: Option<&str>, path: &str) -> Option<String> {
    let dir = dir_of(path);
    if last == Some(dir.as_str()) {
        return None;
    }
    if dir == "." {
        return last.map(|_| crate::i18n::text("根目录", "Root").to_string());
    }
    Some(dir)
}

/// The first `shown` of `files`; with `group`, a dim header line whenever the directory changes (the files
/// are sorted by path, so a directory's files are together). Click selects, `⌘`-click previews, right click
/// copies 「路径:行号」; `no_counts` leaves out the +/− numbers.
#[allow(clippy::too_many_arguments)]
fn file_rows(
    files: &[FileRow],
    shown: usize,
    group: bool,
    no_counts: bool,
    owner: Owner,
    selected: Option<Sel>,
    reveal: Option<&ScrollHandle>,
    k: &Colors,
    cx: &mut Context<Workspace>,
) -> Vec<AnyElement> {
    let mut rows = Vec::new();
    let mut last_dir: Option<String> = None;
    for (i, f) in files.iter().enumerate().take(shown) {
        if group {
            if let Some(dir) = dir_header(last_dir.as_deref(), &f.path) {
                rows.push(div().mt(px(4.)).text_size(px(10.5)).text_color(k.muted).child(dir).into_any_element());
            }
            last_dir = Some(dir_of(&f.path));
        }
        let sel = owner.sel(i);
        let id = match owner {
            Owner::Net => ("artifact-net-file", i),
            Owner::Card { first, .. } => ("artifact-file", first + i),
        };
        let status_color = match f.status {
            'A' => k.green,
            'D' => k.red,
            _ => k.yellow,
        };
        rows.push(
            div()
                .id(id)
                .children(rects::recorder(owner.rect(i)))
                .children(reveal.filter(|_| selected == Some(sel)).map(revealer))
                .relative()
                .flex()
                .gap(px(6.))
                .px(px(4.))
                .py(px(2.))
                .rounded(px(4.))
                .when(selected == Some(sel), |d| d.bg(k.selected))
                .hover(|s| s.bg(k.row_hover))
                .text_size(px(11.5))
                .on_click(cx.listener(move |ws, e: &ClickEvent, window, cx| {
                    // The card's / the net's own click (bubbling) must not toggle it.
                    cx.stop_propagation();
                    ws.select_artifact(sel, cx);
                    if e.modifiers().platform {
                        match owner {
                            Owner::Net => ws.open_net_file(i, window, cx),
                            Owner::Card { key, .. } => ws.open_artifact_file(key, i, window, cx),
                        }
                    }
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |ws, _, _, cx| match owner {
                        Owner::Net => ws.copy_net_path(i, cx),
                        Owner::Card { key, .. } => ws.copy_artifact_path(key, i, cx),
                    }),
                )
                .child(div().flex_none().w(px(12.)).text_color(status_color).child(f.status.to_string()))
                .child(truncated(match &f.old_path {
                    Some(old) => format!("{old} → {}", f.path),
                    None => f.path.clone(),
                }).flex_1())
                .child(div().flex_none().text_color(k.muted).child(if f.binary {
                    crate::i18n::text("二进制", "Binary").to_string()
                } else if no_counts {
                    String::new()
                } else {
                    format!("+{} −{}", f.added, f.removed)
                }))
                .into_any_element(),
        );
    }
    rows
}

/// The small line under a card's header: 「第 5–7 轮 · 含 2 次跟进：继续 → 好的」 ("" when there is neither a
/// turn number nor a follow-up).
pub(crate) fn sub_line(c: &ArtCard) -> String {
    let turns = match (c.turns.first(), c.turns.last()) {
        (Some(a), Some(b))
            if a != b && crate::i18n::current() == crate::i18n::Language::English =>
        {
            format!("Turns {a}–{b}")
        }
        (Some(a), Some(b)) if a != b => format!("第 {a}–{b} 轮"),
        (Some(a), _) if crate::i18n::current() == crate::i18n::Language::English => {
            format!("Turn {a}")
        }
        (Some(a), _) => format!("第 {a} 轮"),
        _ => String::new(),
    };
    let follow = match c.follow_ups.len() {
        0 => String::new(),
        n if n <= 3 && crate::i18n::current() == crate::i18n::Language::English => {
            format!("{n} follow-ups: {}", c.follow_ups.join(" → "))
        }
        n if n <= 3 => format!("含 {n} 次跟进：{}", c.follow_ups.join(" → ")),
        n if crate::i18n::current() == crate::i18n::Language::English => {
            format!("{n} follow-ups: {} → …", c.follow_ups[..2].join(" → "))
        }
        n => format!("含 {n} 次跟进：{} → …", c.follow_ups[..2].join(" → ")),
    };
    [turns, follow].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// The non-empty parts joined with 「 · 」 (「14:22 · 标题」, 「标题」, 「14:22」).
fn join_some<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// The header's right side: 「3 文件 +96 −12 · 8m42s」, 「3 文件 · 计算中 · 8m42s」, 「3 文件 +96 −12 · 进行中」
/// (no +/− when the counts are hidden); a quiet card 「无文件改动 · 8m42s」; a degraded card only its duration.
fn header_meta(c: &ArtCard, k: &Colors) -> Div {
    let meta = div().flex_none().flex().gap(px(4.)).text_color(k.muted);
    match c.state {
        CardState::Quiet => {
            return meta.child(join_some([
                crate::i18n::text("无文件改动", "No file changes"),
                c.took.as_deref().unwrap_or(""),
            ]))
        }
        CardState::Degraded => return meta.child(c.took.clone().unwrap_or_default()),
        CardState::Done | CardState::Running => {}
    }
    let tail = if c.state == CardState::Running {
        Some(crate::i18n::text("进行中", "In progress").to_string())
    } else {
        c.took.clone()
    };
    let mut meta = meta.child(
        if crate::i18n::current() == crate::i18n::Language::English {
            format!("{} files", c.files.len())
        } else {
            format!("{} 文件", c.files.len())
        },
    );
    if c.computing {
        meta = meta.child(crate::i18n::text("· 计算中", "· Calculating"));
    } else if !c.counts_hidden {
        meta = meta.children(counts(c.files.iter().map(|f| f.added).sum(), c.files.iter().map(|f| f.removed).sum(), k));
    }
    meta.children(tail.map(|t| format!("· {t}")))
}

/// One line of `text` cut with a trailing 「…」 at whatever width its slot ends up with; the caller sizes the
/// slot (`flex_1()` fills the row, `flex_shrink()` hugs the text and shrinks; a block child takes the full width).
///
/// A plain `.truncate()` text gets clipped mid-letter here instead: gpui keeps a non-wrapping text's first
/// measurement for the whole layout pass, whatever width it is measured at later (gpui-0.2.2
/// `elements/text.rs:373-378`), and taffy first measures it while the row's width is still unknown — a flex
/// item with a `0%` basis in a row of unknown width is sized as max-content (taffy-0.9.0
/// `compute/flexbox.rs:697-764`), and a block sized as content lays its children out at that width
/// (`compute/block.rs:207-232`, absolute children included, `:244-249`). So the text is never re-cut at its
/// final width. Here the visible copy is shaped and cut while painting, when the slot's bounds are final; the
/// invisible copy under it gives the slot its line height and its content width, and is never cut.
fn truncated(text: impl Into<SharedString>) -> Div {
    let text = text.into();
    let shown = text.clone();
    div()
        .relative()
        .min_w(px(0.))
        .overflow_hidden()
        .whitespace_nowrap()
        .child(div().invisible().child(text))
        .child(canvas(|_, _, _| {}, move |bounds, (), window, cx| paint_truncated(shown, bounds, window, cx)).absolute().top_0().left_0().size_full())
}

/// Paints `text` in the inherited text style at `bounds`' top left, cut with 「…」 when it is wider than `bounds`.
fn paint_truncated(text: SharedString, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let style = window.text_style();
    let font_size = style.font_size.to_pixels(window.rem_size());
    let line_height = style.line_height.to_pixels(font_size.into(), window.rem_size());
    let mut runs = vec![style.to_run(text.len())];
    let mut line = window.text_system().shape_line(text.clone(), font_size, &runs, None);
    // Compare the shaped width first: `truncate_line` sums per-character advances (no kerning), which can
    // exceed a text that just fits and cut it for nothing.
    if line.width > bounds.size.width {
        let cut = cx.text_system().line_wrapper(style.font(), font_size).truncate_line(text, bounds.size.width, "…", &mut runs);
        line = window.text_system().shape_line(cut, font_size, &runs, None);
    }
    // Painting only fails when a glyph can't be rasterized; the frame goes on without this line.
    let _ = line.paint(bounds.origin, line_height, window, cx);
}

/// One task's card; `card_ix` is its index in `artifacts.cards` (the rects' `cards[n]`).
#[allow(clippy::too_many_arguments)]
fn render_card(
    c: &ArtCard,
    card_ix: usize,
    open: bool,
    all: bool,
    selected: Option<Sel>,
    reveal: Option<&ScrollHandle>,
    file_ix: &mut usize,
    k: &Colors,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let key = c.key;
    let is_selected = selected == Some(Sel::Card(key));
    let quiet = c.state == CardState::Quiet;
    let arrow = if open && !quiet { "⌄" } else { "▸" };
    let title = join_some([c.started.as_str(), c.title.as_str()]);
    let head = div()
        .relative()
        .children(reveal.filter(|_| is_selected).map(revealer))
        .flex()
        .justify_between()
        .gap(px(8.))
        .child(truncated(format!("{arrow} {title}")).flex_1().font_weight(FontWeight::SEMIBOLD))
        .child(header_meta(c, k));
    let mut card = div()
        .id(("artifact-card", card_ix))
        .children(rects::recorder(RectId::ArtifactCard(card_ix)))
        .relative()
        .px(px(10.))
        .py(px(8.))
        .rounded(px(8.))
        .bg(if is_selected { k.selected } else { k.card })
        .border_1()
        .when(c.state == CardState::Running, |d| d.border_dashed())
        .border_color(if c.state == CardState::Running { k.blue } else { k.card_border })
        .cursor_pointer()
        .on_click(cx.listener(move |ws, _: &ClickEvent, _, cx| {
            ws.select_artifact(Sel::Card(key), cx);
            // A quiet card has nothing to unfold.
            if !quiet {
                ws.toggle_artifact_card(key, cx);
            }
        }))
        .child(head);
    // A quiet task (drawn inside its unfolded group) is its header alone.
    if quiet {
        return card.into_any_element();
    }
    let test = c.test.as_ref().map(|t| if t.ok { ("✓", k.green, t) } else { ("✗", k.red, t) });
    let sub = sub_line(c);
    // Closed, the test result rides on the small line.
    let closed_test = test.filter(|_| !open).map(|(mark, color, t)| {
        (
            color,
            format!(
                "{mark} {} {}",
                t.command,
                if t.ok {
                    crate::i18n::text("通过", "passed")
                } else {
                    crate::i18n::text("失败", "failed")
                }
            ),
        )
    });
    if !sub.is_empty() || closed_test.is_some() {
        let sep = if sub.is_empty() { "" } else { " · " };
        // The turns / follow-ups (prompt heads, possibly long) shrink and truncate; the test result stays whole
        // right after them (shrinking without growing keeps it beside the text, not at the far edge).
        card = card.child(
            div()
                .flex()
                .min_w(px(0.))
                .text_size(px(11.))
                .text_color(k.muted)
                .when(!sub.is_empty(), |d| d.child(truncated(sub).flex_shrink()))
                .children(closed_test.map(|(color, text)| {
                    div().flex_none().flex().child(sep).child(div().text_color(color).child(text))
                })),
        );
    }
    if c.touched_later {
        let text = c.touched_later_turn.map_or(
            crate::i18n::text("后被改动", "Changed later").to_string(),
            |n| {
                if crate::i18n::current() == crate::i18n::Language::English {
                    format!("Changed later · Turn {n}")
                } else {
                    format!("后被改动 · 第 {n} 轮")
                }
            },
        );
        card = card.child(div().text_size(px(11.)).text_color(k.yellow).child(text));
    }
    if open {
        let shown = if all { c.files.len() } else { c.files.len().min(SHOWN_FILES) };
        // Grouped when every file is listed, or when the files are in more than one directory (spec §5.2).
        let group = all || c.files.iter().map(|f| dir_of(&f.path)).collect::<HashSet<_>>().len() > 1;
        let owner = Owner::Card { key, ix: card_ix, first: *file_ix };
        *file_ix += shown;
        card = card.children(file_rows(&c.files, shown, group, c.counts_hidden, owner, selected, reveal, k, cx));
        if !all && c.files.len() > SHOWN_FILES {
            card = card.child(
                div()
                    .id(("artifact-more", card_ix))
                    .children(rects::recorder(RectId::ArtifactMore(card_ix)))
                    .relative()
                    .px(px(4.))
                    .py(px(2.))
                    .text_size(px(11.))
                    .text_color(k.blue)
                    .cursor_pointer()
                    .on_click(cx.listener(move |ws, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        ws.show_all_artifact_files(key, cx)
                    }))
                    .child(
                        if crate::i18n::current() == crate::i18n::Language::English {
                            format!(
                                "{} more files · View all grouped by folder",
                                c.files.len() - SHOWN_FILES
                            )
                        } else {
                            format!(
                                "另有 {} 个文件 · 按目录分组查看全部",
                                c.files.len() - SHOWN_FILES
                            )
                        },
                    ),
            );
        }
        if let Some((mark, color, t)) = test {
            card = card.child(
                div()
                    .text_size(px(11.))
                    .text_color(color)
                    .child(match t.exit {
                        Some(code)
                            if !t.ok
                                && crate::i18n::current() == crate::i18n::Language::English =>
                        {
                            format!("{mark} {} failed (exit code {code})", t.command)
                        }
                        Some(code) if !t.ok => {
                            format!("{mark} {} 失败（退出码 {code}）", t.command)
                        }
                        _ if t.ok => format!(
                            "{mark} {} {}",
                            t.command,
                            crate::i18n::text("通过", "passed")
                        ),
                        _ => format!(
                            "{mark} {} {}",
                            t.command,
                            crate::i18n::text("失败", "failed")
                        ),
                    }),
            );
        }
    }
    for n in &c.notices {
        card = card.child(div().text_size(px(11.)).text_color(k.muted).child(n.text()));
    }
    if !c.quote.is_empty() {
        let quote = if crate::i18n::english() { format!("\u{201c}{}\u{201d}", c.quote) } else { format!("「{}」", c.quote) };
        card = card.child(div().mt(px(3.)).text_size(px(11.)).text_color(k.meta).child(quote));
    }
    card.into_any_element()
}

#[cfg(test)]
mod tests {
    #[test]
    fn sub_lines() {
        let mut c = crate::inspector::artifacts_model::tests_support::card(5);
        c.turns = vec![5, 6, 7];
        c.follow_ups = vec!["继续".into(), "好的".into()];
        assert_eq!(super::sub_line(&c), "第 5–7 轮 · 含 2 次跟进：继续 → 好的");
        c.follow_ups = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        assert_eq!(super::sub_line(&c), "第 5–7 轮 · 含 4 次跟进：a → b → …");
        c.follow_ups = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(super::sub_line(&c), "第 5–7 轮 · 含 3 次跟进：a → b → c", "three follow-ups are all listed");
        c.turns = vec![5];
        c.follow_ups = vec![];
        assert_eq!(super::sub_line(&c), "第 5 轮");
        c.turns = vec![];
        assert_eq!(super::sub_line(&c), "");
    }

    #[test]
    fn dir_headers() {
        assert_eq!(super::dir_header(None, "README.md"), None, "root files get no header");
        assert_eq!(super::dir_header(Some("internal"), "z.txt"), Some("根目录".to_string()), "a root file after a directory");
        assert_eq!(super::dir_header(Some("."), "z.txt"), None, "root after root");
        assert_eq!(super::dir_header(None, "internal/a.go"), Some("internal".to_string()));
        assert_eq!(super::dir_header(Some("internal"), "internal/b.go"), None, "same directory");
        assert_eq!(super::dir_header(Some("."), "internal/a.go"), Some("internal".to_string()));
    }

    #[test]
    fn header_parts_join_only_when_present() {
        assert_eq!(super::join_some(["14:22", "给 /users 加分页"]), "14:22 · 给 /users 加分页");
        assert_eq!(super::join_some(["14:22", ""]), "14:22", "no dangling separator for an empty title");
        assert_eq!(super::join_some(["", "标题"]), "标题");
        assert_eq!(super::join_some(["无文件改动", ""]), "无文件改动");
    }
}
