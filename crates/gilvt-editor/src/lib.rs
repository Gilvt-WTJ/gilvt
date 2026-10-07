//! The text-editing core behind gilvt's built-in editor: a rope buffer with grapheme-aware cursors,
//! undo groups, encoding and line-ending handling, atomic saves and external-change detection. It knows
//! nothing about windows, keys or fonts; the editor view drives it.

mod buffer;
mod commands;
mod encoding;
mod error;
mod file;
mod history;
mod motion;
mod position;

pub use buffer::{Buffer, Change};
pub use encoding::{common_encodings, decode, decode_with, encode, encoding_by_label, normalize_newlines, Decoded, Encoding, LineEnding};
pub use error::EditorError;
pub use file::{ExternalState, OpenOptions, MAX_FILE_SIZE};
pub use history::{Edit, EditKind, History, MAX_UNDO_GROUPS, MERGE_WINDOW_MS};
pub use position::{cell_width, Position, Selection};
