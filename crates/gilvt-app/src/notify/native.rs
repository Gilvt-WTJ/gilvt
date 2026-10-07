//! Native notifications through UNUserNotificationCenter. Only usable from a real `.app` bundle
//! (scripts/bundle.sh): in a bare binary the framework has no bundle proxy and throws.

use std::sync::{Mutex, OnceLock};

use block2::RcBlock;
use gilvt_agent::{AgentKind, PaneId, SessionKey};
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread};
use objc2_foundation::{NSBundle, NSDictionary, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};

use super::decide::Post;

const PANE_KEY: &str = "pane";
const AGENT_KEY: &str = "agent";
const SESSION_KEY: &str = "session";

/// A clicked notification: its pane and, for an agent's, the session it was about.
pub type Click = (PaneId, Option<SessionKey>);

/// Where clicks go (the main thread drains it).
static CLICKS: OnceLock<async_channel::Sender<Click>> = OnceLock::new();

/// Authorization is requested at the first notification; ones posted before the answer wait.
enum Auth {
    Unasked,
    Pending(Vec<Post>),
    Asked,
}

static AUTH: Mutex<Auth> = Mutex::new(Auth::Unasked);

/// A bundle LaunchServices knows: an identifier, the main bundle is a `.app` directory, and it is
/// not under /tmp (LaunchServices ignores bundles there, so the center would have no app to use).
pub fn is_app_bundle(bundle_id: Option<&str>, bundle_path: &str) -> bool {
    let in_tmp = ["/tmp/", "/private/tmp/"].iter().any(|t| bundle_path.starts_with(t));
    bundle_id.is_some_and(|id| !id.trim().is_empty()) && bundle_path.trim_end_matches('/').ends_with(".app") && !in_tmp
}

/// Whether this process runs from an app bundle (so the native center is usable).
pub fn bundled() -> bool {
    let bundle = NSBundle::mainBundle();
    let id = bundle.bundleIdentifier().map(|s| s.to_string());
    is_app_bundle(id.as_deref(), &bundle.bundlePath().to_string())
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Delegate` has no Drop impl.
    #[unsafe(super(NSObject))]
    #[name = "GilvtNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        /// Everything posted already passed the decision chain: show it even while gilvt is frontmost.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let info = response.notification().request().content().userInfo();
            let get = |key: &str| {
                info.objectForKey(&NSString::from_str(key)).and_then(|v| v.downcast::<NSString>().ok()).map(|s| s.to_string())
            };
            let pane = get(PANE_KEY).and_then(|s| s.parse::<PaneId>().ok());
            let session = get(AGENT_KEY).and_then(|a| AgentKind::from_name(&a)).zip(get(SESSION_KEY));
            if let (Some(pane), Some(tx)) = (pane, CLICKS.get()) {
                let _ = tx.try_send((pane, session));
            }
            handler.call(());
        }
    }
);

impl Delegate {
    fn new() -> Retained<Self> {
        unsafe { msg_send![Self::alloc(), init] }
    }
}

/// Sets the center's delegate; clicked notifications send their pane and session to `clicks`.
pub fn install(clicks: async_channel::Sender<Click>) {
    let _ = CLICKS.set(clicks);
    let delegate = Delegate::new();
    UNUserNotificationCenter::currentNotificationCenter().setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // The delegate property is weak: keep the object for the life of the process.
    std::mem::forget(delegate);
}

pub fn post(p: Post) {
    let mut auth = AUTH.lock().unwrap_or_else(|e| e.into_inner());
    match &mut *auth {
        Auth::Asked => add(&p),
        Auth::Pending(queue) => queue.push(p),
        Auth::Unasked => {
            *auth = Auth::Pending(vec![p]);
            drop(auth);
            let done = RcBlock::new(|_granted: Bool, _err: *mut NSError| {
                let queued = match std::mem::replace(&mut *AUTH.lock().unwrap_or_else(|e| e.into_inner()), Auth::Asked) {
                    Auth::Pending(q) => q,
                    _ => Vec::new(),
                };
                // Also when denied: the system drops them, and turning notifications on later in
                // System Settings works without a restart.
                queued.iter().for_each(add);
            });
            // Badge too: once the app is registered with the Notification Center, macOS only draws the Dock
            // tile's badge label (`dock_tile::set_badge`) when the app's 「标记应用程序图标」 is on, and that
            // switch is only offered (and on) for an app that asked for badges.
            UNUserNotificationCenter::currentNotificationCenter().requestAuthorizationWithOptions_completionHandler(
                UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound | UNAuthorizationOptions::Badge,
                &done,
            );
        }
    }
}

fn add(p: &Post) {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&p.title));
    if !p.subtitle.is_empty() {
        content.setSubtitle(&NSString::from_str(&p.subtitle));
    }
    content.setBody(&NSString::from_str(&p.body));
    content.setThreadIdentifier(&NSString::from_str(&p.thread));
    if p.sound {
        content.setSound(Some(&UNNotificationSound::defaultSound()));
    }
    let mut keys = Vec::new();
    let mut values = Vec::new();
    if let Some(pane) = p.pane {
        keys.push(NSString::from_str(PANE_KEY));
        values.push(NSString::from_str(&pane.to_string()));
    }
    if let Some((agent, id)) = &p.session {
        keys.extend([NSString::from_str(AGENT_KEY), NSString::from_str(SESSION_KEY)]);
        values.extend([NSString::from_str(agent.name()), NSString::from_str(id)]);
    }
    let keys: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<&NSString> = values.iter().map(|v| &**v).collect();
    let info = NSDictionary::<NSString, NSString>::from_slices(&keys, &values);
    // SAFETY: a dictionary of strings is a valid userInfo (a property-list dictionary).
    unsafe { content.setUserInfo(Retained::cast_unchecked::<NSDictionary>(info).as_ref()) };
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(&p.id), &content, None);
    UNUserNotificationCenter::currentNotificationCenter().addNotificationRequest_withCompletionHandler(&request, None);
}

#[cfg(test)]
mod tests {
    use super::is_app_bundle;

    #[test]
    fn a_bundle_needs_an_id_and_an_app_path() {
        assert!(is_app_bundle(Some("com.gilvt.app"), "/Applications/Gilvt.app"));
        assert!(is_app_bundle(Some("com.gilvt.app"), "/Users/u/gilvt/target/debug/Gilvt.app/"));
        // `cargo run`: the main bundle is the binary's directory and has no identifier.
        assert!(!is_app_bundle(None, "/Users/u/gilvt/target/debug"));
        assert!(!is_app_bundle(Some(""), "/Applications/Gilvt.app"));
        // An embedded __info_plist gives a bare binary an id, but not a bundle.
        assert!(!is_app_bundle(Some("com.gilvt.app"), "/Users/u/gilvt/target/debug"));
        assert!(!is_app_bundle(Some("com.gilvt.app"), "/private/tmp/x/target/debug/Gilvt.app"));
        assert!(!is_app_bundle(Some("com.gilvt.app"), "/tmp/Gilvt.app"));
    }
}
