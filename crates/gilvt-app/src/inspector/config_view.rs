//! The M5a read-only configuration summary. Reading happens off the UI thread in `workspace::inspector`;
//! this module only renders the latest immutable snapshot and never exposes secret-bearing values.

use std::path::{Path, PathBuf};

use gilvt_config::{NamedItem, SourceLayer, Summary, Value};
use gpui::{div, prelude::*, px, relative, AnyElement, ClickEvent, Context, FontWeight, KeyDownEvent, Window};

use super::colors::Colors;
use super::config_model::{Disclosure, Group, Section};
use crate::debug_state::rects::{self, RectId};
use crate::workspace::Workspace;

/// The file a row's 「编辑」 opens, None for no 「编辑」: only a Markdown file. `gilvt-config` already gives
/// JSON / TOML-sourced items (MCP, hooks) no path and lists only Markdown resources; this keeps it so.
pub(crate) fn edit_path(path: Option<&Path>) -> Option<&Path> {
    path.filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")))
}

pub fn render(ws: &Workspace, k: Colors, cx: &mut Context<Workspace>) -> AnyElement {
    let state = &ws.inspector().config;
    let loading = state.loading;
    let summary = state.summary.clone();
    div()
        .id("config-scroll")
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .px(px(10.))
        .pt(px(8.))
        .pb(px(14.))
        .when(state.target.is_none(), |d| d.child(empty(&k)))
        .when(state.target.is_some(), |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb(px(8.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(k.meta)
                            .child(if loading {
                                crate::i18n::text("正在读取配置…", "Loading configuration…")
                            } else {
                                crate::i18n::text(
                                    "只读 · 不显示密钥和环境变量值",
                                    "Read-only · secrets and environment values are hidden",
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("config-refresh")
                            .relative()
                            .children(rects::recorder(RectId::InspectorConfigRefresh))
                            .cursor_pointer()
                            .text_color(k.blue)
                            .child(crate::i18n::text("刷新", "Refresh"))
                            .on_click(cx.listener(|ws, _, _, cx| ws.refresh_config_summary(cx))),
                    ),
            )
            .children(summary.map(|summary| cards(ws, &summary, &k, cx)))
        })
        .into_any_element()
}

fn empty(k: &Colors) -> AnyElement {
    div()
        .px(px(8.))
        .py(px(40.))
        .flex()
        .flex_col()
        .items_center()
        .line_height(relative(1.8))
        .text_color(k.empty)
        .child(crate::i18n::text(
            "当前 pane 没有 Agent 会话",
            "The current pane has no Agent session",
        ))
        .child(crate::i18n::text(
            "运行 claude 或 codex 后查看生效配置",
            "Run claude or codex to inspect its effective configuration",
        ))
        .into_any_element()
}

fn cards(ws: &Workspace, summary: &Summary, k: &Colors, cx: &mut Context<Workspace>) -> AnyElement {
    let title = match summary.agent {
        gilvt_agent::AgentKind::Claude => "Claude Code 配置",
        gilvt_agent::AgentKind::Codex => "Codex 配置",
    };
    let open = &ws.inspector().config.disclosure;
    // Drawing-order index of the expandable rows, for the debug rects.
    let mut row = 0;
    div()
        .child(
            div()
                .mb(px(8.))
                .child(div().font_weight(FontWeight::BOLD).child(title))
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(k.meta)
                        .child(summary.cwd.display().to_string()),
                ),
        )
        .child(values_card(
            crate::i18n::text("模型 / 模式", "Model / Mode"),
            [
                (crate::i18n::text("模型", "Model"), summary.model.as_ref()),
                (
                    crate::i18n::text("权限模式", "Permission mode"),
                    summary.permission.as_ref(),
                ),
            ],
            k,
        ))
        .child(items_card(
            Section::Mcp,
            &format!("MCP · {}", summary.mcp.len()),
            &summary.mcp,
            crate::i18n::text("没有发现 MCP server", "No MCP servers found"),
            (open, &mut row),
            k,
            cx,
        ))
        .child(resource_card(summary, k, cx))
        .child(items_card(
            Section::Hooks,
            &format!("Hooks · {}", summary.hooks.len()),
            &summary.hooks,
            crate::i18n::text(
                "没有发现 hooks / notify",
                "No hooks / notify configuration found",
            ),
            (open, &mut row),
            k,
            cx,
        ))
        .child(items_card(
            Section::Memory,
            crate::i18n::text("记忆 / 指令", "Memory / Instructions"),
            &summary.memory,
            crate::i18n::text(
                "没有发现 CLAUDE.md / AGENTS.md",
                "No CLAUDE.md / AGENTS.md found",
            ),
            (open, &mut row),
            k,
            cx,
        ))
        .child(source_card(summary, k))
        .into_any_element()
}

fn card(title: impl Into<String>, k: &Colors) -> gpui::Div {
    div()
        .mb(px(8.))
        .px(px(10.))
        .py(px(8.))
        .rounded(px(8.))
        .bg(k.card)
        .border_1()
        .border_color(k.card_border)
        .child(
            div()
                .mb(px(5.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(k.head)
                .child(title.into()),
        )
}

fn values_card<'a>(
    title: &str,
    values: impl IntoIterator<Item = (&'a str, Option<&'a Value>)>,
    k: &Colors,
) -> AnyElement {
    let mut body = card(title, k);
    for (label, value) in values {
        body = body.child(value_row(label, value, k));
    }
    body.into_any_element()
}

fn value_row(label: &str, value: Option<&Value>, k: &Colors) -> AnyElement {
    let (text, source) = value.map_or(
        (
            crate::i18n::text("未检测到", "Not detected").to_string(),
            None,
        ),
        |v| (v.text.clone(), Some(v.source)),
    );
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .py(px(2.))
        .child(div().text_color(k.meta).child(label.to_string()))
        .child(
            div()
                .min_w(px(0.))
                .flex()
                .items_center()
                .gap(px(5.))
                .child(div().truncate().child(text))
                .children(source.map(|s| source_tag(s, k))),
        )
        .into_any_element()
}

/// A card whose rows expand in place: ▸ name … tag, then the redacted detail lines and, for Markdown
/// files, 「打开」. Every row is listed (no cap), so nothing is hidden behind 「另有 N 项」.
fn items_card(
    section: Section,
    title: &str,
    items: &[NamedItem],
    empty: &str,
    (open, row): (&Disclosure, &mut usize),
    k: &Colors,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let mut body = card(title, k);
    if items.is_empty() {
        body = body.child(
            div()
                .text_size(px(11.))
                .text_color(k.muted)
                .child(empty.to_string()),
        );
    }
    for item in items {
        let ix = *row;
        *row += 1;
        let expandable = !item.lines.is_empty() || item.path.is_some();
        let expanded = expandable && open.is_open(section, &item.name);
        let name = item.name.clone();
        let mut line = div()
            .id(("config-row", ix))
            .relative()
            .children(rects::recorder(RectId::ConfigRow(ix)))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.))
            .py(px(2.))
            .child(
                div()
                    .min_w(px(0.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(div().flex_none().w(px(10.)).text_color(k.meta).child(if !expandable { "" } else if expanded { "▾" } else { "▸" }))
                    .child(div().min_w(px(0.)).truncate().child(item.name.clone())),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .children(
                        item.detail
                            .clone()
                            .map(|d| div().text_size(px(11.)).text_color(k.meta).child(d)),
                    )
                    .child(source_tag(item.source, k)),
            );
        if expandable {
            line = line
                .cursor_pointer()
                .on_click(cx.listener(move |ws, _, _, cx| ws.toggle_config_row(section, &name, cx)));
        }
        body = body.child(line);
        if expanded {
            let mut detail = div()
                .ml(px(14.))
                .mb(px(3.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(6.))
                .bg(k.detail_bg)
                .text_size(px(11.))
                .text_color(k.detail);
            for text in &item.lines {
                detail = detail.child(div().py(px(1.)).child(text.clone()));
            }
            if let Some(path) = item.path.clone() {
                let edit = edit_path(Some(&path)).map(|p| edit_link(ix, p.to_path_buf(), k, cx));
                detail = detail.child(div().flex().gap(px(10.)).child(open_link(ix, path, k, cx)).children(edit));
            }
            body = body.child(detail);
        }
    }
    body.into_any_element()
}

fn open_link(ix: usize, path: PathBuf, k: &Colors, cx: &mut Context<Workspace>) -> AnyElement {
    div()
        .id(("config-open", ix))
        .relative()
        .children(rects::recorder(RectId::ConfigOpen(ix)))
        .pt(px(3.))
        .cursor_pointer()
        .text_color(k.blue)
        .child(crate::i18n::text("打开", "Open"))
        .on_click(cx.listener(move |ws, _, window, cx| {
            ws.open_config_files(vec![path.clone()], 0, window, cx)
        }))
        .into_any_element()
}

/// 「编辑」 beside 「打开」: the file in the built-in editor (⌥ flips split / new tab).
fn edit_link(ix: usize, path: PathBuf, k: &Colors, cx: &mut Context<Workspace>) -> AnyElement {
    div()
        .id(("config-edit", ix))
        .pt(px(3.))
        .cursor_pointer()
        .text_color(k.blue)
        .child(crate::i18n::text("编辑", "Edit"))
        .on_click(cx.listener(move |ws, ev: &ClickEvent, window, cx| {
            ws.open_config_edit(path.clone(), ev.modifiers().alt, window, cx)
        }))
        .into_any_element()
}

/// 「扩展」: one line per kind, with its count; a click opens the dialog that lists them.
fn resource_card(summary: &Summary, k: &Colors, cx: &mut Context<Workspace>) -> AnyElement {
    let mut body = card(crate::i18n::text("扩展", "Extensions"), k);
    for (ix, group) in Group::ALL.into_iter().enumerate() {
        let count = group.items(summary).len();
        body =
            body.child(
                div()
                    .id(("config-group", ix))
                    .relative()
                    .children(rects::recorder(RectId::ConfigGroup(ix)))
                    .flex()
                    .justify_between()
                    .py(px(2.))
                    .when(count > 0, |d| {
                        d.cursor_pointer()
                            .on_click(cx.listener(move |ws, _, window, cx| {
                                ws.open_config_dialog(group, window, cx)
                            }))
                    })
                    .child(div().text_color(k.meta).child(group.title(summary.agent)))
                    .child(div().flex().gap(px(4.)).child(count.to_string()).when(
                        count > 0,
                        |d| {
                            d.child(
                                div()
                                    .text_color(k.blue)
                                    .child(crate::i18n::text("查看 ›", "View ›")),
                            )
                        },
                    )),
            );
    }
    body.into_any_element()
}

/// The dialog listing every Skill / command / sub-agent with its description; a click shows the file, the
/// hover-only 「编辑」 edits it.
pub fn dialog(ws: &Workspace, _window: &mut Window, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let group = ws.inspector().config.disclosure.dialog?;
    let summary = ws.inspector().config.summary.clone()?;
    let theme = crate::theme::current(cx);
    let k = Colors::new(&theme.ui, theme.dark);
    let items = group.items(&summary);
    let paths: Vec<PathBuf> = items.iter().map(|r| r.path.clone()).collect();
    let focus = ws.inspector().config.dialog_focus.clone();
    let mut list = div().id("config-dialog-scroll").flex_1().min_h(px(0.)).overflow_y_scroll().px(px(12.)).pb(px(10.));
    for (ix, item) in items.iter().enumerate() {
        let paths = paths.clone();
        // Visible only while the row is hovered (its rect is still recorded while hidden: gpui prepaints hidden elements); a click edits instead of previewing (⌥ flips split / new tab).
        let edit = edit_path(Some(&item.path)).map(|p| {
            let path = p.to_path_buf();
            div()
                .id(("config-dialog-edit", ix))
                .flex_none()
                .px(px(8.))
                .py(px(1.))
                .rounded(px(5.))
                .bg(k.chip_on)
                .text_color(k.chip_text_on)
                .text_size(px(11.))
                .cursor_pointer()
                .invisible()
                .group_hover("config-dlg-row", |d| d.visible())
                .relative()
                .children(rects::recorder(RectId::ConfigDialogEdit(ix)))
                .on_click(cx.listener(move |ws, ev: &ClickEvent, window, cx| {
                    // Not also the row's preview.
                    cx.stop_propagation();
                    ws.open_config_edit(path.clone(), ev.modifiers().alt, window, cx);
                }))
                .child(crate::i18n::text("编辑", "Edit"))
        });
        list = list.child(
            div()
                .id(("config-dialog-row", ix))
                .group("config-dlg-row")
                .relative()
                .children(rects::recorder(RectId::ConfigDialogRow(ix)))
                .mb(px(6.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(8.))
                .bg(k.card)
                .border_1()
                .border_color(k.card_border)
                .cursor_pointer()
                .hover(|d| d.bg(k.row_hover))
                .on_click(cx.listener(move |ws, _, window, cx| ws.open_config_files(paths.clone(), ix, window, cx)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(6.))
                        .child(
                            div()
                                .min_w(px(0.))
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(item.name.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(source_tag(item.source, &k))
                                .children(edit)
                                .child(
                                    div()
                                        .text_color(k.blue)
                                        .child(crate::i18n::text("打开", "Open")),
                                ),
                        ),
                )
                .children(item.description.clone().map(|d| div().mt(px(2.)).text_size(px(12.)).text_color(k.detail).child(d)))
                .child(div().mt(px(2.)).truncate().text_size(px(10.)).text_color(k.muted).child(item.path.display().to_string())),
        );
    }
    Some(
        div()
            .track_focus(&focus)
            .on_key_down(cx.listener(|ws, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    ws.close_config_dialog(window, cx);
                }
            }))
            .max_h(px(520.))
            .flex()
            .flex_col()
            .bg(k.bg)
            .text_color(k.text)
            .text_size(px(13.))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(12.))
                    .pt(px(10.))
                    .pb(px(4.))
                    .child(div().font_weight(FontWeight::BOLD).child(format!("{} · {}", group.title(summary.agent), items.len())))
                    .child(
                        div()
                            .id("config-dialog-close")
                            .relative()
                            .children(rects::recorder(RectId::ConfigDialogClose))
                            .cursor_pointer()
                            .text_color(k.meta)
                            .child("✕")
                            .on_click(cx.listener(|ws, _, window, cx| ws.close_config_dialog(window, cx))),
                    ),
            )
            .child(div().flex_none().px(px(12.)).pb(px(8.)).text_size(px(11.)).text_color(k.meta).child(crate::i18n::text(
                "点击一项预览，悬停出「编辑」（⌥ 翻转分屏 / 新标签），←/→ 切换，Esc 关闭",
                "Click to preview; hover for Edit (⌥ toggles split / new tab), ←/→ switches, Esc closes",
            )))
            .child(list)
            .into_any_element(),
    )
}

fn source_card(summary: &Summary, k: &Colors) -> AnyElement {
    let mut body = card(
        if crate::i18n::current() == crate::i18n::Language::English {
            format!("Configuration Sources · {}", summary.sources.len())
        } else {
            format!("配置来源 · {}", summary.sources.len())
        },
        k,
    );
    if summary.sources.is_empty() {
        body = body.child(
            div()
                .text_size(px(11.))
                .text_color(k.muted)
                .child(crate::i18n::text(
                    "使用 Agent 默认配置",
                    "Using Agent defaults",
                )),
        );
    }
    for source in &summary.sources {
        body = body.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(5.))
                .py(px(2.))
                .child(
                    div()
                        .min_w(px(0.))
                        .truncate()
                        .text_size(px(11.))
                        .child(source.path.display().to_string()),
                )
                .child(source_tag(source.source, k)),
        );
    }
    for warning in &summary.warnings {
        body = body.child(
            div()
                .pt(px(4.))
                .text_size(px(11.))
                .text_color(k.red)
                .child(warning.clone()),
        );
    }
    body.into_any_element()
}

fn source_tag(source: SourceLayer, k: &Colors) -> AnyElement {
    div()
        .flex_none()
        .px(px(5.))
        .rounded(px(7.))
        .bg(k.tag)
        .text_size(px(10.))
        .text_color(k.tag_text)
        .child(source.label())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::edit_path;

    #[test]
    fn only_markdown_paths_get_an_edit_button() {
        assert_eq!(edit_path(Some(Path::new("/r/.claude/skills/a/SKILL.md"))), Some(Path::new("/r/.claude/skills/a/SKILL.md")));
        assert_eq!(edit_path(Some(Path::new("/r/CLAUDE.MD"))), Some(Path::new("/r/CLAUDE.MD")));
        // Entries from JSON / TOML configuration carry no path: no 「编辑」.
        assert_eq!(edit_path(None), None);
        // Defence in depth: a configuration file never becomes editable from here.
        assert_eq!(edit_path(Some(Path::new("/r/.mcp.json"))), None);
        assert_eq!(edit_path(Some(Path::new("/h/.codex/config.toml"))), None);
        assert_eq!(edit_path(Some(Path::new("/r/Makefile"))), None);
    }
}
