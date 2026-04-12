//! `AiPlugin`: Bevy plugin that wires `macrocosmo-ai` into the game app.
//!
//! Responsibilities (Phase 1 / #203 — infrastructure only):
//!
//! - Registers [`AiBusResource`] (a thin newtype wrapper around
//!   `macrocosmo_ai::AiBus`).
//! - Runs [`schema::declare_all`] once at `Startup` so downstream systems
//!   can assume every content-level topic they depend on is declared.
//! - Declares the [`AiTickSet`] ordering
//!   (`MetricProduce → Reason → CommandDrain`) under `Update`, all chained
//!   `.after(crate::time_system::advance_game_time)` so the bus observes
//!   a monotonically advancing `GameClock`.
//!
//! No concrete AI systems are registered here — future issues (#204 et al.)
//! add systems under the appropriate `AiTickSet` set.

use std::ops::{Deref, DerefMut};

use bevy::prelude::*;
use macrocosmo_ai::{AiBus, WarningMode};

use crate::ai::schema;

/// Resource wrapper around [`AiBus`].
///
/// The wrapper exists because `AiBus` is defined in a dependency crate and
/// cannot have `#[derive(Resource)]` applied directly. `Deref` /
/// `DerefMut` forward all bus operations transparently.
///
/// Default `WarningMode` is [`WarningMode::Enabled`] (the `AiBus::default()`
/// behaviour), which logs through the `log` crate when the bus sees a
/// misuse (emitting to an undeclared topic, time-reversed emits, etc.).
#[derive(Resource, Debug, Default)]
pub struct AiBusResource(pub AiBus);

impl AiBusResource {
    /// Construct with an explicit [`WarningMode`].
    pub fn with_warning_mode(mode: WarningMode) -> Self {
        Self(AiBus::with_warning_mode(mode))
    }
}

impl Deref for AiBusResource {
    type Target = AiBus;
    fn deref(&self) -> &AiBus {
        &self.0
    }
}

impl DerefMut for AiBusResource {
    fn deref_mut(&mut self) -> &mut AiBus {
        &mut self.0
    }
}

/// Ordered system sets for AI-related work under `Update`.
///
/// All three run `.after(crate::time_system::advance_game_time)` and are
/// chained: `MetricProduce → Reason → CommandDrain`.
///
/// Phase 1 (#203) adds no systems to these sets; they exist so downstream
/// issues can register systems with the correct ordering from the start
/// without a later schema-change.
#[derive(SystemSet, Debug, Clone, Hash, PartialEq, Eq)]
pub enum AiTickSet {
    /// Systems that **read** game state and **write** metric / evidence
    /// topics into the bus.
    MetricProduce,
    /// Systems that **read** bus metrics / evidence and decide what
    /// commands to emit.
    Reason,
    /// Systems that **drain** pending commands from the bus and apply
    /// them to ECS game state.
    CommandDrain,
}

/// AI integration plugin. See module docs.
pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AiBusResource>()
            .add_systems(Startup, schema::declare_all)
            .configure_sets(
                Update,
                (
                    AiTickSet::MetricProduce,
                    AiTickSet::Reason,
                    AiTickSet::CommandDrain,
                )
                    .chain()
                    .after(crate::time_system::advance_game_time),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_registers_bus_resource() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(crate::time_system::GameClock::new(0));
        app.insert_resource(crate::time_system::GameSpeed::default());
        app.add_plugins(AiPlugin);
        app.update();
        assert!(app.world().get_resource::<AiBusResource>().is_some());
    }

    #[test]
    fn bus_resource_deref_exposes_ai_bus() {
        let r = AiBusResource::default();
        // default() through Deref should match a fresh AiBus behaviourally:
        // no metrics declared.
        assert!(!r.has_metric(&macrocosmo_ai::MetricId::from("nope")));
    }
}
