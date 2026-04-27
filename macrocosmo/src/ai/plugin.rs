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
#[derive(Resource, Debug, Default, Reflect)]
#[reflect(Resource)]
pub struct AiBusResource(
    /// `AiBus` lives in the engine-agnostic `macrocosmo-ai` crate which
    /// cannot take a `bevy_reflect` dependency (`ai-core-isolation.yml`
    /// CI). The wrapper resource still appears in the type registry for
    /// BRP, but the inner bus is not introspectable via reflection.
    #[reflect(ignore)]
    pub AiBus,
);

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
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
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
            .init_resource::<super::orchestrator_runtime::OrchestratorRegistry>()
            // Round 9 PR #3: AI command light-speed delay shim. Outbox
            // resource is initialised here so save/load round-trips see
            // a consistent type-registered Resource even on fresh runs.
            .init_resource::<super::command_outbox::AiCommandOutbox>()
            .add_systems(Startup, schema::declare_all)
            // #439 Phase 3: `declare_foreign_slots_for_existing_factions`
            // must run after NPC empires have spawned, so it moves with
            // the world-spawn chain to `OnEnter(NewGame)`. `schema::declare_all`
            // stays on Startup — it only registers bus topic names.
            // Mid-game faction spawns are still handled by
            // `declare_foreign_slots_on_awareness` on Update.
            .add_systems(
                OnEnter(crate::game_state::GameState::NewGame),
                (
                    declare_foreign_slots_for_existing_factions
                        .after(crate::setup::run_all_factions_on_game_start),
                    super::orchestrator_runtime::register_demo_orchestrator
                        .after(declare_foreign_slots_for_existing_factions),
                ),
            )
            // Foreign-slot declaration must run during Bootstrapping /
            // NewGame / LoadingSave too — it reacts to `Added<Faction>` so
            // freshly-spawned empires get their metric slots before the
            // first InGame tick fires the emitters. Intentionally NOT
            // gated on `GameState::InGame` (#439 Phase 2).
            .add_systems(
                Update,
                declare_foreign_slots_on_awareness.in_set(AiTickSet::MetricProduce),
            )
            // Metric emitters are game-tick work — gate on InGame.
            .add_systems(
                Update,
                (
                    super::emitters::emit_military_metrics,
                    super::emitters::emit_economic_metrics,
                    super::emitters::emit_foreign_metrics,
                )
                    .after(declare_foreign_slots_on_awareness)
                    .in_set(AiTickSet::MetricProduce)
                    .run_if(in_state(crate::game_state::GameState::InGame)),
            )
            // Mark empires as AiControlled before the decision tick runs.
            .add_systems(
                Update,
                (
                    super::npc_decision::mark_npc_empires_ai_controlled,
                    super::npc_decision::mark_player_ai_controlled,
                )
                    .before(AiTickSet::MetricProduce)
                    .after(crate::time_system::advance_game_time)
                    .run_if(in_state(crate::game_state::GameState::InGame)),
            )
            // NPC decision tick — MidStanceAgent reads metrics and emits commands.
            .add_systems(
                Update,
                super::npc_decision::npc_decision_tick
                    .in_set(AiTickSet::Reason)
                    .run_if(in_state(crate::game_state::GameState::InGame)),
            )
            // Three-layer orchestrator tick — runs alongside (not instead
            // of) `MidStanceAgent`. Ordered `.after(npc_decision_tick)`
            // to avoid `ResMut<AiBusResource>` contention within the same
            // schedule step. Both write the bus; the orchestrator only
            // emits `pursue_metric:*` kinds which `drain_ai_commands`
            // logs as `unknown` and ignores — observed-only for now.
            .add_systems(
                Update,
                super::orchestrator_runtime::run_orchestrators
                    .after(super::npc_decision::npc_decision_tick)
                    .in_set(AiTickSet::Reason)
                    .run_if(in_state(crate::game_state::GameState::InGame)),
            )
            // Round 9 PR #3: AI command light-speed delay shim. The
            // dispatcher runs at the **end** of `Reason` (after both
            // producers `npc_decision_tick` and `run_orchestrators`)
            // so it sees every command emitted this tick before the
            // bus reaches `CommandDrain`. The processor runs at the
            // **start** of `CommandDrain`, before `drain_ai_commands`,
            // and re-pushes mature outbox entries via
            // `bus.push_command_already_dispatched` so the consumer
            // sees them at the right tick.
            .add_systems(
                Update,
                super::command_outbox::dispatch_ai_pending_commands
                    .after(super::orchestrator_runtime::run_orchestrators)
                    .in_set(AiTickSet::Reason)
                    .run_if(in_state(crate::game_state::GameState::InGame)),
            )
            // Command consumer — drains AI commands and converts to ECS actions.
            // `process_ai_pending_commands` runs first so mature outbox
            // entries are released to the bus before `drain_ai_commands`
            // pulls from it.
            // `process_ruler_boarding` runs after `drain_ai_commands` to handle
            // deferred ruler boarding (needs mutable Ship access).
            // `sweep_resolved_survey_assignments` runs alongside the consumer
            // in the same set so the sweep happens at the natural "AI command
            // resolution" boundary; it has no data dependency on
            // `drain_ai_commands`.
            .add_systems(
                Update,
                (
                    super::command_outbox::process_ai_pending_commands,
                    super::command_consumer::drain_ai_commands
                        .after(super::command_outbox::process_ai_pending_commands),
                    super::command_consumer::process_ruler_boarding
                        .after(super::command_consumer::drain_ai_commands),
                    super::assignments::sweep_resolved_survey_assignments,
                )
                    .in_set(AiTickSet::CommandDrain)
                    .run_if(in_state(crate::game_state::GameState::InGame)),
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
fn declare_faction_slots(bus: &mut AiBus, declared: &mut DeclaredFactionSlots, entity: Entity) {
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
