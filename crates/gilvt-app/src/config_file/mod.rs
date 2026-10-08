//! config.toml at runtime (S2 §5.1): writes from the settings window (`[monitor]` from 「◎ 监控官」, `theme` from
//! 「外观」), reloads when anything else changes it, read-only while it does not parse.

pub mod edit;
pub mod reload;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gilvt_theme::Selection;
use gpui::{App, BorrowAppContext, Global, Task};

use crate::settings::{Settings, ThemeSetting};
use crate::theme::AppSettings;
use edit::{Changes, Edit, WriteError};
use reload::{Reload, WatchPlan};

/// Editors write in bursts (truncate, write, rename): read once things have been quiet this long.
pub const DEBOUNCE: Duration = Duration::from_millis(200);

/// The 「外观」 page applies a theme at once but writes it only once the choice has been stable this long: holding
/// ↓ in the list does not rewrite config.toml on every row.
pub const THEME_WRITE_DELAY: Duration = Duration::from_millis(300);

pub struct ConfigFile {
    pub path: PathBuf,
    /// Why the file cannot be used now (it does not parse): the settings window is read-only and the settings in
    /// memory are the last valid ones.
    pub error: Option<String>,
    /// The last write-back failed (the change still applies in memory).
    pub write_error: Option<String>,
    /// The changes that are only in memory: `[monitor]` edits whose write-back failed, and the theme the 「外观」
    /// page chose (waiting for [`THEME_WRITE_DELAY`], or its write failed). They go into the next write too, and
    /// `write_error` stays until they are persisted (or the file itself changes `[monitor]` / `theme`).
    unsaved: Changes,
    /// The pending theme write ([`THEME_WRITE_DELAY`]); replacing it cancels the earlier one.
    theme_write: Option<Task<()>>,
    /// What gilvt last loaded or wrote: a watcher event with this content is gilvt's own write.
    last_hash: Option<u64>,
    /// The last read failed (other than NotFound) and that was reported: the same failure is not reported on
    /// every event until a read succeeds again.
    unreadable_reported: bool,
    /// The settings the file itself last gave: a reload takes from the file only what changed since, so changes
    /// made in this session (⌘+ / ⌘−) survive a write-back or an edit of other keys.
    file_settings: Option<Settings>,
    /// The last warning about the file's values: shown again only when it changes.
    warning: Option<String>,
    /// The last good read had any text: a read that finds it gone or empty is read once more.
    had_text: bool,
    watcher: Option<notify::RecommendedWatcher>,
    /// The watcher waits for this missing directory's ancestor to gain the next directory down.
    awaiting: Option<PathBuf>,
    _task: Option<Task<()>>,
}

impl Global for ConfigFile {}

/// What a read of the file asks of the app.
#[derive(Debug, PartialEq)]
enum Step {
    Nothing,
    /// `previous`: what the file gave before ([`merge_file_change`]). `warning`: only a new one.
    Apply { settings: Settings, previous: Option<Box<Settings>>, warning: Option<String> },
    /// The file cannot be used: say why (once).
    Report(String),
}

impl ConfigFile {
    fn new(path: PathBuf) -> ConfigFile {
        ConfigFile {
            path,
            error: None,
            write_error: None,
            unsaved: Changes::default(),
            theme_write: None,
            last_hash: None,
            unreadable_reported: false,
            file_settings: None,
            warning: None,
            had_text: false,
            watcher: None,
            awaiting: None,
            _task: None,
        }
    }

    /// A file that is not read or watched (tests of windows that observe it).
    #[cfg(test)]
    pub fn unwatched(path: PathBuf) -> ConfigFile {
        ConfigFile::new(path)
    }

    /// Takes in the file as just read (`read`), updating `error` and what is remembered.
    fn observe(&mut self, read: std::io::Result<String>) -> Step {
        let unreadable = matches!(&read, Err(e) if e.kind() != std::io::ErrorKind::NotFound);
        let has_text = matches!(&read, Ok(t) if !t.trim().is_empty());
        let (decision, hash) = reload::classify(read, &self.path, self.last_hash);
        if unreadable {
            let Reload::Invalid(e) = decision else { unreachable!("a failed read is Invalid") };
            // Forget the last text: when the same text can be read again it applies, and `error` clears.
            self.last_hash = None;
            self.error = Some(e.clone());
            let repeated = std::mem::replace(&mut self.unreadable_reported, true);
            return if repeated { Step::Nothing } else { Step::Report(e) };
        }
        self.unreadable_reported = false;
        self.last_hash = hash;
        self.had_text = has_text;
        match decision {
            Reload::Same => Step::Nothing,
            Reload::Apply { settings, warning } => {
                self.error = None;
                let previous = self.file_settings.replace(settings.clone()).map(Box::new);
                // Unsaved `[monitor]` edits (and an unsaved theme) stay only in memory through an unrelated change of
                // the file, and so does their banner; once the file changes `[monitor]` (`theme`) itself, the file
                // wins ([`merge_file_change`]).
                if previous.as_ref().is_some_and(|p| p.monitor != settings.monitor) {
                    self.unsaved.monitor.clear();
                }
                if previous.as_ref().is_some_and(|p| p.theme != settings.theme) {
                    self.unsaved.theme = None;
                }
                if previous
                    .as_ref()
                    .is_some_and(|p| p.language != settings.language)
                {
                    self.unsaved.language = None;
                }
                if self.unsaved.is_empty() {
                    self.write_error = None;
                }
                let fresh = warning.clone().filter(|_| warning != self.warning);
                self.warning = warning;
                Step::Apply { settings, previous, warning: fresh }
            }
            Reload::Invalid(e) => {
                self.error = Some(e.clone());
                Step::Report(e)
            }
        }
    }

    /// gilvt wrote `text` (its edits merged into whatever the file held): that is the truth now, other changes made
    /// to the file meanwhile included. The watcher's event for it then finds the same text.
    fn written(&mut self, text: String) -> Step {
        self.last_hash = None;
        self.observe(Ok(text))
    }
}

/// The settings after the file went from `previous` to `new` while `memory` was in effect: each top-level key
/// the file changed comes from the file, every other one stays as it is in memory (a ⌘+ zoom, a change whose
/// write-back failed). Without a `previous` (nothing valid read yet) the file's settings are taken whole.
pub fn merge_file_change(previous: Option<&Settings>, new: Settings, memory: &Settings) -> Settings {
    let Some(prev) = previous else { return new };
    macro_rules! merged {
        ($($field:ident),* $(,)?) => {
            Settings { $($field: if new.$field != prev.$field { new.$field } else { memory.$field.clone() }),* }
        };
    }
    // Every field: a new one that is missing here does not compile.
    merged!(
        language_setting,
        language,
        font_family,
        font_size,
        line_height,
        fallback_fonts,
        theme,
        scrollback,
        option_as_meta,
        kitty_keyboard,
        shell,
        shell_integration,
        agent,
        notify,
        monitor,
        update,
        remote,
        colors,
    )
}

/// After `AppSettings` is set from the same file. A file that does not parse makes the settings window read-only
/// from the start (the settings in memory are the defaults then, as before); its error was reported at startup.
pub fn init(path: PathBuf, cx: &mut App) {
    let mut file = ConfigFile::new(path);
    let _ = file.observe(std::fs::read_to_string(&file.path));
    cx.set_global(file);
    ensure_watching(cx);
    cx.on_app_quit(|cx| {
        flush_theme(cx);
        async {}
    })
    .detach();
}

pub fn readonly(cx: &App) -> bool {
    cx.try_global::<ConfigFile>().is_some_and(|f| f.error.is_some())
}

/// Starts watching the config's directory. When it does not exist yet (a first-time user), polls for it every
/// [`reload::ANCESTOR_POLL`] instead (a filesystem watcher on its nearest ancestor, `$HOME`, would wake on every
/// file written under it) and moves down as directories appear; the settings window's 「在编辑器中打开」 and
/// gilvt's first write call this again once they created it.
pub(crate) fn ensure_watching(cx: &mut App) {
    use notify::Watcher;
    let Some(file) = cx.try_global::<ConfigFile>() else { return };
    if file.watcher.is_some() && file.awaiting.is_none() {
        return;
    }
    let path = file.path.clone();
    let awaiting = file.awaiting.clone();
    let plan = reload::watch_plan(&reload::watch_dirs(&path), Path::is_dir);
    if awaiting.as_ref().is_some_and(|a| reload::still_awaiting(a, &plan)) {
        return;
    }
    let Some(plan) = plan else { return };
    let (dirs, names) = match plan {
        WatchPlan::Dirs(dirs) => {
            let names = [path.file_name().map(OsString::from), edit::real_target(&path).file_name().map(OsString::from)];
            (dirs, names.into_iter().flatten().collect::<Vec<_>>())
        }
        WatchPlan::Ancestor { dir, .. } => {
            // The task is the poll: it ends when the directory appears or the plan moves (a new task replaces it).
            let task = cx.spawn(async move |cx| loop {
                cx.background_executor().timer(reload::ANCESTOR_POLL).await;
                if !matches!(cx.update(directory_appeared), Ok(true)) {
                    break;
                }
            });
            cx.update_global::<ConfigFile, _>(|f, _| {
                f.watcher = None;
                f.awaiting = Some(dir);
                f._task = Some(task);
            });
            return;
        }
    };
    let (tx, rx) = async_channel::unbounded::<()>();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok_and(|e| reload::concerns(&e.paths, &names)) {
            let _ = tx.try_send(());
        }
    });
    let Ok(mut watcher) = watcher else { return };
    if dirs.iter().any(|d| watcher.watch(d, notify::RecursiveMode::NonRecursive).is_err()) {
        return;
    }
    let task = cx.spawn(async move |cx| {
        while rx.recv().await.is_ok() {
            // Trailing debounce: wait until the file has been quiet for a whole DEBOUNCE.
            // At most MAX_DEBOUNCE in all: a file rewritten without pause is read anyway.
            let mut rounds = 0;
            loop {
                cx.background_executor().timer(DEBOUNCE).await;
                rounds += 1;
                let mut more = false;
                while rx.try_recv().is_ok() {
                    more = true;
                }
                if !reload::keep_waiting(more, rounds) {
                    break;
                }
            }
            match cx.update(reload_checked) {
                Err(_) => break,
                Ok(false) => {}
                Ok(true) => {
                    cx.background_executor().timer(DEBOUNCE).await;
                    while rx.try_recv().is_ok() {}
                    if cx.update(reload_now).is_err() {
                        break;
                    }
                }
            }
        }
    });
    cx.update_global::<ConfigFile, _>(|f, _| {
        f.watcher = Some(watcher);
        f.awaiting = None;
        f._task = Some(task);
    });
}

/// One poll for the missing directory: plans again, and reads the file once the config's own directory exists
/// (it may have been written before the watch started). `true`: still waiting on the same ancestor.
fn directory_appeared(cx: &mut App) -> bool {
    let Some(waiting) = cx.try_global::<ConfigFile>().and_then(|f| f.awaiting.clone()) else { return false };
    ensure_watching(cx);
    let file = cx.global::<ConfigFile>();
    if file.watcher.is_some() && file.awaiting.is_none() {
        reload_now(cx);
        return false;
    }
    file.awaiting.as_ref() == Some(&waiting)
}

/// [`reload_now`], unless the file just lost its text ([`reload::read_again_first`]): then nothing happens and
/// `true` asks the caller to read again after another [`DEBOUNCE`].
fn reload_checked(cx: &mut App) -> bool {
    let Some(file) = cx.try_global::<ConfigFile>() else { return false };
    let text = std::fs::read_to_string(&file.path);
    if reload::read_again_first(&text, file.had_text) {
        return true;
    }
    let step = cx.update_global::<ConfigFile, _>(|f, _| f.observe(text));
    take_step(step, cx);
    false
}

/// Reads the file again (the watcher calls this after [`DEBOUNCE`]).
pub fn reload_now(cx: &mut App) {
    let Some(file) = cx.try_global::<ConfigFile>() else { return };
    let text = std::fs::read_to_string(&file.path);
    let step = cx.update_global::<ConfigFile, _>(|f, _| f.observe(text));
    take_step(step, cx);
}

/// Applies what a read of the file asks for and says what is wrong with it: the file's warning and the errors of
/// a theme it changed to (a name that does not resolve, in either slot of a pair) share one banner, as at startup.
fn take_step(step: Step, cx: &mut App) {
    let banner = match step {
        Step::Nothing => return,
        Step::Apply { settings, previous, warning } => {
            let merged = merge_file_change(previous.as_deref(), settings, &cx.global::<AppSettings>().0);
            let theme_errors = apply_settings(merged, cx);
            // A theme that could not be written while the file did not parse goes in now that it does.
            if cx.global::<ConfigFile>().unsaved.theme.is_some() && cx.global::<ConfigFile>().theme_write.is_none() {
                write_unsaved_theme(cx);
            }
            let all: Vec<String> = warning.into_iter().chain(theme_errors).collect();
            (!all.is_empty()).then(|| all.join(crate::i18n::text("；", "; ")))
        }
        Step::Report(e) => Some(
            if crate::i18n::current() == crate::i18n::Language::English {
                format!("{e} (continuing with the last valid settings)")
            } else {
                format!("{e}（沿用上一次有效的设置）")
            },
        ),
    };
    if let Some(message) = banner {
        show_banner(message, cx);
    }
    refresh_all(cx);
}

/// The red banner at the top of every workspace window.
fn show_banner(message: String, cx: &mut App) {
    eprintln!("gilvt: {message}");
    for w in crate::workspace::workspaces(cx) {
        let _ = w.update(cx, |ws, _, cx| ws.show_error(message.clone(), cx));
    }
}

/// New settings take effect at once: every window redraws with them, the 监控官 reacts to its own keys, a new
/// `theme` / `[colors]` recolors every window. Returns the new theme's errors (empty when the theme is unchanged).
pub fn apply_settings(new: Settings, cx: &mut App) -> Vec<String> {
    let old = cx.global::<AppSettings>().0.monitor.clone();
    let monitor = new.monitor.clone();
    let prev = &cx.global::<AppSettings>().0;
    let language_changed = prev.language != new.language;
    let theme_changed = prev.theme != new.theme || prev.colors != new.colors;
    let (selection, overrides) = (new.theme.selection(), new.colors.overrides());
    cx.update_global::<AppSettings, _>(|s, _| s.0 = new);
    if language_changed {
        crate::i18n::set_current(cx.global::<AppSettings>().0.language);
        cx.set_menus(crate::actions::menus(cx.global::<AppSettings>().0.language));
    }
    let mut theme_errors = Vec::new();
    if theme_changed && cx.has_global::<crate::theme::ThemeState>() {
        cx.update_global::<crate::theme::ThemeState, _>(|t, _| t.reconfigure(selection, overrides));
        theme_errors = cx.global::<crate::theme::ThemeState>().errors();
        // Also sets each window's AppKit appearance (fixed theme vs following the system).
        crate::theme::refresh_all(cx);
    }
    crate::monitor::summaries::settings_changed(&old, &monitor, cx);
    crate::monitor::chat::settings_changed(&old, &monitor, cx);
    crate::updater::settings_changed(cx);
    refresh_all(cx);
    theme_errors
}

/// Changes the application language immediately and writes the top-level `language` key.
pub fn set_language(language: crate::i18n::Language, cx: &mut App) -> Result<(), String> {
    if readonly(cx) {
        return Err(crate::i18n::text(
            "config.toml 有语法错误，修好之前不能修改",
            "config.toml has a syntax error and cannot be changed until it is fixed",
        )
        .into());
    }
    // Picking the language that only follows macOS still writes it: from then on it stays put.
    if cx.global::<AppSettings>().0.language_setting == Some(language) {
        return Ok(());
    }
    let mut settings = cx.global::<AppSettings>().0.clone();
    settings.language_setting = Some(language);
    settings.language = language;
    apply_settings(settings, cx);
    let change = Changes {
        language: Some(language),
        ..Changes::default()
    };
    let pending = cx.global::<ConfigFile>().unsaved.then(&change);
    write(pending, cx);
    Ok(())
}

fn refresh_all(cx: &mut App) {
    for handle in cx.windows() {
        let _ = handle.update(cx, |_, window, _| window.refresh());
    }
    crate::workspace::notify_all(cx);
}

/// The settings window changed `edits`: they apply in memory first, then go into the file. Refused while the file
/// does not parse (`Err` with the reason). A failed write keeps the change in memory and is kept in `write_error`.
pub fn set_monitor(edits: Vec<Edit>, cx: &mut App) -> Result<(), String> {
    if readonly(cx) {
        return Err(crate::i18n::text(
            "config.toml 有语法错误，修好之前不能修改",
            "config.toml has a syntax error and cannot be changed until it is fixed",
        )
        .into());
    }
    if edits.is_empty() {
        return Ok(());
    }
    let mut settings = cx.global::<AppSettings>().0.clone();
    for (key, value) in &edits {
        edit::apply_in_memory(&mut settings.monitor, key, value);
    }
    apply_settings(settings, cx);
    // Earlier changes only in memory (a failed write, a theme still waiting) go in again with the new ones.
    let pending = cx.global::<ConfigFile>().unsaved.then(&Changes::monitor(&edits));
    write(pending, cx);
    Ok(())
}

/// Writes `pending` (everything not yet in the file) and takes in the result: on success the file is the truth,
/// on failure the changes stay in memory and in `unsaved`, and `write_error` says why.
fn write(pending: Changes, cx: &mut App) {
    let path = cx.global::<ConfigFile>().path.clone();
    let result = edit::write_changes(&path, &pending);
    let step = cx.update_global::<ConfigFile, _>(|f, _| match result {
        Ok(w) => {
            f.unsaved = Changes::default();
            f.write_error = None;
            f.written(w.text)
        }
        Err(e) => {
            if let WriteError::Syntax(msg) = &e {
                f.error = Some(msg.clone());
            }
            f.unsaved = pending;
            f.write_error = Some(e.message());
            Step::Nothing
        }
    });
    // The file may have changed between gilvt's read and its write: memory follows what was written.
    take_step(step, cx);
    ensure_watching(cx);
    refresh_all(cx);
}

/// The 「外观」 page chose `sel`: it applies at once in every window (ThemeState, each window's appearance) and goes
/// into config.toml's `theme` once the choice has been stable for [`THEME_WRITE_DELAY`]. Refused while the file
/// does not parse (`Err` with the reason), like [`set_monitor`]. A failed write keeps the theme in memory.
pub fn set_theme(sel: Selection, cx: &mut App) -> Result<(), String> {
    if readonly(cx) {
        return Err(crate::i18n::text(
            "config.toml 有语法错误，修好之前不能修改",
            "config.toml has a syntax error and cannot be changed until it is fixed",
        )
        .into());
    }
    if cx.try_global::<crate::theme::ThemeState>().is_some_and(|t| *t.selection() == sel) {
        return Ok(());
    }
    let mut settings = cx.global::<AppSettings>().0.clone();
    settings.theme = ThemeSetting::from(&sel);
    let errors = apply_settings(settings, cx);
    if !errors.is_empty() {
        show_banner(errors.join(crate::i18n::text("；", "; ")), cx);
    }
    cx.update_global::<ConfigFile, _>(|f, _| f.unsaved.theme = Some(sel));
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(THEME_WRITE_DELAY).await;
        let _ = cx.update(theme_write_done);
    });
    // Replacing the task drops (cancels) the write of an earlier choice.
    cx.update_global::<ConfigFile, _>(|f, _| f.theme_write = Some(task));
    Ok(())
}

/// Writes the theme the page chose, if it is still waiting (a reload may have dropped it: the file changed `theme`
/// itself). While the file does not parse it stays in memory and `write_error` says so, as for any blocked
/// write-back; it is written once a reload finds the file valid again.
fn write_unsaved_theme(cx: &mut App) {
    let Some(file) = cx.try_global::<ConfigFile>() else { return };
    if file.unsaved.theme.is_none() {
        return;
    }
    if let Some(e) = file.error.clone() {
        cx.update_global::<ConfigFile, _>(|f, _| f.write_error = Some(WriteError::Syntax(e).message()));
        refresh_all(cx);
        return;
    }
    write(file.unsaved.clone(), cx);
}

/// The delayed theme write ran: it no longer counts as waiting.
fn theme_write_done(cx: &mut App) {
    cx.update_global::<ConfigFile, _>(|f, _| f.theme_write = None);
    write_unsaved_theme(cx);
}

/// gilvt is quitting: a theme still waiting for [`THEME_WRITE_DELAY`] is written now.
pub fn flush_theme(cx: &mut App) {
    if cx.try_global::<ConfigFile>().is_some_and(|f| f.unsaved.theme.is_some()) {
        cx.update_global::<ConfigFile, _>(|f, _| f.theme_write = None);
        write_unsaved_theme(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit::TomlValue;
    use std::io::{Error, ErrorKind};

    fn denied() -> std::io::Result<String> {
        Err(Error::new(ErrorKind::PermissionDenied, "denied"))
    }

    #[test]
    fn a_write_racing_an_external_edit_keeps_both_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "font_size = 14\n").unwrap();
        let mut f = ConfigFile::new(path.clone());
        assert!(matches!(f.observe(std::fs::read_to_string(&path)), Step::Apply { .. }));
        let edits = [("enabled", TomlValue::Bool(true))];
        // Another program changes font_size between gilvt's read and its rename (once).
        let mut external = Some("font_size = 16\n");
        let w = edit::write_monitor_keys_with(&path, &edits, &mut || {
            if let Some(text) = external.take() {
                std::fs::write(&path, text).unwrap();
            }
        })
        .unwrap();
        let Step::Apply { settings, .. } = f.written(w.text) else { panic!("the written text applies") };
        assert!(settings.monitor.enabled, "gilvt's edit");
        assert_eq!(settings.font_size, 16.0, "the external change");
        assert_eq!(f.observe(std::fs::read_to_string(&path)), Step::Nothing, "the watcher's event for gilvt's own write");
    }

    #[test]
    fn readable_again_after_a_failed_read_is_not_read_only() {
        let mut f = ConfigFile::new(PathBuf::from("/c/config.toml"));
        let a = "font_size = 15\n";
        assert!(matches!(f.observe(Ok(a.into())), Step::Apply { .. }));
        assert!(matches!(f.observe(denied()), Step::Report(e) if e.contains("denied")));
        assert!(f.error.is_some(), "read-only while unreadable");
        assert_eq!(f.observe(denied()), Step::Nothing, "the same failure is reported once");
        assert!(matches!(f.observe(Ok(a.into())), Step::Apply { .. }));
        assert_eq!(f.error, None, "the same text readable again: editable");
        assert!(matches!(f.observe(denied()), Step::Report(_)), "a new failure after a good read is reported again");
    }

    /// What the app holds after taking `step` with `memory` in effect (as `take_step` does).
    fn after(step: Step, memory: &Settings) -> Settings {
        let Step::Apply { settings, previous, .. } = step else { panic!("{step:?}") };
        merge_file_change(previous.as_deref(), settings, memory)
    }

    #[test]
    fn a_zoom_survives_a_write_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "font_size = 14\n").unwrap();
        let mut f = ConfigFile::new(path.clone());
        let mut memory = after(f.observe(std::fs::read_to_string(&path)), &Settings::default());
        assert_eq!(memory.font_size, 14.0);
        memory.font_size = 16.0; // ⌘+ twice
        memory.monitor.enabled = true; // set_monitor applies in memory first
        let w = edit::write_monitor_keys(&path, &[("enabled", TomlValue::Bool(true))]).unwrap();
        let memory = after(f.written(w.text), &memory);
        assert_eq!(memory.font_size, 16.0, "the session's zoom stays");
        assert!(memory.monitor.enabled);
    }

    #[test]
    fn an_external_font_size_edit_beats_the_zoom() {
        let mut f = ConfigFile::new(PathBuf::from("/c/config.toml"));
        let mut memory = after(f.observe(Ok("font_size = 14\n".into())), &Settings::default());
        memory.font_size = 16.0;
        let mut memory = after(f.observe(Ok("font_size = 18\n".into())), &memory);
        assert_eq!(memory.font_size, 18.0, "the file changed font_size: the file wins");
        memory.font_size = 20.0;
        let memory = after(f.observe(Ok("font_size = 18\nscrollback = 500\n".into())), &memory);
        assert_eq!(memory.font_size, 20.0, "another key changed: the zoom stays");
        assert_eq!(memory.scrollback, 500);
    }

    #[test]
    fn merge_takes_only_what_the_file_changed() {
        let prev = Settings::default();
        let theme = |n: &str| crate::settings::ThemeSetting::Name(n.into());
        let new = Settings { theme: theme("dark"), ..Settings::default() };
        let memory = Settings { font_size: 20.0, theme: theme("light"), ..Settings::default() };
        let out = merge_file_change(Some(&prev), new.clone(), &memory);
        assert_eq!(out.font_size, 20.0);
        assert_eq!(out.theme, theme("dark"));
        assert_eq!(merge_file_change(None, new.clone(), &memory), new, "nothing known from the file yet: all of it");
    }

    #[test]
    fn merge_applies_only_a_language_changed_on_disk() {
        let prev = Settings::default();
        let new = Settings {
            language: crate::i18n::Language::English,
            ..prev.clone()
        };
        let memory = Settings {
            font_size: 20.0,
            ..prev.clone()
        };

        let out = merge_file_change(Some(&prev), new, &memory);
        assert_eq!(out.language, crate::i18n::Language::English);
        assert_eq!(out.font_size, 20.0, "an unrelated in-memory zoom survives");
    }

    #[test]
    fn a_warning_is_shown_only_when_it_changes() {
        let mut f = ConfigFile::new(PathBuf::from("/c/config.toml"));
        let bad = "[monitor]\nsummary_interval = \"x\"\n";
        assert!(matches!(f.observe(Ok(bad.into())), Step::Apply { warning: Some(_), .. }));
        let again = format!("{bad}enabled = true\n");
        assert!(matches!(f.observe(Ok(again)), Step::Apply { warning: None, .. }), "the same warning is not shown again");
        assert!(matches!(f.observe(Ok("font_size = 15\n".into())), Step::Apply { warning: None, .. }));
        assert!(matches!(f.observe(Ok(bad.into())), Step::Apply { warning: Some(_), .. }), "back after it was fixed: shown");
    }

    #[test]
    fn a_successful_reload_clears_the_write_error() {
        let mut f = ConfigFile::new(PathBuf::from("/c/config.toml"));
        f.write_error = Some("写回 config.toml 失败：denied".into());
        assert!(matches!(f.observe(Ok("font_size = 15\n".into())), Step::Apply { .. }));
        assert_eq!(f.write_error, None, "nothing unsaved: it clears");
    }

    fn failed_monitor_write(f: &mut ConfigFile) {
        f.unsaved = Changes::monitor(&[("enabled", TomlValue::Bool(true))]);
        f.write_error = Some("写回 config.toml 失败：denied".into());
    }

    #[test]
    fn an_unrelated_reload_keeps_the_write_error() {
        let mut f = ConfigFile::new(PathBuf::from("/c/config.toml"));
        assert!(matches!(f.observe(Ok("font_size = 14\n".into())), Step::Apply { .. }));
        failed_monitor_write(&mut f);
        assert!(matches!(f.observe(Ok("font_size = 15\n".into())), Step::Apply { .. }));
        assert!(f.write_error.is_some(), "the monitor change is still only in memory");
        assert_eq!(f.unsaved.monitor.len(), 1);
    }

    #[test]
    fn a_file_change_to_monitor_clears_it() {
        let mut f = ConfigFile::new(PathBuf::from("/c/config.toml"));
        assert!(matches!(f.observe(Ok("font_size = 14\n".into())), Step::Apply { .. }));
        failed_monitor_write(&mut f);
        assert!(matches!(f.observe(Ok("font_size = 14\n[monitor]\nenabled = false\nprovider = \"codex\"\n".into())), Step::Apply { .. }));
        assert_eq!(f.write_error, None, "the file's [monitor] wins over the unsaved change");
        assert!(f.unsaved.is_empty());
    }

    #[test]
    fn a_file_language_change_clears_an_unsaved_language() {
        let mut f = ConfigFile::new(PathBuf::from("/c/config.toml"));
        assert!(matches!(
            f.observe(Ok("language = \"zh-CN\"\n".into())),
            Step::Apply { .. }
        ));
        f.unsaved.language = Some(crate::i18n::Language::English);
        f.write_error = Some("write failed".into());

        assert!(matches!(
            f.observe(Ok("language = \"en\"\n".into())),
            Step::Apply { .. }
        ));
        assert_eq!(f.unsaved.language, None);
        assert_eq!(
            f.write_error, None,
            "the value from disk wins and clears the stale write error"
        );
    }

    // The theme (the 「外观」 page, hot reload).

    use crate::theme::ThemeState;
    use gpui::TestAppContext;
    use std::os::unix::fs::PermissionsExt;

    const COMMENTED: &str = "# 我的配置\nfont_size = 14 # 字号\n\n[agent]\nclaude_launch = \"claude\"\n";

    fn app_with(cx: &mut TestAppContext, path: &Path) {
        cx.update(|cx| {
            let (settings, _) = Settings::parse(&std::fs::read_to_string(path).unwrap_or_default(), path).unwrap();
            cx.set_global(ThemeState::new(settings.theme.selection(), settings.colors.overrides(), None));
            cx.set_global(AppSettings(settings));
            let mut file = ConfigFile::new(path.to_path_buf());
            let _ = file.observe(std::fs::read_to_string(path));
            cx.set_global(file);
        });
    }

    fn in_use(cx: &mut TestAppContext) -> Selection {
        cx.update(|cx| cx.global::<ThemeState>().selection().clone())
    }

    fn nord() -> Selection {
        Selection::Fixed("Nord".into())
    }

    #[gpui::test]
    fn a_chosen_theme_applies_at_once_and_is_written_once_stable(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, COMMENTED).unwrap();
        app_with(cx, &path);
        // ↓ ↓ ↓ in the list: each applies at once, none is written yet.
        for name in ["Dracula", "Catppuccin Mocha", "Nord"] {
            cx.update(|cx| set_theme(Selection::Fixed(name.into()), cx)).unwrap();
            assert_eq!(in_use(cx), Selection::Fixed(name.into()));
            cx.executor().advance_clock(THEME_WRITE_DELAY / 2);
            cx.run_until_parked();
        }
        assert_eq!(cx.update(|cx| cx.global::<AppSettings>().0.theme.clone()), ThemeSetting::Name("Nord".into()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), COMMENTED, "not written while the choice keeps changing");
        cx.executor().advance_clock(THEME_WRITE_DELAY);
        cx.run_until_parked();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, COMMENTED.replace("font_size = 14 # 字号\n", "font_size = 14 # 字号\ntheme = \"Nord\"\n"), "only the last one, comments kept");
        assert_eq!(cx.update(|cx| cx.global::<ConfigFile>().write_error.clone()), None);
        // The watcher's read of gilvt's own write changes nothing.
        cx.update(reload_now);
        assert_eq!(in_use(cx), nord());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[gpui::test]
    fn the_theme_waiting_is_written_on_quit(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        app_with(cx, &path);
        cx.update(|cx| set_theme(Selection::Pair { light: "Rose Pine Dawn".into(), dark: "Rose Pine".into() }, cx)).unwrap();
        cx.update(flush_theme);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "theme = { light = \"Rose Pine Dawn\", dark = \"Rose Pine\" }\n");
    }

    #[gpui::test]
    fn a_read_only_file_keeps_the_theme_in_memory(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, COMMENTED).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        app_with(cx, &path);
        cx.update(|cx| set_theme(nord(), cx)).unwrap();
        cx.executor().advance_clock(THEME_WRITE_DELAY);
        cx.run_until_parked();
        assert_eq!(in_use(cx), nord(), "in use all the same");
        let error = cx.update(|cx| cx.global::<ConfigFile>().write_error.clone()).unwrap();
        assert!(error.starts_with("config.toml 是只读的"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), COMMENTED);
        // Writable again: the next write (any page's) takes the theme along.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        set_monitor_in(cx, vec![("enabled", TomlValue::Bool(true))]);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("theme = \"Nord\"") && text.contains("enabled = true"), "{text}");
        assert_eq!(cx.update(|cx| cx.global::<ConfigFile>().write_error.clone()), None);
    }

    fn set_monitor_in(cx: &mut TestAppContext, edits: Vec<Edit>) {
        cx.update(|cx| set_monitor(edits, cx)).unwrap();
    }

    #[gpui::test]
    fn a_file_broken_while_the_theme_waits_says_so_and_takes_it_once_fixed(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, COMMENTED).unwrap();
        app_with(cx, &path);
        cx.update(|cx| set_theme(nord(), cx)).unwrap();
        std::fs::write(&path, "oops =\n").unwrap();
        cx.update(reload_now);
        cx.executor().advance_clock(THEME_WRITE_DELAY);
        cx.run_until_parked();
        assert_eq!(in_use(cx), nord());
        let error = cx.update(|cx| cx.global::<ConfigFile>().write_error.clone()).unwrap();
        assert!(error.contains("语法错误"), "the page shows the theme is not written: {error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "oops =\n");
        assert!(cx.update(|cx| cx.global::<ConfigFile>().theme_write.is_none()), "the delayed write has run");
        // Fixed: the reload writes the waiting theme.
        std::fs::write(&path, COMMENTED).unwrap();
        cx.update(reload_now);
        assert!(std::fs::read_to_string(&path).unwrap().contains("theme = \"Nord\""));
        assert_eq!(cx.update(|cx| cx.global::<ConfigFile>().write_error.clone()), None);
        assert_eq!(in_use(cx), nord());
    }

    #[gpui::test]
    fn quitting_after_the_delayed_write_writes_nothing_more(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        app_with(cx, &path);
        cx.update(|cx| set_theme(nord(), cx)).unwrap();
        cx.executor().advance_clock(THEME_WRITE_DELAY);
        cx.run_until_parked();
        std::fs::write(&path, "theme = \"dark\"\n").unwrap();
        cx.update(flush_theme);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "theme = \"dark\"\n", "nothing was waiting");
    }

    #[gpui::test]
    fn a_broken_file_refuses_a_theme(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        app_with(cx, &path);
        std::fs::write(&path, "oops =\n").unwrap();
        cx.update(reload_now);
        assert!(cx.update(|cx| set_theme(nord(), cx)).is_err());
        assert_eq!(in_use(cx), Selection::system(), "unchanged");
    }

    #[gpui::test]
    fn editing_theme_or_colors_on_disk_applies_live(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "font_size = 14\n").unwrap();
        app_with(cx, &path);
        std::fs::write(&path, "font_size = 14\ntheme = \"Nord\"\n").unwrap();
        cx.update(reload_now);
        assert_eq!(in_use(cx), nord());
        std::fs::write(&path, "font_size = 14\ntheme = \"Nord\"\n[colors]\nbackground = \"#202040\"\n").unwrap();
        cx.update(reload_now);
        assert_eq!(cx.update(|cx| crate::theme::current(cx).overrides), 1);
        // A theme waiting to be written loses to an edit of `theme` on disk.
        cx.update(|cx| set_theme(Selection::Fixed("Dracula".into()), cx)).unwrap();
        std::fs::write(&path, "font_size = 14\ntheme = \"dark\"\n").unwrap();
        cx.update(reload_now);
        cx.executor().advance_clock(THEME_WRITE_DELAY);
        cx.run_until_parked();
        assert_eq!(in_use(cx), Selection::Fixed(gilvt_theme::GILVT_DARK.into()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "font_size = 14\ntheme = \"dark\"\n", "the file wins");
    }

    #[gpui::test]
    fn a_reloaded_theme_that_does_not_resolve_is_reported_for_both_slots(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        app_with(cx, &path);
        let mut settings = cx.update(|cx| cx.global::<AppSettings>().0.clone());
        settings.theme = ThemeSetting::Pair(crate::settings::ThemePair { light: "nope-light".into(), dark: "nope-dark".into() });
        let errors = cx.update(|cx| apply_settings(settings.clone(), cx));
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("nope-light") && errors[1].contains("nope-dark"), "{errors:?}");
        assert!(cx.update(|cx| apply_settings(settings, cx)).is_empty(), "the theme did not change: nothing to report");
    }

    #[test]
    fn an_unsaved_theme_survives_an_unrelated_reload() {
        let mut f = ConfigFile::new(PathBuf::from("/c/config.toml"));
        assert!(matches!(f.observe(Ok("font_size = 14\n".into())), Step::Apply { .. }));
        f.unsaved.theme = Some(nord());
        f.write_error = Some(WriteError::ReadOnly.message());
        assert!(matches!(f.observe(Ok("font_size = 15\n".into())), Step::Apply { .. }));
        assert!(f.write_error.is_some() && f.unsaved.theme.is_some());
        assert!(matches!(f.observe(Ok("font_size = 15\ntheme = \"dark\"\n".into())), Step::Apply { .. }));
        assert_eq!((f.write_error.as_deref(), &f.unsaved.theme), (None, &None), "the file's theme wins");
    }

    #[test]
    fn a_broken_file_is_reported_once_and_a_fix_applies() {
        let mut f = ConfigFile::new(PathBuf::from("/c/config.toml"));
        assert!(matches!(f.observe(Ok("oops =\n".into())), Step::Report(_)));
        assert_eq!(f.observe(Ok("oops =\n".into())), Step::Nothing);
        assert!(f.error.is_some());
        assert!(matches!(f.observe(Ok("font_size = 15\n".into())), Step::Apply { .. }));
        assert_eq!(f.error, None);
    }
}
