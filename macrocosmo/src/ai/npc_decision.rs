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

use crate::ai::assignments::{AssignmentKind, AssignmentTarget, PendingAssignment};
use crate::ai::command_outbox::AiCommandOutbox;
use crate::ai::convert::{from_ai_system, to_ai_faction, to_ai_system};
use crate::ai::plugin::AiBusResource;
use crate::ai::schema::ids::{command as cmd_ids, metric};
use crate::knowledge::KnowledgeStore;
use crate::player::{AboardShip, Empire, EmpireRuler, Faction, PlayerEmpire, Ruler, StationedAt};
use crate::technology::ResearchQueue;
use crate::time_system::GameClock;

/// Marker component: this empire's decisions are made by the AI policy.
/// Applied to NPC empires automatically, and optionally to the player
/// empire when `--ai-player` is passed or `AiPlayerMode(true)` is set.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct AiControlled;

/// Resource that opts the player empire into AI control.
/// Default is `false` — normal gameplay where the player makes decisions.
/// Set to `true` to let the AI policy drive the player empire.
#[derive(Resource, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Resource)]
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
            for (ship, &target) in idle_surveyors.iter().zip(context.unsurveyed_systems.iter()) {
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

        // Rule 5a: System building — construct a shipyard when a
        // Core-equipped system exists but no shipyard does. Without this
        // the empire can never reach `can_build_ships == 1.0`, blocking
        // Rules 6/8 permanently. `systems_with_core > 0` is the #370 gate.
        // The handler-side dedup (`handle_build_structure` skips if the
        // same building id is already queued) absorbs per-tick re-emission
        // while the queue drains.
        let systems_with_core = bus
            .current(&metric::for_faction("systems_with_core", faction_id))
            .unwrap_or(0.0);
        if can_build < 1.0 && systems_with_core > 0.0 && colony_count > 0.0 {
            let cmd = Command::new(cmd_ids::build_structure(), faction_id.clone(), now)
                .with_param("building_id", CommandValue::Str("shipyard".into()));
            commands.push(cmd);
        }

        // Rule 5b: Colony building — fill empty building slots
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
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct LastAiDecisionTick(pub i64);

/// Rank candidate unsurveyed systems by "accessibility" — an approximation
/// of travel time that's more useful than raw distance.
///
/// FTL routing ([`crate::ship::movement::plan_ftl_route`]) rejects
/// unsurveyed destinations, so reaching an unsurveyed star always ends in
/// a sublight leg from the nearest surveyed waypoint. That sublight gap
/// dominates the surveyor's travel time in the common case, so we rank
/// targets by:
///   1. `gap` — distance from the target to the nearest surveyed system
///      (smaller = closer to the frontier of known space).
///   2. `home_dist` — distance from the target to the empire's reference
///      position (ruler's home, or fallback). Tie-breaks same-gap targets
///      by "prefer systems closer to our base."
///
/// When the empire has no surveyed systems at all (fresh start, pre-capital),
/// gap collapses to raw distance from the reference position — good enough
/// for the first dispatch.
///
/// Returns entity ids in rank order (best first).
pub fn rank_survey_targets(
    candidates: &[(Entity, [f64; 3])],
    surveyed_positions: &[[f64; 3]],
    reference_pos: [f64; 3],
) -> Vec<Entity> {
    let mut scored: Vec<(Entity, f64, f64)> = candidates
        .iter()
        .map(|(e, pos)| {
            let gap = if surveyed_positions.is_empty() {
                crate::physics::distance_ly_arr(*pos, reference_pos)
            } else {
                surveyed_positions
                    .iter()
                    .map(|sp| crate::physics::distance_ly_arr(*pos, *sp))
                    .fold(f64::INFINITY, f64::min)
            };
            let home_dist = crate::physics::distance_ly_arr(*pos, reference_pos);
            (*e, gap, home_dist)
        })
        .collect();
    scored.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
    });
    scored.into_iter().map(|(e, _, _)| e).collect()
}

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
    // SimpleNpcPolicy needs to know which systems exist at all — the
    // KnowledgeStore only carries entries the empire has already
    // surveyed / been told about (one entry per owned capital at spawn),
    // so `unsurveyed_systems` derived from it was always empty for
    // fresh empires, freezing Explorers in dock.
    //
    // Position is pulled alongside so we can rank survey targets by
    // "accessibility" — distance alone is a poor proxy for travel time
    // in a universe with FTL, obscured regions, and a surveyed-frontier
    // requirement.
    star_positions: Query<(Entity, &crate::components::Position), With<crate::galaxy::StarSystem>>,
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
    // Round 9 PR #2 Step 4: dedup against in-flight survey dispatches so
    // we don't double-assign two surveyors to the same target across
    // overlapping decision ticks. Marker is per-faction.
    pending_assignments: Query<(Entity, &PendingAssignment)>,
    // Round 11 Bug A: also dedup against commands sitting in the
    // light-speed outbox — between `bus.emit_command` (here) and
    // handler insert (`drain_ai_commands` → `PendingAssignment`),
    // a `survey_system` / `colonize_system` may live in
    // `AiCommandOutbox.entries` for hundreds of hexadies. Without
    // this, `mid_cadence=2` re-fires every other tick onto the same
    // target. See `tests/ai_npc_outbox_dedup.rs` for the regression.
    outbox: Res<AiCommandOutbox>,
    // #299 / #446 short-term loop fix: only systems hosting one of the
    // empire's own Cores are colonizable. Without this filter the AI
    // re-emits `colonize_system` every tick for systems where the
    // settling handler will reject the order on Core sovereignty grounds.
    // Long-term plan (#446 / #447): give the AI explicit `deploy_core`
    // commands and let the Short layer decompose colonize → deploy + colonize.
    core_ships: Query<
        (&crate::galaxy::AtSystem, &crate::faction::FactionOwner),
        With<crate::ship::CoreShip>,
    >,
    mut policy: Local<SimpleNpcPolicy>,
    // #448 PR2b: AiPolicyMode gate. Default `Legacy` runs the
    // existing `SimpleNpcPolicy` path; `Layered` runs a no-op
    // scaffold that PR2c/2d fill with rule ports. The branch lives
    // inside the per-empire loop below so swapping modes per tick
    // takes effect on the next decision.
    policy_mode: Res<crate::ai::mid_adapter::AiPolicyMode>,
    #[cfg(feature = "ai-log")] mut log: Option<ResMut<super::debug_log::AiLogConfig>>,
) {
    use crate::knowledge::SystemVisibilityTier;

    let now = clock.elapsed;
    if now <= last_tick.0 {
        return;
    }
    last_tick.0 = now;

    // #299 / #446: precompute per-empire "systems with our own Core"
    // before the empire loop. Used to filter `colonizable_systems` —
    // without this gate, NPCs re-emit `colonize_system` every tick for
    // targets that the settling handler will reject on Core sovereignty
    // grounds (Bug 4 in handoff doc).
    let mut core_systems_per_empire: std::collections::HashMap<
        Entity,
        std::collections::HashSet<Entity>,
    > = std::collections::HashMap::new();
    for (at, owner) in &core_ships {
        core_systems_per_empire
            .entry(owner.0)
            .or_default()
            .insert(at.0);
    }

    // Round 11 Bug A: precompute per-empire "in-flight survey /
    // colonize targets" from the light-speed outbox. The handler
    // insert path (`PendingAssignment` marker) only covers commands
    // that have already arrived; a command emitted on tick N with a
    // 30-hex light delay sits in `outbox.entries` until tick N+30,
    // and without this scan a `mid_cadence=2` `npc_decision_tick`
    // would happily re-fire onto the same target every other tick
    // for the entire delay window. The scan is a single pass over
    // outbox entries (typically small — only commands currently in
    // flight), so the cost is bounded and amortised across all
    // empires in the loop below.
    let survey_kind = cmd_ids::survey_system();
    let colonize_kind = cmd_ids::colonize_system();
    let mut outbox_survey_per_empire: std::collections::HashMap<
        Entity,
        std::collections::HashSet<Entity>,
    > = std::collections::HashMap::new();
    let mut outbox_colonize_per_empire: std::collections::HashMap<
        Entity,
        std::collections::HashSet<Entity>,
    > = std::collections::HashMap::new();
    // Both maps are mutated only inside the next block (single pass over
    // outbox.entries); the empire loop below reads via shared `&` only.
    if !outbox.entries.is_empty() {
        // Build issuer FactionId → empire Entity once (faction_id
        // encodes only `Entity::index()`, see `to_ai_faction`); then
        // each entry is an O(1) hashmap lookup instead of an O(empires)
        // scan.
        let mut faction_to_empire: std::collections::HashMap<macrocosmo_ai::FactionId, Entity> =
            std::collections::HashMap::new();
        for (entity, _, _, _) in &npcs {
            faction_to_empire.insert(to_ai_faction(entity), entity);
        }
        for entry in &outbox.entries {
            let cmd = &entry.command;
            let Some(&empire_entity) = faction_to_empire.get(&cmd.issuer) else {
                continue;
            };
            let target_set = if cmd.kind.as_str() == survey_kind.as_str() {
                Some(&mut outbox_survey_per_empire)
            } else if cmd.kind.as_str() == colonize_kind.as_str() {
                Some(&mut outbox_colonize_per_empire)
            } else {
                None
            };
            let Some(target_set) = target_set else {
                continue;
            };
            if let Some(macrocosmo_ai::CommandValue::System(s)) = cmd.params.get("target_system") {
                target_set
                    .entry(empire_entity)
                    .or_default()
                    .insert(from_ai_system(*s));
            }
        }
    }

    for (entity, faction, knowledge, vis_map_opt) in &npcs {
        // Round 9 PR #2 Step 4: pre-collect this faction's in-flight
        // assignments so we can filter both ship and target candidates.
        // `pending_survey_targets` excludes systems already being
        // surveyed by one of our ships; `pending_assigned_ships`
        // excludes ships already carrying a marker (defense in depth —
        // by the time the handler runs, queue.is_empty() is also false,
        // but the marker covers the same-tick race).
        let mut pending_survey_targets: std::collections::HashSet<Entity> =
            std::collections::HashSet::new();
        let mut pending_assigned_ships: std::collections::HashSet<Entity> =
            std::collections::HashSet::new();
        for (ship_entity, pa) in &pending_assignments {
            if pa.faction != entity {
                continue;
            }
            pending_assigned_ships.insert(ship_entity);
            if pa.kind == AssignmentKind::Survey {
                if let AssignmentTarget::System(sys) = pa.target {
                    pending_survey_targets.insert(sys);
                }
            }
        }
        // Round 11 Bug A: union in outbox-resident in-flight commands
        // (handler hasn't inserted markers yet because the light-speed
        // window hasn't elapsed). Covers both decision-tick paths the
        // handler-side dedup misses.
        if let Some(set) = outbox_survey_per_empire.get(&entity) {
            pending_survey_targets.extend(set.iter().copied());
        }
        let pending_colonize_targets: std::collections::HashSet<Entity> =
            outbox_colonize_per_empire
                .get(&entity)
                .cloned()
                .unwrap_or_default();

        // Extract system intel. Hostile / colonizable signals still come
        // from the KnowledgeStore (those require detailed snapshots),
        // but `unsurveyed_systems` is derived from the galaxy-wide star
        // list minus whatever the empire has already surveyed —
        // otherwise freshly-spawned empires never find survey targets
        // because their KnowledgeStore is empty aside from the capital.
        let mut hostile_systems = Vec::new();
        let mut hostile_systems_set: std::collections::HashSet<Entity> =
            std::collections::HashSet::new();
        let mut colonizable_systems = Vec::new();
        let mut surveyed_ids: std::collections::HashSet<Entity> = std::collections::HashSet::new();
        // #299 / #446 short-term: limit colonization candidates to
        // systems where this empire already has a Core deployed. Without
        // an empty set this collapses to "no colonization possible" —
        // matching the settling handler's reject behavior, so the AI
        // stops looping on impossible orders. Once #446 lands and the AI
        // can issue `deploy_core`, this gate falls away naturally.
        let empty_core_set: std::collections::HashSet<Entity> = std::collections::HashSet::new();
        let owned_core_systems = core_systems_per_empire
            .get(&entity)
            .unwrap_or(&empty_core_set);
        for (_, k) in knowledge.iter() {
            if k.data.has_hostile {
                hostile_systems.push(k.system);
                hostile_systems_set.insert(k.system);
            }
            if k.data.surveyed {
                surveyed_ids.insert(k.system);
                // Bug B fix: skip systems known-hostile so the AI doesn't
                // ferry colonists into a meat grinder. Rule 1 (attack)
                // still consumes `hostile_systems` separately.
                if !k.data.colonized
                    && !k.data.has_hostile
                    && owned_core_systems.contains(&k.system)
                    // Round 11 Bug A: skip targets with a `colonize_system`
                    // already in the light-speed outbox — without this,
                    // the policy re-emits onto the same target every
                    // mid_cadence tick until the command lands.
                    && !pending_colonize_targets.contains(&k.system)
                {
                    colonizable_systems.push(k.system);
                }
            }
        }
        // Every catalogued system (which, right now, means every system
        // in the galaxy thanks to `initialize_visibility_tiers`) is a
        // valid survey target if we haven't surveyed it yet. Fall back
        // to all stars when the empire has no visibility map — defensive
        // for test setups.
        let surveyed_positions: Vec<[f64; 3]> = star_positions
            .iter()
            .filter(|(e, _)| surveyed_ids.contains(e))
            .map(|(_, p)| p.as_array())
            .collect();

        // Resolve the empire's reference position for tiebreaks.
        let ruler_stationed_system: Option<Entity> = empire_rulers
            .get(entity)
            .ok()
            .and_then(|er| ruler_q.get(er.0).ok())
            .map(|(s, _)| s.system);
        let reference_pos: [f64; 3] = ruler_stationed_system
            .and_then(|sys| {
                star_positions
                    .iter()
                    .find(|(e, _)| *e == sys)
                    .map(|(_, p)| p.as_array())
            })
            .or_else(|| surveyed_positions.first().copied())
            .unwrap_or([0.0, 0.0, 0.0]);

        let candidates: Vec<(Entity, [f64; 3])> = star_positions
            .iter()
            .filter(|(e, _)| !surveyed_ids.contains(e))
            // Round 9 PR #2 Step 4: skip targets that already have a
            // surveyor in flight — prevents the "Vesk Scout-2 chases Vesk
            // Scout-1" double dispatch the handler-side dedup couldn't
            // catch from prior commits.
            .filter(|(e, _)| !pending_survey_targets.contains(e))
            // Bug B fix: skip systems we already know are hostile;
            // re-surveying a confirmed-hostile system loses scouts in a
            // tight loop. ROE-based engagement still flows through Rule 1.
            .filter(|(e, _)| !hostile_systems_set.contains(e))
            .filter(|(e, _)| {
                vis_map_opt
                    .map(|vm| vm.get(*e) >= SystemVisibilityTier::Catalogued)
                    .unwrap_or(true)
            })
            .map(|(e, p)| (e, p.as_array()))
            .collect();
        let unsurveyed_systems =
            rank_survey_targets(&candidates, &surveyed_positions, reference_pos);

        // Build ship inventory for this empire.
        // Round 9 PR #2 Step 4: ships carrying a `PendingAssignment` are
        // treated as non-idle even if their command queue hasn't yet been
        // populated by the handler (the marker is the AI-side "intent
        // already issued" signal). This closes the same-tick race where
        // `drain_ai_commands` writes a `SurveyRequested` event but
        // `handle_survey_requested` hasn't yet pushed the `MoveTo` /
        // `Survey` pair into the queue.
        let ships: Vec<ShipInfo> = all_ships
            .iter()
            .filter(|(_, ship, _, _)| ship.owner == crate::ship::Owner::Empire(entity))
            .map(|(ship_entity, ship, state, queue)| {
                let system = match state {
                    crate::ship::ShipState::InSystem { system } => Some(*system),
                    _ => None,
                };
                let has_pending = pending_assigned_ships.contains(&ship_entity);
                let is_idle = system.is_some() && queue.commands.is_empty() && !has_pending;
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

        // #448 PR2c: AiPolicyMode gate. Layered now routes through
        // `MidStanceAgent::decide` which ports Rules 1 + 5a; the
        // remaining Rules (2/3/4/5b/6/7/8) still land in PR2d. Until
        // every rule is ported the parity test
        // (`tests/ai_layered_parity.rs`) restricts itself to fixtures
        // where only Rule 1 / 5a fire. Default = Legacy → all
        // existing production paths and tests untouched.
        let commands: Vec<Command> = match *policy_mode {
            crate::ai::mid_adapter::AiPolicyMode::Legacy => {
                policy.decide(&faction.id, entity, now, &bus.0, &context)
            }
            crate::ai::mid_adapter::AiPolicyMode::Layered => {
                // Pre-compute idle_combat / idle_colonizers with the
                // same expressions `SimpleNpcPolicy::decide` uses
                // (Rule 1 / Rule 3) so the adapter can hand them
                // straight to MidStanceAgent without re-scanning the
                // ship list. Rule 2 (`idle_surveyors`) stays in the
                // legacy path until the Short layer migration in
                // #449 — see `MidStanceAgent::decide` rule comments.
                let idle_combat: Vec<Entity> = context
                    .ships
                    .iter()
                    .filter(|s| s.is_idle && s.is_combat)
                    .map(|s| s.entity)
                    .collect();
                let idle_colonizers: Vec<Entity> = context
                    .ships
                    .iter()
                    .filter(|s| s.is_idle && s.can_colonize)
                    .map(|s| s.entity)
                    .collect();
                let adapter = crate::ai::mid_adapter::BevyMidGameAdapter {
                    faction: entity,
                    context: &context,
                    bus: &bus.0,
                    idle_combat: &idle_combat,
                    idle_colonizers: &idle_colonizers,
                };
                let proposals = crate::ai::mid_adapter::layered_decide(
                    &adapter,
                    &macrocosmo_ai::Stance::default(),
                    &faction.id,
                    now,
                );
                crate::ai::mid_adapter::arbitrate(proposals)
            }
        };
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
            bus.declare_metric(
                id.clone(),
                macrocosmo_ai::MetricSpec::gauge(
                    macrocosmo_ai::Retention::Medium,
                    "per-faction self metric",
                ),
            );
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
        let cmds = policy.decide("test_faction", test_faction_entity(), 10, &bus, &ctx);

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
        let cmds = policy.decide("test_faction", test_faction_entity(), 10, &bus, &ctx);

        assert_eq!(
            cmds.len(),
            1,
            "should only emit attack_target, not move_ruler"
        );
        assert_eq!(cmds[0].kind.as_str(), "attack_target");
    }
}
