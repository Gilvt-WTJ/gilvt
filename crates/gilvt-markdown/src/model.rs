//! The drawable block model of a Markdown document.

/// 1-based, inclusive source line range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lines {
    pub start: u32,
    pub end: u32,
}

impl Lines {
    pub fn contains(&self, line: u32) -> bool {
        (self.start..=self.end).contains(&line)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub kind: BlockKind,
    pub lines: Lines,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockKind {
    /// `anchor`: GitHub-style slug, unique within the document (`-1`, `-2` suffixes).
    Heading { level: u8, content: Vec<Inline>, anchor: String },
    Paragraph(Vec<Inline>),
    List { ordered: bool, start: u64, items: Vec<ListItem> },
    Quote(Vec<Block>),
    /// `text` has no trailing newline.
    Code { lang: Option<String>, text: String },
    /// A fenced code block whose info string is `mermaid`.
    Mermaid(String),
    Table(Table),
    Rule,
    /// A paragraph consisting of exactly one image.
    Image { src: String, alt: String },
    /// Top-level `key: value` lines of a leading `---` block.
    FrontMatter(Vec<(String, String)>),
    /// A raw HTML block, shown as muted source.
    Html(String),
    /// Always the last block; present only if any referenced definition exists.
    Footnotes(Vec<Footnote>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    /// `Some(checked)` for a task item.
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
    pub lines: Lines,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub align: Vec<Align>,
    /// Per column: every non-empty body cell is a number (and there is at least one).
    pub numeric: Vec<bool>,
    pub header: Vec<Vec<Inline>>,
    /// Every row has exactly as many cells as the header.
    pub rows: Vec<Vec<Vec<Inline>>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Footnote {
    /// 1-based, in order of first reference.
    pub number: usize,
    pub label: String,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    /// Link destination as written.
    pub link: Option<String>,
    /// Footnote reference number; the run's text is the number.
    pub footnote: Option<usize>,
}

/// A styled run of text. Adjacent runs never share a style; a soft break is " ", a hard break "\n".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub style: Style,
}
