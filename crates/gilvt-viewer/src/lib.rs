//! Quick Look model for gilvt: loading files, syntax highlighting, diffs against git.
//! Pure data — rendering lives in gilvt-app, so this crate must not depend on gpui.

pub mod ansi;
pub mod diff;
pub mod display;
pub mod document;
pub mod highlight;
pub mod nav;
pub mod preview;

/// The git queries (moved to `gilvt-client`, shared with the remote daemon); the path `gilvt_viewer::git` stays.
pub use gilvt_client::git;
#[cfg(test)]
pub(crate) mod testrepo;

pub use diff::{Diff, DiffLine, LineKind, Row, SplitRow};
pub use document::{Content, Document};
pub use highlight::{Appearance, ClassSpan, Color, Highlighter, Span, TokenClass};
pub use preview::{DiffBase, Preview, TurnRange};
