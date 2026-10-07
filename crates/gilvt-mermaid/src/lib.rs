//! Mermaid diagrams for gilvt's Markdown preview: a hidden WKWebView turns source into SVG,
//! resvg turns SVG into pixels, and a disk cache skips the web view on repeat renders.
//! macOS only; no gpui, so the app decides when to create, time out and drop the engine.

mod cache;
mod engine;
mod raster;

pub use cache::{cache_key, Cache};
pub use engine::{Engine, IMAGE_LOAD_ERROR};
pub use raster::{rasterize, Raster};

/// Version of the bundled `assets/mermaid.min.js`; part of every cache key.
pub const MERMAID_VERSION: &str = "12.0.0";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    /// mermaid's name for the theme.
    fn mermaid_name(self) -> &'static str {
        match self {
            Theme::Light => "default",
            Theme::Dark => "dark",
        }
    }
}
