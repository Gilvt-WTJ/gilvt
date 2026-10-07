//! The `⌘,` settings window (S2 §5.2): one per app, with the 「外观」 page (themes) and the 「◎ 监控官」 page.

pub mod appearance;
pub mod appearance_model;
pub mod debug;
pub mod form;

pub mod probe;
mod view;

use gpui::{px, size, App, AppContext, Bounds, Global, TitlebarOptions, WindowBounds, WindowHandle, WindowOptions};

use crate::actions::OpenSettings;

pub use view::SettingsWindow;

pub const WIDTH: f32 = 720.0;
pub const HEIGHT: f32 = 560.0;

#[derive(Default)]
struct Open(Option<WindowHandle<SettingsWindow>>);

impl Global for Open {}

/// A page of the window, in nav order: 「外观」 comes first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Appearance,
    Monitor,
}

impl Page {
    pub const ALL: [Page; 2] = [Page::Appearance, Page::Monitor];

    pub fn index(self) -> usize {
        Page::ALL.iter().position(|p| *p == self).expect("in ALL")
    }

    /// DebugState `settings.page`.
    pub fn id(self) -> &'static str {
        match self {
            Page::Appearance => "appearance",
            Page::Monitor => "monitor",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Page::Appearance => "◐ 外观",
            Page::Monitor => "◎ 监控官",
        }
    }
}

/// The page last shown (this run): the window opens on it again; 「外观」 the first time.
#[derive(Default)]
pub struct LastPage(pub Page);

impl Global for LastPage {}

pub fn last_page(cx: &App) -> Page {
    cx.try_global::<LastPage>().map(|p| p.0).unwrap_or_default()
}

pub fn handle(cx: &App) -> Option<WindowHandle<SettingsWindow>> {
    cx.try_global::<Open>().and_then(|o| o.0)
}

/// `⌘,`: brings the window to the front when it is open, else opens it. Must run outside action dispatch (the
/// dispatching window is leased then and cannot be updated): [`init`] defers into it.
pub fn open(cx: &mut App) {
    if let Some(h) = handle(cx) {
        if cx.windows().iter().any(|w| w.window_id() == h.window_id()) {
            let _ = h.update(cx, |_, window, _| window.activate_window());
            cx.activate(true);
            return;
        }
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx))),
        titlebar: Some(TitlebarOptions { title: Some("设置".into()), ..Default::default() }),
        window_min_size: Some(size(px(560.), px(420.))),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        // Its titlebar follows the theme like the workspaces' (refresh_all keeps it in step afterwards).
        crate::native::apply_appearance(window, cx);
        cx.new(|cx| SettingsWindow::new(window, cx))
    }) {
        Ok(h) => {
            cx.set_global(Open(Some(h)));
            cx.activate(true);
        }
        Err(e) => eprintln!("gilvt: failed to open the settings window: {e}"),
    }
}

/// Closes the window if it is open (gilvt is quitting).
pub fn close(cx: &mut App) {
    if let Some(h) = handle(cx) {
        let _ = h.update(cx, |_, window, _| window.remove_window());
    }
    cx.set_global(Open(None));
}

/// A window closed: forget the handle when it was the settings window.
pub fn forget_closed(cx: &mut App) {
    let Some(h) = handle(cx) else { return };
    if !cx.windows().iter().any(|w| w.window_id() == h.window_id()) {
        cx.set_global(Open(None));
    }
}

/// After a window closed: quit when no workspace window is left (the settings window alone does not keep gilvt).
pub fn should_quit(workspace_windows: usize) -> bool {
    workspace_windows == 0
}

/// Registers `⌘,` / 「设置…」.
pub fn init(cx: &mut App) {
    // Deferred like ⌘Q: during dispatch the active window (maybe this one) is leased.
    cx.on_action(|_: &OpenSettings, cx| cx.defer(open));
}

/// A window closed (it is gone from `cx.windows()` now): forgets a closed settings window; true, once, when gilvt
/// should quit — no workspace window is left (`workspace_windows`); the settings window is closed first then.
pub fn window_closed(workspace_windows: usize, quitting: &mut bool, cx: &mut App) -> bool {
    forget_closed(cx);
    if *quitting || !should_quit(workspace_windows) {
        return false;
    }
    *quitting = true;
    close(cx);
    true
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{AnyWindowHandle, Empty, IntoElement, Render, TestAppContext};

    use super::*;
    use crate::config_file::ConfigFile;
    use crate::settings::Settings;
    use crate::theme::AppSettings;

    #[test]
    fn quits_when_no_workspace_is_left() {
        assert!(should_quit(0), "the settings window alone does not keep gilvt running");
        assert!(!should_quit(1));
    }

    /// Stands in for a workspace window.
    struct Other;

    impl Render for Other {
        fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            Empty
        }
    }

    fn setup(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(AppSettings(Settings::default()));
            crate::theme::init_for_tests(cx);
            cx.set_global(ConfigFile::unwatched("/nonexistent/gilvt/config.toml".into()));
            init(cx);
        });
    }

    fn settings_windows(cx: &mut TestAppContext) -> Vec<AnyWindowHandle> {
        cx.update(|cx| cx.windows().into_iter().filter(|w| w.downcast::<SettingsWindow>().is_some()).collect())
    }

    fn open_other(cx: &mut TestAppContext) -> AnyWindowHandle {
        cx.update(|cx| cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| Other)).unwrap().into())
    }

    #[gpui::test]
    fn open_twice_keeps_one_window(cx: &mut TestAppContext) {
        setup(cx);
        cx.update(open);
        cx.run_until_parked();
        cx.update(open);
        cx.run_until_parked();
        let windows = settings_windows(cx);
        assert_eq!(windows.len(), 1);
        assert_eq!(cx.update(|cx| handle(cx).map(|h| h.window_id())), Some(windows[0].window_id()));
    }

    #[gpui::test]
    fn open_from_inside_the_settings_window_keeps_one_window(cx: &mut TestAppContext) {
        setup(cx);
        let other = open_other(cx);
        cx.dispatch_action(other, OpenSettings);
        let first = settings_windows(cx);
        assert_eq!(first.len(), 1);
        // ⌘, while the settings window is the one dispatching (it is leased then).
        cx.dispatch_action(first[0], OpenSettings);
        let ids: Vec<_> = settings_windows(cx).iter().map(|w| w.window_id()).collect();
        assert_eq!(ids, vec![first[0].window_id()]);
        assert_eq!(cx.update(|cx| handle(cx).map(|h| h.window_id())), Some(first[0].window_id()));
    }

    #[gpui::test]
    fn opens_on_appearance_then_on_the_last_page_shown(cx: &mut TestAppContext) {
        setup(cx);
        cx.update(open);
        cx.run_until_parked();
        let w = cx.update(|cx| handle(cx)).unwrap();
        assert_eq!(w.read_with(cx, |view, _| view.page()).unwrap(), Page::Appearance, "the first time: 外观");
        w.update(cx, |view, window, cx| view.show_page(Page::Monitor, window, cx)).unwrap();
        w.update(cx, |_, window, _| window.remove_window()).unwrap();
        cx.run_until_parked();
        cx.update(|cx| forget_closed(cx));
        cx.update(open);
        cx.run_until_parked();
        let w = cx.update(|cx| handle(cx)).unwrap();
        assert_eq!(w.read_with(cx, |view, _| view.page()).unwrap(), Page::Monitor, "the page shown last");
    }

    #[gpui::test]
    fn close_then_reopen_gives_a_fresh_window(cx: &mut TestAppContext) {
        setup(cx);
        let quits = Rc::new(Cell::new(0));
        let q = quits.clone();
        let mut quitting = false;
        // One window stays open the whole time: a workspace.
        cx.update(|cx| {
            cx.on_window_closed(move |cx| {
                if window_closed(1, &mut quitting, cx) {
                    q.set(q.get() + 1);
                }
            })
            .detach()
        });
        cx.update(open);
        cx.run_until_parked();
        let first = settings_windows(cx);
        first[0].update(cx, |_, window, _| window.remove_window()).unwrap();
        cx.run_until_parked();
        assert!(settings_windows(cx).is_empty());
        assert!(cx.update(|cx| handle(cx)).is_none(), "the closed window is forgotten");
        cx.update(open);
        cx.run_until_parked();
        let second = settings_windows(cx);
        assert_eq!(second.len(), 1);
        assert_ne!(second[0].window_id(), first[0].window_id());
        assert_eq!(quits.get(), 0, "closing the settings window with a workspace open does not quit");
    }

    #[gpui::test]
    fn the_last_workspace_closing_closes_the_settings_window_and_quits_once(cx: &mut TestAppContext) {
        setup(cx);
        let quits = Rc::new(Cell::new(0));
        let q = quits.clone();
        let mut quitting = false;
        let workspace = open_other(cx);
        cx.update(|cx| {
            cx.on_window_closed(move |cx| {
                // Only `Other` windows count, as only `Workspace` windows do in gilvt.
                let workspaces = cx.windows().iter().filter(|w| w.downcast::<Other>().is_some()).count();
                if window_closed(workspaces, &mut quitting, cx) {
                    q.set(q.get() + 1);
                }
            })
            .detach()
        });
        cx.update(open);
        cx.run_until_parked();
        assert_eq!(settings_windows(cx).len(), 1);
        workspace.update(cx, |_, window, _| window.remove_window()).unwrap();
        cx.run_until_parked();
        assert!(settings_windows(cx).is_empty(), "the settings window is closed before quitting");
        assert_eq!(quits.get(), 1, "one quit decision, not one per closed window");
    }
}
