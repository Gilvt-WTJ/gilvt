//! Hidden WKWebView running the bundled mermaid.js. WebKit is main-thread only; results come
//! back through completion handlers, which run on the main run loop.
//!
//! The scripts are injected as WKUserScripts rather than inlined into the page's HTML, so no
//! byte sequence in mermaid.min.js (`</script>`, `<!--`) can end or confuse a script element.
//! Requests run one at a time, each as a fresh `callAsyncJavaScript` issued from here:
//! chaining them inside the page hits WebKit's hidden-page timer throttling (~2 s per render).
//!
//! The page cannot leave: the CSP blocks loads, the navigation policy cancels every navigation
//! but the initial one, and with no WKUIDelegate `window.open` gets no web view (returns null).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;

use block2::{DynBlock, RcBlock};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
use objc2_foundation::{
    ns_string, NSDictionary, NSError, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use objc2_web_kit::{
    WKContentWorld, WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKUserScript,
    WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration,
};

use crate::Theme;

const MERMAID_JS: &str = include_str!("../assets/mermaid.min.js");
/// Defines `gilvtRender(src, theme)`, which resolves to the SVG or rejects with mermaid's error.
const RENDER_JS: &str = include_str!("../assets/render.js");
/// Body of the async function evaluated per request; `src` and `theme` are its arguments.
/// Errors are returned as data because WebKit reduces a rejection to a generic NSError.
const CALL_JS: &str = "try { return { svg: await gilvtRender(src, theme) }; } \
                       catch (e) { return { error: String(e?.message ?? e) }; }";

/// The page itself. Its CSP allows no loads but data: URLs (mermaid's image nodes would
/// otherwise fetch remote images); user scripts and `callAsyncJavaScript` are not subject to it.
const PAGE: &str = "<!doctype html><html><head><meta http-equiv=\"Content-Security-Policy\" \
                    content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:\">\
                    </head><body></body></html>";

/// A render's error when an image node's picture did not load: WebKit's rejection of
/// `HTMLImageElement.decode()`, which mermaid passes on. The CSP fails every remote picture so.
pub const IMAGE_LOAD_ERROR: &str = "Loading error.";

type Reply = async_channel::Sender<Result<String, String>>;

struct Job {
    source: String,
    theme: Theme,
    reply: Reply,
}

enum Page {
    Loading,
    Idle,
    /// One request is running in the page.
    Busy,
    /// The page failed to load, its web content process died, or the engine was dropped.
    Failed(String),
}

struct State {
    page: Page,
    /// Requests waiting for the page to load or for the running request to finish.
    queue: VecDeque<Job>,
    /// Waiting for the page to load.
    ready: Vec<async_channel::Sender<Result<(), String>>>,
}

define_class!(
    // Owns the request state. The web view holds its navigation delegate weakly, so `Engine`
    // keeps it alive; completion blocks retain it too, until WebKit releases them.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "GilvtMermaidNavigationDelegate"]
    #[ivars = RefCell<State>]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl WKNavigationDelegate for Delegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_policy(
            &self,
            _web: &WKWebView,
            action: &WKNavigationAction,
            decide: &DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let loading = matches!(self.ivars().borrow().page, Page::Loading);
            // SAFETY: property reads on the action WebKit passed in, on the main thread.
            let (main_frame, url) = unsafe {
                let main_frame = action.targetFrame().is_some_and(|frame| frame.isMainFrame());
                let url = action.request().URL().and_then(|url| url.absoluteString());
                (main_frame, url.map(|url| url.to_string()))
            };
            let allow = allows_navigation(loading, main_frame, url.as_deref());
            decide.call((if allow { WKNavigationActionPolicy::Allow } else { WKNavigationActionPolicy::Cancel },));
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, web: &WKWebView, _navigation: Option<&WKNavigation>) {
            let mut state = self.ivars().borrow_mut();
            if matches!(state.page, Page::Loading) {
                state.page = Page::Idle;
            }
            let ready = std::mem::take(&mut state.ready);
            drop(state);
            for waiter in ready {
                let _ = waiter.try_send(Ok(()));
            }
            self.pump(web);
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, _web: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.navigation_failed(error);
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(
            &self,
            _web: &WKWebView,
            _navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            self.navigation_failed(error);
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn process_terminated(&self, _web: &WKWebView) {
            self.fail("mermaid web process terminated".to_string());
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let state = State { page: Page::Loading, queue: VecDeque::new(), ready: Vec::new() };
        let this = Self::alloc(mtm).set_ivars(RefCell::new(state));
        unsafe { msg_send![super(this), init] }
    }

    fn enqueue(&self, web: &WKWebView, job: Job) {
        let mut state = self.ivars().borrow_mut();
        if let Page::Failed(message) = &state.page {
            let _ = job.reply.try_send(Err(message.clone()));
            return;
        }
        state.queue.push_back(job);
        drop(state);
        self.pump(web);
    }

    /// Starts the next queued request if the page is idle.
    fn pump(&self, web: &WKWebView) {
        let mut state = self.ivars().borrow_mut();
        if !matches!(state.page, Page::Idle) {
            return;
        }
        let Some(job) = state.queue.pop_front() else { return };
        state.page = Page::Busy;
        drop(state);
        self.dispatch(web, job);
    }

    /// Runs one request in the page. `src` and `theme` travel as JS arguments, never as script
    /// text. The completion block replies, then starts the next request.
    fn dispatch(&self, web: &WKWebView, job: Job) {
        let src = NSString::from_str(&job.source);
        let theme = NSString::from_str(job.theme.mermaid_name());
        let args: Retained<NSDictionary<NSString, AnyObject>> =
            NSDictionary::from_slices(&[ns_string!("src"), ns_string!("theme")], &[&*src, &*theme]);
        let reply = job.reply;
        let delegate = self.retain();
        // Weak: a strong reference would keep the web view alive while it holds this block.
        let web_ref = Weak::new(web);
        // WebKit copies the block and calls it once, on the main thread.
        let done = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
            // SAFETY: WebKit passes null or objects that stay valid for the duration of the call.
            let _ = reply.try_send(unsafe { outcome(value.as_ref(), error.as_ref()) });
            let mut state = delegate.ivars().borrow_mut();
            if matches!(state.page, Page::Busy) {
                state.page = Page::Idle;
            }
            drop(state);
            if let Some(web) = web_ref.load() {
                delegate.pump(&web);
            }
        });
        // SAFETY: main-thread call; `args` holds only NSStrings, which JS receives as strings.
        unsafe {
            web.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                &NSString::from_str(CALL_JS),
                Some(&args),
                None,
                &WKContentWorld::pageWorld(web.mtm()),
                Some(&done),
            );
        }
    }

    /// Fails the engine when the initial load failed. A navigation failing later is one the policy
    /// cancelled (the page stays as it was), so it is ignored.
    fn navigation_failed(&self, error: &NSError) {
        if navigation_failure_is_fatal(&self.ivars().borrow().page) {
            self.fail(format!("mermaid page failed to load: {}", error.localizedDescription()));
        }
    }

    /// Marks the page unusable and fails every queued request and `ready` waiter. A request
    /// already running in the page is answered by WebKit through its completion handler.
    fn fail(&self, message: String) {
        let mut state = self.ivars().borrow_mut();
        let queue = std::mem::take(&mut state.queue);
        let ready = std::mem::take(&mut state.ready);
        state.page = Page::Failed(message.clone());
        drop(state);
        for job in queue {
            let _ = job.reply.try_send(Err(message.clone()));
        }
        for waiter in ready {
            let _ = waiter.try_send(Err(message.clone()));
        }
    }
}

/// Whether a navigation may proceed: only `loadHTMLString` with no base URL, which loads
/// about:blank into the main frame before the page first finished loading. Script navigations
/// (`location`, form submits, links, `target=_blank` with no target frame) are all refused.
fn allows_navigation(page_loading: bool, main_frame: bool, url: Option<&str>) -> bool {
    page_loading && main_frame && url == Some("about:blank")
}

/// Only the initial load can fail the page: afterwards every navigation is one the policy refused.
fn navigation_failure_is_fatal(page: &Page) -> bool {
    matches!(page, Page::Loading)
}

/// Reads `{svg}` / `{error}` as returned by `CALL_JS`.
fn outcome(value: Option<&AnyObject>, error: Option<&NSError>) -> Result<String, String> {
    if let Some(error) = error {
        return Err(error.localizedDescription().to_string());
    }
    let dict = value.and_then(|value| value.downcast_ref::<NSDictionary>());
    let text = |key: &NSString| {
        let value = dict?.objectForKey(key)?;
        value.downcast_ref::<NSString>().map(|s| s.to_string())
    };
    match (text(ns_string!("svg")), text(ns_string!("error"))) {
        (Some(svg), _) => Ok(svg),
        (None, Some(message)) => Err(message),
        (None, None) => Err("mermaid returned no result".to_string()),
    }
}

/// Hidden WKWebView running mermaid.js. Main thread only (not Send). Requests made before the
/// page finished loading are queued.
pub struct Engine {
    web: Retained<WKWebView>,
    delegate: Retained<Delegate>,
}

impl Engine {
    pub fn new() -> Result<Engine, String> {
        let mtm = MainThreadMarker::new().ok_or("the mermaid engine must be created on the main thread")?;
        let delegate = Delegate::new(mtm);
        // SAFETY: plain WebKit calls on the main thread with valid, retained arguments.
        let web = unsafe {
            let config = WKWebViewConfiguration::new(mtm);
            let scripts = config.userContentController();
            for source in [MERMAID_JS, RENDER_JS] {
                let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    WKUserScript::alloc(mtm),
                    &NSString::from_str(source),
                    WKUserScriptInjectionTime::AtDocumentEnd,
                    true,
                );
                scripts.addUserScript(&script);
            }
            // Never shown; mermaid only needs a layout viewport to measure text.
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
            let web = WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config);
            web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            web.loadHTMLString_baseURL(&NSString::from_str(PAGE), None)
                .ok_or("the mermaid page did not start loading")?;
            web
        };
        Ok(Engine { web, delegate })
    }

    /// SVG markup, or mermaid's error message. The future is 'static and resolves on the main
    /// thread's run loop (completion handlers of WKWebView).
    pub fn render(
        &self,
        source: &str,
        theme: Theme,
    ) -> impl Future<Output = Result<String, String>> + 'static {
        let (reply, result) = async_channel::bounded(1);
        self.delegate.enqueue(&self.web, Job { source: source.to_string(), theme, reply });
        async move {
            let dropped = |_| Err("the mermaid engine was dropped".to_string());
            result.recv().await.unwrap_or_else(dropped)
        }
    }

    /// Resolves once the page has loaded (at once if it has), or with why it never will.
    /// Loading takes a while on a cold start (mermaid.js is 5.5 MB), so time it on its own.
    pub fn ready(&self) -> impl Future<Output = Result<(), String>> + 'static {
        let (tx, rx) = async_channel::bounded(1);
        let state = &mut *self.delegate.ivars().borrow_mut();
        match &state.page {
            Page::Loading => state.ready.push(tx),
            Page::Idle | Page::Busy => drop(tx.try_send(Ok(()))),
            Page::Failed(message) => drop(tx.try_send(Err(message.clone()))),
        }
        async move {
            let dropped = |_| Err("the mermaid engine was dropped".to_string());
            rx.recv().await.unwrap_or_else(dropped)
        }
    }

    /// False once the page failed to load or its web process died: such an engine answers
    /// every request with an error, so replace it.
    pub fn is_alive(&self) -> bool {
        !matches!(self.delegate.ivars().borrow().page, Page::Failed(_))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.delegate.fail("the mermaid engine was dropped".to_string());
        // SAFETY: main-thread WebKit calls on a live web view.
        unsafe {
            self.web.stopLoading();
            self.web.setNavigationDelegate(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{allows_navigation, navigation_failure_is_fatal, Page};

    #[test]
    fn only_a_failed_initial_load_fails_the_engine() {
        assert!(navigation_failure_is_fatal(&Page::Loading));
        assert!(!navigation_failure_is_fatal(&Page::Idle), "a refused navigation leaves the page as it was");
        assert!(!navigation_failure_is_fatal(&Page::Busy));
        assert!(!navigation_failure_is_fatal(&Page::Failed("x".into())));
    }

    #[test]
    fn only_the_initial_page_load_may_navigate() {
        assert!(allows_navigation(true, true, Some("about:blank")));
        assert!(!allows_navigation(false, true, Some("about:blank")), "once loaded, the page stays");
        assert!(!allows_navigation(true, true, Some("https://example.com/")));
        assert!(!allows_navigation(true, false, Some("about:blank")), "no frames, no new windows");
        assert!(!allows_navigation(true, true, None));
    }
}
