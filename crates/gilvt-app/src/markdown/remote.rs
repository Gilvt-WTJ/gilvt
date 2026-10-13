//! A remote document's links and images (R2a §8.3): drawn first as if every file link existed and every image were
//! loading, then one batched `Stat` (all link candidates and image paths, 256 per request, sent together) decides
//! them; images that exist, are a supported format and fit 5 MB are read with `ReadFile` and cached on this Mac by
//! (host, path, mtime, size).

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gilvt_ipc::remote::{StatEntry, MAX_PREVIEW_BYTES};
use gpui::Context;

use gilvt_agent::host::HostPath;

use super::images::{self, Image};
use super::links::{self, Parsed};
use super::MdDoc;
use crate::preview_view::PreviewView;
use crate::remote::fs::{too_large_text, RemoteFs};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemoteProbes {
    /// Destination as written → files to try, in order.
    pub links: Vec<(String, Vec<PathBuf>)>,
    /// `src` → the remote path.
    pub images: Vec<(String, PathBuf)>,
}

impl RemoteProbes {
    /// Every path to `Stat`, without duplicates, links first.
    pub fn paths(&self) -> Vec<PathBuf> {
        let mut seen = HashSet::new();
        self.links.iter().flat_map(|(_, c)| c.iter()).chain(self.images.iter().map(|(_, p)| p)).filter(|p| seen.insert((*p).clone())).cloned().collect()
    }
}

/// What the host said, over what the document was built with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MdOverrides {
    pub missing_links: HashSet<String>,
    pub link_targets: HashMap<String, PathBuf>,
    pub images: HashMap<String, Image>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetch {
    pub src: String,
    pub path: PathBuf,
    pub mtime_ns: u64,
    pub size: u64,
}

pub fn apply_stats(probes: &RemoteProbes, paths: &[PathBuf], stats: &[StatEntry]) -> (MdOverrides, Vec<Fetch>) {
    let by_path: HashMap<&Path, &StatEntry> = paths.iter().map(PathBuf::as_path).zip(stats).collect();
    let mut o = MdOverrides::default();
    for (dest, candidates) in &probes.links {
        match candidates.iter().find(|c| by_path.get(c.as_path()).is_some_and(|s| s.exists)) {
            Some(found) => {
                o.link_targets.insert(dest.clone(), found.clone());
            }
            // A candidate the host could not look at (EACCES…) may be there: the link stays as drawn and a click
            // says what is wrong.
            None if candidates.iter().any(|c| by_path.get(c.as_path()).is_some_and(|s| s.error.is_some())) => {}
            None => {
                o.missing_links.insert(dest.clone());
            }
        }
    }
    let t = crate::i18n::text;
    let mut fetch = Vec::new();
    for (src, path) in &probes.images {
        let unavailable = |reason| Image::Unavailable { path: src.clone(), reason };
        match by_path.get(path.as_path()) {
            Some(s) if s.error.is_some() => {
                o.images.insert(src.clone(), unreadable(src));
            }
            Some(s) if s.exists && s.is_file => {
                if !images::supported(path) {
                    o.images.insert(src.clone(), unavailable(t("不支持的图片格式", "Unsupported image format")));
                } else if s.size > MAX_PREVIEW_BYTES {
                    o.images.insert(src.clone(), unavailable(too_large_text()));
                } else {
                    fetch.push(Fetch { src: src.clone(), path: path.clone(), mtime_ns: s.mtime_ns, size: s.size });
                }
            }
            _ => {
                o.images.insert(src.clone(), unavailable(t("找不到图片", "Image not found")));
            }
        }
    }
    (o, fetch)
}

/// The Stat failed: links stay as drawn (a click then says what is wrong), but no image may stay "reading" for good.
pub fn probe_failed(probes: &RemoteProbes) -> MdOverrides {
    let images = probes.images.iter().map(|(src, _)| (src.clone(), unreadable(src))).collect();
    MdOverrides { images, ..MdOverrides::default() }
}

fn unreadable(src: &str) -> Image {
    Image::Unavailable { path: src.to_string(), reason: crate::i18n::text("无法读取图片", "Could not read image") }
}

/// An error placeholder names the image as the document wrote it, not its cache file on this Mac.
fn as_written(src: &str, image: Image) -> Image {
    match image {
        Image::Unavailable { reason, .. } => Image::Unavailable { path: src.to_string(), reason },
        other => other,
    }
}

/// Image reads in flight at once for one document: a page of screenshots must not fill the bridge (and the hooks
/// behind it) with megabytes of base64 at the same moment.
pub const MAX_IMAGE_READS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadStep {
    Start(usize),
    Await(usize),
}

/// In which order `n` reads are started and awaited, never more than `limit` started and not yet awaited.
pub fn read_schedule(n: usize, limit: usize) -> Vec<ReadStep> {
    let limit = limit.max(1);
    let mut steps = Vec::with_capacity(2 * n);
    for i in 0..n {
        if i >= limit {
            steps.push(ReadStep::Await(i - limit));
        }
        steps.push(ReadStep::Start(i));
    }
    steps.extend((n.saturating_sub(limit)..n).map(ReadStep::Await));
    steps
}

pub fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("gilvt-remote-images").join(std::process::id().to_string())
}

pub fn cache_file(host: &str, f: &Fetch) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (host, &f.path, f.mtime_ns, f.size).hash(&mut h);
    let ext = f.path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    cache_dir().join(format!("{:016x}.{ext}", h.finish()))
}


/// What following `dest` in a remote document does. Never yields a Mac path: a file target is always on `host`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteNav {
    External(String),
    Anchor(String),
    Missing(String),
    /// The Stat already said no candidate exists.
    NoSuchFile,
    Open { target: HostPath, anchor: Option<String>, line: Option<u32> },
}

pub fn remote_nav(host: &str, file: &Path, dest: &str, repo_root: Option<&Path>, overrides: &MdOverrides) -> RemoteNav {
    match links::parse(dest, file.parent(), repo_root) {
        Parsed::External(u) => RemoteNav::External(u),
        Parsed::Anchor(a) => RemoteNav::Anchor(a),
        Parsed::Missing(m) => RemoteNav::Missing(m),
        Parsed::Paths { .. } if overrides.missing_links.contains(dest) => RemoteNav::NoSuchFile,
        Parsed::Paths { candidates, written, anchor, line } => {
            // Before the Stat answered, the first candidate (a missing file then says so when it loads).
            match overrides.link_targets.get(dest).cloned().or_else(|| candidates.into_iter().next()) {
                Some(path) => RemoteNav::Open { target: HostPath::remote(host, path), anchor, line },
                None => RemoteNav::Missing(written),
            }
        }
    }
}

impl PreviewView {
    /// After a remote Markdown document is shown: one batched `Stat`, then the images it needs (all requests in
    /// flight together). Answers for a document that has since been replaced are dropped (`Rc::ptr_eq`).
    pub(crate) fn probe_remote_markdown(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.host_path().and_then(|p| p.remote_host()).map(str::to_string) else { return };
        let Some(doc) = self.md.as_ref().map(|md| md.doc.clone()) else { return };
        let paths = doc.probes.paths();
        if paths.is_empty() {
            return;
        }
        let fs = RemoteFs::new(&host, cx);
        let stat = fs.stat(paths.clone(), cx);
        self._probe = Some(cx.spawn(async move |this, cx| {
            let (overrides, fetches) = match stat.await {
                Ok(stats) => apply_stats(&doc.probes, &paths, &stats),
                Err(_) => (probe_failed(&doc.probes), Vec::new()),
            };
            if this.update(cx, |v, cx| v.set_md_overrides(&doc, overrides, cx)).is_err() {
                return;
            }
            // At most `MAX_IMAGE_READS` `ReadFile`s in flight: each is created only when its turn comes.
            let mut started: HashMap<usize, (PathBuf, Option<gpui::Task<Result<crate::remote::fs::FileRead, crate::remote::fs::FsFailure>>>)> = HashMap::new();
            for step in read_schedule(fetches.len(), MAX_IMAGE_READS) {
                let i = match step {
                    ReadStep::Start(i) => {
                        let f = &fetches[i];
                        let cached = cache_file(&host, f);
                        let path = f.path.clone();
                        let fs = fs.clone();
                        let read = if cached.is_file() {
                            None
                        } else {
                            match this.update(cx, |_, cx| fs.read_file(path, MAX_PREVIEW_BYTES, cx)) {
                                Ok(t) => Some(t),
                                Err(_) => return,
                            }
                        };
                        started.insert(i, (cached, read));
                        continue;
                    }
                    ReadStep::Await(i) => i,
                };
                let Some((cached, read)) = started.remove(&i) else { continue };
                let f = &fetches[i];
                let image = match read {
                    Some(read) => match read.await {
                        Ok(r) => {
                            let path = cached.clone();
                            cx.background_executor()
                                .spawn(async move {
                                    let _ = std::fs::create_dir_all(cache_dir());
                                    match std::fs::write(&path, &r.bytes) {
                                        Ok(()) => images::from_bytes(path, &r.bytes),
                                        Err(_) => unreadable(""),
                                    }
                                })
                                .await
                        }
                        Err(e) => Image::Unavailable { path: f.src.clone(), reason: if matches!(e, crate::remote::fs::FsFailure::TooLarge { .. }) { too_large_text() } else { crate::i18n::text("无法读取图片", "Could not read image") } },
                    },
                    None => cx.background_executor().spawn(async move { std::fs::read(&cached).map(|b| images::from_bytes(cached.clone(), &b)).unwrap_or_else(|_| unreadable("")) }).await,
                };
                let image = as_written(&f.src, image);
                let _ = this.update(cx, |v, cx| v.set_md_image(&doc, &f.src, image, cx));
            }
        }));
    }

    fn set_md_overrides(&mut self, doc: &Rc<MdDoc>, overrides: MdOverrides, cx: &mut Context<Self>) {
        let Some(md) = self.md.as_mut().filter(|md| Rc::ptr_eq(&md.doc, doc)) else { return };
        let images = overrides.images.keys().filter_map(|src| doc.image_rows.get(src));
        let links = overrides.missing_links.iter().filter_map(|dest| doc.link_rows.get(dest));
        let changed: Vec<usize> = images.chain(links).flatten().copied().collect();
        md.overrides = Rc::new(overrides);
        for row in changed {
            md.remeasure_row(row);
        }
        cx.notify();
    }

    fn set_md_image(&mut self, doc: &Rc<MdDoc>, src: &str, image: Image, cx: &mut Context<Self>) {
        let Some(md) = self.md.as_mut().filter(|md| Rc::ptr_eq(&md.doc, doc)) else { return };
        Rc::make_mut(&mut md.overrides).images.insert(src.to_string(), image);
        for &row in doc.image_rows.get(src).into_iter().flatten() {
            md.remeasure_row(row);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(exists: bool, is_file: bool, size: u64) -> StatEntry {
        StatEntry { exists, is_file, size, mtime_ns: 7, error: None }
    }

    #[test]
    fn stats_decide_links_and_which_images_to_fetch() {
        let probes = RemoteProbes {
            links: vec![("a.md".into(), vec!["/r/a.md".into()]), ("/g.md".into(), vec!["/g.md".into(), "/r/g.md".into()]), ("gone.md".into(), vec!["/r/gone.md".into()])],
            images: vec![("ok.png".into(), "/r/ok.png".into()), ("big.png".into(), "/r/big.png".into()), ("x.webp".into(), "/r/x.webp".into()), ("no.png".into(), "/r/no.png".into())],
        };
        let paths = probes.paths();
        assert_eq!(paths.len(), 8);
        let stats: Vec<StatEntry> = paths
            .iter()
            .map(|p| match p.to_str().unwrap() {
                "/r/a.md" | "/r/g.md" => st(true, true, 10),
                "/r/ok.png" => st(true, true, 100),
                "/r/big.png" => st(true, true, MAX_PREVIEW_BYTES + 1),
                "/r/x.webp" => st(true, true, 5),
                _ => st(false, false, 0),
            })
            .collect();
        let (o, fetch) = apply_stats(&probes, &paths, &stats);
        assert_eq!(o.missing_links, HashSet::from(["gone.md".to_string()]));
        assert_eq!(o.link_targets["/g.md"], PathBuf::from("/r/g.md"), "the repository-rooted candidate");
        assert_eq!(fetch.iter().map(|f| f.src.as_str()).collect::<Vec<_>>(), ["ok.png"], "only a supported image within 5 MB is read");
        assert!(matches!(o.images["big.png"], Image::Unavailable { reason, .. } if reason == "文件太大，远端暂不支持"));
        assert!(matches!(o.images["x.webp"], Image::Unavailable { reason, .. } if reason == "不支持的图片格式"));
        assert!(matches!(o.images["no.png"], Image::Unavailable { reason, .. } if reason == "找不到图片"));
    }

    #[test]
    fn a_stat_failure_is_not_a_missing_file() {
        let probes = RemoteProbes { links: vec![("a.md".into(), vec!["/r/a.md".into()])], images: vec![("p.png".into(), "/r/p.png".into())] };
        let paths = probes.paths();
        let denied = StatEntry { error: Some("/r: Permission denied".into()), ..StatEntry::default() };
        let (o, fetch) = apply_stats(&probes, &paths, &[denied.clone(), denied]);
        assert!(o.missing_links.is_empty() && o.link_targets.is_empty(), "the link stays as drawn: a click says what is wrong");
        assert!(fetch.is_empty());
        assert!(matches!(o.images["p.png"], Image::Unavailable { reason, .. } if reason == "无法读取图片"), "not 找不到图片");
    }

    #[test]
    fn at_most_two_images_are_read_at_once() {
        let steps = read_schedule(5, MAX_IMAGE_READS);
        let (mut in_flight, mut most, mut started, mut awaited) = (0usize, 0usize, Vec::new(), Vec::new());
        for s in &steps {
            match *s {
                ReadStep::Start(i) => {
                    started.push(i);
                    in_flight += 1;
                    most = most.max(in_flight);
                }
                ReadStep::Await(i) => {
                    assert!(started.contains(&i) && !awaited.contains(&i));
                    awaited.push(i);
                    in_flight -= 1;
                }
            }
        }
        assert_eq!(MAX_IMAGE_READS, 2);
        assert_eq!(most, 2, "{steps:?}");
        assert_eq!((started, awaited), ((0..5).collect::<Vec<_>>(), (0..5).collect::<Vec<_>>()), "in document order");
        assert!(read_schedule(0, 2).is_empty());
    }

    #[test]
    fn following_a_link_in_a_remote_document_opens_the_remote_target() {
        let file = Path::new("/home/dev/r/docs/a.md");
        let root = Path::new("/home/dev/r");
        let mut o = MdOverrides::default();
        o.link_targets.insert("/g.md".into(), "/home/dev/r/g.md".into());
        o.missing_links.insert("gone.md".into());
        assert_eq!(
            remote_nav("h", file, "../main.go#L4", Some(root), &o),
            RemoteNav::Open { target: HostPath::remote("h", "/home/dev/r/docs/../main.go"), anchor: None, line: Some(4) }
        );
        let RemoteNav::Open { target, .. } = remote_nav("h", file, "/g.md", Some(root), &o) else { panic!() };
        assert_eq!(target, HostPath::remote("h", "/home/dev/r/g.md"), "the Stat-found candidate");
        assert_eq!(target.remote_host(), Some("h"), "never a Mac path");
        assert_eq!(remote_nav("h", file, "gone.md", Some(root), &o), RemoteNav::NoSuchFile);
        assert_eq!(remote_nav("h", file, "#top", None, &o), RemoteNav::Anchor("top".into()));
    }

    #[test]
    fn a_failed_stat_leaves_links_alone_and_no_image_pending() {
        let probes = RemoteProbes { links: vec![("a.md".into(), vec!["/r/a.md".into()])], images: vec![("p.png".into(), "/r/p.png".into())] };
        let o = probe_failed(&probes);
        assert!(o.missing_links.is_empty() && o.link_targets.is_empty(), "links stay as drawn");
        assert!(matches!(&o.images["p.png"], Image::Unavailable { path, .. } if path == "p.png"));
        let cached = Image::Unavailable { path: "/tmp/gilvt-remote-images/1/ab.png".into(), reason: "r" };
        assert!(matches!(as_written("img/p.png", cached), Image::Unavailable { path, .. } if path == "img/p.png"), "not the Mac cache path");
    }

    #[test]
    fn the_cache_key_is_the_host_path_and_its_stat() {
        let f = |mtime_ns| Fetch { src: "a.png".into(), path: "/r/a.png".into(), mtime_ns, size: 3 };
        assert_eq!(cache_file("h", &f(1)), cache_file("h", &f(1)));
        assert_ne!(cache_file("h", &f(1)), cache_file("h", &f(2)), "a changed file is fetched again");
        assert_ne!(cache_file("h", &f(1)), cache_file("other", &f(1)), "same path on another host");
        assert_eq!(cache_file("h", &f(1)).extension().unwrap(), "png");
    }
}
