//! The cleanup wizard (spec §4; ⌘⇧K, 会话 palette 「清理…」, 会话 menu 「清理…」): the four presets of
//! `cleanup_presets` on the left, the sessions a preset hits on the right, 归档 / 移到废纸篓 at the bottom.
//! `model` is the pure part (picks, keys, copy, the candidates); `view` holds the state and acts;
//! `render` draws it.

pub mod model;
mod render;
pub mod view;

pub use view::CleanupWizard;
