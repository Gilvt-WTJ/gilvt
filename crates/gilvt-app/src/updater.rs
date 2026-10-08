//! Automatic updates through Sparkle 2 (`Contents/Frameworks/Sparkle.framework`, embedded by
//! `scripts/bundle.sh` for release builds). The framework is loaded at run time, so builds without it
//! (development, GUI tests) simply have no updater.
//!
//! `[update] mode`: `download` (default) checks in the background, downloads, and installs when gilvt quits
//! — Sparkle's helper replaces the bundle after this process exits, so running agents are never cut off
//! by an update; `check` only checks (Sparkle shows its own window when it finds one); `off` never checks.
//! When an update is downloaded and waiting for the quit, the sidebar footer says so (`ready`).
//!
//! The feed and the EdDSA public key come from Info.plist (`SUFeedURL`, `SUPublicEDKey`);
//! `GILVT_UPDATE_FEED_URL` replaces the feed for testing and makes gilvt check at launch.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, AllocAnyThread};
use objc2_foundation::{NSBundle, NSString};

use gpui::{App, Global};

use crate::settings::UpdateMode;
use crate::AppSettings;

/// Replaces Info.plist's `SUFeedURL` (testing).
pub const ENV_FEED_URL: &str = "GILVT_UPDATE_FEED_URL";

pub struct Updater {
    controller: Retained<AnyObject>,
    /// The version downloaded and waiting to be installed when gilvt quits.
    pub ready: Option<String>,
}

impl Global for Updater {}

/// Messages from Sparkle's delegate (any thread) to the main thread.
enum Event {
    Ready(String),
}

static EVENTS: std::sync::OnceLock<async_channel::Sender<Event>> = std::sync::OnceLock::new();

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Delegate` has no Drop impl.
    #[unsafe(super(NSObject))]
    #[name = "GilvtUpdaterDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    // SPUUpdaterDelegate. Sparkle asks with respondsToSelector:, so no protocol conformance is declared.
    impl Delegate {
        /// An update was downloaded and installs when gilvt quits. NO: Sparkle keeps installing it on quit
        /// (gilvt never installs immediately, which would bypass the close confirmation).
        #[unsafe(method(updater:willInstallUpdateOnQuit:immediateInstallationBlock:))]
        fn will_install_on_quit(&self, _updater: &AnyObject, item: &AnyObject, _install_now: &AnyObject) -> Bool {
            let version: Option<Retained<NSString>> = unsafe { msg_send![item, displayVersionString] };
            if let (Some(v), Some(tx)) = (version, EVENTS.get()) {
                let _ = tx.try_send(Event::Ready(v.to_string()));
            }
            Bool::NO
        }

        /// The feed: `GILVT_UPDATE_FEED_URL` when set, else nil (Info.plist's `SUFeedURL`).
        #[unsafe(method_id(feedURLStringForUpdater:))]
        fn feed_url(&self, _updater: &AnyObject) -> Option<Retained<NSString>> {
            std::env::var(ENV_FEED_URL).ok().filter(|u| !u.is_empty()).map(|u| NSString::from_str(&u))
        }

        #[unsafe(method(updater:didAbortWithError:))]
        fn did_abort(&self, _updater: &AnyObject, error: &AnyObject) {
            let text: Option<Retained<NSString>> = unsafe { msg_send![error, localizedDescription] };
            eprintln!("gilvt: update check failed: {}", text.map(|t| t.to_string()).unwrap_or_default());
        }

        #[unsafe(method(updaterDidNotFindUpdate:error:))]
        fn did_not_find(&self, _updater: &AnyObject, error: &AnyObject) {
            let text: Option<Retained<NSString>> = unsafe { msg_send![error, description] };
            eprintln!("gilvt: no update: {}", text.map(|t| t.to_string()).unwrap_or_default());
        }

        #[unsafe(method(updater:failedToDownloadUpdate:error:))]
        fn failed_to_download(&self, _updater: &AnyObject, _item: &AnyObject, error: &AnyObject) {
            let text: Option<Retained<NSString>> = unsafe { msg_send![error, description] };
            eprintln!("gilvt: update download failed: {}", text.map(|t| t.to_string()).unwrap_or_default());
        }
    }
);

impl Delegate {
    fn new() -> Retained<Self> {
        unsafe { msg_send![Self::alloc(), init] }
    }
}

/// `Contents/Frameworks/Sparkle.framework` of the running bundle, loaded; None outside a bundle that has it.
fn load_framework() -> Option<()> {
    let main = NSBundle::mainBundle();
    let frameworks = main.privateFrameworksPath()?.to_string();
    let path = format!("{frameworks}/Sparkle.framework");
    if !std::path::Path::new(&path).exists() {
        return None;
    }
    let bundle = NSBundle::bundleWithPath(&NSString::from_str(&path))?;
    let loaded: Bool = unsafe { msg_send![&*bundle, load] };
    loaded.as_bool().then_some(())
}

/// Starts Sparkle when the bundle carries it and `[update] mode` is not `off`.
pub fn init(cx: &mut App) {
    let mode = cx.global::<AppSettings>().0.update.mode;
    if mode == UpdateMode::Off {
        return;
    }
    if load_framework().is_none() {
        return;
    }
    let Some(class) = AnyClass::get(c"SPUStandardUpdaterController") else {
        eprintln!("gilvt: Sparkle.framework has no SPUStandardUpdaterController; updates are off");
        return;
    };
    let (tx, rx) = async_channel::unbounded();
    let _ = EVENTS.set(tx);
    let delegate = Delegate::new();
    let controller: Option<Retained<AnyObject>> = unsafe {
        let alloc: objc2::rc::Allocated<AnyObject> = msg_send![class, alloc];
        msg_send![alloc, initWithStartingUpdater: Bool::NO, updaterDelegate: &*delegate, userDriverDelegate: std::ptr::null::<AnyObject>()]
    };
    // Sparkle keeps a weak reference to its delegate.
    std::mem::forget(delegate);
    let Some(controller) = controller else { return };
    cx.set_global(Updater { controller, ready: None });
    apply_mode(mode, cx);
    unsafe {
        let _: () = msg_send![&*controller_of(cx), startUpdater];
    }
    // A test feed: check right away instead of waiting for Sparkle's schedule.
    if std::env::var(ENV_FEED_URL).is_ok_and(|u| !u.is_empty()) {
        if let Some(updater) = updater_object(cx) {
            unsafe {
                let _: () = msg_send![&*updater, checkForUpdatesInBackground];
            }
        }
    }
    cx.spawn(async move |cx| {
        while let Ok(event) = rx.recv().await {
            let _ = cx.update(|cx| match event {
                Event::Ready(version) => {
                    if cx.has_global::<Updater>() {
                        cx.global_mut::<Updater>().ready = Some(version);
                        crate::workspace::notify_all(cx);
                    }
                }
            });
        }
    })
    .detach();
}

fn controller_of(cx: &App) -> Retained<AnyObject> {
    cx.global::<Updater>().controller.clone()
}

fn updater_object(cx: &App) -> Option<Retained<AnyObject>> {
    let controller = controller_of(cx);
    unsafe { msg_send![&*controller, updater] }
}

/// Maps the mode onto Sparkle's two switches (Sparkle also persists them in the app's defaults).
fn apply_mode(mode: UpdateMode, cx: &App) {
    let Some(updater) = updater_object(cx) else { return };
    let (check, download) = match mode {
        UpdateMode::Off => (false, false),
        UpdateMode::Check => (true, false),
        UpdateMode::Download => (true, true),
    };
    unsafe {
        let _: () = msg_send![&*updater, setAutomaticallyChecksForUpdates: Bool::new(check)];
        let _: () = msg_send![&*updater, setAutomaticallyDownloadsUpdates: Bool::new(download)];
    }
}

/// After config.toml changed: the new mode applies at once (turning updates on from `off` needs a restart,
/// since Sparkle was never started).
pub fn settings_changed(cx: &mut App) {
    if cx.has_global::<Updater>() {
        apply_mode(cx.global::<AppSettings>().0.update.mode, cx);
    }
}

/// 「检查更新…」: Sparkle's user-initiated check (it shows its own window with the result).
pub fn check_now(cx: &mut App) {
    if !cx.has_global::<Updater>() {
        return;
    }
    let controller = controller_of(cx);
    unsafe {
        let _: () = msg_send![&*controller, checkForUpdates: std::ptr::null::<AnyObject>()];
    }
}

/// Whether this build can update itself (Sparkle loaded and started).
pub fn available(cx: &App) -> bool {
    cx.has_global::<Updater>()
}

/// The version waiting for the quit, if any.
pub fn ready(cx: &App) -> Option<&str> {
    cx.try_global::<Updater>().and_then(|u| u.ready.as_deref())
}

/// Top-level `update` of `gilvt debug state`.
pub fn debug(cx: &App) -> crate::debug_state::UpdateState {
    crate::debug_state::UpdateState {
        available: available(cx),
        mode: cx.global::<AppSettings>().0.update.mode.id(),
        ready: ready(cx).map(str::to_string),
    }
}
