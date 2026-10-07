//! Draws the 新建 Agent panel (mockup m3c-new-agent.html): the Agent switch, the 目录 and 初始任务 fields,
//! the 更多 rows, the command preview and the key hints. Colors are the 会话 palette's. Also the text
//! input (IME) of the focused field.

use std::ops::Range;
use std::path::Path;

use gilvt_agent::AgentKind;
use gpui::{
    canvas, div, prelude::*, px, AnyElement, Bounds, Context, Div, ElementInputHandler, FontWeight, Pixels, Point, SharedString,
    UTF16Selection, Window,
};

use super::new_agent_model::{hints, more_line, permission_options, Field, Pick, FOLLOW};
use super::new_agent_view::NewAgentView;
use super::sessions_render::Colors;
use super::Location;
use crate::actions::NEW_AGENT_CONTEXT;
use crate::debug_state::rects::{self, RectId};
use crate::terminal_view::TerminalView;
use crate::theme::{hsla, terminal_font, AppSettings};

/// Candidate directories listed after an ambiguous Tab.
const MAX_CANDIDATES: usize = 8;

fn caret(k: Colors) -> Div {
    div().flex_none().w(px(1.5)).h(px(14.)).bg(k.accent)
}

impl NewAgentView {
    /// The IME input handler over the focused text field (typed text reaches `replace_text_in_range`).
    fn input(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = self.focus_handle.clone();
        let entity = cx.entity();
        canvas(|_, _, _| {}, move |bounds, _, window, cx| {
            window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
        })
        .absolute()
        .size_full()
    }

    /// A labelled row (label column 64 px, as in the mockup); the label is accented while its row has the keys.
    fn row(&self, label: &'static str, field: Option<Field>, k: Colors) -> Div {
        let on = field.is_some_and(|f| f == self.form.field);
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px_3()
            .py(px(5.))
            .child(div().w(px(64.)).flex_none().text_color(if on { k.accent } else { k.muted }).child(label))
    }

    /// A text box; focused → accent border, the caret after the text, the input handler.
    fn text_box(&self, field: Field, k: Colors, cx: &mut Context<Self>) -> Div {
        let focused = self.form.field == field;
        div()
            .relative()
            .children(rects::recorder(RectId::NewAgentField(field as usize)))
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(if focused { k.accent } else { k.menu_border })
            .bg(k.menu_bg)
            .when(focused, |d| d.child(self.input(cx)))
    }

    /// One line of a field's text: the text, then (focused) the IME composition and the caret.
    fn line(&self, text: String, focused: bool, k: Colors) -> Div {
        let row = div().flex().items_center().min_h(px(16.)).child(text);
        match focused {
            true => row.children(self.marked.clone().map(|m| div().underline().child(m))).child(caret(k)),
            false => row,
        }
    }

    fn placeholder(&self, text: &'static str, focused: bool, k: Colors) -> Div {
        div().flex().items_center().min_h(px(16.)).when(focused, |d| d.child(caret(k).mr(px(3.)))).child(div().text_color(k.muted).child(text))
    }

    fn agent_row(&self, k: Colors, cx: &mut Context<Self>) -> Div {
        let segment = |id: &'static str, agent: AgentKind, letter: &'static str, name: &'static str, icon: (gpui::Hsla, gpui::Hsla)| {
            let on = self.form.agent == agent;
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(4.))
                .px(px(12.))
                .py(px(3.))
                .when(on, |d| d.bg(k.accent).text_color(gpui::white()))
                .child(
                    div()
                        .size(px(14.))
                        .rounded(px(4.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(icon.0)
                        .text_color(icon.1)
                        .text_size(px(8.))
                        .font_weight(FontWeight::BOLD)
                        .child(letter),
                )
                .child(name)
                .on_click(cx.listener(move |v, _, _, cx| v.edit(|f| f.set_agent(agent), cx)))
        };
        self.row("Agent", None, k)
            .child(
                div()
                    .flex()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(k.menu_border)
                    .overflow_hidden()
                    .child(segment("agent-claude", AgentKind::Claude, "C", "Claude", (k.claude, gpui::white())))
                    .child(segment("agent-codex", AgentKind::Codex, "X", "Codex", (k.codex, k.codex_text))),
            )
            .child(div().text_size(px(11.)).text_color(k.muted).child("⌘1 / ⌘2 切换"))
    }

    fn dir_row(&self, k: Colors, cx: &mut Context<Self>) -> Div {
        let focused = self.form.field == Field::Dir;
        let note = self.note.filter(|_| self.form.dir_is_default());
        let content = self
            .line(self.form.dir.clone(), focused, k)
            .children(note.map(|n| div().ml(px(4.)).text_color(k.muted).child(n)));
        self.row("目录", Some(Field::Dir), k).child(
            self.text_box(Field::Dir, k, cx)
                .id("new-agent-dir")
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(content)
                .on_click(cx.listener(|v, _, _, cx| v.edit(|f| f.focus(Field::Dir), cx))),
        )
    }

    /// The directories an ambiguous Tab matched, under the 目录 field.
    fn candidates(&self, k: Colors) -> Option<Div> {
        let names = &self.form.candidates;
        (!names.is_empty()).then(|| {
            let mut text: Vec<String> = names.iter().take(MAX_CANDIDATES).map(|n| format!("{n}/")).collect();
            if names.len() > MAX_CANDIDATES {
                text.push("…".into());
            }
            div().pl(px(86.)).pr_3().pb(px(2.)).text_size(px(11.)).text_color(k.muted).truncate().child(text.join("  "))
        })
    }

    fn prompt_row(&self, k: Colors, cx: &mut Context<Self>) -> Div {
        let focused = self.form.field == Field::Prompt;
        let body = if self.form.prompt.is_empty() && self.marked.is_none() {
            div().child(self.placeholder("可以留空，只启动 Agent", focused, k))
        } else {
            let lines: Vec<&str> = self.form.prompt.split('\n').collect();
            let last = lines.len() - 1;
            div().flex().flex_col().children(lines.into_iter().enumerate().map(|(i, l)| self.line(l.to_string(), focused && i == last, k)))
        };
        self.row("初始任务", Some(Field::Prompt), k).items_start().child(
            self.text_box(Field::Prompt, k, cx)
                .id("new-agent-prompt")
                .flex_1()
                .min_w(px(0.))
                .min_h(px(46.))
                .child(body)
                .on_click(cx.listener(|v, _, _, cx| v.edit(|f| f.focus(Field::Prompt), cx))),
        )
    }

    /// The 「在新 worktree 中运行」 checkbox row; drawn only while the directory is inside a git repository.
    fn worktree_row(&self, k: Colors, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        self.form.worktree_available.then(|| {
            let mark = if self.form.worktree { "☑" } else { "☐" };
            div()
                .id("new-agent-worktree")
                .px_3()
                .py(px(4.))
                .text_color(if self.form.worktree { k.accent } else { k.muted })
                .child(format!("{mark} 在新 worktree 中运行（⌥W）"))
                .on_click(cx.listener(|v, _, _, cx| v.edit(|f| f.toggle_worktree(), cx)))
        })
    }

    /// The creation status line: busy (muted) or the failure (red).
    fn status_line(&self, k: Colors) -> Option<Div> {
        let (text, color) = match (&self.busy, &self.error) {
            (Some(busy), _) => (busy.to_string(), k.muted),
            (None, Some(error)) => (error.clone(), k.danger),
            _ => return None,
        };
        Some(div().px_3().pb(px(4.)).text_size(px(11.5)).text_color(color).child(text))
    }

    fn more_line(&self, k: Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let text = more_line(self.form.more);
        let on = self.form.field == Field::More;
        div()
            .id("new-agent-more")
            .relative()
            .children(rects::recorder(RectId::NewAgentField(Field::More as usize)))
            .px_3()
            .py(px(4.))
            .text_color(k.accent)
            .when(on, |d| d.underline())
            .child(text)
            .on_click(cx.listener(|v, _, _, cx| {
                v.edit(
                    |f| {
                        f.focus(Field::More);
                        f.toggle_more();
                    },
                    cx,
                )
            }))
    }

    fn chip(&self, id: impl Into<gpui::ElementId>, label: impl Into<SharedString>, on: bool, k: Colors) -> gpui::Stateful<Div> {
        div()
            .id(id)
            .flex_none()
            .px(px(8.))
            .py(px(2.))
            .rounded(px(10.))
            .text_size(px(11.5))
            .bg(if on { k.chip_on } else { k.chip })
            .text_color(if on { k.accent } else { k.muted })
            .child(label.into())
    }

    fn model_row(&self, k: Colors, cx: &mut Context<Self>) -> Div {
        let form = self.form.current();
        let mut row = self.row("模型", Some(Field::Model), k).relative().children(rects::recorder(RectId::NewAgentField(Field::Model as usize))).flex_wrap().child(
            self.chip("model-follow", FOLLOW, form.pick == Pick::Follow, k)
                .on_click(cx.listener(|v, _, _, cx| v.edit(|f| f.pick_model(Pick::Follow), cx))),
        );
        for (i, preset) in form.presets.iter().enumerate() {
            let pick = Pick::Preset(preset.clone());
            let on = form.pick == pick;
            row = row.child(
                self.chip(("model-preset", i), preset.clone(), on, k)
                    .on_click(cx.listener(move |v, _, _, cx| v.edit(|f| f.pick_model(pick.clone()), cx))),
            );
        }
        let focused = self.form.field == Field::Model;
        let custom = form.pick == Pick::Custom;
        // The caret sits here while the row has the keys: typing there picks the typed name.
        let text = if form.custom.is_empty() && !(focused && self.marked.is_some()) {
            self.placeholder("自定义", focused, k)
        } else {
            self.line(form.custom.clone(), focused, k)
        };
        row.child(
            div()
                .id("model-custom")
                .relative()
                .min_w(px(72.))
                .px(px(6.))
                .py(px(1.))
                .rounded(px(6.))
                .border_1()
                .border_color(if custom { k.accent } else { k.menu_border })
                .bg(k.menu_bg)
                .text_size(px(11.5))
                .when(focused, |d| d.child(self.input(cx)))
                .child(text)
                .on_click(cx.listener(|v, _, _, cx| v.edit(|f| f.pick_model(Pick::Custom), cx))),
        )
    }

    fn permission_row(&self, k: Colors, cx: &mut Context<Self>) -> Div {
        let current = self.form.current().permission;
        let mut row =
            self.row("权限模式", Some(Field::Permission), k).relative().children(rects::recorder(RectId::NewAgentField(Field::Permission as usize))).flex_wrap();
        for (i, option) in permission_options(self.form.agent).into_iter().enumerate() {
            let label = option.map_or(FOLLOW, |p| p.label());
            row = row.child(
                self.chip(("permission", i), label, option == current, k)
                    .on_click(cx.listener(move |v, _, _, cx| v.edit(|f| f.pick_permission(option), cx))),
            );
        }
        row
    }

    fn preview(&self, window: &Window, k: Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let m = window.modifiers();
        let word = self.where_word(Location::from_enter(m.platform, m.shift), cx);
        let preview = self.form.preview(&self.launch_name(cx), word, Path::is_dir);
        // The terminal's font with its fallbacks: a bare family would draw CJK (「将在当前 pane 执行：」, a
        // Chinese path) with Menlo's glyph ids (see `inspector::timeline_view::mono_font`).
        let font = terminal_font(&cx.global::<AppSettings>().0, false, false);
        let color = if preview.missing { k.danger } else { k.text };
        div()
            .mx_3()
            .mt(px(6.))
            .mb(px(8.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(k.bar)
            .text_size(px(11.))
            .font(font)
            .child(div().text_color(if preview.missing { k.danger } else { k.muted }).child(preview.head))
            .child(div().text_color(color).child(preview.command))
    }

    fn footer(&self, k: Colors) -> impl IntoElement {
        let hint = |key: &'static str, what: &'static str| {
            div().flex().gap(px(3.)).child(div().text_color(k.text).font_weight(FontWeight::SEMIBOLD).child(key)).child(what)
        };
        div()
            .flex()
            .flex_wrap()
            .gap_x_3()
            .px_3()
            .py(px(6.))
            .border_t_1()
            .border_color(k.rule)
            .text_size(px(11.))
            .text_color(k.muted)
            .children(hints(self.form.more, self.form.field).into_iter().map(|(key, what)| hint(key, what)))
    }
}

impl Render for NewAgentView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = Colors::new(window, cx);
        let more: Option<[AnyElement; 2]> =
            self.form.more.then(|| [self.model_row(k, cx).into_any_element(), self.permission_row(k, cx).into_any_element()]);
        let root = div().id("new-agent").key_context(NEW_AGENT_CONTEXT).track_focus(&self.focus_handle);
        self.listeners(root, cx)
            .flex()
            .flex_col()
            .bg(hsla(TerminalView::palette(window, cx).background))
            .text_color(k.text)
            .text_size(px(12.5))
            .child(div().px_3().pt(px(9.)).pb(px(4.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child("新建 Agent"))
            .child(self.agent_row(k, cx))
            .child(self.dir_row(k, cx))
            .children(self.candidates(k))
            .child(self.prompt_row(k, cx))
            .children(self.worktree_row(k, cx))
            .child(self.more_line(k, cx))
            .children(more.into_iter().flatten())
            .child(self.preview(window, k, cx))
            .children(self.status_line(k))
            .child(self.footer(k))
    }
}

impl gpui::EntityInputHandler for NewAgentView {
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
        self.insert(text, cx);
    }

    fn replace_and_mark_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    fn bounds_for_range(&mut self, _: Range<usize>, element: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        // The candidate window opens below the focused field.
        Some(element)
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}
