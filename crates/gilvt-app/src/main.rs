mod actions;
mod config_file;
mod agents;
mod debug_state;
mod drop;
mod editor;
mod external_navigation;
mod finder;
mod i18n;
mod inspector;
mod install_notice;
mod ipc_bridge;
mod launch;
mod launcher;
mod live_preview;
mod markdown;
mod monitor;
mod native;
mod notify;
mod pane_tree;
mod persist;
mod preview_element;
mod preview_select;
mod preview_view;
mod remote;
pub mod review;
mod session_center;
mod session_review;
mod settings;
mod settings_window;
mod sidebar;
mod terminal_element;
mod terminal_view;
mod theme;
mod updater;
mod workspace;

use gilvt_mermaid::Cache;
use gpui::{px, size, App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions};

use crate::actions::{CheckForUpdates, NewWindow, Quit};
use crate::finder::Listings;
use crate::launch::ShellEnv;
use crate::markdown::mermaid::Mermaid;
use crate::persist::snapshot::WindowSnap;
use crate::settings::Settings;
use crate::sidebar::UiPrefs;
use crate::theme::AppSettings;
use crate::workspace::Workspace;

fn open_window(error: Option<String>, snap: Option<&WindowSnap>, cx: &mut App) {
    // The inspector column (when shown) comes on top of the terminal's usual width.
    let prefs = cx.try_global::<UiPrefs>().cloned().unwrap_or_default();
    let inspector = if prefs.inspector_hidden { 0. } else { prefs.inspector_px() };
    let bounds = match snap.and_then(|s| s.frame) {
        Some(f) => persist::clamp_frame(f, cx),
        None => Bounds::centered(None, size(px(960. + inspector), px(620.)), cx),
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some("gilvt".into()), ..Default::default() }),
        ..Default::default()
    };
    let snap = snap.cloned();
    let result = cx.open_window(options, move |window, cx| {
        cx.new(|cx| match &snap {
            Some(s) => Workspace::restore(error, s, window, cx),
            None => Workspace::new(error, window, cx),
        })
    });
    if let Err(e) = result {
        eprintln!("gilvt: failed to open window: {e}");
    }
}

fn main() {
    launch::scrub_inherited_agent_env();
    // Read once, then removed from the environment: panes never see it.
    debug_state::init_from_env();
    let (settings, error) = Settings::load(&Settings::default_path());
    Application::new().run(move |cx: &mut App| {
        let language = settings.language;
        i18n::set_current(language);
        let state_dir = agents::state_dir();
        cx.set_global(state_dir.as_deref().map(UiPrefs::load).unwrap_or_default());
        notify::init(cx);
        let hooks = agents::init(cx);
        review::ReviewService::init(state_dir.clone(), cx);
        launcher::History::init(state_dir, cx);
        remote::init(cx);
        let socket = ipc_bridge::start(hooks, cx);
        cx.background_executor().spawn(async { gilvt_viewer::highlight::preload() }).detach();
        cx.background_executor().spawn(async { markdown::mermaid::preload() }).detach();
        let theme_state = theme::ThemeState::new(settings.theme.selection(), settings.colors.overrides(), Some(Settings::themes_dir()));
        cx.set_global(ShellEnv::detect(&settings, socket));
        cx.set_global(AppSettings(settings));
        cx.set_global(theme_state);
        // A missing or invalid theme is reported with the config's own errors.
        let error = error.into_iter().chain(cx.global::<theme::ThemeState>().errors()).collect::<Vec<_>>();
        let error = if error.is_empty() { None } else { Some(error.join(i18n::text("；", "; "))) };
        if let Some(e) = &error {
            eprintln!("gilvt: {e}");
        }
        config_file::init(Settings::default_path(), cx);
        monitor::summaries::init(agents::state_dir(), cx);
        monitor::chat::init(cx);
        cx.set_global(Mermaid::new(Cache::default_dir()));
        cx.set_global(Listings::default());
        actions::bind_keys(cx);
        cx.set_menus(actions::menus(language));
        // Deferred: during dispatch the active window is leased and could not be asked about its agents.
        cx.on_action(|_: &Quit, cx| cx.defer(workspace::quit_requested));
        cx.on_action(|_: &NewWindow, cx| open_window(None, None, cx));
        settings_window::init(cx);
        install_notice::init(cx);
        updater::init(cx);
        cx.on_action(|_: &CheckForUpdates, cx| cx.defer(updater::check_now));
        let mut quitting = false;
        cx.on_window_closed(move |cx| {
            debug_state::rects::forget_closed_windows(cx);
            let workspaces = workspace::workspaces(cx).len();
            if settings_window::window_closed(workspaces, &mut quitting, cx) {
                persist::cleared(cx);
                cx.quit();
            }
        })
        .detach();
        let state = agents::state_dir();
        let restored = state.as_deref().and_then(persist::startup_snapshot);
        match &restored {
            Some(s) => {
                crate::pane_tree::seed_pane_ids(s.max_pane_id());
                let mut error = error;
                for w in &s.windows {
                    open_window(error.take(), Some(w), cx);
                }
            }
            None => open_window(error, None, cx),
        }
        persist::start(state, restored, cx);
        cx.activate(true);
    });
}
