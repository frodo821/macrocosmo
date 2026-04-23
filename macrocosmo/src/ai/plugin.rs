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

/// Tracks which faction entities already have their per-faction metric slots
/// declared on the bus. Prevents duplicate `declare_metric` calls (which
/// would trigger re-declaration warnings) between Startup and Update.
#[derive(Resource, Default)]
pub struct DeclaredFactionSlots(pub std::collections::HashSet<Entity>);

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
        // Ensure MoveRequested message is registered (drain_ai_commands writes it).
        // Idempotent if CommandEventsPlugin already registered it.
        app.add_message::<crate::ship::command_events::MoveRequested>();
        app.init_resource::<crate::ship::command_events::NextCommandId>();
        app.init_resource::<AiBusResource>()
            .init_resource::<super::npc_decision::AiPlayerMode>()
            .init_resource::<super::npc_decision::LastAiDecisionTick>()
            .init_resource::<super::command_consumer::PendingRulerBoarding>()
            .init_resource::<DeclaredFactionSlots>()
            .add_systems(Startup, schema::declare_all)
            // Declare per-faction metric slots explicitly at Startup for all
            // existing factions. Mid-game faction spawns are handled by
            // declare_foreign_slots_on_awareness on Update.
            .add_systems(
                Startup,
                declare_foreign_slots_for_existing_factions
                    .after(schema::declare_all)
                    .after(crate::setup::run_all_factions_on_game_start),
            )
            .add_systems(
                Update,
                (
                    declare_foreign_slots_on_awareness,
                    (
                        super::emitters::emit_military_metrics,
                        super::emitters::emit_economic_metrics,
                        super::emitters::emit_foreign_metrics,
                    ),
                )
                    .chain()
                    .in_set(AiTickSet::MetricProduce),
            )
            // Mark empires as AiControlled before the decision tick runs.
            .add_systems(
                Update,
                (
                    super::npc_decision::mark_npc_empires_ai_controlled,
                    super::npc_decision::mark_player_ai_controlled,
                )
                    .before(AiTickSet::MetricProduce)
                    .after(crate::time_system::advance_game_time),
            )
            // NPC decision tick — SimpleNpcPolicy reads metrics and emits commands.
            .add_systems(
                Update,
                super::npc_decision::npc_decision_tick.in_set(AiTickSet::Reason),
            )
            // Command consumer — drains AI commands and converts to ECS actions.
            // `process_ruler_boarding` runs after `drain_ai_commands` to handle
            // deferred ruler boarding (needs mutable Ship access).
            .add_systems(
                Update,
                (
                    super::command_consumer::drain_ai_commands,
                    super::command_consumer::process_ruler_boarding
                        .after(super::command_consumer::drain_ai_commands),
                )
                    .in_set(AiTickSet::CommandDrain),
            )
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

        #[cfg(feature = "ai-log")]
        {
            app.add_systems(Startup, super::debug_log::setup_ai_log);
            app.add_systems(
                Update,
                super::debug_log::emit_world_state_log
                    .in_set(AiTickSet::MetricProduce)
                    .run_if(resource_exists::<super::debug_log::AiLogConfig>),
            );
        }
    }
}

/// Declare per-faction metric slots (self + foreign) for a single faction
/// entity. No-op if already declared for this entity.
fn declare_faction_slots(
    bus: &mut AiBus,
    declared: &mut DeclaredFactionSlots,
    entity: Entity,
) {
    if !declared.0.insert(entity) {
        return; // already declared
    }
    let fid = super::convert::to_ai_faction(entity);

    // Per-faction "self" metric slots.
    for base in super::schema::ids::metric::PER_FACTION_METRIC_BASES {
        let id = super::schema::ids::metric::for_faction(base, fid);
        bus.declare_metric(
            id,
            macrocosmo_ai::MetricSpec::gauge(
                macrocosmo_ai::Retention::Medium,
                "per-faction self metric",
            ),
        );
    }

    // Foreign-faction metric slots (Tier 2).
    for template in super::schema::foreign::foreign_metric_templates() {
        let id = super::schema::foreign::foreign_metric_id(&template.prefix, fid);
        bus.declare_metric(id, (template.spec_factory)());
    }
}

/// Startup system: declare metric slots for all factions that already exist.
/// Runs after `run_all_factions_on_game_start` so NPC empires are included.
pub fn declare_foreign_slots_for_existing_factions(
    mut bus: ResMut<AiBusResource>,
    mut declared: ResMut<DeclaredFactionSlots>,
    factions: Query<Entity, With<crate::player::Faction>>,
) {
    for entity in &factions {
        declare_faction_slots(&mut bus, &mut declared, entity);
    }
}

/// Update system: declare metric slots for factions spawned mid-game.
/// Uses `Added<Faction>` to detect new spawns; `DeclaredFactionSlots`
/// dedup avoids re-declaring factions already handled at Startup.
pub fn declare_foreign_slots_on_awareness(
    mut bus: ResMut<AiBusResource>,
    mut declared: ResMut<DeclaredFactionSlots>,
    new_factions: Query<Entity, (With<crate::player::Faction>, Added<crate::player::Faction>)>,
) {
    for entity in &new_factions {
        declare_faction_slots(&mut bus, &mut declared, entity);
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
