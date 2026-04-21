//! Time primitives for the AI bus.
//!
//! AI core is agnostic to the unit of time — the engine uses hexadies (integer days),
//! but this crate only cares that time is a monotonic `i64`.

use serde::{Deserialize, Serialize};

/// Monotonic integer timestamp. In macrocosmo this is a hexadies count.
pub type Tick = i64;

/// A value sampled at a specific tick.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimestampedValue {
    pub at: Tick,
    pub value: f64,
}

impl TimestampedValue {
    pub fn new(at: Tick, value: f64) -> Self {
        Self { at, value }
    }
}
