//! UI DSL contracts and pure runtime helpers.
//!
//! This crate intentionally stays independent of the game crate. Hosts adapt
//! game-specific handles into opaque DSL ids before matching or reconciling
//! fragments.

pub mod lua;
pub mod render;
pub mod runtime;

pub use render::*;
pub use runtime::*;
