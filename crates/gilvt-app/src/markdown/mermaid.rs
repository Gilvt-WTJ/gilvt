//! Mermaid diagrams in the rendered view. One app-wide `Mermaid` global turns sources into SVG
//! (cache first, then the WKWebView engine, created on demand and dropped when idle); each
//! preview rasterizes the SVGs of its document and shows them in place of the source.

mod bookkeeping;

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Instant;

use gpui::{App, AsyncApp, Context, FutureExt, Global, RenderImage, Task, Timeout, Window};
use gilvt_mermaid::{cache_key, rasterize, Cache, Engine, Theme};
use image::{Frame, RgbaImage};

use crate::markdown::{MdDoc, MdState};
use crate::preview_view::PreviewView;

use bookkeeping::{classify, to_straight_bgra, Answer, Failure, IdleClock, Outcome, Queue, Requests, IDLE, LOAD_TIMEOUT, TIMEOUT};

/// A diagram missing from the cache.
struct Job {
    key: String,
    source: String,
    theme: Theme,
}

/// The app-wide mermaid service.
pub struct Mermaid {
    cache: Arc<Cache>,
    requests: Requests<async_channel::Sender<Outcome>>,
    /// Cache misses, handed to the engine one at a time.
    queue: Queue<Job>,
    engine: Option<Engine>,
    clock: IdleClock,
    _idle: Option<Task<()>>,
}

impl Global for Mermaid {}

impl Mermaid {
    pub fn new(cache_dir: PathBuf) -> Mermaid {
        Mermaid {
            cache: Arc::new(Cache::new(cache_dir)),
            requests: Requests::default(),
            queue: Queue::default(),
            engine: None,
            clock: IdleClock::default(),
            _idle: None,
        }
    }

    /// The SVG of `source` in `theme`. Resolves on the main thread; concurrent requests for the
    /// same diagram share one render, and failures are not retried until `clear_failures`.
    pub fn svg(source: &str, theme: Theme, cx: &mut App) -> impl Future<Output = Outcome> + 'static {
        let key = cache_key(source, theme);
        let (tx, rx) = async_channel::bounded(1);
        let this = cx.global_mut::<Mermaid>();
        if let Some(failure) = this.requests.failure(&key) {
            let _ = tx.try_send(Err(failure));
        } else if this.requests.wait(&key, tx) {
            Self::start(key, source.to_string(), theme, cx);
        }
        async move { rx.recv().await.unwrap_or(Err(Failure::Engine)) }
    }

    /// Forgets failed diagrams, so the next request tries them again.
    pub fn clear_failures(cx: &mut App) {
        cx.global_mut::<Mermaid>().requests.clear_failures();
    }

    /// Looks `key` up in the cache (off the main thread), queues it for the engine on a miss.
    fn start(key: String, source: String, theme: Theme, cx: &mut App) {
        let cache = cx.global::<Mermaid>().cache.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |cx| {
            let cached = executor.spawn({
                let key = key.clone();
                async move { cache.get(&key) }
            });
            let cached = cached.await;
            let _ = cx.update_global::<Mermaid, _>(|this, cx| match cached {
                Some(svg) => this.answer(&key, Ok(svg.into())),
                None => {
                    this.queue.push(Job { key, source, theme });
                    this.pump(cx);
                }
            });
        })
        .detach();
    }

    /// Answers everyone waiting for `key`.
    fn answer(&mut self, key: &str, outcome: Outcome) {
        for waiter in self.requests.complete(key, &outcome) {
            let _ = waiter.try_send(outcome.clone());
        }
    }

    /// Starts the next queued diagram unless the engine is busy with one. Diagrams whose views
    /// all went away (closed, or now showing the other theme) are dropped unrendered.
    fn pump(&mut self, cx: &mut App) {
        let requests = &mut self.requests;
        let Some(job) = self.queue.next(|job| requests.still_wanted(&job.key, |waiter| !waiter.is_closed())) else {
            return;
        };
        self.clock.begin();
        cx.spawn(async move |cx| {
            let outcome = Self::run(&job, cx).await;
            let _ = cx.update_global::<Mermaid, _>(|this, cx| {
                if let Ok(svg) = &outcome {
                    let (cache, key, svg) = (this.cache.clone(), job.key.clone(), svg.clone());
                    cx.background_executor().spawn(async move { cache.put(&key, &svg) }).detach();
                }
                this.answer(&job.key, outcome);
                this.queue.finish();
                this.clock.end(Instant::now());
                this.pump(cx);
                this.drop_when_idle(cx);
            });
        })
        .detach();
    }

    /// Renders `job`, starting the engine first if needed. Loading the page has its own
    /// budget; the diagram's starts when the engine gets it.
    async fn run(job: &Job, cx: &mut AsyncApp) -> Outcome {
        let executor = cx.background_executor().clone();
        let ready = cx.update_global::<Mermaid, _>(|this, _| this.engine().map(Engine::ready));
        let loaded = match ready {
            Ok(Ok(ready)) => ready.with_timeout(LOAD_TIMEOUT, &executor).await.unwrap_or_else(|Timeout| {
                Err(format!("the page did not load within {LOAD_TIMEOUT:?}"))
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => return Err(Failure::Engine),
        };
        if let Err(e) = loaded {
            // No diagram can run without a page: fail those waiting too.
            eprintln!("gilvt: mermaid engine unavailable: {e}");
            let _ = cx.update_global::<Mermaid, _>(|this, _| {
                this.engine = None;
                for job in this.queue.drain() {
                    this.answer(&job.key, Err(Failure::Engine));
                }
            });
            return Err(Failure::Engine);
        }
        let render = cx.update_global::<Mermaid, _>(|this, _| this.engine.as_ref().map(|e| e.render(&job.source, job.theme)));
        let Ok(Some(render)) = render else { return Err(Failure::Engine) };
        let answer = match render.with_timeout(TIMEOUT, &executor).await {
            Ok(Ok(svg)) => Answer::Svg(svg),
            Ok(Err(message)) => Answer::Rejected(message),
            Err(Timeout) => Answer::TimedOut,
        };
        cx.update_global::<Mermaid, _>(|this, _| {
            let alive = this.engine.as_ref().is_some_and(Engine::is_alive);
            match &answer {
                Answer::TimedOut => eprintln!("gilvt: mermaid render timed out after {TIMEOUT:?}"),
                Answer::Rejected(message) if !alive => eprintln!("gilvt: mermaid engine failed: {message}"),
                _ => {}
            }
            let (outcome, keep) = classify(answer, alive, &job.source);
            if !keep {
                this.engine = None;
            }
            outcome
        })
        .unwrap_or(Err(Failure::Engine))
    }

    /// The engine, created when there is none or the one there died.
    fn engine(&mut self) -> Result<&Engine, String> {
        // A dead engine is dropped before its replacement loads.
        let engine = match self.engine.take().filter(Engine::is_alive) {
            Some(engine) => engine,
            None => Engine::new()?,
        };
        Ok(self.engine.insert(engine))
    }

    /// Drops the engine once it has been idle for `IDLE`.
    fn drop_when_idle(&mut self, cx: &mut App) {
        if !self.clock.is_idle() || self.engine.is_none() {
            return;
        }
        self._idle = Some(cx.spawn(async move |cx| {
            cx.background_executor().timer(IDLE).await;
            let _ = cx.update_global::<Mermaid, _>(|this, _| {
                if this.clock.expired(Instant::now()) {
                    this.engine = None;
                }
            });
        }));
    }
}

/// A diagram as the rendered view shows it.
#[derive(Clone)]
pub enum Shown {
    Rendering,
    /// `width` × `height` is the diagram's natural size in pixels; `scale` what it was rasterized at.
    Ready { image: Arc<RenderImage>, width: f32, height: f32, scale: f32 },
    /// mermaid's message.
    Error(String),
    /// An image node points at an http(s) URL, which is never loaded.
    RemoteImage,
    /// The engine failed or timed out.
    Failed,
}

/// The diagrams of a preview's document by cache key; kept across reloads of the same file, so
/// unchanged diagrams do not flash back to the placeholder.
#[derive(Default)]
pub struct Diagrams {
    pub shown: Rc<HashMap<String, Shown>>,
    /// Rows to re-measure when a diagram arrives.
    rows: HashMap<String, Vec<usize>>,
    tasks: HashMap<String, Task<()>>,
    /// Document, theme and scale factor `shown` was last brought up to date for.
    synced: Option<(Weak<MdDoc>, Theme, f32)>,
}

impl Diagrams {
    /// Forgets every diagram and frees the sprite-atlas copies of their images (the view is
    /// going away or no longer shows a Markdown document). Not for use inside a window update.
    pub fn release(&mut self, cx: &mut App) {
        for shown in std::mem::take(&mut self.shown).values() {
            if let Shown::Ready { image, .. } = shown {
                cx.drop_image(image.clone(), None);
            }
        }
        self.rows.clear();
        self.tasks.clear();
        self.synced = None;
    }
}

/// Loads the system fonts rasterizing needs (a few hundred ms), so the first diagram does not
/// wait for them. Blocking.
pub fn preload() {
    let _ = rasterize(r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#, 1.0);
}

/// Rasterized SVG as a gpui image. Blocking.
fn raster_image(svg: &str, scale: f32) -> Result<Shown, String> {
    let mut raster = rasterize(svg, scale)?;
    to_straight_bgra(&mut raster.rgba);
    let (w, h) = (raster.width, raster.height);
    let buffer = RgbaImage::from_raw(w, h, raster.rgba).ok_or("raster size mismatch")?;
    let image = Arc::new(RenderImage::new([Frame::new(buffer)]));
    Ok(Shown::Ready { image, width: w as f32 / scale, height: h as f32 / scale, scale })
}

impl PreviewView {
    /// Brings the diagrams up to date with the shown document, `theme` and the window's scale:
    /// drops those no longer shown and requests the missing ones. Cheap when nothing changed.
    pub(crate) fn sync_diagrams(&mut self, theme: Theme, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.md.as_ref().map(|md| md.doc.clone()) else { return };
        let scale = window.scale_factor();
        let d = &mut self.diagrams;
        if d.synced.as_ref().is_some_and(|(w, t, s)| w.ptr_eq(&Rc::downgrade(&doc)) && *t == theme && *s == scale) {
            return;
        }
        let rescaled = d.synced.as_ref().is_some_and(|(.., s)| *s != scale);
        d.synced = Some((Rc::downgrade(&doc), theme, scale));
        let mut sources = HashMap::new();
        d.rows.clear();
        for (row, source) in doc.mermaid_blocks() {
            let key = cache_key(source, theme);
            d.rows.entry(key.clone()).or_default().push(row);
            sources.entry(key).or_insert(source);
        }
        let shown = Rc::make_mut(&mut d.shown);
        shown.retain(|key, s| {
            // Rasterized (or being rasterized) for another display: redo.
            let keep = sources.contains_key(key)
                && match s {
                    Shown::Ready { scale: at, .. } => *at == scale,
                    Shown::Rendering => !rescaled,
                    Shown::Error(_) | Shown::RemoteImage | Shown::Failed => true,
                };
            if let (false, Shown::Ready { image, .. }) = (keep, &s) {
                cx.drop_image(image.clone(), Some(window));
            }
            keep
        });
        d.tasks.retain(|key, _| shown.contains_key(key));
        for (key, source) in sources {
            if shown.contains_key(&key) {
                continue;
            }
            shown.insert(key.clone(), Shown::Rendering);
            let svg = Mermaid::svg(source, theme, cx);
            let executor = cx.background_executor().clone();
            let task = cx.spawn({
                let key = key.clone();
                async move |this, cx| {
                    let next = match svg.await {
                        Ok(svg) => executor.spawn(async move { raster_image(&svg, scale) }).await.unwrap_or_else(|e| {
                            eprintln!("gilvt: cannot draw mermaid diagram: {e}");
                            Shown::Failed
                        }),
                        Err(Failure::Syntax(message)) => Shown::Error(message),
                        Err(Failure::RemoteImage) => Shown::RemoteImage,
                        Err(Failure::Engine) => Shown::Failed,
                    };
                    let _ = this.update(cx, |view, cx| view.diagram_done(key, next, cx));
                }
            });
            d.tasks.insert(key, task);
        }
    }

    /// Shows a finished diagram and re-measures the rows it is in.
    fn diagram_done(&mut self, key: String, next: Shown, cx: &mut Context<Self>) {
        let d = &mut self.diagrams;
        if d.tasks.remove(&key).is_none() {
            return;
        }
        Rc::make_mut(&mut d.shown).insert(key.clone(), next);
        // `rows` belong to the document last synced; a newer one is measured afresh anyway.
        let synced = |md: &&MdState| d.synced.as_ref().is_some_and(|(doc, ..)| doc.ptr_eq(&Rc::downgrade(&md.doc)));
        if let Some(md) = self.md.as_ref().filter(synced) {
            for &row in d.rows.get(&key).into_iter().flatten() {
                md.remeasure_row(row);
            }
        }
        cx.notify();
    }

    /// `R`: failed diagrams get another try.
    pub(crate) fn retry_diagrams(&mut self, cx: &mut App) {
        Mermaid::clear_failures(cx);
        let d = &mut self.diagrams;
        Rc::make_mut(&mut d.shown).retain(|_, s| !matches!(s, Shown::Error(_) | Shown::RemoteImage | Shown::Failed));
        d.synced = None;
    }
}
