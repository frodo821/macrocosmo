//! # Profiling support (#284)
//!
//! Zero-overhead span helper for [Tracy](https://github.com/wolfpld/tracy)
//! profiling. Activated via the `profile` cargo feature, which also
//! pass-throughs `bevy/trace_tracy` and `bevy/debug`.
//!
//! ## Usage
//!
//! ```ignore
//! use crate::prof_span;
//!
//! fn my_hot_system(...) {
//!     prof_span!("my_hot_system");
//!     // ... work ...
//! }
//! ```
//!
//! With the default build (`profile` feature off), the macro expands to
//! **nothing** — no `tracing` calls, no local guard, zero runtime and binary
//! overhead. See `docs/profiling.md` for the full workflow.

/// Open an `info_span!` in the current scope when the `profile` feature is
/// enabled; no-op otherwise. Accepts the same argument shapes as
/// `tracing::info_span!` (at minimum a static name, optionally key/value
/// fields).
///
/// The generated guard lives until the surrounding scope ends, so simply
/// call this as the first statement of a function to measure the whole
/// body.
#[cfg(feature = "profile")]
#[macro_export]
macro_rules! prof_span {
    ($name:expr $(, $($arg:tt)*)?) => {
        let __prof_span = ::bevy::prelude::info_span!($name $(, $($arg)*)?);
        let __prof_guard = __prof_span.entered();
    };
}

/// No-op variant of [`prof_span!`] used when the `profile` feature is
/// disabled. Expands to nothing so default builds carry zero overhead.
#[cfg(not(feature = "profile"))]
#[macro_export]
macro_rules! prof_span {
    ($name:expr $(, $($arg:tt)*)?) => {};
}
