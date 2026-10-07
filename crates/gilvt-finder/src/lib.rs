//! File finder for gilvt's `⌘P` and Finder drops: search root, file listing (git or directory walk),
//! fuzzy ranking, and paths as shell words. Pure logic — the palette lives in gilvt-app, so this crate
//! must not depend on gpui.

mod git;
mod list;
mod rank;
mod root;
mod shell;
#[cfg(test)]
mod testutil;

pub use list::{list, Listing, MAX_FILES};
pub use rank::{Context, Finder, Hit};
pub use root::{relative_to, search_root, Root};
pub use shell::{insertion, shell_escape};
