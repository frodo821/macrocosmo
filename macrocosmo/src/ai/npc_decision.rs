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
use crate::knowledge::KnowledgeStore;
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

/// Per-ship summary extracted from ECS for the NPC policy.
pub struct ShipInfo {
    pub entity: Entity,
    pub design_id: String,
    /// The system the ship is currently docked at, or `None` if in transit.
    pub system: Option<Entity>,
    /// `true` when the ship is `InSystem` with an empty command queue.
    pub is_idle: bool,
    pub can_survey: bool,
    pub can_colonize: bool,
    /// `true` when the ship is not a dedicated survey/colony vessel — i.e.
    /// it can participate in combat.
    pub is_combat: bool,
    pub ftl_range: f64,
}

/// Read-only context data extracted from ECS for the NPC policy.
///
/// This keeps the policy trait free of Bevy `Query` types, making it
/// testable without a full Bevy app.
pub struct NpcContext {
    /// Systems with hostile entities present (from KnowledgeStore).
    pub hostile_systems: Vec<Entity>,
    /// Known systems that have not yet been surveyed.
    pub unsurveyed_systems: Vec<Entity>,
    /// Surveyed systems that are not yet colonized (potential colony targets).
    pub colonizable_systems: Vec<Entity>,
    /// All ships owned by the empire being decided for.
    pub ships: Vec<ShipInfo>,
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
/// Ship selection is the policy's responsibility — commands carry explicit
/// ship entity lists so the command consumer dispatches only what the
/// policy chose. No cooldown is needed: the policy only selects idle ships,
/// so ships already dispatched (in transit) are naturally excluded.
///
/// Decision rules:
///
/// 1. **Attack hostiles**: If there are known hostile systems AND idle
///    combat ships exist → emit `attack_target` with the selected ships.
///
/// 2. **Retreat**: If `my_fleet_ready < 0.3` → emit `retreat`.
///
/// 3. **Fortify**: If `can_build_ships == 1.0` AND
///    `my_total_ships < colony_count * 2` → emit `fortify_system` (logged
///    only by the consumer in Phase 1).
///
/// Reads per-faction metrics from the bus using faction-suffixed IDs
/// (e.g. `my_total_ships.faction_42`), so each NPC sees only its own
/// empire's data.
#[derive(Default)]
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

        let fleet_ready = bus
            .current(&metric::for_faction("my_fleet_ready", faction_id))
            .unwrap_or(0.0);
        let colony_count = bus
            .current(&metric::for_faction("colony_count", faction_id))
            .unwrap_or(0.0);
        let can_build = bus
            .current(&metric::for_faction("can_build_ships", faction_id))
            .unwrap_or(0.0);
        let total_ships = bus
            .current(&metric::for_faction("my_total_ships", faction_id))
            .unwrap_or(0.0);

        // Idle combat ships: not survey/colony capable, currently docked.
        let idle_combat: Vec<Entity> = context
            .ships
            .iter()
            .filter(|s| s.is_idle && s.is_combat)
            .map(|s| s.entity)
            .collect();

        // Rule 1: Attack hostiles — have idle combat ships and known hostile systems
        if !context.hostile_systems.is_empty() && !idle_combat.is_empty() {
            let target = context.hostile_systems[0];
            let mut cmd = Command::new(cmd_ids::attack_target(), faction_id, now)
                .with_param("target_system", CommandValue::System(to_ai_system(target)))
                .with_param("ship_count", CommandValue::I64(idle_combat.len() as i64));
            for (i, &ship) in idle_combat.iter().enumerate() {
                cmd = cmd.with_param(
                    format!("ship_{i}"),
                    CommandValue::Entity(crate::ai::convert::to_ai_entity(ship)),
                );
            }
            commands.push(cmd);
            return commands;
        }

        // Rule 2: Survey unsurveyed systems — send idle survey ships
        let idle_surveyors: Vec<Entity> = context
            .ships
            .iter()
            .filter(|s| s.is_idle && s.can_survey)
            .map(|s| s.entity)
            .collect();
        if !context.unsurveyed_systems.is_empty() && !idle_surveyors.is_empty() {
            // Send one survey ship per unsurveyed system (up to available ships).
            for (ship, &target) in idle_surveyors
                .iter()
                .zip(context.unsurveyed_systems.iter())
            {
                let cmd = Command::new(cmd_ids::survey_system(), faction_id.clone(), now)
                    .with_param("target_system", CommandValue::System(to_ai_system(target)))
                    .with_param("ship_count", CommandValue::I64(1))
                    .with_param(
                        "ship_0",
                        CommandValue::Entity(crate::ai::convert::to_ai_entity(*ship)),
                    );
                commands.push(cmd);
            }
        }

        // Rule 3: Colonize surveyed uncolonized systems — send idle colony ships
        let idle_colonizers: Vec<Entity> = context
            .ships
            .iter()
            .filter(|s| s.is_idle && s.can_colonize)
            .map(|s| s.entity)
            .collect();
        if !context.colonizable_systems.is_empty() && !idle_colonizers.is_empty() {
            for (ship, &target) in idle_colonizers
                .iter()
                .zip(context.colonizable_systems.iter())
            {
                let cmd = Command::new(cmd_ids::colonize_system(), faction_id.clone(), now)
                    .with_param("target_system", CommandValue::System(to_ai_system(target)))
                    .with_param("ship_count", CommandValue::I64(1))
                    .with_param(
                        "ship_0",
                        CommandValue::Entity(crate::ai::convert::to_ai_entity(*ship)),
                    );
                commands.push(cmd);
            }
        }

        // Rule 4: Retreat — fleet is weak
        if fleet_ready > 0.0 && fleet_ready < 0.3 {
            let cmd = Command::new(cmd_ids::retreat(), faction_id, now);
            commands.push(cmd);
            return commands;
        }

        // Rule 5: Fortify / build ships — have shipyard but few ships
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
/// Tracks the last game tick at which AI decisions were made, so the
/// policy runs once per hexadies advance, not every render frame.
#[derive(Resource, Default)]
pub struct LastAiDecisionTick(pub i64);

pub fn npc_decision_tick(
    clock: Res<GameClock>,
    mut last_tick: ResMut<LastAiDecisionTick>,
    mut bus: ResMut<AiBusResource>,
    npcs: Query<(Entity, &Faction, &KnowledgeStore), With<AiControlled>>,
    all_ships: Query<(
        Entity,
        &crate::ship::Ship,
        &crate::ship::ShipState,
        &crate::ship::CommandQueue,
    )>,
    design_registry: Option<Res<crate::ship_design::ShipDesignRegistry>>,
    mut policy: Local<SimpleNpcPolicy>,
    #[cfg(feature = "ai-log")] mut log: Option<ResMut<super::debug_log::AiLogConfig>>,
) {
    let now = clock.elapsed;
    if now <= last_tick.0 {
        return;
    }
    last_tick.0 = now;

    for (entity, faction, knowledge) in &npcs {
        // Extract system intel from KnowledgeStore.
        let mut hostile_systems = Vec::new();
        let mut unsurveyed_systems = Vec::new();
        let mut colonizable_systems = Vec::new();
        for (_, k) in knowledge.iter() {
            if k.data.has_hostile {
                hostile_systems.push(k.system);
            }
            if !k.data.surveyed {
                unsurveyed_systems.push(k.system);
            }
            if k.data.surveyed && !k.data.colonized {
                colonizable_systems.push(k.system);
            }
        }

        // Build ship inventory for this empire.
        let ships: Vec<ShipInfo> = all_ships
            .iter()
            .filter(|(_, ship, _, _)| ship.owner == crate::ship::Owner::Empire(entity))
            .map(|(ship_entity, ship, state, queue)| {
                let system = match state {
                    crate::ship::ShipState::InSystem { system } => Some(*system),
                    _ => None,
                };
                let is_idle = system.is_some() && queue.commands.is_empty();
                let can_survey = design_registry
                    .as_ref()
                    .is_some_and(|r| r.can_survey(&ship.design_id));
                let can_colonize = design_registry
                    .as_ref()
                    .is_some_and(|r| r.can_colonize(&ship.design_id));
                let is_combat = !can_survey && !can_colonize && !ship.is_immobile();
                ShipInfo {
                    entity: ship_entity,
                    design_id: ship.design_id.clone(),
                    system,
                    is_idle,
                    can_survey,
                    can_colonize,
                    is_combat,
                    ftl_range: ship.ftl_range,
                }
            })
            .collect();

        let context = NpcContext {
            hostile_systems,
            unsurveyed_systems,
            colonizable_systems,
            ships,
        };

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
            unsurveyed_systems: vec![],
            colonizable_systems: vec![],
            ships: vec![],
        };
        let cmds = p.decide("vesk_hegemony", Entity::PLACEHOLDER, 0, &bus, &ctx);
        assert!(cmds.is_empty());

        let cmds = p.decide("aurelian_concord", Entity::PLACEHOLDER, 100, &bus, &ctx);
        assert!(cmds.is_empty());
    }

    /// Helper: create a bus with per-faction metrics declared and set.
    ///
    /// Metric names in `metrics` are base names (e.g. `"my_total_ships"`);
    /// they are automatically suffixed with the faction id.
    fn bus_with_metrics(
        faction: macrocosmo_ai::FactionId,
        metrics: &[(&str, f64)],
    ) -> macrocosmo_ai::AiBus {
        let mut bus = macrocosmo_ai::AiBus::with_warning_mode(WarningMode::Silent);
        schema::declare_metrics_standalone(&mut bus);
        // Declare + emit per-faction slots.
        for (name, value) in metrics {
            let id = metric::for_faction(name, faction);
            bus.declare_metric(id.clone(), macrocosmo_ai::MetricSpec::gauge(macrocosmo_ai::Retention::Medium, "per-faction self metric"));
            bus.emit(&id, *value, 10);
        }
        // Also declare the global metrics that remain un-suffixed.
        bus
    }

    /// The faction entity used in all SimpleNpcPolicy tests.
    fn test_faction_entity() -> Entity {
        Entity::from_raw_u32(1).unwrap()
    }

    /// The AI faction id corresponding to [`test_faction_entity`].
    fn test_faction_id() -> macrocosmo_ai::FactionId {
        crate::ai::convert::to_ai_faction(test_faction_entity())
    }

    #[test]
    fn simple_policy_emits_attack_when_conditions_met() {
        let bus = bus_with_metrics(
            test_faction_id(),
            &[
                ("my_total_ships", 5.0),
                ("my_fleet_ready", 0.8),
                ("systems_with_hostiles", 2.0),
                ("colony_count", 3.0),
                ("can_build_ships", 1.0),
            ],
        );

        let hostile_sys = Entity::from_raw_u32(42).unwrap();
        let combat_ship = Entity::from_raw_u32(100).unwrap();
        let ctx = NpcContext {
            hostile_systems: vec![hostile_sys],
            unsurveyed_systems: vec![],
            colonizable_systems: vec![],
            ships: vec![ShipInfo {
                entity: combat_ship,
                design_id: "corvette".into(),
                system: Some(Entity::from_raw_u32(1).unwrap()),
                is_idle: true,
                can_survey: false,
                can_colonize: false,
                is_combat: true,
                ftl_range: 15.0,
            }],
        };

        let mut policy = SimpleNpcPolicy::default();
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
        let bus = bus_with_metrics(
            test_faction_id(),
            &[
                ("my_total_ships", 2.0),
                ("my_fleet_ready", 0.2),
                ("systems_with_hostiles", 0.0),
                ("colony_count", 1.0),
                ("can_build_ships", 0.0),
            ],
        );

        let ctx = NpcContext {
            hostile_systems: vec![],
            unsurveyed_systems: vec![],
            colonizable_systems: vec![],
            ships: vec![],
        };

        let mut policy = SimpleNpcPolicy::default();
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
        let bus = bus_with_metrics(
            test_faction_id(),
            &[
                ("my_total_ships", 1.0),
                ("my_fleet_ready", 0.9),
                ("systems_with_hostiles", 0.0),
                ("colony_count", 3.0),
                ("can_build_ships", 1.0),
            ],
        );

        let ctx = NpcContext {
            hostile_systems: vec![],
            unsurveyed_systems: vec![],
            colonizable_systems: vec![],
            ships: vec![],
        };

        let mut policy = SimpleNpcPolicy::default();
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
        let bus = bus_with_metrics(
            test_faction_id(),
            &[
                ("my_total_ships", 8.0),
                ("my_fleet_ready", 0.9),
                ("systems_with_hostiles", 0.0),
                ("colony_count", 3.0),
                ("can_build_ships", 1.0),
            ],
        );

        let ctx = NpcContext {
            hostile_systems: vec![],
            unsurveyed_systems: vec![],
            colonizable_systems: vec![],
            ships: vec![],
        };

        let mut policy = SimpleNpcPolicy::default();
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
    fn simple_policy_no_attack_without_combat_ships() {
        let bus = bus_with_metrics(
            test_faction_id(),
            &[
                ("my_total_ships", 2.0),
                ("my_fleet_ready", 0.9),
                ("systems_with_hostiles", 1.0),
                ("colony_count", 1.0),
                ("can_build_ships", 0.0),
            ],
        );

        let hostile_sys = Entity::from_raw_u32(42).unwrap();
        // Only survey ships — no combat capability
        let survey_ship = Entity::from_raw_u32(100).unwrap();
        let ctx = NpcContext {
            hostile_systems: vec![hostile_sys],
            unsurveyed_systems: vec![],
            colonizable_systems: vec![],
            ships: vec![ShipInfo {
                entity: survey_ship,
                design_id: "explorer".into(),
                system: Some(Entity::from_raw_u32(1).unwrap()),
                is_idle: true,
                can_survey: true,
                can_colonize: false,
                is_combat: false,
                ftl_range: 15.0,
            }],
        };

        let mut policy = SimpleNpcPolicy::default();
        let cmds = policy.decide(
            "test_faction",
            Entity::from_raw_u32(1).unwrap(),
            10,
            &bus,
            &ctx,
        );

        assert!(
            cmds.iter().all(|c| c.kind.as_str() != "attack_target"),
            "should not attack without combat-capable ships"
        );
    }
}
