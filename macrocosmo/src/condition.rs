pub use crate::modifier::ScopedModifications;
pub use macrocosmo_core::condition::*;

/// Compatibility name while callers move from flag-only state to scoped modifications.
pub type ScopedFlags = ScopedModifications;
