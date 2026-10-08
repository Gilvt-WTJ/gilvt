//! M3c: starting and resuming agents by typing a command into a shell pane, exactly as the user would.
//! Only an explicit user action (↩ in a launcher panel, a sidebar entry) writes to a PTY here. Also the
//! history of past sessions, the 会话 palette (⌘⇧R) over it, moving sessions to the Trash, and the
//! 新建 Agent panel (⌘⇧N).

pub mod archive;
pub mod cleanup;
pub mod cleanup_presets;
pub mod cleanup_wizard;
pub mod history;
pub mod new_agent_model;
mod new_agent_render;
pub mod new_agent_view;
pub mod place;
mod projects;
mod sessions_render;
pub mod sessions_model;
pub mod sessions_view;
mod dir_label;
pub(crate) mod trash;

pub use archive::archive_sessions;
pub use dir_label::{copy_text as dir_copy_text, label_home, last_component, shorten_dir, DIR_MAX_CHARS};
pub use history::{History, HistorySnapshot};
pub use place::{Location, Placement};
