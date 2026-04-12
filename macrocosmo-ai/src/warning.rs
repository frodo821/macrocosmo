//! Warning policy for the AI bus.
//!
//! The bus emits warnings via the `log` crate on recoverable misuses:
//! - re-declaring an already-declared topic (spec override)
//! - emitting to an undeclared topic (no-op)
//! - emitting a time-reversed sample (no-op, sample dropped)
//!
//! `WarningMode::Silent` suppresses these warnings — useful for tests or
//! performance-sensitive production configurations where the caller has
//! audited the emit sites.

use serde::{Deserialize, Serialize};

/// Warning emission policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WarningMode {
    /// Emit warnings via `log::warn` (default).
    #[default]
    Enabled,
    /// Suppress warnings.
    Silent,
}

/// Emit a warning if `mode` is `Enabled`. Internal helper.
#[doc(hidden)]
#[macro_export]
macro_rules! bus_warn {
    ($mode:expr, $($arg:tt)+) => {
        if matches!($mode, $crate::warning::WarningMode::Enabled) {
            log::warn!($($arg)+);
        }
    };
}
