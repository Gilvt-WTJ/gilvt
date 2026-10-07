//! `⌘P`: fuzzy search over the files of a pane's repository (or directory), then preview, pin
//! or insert the path.

mod palette;
mod state;

use gpui::{Global, WeakEntity};

pub use palette::{FinderEvent, FinderView};
pub use state::{palette_stays, InFlight, ListingCache, Recent};

/// Listings of search roots: the cache, and the listings running now with the palettes waiting
/// for them. One for all windows: panes of different windows often share a repository.
#[derive(Default)]
pub struct Listings {
    pub cache: ListingCache,
    pub in_flight: InFlight<WeakEntity<FinderView>>,
}

impl Global for Listings {}
