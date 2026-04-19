//! NPC decision tick — hook point for pluggable per-faction AI policies (#173).
//!
//! `AiPlugin` registers [`npc_decision_tick`] under [`AiTickSet::Reason`].
//! The production policy is [`SimpleNpcPolicy`], which reads bus metrics and
//! emits commands when basic thresholds are met.
//!
//! The trait exists so future issues under #189 can swap in
//! `macrocosmo_ai`-backed policies (campaign / Nash / feasibility) without
//! touching the system wiring.
//!
//! Scope note: this module intentionally carries **no** dependency on the
//! optional `macrocosmo_ai::mock` feature. The dev-dependency in
//! `macrocosmo/Cargo.toml` activates `mock` for the integration test
//! binary only, so callers of the production game crate never pay for the
//! feature.
//!
//! See `docs/plan-173-npc-empire-mock-ai.md` for the rollout plan.
//!
//! [`AiTickSet::Reason`]: super::AiTickSet::Reason

use bevy::prelude::*;

use macrocosmo_ai::{Command, CommandValue};

use crate::ai::convert::{to_ai_faction, to_ai_system};
use crate::ai::plugin::AiBusResource;
use crate::ai::schema::ids::{command as cmd_ids, metric};
use crate::galaxy::{AtSystem, Hostile};
use crate::player::{Empire, Faction, PlayerEmpire};
use crate::time_system::GameClock;

/// Marker component: this empire's decisions are made by the AI policy.
/// Applied to NPC empires automatically, and optionally to the player
/// empire when `--ai-player` is passed or `AiPlayerMode(true)` is set.
#[derive(Component)]
pub struct AiControlled;

/// Resource that opts the player empire into AI control.
/// Default is `false` — normal gameplay where the player makes decisions.
/// Set to `true` to let the AI policy drive the player empire.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct AiPlayerMode(pub bool);

/// System that marks all NPC empires (those with `Empire` but without
/// `PlayerEmpire`) with `AiControlled`. Runs every frame to catch newly
/// spawned empires.
pub fn mark_npc_empires_ai_controlled(
    mut commands: Commands,
    empires: Query<Entity, (With<Empire>, Without<PlayerEmpire>, Without<AiControlled>)>,
) {
    for entity in &empires {
        commands.entity(entity).insert(AiControlled);
    }
}

/// System that marks the player empire with `AiControlled` when
/// `AiPlayerMode(true)` is set.
pub fn mark_player_ai_controlled(
    mut commands: Commands,
    mode: Res<AiPlayerMode>,
    player: Query<Entity, (With<PlayerEmpire>, Without<AiControlled>)>,
) {
    if mode.0 {
        for entity in &player {
            commands.entity(entity).insert(AiControlled);
        }
    }
}

/// Trait implemented by pluggable NPC decision policies. Stateless policies
/// are encouraged; stateful policies can live in a `Resource` and be read
/// from the tick system.
///
/// Phase 1 (#173): `npc_decision_tick` calls [`SimpleNpcPolicy`] directly.
/// Future issues will route the call through a `Resource<Box<dyn NpcPolicy>>`
/// so Lua-defined per-empire policies can be swapped in.
pub trait NpcPolicy: Send + Sync + 'static {
    /// Called once per `Update` tick per NPC empire. The return value is
    /// a list of commands to emit on the bus.
    fn decide(
        &mut self,
        faction_id: &str,
        faction_entity: Entity,
        now: i64,
        bus: &macrocosmo_ai::AiBus,
        context: &NpcContext,
    ) -> Vec<Command>;
}

/// Read-only context data extracted from ECS for the NPC policy.
///
/// This keeps the policy trait free of Bevy `Query` types, making it
/// testable without a full Bevy app.
pub struct NpcContext {
    /// Systems with hostile entities present.
    pub hostile_systems: Vec<Entity>,
}

/// Default policy: do nothing. Useful for tests that want a quiet baseline.
#[derive(Default, Debug, Clone, Copy)]
pub struct NoOpPolicy;

impl NpcPolicy for NoOpPolicy {
    fn decide(
        &mut self,
        _faction_id: &str,
        _faction_entity: Entity,
        _now: i64,
        _bus: &macrocosmo_ai::AiBus,
        _context: &NpcContext,
    ) -> Vec<Command> {
        Vec::new()
    }
}

/// Simple heuristic NPC policy that reads bus metrics and emits commands.
///
/// Decision rules (Phase 1):
///
/// 1. **Attack hostiles**: If `systems_with_hostiles > 0` AND
///    `my_fleet_ready >= 0.5` AND `my_total_ships >= 3` → emit
///    `attack_target` for a hostile system.
///
/// 2. **Retreat**: If `my_fleet_ready < 0.3` → emit `retreat`.
///
/// 3. **Fortify**: If `can_build_ships == 1.0` AND
///    `my_total_ships < colony_count * 2` → emit `fortify_system` (logged
///    only by the consumer in Phase 1).
///
/// Known limitation: metrics are global (not per-faction) in Phase 1; the
/// policy treats them as "self" metrics for each NPC.
#[derive(Default, Debug, Clone, Copy)]
pub struct SimpleNpcPolicy;

impl NpcPolicy for SimpleNpcPolicy {
    fn decide(
        &mut self,
        _faction_id: &str,
        faction_entity: Entity,
        now: i64,
        bus: &macrocosmo_ai::AiBus,
        context: &NpcContext,
    ) -> Vec<Command> {
        let mut commands = Vec::new();
        let faction_id = to_ai_faction(faction_entity);

        let total_ships = bus.current(&metric::my_total_ships()).unwrap_or(0.0);
        let fleet_ready = bus.current(&metric::my_fleet_ready()).unwrap_or(0.0);
        let systems_with_hostiles = bus.current(&metric::systems_with_hostiles()).unwrap_or(0.0);
        let colony_count = bus.current(&metric::colony_count()).unwrap_or(0.0);
        let can_build = bus.current(&metric::can_build_ships()).unwrap_or(0.0);

        // Rule 1: Attack hostiles — fleet is strong enough
        if systems_with_hostiles > 0.0
            && fleet_ready >= 0.5
            && total_ships >= 3.0
            && !context.hostile_systems.is_empty()
        {
            // Pick the first hostile system as target
            let target = context.hostile_systems[0];
            let cmd = Command::new(cmd_ids::attack_target(), faction_id, now)
                .with_param("target_system", CommandValue::System(to_ai_system(target)));
            commands.push(cmd);
            return commands;
        }

        // Rule 2: Retreat — fleet is weak
        if fleet_ready > 0.0 && fleet_ready < 0.3 {
            let cmd = Command::new(cmd_ids::retreat(), faction_id, now);
            commands.push(cmd);
            return commands;
        }

        // Rule 3: Fortify / build ships — have shipyard but few ships
        if can_build >= 1.0 && total_ships < colony_count * 2.0 {
            let cmd = Command::new(cmd_ids::fortify_system(), faction_id, now);
            commands.push(cmd);
        }

        commands
    }
}

/// System run under [`AiTickSet::Reason`](super::AiTickSet::Reason):
/// walk every empire marked [`AiControlled`] and invoke [`SimpleNpcPolicy`].
/// NPC empires are auto-marked by [`mark_npc_empires_ai_controlled`].
/// The player empire is also marked when [`AiPlayerMode`]`(true)` is set.
pub fn npc_decision_tick(
    clock: Res<GameClock>,
    mut bus: ResMut<AiBusResource>,
    npcs: Query<(Entity, &Faction), With<AiControlled>>,
    hostiles: Query<&AtSystem, With<Hostile>>,
    #[cfg(feature = "ai-log")] mut log: Option<ResMut<super::debug_log::AiLogConfig>>,
) {
    let now = clock.elapsed;
    let mut policy = SimpleNpcPolicy;

    let hostile_systems: Vec<Entity> = hostiles.iter().map(|at| at.0).collect();
    let context = NpcContext {
        hostile_systems: hostile_systems.clone(),
    };

    for (entity, faction) in &npcs {
        let commands = policy.decide(&faction.id, entity, now, &bus.0, &context);
        for cmd in commands {
            bus.0.emit_command(cmd);
        }

        #[cfg(feature = "ai-log")]
        if let Some(ref mut log) = log {
            super::debug_log::write_decision_log(log, now, &faction.id, &bus);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::schema;
    use macrocosmo_ai::WarningMode;

    #[test]
    fn no_op_policy_is_silent() {
        let mut p = NoOpPolicy;
        let bus = macrocosmo_ai::AiBus::default();
        let ctx = NpcContext {
            hostile_systems: vec![],
        };
        let cmds = p.decide("vesk_hegemony", Entity::PLACEHOLDER, 0, &bus, &ctx);
        assert!(cmds.is_empty());

        let cmds = p.decide("aurelian_concord", Entity::PLACEHOLDER, 100, &bus, &ctx);
        assert!(cmds.is_empty());
    }

    /// Helper: create a bus with all metrics declared and set values.
    fn bus_with_metrics(metrics: &[(&str, f64)]) -> macrocosmo_ai::AiBus {
        let mut bus = macrocosmo_ai::AiBus::with_warning_mode(WarningMode::Silent);
        schema::declare_metrics_standalone(&mut bus);
        for (name, value) in metrics {
            let id = macrocosmo_ai::MetricId::from(*name);
            bus.emit(&id, *value, 10);
        }
        bus
    }

    #[test]
    fn simple_policy_emits_attack_when_conditions_met() {
        let bus = bus_with_metrics(&[
            ("my_total_ships", 5.0),
            ("my_fleet_ready", 0.8),
            ("systems_with_hostiles", 2.0),
            ("colony_count", 3.0),
            ("can_build_ships", 1.0),
        ]);

        let hostile_sys = Entity::from_raw_u32(42).unwrap();
        let ctx = NpcContext {
            hostile_systems: vec![hostile_sys],
        };

        let mut policy = SimpleNpcPolicy;
        let cmds = policy.decide(
            "test_faction",
            Entity::from_raw_u32(1).unwrap(),
            10,
            &bus,
            &ctx,
        );

        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].kind.as_str(), "attack_target");
        // Verify the target_system param is present
        match cmds[0].params.get("target_system") {
            Some(CommandValue::System(sys_ref)) => {
                let entity = crate::ai::convert::from_ai_system(*sys_ref);
                assert_eq!(entity, hostile_sys);
            }
            _ => panic!("expected target_system param"),
        }
    }

    #[test]
    fn simple_policy_emits_retreat_when_fleet_weak() {
        let bus = bus_with_metrics(&[
            ("my_total_ships", 2.0),
            ("my_fleet_ready", 0.2),
            ("systems_with_hostiles", 0.0),
            ("colony_count", 1.0),
            ("can_build_ships", 0.0),
        ]);

        let ctx = NpcContext {
            hostile_systems: vec![],
        };

        let mut policy = SimpleNpcPolicy;
        let cmds = policy.decide(
            "test_faction",
            Entity::from_raw_u32(1).unwrap(),
            10,
            &bus,
            &ctx,
        );

        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].kind.as_str(), "retreat");
    }

    #[test]
    fn simple_policy_emits_fortify_when_few_ships() {
        let bus = bus_with_metrics(&[
            ("my_total_ships", 1.0),
            ("my_fleet_ready", 0.9),
            ("systems_with_hostiles", 0.0),
            ("colony_count", 3.0),
            ("can_build_ships", 1.0),
        ]);

        let ctx = NpcContext {
            hostile_systems: vec![],
        };

        let mut policy = SimpleNpcPolicy;
        let cmds = policy.decide(
            "test_faction",
            Entity::from_raw_u32(1).unwrap(),
            10,
            &bus,
            &ctx,
        );

        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].kind.as_str(), "fortify_system");
    }

    #[test]
    fn simple_policy_does_nothing_when_fleet_sufficient() {
        let bus = bus_with_metrics(&[
            ("my_total_ships", 8.0),
            ("my_fleet_ready", 0.9),
            ("systems_with_hostiles", 0.0),
            ("colony_count", 3.0),
            ("can_build_ships", 1.0),
        ]);

        let ctx = NpcContext {
            hostile_systems: vec![],
        };

        let mut policy = SimpleNpcPolicy;
        let cmds = policy.decide(
            "test_faction",
            Entity::from_raw_u32(1).unwrap(),
            10,
            &bus,
            &ctx,
        );

        assert!(cmds.is_empty(), "no commands when fleet is sufficient");
    }

    #[test]
    fn simple_policy_no_attack_when_too_few_ships() {
        let bus = bus_with_metrics(&[
            ("my_total_ships", 2.0),
            ("my_fleet_ready", 0.9),
            ("systems_with_hostiles", 1.0),
            ("colony_count", 1.0),
            ("can_build_ships", 0.0),
        ]);

        let hostile_sys = Entity::from_raw_u32(42).unwrap();
        let ctx = NpcContext {
            hostile_systems: vec![hostile_sys],
        };

        let mut policy = SimpleNpcPolicy;
        let cmds = policy.decide(
            "test_faction",
            Entity::from_raw_u32(1).unwrap(),
            10,
            &bus,
            &ctx,
        );

        // Should NOT emit attack_target (only 2 ships, need >= 3)
        assert!(
            cmds.iter().all(|c| c.kind.as_str() != "attack_target"),
            "should not attack with fewer than 3 ships"
        );
    }
}
