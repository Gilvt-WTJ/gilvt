//! Session Center queue projection and view. The existing M3c sessions palette remains the All Sessions tab.

pub mod model;
pub mod pending;
pub(crate) mod render;
pub mod view;

#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod pending_tests;
