//! FTL Comm Relay modifier bundle attached to a `PlayerEmpire` entity (#233).
//!
//! Four modifier buckets are currently defined:
//!
//! | Field | Tech target | Consumer |
//! |-------|-------------|----------|
//! | `empire_relay_range`        | `empire.comm_relay_range`        | `effective_relay_range` (knowledge::facts) |
//! | `empire_relay_inv_latency`  | `empire.comm_relay_inv_latency`  | `relay_delay_hexadies` (knowledge::facts) |
//! | `fleet_relay_range`         | `fleet.comm_relay_range`         | reserved — no consumer yet |
//! | `fleet_relay_inv_latency`   | `fleet.comm_relay_inv_latency`   | reserved — no consumer yet |
//!
//! `fleet.*` are storage-only today. They route through the same tech-effect
//! pipeline so Lua definitions can already declare them; consumers will be
//! added in a follow-up when per-ship comm modules become relevant.

use bevy::prelude::*;

use crate::modifier::ModifiedValue;

/// Empire-level FTL Comm modifier bundle. See module docs for field meanings.
#[derive(Component, Default, Debug, Clone)]
pub struct CommsParams {
    pub empire_relay_range: ModifiedValue,
    pub empire_relay_inv_latency: ModifiedValue,
    /// Reserved: will be consumed by future per-fleet comm modules.
    pub fleet_relay_range: ModifiedValue,
    /// Reserved: will be consumed by future per-fleet comm modules.
    pub fleet_relay_inv_latency: ModifiedValue,
}
