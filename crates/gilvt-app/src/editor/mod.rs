//! Built-in text editor pane (E2a). `route`, `wrap`, `indent` and `model` are pure logic (no gpui).

pub mod route;
pub mod wrap;
pub mod indent;
pub mod model;
mod model_edit;
pub mod element;
pub mod view;
pub mod chrome;
mod syntax;
pub mod external;
pub mod compare;
pub mod popup;
