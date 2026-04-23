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
use crate::player::{AboardShip, Empire, EmpireRuler, Faction, PlayerEmpire, Ruler, StationedAt};
use crate::technology::ResearchQueue;
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
    /// `true` when the empire has an active research target in its queue.
    pub is_researching: bool,
    /// The Ruler entity for this empire, if one exists.
    pub ruler_entity: Option<Entity>,
    /// The system the Ruler is currently stationed at.
    pub ruler_system: Option<Entity>,
    /// Whether the Ruler is currently aboard a ship.
    pub ruler_aboard: bool,
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
/// Decision rules (evaluated in order):
///
/// 1. **Attack hostiles**: If there are known hostile systems AND idle
///    combat ships exist → emit `attack_target` with the selected ships.
///    (early-returns — combat is highest priority)
///
/// 2. **Survey**: Send idle survey ships to unsurveyed systems.
///
/// 3. **Colonize**: Send idle colony ships to colonizable systems.
///
/// 4. **Research**: If no research is active and techs are available →
///    emit `research_focus` (auto-pick).
///
/// 5. **Colony building**: If `free_building_slots > 0` → emit
///    `build_structure` with a building chosen by production heuristic
///    (power plant if energy negative, farm if food negative, else mine).
///
/// 6. **Fleet composition**: If `can_build_ships >= 1.0` and the fleet
///    is missing key roles (survey, colony, combat) → emit `build_ship`
///    for the most-needed role.
///
/// 7. **Retreat**: If `my_fleet_ready < 0.3` → emit `retreat`.
///
/// 8. **Fortify**: If `can_build_ships == 1.0` AND
///    `my_total_ships < colony_count * 2` → emit `fortify_system`.
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

            // Follow-up: move the Ruler to the attack target if idle (not aboard a ship).
            if !context.ruler_aboard && context.ruler_entity.is_some() {
                let ruler_cmd = Command::new(cmd_ids::move_ruler(), faction_id, now)
                    .with_param("target_system", CommandValue::System(to_ai_system(target)));
                commands.push(ruler_cmd);
            }

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

        // Rule 4: Research — keep research queue active
        let tech_unlocks = bus
            .current(&metric::for_faction("tech_unlocks_available", faction_id))
            .unwrap_or(0.0);
        if tech_unlocks > 0.0 && !context.is_researching {
            let cmd = Command::new(cmd_ids::research_focus(), faction_id, now);
            commands.push(cmd);
        }

        // Rule 5: Colony building — fill empty building slots
        let free_slots = bus
            .current(&metric::for_faction("free_building_slots", faction_id))
            .unwrap_or(0.0);
        if free_slots > 0.0 {
            let net_energy = bus
                .current(&metric::for_faction("net_production_energy", faction_id))
                .unwrap_or(0.0);
            let net_food = bus
                .current(&metric::for_faction("net_production_food", faction_id))
                .unwrap_or(0.0);

            let building_id = if net_energy < 0.0 {
                "power_plant"
            } else if net_food < 0.0 {
                "farm"
            } else {
                "mine"
            };

            let cmd = Command::new(cmd_ids::build_structure(), faction_id, now)
                .with_param("building_id", CommandValue::Str(building_id.into()));
            commands.push(cmd);
        }

        // Rule 6: Fleet composition — build missing ship roles
        if can_build >= 1.0 {
            let survey_count = context.ships.iter().filter(|s| s.can_survey).count();
            let colony_count_ships = context.ships.iter().filter(|s| s.can_colonize).count();
            let combat_count = context.ships.iter().filter(|s| s.is_combat).count();

            if survey_count == 0 && !context.unsurveyed_systems.is_empty() {
                let cmd = Command::new(cmd_ids::build_ship(), faction_id, now)
                    .with_param("design_id", CommandValue::Str("explorer_mk1".into()));
                commands.push(cmd);
            } else if colony_count_ships == 0 && !context.colonizable_systems.is_empty() {
                let cmd = Command::new(cmd_ids::build_ship(), faction_id, now)
                    .with_param("design_id", CommandValue::Str("colony_ship_mk1".into()));
                commands.push(cmd);
            } else if combat_count < 3 {
                let cmd = Command::new(cmd_ids::build_ship(), faction_id, now)
                    .with_param("design_id", CommandValue::Str("patrol_corvette".into()));
                commands.push(cmd);
            }
        }

        // Rule 7: Retreat — fleet is weak
        if fleet_ready > 0.0 && fleet_ready < 0.3 {
            let cmd = Command::new(cmd_ids::retreat(), faction_id, now);
            commands.push(cmd);
            return commands;
        }

        // Rule 8: Fortify / build ships — have shipyard but few ships
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
    npcs: Query<
        (
            Entity,
            &Faction,
            &KnowledgeStore,
            Option<&crate::knowledge::SystemVisibilityMap>,
        ),
        With<AiControlled>,
    >,
    // #? SimpleNpcPolicy needs to know which systems exist at all — the
    // KnowledgeStore only carries entries the empire has already
    // surveyed / been told about (one entry per owned capital at spawn),
    // so `unsurveyed_systems` derived from it was always empty for
    // fresh empires, freezing Explorers in dock.
    all_stars: Query<Entity, With<crate::galaxy::StarSystem>>,
    all_ships: Query<(
        Entity,
        &crate::ship::Ship,
        &crate::ship::ShipState,
        &crate::ship::CommandQueue,
    )>,
    research_queues: Query<&ResearchQueue, With<Empire>>,
    design_registry: Option<Res<crate::ship_design::ShipDesignRegistry>>,
    empire_rulers: Query<&EmpireRuler, With<Empire>>,
    ruler_q: Query<(&StationedAt, Option<&AboardShip>), With<Ruler>>,
    mut policy: Local<SimpleNpcPolicy>,
    #[cfg(feature = "ai-log")] mut log: Option<ResMut<super::debug_log::AiLogConfig>>,
) {
    use crate::knowledge::SystemVisibilityTier;

    let now = clock.elapsed;
    if now <= last_tick.0 {
        return;
    }
    last_tick.0 = now;

    for (entity, faction, knowledge, vis_map_opt) in &npcs {
        // Extract system intel. Hostile / colonizable signals still come
        // from the KnowledgeStore (those require detailed snapshots),
        // but `unsurveyed_systems` is derived from the galaxy-wide star
        // list minus whatever the empire has already surveyed —
        // otherwise freshly-spawned empires never find survey targets
        // because their KnowledgeStore is empty aside from the capital.
        let mut hostile_systems = Vec::new();
        let mut colonizable_systems = Vec::new();
        let mut surveyed_ids: std::collections::HashSet<Entity> =
            std::collections::HashSet::new();
        for (_, k) in knowledge.iter() {
            if k.data.has_hostile {
                hostile_systems.push(k.system);
            }
            if k.data.surveyed {
                surveyed_ids.insert(k.system);
                if !k.data.colonized {
                    colonizable_systems.push(k.system);
                }
            }
        }
        // Every catalogued system (which, right now, means every system
        // in the galaxy thanks to `initialize_visibility_tiers`) is a
        // valid survey target if we haven't surveyed it yet. Fall back
        // to all stars when the empire has no visibility map — defensive
        // for test setups.
        let mut unsurveyed_systems: Vec<Entity> = Vec::new();
        for system_entity in all_stars.iter() {
            if surveyed_ids.contains(&system_entity) {
                continue;
            }
            let knowable = vis_map_opt
                .map(|vm| vm.get(system_entity) >= SystemVisibilityTier::Catalogued)
                .unwrap_or(true);
            if knowable {
                unsurveyed_systems.push(system_entity);
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

        let is_researching = research_queues
            .get(entity)
            .is_ok_and(|rq| rq.current.is_some());

        // Extract Ruler info for this empire.
        let (ruler_entity, ruler_system, ruler_aboard) =
            if let Ok(empire_ruler) = empire_rulers.get(entity) {
                let ruler_e = empire_ruler.0;
                if let Ok((stationed, aboard)) = ruler_q.get(ruler_e) {
                    (Some(ruler_e), Some(stationed.system), aboard.is_some())
                } else {
                    (Some(ruler_e), None, false)
                }
            } else {
                (None, None, false)
            };

        let context = NpcContext {
            hostile_systems,
            unsurveyed_systems,
            colonizable_systems,
            ships,
            is_researching,
            ruler_entity,
            ruler_system,
            ruler_aboard,
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
            is_researching: false,
            ruler_entity: None,
            ruler_system: None,
            ruler_aboard: false,
        };
        let cmds = p.decide("vesk_hegemony", Entity::PLACEHOLDER, 0, &bus, &ctx);
        assert!(cmds.is_empty());

        let cmds = p.decide("aurelian_concord", Entity::PLACEHOLDER, 100, &bus, &ctx);
        assert!(cmds.is_empty(), "no-op policy should emit nothing");
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
            is_researching: false,
            ruler_entity: None,
            ruler_system: None,
            ruler_aboard: false,
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
            is_researching: false,
            ruler_entity: None,
            ruler_system: None,
            ruler_aboard: false,
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
            is_researching: true,
            ruler_entity: None,
            ruler_system: None,
            ruler_aboard: false,
        };

        let mut policy = SimpleNpcPolicy::default();
        let cmds = policy.decide(
            "test_faction",
            Entity::from_raw_u32(1).unwrap(),
            10,
            &bus,
            &ctx,
        );

        // Fortify + build_ship (combat_count < 3 with can_build=1.0)
        assert!(
            cmds.iter().any(|c| c.kind.as_str() == "fortify_system"),
            "should emit fortify_system when few ships"
        );
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
                ("free_building_slots", 0.0),
                ("tech_unlocks_available", 0.0),
            ],
        );

        // Provide 3 combat ships so fleet composition rule doesn't trigger
        let combat_ships: Vec<ShipInfo> = (0..3)
            .map(|i| ShipInfo {
                entity: Entity::from_raw_u32(200 + i).unwrap(),
                design_id: "patrol_corvette".into(),
                system: Some(Entity::from_raw_u32(1).unwrap()),
                is_idle: true,
                can_survey: false,
                can_colonize: false,
                is_combat: true,
                ftl_range: 15.0,
            })
            .collect();

        let ctx = NpcContext {
            hostile_systems: vec![],
            unsurveyed_systems: vec![],
            colonizable_systems: vec![],
            ships: combat_ships,
            is_researching: true,
            ruler_entity: None,
            ruler_system: None,
            ruler_aboard: false,
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
            is_researching: false,
            ruler_entity: None,
            ruler_system: None,
            ruler_aboard: false,
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

    #[test]
    fn simple_policy_emits_research_when_not_researching() {
        let bus = bus_with_metrics(
            test_faction_id(),
            &[
                ("my_total_ships", 8.0),
                ("my_fleet_ready", 0.9),
                ("colony_count", 3.0),
                ("can_build_ships", 0.0),
                ("tech_unlocks_available", 3.0),
                ("free_building_slots", 0.0),
            ],
        );

        let ctx = NpcContext {
            hostile_systems: vec![],
            unsurveyed_systems: vec![],
            colonizable_systems: vec![],
            ships: vec![],
            is_researching: false,
            ruler_entity: None,
            ruler_system: None,
            ruler_aboard: false,
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
            cmds.iter().any(|c| c.kind.as_str() == "research_focus"),
            "should emit research_focus when not researching and techs available"
        );
    }

    #[test]
    fn simple_policy_no_research_when_already_researching() {
        let bus = bus_with_metrics(
            test_faction_id(),
            &[
                ("my_total_ships", 8.0),
                ("my_fleet_ready", 0.9),
                ("colony_count", 3.0),
                ("can_build_ships", 0.0),
                ("tech_unlocks_available", 3.0),
                ("free_building_slots", 0.0),
            ],
        );

        let ctx = NpcContext {
            hostile_systems: vec![],
            unsurveyed_systems: vec![],
            colonizable_systems: vec![],
            ships: vec![],
            is_researching: true,
            ruler_entity: None,
            ruler_system: None,
            ruler_aboard: false,
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
            cmds.iter().all(|c| c.kind.as_str() != "research_focus"),
            "should not emit research_focus when already researching"
        );
    }

    #[test]
    fn simple_policy_builds_power_plant_when_energy_negative() {
        let bus = bus_with_metrics(
            test_faction_id(),
            &[
                ("my_total_ships", 8.0),
                ("my_fleet_ready", 0.9),
                ("colony_count", 3.0),
                ("can_build_ships", 0.0),
                ("free_building_slots", 2.0),
                ("net_production_energy", -5.0),
                ("net_production_food", 10.0),
            ],
        );

        let ctx = NpcContext {
            hostile_systems: vec![],
            unsurveyed_systems: vec![],
            colonizable_systems: vec![],
            ships: vec![],
            is_researching: true,
            ruler_entity: None,
            ruler_system: None,
            ruler_aboard: false,
        };

        let mut policy = SimpleNpcPolicy::default();
        let cmds = policy.decide(
            "test_faction",
            Entity::from_raw_u32(1).unwrap(),
            10,
            &bus,
            &ctx,
        );

        let build_cmd = cmds
            .iter()
            .find(|c| c.kind.as_str() == "build_structure")
            .expect("should emit build_structure");
        match build_cmd.params.get("building_id") {
            Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "power_plant"),
            _ => panic!("expected building_id param"),
        }
    }

    #[test]
    fn simple_policy_builds_farm_when_food_negative() {
        let bus = bus_with_metrics(
            test_faction_id(),
            &[
                ("my_total_ships", 8.0),
                ("my_fleet_ready", 0.9),
                ("colony_count", 3.0),
                ("can_build_ships", 0.0),
                ("free_building_slots", 2.0),
                ("net_production_energy", 5.0),
                ("net_production_food", -3.0),
            ],
        );

        let ctx = NpcContext {
            hostile_systems: vec![],
            unsurveyed_systems: vec![],
            colonizable_systems: vec![],
            ships: vec![],
            is_researching: true,
            ruler_entity: None,
            ruler_system: None,
            ruler_aboard: false,
        };

        let mut policy = SimpleNpcPolicy::default();
        let cmds = policy.decide(
            "test_faction",
            Entity::from_raw_u32(1).unwrap(),
            10,
            &bus,
            &ctx,
        );

        let build_cmd = cmds
            .iter()
            .find(|c| c.kind.as_str() == "build_structure")
            .expect("should emit build_structure");
        match build_cmd.params.get("building_id") {
            Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "farm"),
            _ => panic!("expected building_id param"),
        }
    }

    #[test]
    fn simple_policy_builds_mine_by_default() {
        let bus = bus_with_metrics(
            test_faction_id(),
            &[
                ("my_total_ships", 8.0),
                ("my_fleet_ready", 0.9),
                ("colony_count", 3.0),
                ("can_build_ships", 0.0),
                ("free_building_slots", 1.0),
                ("net_production_energy", 5.0),
                ("net_production_food", 5.0),
            ],
        );

        let ctx = NpcContext {
            hostile_systems: vec![],
            unsurveyed_systems: vec![],
            colonizable_systems: vec![],
            ships: vec![],
            is_researching: true,
            ruler_entity: None,
            ruler_system: None,
            ruler_aboard: false,
        };

        let mut policy = SimpleNpcPolicy::default();
        let cmds = policy.decide(
            "test_faction",
            Entity::from_raw_u32(1).unwrap(),
            10,
            &bus,
            &ctx,
        );

        let build_cmd = cmds
            .iter()
            .find(|c| c.kind.as_str() == "build_structure")
            .expect("should emit build_structure");
        match build_cmd.params.get("building_id") {
            Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "mine"),
            _ => panic!("expected building_id param"),
        }
    }

    #[test]
    fn simple_policy_builds_explorer_when_no_survey_ships() {
        let unsurveyed = Entity::from_raw_u32(50).unwrap();
        let bus = bus_with_metrics(
            test_faction_id(),
            &[
                ("my_total_ships", 3.0),
                ("my_fleet_ready", 0.9),
                ("colony_count", 1.0),
                ("can_build_ships", 1.0),
                ("free_building_slots", 0.0),
            ],
        );

        // 3 combat ships, no survey ships, unsurveyed systems exist
        let combat_ships: Vec<ShipInfo> = (0..3)
            .map(|i| ShipInfo {
                entity: Entity::from_raw_u32(200 + i).unwrap(),
                design_id: "patrol_corvette".into(),
                system: Some(Entity::from_raw_u32(1).unwrap()),
                is_idle: true,
                can_survey: false,
                can_colonize: false,
                is_combat: true,
                ftl_range: 15.0,
            })
            .collect();

        let ctx = NpcContext {
            hostile_systems: vec![],
            unsurveyed_systems: vec![unsurveyed],
            colonizable_systems: vec![],
            ships: combat_ships,
            is_researching: true,
            ruler_entity: None,
            ruler_system: None,
            ruler_aboard: false,
        };

        let mut policy = SimpleNpcPolicy::default();
        let cmds = policy.decide(
            "test_faction",
            Entity::from_raw_u32(1).unwrap(),
            10,
            &bus,
            &ctx,
        );

        let build_cmd = cmds
            .iter()
            .find(|c| c.kind.as_str() == "build_ship")
            .expect("should emit build_ship for explorer");
        match build_cmd.params.get("design_id") {
            Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "explorer_mk1"),
            _ => panic!("expected design_id param"),
        }
    }

    #[test]
    fn simple_policy_builds_colony_ship_when_no_colonizers() {
        let colonizable = Entity::from_raw_u32(50).unwrap();
        let bus = bus_with_metrics(
            test_faction_id(),
            &[
                ("my_total_ships", 4.0),
                ("my_fleet_ready", 0.9),
                ("colony_count", 1.0),
                ("can_build_ships", 1.0),
                ("free_building_slots", 0.0),
            ],
        );

        // 3 combat ships + 1 survey ship, no colony ships
        let mut ships: Vec<ShipInfo> = (0..3)
            .map(|i| ShipInfo {
                entity: Entity::from_raw_u32(200 + i).unwrap(),
                design_id: "patrol_corvette".into(),
                system: Some(Entity::from_raw_u32(1).unwrap()),
                is_idle: true,
                can_survey: false,
                can_colonize: false,
                is_combat: true,
                ftl_range: 15.0,
            })
            .collect();
        ships.push(ShipInfo {
            entity: Entity::from_raw_u32(300).unwrap(),
            design_id: "explorer_mk1".into(),
            system: Some(Entity::from_raw_u32(1).unwrap()),
            is_idle: true,
            can_survey: true,
            can_colonize: false,
            is_combat: false,
            ftl_range: 15.0,
        });

        let ctx = NpcContext {
            hostile_systems: vec![],
            unsurveyed_systems: vec![],
            colonizable_systems: vec![colonizable],
            ships,
            is_researching: true,
            ruler_entity: None,
            ruler_system: None,
            ruler_aboard: false,
        };

        let mut policy = SimpleNpcPolicy::default();
        let cmds = policy.decide(
            "test_faction",
            Entity::from_raw_u32(1).unwrap(),
            10,
            &bus,
            &ctx,
        );

        let build_cmd = cmds
            .iter()
            .find(|c| c.kind.as_str() == "build_ship")
            .expect("should emit build_ship for colony ship");
        match build_cmd.params.get("design_id") {
            Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "colony_ship_mk1"),
            _ => panic!("expected design_id param"),
        }
    }

    #[test]
    fn simple_policy_builds_combat_ship_when_few_combat() {
        let bus = bus_with_metrics(
            test_faction_id(),
            &[
                ("my_total_ships", 2.0),
                ("my_fleet_ready", 0.9),
                ("colony_count", 1.0),
                ("can_build_ships", 1.0),
                ("free_building_slots", 0.0),
            ],
        );

        // 1 survey + 1 combat = only 1 combat ship (< 3 threshold)
        let ships = vec![
            ShipInfo {
                entity: Entity::from_raw_u32(200).unwrap(),
                design_id: "explorer_mk1".into(),
                system: Some(Entity::from_raw_u32(1).unwrap()),
                is_idle: true,
                can_survey: true,
                can_colonize: false,
                is_combat: false,
                ftl_range: 15.0,
            },
            ShipInfo {
                entity: Entity::from_raw_u32(201).unwrap(),
                design_id: "patrol_corvette".into(),
                system: Some(Entity::from_raw_u32(1).unwrap()),
                is_idle: true,
                can_survey: false,
                can_colonize: false,
                is_combat: true,
                ftl_range: 15.0,
            },
        ];

        let ctx = NpcContext {
            hostile_systems: vec![],
            unsurveyed_systems: vec![],
            colonizable_systems: vec![],
            ships,
            is_researching: true,
            ruler_entity: None,
            ruler_system: None,
            ruler_aboard: false,
        };

        let mut policy = SimpleNpcPolicy::default();
        let cmds = policy.decide(
            "test_faction",
            Entity::from_raw_u32(1).unwrap(),
            10,
            &bus,
            &ctx,
        );

        let build_cmd = cmds
            .iter()
            .find(|c| c.kind.as_str() == "build_ship")
            .expect("should emit build_ship for combat");
        match build_cmd.params.get("design_id") {
            Some(CommandValue::Str(s)) => assert_eq!(s.as_ref(), "patrol_corvette"),
            _ => panic!("expected design_id param"),
        }
    }

    #[test]
    fn simple_policy_emits_move_ruler_with_attack() {
        let bus = bus_with_metrics(
            test_faction_id(),
            &[
                ("my_total_ships", 5.0),
                ("my_fleet_ready", 0.8),
                ("colony_count", 3.0),
                ("can_build_ships", 1.0),
            ],
        );

        let hostile_sys = Entity::from_raw_u32(42).unwrap();
        let combat_ship = Entity::from_raw_u32(100).unwrap();
        let ruler_entity = Entity::from_raw_u32(999).unwrap();
        let ruler_system = Entity::from_raw_u32(1).unwrap();

        let ctx = NpcContext {
            hostile_systems: vec![hostile_sys],
            unsurveyed_systems: vec![],
            colonizable_systems: vec![],
            ships: vec![ShipInfo {
                entity: combat_ship,
                design_id: "corvette".into(),
                system: Some(ruler_system),
                is_idle: true,
                can_survey: false,
                can_colonize: false,
                is_combat: true,
                ftl_range: 15.0,
            }],
            is_researching: false,
            ruler_entity: Some(ruler_entity),
            ruler_system: Some(ruler_system),
            ruler_aboard: false,
        };

        let mut policy = SimpleNpcPolicy::default();
        let cmds = policy.decide(
            "test_faction",
            test_faction_entity(),
            10,
            &bus,
            &ctx,
        );

        assert_eq!(cmds.len(), 2, "should emit attack_target + move_ruler");
        assert_eq!(cmds[0].kind.as_str(), "attack_target");
        assert_eq!(cmds[1].kind.as_str(), "move_ruler");
        match cmds[1].params.get("target_system") {
            Some(CommandValue::System(sys_ref)) => {
                let entity = crate::ai::convert::from_ai_system(*sys_ref);
                assert_eq!(entity, hostile_sys);
            }
            _ => panic!("expected target_system param on move_ruler"),
        }
    }

    #[test]
    fn simple_policy_no_move_ruler_when_already_aboard() {
        let bus = bus_with_metrics(
            test_faction_id(),
            &[
                ("my_total_ships", 5.0),
                ("my_fleet_ready", 0.8),
                ("colony_count", 3.0),
                ("can_build_ships", 1.0),
            ],
        );

        let hostile_sys = Entity::from_raw_u32(42).unwrap();
        let combat_ship = Entity::from_raw_u32(100).unwrap();
        let ruler_entity = Entity::from_raw_u32(999).unwrap();
        let ruler_system = Entity::from_raw_u32(1).unwrap();

        let ctx = NpcContext {
            hostile_systems: vec![hostile_sys],
            unsurveyed_systems: vec![],
            colonizable_systems: vec![],
            ships: vec![ShipInfo {
                entity: combat_ship,
                design_id: "corvette".into(),
                system: Some(ruler_system),
                is_idle: true,
                can_survey: false,
                can_colonize: false,
                is_combat: true,
                ftl_range: 15.0,
            }],
            is_researching: false,
            ruler_entity: Some(ruler_entity),
            ruler_system: Some(ruler_system),
            ruler_aboard: true, // already aboard
        };

        let mut policy = SimpleNpcPolicy::default();
        let cmds = policy.decide(
            "test_faction",
            test_faction_entity(),
            10,
            &bus,
            &ctx,
        );

        assert_eq!(cmds.len(), 1, "should only emit attack_target, not move_ruler");
        assert_eq!(cmds[0].kind.as_str(), "attack_target");
    }
}
