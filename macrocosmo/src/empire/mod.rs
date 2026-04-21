//! Empire-level scoped components (#233).
//!
//! This module hosts ECS components that belong to the `PlayerEmpire` entity
//! but are topic-specific enough to deserve their own file. The first such
//! component is [`CommsParams`], which carries the four FTL-Comm modifier
//! buckets consumed by the knowledge-fact arrival-time computation.
pub mod comms;

pub use comms::CommsParams;
