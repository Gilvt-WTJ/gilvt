//! Markdown for gilvt's Quick Look: source → drawable block model, and line diffs mapped onto blocks.
//! Pure data — rendering lives in gilvt-app, so this crate must not depend on gpui.

mod changes;
mod inline;
mod model;
mod parse;
mod table;

pub use changes::{block_at_line, line_of_block, map_changes, ChangeKind, Changes, Deleted};
pub use model::{Align, Block, BlockKind, Footnote, Inline, Lines, ListItem, Style, Table};
pub use parse::parse;
