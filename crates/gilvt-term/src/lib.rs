//! Terminal core for gilvt: PTY session, input encoding and render snapshots.
//! This crate must not depend on gpui.

pub mod blocks;
pub mod capture;
pub mod click_move;
pub mod keys;
pub mod lines;
pub mod links;
pub mod listener;
pub mod locale;
pub mod mouse;
pub mod osc;
pub mod palette;
pub mod paste;
pub mod paths;
pub mod procinfo;
pub mod session;
pub mod shellmarks;
pub mod size;
pub mod snapshot;
mod tap;

pub use alacritty_terminal::grid::Scroll;
pub use alacritty_terminal::index::{Direction, Point, Side};
pub use alacritty_terminal::selection::SelectionType;
pub use alacritty_terminal::term::TermMode;
pub use blocks::{CommandBlock, CommandLog};
pub use lines::ScrollOutcome;
pub use listener::{EventProxy, TermEvent};
pub use osc::Notification;
pub use palette::{Palette, Rgb};
pub use session::{SessionOptions, TermSession};
pub use shellmarks::{PromptMark, ReportedCwd};
pub use size::TermSize;
