//! Terminal themes (no gpui): Ghostty-format theme files, the built-in library, the chrome's semantic
//! colors derived from a theme, and the contrast rules that keep gilvt's status colors readable.

pub mod color;
pub mod parse;
pub mod builtin;
pub mod ui;
pub mod resolve;
pub use resolve::*;
pub use ui::{Status, UiColors};
