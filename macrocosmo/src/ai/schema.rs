//! Startup-time schema declarations for the AI bus.
//!
//! The bus requires every metric / command / evidence kind to be declared
//! before values can be emitted. This module centralises those
//! declarations so downstream systems can assume the schema is available
//! by the time `Update` runs.
//!
//! Phase 1 (#203) keeps the schema **empty** — content is added by later
//! issues (first concrete capability: #204 FleetCombatCapability). The
//! `declare_all` system is still wired into `Startup` in `AiPlugin` so
//! future issues only need to add entries to `declare_metrics`,
//! `declare_commands`, or `declare_evidence`.

use bevy::prelude::*;
use macrocosmo_ai::AiBus;

use crate::ai::plugin::AiBusResource;

/// Declare every metric / command / evidence topic used by the engine.
///
/// Runs once in `Startup` via [`AiPlugin`](crate::ai::AiPlugin).
pub fn declare_all(mut bus: ResMut<AiBusResource>) {
    declare_metrics(&mut bus.0);
    declare_commands(&mut bus.0);
    declare_evidence(&mut bus.0);
}

/// Metric topics. Empty in #203; populated by downstream capability issues.
fn declare_metrics(_bus: &mut AiBus) {}

/// Command kinds. Empty in #203; populated by downstream capability issues.
fn declare_commands(_bus: &mut AiBus) {}

/// Evidence kinds. Empty in #203; populated by downstream capability issues.
fn declare_evidence(_bus: &mut AiBus) {}
