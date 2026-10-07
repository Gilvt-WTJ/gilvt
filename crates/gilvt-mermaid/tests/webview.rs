//! Real WKWebView rendering of the sample diagrams (flowchart, sequence, class, state, ER,
//! gantt, pie, mindmap) in both themes; remote images and navigations the page must refuse.
//! `harness = false`: WebKit needs the process's main thread and its run loop, which libtest's
//! worker threads do not have.

#[path = "webview/navigation.rs"]
mod navigation;

use std::future::Future;
use std::net::TcpListener;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use gilvt_mermaid::{rasterize, Engine, Theme, IMAGE_LOAD_ERROR};
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSRunLoop};

const DIAGRAMS: &str = include_str!("../assets/diagrams.txt");
const DEADLINE: Duration = Duration::from_secs(30);

type Render = Pin<Box<dyn Future<Output = Result<String, String>>>>;

/// Polls every future to completion while pumping the main run loop, returning each result
/// with the time it resolved (since `start`). Panics past `DEADLINE`.
fn run_all(mut pending: Vec<Render>, start: Instant) -> Vec<(Result<String, String>, Duration)> {
    let mut done: Vec<Option<(Result<String, String>, Duration)>> = pending.iter().map(|_| None).collect();
    let mut cx = Context::from_waker(Waker::noop());
    let run_loop = NSRunLoop::currentRunLoop();
    while done.iter().any(Option::is_none) {
        assert!(start.elapsed() < DEADLINE, "renders did not finish within {DEADLINE:?}");
        let slice = NSDate::dateWithTimeIntervalSinceNow(0.01);
        run_loop.runMode_beforeDate(unsafe { NSDefaultRunLoopMode }, &slice);
        for (future, slot) in pending.iter_mut().zip(&mut done) {
            if slot.is_none() {
                if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
                    *slot = Some((result, start.elapsed()));
                }
            }
        }
    }
    done.into_iter().map(Option::unwrap).collect()
}

/// The SVG with its per-render element ids (`gilvt-mermaid-<n>`) blanked out.
fn without_ids(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(i) = rest.find("gilvt-mermaid-") {
        out.push_str(&rest[..i]);
        rest = rest[i + "gilvt-mermaid-".len()..].trim_start_matches(|c: char| c.is_ascii_digit());
    }
    out.push_str(rest);
    out
}

fn main() {
    let mtm = MainThreadMarker::new().expect("harness = false tests run on the main thread");
    let _app = NSApplication::sharedApplication(mtm);

    let diagrams: Vec<(&str, &str)> = DIAGRAMS
        .trim_end()
        .split("\n---\n")
        .map(|chunk| chunk.split_once('\n').expect("name line"))
        .collect();
    assert_eq!(diagrams.len(), 8);

    let start = Instant::now();
    let engine = Engine::new().expect("engine");
    let engine_ready = engine.ready();
    // Everything is requested before the page has loaded, exercising the pending queue.
    let mut renders: Vec<Render> = Vec::new();
    let mut labels = Vec::new();
    for theme in [Theme::Light, Theme::Dark] {
        for (name, source) in &diagrams {
            renders.push(Box::pin(engine.render(source, theme)));
            labels.push((name, theme));
        }
    }
    renders.push(Box::pin(engine.render("flowchart LR\n A -->", Theme::Light)));
    let ready: Render = Box::pin(async move { engine_ready.await.map(|()| String::new()) });
    renders.push(ready);
    let mut results = run_all(renders, start);
    let (loaded, _) = results.pop().unwrap();
    assert_eq!(loaded, Ok(String::new()), "the page must load");

    let (invalid, _) = results.pop().unwrap();
    let message = invalid.expect_err("invalid diagram must fail");
    assert!(!message.trim().is_empty());
    println!("invalid    error: {}", message.lines().next().unwrap_or_default());

    let mut previous = Duration::ZERO;
    let mut svgs = Vec::new();
    for ((result, at), (name, theme)) in results.into_iter().zip(labels) {
        let svg = result.unwrap_or_else(|e| panic!("{name} {theme:?}: {e}"));
        assert!(svg.contains("<svg"), "{name} {theme:?}: not an SVG");
        assert!(!svg.contains("foreignObject"), "{name} {theme:?}: uses foreignObject");
        let raster = rasterize(&svg, 2.0).unwrap_or_else(|e| panic!("{name} {theme:?}: {e}"));
        assert!(raster.width > 0 && raster.height > 0, "{name} {theme:?}: empty raster");
        println!(
            "{name:<10} {:<5}  done at {:>5} ms (+{:>4} ms)  {:>6} bytes  {}x{} px",
            format!("{theme:?}"),
            at.as_millis(),
            (at - previous).as_millis(),
            svg.len(),
            raster.width,
            raster.height,
        );
        previous = at;
        svgs.push(svg);
    }
    let (light, dark) = svgs.split_at(diagrams.len());
    for ((name, _), (light, dark)) in diagrams.iter().zip(light.iter().zip(dark)) {
        assert_ne!(without_ids(light), without_ids(dark), "{name}: theme had no effect");
    }

    assert!(engine.is_alive(), "a syntax error must not fail the engine");
    let loaded = engine.ready();
    let loaded = run_all(vec![Box::pin(async move { loaded.await.map(|()| String::new()) })], Instant::now());
    assert_eq!(loaded[0].0, Ok(String::new()), "a loaded page is ready at once");

    // Remote images in a diagram are never fetched: the page may load nothing but data: URLs.
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/x.png", listener.local_addr().unwrap());
    let source = format!("flowchart LR\n  A@{{ img: \"{url}\", label: \"remote\", pos: \"t\", w: 60, h: 60 }} --> B");
    let at = Instant::now();
    let (remote, took) = run_all(vec![Box::pin(engine.render(&source, Theme::Light))], at).pop().unwrap();
    // Give a late fetch a chance to show up.
    let settle = Instant::now();
    while settle.elapsed() < Duration::from_millis(300) {
        NSRunLoop::currentRunLoop().runMode_beforeDate(unsafe { NSDefaultRunLoopMode }, &NSDate::dateWithTimeIntervalSinceNow(0.05));
    }
    let fetched = match listener.accept() {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => false,
        Err(e) => panic!("listener: {e}"),
    };
    assert!(!fetched, "the page requested {url}");
    assert!(took < Duration::from_secs(2), "remote image render took {took:?}");
    // The app tells a remote image apart from a syntax error by this message.
    assert_eq!(remote, Err(IMAGE_LOAD_ERROR.to_string()), "remote image render");
    println!("remote img {IMAGE_LOAD_ERROR:?} in {} ms, no request made", took.as_millis());

    // The navigation checks run on a probe web view of their own (engine.rs unit-tests the engine's
    // rule); a render requested before them still completes, and the engine stays alive.
    let during: Render = Box::pin(engine.render("pie\n  \"n\" : 1", Theme::Light));
    navigation::check(mtm);
    let (during, _) = run_all(vec![during], Instant::now()).pop().unwrap();
    assert!(during.is_ok_and(|svg| svg.contains("<svg")), "render during navigation attempts");
    assert!(engine.is_alive());

    // Dropping the engine fails requests still waiting behind the running one.
    let running: Render = Box::pin(engine.render("pie\n  \"a\" : 1", Theme::Light));
    let waiting: Render = Box::pin(engine.render("pie\n  \"b\" : 1", Theme::Light));
    drop(engine);
    let (waiting, _) = run_all(vec![running, waiting], Instant::now()).pop().unwrap();
    assert_eq!(waiting, Err("the mermaid engine was dropped".to_string()));

    // So does waiting for a page that never got to load.
    let fresh = Engine::new().expect("engine");
    let loading = fresh.ready();
    drop(fresh);
    let loading = run_all(vec![Box::pin(async move { loading.await.map(|()| String::new()) })], Instant::now());
    assert_eq!(loading[0].0, Err("the mermaid engine was dropped".to_string()));
    println!("webview: {} renders ok in {} ms", svgs.len() + 1, start.elapsed().as_millis());
}
