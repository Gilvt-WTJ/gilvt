//! The editor's live preview (spec 2026-10-05): which previews a file type offers, and how the editor
//! buffer becomes something Quick Look's views can draw. No gpui here; everything is unit-tested.

use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use gilvt_viewer::Preview;

/// Quiet time after the last keystroke before the preview is rebuilt.
pub const DEBOUNCE_MS: u64 = 200;
pub fn too_large_banner() -> &'static str {
    crate::i18n::text("文件太大，未实时预览 · 保存后更新", "File too large for live preview · updates on save")
}
/// The editor's highlight limits (`editor::model`): past them the preview only follows saves.
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_LINES: usize = 50_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provider {
    /// `.md` / `.markdown`: the typeset document.
    Rendered,
    /// `.mmd` / `.mermaid`: the whole file as one diagram.
    Diagram,
    /// `.svg`: the picture.
    Image,
    /// Any text: unsaved changes against the version on disk.
    Changes,
}

impl Provider {
    pub fn id(self) -> &'static str {
        match self {
            Provider::Rendered => "rendered",
            Provider::Diagram => "diagram",
            Provider::Image => "image",
            Provider::Changes => "changes",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Rendered => crate::i18n::text("渲染", "Rendered"),
            Provider::Diagram => crate::i18n::text("图表", "Diagram"),
            Provider::Image => crate::i18n::text("图片", "Image"),
            Provider::Changes => crate::i18n::text("改动", "Changes"),
        }
    }
}

fn extension(file_name: &str) -> Option<String> {
    Path::new(file_name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase())
}

/// The key a provider choice is remembered under: the lower-cased extension, else the lower-cased name.
pub fn pref_key(file_name: &str) -> String {
    extension(file_name).unwrap_or_else(|| file_name.to_ascii_lowercase())
}

/// What a file of this name can be previewed as; the first is the default. Never empty.
pub fn providers_for(file_name: &str) -> Vec<Provider> {
    match extension(file_name).as_deref() {
        Some("md" | "markdown") => vec![Provider::Rendered, Provider::Changes],
        Some("mmd" | "mermaid") => vec![Provider::Diagram, Provider::Changes],
        Some("svg") => vec![Provider::Image, Provider::Changes],
        _ => vec![Provider::Changes],
    }
}

/// The remembered provider for this file type if it is still on offer, else the first one.
pub fn default_provider(file_name: &str, prefs: &BTreeMap<String, String>) -> Provider {
    let list = providers_for(file_name);
    prefs.get(&pref_key(file_name)).and_then(|id| list.iter().copied().find(|p| p.id() == id)).unwrap_or(list[0])
}

/// The provider after `current` in `list` (wrapping); the first when `current` is not in it.
pub fn next_provider(list: &[Provider], current: Provider) -> Provider {
    match list.iter().position(|&p| p == current) {
        Some(i) => list[(i + 1) % list.len()],
        None => list[0],
    }
}

pub fn too_large(bytes: usize, lines: usize) -> bool {
    bytes > MAX_BYTES || lines > MAX_LINES
}

/// What a click on the header's 「预览」 button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewClick {
    Open,
    Cycle,
    Close,
}

pub fn click_action(open: bool, providers: usize) -> PreviewClick {
    match (open, providers > 1) {
        (false, _) => PreviewClick::Open,
        (true, true) => PreviewClick::Cycle,
        (true, false) => PreviewClick::Close,
    }
}

/// Everything a provider needs, owned so it can go to a background thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveInput {
    pub provider: Provider,
    /// The file the buffer belongs to; None for an unsaved buffer.
    pub path: Option<PathBuf>,
    /// The whole buffer, unsaved edits included.
    pub text: String,
    /// The editor pane's id: names this editor's SVG cache file.
    pub slot: u64,
}

/// Where the SVG pictures of unsaved buffers are written (one directory per process: slots are pane ids, so
/// two gilvt processes must not share one) for the Markdown image path to read.
pub fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("gilvt-live-preview").join(std::process::id().to_string())
}

/// A tilde fence longer than any run of tildes starting a line of `text` (at least 4).
fn fence_for(text: &str) -> String {
    let longest = text.lines().map(|l| l.trim_start().chars().take_while(|&c| c == '~').count()).max().unwrap_or(0);
    "~".repeat((longest + 1).max(4))
}

fn diagram_markdown(text: &str) -> String {
    let fence = fence_for(text);
    format!("{fence}mermaid\n{}\n{fence}\n", text.trim_end_matches('\n'))
}

/// Writes `svg` to `cache_dir` (one file per slot: older files of the slot are removed) and returns the
/// Markdown that shows it. The path goes between `<>` because it may contain spaces.
fn image_markdown(svg: &str, slot: u64, cache_dir: &Path) -> Result<String, String> {
    std::fs::create_dir_all(cache_dir).map_err(|e| format!("{}: {e}", cache_dir.display()))?;
    let mut hasher = DefaultHasher::new();
    svg.hash(&mut hasher);
    let prefix = format!("live-{slot}-");
    let name = format!("{prefix}{:016x}.svg", hasher.finish());
    if let Ok(entries) = std::fs::read_dir(cache_dir) {
        for entry in entries.flatten() {
            let old = entry.file_name().to_string_lossy().into_owned();
            if old.starts_with(&prefix) && old != name {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let path = cache_dir.join(name);
    std::fs::write(&path, svg).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!("![svg](<{}>)\n", path.display()))
}

/// Turns the buffer into a `Preview` the Quick Look views can draw. Blocking (may read the file on disk
/// or write the SVG cache): call it off the main thread.
pub fn build_preview(input: &LiveInput, cache_dir: &Path) -> Result<Preview, String> {
    let md = || Some("md".to_string());
    let path = input.path.as_deref();
    match input.provider {
        Provider::Rendered => Ok(Preview::from_texts(path, &input.text, input.text.clone(), md())),
        Provider::Diagram => Ok(Preview::from_texts(path, "", diagram_markdown(&input.text), md())),
        Provider::Image => Ok(Preview::from_texts(None, "", image_markdown(&input.text, input.slot, cache_dir)?, md())),
        Provider::Changes => {
            let on_disk = path.and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
            Ok(Preview::from_texts(path, &on_disk, input.text.clone(), None))
        }
    }
}

/// Which preview pane belongs to which editor pane. Pane ids are `PaneId`s.
#[derive(Debug, Default)]
pub struct LivePairs {
    by_editor: BTreeMap<u64, u64>,
}

impl LivePairs {
    pub fn insert(&mut self, editor: u64, preview: u64) {
        self.by_editor.insert(editor, preview);
    }

    pub fn preview_of(&self, editor: u64) -> Option<u64> {
        self.by_editor.get(&editor).copied()
    }

    pub fn editor_of(&self, preview: u64) -> Option<u64> {
        self.by_editor.iter().find(|(_, &p)| p == preview).map(|(&e, _)| e)
    }

    /// Forgets the pair of `editor`; returns its preview.
    pub fn remove_editor(&mut self, editor: u64) -> Option<u64> {
        self.by_editor.remove(&editor)
    }

    /// Forgets the pair of `preview`; returns its editor.
    pub fn remove_preview(&mut self, preview: u64) -> Option<u64> {
        let editor = self.editor_of(preview)?;
        self.by_editor.remove(&editor);
        Some(editor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::MdDoc;

    fn input(provider: Provider, path: Option<&str>, text: &str) -> LiveInput {
        LiveInput { provider, path: path.map(PathBuf::from), text: text.to_string(), slot: 7 }
    }

    #[test]
    fn provider_ids_and_labels_are_pinned() {
        let all = [Provider::Rendered, Provider::Diagram, Provider::Image, Provider::Changes];
        assert_eq!(all.map(Provider::id), ["rendered", "diagram", "image", "changes"]);
        assert_eq!(all.map(Provider::label), ["渲染", "图表", "图片", "改动"]);
    }

    #[test]
    fn providers_follow_the_file_type() {
        assert_eq!(providers_for("a.md"), [Provider::Rendered, Provider::Changes]);
        assert_eq!(providers_for("README.MARKDOWN"), [Provider::Rendered, Provider::Changes]);
        assert_eq!(providers_for("flow.mmd"), [Provider::Diagram, Provider::Changes]);
        assert_eq!(providers_for("flow.mermaid"), [Provider::Diagram, Provider::Changes]);
        assert_eq!(providers_for("logo.svg"), [Provider::Image, Provider::Changes]);
        assert_eq!(providers_for("main.rs"), [Provider::Changes]);
        assert_eq!(providers_for("Makefile"), [Provider::Changes]);
    }

    #[test]
    fn unsaved_buffer_only_offers_changes() {
        assert_eq!(providers_for("未命名"), [Provider::Changes]);
    }

    #[test]
    fn preference_picks_a_listed_provider_else_the_first() {
        let mut prefs = BTreeMap::new();
        assert_eq!(default_provider("a.md", &prefs), Provider::Rendered);
        prefs.insert("md".to_string(), "changes".to_string());
        assert_eq!(default_provider("b.MD", &prefs), Provider::Changes);
        // A stale or foreign value is ignored.
        prefs.insert("md".to_string(), "diagram".to_string());
        assert_eq!(default_provider("a.md", &prefs), Provider::Rendered);
        prefs.insert("md".to_string(), "nonsense".to_string());
        assert_eq!(default_provider("a.md", &prefs), Provider::Rendered);
        assert_eq!(pref_key("Makefile"), "makefile");
        assert_eq!(pref_key("a.B.Md"), "md");
    }

    #[test]
    fn next_provider_wraps() {
        let list = providers_for("a.md");
        assert_eq!(next_provider(&list, Provider::Rendered), Provider::Changes);
        assert_eq!(next_provider(&list, Provider::Changes), Provider::Rendered);
        assert_eq!(next_provider(&providers_for("a.rs"), Provider::Changes), Provider::Changes);
        // Not in the list (the file was renamed): start from the first.
        assert_eq!(next_provider(&list, Provider::Image), Provider::Rendered);
    }

    #[test]
    fn click_opens_cycles_or_closes() {
        assert_eq!(click_action(false, 2), PreviewClick::Open);
        assert_eq!(click_action(false, 1), PreviewClick::Open);
        assert_eq!(click_action(true, 2), PreviewClick::Cycle);
        assert_eq!(click_action(true, 1), PreviewClick::Close);
    }

    #[test]
    fn too_large_counts_lines_the_way_the_buffer_does() {
        // 50 000 lines ending in a newline: the buffer counts 50 001 lines, so it is past the limit.
        let text = "x\n".repeat(50_000);
        assert!(too_large(text.len(), text.split('\n').count()));
        let text = "x\n".repeat(49_999);
        assert!(!too_large(text.len(), text.split('\n').count()));
    }

    #[test]
    fn too_large_is_strictly_above_either_limit() {
        assert!(!too_large(2 * 1024 * 1024, 50_000));
        assert!(too_large(2 * 1024 * 1024 + 1, 10));
        assert!(too_large(10, 50_001));
    }

    #[test]
    fn rendered_builds_a_markdown_document_without_change_marks() {
        let p = build_preview(&input(Provider::Rendered, Some("/tmp/x/a.md"), "# T\n\ntext\n"), Path::new("/nonexistent")).unwrap();
        let doc = MdDoc::build(&p).expect("markdown");
        assert!(doc.changes.is_none());
        assert_eq!(doc.blocks.len(), 2);
    }

    #[test]
    fn diagram_wraps_the_whole_text_in_one_mermaid_block() {
        let p = build_preview(&input(Provider::Diagram, Some("/tmp/x/flow.mmd"), "graph TD\n  A-->B\n"), Path::new("/nonexistent")).unwrap();
        let doc = MdDoc::build(&p).expect("markdown");
        assert_eq!(doc.mermaid_blocks().len(), 1);
        assert_eq!(doc.mermaid_blocks()[0].1, "graph TD\n  A-->B");
    }

    #[test]
    fn fence_outgrows_the_text() {
        let text = "graph TD\n~~~~\nA-->B\n";
        let p = build_preview(&input(Provider::Diagram, None, text), Path::new("/nonexistent")).unwrap();
        let doc = MdDoc::build(&p).expect("markdown");
        assert_eq!(doc.mermaid_blocks().len(), 1);
        assert!(doc.mermaid_blocks()[0].1.contains("A-->B"));
        assert_eq!(fence_for("a\n~~~~~~\nb"), "~~~~~~~");
        assert_eq!(fence_for("plain"), "~~~~");
    }

    #[test]
    fn empty_text_builds_for_every_provider() {
        let dir = tempfile::tempdir().unwrap();
        for provider in [Provider::Rendered, Provider::Diagram, Provider::Image, Provider::Changes] {
            let p = build_preview(&input(provider, None, ""), dir.path());
            assert!(p.is_ok(), "{provider:?}: {p:?}");
        }
    }

    const SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"20\"/>";

    #[test]
    fn svg_cache_path_with_space_loads() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("my cache");
        let p = build_preview(&input(Provider::Image, Some("/tmp/x/logo.svg"), SVG), &cache).unwrap();
        let doc = MdDoc::build(&p).expect("markdown");
        let images: Vec<_> = doc.images.values().collect();
        assert_eq!(images.len(), 1);
        assert!(matches!(images[0], crate::markdown::Image::Local { width, height, .. } if (*width, *height) == (10.0, 20.0)), "{images:?}");
    }

    #[test]
    fn svg_cache_keeps_one_file_per_slot() {
        let dir = tempfile::tempdir().unwrap();
        for text in [SVG.to_string(), SVG.replace("10", "11"), SVG.replace("10", "12")] {
            build_preview(&input(Provider::Image, None, &text), dir.path()).unwrap();
        }
        let mut other = input(Provider::Image, None, SVG);
        other.slot = 8;
        build_preview(&other, dir.path()).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names[0].starts_with("live-7-") && names[1].starts_with("live-8-"));
    }

    #[test]
    fn changes_diffs_the_buffer_against_the_disk_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.rs");
        std::fs::write(&path, "fn a() {}\n").unwrap();
        let p = build_preview(&input(Provider::Changes, path.to_str(), "fn a() {}\nfn b() {}\n"), dir.path()).unwrap();
        let diff = p.diff.unwrap();
        assert!(diff.has_changes());
        assert_eq!(diff.stats(), (1, 0));
    }

    #[test]
    fn changes_of_unsaved_buffer_has_empty_base() {
        let p = build_preview(&input(Provider::Changes, None, "new\n"), Path::new("/nonexistent")).unwrap();
        assert_eq!(p.diff.unwrap().stats(), (1, 0));
    }

    #[test]
    fn changes_of_unchanged_buffer_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "same\n").unwrap();
        let p = build_preview(&input(Provider::Changes, path.to_str(), "same\n"), dir.path()).unwrap();
        assert!(!p.diff.unwrap().has_changes());
    }

    #[test]
    fn pairs_look_up_both_ways_and_remove_from_either_side() {
        let mut pairs = LivePairs::default();
        pairs.insert(1, 10);
        pairs.insert(2, 20);
        assert_eq!(pairs.preview_of(1), Some(10));
        assert_eq!(pairs.editor_of(20), Some(2));
        assert_eq!(pairs.preview_of(3), None);
        assert_eq!(pairs.remove_preview(10), Some(1));
        assert_eq!(pairs.preview_of(1), None);
        assert_eq!(pairs.remove_editor(2), Some(20));
        assert_eq!(pairs.editor_of(20), None);
        assert_eq!(pairs.remove_editor(2), None);
    }

    #[test]
    fn inserting_an_editor_twice_replaces_its_preview() {
        let mut pairs = LivePairs::default();
        pairs.insert(1, 10);
        pairs.insert(1, 11);
        assert_eq!(pairs.preview_of(1), Some(11));
        assert_eq!(pairs.editor_of(10), None);
    }
}
