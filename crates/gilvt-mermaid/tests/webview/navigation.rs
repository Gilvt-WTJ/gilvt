//! What the engine's navigation policy relies on, checked on a probe web view loaded the way the
//! engine loads its page and refusing navigations the way it does: the initial load is
//! about:blank in the main frame, every script navigation asks the delegate first (so refusing
//! keeps the page), and with no WKUIDelegate `window.open` opens nothing. The engine's own
//! delegate is out of reach here; its rule is unit-tested in engine.rs.

use std::cell::RefCell;
use std::net::TcpListener;
use std::time::{Duration, Instant};

use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSError, NSObject, NSObjectProtocol, NSRunLoop, NSString};
use objc2_web_kit::{
    WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationType, WKWebView,
    WKWebViewConfiguration,
};

use super::{run_all, Render};

#[derive(Default)]
struct Seen {
    finished: bool,
    /// Every navigation the delegate was asked about: URL, into the main frame, type.
    asked: Vec<(String, bool, WKNavigationType)>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "GilvtMermaidNavigationProbe"]
    #[ivars = RefCell<Seen>]
    struct Probe;

    unsafe impl NSObjectProtocol for Probe {}

    unsafe impl WKNavigationDelegate for Probe {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_policy(
            &self,
            _web: &WKWebView,
            action: &WKNavigationAction,
            decide: &DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let mut seen = self.ivars().borrow_mut();
            // SAFETY: property reads on the action WebKit passed in, on the main thread.
            let asked = unsafe {
                let url = action.request().URL().and_then(|url| url.absoluteString()).map(|url| url.to_string());
                let main_frame = action.targetFrame().is_some_and(|frame| frame.isMainFrame());
                (url.unwrap_or_default(), main_frame, action.navigationType())
            };
            seen.asked.push(asked);
            let policy = if seen.finished { WKNavigationActionPolicy::Cancel } else { WKNavigationActionPolicy::Allow };
            drop(seen);
            decide.call((policy,));
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, _web: &WKWebView, _navigation: Option<&WKNavigation>) {
            self.ivars().borrow_mut().finished = true;
        }
    }
);

impl Probe {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RefCell::default());
        unsafe { msg_send![super(this), init] }
    }

    fn asked(&self) -> usize {
        self.ivars().borrow().asked.len()
    }
}

/// Runs `script` in the page; resolves to its result as a string.
fn evaluate(web: &WKWebView, script: &str) -> Render {
    let (tx, rx) = async_channel::bounded(1);
    let done = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
        // SAFETY: WebKit passes null or objects that stay valid for the duration of the call.
        let result = match unsafe { (value.as_ref(), error.as_ref()) } {
            (_, Some(error)) => Err(error.localizedDescription().to_string()),
            (Some(value), None) => Ok(value.downcast_ref::<NSString>().map(|s| s.to_string()).unwrap_or_default()),
            (None, None) => Ok(String::new()),
        };
        let _ = tx.try_send(result);
    });
    // SAFETY: main-thread call on a live web view.
    unsafe { web.evaluateJavaScript_completionHandler(&NSString::from_str(script), Some(&done)) };
    Box::pin(async move { rx.recv().await.unwrap_or_else(|_| Err("no completion".to_string())) })
}

/// Pumps the main run loop until `done` or `limit` passes.
fn pump_until(limit: Duration, done: impl Fn() -> bool) {
    let start = Instant::now();
    while !done() && start.elapsed() < limit {
        let slice = NSDate::dateWithTimeIntervalSinceNow(0.01);
        NSRunLoop::currentRunLoop().runMode_beforeDate(unsafe { NSDefaultRunLoopMode }, &slice);
    }
}

pub fn check(mtm: MainThreadMarker) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    listener.set_nonblocking(true).unwrap();
    let away = format!("http://{}/", listener.local_addr().unwrap());

    let probe = Probe::new(mtm);
    // SAFETY: plain WebKit calls on the main thread with valid, retained arguments.
    let web = unsafe {
        let config = WKWebViewConfiguration::new(mtm);
        let web = WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), Default::default(), &config);
        web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*probe)));
        web.loadHTMLString_baseURL(&NSString::from_str("<!doctype html><body></body>"), None).expect("load");
        web
    };
    pump_until(Duration::from_secs(10), || probe.ivars().borrow().finished);
    assert!(probe.ivars().borrow().finished, "the probe page did not load");
    let initial = probe.ivars().borrow().asked.clone();
    assert_eq!(initial, [("about:blank".to_string(), true, WKNavigationType::Other)], "the initial load");

    let marked = run_all(vec![evaluate(&web, "window.gilvtMarker = 'kept'")], Instant::now());
    assert!(marked[0].0.is_ok());
    // Name, script, and what the delegate is asked: URL, into the main frame, type.
    // `target=_blank` has no target frame, so the engine refuses it as not the main frame.
    let attempts = [
        ("location", "location.href = '{away}location'", "location", true, WKNavigationType::Other),
        (
            "form",
            "const f = document.createElement('form'); f.action = '{away}form'; document.body.append(f); f.submit()",
            "form?",
            true,
            WKNavigationType::FormSubmitted,
        ),
        (
            "_blank",
            "const a = document.createElement('a'); a.href = '{away}blank'; a.target = '_blank'; \
             document.body.append(a); a.click()",
            "blank",
            false,
            WKNavigationType::LinkActivated,
        ),
    ];
    for (name, script, path, main_frame, kind) in attempts {
        let before = probe.asked();
        let _ = run_all(vec![evaluate(&web, &script.replace("{away}", &away))], Instant::now());
        pump_until(Duration::from_secs(5), || probe.asked() > before);
        let seen = probe.ivars().borrow();
        let asked = seen.asked.get(before).unwrap_or_else(|| panic!("{name}: delegate not asked"));
        assert_eq!(asked, &(format!("{away}{path}"), main_frame, kind), "{name}");
        println!("navigation {name:<8} asked the delegate, refused");
    }

    let asked = probe.asked();
    let opened = run_all(vec![evaluate(&web, "String(window.open('about:blank'))")], Instant::now());
    assert_eq!(opened[0].0, Ok("null".to_string()), "window.open with no WKUIDelegate");
    let stayed = run_all(vec![evaluate(&web, "location.href + ' ' + window.gilvtMarker")], Instant::now());
    assert_eq!(stayed[0].0, Ok("about:blank kept".to_string()), "the page was replaced");
    assert_eq!(probe.asked(), asked, "window.open asked for a navigation");
    let requested = match listener.accept() {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => false,
        Err(e) => panic!("listener: {e}"),
    };
    assert!(!requested, "a refused navigation reached {away}");
    println!("navigation window.open returned null, page kept");
    // SAFETY: main-thread WebKit call on a live web view.
    unsafe { web.setNavigationDelegate(None) };
}
