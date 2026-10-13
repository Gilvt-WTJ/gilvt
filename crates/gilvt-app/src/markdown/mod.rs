//! Quick Look's rendered Markdown view: `.md` / `.markdown` laid out as a document, with the
//! changes against the diff base marked per block.

mod history;
mod images;
mod layout;
mod links;
pub mod mermaid;
pub(crate) mod remote;
mod render;
mod rows;
pub(crate) mod select;
pub(crate) mod style;
mod view;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use gilvt_markdown::{map_changes, parse, Block, BlockKind, Changes, Inline};
use gilvt_viewer::{highlight, Appearance, DiffBase, Document, Preview, Span};

pub use history::History;
pub use view::MdState;

pub use images::Image;
use rows::{Mark, RowInfo};

/// A parsed document plus everything rendering needs that touches the disk; built off the main thread.
pub struct MdDoc {
    pub blocks: Vec<Block>,
    /// `None` when comparing against nothing ("file only").
    pub changes: Option<Changes>,
    pub rows: Vec<RowInfo>,
    pub rail: Vec<(f32, f32, Mark)>,
    pub line_count: u32,
    /// Image `src` → what to draw.
    pub images: HashMap<String, Image>,
    /// Link destinations that are existing local files (drawn with ↗).
    pub file_links: HashSet<String>,
    /// A remote document's links and images to `Stat` (empty for a local one).
    pub probes: remote::RemoteProbes,
    /// Rows showing each image `src` (re-measured when a remote image arrives).
    pub image_rows: HashMap<String, Vec<usize>>,
    /// Rows holding each link destination (re-measured when the Stat takes its mark away).
    pub link_rows: HashMap<String, Vec<usize>>,
}

/// Highlight spans per code block, keyed by the block's first source line.
pub type CodeSpans = HashMap<u32, Vec<Vec<Span>>>;

pub fn is_markdown(doc: &Document) -> bool {
    let token = doc.syntax_token();
    token.eq_ignore_ascii_case("md") || token.eq_ignore_ascii_case("markdown")
}

/// Calls `f` for every block, nested ones included, in document order.
fn walk<'a>(blocks: &'a [Block], f: &mut impl FnMut(&'a Block)) {
    for b in blocks {
        f(b);
        match &b.kind {
            BlockKind::List { items, .. } => items.iter().for_each(|i| walk(&i.blocks, f)),
            BlockKind::Quote(inner) => walk(inner, f),
            BlockKind::Footnotes(notes) => notes.iter().for_each(|n| walk(&n.blocks, f)),
            _ => {}
        }
    }
}

/// The inline runs a block shows directly (not those of nested blocks).
fn own_inlines(block: &Block) -> Vec<&[Inline]> {
    match &block.kind {
        BlockKind::Heading { content, .. } | BlockKind::Paragraph(content) => vec![content],
        BlockKind::Table(t) => t.header.iter().chain(t.rows.iter().flatten()).map(Vec::as_slice).collect(),
        _ => Vec::new(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Images and link targets are looked up on this Mac (blocking).
    Local,
    /// A file of an ssh host: nothing on this Mac is looked at; links count as existing and images as pending until
    /// `probe_remote_markdown` asks the host (R2a section 8.3).
    Remote,
}

impl MdDoc {
    /// `None` unless the preview is a Markdown text. Blocking: probes images and link targets.
    pub fn build(preview: &Preview) -> Option<MdDoc> {
        MdDoc::build_with(preview, Probe::Local)
    }

    /// A remote file's rendered document: nothing on this Mac is looked at.
    pub fn build_remote(preview: &Preview) -> Option<MdDoc> {
        MdDoc::build_with(preview, Probe::Remote)
    }

    fn build_with(preview: &Preview, probe: Probe) -> Option<MdDoc> {
        if !is_markdown(&preview.doc) {
            return None;
        }
        let text = preview.doc.text()?;
        let blocks = parse(text);
        let changes = preview.diff.as_ref().filter(|_| preview.base != DiffBase::None).map(|d| map_changes(&blocks, d));
        let rows = rows::build_rows(&blocks, changes.as_ref());
        let line_count = text.lines().count().max(1) as u32;
        let base_dir = preview.doc.path.as_deref().and_then(Path::parent);
        let repo_root = preview.repo_root.as_deref();
        let mut images = HashMap::new();
        let mut file_links = HashSet::new();
        let mut probes = remote::RemoteProbes::default();
        walk(&blocks, &mut |b| {
            if let BlockKind::Image { src, .. } = &b.kind {
                if !images.contains_key(src) {
                    let image = match probe {
                        Probe::Local => images::resolve(src, base_dir),
                        Probe::Remote => match images::locate(src, base_dir) {
                            Ok(path) => {
                                probes.images.push((src.clone(), path));
                                Image::Pending
                            }
                            Err(image) => image,
                        },
                    };
                    images.insert(src.clone(), image);
                }
            }
            for inl in own_inlines(b).into_iter().flatten() {
                let Some(dest) = inl.style.link.as_ref().filter(|d| !file_links.contains(*d)) else { continue };
                match probe {
                    Probe::Local => {
                        if matches!(links::resolve(dest, base_dir, repo_root), links::Target::File { .. }) {
                            file_links.insert(dest.clone());
                        }
                    }
                    Probe::Remote => {
                        if let links::Parsed::Paths { candidates, .. } = links::parse(dest, base_dir, repo_root) {
                            if !candidates.is_empty() {
                                file_links.insert(dest.clone());
                                probes.links.push((dest.clone(), candidates));
                            }
                        }
                    }
                }
            }
        });
        let mut image_rows: HashMap<String, Vec<usize>> = HashMap::new();
        let mut link_rows: HashMap<String, Vec<usize>> = HashMap::new();
        for (ix, info) in rows.iter().enumerate() {
            if let rows::Row::Block(b) = info.row {
                walk(std::slice::from_ref(&blocks[b]), &mut |blk| {
                    for inl in own_inlines(blk).into_iter().flatten() {
                        if let Some(dest) = inl.style.link.as_ref().filter(|d| file_links.contains(*d)) {
                            let rows = link_rows.entry(dest.clone()).or_default();
                            if rows.last() != Some(&ix) {
                                rows.push(ix);
                            }
                        }
                    }
                    if let BlockKind::Image { src, .. } = &blk.kind {
                        image_rows.entry(src.clone()).or_default().push(ix);
                    }
                });
            }
        }
        Some(MdDoc { rail: rows::rail_marks(&rows, line_count), blocks, changes, rows, line_count, images, file_links, probes, image_rows, link_rows })
    }

    /// `(first line, language, text)` of every code block that names a language.
    pub fn code_blocks(&self) -> Vec<(u32, String, String)> {
        let mut out = Vec::new();
        walk(&self.blocks, &mut |b| {
            if let BlockKind::Code { lang: Some(lang), text } = &b.kind {
                out.push((b.lines.start, lang.clone(), text.clone()));
            }
        });
        out
    }

    /// `(row, source)` of every Mermaid block, nested ones included, in document order.
    pub fn mermaid_blocks(&self) -> Vec<(usize, &str)> {
        let mut out = Vec::new();
        for (ix, info) in self.rows.iter().enumerate() {
            if let rows::Row::Block(b) = info.row {
                walk(std::slice::from_ref(&self.blocks[b]), &mut |b| {
                    if let BlockKind::Mermaid(source) = &b.kind {
                        out.push((ix, source.as_str()));
                    }
                });
            }
        }
        out
    }
}

/// Highlights code blocks from `MdDoc::code_blocks` (the slow part of loading a document).
pub fn highlight_code(blocks: Vec<(u32, String, String)>, appearance: Appearance) -> CodeSpans {
    blocks.into_iter().map(|(line, lang, text)| (line, highlight::highlight(&text, &lang, appearance))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview(dir: &Path, name: &str, text: &str) -> Preview {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        Preview::load(&path, &DiffBase::Head).unwrap()
    }

    #[test]
    fn builds_only_for_markdown() {
        let dir = tempfile::tempdir().unwrap();
        assert!(MdDoc::build(&preview(dir.path(), "a.rs", "fn x() {}\n")).is_none());
        assert!(MdDoc::build(&Preview::from_content("# hi\n".into(), Some("MD".into()))).is_some());
    }

    #[test]
    fn collects_nested_images_links_and_code() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("other.md"), "").unwrap();
        let text = "# T\n\n> - [x] see [other](other.md) and [web](https://x.dev)\n>\n>   ![pic](missing.png)\n\n```rust\nfn a() {}\n```\n\n```\nplain\n```\n";
        let doc = MdDoc::build(&preview(dir.path(), "a.md", text)).unwrap();
        assert!(doc.changes.is_none(), "outside a repository there is nothing to compare with");
        assert_eq!(doc.rows.len(), doc.blocks.len());
        assert_eq!(doc.file_links, HashSet::from(["other.md".to_string()]));
        assert!(matches!(doc.images["missing.png"], Image::Unavailable { .. }));
        assert_eq!(doc.code_blocks(), [(7, "rust".to_string(), "fn a() {}".to_string())]);
        assert_eq!(doc.line_count, 13);
        let spans = highlight_code(doc.code_blocks(), Appearance::Dark);
        assert_eq!(spans[&7].len(), 1);
    }

    #[test]
    fn finds_mermaid_blocks_by_row() {
        let text = "```mermaid\npie\n```\n\npara\n\n> ```mermaid\n> graph TD\n> ```\n";
        let doc = MdDoc::build(&Preview::from_content(text.into(), Some("md".into()))).unwrap();
        assert_eq!(doc.mermaid_blocks(), [(0, "pie"), (2, "graph TD")]);
    }

    #[test]
    fn a_remote_document_looks_at_nothing_on_this_mac() {
        // Files with these names exist on the Mac; the remote document must neither find nor miss them here.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("here.md"), "").unwrap();
        std::fs::write(dir.path().join("pic.png"), "not even a png").unwrap();
        let text = "[here](here.md) [gone](gone.md) [web](https://x.dev)\n\n![p](pic.png)\n\n![w](https://e/a.png)\n";
        let preview = Preview::from_remote(&dir.path().join("a.md"), text.as_bytes().to_vec(), None, &DiffBase::Head, None, false);
        let doc = MdDoc::build_remote(&preview).unwrap();
        assert_eq!(doc.file_links, HashSet::from(["here.md".to_string(), "gone.md".to_string()]), "both drawn as files until the Stat answers");
        assert_eq!(doc.images["pic.png"], Image::Pending, "not read from the Mac");
        assert_eq!(doc.images["https://e/a.png"], Image::Remote("https://e/a.png".into()));
        assert_eq!(doc.probes.links.iter().map(|(d, _)| d.as_str()).collect::<Vec<_>>(), ["here.md", "gone.md"]);
        assert_eq!(doc.probes.images, vec![("pic.png".to_string(), dir.path().join("pic.png"))]);
        assert_eq!(doc.image_rows["pic.png"].len(), 1, "the one row that shows it");
        assert_eq!(doc.link_rows["gone.md"], vec![0], "the row whose mark the Stat may take away");
        std::fs::write(dir.path().join("a.md"), text).unwrap();
        let local = MdDoc::build(&Preview::load(&dir.path().join("a.md"), &DiffBase::Head).unwrap()).unwrap();
        assert!(local.probes.links.is_empty() && local.probes.images.is_empty(), "a local document probes nothing remote");
    }
}
