//! The settings window's view: a nav column with the pages (「外观」 … 「⌘ Agent」), and the page shown.

use std::process::Stdio;

use gilvt_monitor::provider::models::ModelChoice;
use gilvt_monitor::provider::ProviderError;
use gilvt_term::Rgb;
use gilvt_theme::color::contrast;
use gilvt_theme::{ResolvedTheme, Status};
use gilvt_agent::AgentKind;
use gpui::{
    anchored, deferred, div, prelude::*, px, AnyElement, ClickEvent, ClipboardItem, Context, Div, Entity, FocusHandle, Focusable, Hsla, KeyDownEvent,
    MouseButton, MouseDownEvent, PathPromptOptions, Subscription, Task, Window,
};

use super::agent_page::{self, AgentField, AgentView, Problem};
use super::appearance::AppearancePage;
use super::form::{self, CodexModels, FieldId, FieldModel, Notice, OptValue, TestState, TrialOutcome};
use super::{probe, Page};
use crate::actions::ClosePane;
use crate::config_file::{self, edit::Edit, ConfigFile};
use crate::debug_state::rects::{self, RectId};
use crate::i18n::Language;
use crate::settings::{MonitorProvider, MonitorSettings};
use crate::sidebar::rename::{RenameEvent, RenameField};
use crate::theme::{hsla, AppSettings};

/// A probe thread ended without an answer (it could not start, or died).
pub const PROBE_LOST: &str = "检查意外中断，请再试一次";

/// [`PROBE_LOST`] in the interface language.
pub fn probe_lost() -> &'static str {
    crate::i18n::text(PROBE_LOST, "The check stopped unexpectedly; try again")
}

/// 「测试连接」's chat half: gilvt's CLI and socket, or why it is not tried. Off: never tried — no token is issued
/// while the switch promises that no data leaves (a test token must not get round `tools::DISABLED`).
fn chat_launch(enabled: bool, env: Option<&crate::launch::ShellEnv>) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    if !enabled {
        return Err(crate::monitor::tools::disabled().to_string());
    }
    let env = env.ok_or_else(|| {
        crate::i18n::text(
            "找不到 gilvt 的启动环境",
            "Could not find gilvt's launch environment",
        )
        .to_string()
    })?;
    match (
        env.bin_dir
            .as_ref()
            .map(|d| d.join("gilvt"))
            .filter(|p| p.is_file()),
        env.socket.clone(),
    ) {
        (Some(gilvt), Some(socket)) => Ok((gilvt, socket)),
        (None, _) => Err(
            crate::i18n::text("找不到 gilvt 命令行", "Could not find the gilvt CLI").to_string(),
        ),
        (_, None) => Err(crate::i18n::text(
            "gilvt 的本地通信没有启动",
            "gilvt's local IPC is not running",
        )
        .to_string()),
    }
}

/// What a probe asked about. An answer whose key no longer matches the settings is stale and not applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeKey {
    provider: MonitorProvider,
    command: String,
    model: String,
}

impl ProbeKey {
    /// A 「其他…」 trial for `field`: its current value counts (a newer choice must not be overwritten).
    pub fn trial(m: &MonitorSettings, field: FieldId) -> Self {
        let model = if field == FieldId::SummaryModel { &m.summary_model } else { &m.model };
        ProbeKey { provider: m.provider, command: m.command.clone(), model: model.clone() }
    }

    /// 「测试连接」 runs with the summary model and the chat model.
    pub fn test(m: &MonitorSettings) -> Self {
        ProbeKey { provider: m.provider, command: m.command.clone(), model: format!("{}\n{}", m.summary_model().unwrap_or_default(), m.chat_model().unwrap_or_default()) }
    }
}

/// The text box of 「其他…」 (a model name to try) or of the CLI path.
pub struct OtherInput {
    pub field: FieldId,
    pub input: Entity<RenameField>,
    /// The trial run is going.
    pub trying: bool,
    _events: Subscription,
}

/// A text box of the 「⌘ Agent」 page while it is open.
pub struct AgentInput {
    pub field: AgentField,
    pub input: Entity<RenameField>,
    /// Commit / cancel / blur, and a repaint per keystroke (the launch preview follows the typing).
    _events: [Subscription; 2],
}

pub struct SettingsWindow {
    focus_handle: FocusHandle,
    /// The page shown.
    page: Page,
    appearance: Entity<AppearancePage>,
    pub(super) open_menu: Option<FieldId>,
    pub(super) other: Option<OtherInput>,
    pub(super) notice: Option<Notice>,
    pub(super) codex: CodexModels,
    pub(super) test: TestState,
    /// Which provider and command `codex` was read for.
    models_for: Option<(MonitorProvider, String)>,
    models_task: Option<Task<()>>,
    test_task: Option<Task<()>>,
    trial_task: Option<Task<()>>,
    /// The 「⌘ Agent」 page: its status (computed when it opens and on 「刷新」) and refusals.
    pub(super) agent: AgentView,
    pub(super) agent_input: Option<AgentInput>,
    _observers: [Subscription; 3],
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

pub(super) struct Colors {
    pub bg: Hsla,
    pub nav: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub rule: Hsla,
    pub field: Hsla,
    pub field_border: Hsla,
    pub accent: Hsla,
    /// Text on `accent`.
    pub on_accent: Hsla,
    pub seg_on: Hsla,
    pub warn: (Hsla, Hsla, Hsla),
    pub error: (Hsla, Hsla, Hsla),
    pub ok: (Hsla, Hsla, Hsla),
    pub menu_bg: Hsla,
    pub menu_hover: Hsla,
    pub menu_hover_text: Hsla,
}

const WHITE: Rgb = Rgb { r: 0xff, g: 0xff, b: 0xff };
const NEAR_BLACK: Rgb = Rgb { r: 0x11, g: 0x11, b: 0x11 };

impl Colors {
    /// From the theme's chrome colors (spec §4.1). The page's accent is the 监控官's purple (`ui.purple`); text on it
    /// and on the menu's hover (`ui.accent`) follows the `on_accent` rule. Text boxes take the terminal background.
    pub fn new(theme: &ResolvedTheme) -> Self {
        let ui = &theme.ui;
        let h = hsla;
        let on = |bg| h(if contrast(WHITE, bg) >= 3.0 { WHITE } else { NEAR_BLACK });
        let status = |s: &Status| (h(s.bg), h(s.border), h(s.fg));
        Colors {
            bg: h(ui.panel),
            nav: h(ui.hover),
            text: h(ui.text),
            muted: h(ui.text_3),
            rule: h(ui.border),
            field: h(theme.palette.background),
            field_border: h(ui.border_strong),
            accent: h(ui.purple),
            on_accent: on(ui.purple),
            seg_on: h(ui.border_strong),
            warn: status(&ui.attention),
            error: status(&ui.error),
            ok: status(&ui.done),
            menu_bg: h(ui.raised),
            menu_hover: h(ui.accent),
            menu_hover_text: h(ui.on_accent),
        }
    }
}

impl SettingsWindow {
    /// DebugState `settings`: the page as data plus the rects of the window's last frame.
    pub fn debug_state(&self, id: Option<u64>, key: bool, titlebar: f32, rects: &std::collections::HashMap<RectId, rects::Rect4>, cx: &gpui::App) -> crate::debug_state::SettingsState {
        let at = |r: RectId| rects.get(&r).map(|x| rects::in_frame(*x, titlebar));
        let appearance = self.appearance.read(cx).debug_state(&at, cx);
        let page = super::debug::PageState {
            page: self.page,
            appearance,
            open_menu: self.open_menu,
            other: self.other.as_ref().map(|o| (o.field, o.input.read(cx).text().to_string(), o.trying)),
            notice: self.notice.clone(),
            codex: self.codex.clone(),
            test: self.test.clone(),
            agent: AgentView {
                editing: self.agent_input.as_ref().map(|i| (i.field, i.input.read(cx).text().to_string())),
                can_restore: can_restore(cx),
                ..self.agent.clone()
            },
        };
        let file = cx.global::<ConfigFile>();
        let view = super::debug::FileView {
            error: file.error.as_deref(),
            write_error: file.write_error.as_deref(),
            path: &file.path,
        };
        super::debug::build(
            &page,
            &cx.global::<AppSettings>().0,
            view,
            id,
            key,
            titlebar,
            rects,
        )
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let appearance = cx.new(AppearancePage::new);
        let page = super::last_page(cx);
        // The 外观 page takes typing (its search box); the 监控官 page has the keyboard itself.
        match page {
            Page::Appearance => window.focus(&appearance.focus_handle(cx)),
            Page::Language | Page::Monitor | Page::Agent => window.focus(&focus_handle),
        }
        // Closing the window mid-test voids the test token (the task that would clear it is dropped with the view).
        cx.on_release(|_, cx| crate::monitor::tools::set_test_token(None, cx)).detach();
        let observers = [
            cx.observe_global::<AppSettings>(|this: &mut Self, cx| {
                this.sync_models(cx);
                cx.notify();
            }),
            cx.observe_global::<ConfigFile>(|_, cx| cx.notify()),
            // Light / dark follow the system: repaint with the other palette.
            cx.observe_window_appearance(window, |_, window, _| window.refresh()),
        ];
        let mut this = SettingsWindow {
            focus_handle,
            page,
            appearance,
            open_menu: None,
            other: None,
            notice: None,
            codex: CodexModels::Loading,
            test: TestState::Idle,
            models_for: None,
            models_task: None,
            test_task: None,
            trial_task: None,
            agent: AgentView::default(),
            agent_input: None,
            _observers: observers,
        };
        this.sync_models(cx);
        if page == Page::Agent {
            this.refresh_agent_status(cx);
        }
        this
    }

    #[cfg(test)]
    pub fn page(&self) -> Page {
        self.page
    }

    /// Shows `page` (a nav click); it is also the page the window opens on next time.
    pub fn show_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        cx.set_global(super::LastPage(page));
        self.open_menu = None;
        self.close_other();
        self.agent_input = None;
        match page {
            Page::Appearance => window.focus(&self.appearance.focus_handle(cx)),
            Page::Language | Page::Monitor | Page::Agent => window.focus(&self.focus_handle),
        }
        if page == Page::Agent {
            self.refresh_agent_status(cx);
        }
        cx.notify();
    }

    fn nav(&self, k: &Colors, cx: &Context<Self>) -> Div {
        let language = cx.global::<AppSettings>().0.language;
        let mut nav = div()
            .w(px(150.))
            .flex_none()
            .h_full()
            .bg(k.nav)
            .border_r_1()
            .border_color(k.rule)
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(2.));
        for page in Page::ALL {
            let on = page == self.page;
            nav = nav.child(
                div()
                    .id(("settings-nav", page.index()))
                    .relative()
                    .children(rects::recorder(RectId::SettingsNav(page.index())))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(5.))
                    .when(on, |d| d.bg(k.accent).text_color(k.on_accent))
                    .when(!on, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(k.seg_on))
                            .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                                view.show_page(page, window, cx)
                            }))
                    })
                    .child(page.label(language)),
            );
        }
        nav
    }

    /// Reads Codex's list when the page shows Codex and it was not read for this provider + command yet.
    fn sync_models(&mut self, cx: &mut Context<Self>) {
        let m = &cx.global::<AppSettings>().0.monitor;
        let key = (m.provider, m.command.clone());
        if self.models_for.as_ref() != Some(&key) {
            self.fetch_models(cx);
        }
    }

    fn fetch_models(&mut self, cx: &mut Context<Self>) {
        let m = cx.global::<AppSettings>().0.monitor.clone();
        let key = (m.provider, m.command.clone());
        self.models_for = Some(key.clone());
        if m.provider != MonitorProvider::Codex {
            self.models_task = None;
            return;
        }
        self.codex = CodexModels::Loading;
        let cfg = probe::provider_config(&m, None);
        let program = cfg.program();
        let rx = probe::spawn(move || probe::models(&cfg));
        // Replacing the task drops the previous one: an older answer is never applied.
        self.models_task = Some(cx.spawn(async move |this, cx| {
            let result = rx.recv().await.ok();
            let _ = this.update(cx, |view, cx| view.models_arrived(key, program, result, cx));
        }));
        cx.notify();
    }

    /// Codex's list for `key`; None: the probe ended without an answer.
    fn models_arrived(&mut self, key: (MonitorProvider, String), program: String, result: Option<Result<Vec<ModelChoice>, ProviderError>>, cx: &mut Context<Self>) {
        if self.models_for.as_ref() != Some(&key) {
            return;
        }
        self.codex = match result {
            Some(Ok(list)) => CodexModels::Ready(list),
            Some(Err(e)) => CodexModels::Failed(form::advice(&program, &e)),
            None => CodexModels::Failed(probe_lost().into()),
        };
        cx.notify();
    }

    fn refresh_models(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.fetch_models(cx);
    }

    /// Writes `edits` (in memory at once, then into config.toml); a refusal (read-only) shows in red.
    fn apply(&mut self, edits: Vec<Edit>, cx: &mut Context<Self>) {
        self.notice = config_file::set_monitor(edits, cx).err().map(|text| Notice { error: true, text });
        cx.notify();
    }

    fn choose_language(
        &mut self,
        language: Language,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.notice = config_file::set_language(language, cx)
            .err()
            .map(|text| Notice { error: true, text });
        window.set_window_title(language.text("设置", "Settings"));
        cx.notify();
    }

    fn language_page(
        &self,
        current: Language,
        readonly: bool,
        k: &Colors,
        cx: &Context<Self>,
    ) -> Div {
        let t = |zh, en| current.text(zh, en);
        let mut choices = div().flex().flex_col().gap(px(8.));
        for (index, language) in Language::ALL.into_iter().enumerate() {
            let selected = language == current;
            choices = choices.child(
                div()
                    .id(("settings-language", index))
                    .relative()
                    .children(rects::recorder(RectId::SettingsLanguage(index)))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if selected { k.accent } else { k.field_border })
                    .bg(if selected { k.seg_on } else { k.field })
                    .child(language.label())
                    .child(if selected { "✓" } else { "" })
                    .when(readonly, |d| d.opacity(0.5))
                    .when(!readonly && !selected, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(k.seg_on))
                            .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                                view.choose_language(language, window, cx);
                            }))
                    }),
            );
        }
        section(k, t("界面语言", "Interface Language"))
            .child(choices)
            .child(
                div()
                    .mt(px(8.))
                    .text_size(px(10.5))
                    .text_color(k.muted)
                    .child(t("选择后立即应用到所有窗口，并写入 config.toml。终端内容不会被翻译。", "Changes apply immediately to every window and are saved to config.toml. Terminal content is never translated.")),
            )
    }

    fn toggle(&mut self, field: FieldId, cx: &mut Context<Self>) {
        let edits = form::edits_for_toggle(field, &cx.global::<AppSettings>().0.monitor);
        self.apply(edits, cx);
    }

    fn pick(&mut self, field: FieldId, value: OptValue, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu = None;
        if value == OptValue::Other {
            self.start_input(field, window, cx);
            return;
        }
        let edits = form::edits_for_choice(field, &value, &cx.global::<AppSettings>().0.monitor);
        self.apply(edits, cx);
    }

    /// Opens the text box of 「其他…」 (`field` = a model field) or of the CLI path (`FieldId::Command`).
    fn start_input(&mut self, field: FieldId, window: &mut Window, cx: &mut Context<Self>) {
        // A trial still running for another box is abandoned with it.
        self.close_other();
        let (current, placeholder) = match field {
            FieldId::Command => (
                cx.global::<AppSettings>().0.monitor.command.clone(),
                crate::i18n::text(
                    "CLI 路径，留空 = 在 PATH 里查找",
                    "CLI path; leave empty to search PATH",
                ),
            ),
            _ => (
                String::new(),
                crate::i18n::text("模型名，⏎ 试跑", "Model name; press Return to try"),
            ),
        };
        let input = cx.new(|cx| RenameField::with_placeholder(&current, placeholder, window, cx));
        let events = cx.subscribe_in(&input, window, move |view, _, event: &RenameEvent, window, cx| match event {
            RenameEvent::Commit(text) if field == FieldId::Command => {
                let edits = form::edits_for_command(text, &cx.global::<AppSettings>().0.monitor);
                view.other = None;
                window.focus(&view.focus_handle);
                view.apply(edits, cx);
            }
            RenameEvent::Commit(name) if !name.is_empty() => view.try_model(field, name.clone(), window, cx),
            RenameEvent::Commit(_) | RenameEvent::Cancel => {
                view.close_other();
                window.focus(&view.focus_handle);
                cx.notify();
            }
            RenameEvent::Blurred => {
                if view.other.as_ref().is_some_and(|o| !o.trying) {
                    view.other = None;
                    cx.notify();
                }
            }
        });
        window.focus(&input.focus_handle(cx));
        self.other = Some(OtherInput { field, input, trying: false, _events: events });
        self.notice = None;
        cx.notify();
    }

    /// Closes the text box; a trial still running for it is abandoned (its answer is never applied).
    fn close_other(&mut self) {
        self.other = None;
        if self.trial_task.take().is_some() {
            self.notice = None;
        }
    }

    /// 「其他…」: one summary with `name`; written only if it works (S2 §5.2).
    fn try_model(&mut self, field: FieldId, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let m = cx.global::<AppSettings>().0.monitor.clone();
        let key = ProbeKey::trial(&m, field);
        let cfg = probe::provider_config(&m, Some(&name));
        let program = cfg.program();
        if let Some(o) = self.other.as_mut() {
            o.trying = true;
        }
        self.notice = Some(Notice {
            error: false,
            text: if crate::i18n::current() == Language::English {
                format!("Trying one summary with {name}…")
            } else {
                format!("正在用 {name} 试跑一次总结…")
            },
        });
        let rx = probe::spawn(move || probe::trial(&cfg));
        // In the window: closing the box hands the keyboard back to the page.
        self.trial_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = rx.recv().await.ok();
            let _ = this.update_in(cx, |view, window, cx| view.trial_arrived(field, name, key, program, result, window, cx));
        }));
        cx.notify();
    }

    /// The trial of `name` for `field`, started with `started`; None: the probe ended without an answer. Dropped
    /// when its box is no longer the one trying (closed, or another box opened since).
    #[allow(clippy::too_many_arguments)]
    fn trial_arrived(&mut self, field: FieldId, name: String, started: ProbeKey, program: String, result: Option<Result<(), ProviderError>>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.other.as_ref().is_some_and(|o| o.field == field && o.trying) {
            return;
        }
        let m = &cx.global::<AppSettings>().0.monitor;
        let now = ProbeKey::trial(m, field);
        let outcome = match result {
            // The CLI path or the field itself changed meanwhile: the answer is about something else.
            Some(_) if now.command != started.command || now.model != started.model => TrialOutcome::Stale,
            Some(r) => form::trial_outcome(field, &name, started.provider, now.provider, &program, r),
            None => TrialOutcome::Rejected(probe_lost().into()),
        };
        match outcome {
            TrialOutcome::Write(edits) => {
                self.other = None;
                window.focus(&self.focus_handle);
                self.apply(edits, cx);
            }
            TrialOutcome::Rejected(text) => {
                if let Some(o) = self.other.as_mut() {
                    o.trying = false;
                }
                self.notice = Some(Notice { error: true, text });
            }
            TrialOutcome::Stale => {
                self.other = None;
                window.focus(&self.focus_handle);
                self.notice = Some(Notice {
                    error: true,
                    text: crate::i18n::text(
                        "设置已改变，试跑结果作废",
                        "Settings changed; the trial result was discarded",
                    )
                    .into(),
                });
            }
        }
        cx.notify();
    }

    fn run_test(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let m = cx.global::<AppSettings>().0.monitor.clone();
        let key = ProbeKey::test(&m);
        let cfg = probe::provider_config(&m, m.summary_model());
        let program = cfg.program();
        // The token is cleared when the answer arrives, or by the window's release if it closes first.
        let launch = chat_launch(m.enabled, cx.try_global::<crate::launch::ShellEnv>());
        let chat = launch.map(|(gilvt, socket)| {
            let token = crate::monitor::tools::new_token();
            crate::monitor::tools::set_test_token(Some(token.clone()), cx);
            probe::ChatProbe { mcp: gilvt_monitor::chat::McpLaunch { gilvt, socket, token }, model: m.chat_model().map(String::from) }
        });
        self.test = TestState::Running;
        let rx = probe::spawn(move || probe::test_connection(&cfg, chat));
        self.test_task = Some(cx.spawn(async move |this, cx| {
            let result = rx.recv().await.ok();
            let _ = cx.update(|cx| crate::monitor::tools::set_test_token(None, cx));
            let _ = this.update(cx, |view, cx| view.test_arrived(key, program, result, cx));
        }));
        cx.notify();
    }

    /// 「测试连接」's answer for the settings `started`; None: the probe ended without an answer.
    fn test_arrived(&mut self, started: ProbeKey, program: String, result: Option<probe::TestResult>, cx: &mut Context<Self>) {
        let (ok, text) = match result {
            _ if ProbeKey::test(&cx.global::<AppSettings>().0.monitor) != started => (
                false,
                crate::i18n::text(
                    "✗ 测试期间设置已改变，请再测一次",
                    "✗ Settings changed during the test; run it again",
                )
                .to_string(),
            ),
            Some(r) => form::test_line(&program, &r.version, &r.summary, &r.chat),
            None => (false, format!("✗ {}", probe_lost())),
        };
        self.test = TestState::Done { ok, text };
        cx.notify();
    }

    fn choose_command(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(crate::i18n::text("选择", "Choose").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |view, cx| {
                let edits = form::edits_for_command(&path.display().to_string(), &cx.global::<AppSettings>().0.monitor);
                view.apply(edits, cx);
            });
        })
        .detach();
    }

    fn add_exclude(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(crate::i18n::text("排除", "Exclude").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(dir) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |view, cx| {
                let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
                let edits = form::edits_for_added_dir(&dir, home.as_deref(), &cx.global::<AppSettings>().0.monitor);
                view.apply(edits, cx);
            });
        })
        .detach();
    }

    /// `open -t` (the default text editor); an empty file is created first when there is none.
    fn open_config(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let path = cx.global::<ConfigFile>().path.clone();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(&path);
        // A first-time user's directory exists only now: watch it, so the editor's saves apply.
        config_file::ensure_watching(cx);
        let spawned = std::process::Command::new("/usr/bin/open").arg("-t").arg(&path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        match spawned {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => {
                self.notice = Some(Notice {
                    error: true,
                    text: if crate::i18n::current() == Language::English {
                        format!("Could not open editor: {e}")
                    } else {
                        format!("无法打开编辑器：{e}")
                    },
                });
                cx.notify();
            }
        }
    }

    /// Where each command name resolves now (spec §5.1 item 3): the app's PATH has no shims directory in it, so the
    /// first match is the real program.
    pub(super) fn refresh_agent_status(&mut self, cx: &mut Context<Self>) {
        let agent = &cx.global::<AppSettings>().0.agent;
        let env = cx.try_global::<crate::launch::ShellEnv>();
        let (shims_dir, shims_error) = env.map(|e| (e.shims_dir.clone(), e.shims_error.clone())).unwrap_or_default();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
        self.agent.status = agent_page::status_rows(&agent.claude_commands, &agent.codex_commands, &path, shims_dir.as_deref(), &home);
        (self.agent.shims_dir, self.agent.shims_error) = (shims_dir, shims_error);
        cx.notify();
    }

    fn agent_names(kind: AgentKind, cx: &gpui::App) -> Vec<String> {
        let agent = &cx.global::<AppSettings>().0.agent;
        match kind {
            AgentKind::Claude => agent.claude_commands.clone(),
            AgentKind::Codex => agent.codex_commands.clone(),
        }
    }

    /// The 「+」 of a command list: `name` joins it (already there: nothing to do; in the other list: refused).
    pub fn add_agent_command(&mut self, kind: AgentKind, name: String, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        let mut names = Self::agent_names(kind, cx);
        let other = match kind {
            AgentKind::Claude => AgentKind::Codex,
            AgentKind::Codex => AgentKind::Claude,
        };
        if name.is_empty() || names.contains(&name) {
            self.agent_input = None;
            cx.notify();
            return;
        }
        if Self::agent_names(other, cx).contains(&name) {
            let text = if crate::i18n::english() {
                format!("\"{name}\" is already a {} command", other.name())
            } else {
                format!("「{name}」已经是 {} 的命令名", other.name())
            };
            self.notice = Some(Notice { error: true, text });
            cx.notify();
            return;
        }
        names.push(name);
        self.set_names(kind, names, cx);
    }

    /// The × of name `ix` in `kind`'s list.
    pub fn remove_agent_command(&mut self, kind: AgentKind, ix: usize, cx: &mut Context<Self>) {
        let mut names = Self::agent_names(kind, cx);
        if ix < names.len() {
            names.remove(ix);
            self.set_names(kind, names, cx);
        }
    }

    fn set_names(&mut self, kind: AgentKind, names: Vec<String>, cx: &mut Context<Self>) {
        match config_file::set_agent_commands(kind, names, cx) {
            Ok(()) => {
                self.notice = None;
                self.agent_input = None;
                // The shims were rebuilt for the new lists.
                self.refresh_agent_status(cx);
            }
            Err(text) => self.notice = Some(Notice { error: true, text }),
        }
        cx.notify();
    }

    /// Opens a text box of the page: the box before a 「+」, or a launch command.
    pub(super) fn start_agent_input(&mut self, field: AgentField, window: &mut Window, cx: &mut Context<Self>) {
        let (current, placeholder) = match field {
            AgentField::Add(_) => (String::new(), crate::i18n::text("命令名，⏎ 添加", "Command name; press Return to add")),
            AgentField::Launch(kind) => (
                cx.global::<AppSettings>().0.agent.launch(kind).to_string(),
                crate::i18n::text("命令和参数，⏎ 保存", "Command and arguments; press Return to save"),
            ),
        };
        let input = cx.new(|cx| RenameField::with_placeholder(&current, placeholder, window, cx));
        let events = cx.subscribe_in(&input, window, move |view, _, event: &RenameEvent, window, cx| match (event, field) {
            (RenameEvent::Commit(text), AgentField::Add(kind)) => {
                view.add_agent_command(kind, text.clone(), cx);
                if view.agent_input.is_none() {
                    window.focus(&view.focus_handle);
                }
            }
            (RenameEvent::Commit(text), AgentField::Launch(kind)) => view.save_launch(kind, text.clone(), window, cx),
            (RenameEvent::Cancel, _) => {
                view.agent_input = None;
                window.focus(&view.focus_handle);
                cx.notify();
            }
            (RenameEvent::Blurred, _) => {
                if view.agent_input.as_ref().is_some_and(|i| i.field == field) {
                    view.agent_input = None;
                    cx.notify();
                }
            }
        });
        let typing = cx.observe(&input, |_, _, cx| cx.notify());
        window.focus(&input.focus_handle(cx));
        if let AgentField::Launch(kind) = field {
            self.agent.launch_error[agent_page::kind_index(kind)] = None;
        }
        self.agent_input = Some(AgentInput { field, input, _events: [events, typing] });
        self.notice = None;
        cx.notify();
    }

    /// ⏎ in a launch command box: saved, or refused under the box (which stays open for a fix).
    pub(super) fn save_launch(&mut self, kind: AgentKind, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let k = agent_page::kind_index(kind);
        match config_file::set_agent_launch(kind, text, cx) {
            Ok(()) => {
                self.agent.launch_error[k] = None;
                self.agent_input = None;
                window.focus(&self.focus_handle);
            }
            Err(e) => self.agent.launch_error[k] = Some(e),
        }
        cx.notify();
    }

    fn copy_text(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.notice = Some(Notice { error: false, text: crate::i18n::text("已复制", "Copied").into() });
        cx.notify();
    }

    /// 「恢复所有已关闭的提示」: every dismissed agent hint shows again.
    fn restore_hints(&mut self, cx: &mut Context<Self>) {
        if !cx.has_global::<crate::agents::AgentHints>() {
            return;
        }
        let hints = cx.global_mut::<crate::agents::AgentHints>();
        hints.dismissed.clear();
        let saved = crate::agents::state_dir().map_or(Ok(()), |dir| hints.dismissed.save(&dir));
        self.notice = Some(match saved {
            Ok(()) => Notice { error: false, text: crate::i18n::text("已恢复，之后会再次提示", "Restored; hints will show again").into() },
            // Restored for this run all the same.
            Err(e) => Notice { error: true, text: format!("agent-hints.json: {e}") },
        });
        cx.notify();
    }

    fn agent_button(&self, i: usize, label: &'static str, enabled: bool, k: &Colors, cx: &Context<Self>, on: impl Fn(&mut Self, &mut Context<Self>) + 'static) -> AnyElement {
        div()
            .id(("settings-agent-button", i))
            .relative()
            .children(rects::recorder(RectId::SettingsAgentButton(i)))
            .flex_none()
            .px(px(10.))
            .py(px(3.))
            .rounded(px(5.))
            .bg(k.seg_on)
            .child(label)
            .when(!enabled, |d| d.opacity(0.5))
            .when(enabled, |d| d.cursor_pointer().on_click(cx.listener(move |view, _: &ClickEvent, _, cx| on(view, cx))))
            .into_any_element()
    }

    /// A launch command / add box: the open text box, or a box showing `shown` that opens it on a click.
    #[allow(clippy::too_many_arguments)]
    fn agent_box(&self, field: AgentField, rect: RectId, shown: String, muted: bool, width: f32, readonly: bool, k: &Colors, cx: &Context<Self>) -> AnyElement {
        if let Some(i) = self.agent_input.as_ref().filter(|i| i.field == field) {
            return div().relative().min_w(px(width)).children(rects::recorder(rect)).child(i.input.clone()).into_any_element();
        }
        let id = match field {
            AgentField::Add(kind) => agent_page::kind_index(kind),
            AgentField::Launch(kind) => 2 + agent_page::kind_index(kind),
        };
        div()
            .id(("settings-agent-box", id))
            .relative()
            .children(rects::recorder(rect))
            .min_w(px(width))
            .px(px(7.))
            .py(px(3.))
            .rounded(px(5.))
            .border_1()
            .border_color(k.field_border)
            .bg(k.field)
            .font_family("Menlo")
            .text_size(px(11.))
            .when(muted, |d| d.text_color(k.muted))
            .child(shown)
            .when(readonly, |d| d.opacity(0.5))
            .when(!readonly, |d| d.cursor_text().on_click(cx.listener(move |view, _: &ClickEvent, window, cx| view.start_agent_input(field, window, cx))))
            .into_any_element()
    }

    fn agent_page(&self, readonly: bool, k: &Colors, cx: &Context<Self>) -> Vec<AnyElement> {
        let t = crate::i18n::text;
        let settings = &cx.global::<AppSettings>().0;
        let kind_label = |kind: AgentKind| match kind {
            AgentKind::Claude => "Claude Code",
            AgentKind::Codex => "Codex",
        };
        let mut out = Vec::new();
        if let Some(e) = &self.agent.shims_error {
            let text = if crate::i18n::english() { format!("The shims folder could not be set up: {e}") } else { format!("垫片目录建不起来：{e}") };
            out.push(notice_box(k.warn, text).into_any_element());
        }
        if let Some(n) = &self.notice {
            out.push(notice_box(if n.error { k.error } else { k.ok }, n.text.clone()).into_any_element());
        }

        let mut names = section(k, t("识别的命令名", "Command names"));
        for kind in agent_page::KINDS {
            let ki = agent_page::kind_index(kind);
            let list = match kind {
                AgentKind::Claude => &settings.agent.claude_commands,
                AgentKind::Codex => &settings.agent.codex_commands,
            };
            let mut wrap = div().flex().flex_wrap().items_center().gap(px(6.));
            for (j, name) in list.iter().enumerate() {
                wrap = wrap.child(
                    div()
                        .id(("settings-agent-chip", ki * 1000 + j))
                        .relative()
                        .children(rects::recorder(RectId::SettingsAgentChip(ki, j)))
                        .flex()
                        .gap(px(4.))
                        .px(px(6.))
                        .rounded(px(4.))
                        .bg(k.seg_on)
                        .font_family("Menlo")
                        .text_size(px(10.5))
                        .child(name.clone())
                        .child(
                            div()
                                .id(("settings-agent-chip-x", ki * 1000 + j))
                                .relative()
                                .children(rects::recorder(RectId::SettingsAgentChipRemove(ki, j)))
                                .child("×")
                                .when(!readonly, |d| d.cursor_pointer().on_click(cx.listener(move |view, _: &ClickEvent, _, cx| view.remove_agent_command(kind, j, cx)))),
                        ),
                );
            }
            let field = AgentField::Add(kind);
            wrap = wrap.child(self.agent_box(field, RectId::SettingsAgentInput(ki), t("命令名", "name").into(), true, 110., readonly, k, cx)).child(
                div()
                    .id(("settings-agent-add", ki))
                    .relative()
                    .children(rects::recorder(RectId::SettingsAgentAdd(ki)))
                    .flex_none()
                    .px(px(8.))
                    .py(px(1.))
                    .rounded(px(5.))
                    .bg(k.seg_on)
                    .child("+")
                    .when(readonly, |d| d.opacity(0.5))
                    .when(!readonly, |d| {
                        // Mouse-down, stopped here: the page's own mouse-down would take the focus, and the box
                        // would close (losing what was typed) before a click arrived.
                        d.cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(move |view, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            window.prevent_default();
                            match view.agent_input.as_ref().filter(|i| i.field == field) {
                                Some(i) => {
                                    let text = i.input.read(cx).text().to_string();
                                    view.add_agent_command(kind, text, cx);
                                    if view.agent_input.is_none() {
                                        window.focus(&view.focus_handle);
                                    }
                                }
                                None => view.start_agent_input(field, window, cx),
                            }
                        }))
                    }),
            );
            names = names.child(row(k, kind_label(kind), wrap, None));
        }
        names = names.child(div().mt(px(6.)).text_size(px(10.5)).text_color(k.muted).child(t(
            "在 gilvt 的 pane 里，这些命令（包括被 ttadk、脚本等调用时）都会被识别为 agent。对新开的 pane 生效；名字只能包含字母、数字和 . _ + -。",
            "In gilvt panes these commands are recognized as agents, also when ttadk, a script or another program runs them. Applies to new panes; names may contain letters, digits and . _ + -.",
        )));
        out.push(names.into_any_element());

        let mut launch = section(k, t("启动命令", "Launch command"));
        for kind in agent_page::KINDS {
            let ki = agent_page::kind_index(kind);
            let field = AgentField::Launch(kind);
            let saved = settings.agent.launch(kind).to_string();
            let typed = self.agent_input.as_ref().filter(|i| i.field == field).map(|i| i.input.read(cx).text().to_string());
            let (new, resume) = agent_page::launch_preview(kind, typed.as_deref().unwrap_or(&saved));
            let preview = div()
                .flex()
                .flex_col()
                .text_size(px(10.5))
                .text_color(k.muted)
                .child(div().font_family("Menlo").child(format!("{} {new}", t("新建：", "New:"))))
                .child(div().font_family("Menlo").child(format!("{} {resume}", t("恢复：", "Resume:"))))
                .children(self.agent.launch_error[ki].clone().map(|e| div().text_color(k.error.2).child(e)));
            let control = div().flex().flex_col().gap(px(4.)).child(self.agent_box(field, RectId::SettingsAgentLaunch(ki), saved, false, 240., readonly, k, cx)).child(preview);
            launch = launch.child(row(k, kind_label(kind), control, None));
        }
        out.push(launch.into_any_element());

        let can_restore = can_restore(cx);
        let mut status = section(k, t("状态", "Status")).child(
            div().flex().items_center().gap(px(8.)).mb(px(4.)).child(self.agent_button(0, agent_page::button_labels(&self.agent)[0], true, k, cx, |view, cx| view.refresh_agent_status(cx))).child(
                div().text_size(px(10.5)).text_color(k.muted).child(t("真正的程序是 PATH 里排在垫片后面的第一个", "The real program is the first one on PATH after the shims")),
            ),
        );
        let copy_buttons: std::collections::HashMap<usize, usize> = (3..).zip(self.agent.copy_rows()).map(|(b, (i, _))| (i, b)).collect();
        let shims_dir = self.agent.shims_dir.clone();
        for (i, r) in self.agent.status.iter().enumerate() {
            let why = r.problem.map(|p| match p {
                Problem::NotFound => t("PATH 里找不到这个命令", "Not found on PATH"),
                Problem::BypassedByTtadk => t(
                    "装在 ~/.local/bin 或 ~/.claude/local，ttadk 会绕过垫片：把 ~/.ttadk/setting.json 的 ai_tool_commands 指向垫片",
                    "Installed in ~/.local/bin or ~/.claude/local, so ttadk bypasses the shim: point ai_tool_commands in ~/.ttadk/setting.json at the shim",
                ),
            });
            let path = r.real_path.as_ref().map_or_else(|| t("找不到", "not found").to_string(), |p| p.display().to_string());
            let shim = if r.shim_ok { t("垫片 ✓", "shim ✓") } else { t("垫片 ✗", "shim ✗") };
            let copy = copy_buttons.get(&i).map(|&b| {
                let kind = r.kind;
                let dir = shims_dir.clone();
                self.agent_button(b, t("复制配置", "Copy Config"), true, k, cx, move |view, cx| {
                    let cause = crate::agents::hints::Cause::Bypassed { parent: "ttadk".into() };
                    if let Some(text) = crate::agents::hints::hint(&cause, kind, dir.as_deref()).copy {
                        view.copy_text(text, cx);
                    }
                })
            });
            status = status.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(6.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .when(r.problem.is_some(), |d| d.bg(k.warn.0).text_color(k.warn.2))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(div().w(px(90.)).flex_none().font_family("Menlo").child(r.name.clone()))
                            .child(div().flex_1().font_family("Menlo").text_size(px(10.5)).child(path))
                            .child(div().flex_none().child(shim))
                            .children(copy),
                    )
                    .children(why.map(|w| div().text_size(px(10.5)).child(w))),
            );
        }
        out.push(status.into_any_element());

        let dir_text = self.agent.shims_dir.as_ref().map_or_else(|| t("（没有）", "(none)").to_string(), |d| d.display().to_string());
        out.push(
            section(k, t("垫片目录", "Shims folder"))
                .child(
                    div().flex().items_center().gap(px(8.)).child(div().flex_1().font_family("Menlo").text_size(px(10.5)).child(dir_text)).child(self.agent_button(
                        1,
                        t("复制", "Copy"),
                        self.agent.shims_dir.is_some(),
                        k,
                        cx,
                        |view, cx| {
                            if let Some(d) = view.agent.shims_dir.clone() {
                                view.copy_text(d.display().to_string(), cx);
                            }
                        },
                    )),
                )
                .into_any_element(),
        );
        out.push(
            section(k, t("提示", "Hints"))
                .child(div().flex().child(self.agent_button(2, t("恢复所有已关闭的提示", "Restore All Dismissed Hints"), can_restore, k, cx, |view, cx| view.restore_hints(cx))))
                .into_any_element(),
        );
        out.push(
            div()
                .text_size(px(10.5))
                .text_color(k.muted)
                .child(t(
                    "这些设置只影响在 gilvt 里运行的 agent，不会修改 ~/.claude、~/.codex 等全局配置。",
                    "These settings only affect agents run inside gilvt; ~/.claude, ~/.codex and other global config are never changed.",
                ))
                .into_any_element(),
        );
        out
    }

    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if e.keystroke.key == "escape" && (self.open_menu.is_some() || self.other.is_some() || self.agent_input.is_some()) {
            self.open_menu = None;
            self.close_other();
            self.agent_input = None;
            window.focus(&self.focus_handle);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn field_box(id: FieldId) -> gpui::Stateful<Div> {
        div().id(("settings-field", id.index())).relative().children(rects::recorder(RectId::SettingsField(id.index())))
    }

    fn switch(&self, f: &FieldModel, readonly: bool, k: &Colors, cx: &Context<Self>) -> AnyElement {
        let id = f.id;
        let on = f.value.as_bool().unwrap_or(false);
        Self::field_box(id)
            .flex_none()
            .w(px(30.))
            .h(px(17.))
            .rounded(px(9.))
            .bg(if on { k.accent } else { k.seg_on })
            .child(div().absolute().top(px(2.)).when(on, |d| d.right(px(2.))).when(!on, |d| d.left(px(2.))).size(px(13.)).rounded(px(7.)).bg(gpui::white()))
            .when(readonly, |d| d.opacity(0.5))
            .when(!readonly, |d| d.cursor_pointer().on_click(cx.listener(move |view, _: &ClickEvent, _, cx| view.toggle(id, cx))))
            .into_any_element()
    }

    fn segmented(&self, f: &FieldModel, readonly: bool, k: &Colors, cx: &Context<Self>) -> AnyElement {
        let id = f.id;
        let mut seg = segment_group(Self::field_box(id), k);
        for (j, o) in f.options.iter().enumerate() {
            let value = o.value.clone();
            seg = seg.child(
                div()
                    .id(("settings-option", id.index() * 1000 + j))
                    .relative()
                    .children(rects::recorder(RectId::SettingsOption(id.index(), j)))
                    .px(px(12.))
                    .py(px(2.))
                    .map(|d| segment_item(d, o.selected, !readonly, k))
                    .when(!readonly, |d| d.on_click(cx.listener(move |view, _: &ClickEvent, window, cx| view.pick(id, value.clone(), window, cx))))
                    .child(o.label.clone()),
            );
        }
        seg.when(readonly, |d| d.opacity(0.5)).into_any_element()
    }

    /// A dropdown (model fields), or the 「其他…」 text box while it is open for this field.
    fn dropdown(&self, f: &FieldModel, readonly: bool, k: &Colors, cx: &Context<Self>) -> AnyElement {
        let id = f.id;
        if let Some(o) = self.other.as_ref().filter(|o| o.field == id) {
            return div().relative().min_w(px(200.)).children(rects::recorder(RectId::SettingsOther)).child(o.input.clone()).into_any_element();
        }
        let (hover, hover_text) = (k.menu_hover, k.menu_hover_text);
        let menu = (self.open_menu == Some(id)).then(|| {
            let mut list = div()
                .id(("settings-menu", id.index()))
                .occlude()
                .min_w(px(200.))
                .py(px(4.))
                .rounded(px(6.))
                .border_1()
                .border_color(k.field_border)
                .bg(k.menu_bg)
                .shadow_lg();
            for (j, o) in f.options.iter().enumerate() {
                let value = o.value.clone();
                list = list.child(
                    div()
                        .id(("settings-option", id.index() * 1000 + j))
                        .relative()
                        .children(rects::recorder(RectId::SettingsOption(id.index(), j)))
                        .mx(px(4.))
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(4.))
                        .flex()
                        .gap(px(6.))
                        .hover(move |s| s.bg(hover).text_color(hover_text))
                        .child(div().w(px(10.)).child(if o.selected { "✓" } else { "" }))
                        .child(o.label.clone())
                        .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| view.pick(id, value.clone(), window, cx))),
                );
            }
            div().absolute().top(px(24.)).left_0().child(deferred(anchored().snap_to_window_with_margin(px(8.)).child(list)).with_priority(1))
        });
        Self::field_box(id)
            .min_w(px(180.))
            .px(px(7.))
            .py(px(3.))
            .rounded(px(5.))
            .border_1()
            .border_color(k.field_border)
            .bg(k.field)
            .flex()
            .justify_between()
            .gap(px(8.))
            .child(f.label.clone())
            .child(div().text_color(k.muted).child("▾"))
            .when(readonly, |d| d.opacity(0.5))
            .when(!readonly, |d| {
                // Mouse-down (not click) and stopped here: the page's own mouse-down closes menus.
                d.cursor_pointer().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _: &MouseDownEvent, _, cx| {
                        view.open_menu = if view.open_menu == Some(id) { None } else { Some(id) };
                        view.close_other();
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
            })
            .children(menu)
            .into_any_element()
    }

    fn command_box(&self, f: &FieldModel, readonly: bool, k: &Colors, cx: &Context<Self>) -> AnyElement {
        if let Some(o) = self.other.as_ref().filter(|o| o.field == FieldId::Command) {
            return div().relative().min_w(px(240.)).children(rects::recorder(RectId::SettingsOther)).child(o.input.clone()).into_any_element();
        }
        let empty = f.value.as_str().is_none_or(|s| s.trim().is_empty());
        Self::field_box(FieldId::Command)
            .min_w(px(240.))
            .px(px(7.))
            .py(px(3.))
            .rounded(px(5.))
            .border_1()
            .border_color(k.field_border)
            .bg(k.field)
            .when(empty, |d| d.text_color(k.muted))
            .child(f.label.clone())
            .when(readonly, |d| d.opacity(0.5))
            .when(!readonly, |d| d.cursor_text().on_click(cx.listener(|view, _: &ClickEvent, window, cx| view.start_input(FieldId::Command, window, cx))))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn button(&self, id: FieldId, label: &str, primary: bool, disabled: bool, k: &Colors, cx: &Context<Self>, on: fn(&mut Self, &mut Window, &mut Context<Self>)) -> AnyElement {
        Self::field_box(id)
            .flex_none()
            .px(px(10.))
            .py(px(3.))
            .rounded(px(5.))
            .bg(if primary { k.accent } else { k.seg_on })
            .when(primary, |d| d.text_color(k.on_accent))
            .child(label.to_string())
            .when(disabled, |d| d.opacity(0.5))
            .when(!disabled, |d| d.cursor_pointer().on_click(cx.listener(move |view, _: &ClickEvent, window, cx| on(view, window, cx))))
            .into_any_element()
    }

    fn excludes(&self, f: &FieldModel, readonly: bool, k: &Colors, cx: &Context<Self>) -> AnyElement {
        let id = f.id;
        let mut wrap = Self::field_box(id).flex().flex_wrap().gap(px(6.));
        for (j, o) in f.options.iter().enumerate() {
            let value = o.value.clone();
            wrap = wrap.child(
                div().flex().gap(px(4.)).px(px(6.)).rounded(px(4.)).bg(k.seg_on).font_family("Menlo").text_size(px(10.5)).child(o.label.clone()).child(
                    div()
                        .id(("settings-option", id.index() * 1000 + j))
                        .relative()
                        .children(rects::recorder(RectId::SettingsOption(id.index(), j)))
                        .child("×")
                        .when(!readonly, |d| d.cursor_pointer().on_click(cx.listener(move |view, _: &ClickEvent, window, cx| view.pick(id, value.clone(), window, cx)))),
                ),
            );
        }
        wrap.into_any_element()
    }

    fn test_box(&self, k: &Colors) -> Option<Div> {
        match &self.test {
            TestState::Idle => None,
            TestState::Running => Some(notice_box(
                k.warn,
                crate::i18n::text("正在测试…", "Testing…").into(),
            )),
            TestState::Done { ok, text } => {
                Some(notice_box(if *ok { k.ok } else { k.error }, text.clone()))
            }
        }
    }
}

/// Some agent hint was dismissed: 「恢复所有已关闭的提示」 has something to do.
fn can_restore(cx: &gpui::App) -> bool {
    cx.try_global::<crate::agents::AgentHints>().is_some_and(|h| !h.dismissed.is_empty())
}

fn section(k: &Colors, title: &str) -> Div {
    div().flex().flex_col().child(div().text_size(px(10.5)).text_color(k.muted).mb(px(6.)).child(title.to_string()))
}

fn row(k: &Colors, title: &str, control: impl IntoElement, hint: Option<String>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .py(px(6.))
        .border_b_1()
        .border_color(k.rule)
        .child(div().w(px(150.)).flex_none().child(title.to_string()))
        .child(div().flex_1().flex().flex_wrap().items_center().gap(px(8.)).child(control).children(hint.map(|h| div().text_size(px(10.5)).text_color(k.muted).child(h))))
}

/// A segmented control's frame (both pages build them the same way).
pub(super) fn segment_group<E: Styled>(el: E, k: &Colors) -> E {
    el.flex().border_1().border_color(k.field_border).rounded(px(5.)).overflow_hidden()
}

/// One item of a segmented control: the selected one is filled with the accent (it must stand out in light themes
/// too); an enabled unselected one shows the pointer and a hover fill.
pub(super) fn segment_item(d: gpui::Stateful<Div>, selected: bool, enabled: bool, k: &Colors) -> gpui::Stateful<Div> {
    d.whitespace_nowrap()
        .when(selected, |d| d.bg(k.accent).text_color(k.on_accent))
        .when(!selected && enabled, |d| d.cursor_pointer().hover(|s| s.bg(k.seg_on)))
}

/// A failed write-back: the short reason first (「config.toml 是只读的…」, not cut off behind a long path), the file
/// on its own line under it.
fn write_error_box(k: &Colors, error: &str, path: &std::path::Path) -> Div {
    let text = if crate::i18n::current() == Language::English {
        format!("{error} (the change is active for this run)")
    } else {
        format!("{error}（修改已在本次运行中生效）")
    };
    notice_box(k.error, text).flex().flex_col().child(
        div()
            .mt(px(2.))
            .text_size(px(10.5))
            .text_color(k.muted)
            .child(path.display().to_string()),
    )
}

/// (background, border, text).
fn notice_box(tone: (Hsla, Hsla, Hsla), text: String) -> Div {
    div().mt(px(8.)).px(px(9.)).py(px(6.)).rounded(px(6.)).border_1().border_color(tone.1).bg(tone.0).text_color(tone.2).text_size(px(11.)).child(text)
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        rects::begin_frame(window, cx);
        let theme = crate::theme::current(cx);
        let k = Colors::new(&theme);
        let settings = cx.global::<AppSettings>().0.clone();
        let language = settings.language;
        let t = |zh, en| language.text(zh, en);
        let m = settings.monitor;
        let file = cx.global::<ConfigFile>();
        let (file_error, write_error, path) = (file.error.clone(), file.write_error.clone(), file.path.clone());
        let ro = file_error.is_some();
        let fields = form::fields(&m, &self.codex);
        let f = |id: FieldId| fields[id.index()].clone();
        let testing = self.test == TestState::Running;
        let model_row = |view: &Self, id: FieldId, cx: &Context<Self>| {
            let field = f(id);
            let refresh =
                (id == FieldId::Model && m.provider == MonitorProvider::Codex).then(|| {
                    view.button(
                        FieldId::RefreshModels,
                        t("↻ 刷新", "↻ Refresh"),
                        false,
                        ro,
                        &k,
                        cx,
                        Self::refresh_models,
                    )
                });
            (
                div()
                    .flex()
                    .gap(px(8.))
                    .child(view.dropdown(&field, ro, &k, cx))
                    .children(refresh),
                field.hint,
            )
        };
        let (chat_model, chat_hint) = model_row(self, FieldId::Model, cx);
        let (summary_model, summary_hint) = model_row(self, FieldId::SummaryModel, cx);
        let readonly_banner = file_error.map(|e| {
            let text = if language == Language::English {
                format!("{e}. Settings cannot be changed until this is fixed.")
            } else {
                format!("{e}。修好之前这里的设置不能修改。")
            };
            notice_box(k.warn, text)
                .flex()
                .gap(px(6.))
                .child(self.button(
                    FieldId::OpenConfig,
                    t("在编辑器中打开", "Open in Editor"),
                    false,
                    false,
                    &k,
                    cx,
                    Self::open_config,
                ))
        });
        let page = div()
            .id("settings-page")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .px(px(18.))
            .py(px(14.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .children(readonly_banner)
            .children(
                write_error
                    .as_deref()
                    .map(|e| write_error_box(&k, e, &path)),
            );
        let page = match self.page {
            Page::Appearance => page.child(self.appearance.clone()),
            Page::Agent => page.children(self.agent_page(ro, &k, cx)),
            Page::Language => page
                .child(self.language_page(language, ro, &k, cx))
                .children(self.notice.clone().map(|n| notice_box(if n.error { k.error } else { k.warn }, n.text))),
            Page::Monitor => page.child(section(&k, t("总开关", "General")).child(row(&k, t("启用监控官", "Enable Monitor"), self.switch(&f(FieldId::Enabled), ro, &k, cx), Some(t("关闭后只剩不调用模型的卡片墙，不会向外发送任何数据", "When off, the activity view remains available but no data is sent to a model.").into()))))
            .child(
                section(&k, t("模型", "Model"))
                    .child(row(&k, t("通道", "Provider"), self.segmented(&f(FieldId::Provider), ro, &k, cx), None))
                    .child(row(&k, t("对话模型", "Chat model"), chat_model, chat_hint))
                    .child(row(&k, t("总结模型", "Summary model"), summary_model, summary_hint))
                    .child(row(
                        &k,
                        t("自定义 CLI 路径", "Custom CLI path"),
                        div().flex().gap(px(8.)).child(self.command_box(&f(FieldId::Command), ro, &k, cx)).child(self.button(FieldId::ChooseCommand, t("选择…", "Choose…"), false, ro, &k, cx, Self::choose_command)),
                        None,
                    ))
                    .child(row(&k, "", self.button(FieldId::Test, t("测试连接", "Test Connection"), true, ro || testing, &k, cx, Self::run_test), Some(t("用当前配置试跑一次总结和一轮对话（对话需开启监控官）", "Run one summary and one chat turn with the current settings (chat requires Monitor to be enabled).").into())))
                    .children(self.test_box(&k))
                    .children(self.notice.clone().map(|n| notice_box(if n.error { k.error } else { k.warn }, n.text))),
            )
            .child(
                section(&k, t("✦ AI 总结", "✦ AI Summaries"))
                    .child(row(&k, t("自动刷新", "Automatic refresh"), self.switch(&f(FieldId::AutoSummary), ro, &k, cx), Some(t("关闭后只在你点「✦ 重新总结」时生成", "When off, summaries are generated only when you click \"✦ Summarize Again\".").into())))
                    .child(row(&k, t("最小间隔", "Minimum interval"), self.segmented(&f(FieldId::SummaryInterval), ro, &k, cx), Some(t("同一会话两次自动总结之间至少隔这么久", "Minimum time between automatic summaries for the same session.").into())))
                    .child(row(&k, t("左栏显示摘要行", "Show summaries in sidebar"), self.switch(&f(FieldId::SidebarSummary), ro, &k, cx), None)),
            )
            .child(
                section(&k, t("隐私", "Privacy"))
                    .child(row(
                        &k,
                        t("排除目录", "Excluded folders"),
                        div().flex().flex_wrap().gap(px(6.)).child(self.excludes(&f(FieldId::ExcludePaths), ro, &k, cx)).child(self.button(FieldId::AddExclude, t("＋ 添加…", "+ Add…"), false, ro, &k, cx, Self::add_exclude)),
                        None,
                    ))
                    .child(div().mt(px(6.)).text_size(px(10.5)).text_color(k.muted).child(t("这些目录下的会话和终端不会送给模型：卡片照常显示，但没有 ✦ 块。", "Sessions and terminals in these folders are not sent to the model. Cards still appear, without ✦ blocks."))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(10.5))
                    .text_color(k.muted)
                    .child(if language == Language::English {
                        format!("Writes the [monitor] table in {}, preserving comments and formatting", path.display())
                    } else {
                        format!("写入 {} 的 [monitor] 表，保留你的注释和格式", path.display())
                    })
                    .child(self.button(FieldId::OpenConfig, t("在编辑器中打开", "Open in Editor"), false, false, &k, cx, Self::open_config)),
            )
        };
        div()
            .id("settings-root")
            .track_focus(&self.focus_handle)
            .key_context("SettingsWindow")
            .on_key_down(cx.listener(Self::on_key))
            .on_action(cx.listener(|_, _: &ClosePane, window, _| window.remove_window()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _: &MouseDownEvent, _, cx| {
                    if view.open_menu.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .size_full()
            .flex()
            .bg(k.bg)
            .text_color(k.text)
            .text_size(px(12.))
            .child(self.nav(&k, cx))
            .child(page)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gilvt_monitor::provider::models::ModelChoice;
    use gilvt_monitor::provider::ProviderError;
    use gpui::{TestAppContext, WindowHandle};

    use super::*;
    use crate::settings::{MonitorSettings, Settings};

    fn open(cx: &mut TestAppContext, path: PathBuf, error: Option<String>) -> WindowHandle<SettingsWindow> {
        cx.update(|cx| {
            cx.set_global(AppSettings(Settings::default()));
            crate::theme::init_for_tests(cx);
            let mut file = ConfigFile::unwatched(path);
            file.error = error;
            cx.set_global(file);
        });
        cx.add_window(SettingsWindow::new)
    }

    fn monitor(cx: &mut TestAppContext) -> MonitorSettings {
        cx.update(|cx| cx.global::<AppSettings>().0.monitor.clone())
    }

    #[test]
    fn the_chat_test_needs_the_monitor_on() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("gilvt"), "").unwrap();
        let env = crate::launch::ShellEnv { integration: None, socket: Some(dir.path().join("sock")), bin_dir: Some(dir.path().to_path_buf()), user: None, shims_dir: None, shims_error: None };
        assert_eq!(chat_launch(true, Some(&env)), Ok((dir.path().join("gilvt"), dir.path().join("sock"))));
        // Off: no token is issued (the switch promises no data leaves), so the chat half is not tried.
        assert_eq!(chat_launch(false, Some(&env)), Err("监控官未开启".to_string()));
        let (ok, text) = form::test_line("claude", &Ok("2.1.291".into()), &Ok(std::time::Duration::from_secs(3)), &probe::ChatTest::Skipped("监控官未开启".into()));
        assert!(ok);
        assert!(text.ends_with("· 对话未测试（监控官未开启）"), "{text}");
        assert_eq!(chat_launch(true, None), Err("找不到 gilvt 的启动环境".to_string()));
        let no_cli = crate::launch::ShellEnv { bin_dir: Some(dir.path().join("nowhere")), ..env.clone() };
        assert_eq!(chat_launch(true, Some(&no_cli)), Err("找不到 gilvt 命令行".to_string()));
        let no_socket = crate::launch::ShellEnv { socket: None, ..env };
        assert_eq!(chat_launch(true, Some(&no_socket)), Err("gilvt 的本地通信没有启动".to_string()));
    }

    #[gpui::test]
    fn debug_state_reads_the_live_window_through_a_shared_borrow(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let w = open(cx, dir.path().join("config.toml"), None);
        w.update(cx, |view, _, cx| {
            view.notice = Some(Notice { error: true, text: "x".into() });
            view.toggle(FieldId::Enabled, cx);
        })
        .unwrap();
        let s = cx.update(|cx| w.read(cx).unwrap().debug_state(Some(7), true, 0.0, &Default::default(), cx));
        assert_eq!((s.id, s.key, s.readonly), (Some(7), true, false));
        assert_eq!(s.config_path, dir.path().join("config.toml").display().to_string());
        assert_eq!(s.fields.iter().find(|f| f.id == "enabled").unwrap().value, serde_json::json!(true));
        assert_eq!((s.notice.as_deref(), s.notice_error), (None, false), "the switch cleared the notice");
    }

    #[gpui::test]
    fn adding_a_command_writes_the_file_and_rebuilds_nothing_else(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let w = open(cx, path.clone(), None);
        w.update(cx, |view, window, cx| {
            view.show_page(Page::Agent, window, cx);
            view.add_agent_command(gilvt_agent::AgentKind::Codex, "cw".into(), cx);
        })
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("codex_commands = [\"codex\", \"cw\"]"), "{text}");
        w.update(cx, |view, _, cx| view.add_agent_command(gilvt_agent::AgentKind::Codex, "bad name".into(), cx)).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("bad name"));
    }

    #[gpui::test]
    fn the_agent_page_removes_names_saves_launch_commands_and_refuses_bad_ones(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let w = open(cx, path.clone(), None);
        let agent = |cx: &mut TestAppContext| cx.update(|cx| cx.global::<AppSettings>().0.agent.clone());
        w.update(cx, |view, window, cx| {
            view.show_page(Page::Agent, window, cx);
            view.add_agent_command(gilvt_agent::AgentKind::Claude, "cc".into(), cx);
            // A name already in the other list is refused.
            view.add_agent_command(gilvt_agent::AgentKind::Codex, "cc".into(), cx);
            assert!(view.notice.as_ref().is_some_and(|n| n.error), "{:?}", view.notice);
            view.remove_agent_command(gilvt_agent::AgentKind::Claude, 0, cx);
        })
        .unwrap();
        assert_eq!(agent(cx).claude_commands, vec!["cc"]);
        assert_eq!(agent(cx).codex_commands, vec!["codex"]);
        w.update(cx, |view, window, cx| {
            view.start_agent_input(AgentField::Launch(gilvt_agent::AgentKind::Claude), window, cx);
            view.save_launch(gilvt_agent::AgentKind::Claude, "ttadk claude".into(), window, cx);
            view.start_agent_input(AgentField::Launch(gilvt_agent::AgentKind::Codex), window, cx);
            view.save_launch(gilvt_agent::AgentKind::Codex, "'bad name' x".into(), window, cx);
            assert!(view.agent.launch_error[1].is_some(), "refused, shown under the box");
            assert!(view.agent_input.is_some(), "the box stays open for a fix");
        })
        .unwrap();
        assert_eq!((agent(cx).claude_launch.as_str(), agent(cx).codex_launch.as_str()), ("ttadk claude", "codex"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("claude_launch = \"ttadk claude\"") && text.contains("claude_commands = [\"cc\"]"), "{text}");
        let s = cx.update(|cx| w.read(cx).unwrap().debug_state(None, true, 0.0, &Default::default(), cx));
        let a = s.agent.unwrap();
        assert_eq!(a.launch.claude.preview[1], "ttadk claude --resume <会话 ID>");
        assert_eq!(a.status.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["cc", "codex"], "status follows the lists");
    }

    fn in_use(cx: &mut TestAppContext) -> gilvt_theme::Selection {
        cx.update(|cx| cx.global::<crate::theme::ThemeState>().selection().clone())
    }

    #[gpui::test]
    fn the_appearance_page_applies_a_choice_at_once(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let w = open(cx, path.clone(), None);
        let s = cx.update(|cx| w.read(cx).unwrap().debug_state(None, true, 0.0, &Default::default(), cx));
        assert_eq!((s.page, s.appearance.mode), ("appearance", "system"), "外观 first; the default theme follows the system");
        let page = w.read_with(cx, |view, _| view.appearance.clone()).unwrap();
        w.update(cx, |_, window, cx| {
            page.update(cx, |page, cx| {
                page.model_mut().set_mode(super::super::appearance_model::Mode::Fixed);
                page.model_mut().set_query("nord".into());
                let ix = (0..page.model().row_count()).find(|&i| page.model().row(i).unwrap().name == "Nord").unwrap();
                page.choose_for_test(ix, window, cx);
            })
        })
        .unwrap();
        assert_eq!(in_use(cx), gilvt_theme::Selection::Fixed("Nord".into()), "in use before anything is written");
        assert!(!path.exists());
        cx.executor().advance_clock(config_file::THEME_WRITE_DELAY);
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "theme = \"Nord\"\n");
        let s = cx.update(|cx| w.read(cx).unwrap().debug_state(None, true, 0.0, &Default::default(), cx));
        assert_eq!((s.appearance.query.as_str(), s.appearance.fixed.as_deref(), s.appearance.selected.as_deref()), ("nord", Some("Nord"), Some("Nord")));
        assert!(s.appearance.rows.iter().any(|r| r.name == "Nord" && r.current && r.selected));
    }

    #[gpui::test]
    fn a_broken_file_refuses_a_theme_and_the_page_shows_the_one_in_use(cx: &mut TestAppContext) {
        let w = open(cx, "/nonexistent/gilvt/config.toml".into(), Some("config.toml 第 3 行：语法错误".into()));
        let page = w.read_with(cx, |view, _| view.appearance.clone()).unwrap();
        w.update(cx, |_, window, cx| {
            page.update(cx, |page, cx| {
                page.model_mut().set_mode(super::super::appearance_model::Mode::Fixed);
                page.choose_for_test(1, window, cx);
            })
        })
        .unwrap();
        assert_eq!(in_use(cx), gilvt_theme::Selection::system(), "unchanged");
        let s = cx.update(|cx| w.read(cx).unwrap().debug_state(None, true, 0.0, &Default::default(), cx));
        assert_eq!(s.appearance.mode, "system", "the page went back to the theme in use");
    }

    #[gpui::test]
    fn a_switch_writes_memory_and_the_file(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let w = open(cx, path.clone(), None);
        w.update(cx, |view, _, cx| view.toggle(FieldId::Enabled, cx)).unwrap();
        assert!(monitor(cx).enabled);
        assert!(std::fs::read_to_string(&path).unwrap().contains("enabled = true"));
        w.update(cx, |view, _, _| assert_eq!(view.notice, None)).unwrap();
    }

    #[gpui::test]
    fn a_broken_file_refuses_changes(cx: &mut TestAppContext) {
        let w = open(cx, "/nonexistent/gilvt/config.toml".into(), Some("config.toml 第 3 行：语法错误".into()));
        w.update(cx, |view, _, cx| view.toggle(FieldId::Enabled, cx)).unwrap();
        assert!(!monitor(cx).enabled, "unchanged in memory");
        w.update(cx, |view, _, _| assert!(view.notice.as_ref().is_some_and(|n| n.error))).unwrap();
    }

    #[gpui::test]
    fn other_opens_the_box_and_a_good_trial_writes(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let w = open(cx, dir.path().join("config.toml"), None);
        let key = ProbeKey::trial(&monitor(cx), FieldId::Model);
        w.update(cx, |view, window, cx| {
            view.pick(FieldId::Model, OptValue::Other, window, cx);
            assert_eq!(view.other.as_ref().map(|o| o.field), Some(FieldId::Model));
            view.other.as_mut().unwrap().trying = true;
            view.trial_arrived(FieldId::Model, "gpt-x".into(), key, "claude".into(), Some(Ok(())), window, cx);
            assert!(view.other.is_none());
            assert!(view.focus_handle.is_focused(window), "the page has the keyboard again (Esc, ⌘W)");
        })
        .unwrap();
        assert_eq!(monitor(cx).model, "gpt-x");
    }

    #[gpui::test]
    fn a_trial_answer_for_other_settings_is_dropped(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let w = open(cx, dir.path().join("config.toml"), None);
        let key = ProbeKey::trial(&monitor(cx), FieldId::Model);
        w.update(cx, |view, window, cx| {
            view.start_input(FieldId::Model, window, cx);
            view.other.as_mut().unwrap().trying = true;
            // The CLI path changed while the trial ran.
            view.apply(form::edits_for_command("/opt/claude-w", &cx.global::<AppSettings>().0.monitor), cx);
            view.trial_arrived(FieldId::Model, "gpt-x".into(), key, "claude".into(), Some(Ok(())), window, cx);
            assert!(view.other.is_none());
            assert!(view.focus_handle.is_focused(window));
            assert!(view.notice.as_ref().is_some_and(|n| n.error && n.text.contains("作废")), "{:?}", view.notice);
        })
        .unwrap();
        assert_eq!(monitor(cx).model, "", "not written");
    }

    #[gpui::test]
    fn cancelling_the_box_abandons_its_trial(cx: &mut TestAppContext) {
        let w = open(cx, "/nonexistent/gilvt/config.toml".into(), None);
        w.update(cx, |view, window, cx| {
            view.start_input(FieldId::Model, window, cx);
            view.other.as_mut().unwrap().trying = true;
            view.trial_task = Some(Task::ready(()));
            view.notice = Some(Notice { error: false, text: "正在用 gpt-x 试跑一次总结…".into() });
            let input = view.other.as_ref().unwrap().input.clone();
            input.update(cx, |_, cx| cx.emit(RenameEvent::Cancel));
        })
        .unwrap();
        w.update(cx, |view, _, _| {
            assert!(view.other.is_none());
            assert!(view.trial_task.is_none(), "a late answer must not write a name the user walked away from");
            assert_eq!(view.notice, None);
        })
        .unwrap();
    }

    #[gpui::test]
    fn a_trial_does_not_touch_the_path_box_opened_after_it(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let w = open(cx, dir.path().join("config.toml"), None);
        let key = ProbeKey::trial(&monitor(cx), FieldId::Model);
        w.update(cx, |view, window, cx| {
            view.start_input(FieldId::Model, window, cx);
            view.other.as_mut().unwrap().trying = true;
            view.trial_task = Some(Task::ready(()));
            view.start_input(FieldId::Command, window, cx);
            assert!(view.trial_task.is_none(), "opening another box abandons the trial");
            // Its answer arrives anyway (it was already on its way).
            view.trial_arrived(FieldId::Model, "gpt-x".into(), key.clone(), "claude".into(), Some(Ok(())), window, cx);
            assert_eq!(view.other.as_ref().map(|o| (o.field, o.trying)), Some((FieldId::Command, false)), "the path box stays open");
            view.trial_arrived(FieldId::Model, "gpt-x".into(), key, "claude".into(), Some(Err(ProviderError::Timeout)), window, cx);
            assert_eq!(view.other.as_ref().map(|o| (o.field, o.trying)), Some((FieldId::Command, false)));
            assert_eq!(view.notice, None);
        })
        .unwrap();
        assert_eq!(monitor(cx).model, "", "nothing written");
    }

    #[gpui::test]
    fn a_lost_probe_is_a_failure_not_a_hang(cx: &mut TestAppContext) {
        let w = open(cx, "/nonexistent/gilvt/config.toml".into(), None);
        let m = monitor(cx);
        w.update(cx, |view, window, cx| {
            view.start_input(FieldId::Model, window, cx);
            view.other.as_mut().unwrap().trying = true;
            view.trial_arrived(FieldId::Model, "gpt-x".into(), ProbeKey::trial(&m, FieldId::Model), "claude".into(), None, window, cx);
            assert!(view.other.as_ref().is_some_and(|o| !o.trying), "the box stays for another try");
            assert_eq!(view.notice, Some(Notice { error: true, text: PROBE_LOST.into() }));

            view.test = TestState::Running;
            view.test_arrived(ProbeKey::test(&m), "claude".into(), None, cx);
            assert_eq!(view.test, TestState::Done { ok: false, text: format!("✗ {PROBE_LOST}") });

            let key = (MonitorProvider::Codex, String::new());
            view.models_for = Some(key.clone());
            view.codex = CodexModels::Loading;
            view.models_arrived(key, "codex".into(), None, cx);
            assert_eq!(view.codex, CodexModels::Failed(PROBE_LOST.into()));
        })
        .unwrap();
    }

    #[gpui::test]
    fn late_answers_for_other_settings_are_ignored(cx: &mut TestAppContext) {
        let w = open(cx, "/nonexistent/gilvt/config.toml".into(), None);
        let m = monitor(cx);
        w.update(cx, |view, _, cx| {
            let list = vec![ModelChoice { id: "gpt-y".into(), display: "Y".into(), is_default: true }];
            view.models_for = Some((MonitorProvider::Codex, "/opt/codex-w".into()));
            view.codex = CodexModels::Loading;
            view.models_arrived((MonitorProvider::Codex, String::new()), "codex".into(), Some(Ok(list.clone())), cx);
            assert_eq!(view.codex, CodexModels::Loading, "read for another command");
            view.models_arrived((MonitorProvider::Codex, "/opt/codex-w".into()), "codex".into(), Some(Ok(list.clone())), cx);
            assert_eq!(view.codex, CodexModels::Ready(list));

            let mut other = m.clone();
            other.summary_model = "haiku".into();
            view.test = TestState::Running;
            let r = probe::TestResult { version: Ok("1.0.0".into()), summary: Err(ProviderError::Timeout), chat: probe::ChatTest::Skipped("x".into()) };
            view.test_arrived(ProbeKey::test(&other), "claude".into(), Some(r), cx);
            assert!(matches!(&view.test, TestState::Done { ok: false, text } if text.contains("已改变")), "{:?}", view.test);
        })
        .unwrap();
    }
}
