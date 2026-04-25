//! Command drain consumer — converts AI bus commands into ECS game actions.
//!
//! Registered under [`AiTickSet::CommandDrain`](super::AiTickSet::CommandDrain).
//! Each tick, drains pending commands from the bus and applies them:
//!
//! - `attack_target` → find idle ships owned by the issuing faction, emit
//!   `MoveRequested` for the target system.
//! - `retreat` → find ships in hostile systems, emit `MoveRequested` back to
//!   the faction's home system (system with most colonies).
//! - `build_ship` → queue ship construction at a system with a shipyard.
//! - `fortify_system` → queue a default combat ship at a system with a shipyard.
//! - `research_focus` → set the empire's active research target.
//! - `build_structure` → queue a building at a colony.
//! - `reposition` / `blockade` → move ships to a target system.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use macrocosmo_ai::CommandValue;

use crate::ai::convert::{from_ai_system, to_ai_faction};
use crate::ai::emit::AiBusDrainer;
use crate::ai::schema::ids::command as cmd_ids;
use crate::amount::Amt;
use crate::colony::building_queue::{
    BuildKind, BuildOrder, BuildQueue, BuildingOrder, BuildingQueue, Buildings,
};
use crate::colony::system_buildings::SlotAssignment;
use crate::colony::{BuildingRegistry, Colony};
use crate::components::Position;
use crate::galaxy::{AtSystem, Hostile, Planet, Sovereignty, StarSystem};
use crate::physics::distance_ly;
use crate::player::{AboardShip, Empire, EmpireRuler, Faction, Ruler, StationedAt};
use crate::ship::command_events::{
    ColonizeRequested, CommandId, MoveRequested, NextCommandId, SurveyRequested,
};
use crate::ship::{CommandQueue, Owner, Ship, ShipState};
use crate::ship_design::ShipDesignRegistry;
use crate::technology::{ResearchQueue, TechId, TechTree};
use crate::time_system::GameClock;

/// Queued ruler boarding requests produced by `drain_ai_commands` and
/// consumed by [`process_ruler_boarding`]. This indirection avoids adding
/// mutable Ship access to `drain_ai_commands` (which would conflict with
/// the existing read-only Ship query).
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct PendingRulerBoarding {
    /// `(ruler_entity, ship_entity, target_system)`
    pub requests: Vec<(Entity, Entity, Entity)>,
}

/// Extra queries needed by build / research / structure commands, bundled
/// into a `SystemParam` to keep `drain_ai_commands` under Bevy's 16-param
/// limit.
#[derive(SystemParam)]
pub struct BuildResearchParams<'w, 's> {
    design_registry: Option<Res<'w, ShipDesignRegistry>>,
    building_registry: Option<Res<'w, BuildingRegistry>>,
    build_queues: Query<'w, 's, &'static mut BuildQueue>,
    station_ships: Query<
        'w,
        's,
        (
            Entity,
            &'static Ship,
            &'static ShipState,
            &'static SlotAssignment,
        ),
    >,
    sys_mods_q: Query<'w, 's, &'static crate::galaxy::SystemModifiers>,
    empire_tech: Query<'w, 's, (&'static mut TechTree, &'static mut ResearchQueue), With<Empire>>,
    colonies: Query<
        'w,
        's,
        (
            Entity,
            &'static Colony,
            &'static Buildings,
            &'static mut BuildingQueue,
        ),
    >,
    planets: Query<'w, 's, &'static Planet>,
    /// System-level building queues + slot state, used by the system-
    /// building branch of `handle_build_structure` to route shipyard /
    /// port / lab orders to the correct queue.
    system_builds: Query<
        'w,
        's,
        (
            Entity,
            &'static crate::colony::SystemBuildings,
            &'static mut crate::colony::SystemBuildingQueue,
        ),
        With<StarSystem>,
    >,
    /// Tracks which systems host a Core-equipped ship. Required gate for
    /// system-building construction (#370): shipyard / port / lab are only
    /// buildable in systems with an Infrastructure Core.
    core_at_system: Query<'w, 's, &'static crate::galaxy::AtSystem, With<crate::ship::CoreShip>>,
}

/// Drain AI commands from the bus and apply them to the game world.
pub fn drain_ai_commands(
    mut commands_buf: Commands,
    mut drainer: AiBusDrainer,
    ships: Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    sovereignty: Query<(Entity, &Sovereignty), With<StarSystem>>,
    hostiles: Query<&AtSystem, With<Hostile>>,
    empires: Query<(Entity, &Faction), With<Empire>>,
    positions: Query<&Position>,
    mut move_writer: MessageWriter<MoveRequested>,
    mut survey_writer: Option<MessageWriter<SurveyRequested>>,
    mut colonize_writer: Option<MessageWriter<ColonizeRequested>>,
    mut next_cmd_id: ResMut<NextCommandId>,
    clock: Res<GameClock>,
    mut build_research: BuildResearchParams,
    empire_rulers: Query<&EmpireRuler, With<Empire>>,
    ruler_q: Query<(&StationedAt, Option<&AboardShip>), With<Ruler>>,
    mut pending_boarding: ResMut<PendingRulerBoarding>,
) {
    let commands = drainer.drain_commands();
    if commands.is_empty() {
        return;
    }

    for cmd in commands {
        let kind_str = cmd.kind.as_str();

        if kind_str == cmd_ids::attack_target().as_str() {
            handle_attack_target(
                &cmd.issuer,
                &cmd.params,
                &ships,
                &empires,
                &mut move_writer,
                &mut next_cmd_id,
                clock.elapsed,
            );
        } else if kind_str == cmd_ids::retreat().as_str() {
            handle_retreat(
                &cmd.issuer,
                &ships,
                &hostiles,
                &sovereignty,
                &empires,
                &positions,
                &mut move_writer,
                &mut next_cmd_id,
                clock.elapsed,
            );
        } else if kind_str == cmd_ids::survey_system().as_str() {
            if let Some(ref mut w) = survey_writer {
                handle_survey_system(
                    &cmd.issuer,
                    &cmd.params,
                    &ships,
                    &empires,
                    w,
                    &mut next_cmd_id,
                    clock.elapsed,
                    &mut commands_buf,
                );
            }
        } else if kind_str == cmd_ids::colonize_system().as_str() {
            if let Some(ref mut w) = colonize_writer {
                handle_colonize_system(
                    &cmd.issuer,
                    &cmd.params,
                    &ships,
                    &empires,
                    w,
                    &mut next_cmd_id,
                    clock.elapsed,
                );
            }
        } else if kind_str == cmd_ids::build_ship().as_str() {
            handle_build_ship(
                &cmd.issuer,
                &cmd.params,
                &sovereignty,
                &empires,
                &mut build_research,
            );
        } else if kind_str == cmd_ids::fortify_system().as_str() {
            handle_fortify_system(
                &cmd.issuer,
                &cmd.params,
                &sovereignty,
                &empires,
                &mut build_research,
            );
        } else if kind_str == cmd_ids::research_focus().as_str() {
            handle_research_focus(&cmd.issuer, &cmd.params, &empires, &mut build_research);
        } else if kind_str == cmd_ids::build_structure().as_str() {
            handle_build_structure(
                &cmd.issuer,
                &cmd.params,
                &sovereignty,
                &empires,
                &mut build_research,
            );
        } else if kind_str == cmd_ids::reposition().as_str() {
            handle_reposition(
                &cmd.issuer,
                &cmd.params,
                &ships,
                &empires,
                &mut move_writer,
                &mut next_cmd_id,
                clock.elapsed,
            );
        } else if kind_str == cmd_ids::move_ruler().as_str() {
            handle_move_ruler(
                &cmd.issuer,
                &cmd.params,
                &ships,
                &empires,
                &empire_rulers,
                &ruler_q,
                &mut pending_boarding,
                &mut move_writer,
                &mut next_cmd_id,
                clock.elapsed,
            );
        } else if kind_str == cmd_ids::blockade().as_str() {
            handle_blockade(
                &cmd.issuer,
                &cmd.params,
                &ships,
                &empires,
                &mut move_writer,
                &mut next_cmd_id,
                clock.elapsed,
            );
        } else {
            debug!(
                "AI command '{}' from faction {:?} not handled by drain_ai_commands",
                kind_str, cmd.issuer
            );
        }
    }
}

/// Find the empire entity for a given AI FactionId.
fn find_empire_entity(
    issuer: &macrocosmo_ai::FactionId,
    empires: &Query<(Entity, &Faction), With<Empire>>,
) -> Option<Entity> {
    for (entity, _faction) in empires {
        if to_ai_faction(entity) == *issuer {
            return Some(entity);
        }
    }
    None
}

/// Extract ship entity list from indexed command params (`ship_count`,
/// `ship_0`, `ship_1`, ...).
fn extract_ship_list(params: &macrocosmo_ai::CommandParams) -> Vec<Entity> {
    let count = match params.get("ship_count") {
        Some(CommandValue::I64(n)) => *n as usize,
        _ => return vec![],
    };
    (0..count)
        .filter_map(|i| {
            let key = format!("ship_{i}");
            match params.get(key.as_str()) {
                Some(CommandValue::Entity(r)) => Some(crate::ai::convert::from_ai_entity(*r)),
                _ => None,
            }
        })
        .collect()
}

/// Handle `attack_target`: dispatch the ships specified by the AI policy
/// to the target system. The policy is responsible for ship selection —
/// the consumer only validates that each ship is still eligible.
fn handle_attack_target(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    move_writer: &mut MessageWriter<MoveRequested>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    let target_system = match params.get("target_system") {
        Some(CommandValue::System(sys_ref)) => from_ai_system(*sys_ref),
        _ => {
            warn!("attack_target command missing target_system param");
            return;
        }
    };

    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => {
            warn!("attack_target: no empire found for faction {:?}", issuer);
            return;
        }
    };

    let selected_ships = extract_ship_list(params);
    if selected_ships.is_empty() {
        debug!(
            "attack_target: no ships specified by policy for faction {:?}",
            issuer
        );
        return;
    }

    let mut dispatched = 0;
    for ship_entity in selected_ships {
        let Ok((_, ship, state, queue)) = ships.get(ship_entity) else {
            continue; // Ship despawned since policy decided
        };
        if ship.owner != Owner::Empire(empire_entity) {
            continue;
        }
        if !matches!(state, ShipState::InSystem { .. }) || !queue.commands.is_empty() {
            continue;
        }
        if let ShipState::InSystem { system } = state {
            if *system == target_system {
                continue;
            }
        }

        move_writer.write(MoveRequested {
            command_id: next_cmd_id.allocate(),
            ship: ship_entity,
            target: target_system,
            issued_at: now,
        });
        dispatched += 1;
    }

    if dispatched > 0 {
        info!(
            "attack_target: dispatched {} ships from faction {:?} to system {:?}",
            dispatched, issuer, target_system
        );
    }
}

/// Handle `survey_system`: dispatch the specified survey ship to the target system.
///
/// Stamps each dispatched ship with a [`crate::ai::assignments::PendingAssignment`]
/// so subsequent NPC decision ticks can dedup against in-flight surveys
/// (Round 9 PR #2 Step 4). The marker is removed by
/// [`crate::ship::handlers::handle_survey_requested`] on terminal results
/// (Ok / Rejected) and swept after `SURVEY_ASSIGNMENT_LIFETIME` hexadies
/// by [`crate::ai::assignments::sweep_stale_assignments`] in case the
/// handler never fires.
#[allow(clippy::too_many_arguments)]
fn handle_survey_system(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    survey_writer: &mut MessageWriter<SurveyRequested>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
    commands_buf: &mut Commands,
) {
    use crate::ai::assignments::{PendingAssignment, SURVEY_ASSIGNMENT_LIFETIME};

    let target_system = match params.get("target_system") {
        Some(CommandValue::System(sys_ref)) => from_ai_system(*sys_ref),
        _ => {
            warn!("survey_system command missing target_system param");
            return;
        }
    };

    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => return,
    };

    let selected_ships = extract_ship_list(params);
    let mut dispatched = 0;
    for ship_entity in selected_ships {
        let Ok((_, ship, state, queue)) = ships.get(ship_entity) else {
            continue;
        };
        if ship.owner != Owner::Empire(empire_entity) {
            continue;
        }
        if !matches!(state, ShipState::InSystem { .. }) || !queue.commands.is_empty() {
            continue;
        }

        survey_writer.write(SurveyRequested {
            command_id: next_cmd_id.allocate(),
            ship: ship_entity,
            target_system,
            issued_at: now,
        });
        commands_buf
            .entity(ship_entity)
            .insert(PendingAssignment::survey_system(
                empire_entity,
                target_system,
                now,
                SURVEY_ASSIGNMENT_LIFETIME,
            ));
        dispatched += 1;
    }

    if dispatched > 0 {
        info!(
            "survey_system: dispatched {} ships from faction {:?} to system {:?}",
            dispatched, issuer, target_system
        );
    }
}

/// Handle `colonize_system`: dispatch the specified colony ship to the target system.
fn handle_colonize_system(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    colonize_writer: &mut MessageWriter<ColonizeRequested>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    let target_system = match params.get("target_system") {
        Some(CommandValue::System(sys_ref)) => from_ai_system(*sys_ref),
        _ => {
            warn!("colonize_system command missing target_system param");
            return;
        }
    };

    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => return,
    };

    let selected_ships = extract_ship_list(params);
    let mut dispatched = 0;
    for ship_entity in selected_ships {
        let Ok((_, ship, state, queue)) = ships.get(ship_entity) else {
            continue;
        };
        if ship.owner != Owner::Empire(empire_entity) {
            continue;
        }
        if !matches!(state, ShipState::InSystem { .. }) || !queue.commands.is_empty() {
            continue;
        }

        colonize_writer.write(ColonizeRequested {
            command_id: next_cmd_id.allocate(),
            ship: ship_entity,
            target_system,
            planet: None, // Let the handler pick the best planet
            issued_at: now,
        });
        dispatched += 1;
    }

    if dispatched > 0 {
        info!(
            "colonize_system: dispatched {} ships from faction {:?} to system {:?}",
            dispatched, issuer, target_system
        );
    }
}

/// Queue a ship build order at a system owned by the faction that has a
/// shipyard. Returns true if the order was queued successfully.
fn queue_ship_at_shipyard(
    empire_entity: Entity,
    design_id: &str,
    target_system: Option<Entity>,
    sovereignty: &Query<(Entity, &Sovereignty), With<StarSystem>>,
    br: &mut BuildResearchParams,
) -> bool {
    let Some(ref design_registry) = br.design_registry else {
        warn!("build_ship/fortify: ShipDesignRegistry not available");
        return false;
    };
    let Some(design) = design_registry.get(design_id) else {
        warn!("build_ship/fortify: unknown design '{}'", design_id);
        return false;
    };
    if !design.is_direct_buildable {
        warn!(
            "build_ship/fortify: design '{}' is not direct-buildable (installation hull)",
            design_id
        );
        return false;
    }

    // Find a system with a shipyard owned by this empire.
    // Prefer the specified target_system if it qualifies.
    let owned_systems: Vec<Entity> = sovereignty
        .iter()
        .filter(|(_, sov)| sov.owner == Some(Owner::Empire(empire_entity)))
        .map(|(e, _)| e)
        .collect();

    let shipyard_system = if let Some(target) = target_system {
        // Verify target is owned and has a shipyard
        if owned_systems.contains(&target) && has_shipyard_check(target, &br.sys_mods_q) {
            Some(target)
        } else {
            // Fall back to any owned system with a shipyard
            owned_systems
                .iter()
                .find(|&&sys| has_shipyard_check(sys, &br.sys_mods_q))
                .copied()
        }
    } else {
        owned_systems
            .iter()
            .find(|&&sys| has_shipyard_check(sys, &br.sys_mods_q))
            .copied()
    };

    let Some(sys_entity) = shipyard_system else {
        debug!(
            "build_ship/fortify: no system with shipyard found for empire {:?}",
            empire_entity
        );
        return false;
    };

    let build_time = design_registry.build_time(design_id);
    let order = BuildOrder {
        order_id: 0,
        kind: BuildKind::default(),
        design_id: design_id.to_string(),
        display_name: design.name.clone(),
        minerals_cost: design.build_cost_minerals,
        minerals_invested: Amt::ZERO,
        energy_cost: design.build_cost_energy,
        energy_invested: Amt::ZERO,
        build_time_total: build_time,
        build_time_remaining: build_time,
    };

    if let Ok(mut build_queue) = br.build_queues.get_mut(sys_entity) {
        build_queue.push_order(order);
        info!(
            "build_ship/fortify: queued '{}' at system {:?} for empire {:?}",
            design_id, sys_entity, empire_entity
        );
        true
    } else {
        debug!(
            "build_ship/fortify: system {:?} has no BuildQueue component",
            sys_entity
        );
        false
    }
}

/// Check if a system has a shipyard capability via `SystemModifiers`.
fn has_shipyard_check(system: Entity, sys_mods_q: &Query<&crate::galaxy::SystemModifiers>) -> bool {
    sys_mods_q
        .get(system)
        .map(|m| m.shipyard_capacity.value().final_value() > crate::amount::Amt::ZERO)
        .unwrap_or(false)
}

/// Handle `build_ship`: queue construction of the specified ship design at
/// a system with a shipyard owned by the faction.
///
/// Params:
/// - `design_id` (Str): the ship design to build.
/// - `target_system` (System, optional): preferred system to build at.
fn handle_build_ship(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    sovereignty: &Query<(Entity, &Sovereignty), With<StarSystem>>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    br: &mut BuildResearchParams,
) {
    let design_id = match params.get("design_id") {
        Some(CommandValue::Str(s)) => s.to_string(),
        _ => {
            warn!("build_ship command missing design_id param");
            return;
        }
    };

    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => {
            warn!("build_ship: no empire found for faction {:?}", issuer);
            return;
        }
    };

    let target_system = params.get("target_system").and_then(|v| match v {
        CommandValue::System(sys_ref) => Some(from_ai_system(*sys_ref)),
        _ => None,
    });

    queue_ship_at_shipyard(empire_entity, &design_id, target_system, sovereignty, br);
}

/// Handle `fortify_system`: queue construction of a default combat ship
/// design at a system with a shipyard. If no specific design is given,
/// picks the first direct-buildable design from the registry that is not a
/// survey or colony ship.
///
/// Params:
/// - `target_system` (System, optional): the system to fortify.
/// - `design_id` (Str, optional): specific design to build. Auto-picks if absent.
fn handle_fortify_system(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    sovereignty: &Query<(Entity, &Sovereignty), With<StarSystem>>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    br: &mut BuildResearchParams,
) {
    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => {
            warn!("fortify_system: no empire found for faction {:?}", issuer);
            return;
        }
    };

    let target_system = params.get("target_system").and_then(|v| match v {
        CommandValue::System(sys_ref) => Some(from_ai_system(*sys_ref)),
        _ => None,
    });

    // Determine which design to build
    let design_id = match params.get("design_id") {
        Some(CommandValue::Str(s)) => s.to_string(),
        _ => {
            // Auto-pick a combat design: direct-buildable, not survey, not colony
            let Some(ref registry) = br.design_registry else {
                warn!("fortify_system: ShipDesignRegistry not available");
                return;
            };
            let combat_design = registry
                .designs
                .values()
                .find(|d| d.is_direct_buildable && !d.can_survey && !d.can_colonize);
            match combat_design {
                Some(d) => d.id.clone(),
                None => {
                    // Fallback: any direct-buildable design
                    match registry.designs.values().find(|d| d.is_direct_buildable) {
                        Some(d) => d.id.clone(),
                        None => {
                            debug!("fortify_system: no buildable designs in registry");
                            return;
                        }
                    }
                }
            }
        }
    };

    queue_ship_at_shipyard(empire_entity, &design_id, target_system, sovereignty, br);
}

/// Handle `research_focus`: set the empire's active research target.
///
/// Params:
/// - `tech_id` (Str, optional): the tech to research. If absent, auto-picks
///   the first available tech whose prerequisites are met.
fn handle_research_focus(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    br: &mut BuildResearchParams,
) {
    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => {
            warn!("research_focus: no empire found for faction {:?}", issuer);
            return;
        }
    };

    let Ok((tech_tree, mut research_queue)) = br.empire_tech.get_mut(empire_entity) else {
        debug!(
            "research_focus: empire {:?} has no TechTree/ResearchQueue",
            empire_entity
        );
        return;
    };

    let tech_id = match params.get("tech_id") {
        Some(CommandValue::Str(s)) => {
            let tid = TechId(s.to_string());
            if !tech_tree.can_research(&tid) {
                debug!(
                    "research_focus: tech '{}' is not researchable for empire {:?}",
                    s, empire_entity
                );
                return;
            }
            tid
        }
        _ => {
            // Auto-pick: find the first tech that can be researched
            let available = tech_tree
                .technologies
                .keys()
                .find(|tid| tech_tree.can_research(tid))
                .cloned();
            match available {
                Some(tid) => tid,
                None => {
                    debug!(
                        "research_focus: no available techs for empire {:?}",
                        empire_entity
                    );
                    return;
                }
            }
        }
    };

    research_queue.start_research(tech_id.clone());
    info!(
        "research_focus: empire {:?} now researching '{}'",
        empire_entity, tech_id.0
    );
}

/// Handle `build_structure`: queue a building at a colony owned by the faction.
///
/// Params:
/// - `building_id` (Str): the building to construct.
/// - `target_system` (System, optional): preferred system.
/// - `colony_entity` (Entity, optional): specific colony to build at.
fn handle_build_structure(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    sovereignty: &Query<(Entity, &Sovereignty), With<StarSystem>>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    br: &mut BuildResearchParams,
) {
    let building_id_str = match params.get("building_id") {
        Some(CommandValue::Str(s)) => s.to_string(),
        _ => {
            warn!("build_structure command missing building_id param");
            return;
        }
    };

    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => {
            warn!("build_structure: no empire found for faction {:?}", issuer);
            return;
        }
    };

    let Some(ref building_registry) = br.building_registry else {
        warn!("build_structure: BuildingRegistry not available");
        return;
    };

    let Some(building_def) = building_registry.get(&building_id_str) else {
        warn!("build_structure: unknown building '{}'", building_id_str);
        return;
    };

    let minerals_cost = building_def.minerals_cost;
    let energy_cost = building_def.energy_cost;
    let build_time = building_def.build_time;
    let is_system_building = building_def.is_system_building;

    // Determine target system for ownership check
    let target_system = params.get("target_system").and_then(|v| match v {
        CommandValue::System(sys_ref) => Some(from_ai_system(*sys_ref)),
        _ => None,
    });

    // Collect owned systems
    let owned_systems: std::collections::HashSet<Entity> = sovereignty
        .iter()
        .filter(|(_, sov)| sov.owner == Some(Owner::Empire(empire_entity)))
        .map(|(e, _)| e)
        .collect();

    // System-level buildings (shipyard, port, research lab, ...) route
    // through `SystemBuildingQueue` on the StarSystem, not the per-colony
    // `BuildingQueue`. We pick the first owned system that:
    //   - hosts a Core ship (#370 gate);
    //   - has a free system-building slot;
    //   - does not already have a pending order for the same building id
    //     (protects against the per-tick emit/retry loop while metrics
    //     catch up — same-tick duplicates would otherwise stack).
    if is_system_building {
        let bid = crate::scripting::building_api::BuildingId::new(&building_id_str);
        let core_systems: std::collections::HashSet<Entity> =
            br.core_at_system.iter().map(|at| at.0).collect();
        if let Some(ref target) = target_system {
            if !owned_systems.contains(target) || !core_systems.contains(target) {
                debug!(
                    "build_structure (system): target {:?} not owned or lacks Core",
                    target
                );
                return;
            }
        }
        let mut queued = false;
        for (sys_entity, sys_bldgs, mut sbq) in br.system_builds.iter_mut() {
            if !owned_systems.contains(&sys_entity) {
                continue;
            }
            if !core_systems.contains(&sys_entity) {
                continue;
            }
            if let Some(ref target) = target_system {
                if sys_entity != *target {
                    continue;
                }
            }
            if sbq
                .queue
                .iter()
                .any(|o| o.building_id.as_str() == building_id_str)
            {
                debug!(
                    "build_structure (system): '{}' already queued at {:?}, skipping",
                    building_id_str, sys_entity
                );
                continue;
            }
            let sys_slots = crate::colony::system_buildings::slots_view(
                sys_entity,
                sys_bldgs.max_slots,
                &br.station_ships,
                building_registry,
            );
            let pending_slots: std::collections::HashSet<usize> =
                sbq.queue.iter().map(|o| o.target_slot).collect();
            let empty_slot = sys_slots
                .iter()
                .enumerate()
                .position(|(i, s)| s.is_none() && !pending_slots.contains(&i));
            let Some(slot_idx) = empty_slot else { continue };
            sbq.push_build_order(crate::colony::building_queue::BuildingOrder {
                order_id: 0,
                building_id: bid.clone(),
                target_slot: slot_idx,
                minerals_remaining: minerals_cost,
                energy_remaining: energy_cost,
                build_time_remaining: build_time,
            });
            info!(
                "build_structure (system): queued '{}' at system {:?} (slot {}) for empire {:?}",
                building_id_str, sys_entity, slot_idx, empire_entity
            );
            queued = true;
            break;
        }
        if !queued {
            debug!(
                "build_structure (system): no Core-equipped system with a free slot for '{}' (empire {:?})",
                building_id_str, empire_entity
            );
        }
        return;
    }

    // Find a colony with an empty building slot
    let bid = crate::scripting::building_api::BuildingId::new(&building_id_str);
    let mut built = false;
    for (colony_entity, colony, buildings, mut building_queue) in br.colonies.iter_mut() {
        // Check colony is in an owned system
        let colony_system = colony.system(&br.planets);
        let Some(sys) = colony_system else { continue };
        if !owned_systems.contains(&sys) {
            continue;
        }
        if let Some(ref target) = target_system {
            if sys != *target {
                continue;
            }
        }

        // Find an empty slot
        let empty_slot = buildings.slots.iter().position(|s| s.is_none());
        let Some(slot_idx) = empty_slot else { continue };

        building_queue.push_build_order(BuildingOrder {
            order_id: 0,
            building_id: bid.clone(),
            target_slot: slot_idx,
            minerals_remaining: minerals_cost,
            energy_remaining: energy_cost,
            build_time_remaining: build_time,
        });
        info!(
            "build_structure: queued '{}' at colony {:?} (slot {}) for empire {:?}",
            building_id_str, colony_entity, slot_idx, empire_entity
        );
        built = true;
        break;
    }

    if !built {
        debug!(
            "build_structure: no colony with empty slot found for building '{}' (empire {:?})",
            building_id_str, empire_entity
        );
    }
}

/// Handle `reposition`: move specified ships to a target system.
///
/// Params:
/// - `target_system` (System): destination system.
/// - `ship_count` / `ship_N` (indexed list): ships to move.
fn handle_reposition(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    move_writer: &mut MessageWriter<MoveRequested>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    dispatch_ships_to_target(
        "reposition",
        issuer,
        params,
        ships,
        empires,
        move_writer,
        next_cmd_id,
        now,
    );
}

/// Handle `blockade`: move specified ships to a target system (tactical positioning).
///
/// Params:
/// - `target_system` (System): destination system.
/// - `ship_count` / `ship_N` (indexed list): ships to move.
fn handle_blockade(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    move_writer: &mut MessageWriter<MoveRequested>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    dispatch_ships_to_target(
        "blockade",
        issuer,
        params,
        ships,
        empires,
        move_writer,
        next_cmd_id,
        now,
    );
}

/// Shared logic for reposition / blockade: dispatch listed ships to a
/// target system (same pattern as `handle_attack_target`).
fn dispatch_ships_to_target(
    cmd_name: &str,
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    move_writer: &mut MessageWriter<MoveRequested>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    let target_system = match params.get("target_system") {
        Some(CommandValue::System(sys_ref)) => from_ai_system(*sys_ref),
        _ => {
            warn!("{} command missing target_system param", cmd_name);
            return;
        }
    };

    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => {
            warn!("{}: no empire found for faction {:?}", cmd_name, issuer);
            return;
        }
    };

    let selected_ships = extract_ship_list(params);
    if selected_ships.is_empty() {
        debug!(
            "{}: no ships specified by policy for faction {:?}",
            cmd_name, issuer
        );
        return;
    }

    let mut dispatched = 0;
    for ship_entity in selected_ships {
        let Ok((_, ship, state, queue)) = ships.get(ship_entity) else {
            continue;
        };
        if ship.owner != Owner::Empire(empire_entity) {
            continue;
        }
        if !matches!(state, ShipState::InSystem { .. }) || !queue.commands.is_empty() {
            continue;
        }
        if let ShipState::InSystem { system } = state {
            if *system == target_system {
                continue;
            }
        }

        move_writer.write(MoveRequested {
            command_id: next_cmd_id.allocate(),
            ship: ship_entity,
            target: target_system,
            issued_at: now,
        });
        dispatched += 1;
    }

    if dispatched > 0 {
        info!(
            "{}: dispatched {} ships from faction {:?} to system {:?}",
            cmd_name, dispatched, issuer, target_system
        );
    }
}

/// Handle `retreat`: find ships in systems with hostiles and send them
/// back to the faction's home system (system with most colonies).
fn handle_retreat(
    issuer: &macrocosmo_ai::FactionId,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    hostiles: &Query<&AtSystem, With<Hostile>>,
    sovereignty: &Query<(Entity, &Sovereignty), With<StarSystem>>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    positions: &Query<&Position>,
    move_writer: &mut MessageWriter<MoveRequested>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => return,
    };

    // 1. Collect all systems owned by this empire.
    let owned_systems: Vec<Entity> = sovereignty
        .iter()
        .filter(|(_, sov)| sov.owner == Some(Owner::Empire(empire_entity)))
        .map(|(e, _)| e)
        .collect();

    if owned_systems.is_empty() {
        debug!("retreat: faction {:?} has no sovereign systems", issuer);
        return;
    }

    // 2. Build set of systems with hostile presence.
    let hostile_set: std::collections::HashSet<Entity> = hostiles.iter().map(|at| at.0).collect();

    // 3. Safe rally candidates = owned systems without hostiles.
    let safe_systems: Vec<Entity> = owned_systems
        .iter()
        .filter(|s| !hostile_set.contains(s))
        .copied()
        .collect();

    // 4. Fall back to any owned system if none are safe.
    let rally_candidates = if safe_systems.is_empty() {
        &owned_systems
    } else {
        &safe_systems
    };

    let mut retreated = 0;
    for (ship_entity, ship, state, queue) in ships.iter() {
        if ship.owner != Owner::Empire(empire_entity) {
            continue;
        }
        if ship.is_immobile() {
            continue;
        }
        // Skip ships already in transit (moving, FTL, etc.).
        let ShipState::InSystem { system } = state else {
            continue;
        };
        // Only retreat ships in hostile systems with empty command queues.
        if !hostile_set.contains(system) || !queue.commands.is_empty() {
            continue;
        }
        // If the ship is already at a safe rally candidate, no need to move.
        if !safe_systems.is_empty() && safe_systems.contains(system) {
            continue;
        }

        // Build per-ship candidates excluding the ship's current system.
        let filtered: Vec<Entity> = rally_candidates
            .iter()
            .filter(|s| **s != *system)
            .copied()
            .collect();
        if filtered.is_empty() {
            continue; // Only system in the empire; nowhere to go.
        }

        // Pick nearest rally point by distance.
        let target = pick_nearest_system(*system, &filtered, positions);
        move_writer.write(MoveRequested {
            command_id: next_cmd_id.allocate(),
            ship: ship_entity,
            target,
            issued_at: now,
        });
        retreated += 1;
    }

    if retreated > 0 {
        info!(
            "retreat: {} ships from faction {:?} retreating to rally points",
            retreated, issuer
        );
    }
}

/// Pick the system from `candidates` nearest to `origin`. Falls back to the
/// first candidate if positions are unavailable.
fn pick_nearest_system(
    origin: Entity,
    candidates: &[Entity],
    positions: &Query<&Position>,
) -> Entity {
    let origin_pos = positions.get(origin).ok();
    let mut best = candidates[0];
    let mut best_dist = f64::MAX;
    for &candidate in candidates {
        let dist = match (origin_pos, positions.get(candidate).ok()) {
            (Some(a), Some(b)) => distance_ly(a, b),
            _ => f64::MAX,
        };
        if dist < best_dist {
            best_dist = dist;
            best = candidate;
        }
    }
    best
}

/// Handle `move_ruler`: find an idle ship at the Ruler's current system,
/// queue the boarding in `PendingRulerBoarding`, and emit `MoveRequested`
/// for the chosen ship.
fn handle_move_ruler(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    empire_rulers: &Query<&EmpireRuler, With<Empire>>,
    ruler_q: &Query<(&StationedAt, Option<&AboardShip>), With<Ruler>>,
    pending_boarding: &mut PendingRulerBoarding,
    move_writer: &mut MessageWriter<MoveRequested>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    let target_system = match params.get("target_system") {
        Some(CommandValue::System(sys_ref)) => from_ai_system(*sys_ref),
        _ => {
            warn!("move_ruler command missing target_system param");
            return;
        }
    };

    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => {
            warn!("move_ruler: no empire found for faction {:?}", issuer);
            return;
        }
    };

    let Ok(empire_ruler) = empire_rulers.get(empire_entity) else {
        debug!("move_ruler: empire {:?} has no EmpireRuler", empire_entity);
        return;
    };
    let ruler_entity = empire_ruler.0;

    let Ok((stationed, aboard)) = ruler_q.get(ruler_entity) else {
        debug!("move_ruler: ruler {:?} has no StationedAt", ruler_entity);
        return;
    };

    if aboard.is_some() {
        debug!("move_ruler: ruler is already aboard a ship");
        return;
    }

    let ruler_system = stationed.system;

    if ruler_system == target_system {
        debug!("move_ruler: ruler is already at target system");
        return;
    }

    let transport_ship = ships
        .iter()
        .find(|(_, ship, state, queue)| {
            ship.owner == Owner::Empire(empire_entity)
                && !ship.is_immobile()
                && matches!(state, ShipState::InSystem { system } if *system == ruler_system)
                && queue.commands.is_empty()
                && !ship.ruler_aboard
        })
        .map(|(e, _, _, _)| e);

    let Some(ship_entity) = transport_ship else {
        debug!(
            "move_ruler: no idle ship at ruler's system {:?} for empire {:?}",
            ruler_system, empire_entity
        );
        return;
    };

    pending_boarding
        .requests
        .push((ruler_entity, ship_entity, target_system));

    move_writer.write(MoveRequested {
        command_id: next_cmd_id.allocate(),
        ship: ship_entity,
        target: target_system,
        issued_at: now,
    });

    info!(
        "move_ruler: boarding ruler {:?} onto ship {:?}, moving to system {:?} for faction {:?}",
        ruler_entity, ship_entity, target_system, issuer
    );
}

/// Process pending ruler boarding requests. Inserts `AboardShip` on the
/// ruler and sets `ruler_aboard = true` on the ship.
pub fn process_ruler_boarding(
    mut commands: Commands,
    mut pending: ResMut<PendingRulerBoarding>,
    mut ships: Query<&mut Ship>,
) {
    for (ruler_entity, ship_entity, _target) in pending.requests.drain(..) {
        commands
            .entity(ruler_entity)
            .insert(AboardShip { ship: ship_entity });
        if let Ok(mut ship) = ships.get_mut(ship_entity) {
            ship.ruler_aboard = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::plugin::AiBusResource;
    use crate::ai::schema;
    use crate::components::Position;
    use crate::time_system::{GameClock, GameSpeed};
    use macrocosmo_ai::{Command, WarningMode};

    #[derive(Resource, Reflect)]
    #[reflect(Resource)]
    struct MoveCount(usize);

    fn count_moves(mut reader: MessageReader<MoveRequested>, mut count: ResMut<MoveCount>) {
        for _msg in reader.read() {
            count.0 += 1;
        }
    }

    /// Minimal app with AI bus and clock for command consumer tests.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GameClock::new(10));
        app.insert_resource(GameSpeed::default());
        app.init_resource::<NextCommandId>();
        app.insert_resource(AiBusResource::with_warning_mode(WarningMode::Silent));
        app.init_resource::<PendingRulerBoarding>();
        app.add_message::<MoveRequested>();
        app.add_systems(Startup, schema::declare_all);
        app.update();
        app
    }

    #[test]
    fn attack_target_dispatches_idle_ships() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let faction_id = to_ai_faction(empire_entity);

        let origin_sys = world
            .spawn((
                StarSystem {
                    name: "Origin".into(),
                    is_capital: false,
                    surveyed: false,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
            ))
            .id();

        let target_sys = world
            .spawn((
                StarSystem {
                    name: "Target".into(),
                    is_capital: false,
                    surveyed: false,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([10.0, 0.0, 0.0]),
            ))
            .id();

        let ship_entity = world
            .spawn((
                Ship {
                    name: "NPC Scout".into(),
                    design_id: "scout".into(),
                    hull_id: "corvette".into(),
                    modules: vec![],
                    owner: Owner::Empire(empire_entity),
                    sublight_speed: 0.1,
                    ftl_range: 5.0,
                    ruler_aboard: false,
                    home_port: origin_sys,
                    design_revision: 0,
                    fleet: None,
                },
                ShipState::InSystem { system: origin_sys },
                CommandQueue::default(),
            ))
            .id();

        let target_ref = crate::ai::convert::to_ai_system(target_sys);
        let ship_ref = crate::ai::convert::to_ai_entity(ship_entity);
        let cmd = Command::new(cmd_ids::attack_target(), faction_id, 10)
            .with_param("target_system", CommandValue::System(target_ref))
            .with_param("ship_count", CommandValue::I64(1))
            .with_param("ship_0", CommandValue::Entity(ship_ref));
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        // Add a counting system that reads MoveRequested messages.
        app.insert_resource(MoveCount(0));
        app.add_systems(Update, (drain_ai_commands, count_moves).chain());
        app.update();

        let count = app.world().resource::<MoveCount>().0;
        assert_eq!(count, 1, "should emit 1 MoveRequested");
    }

    #[test]
    fn attack_target_skips_ships_not_owned_by_faction() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire_a = world
            .spawn((
                Empire {
                    name: "Empire A".into(),
                },
                Faction::new("empire_a", "Empire A"),
            ))
            .id();

        let empire_b = world
            .spawn((
                Empire {
                    name: "Empire B".into(),
                },
                Faction::new("empire_b", "Empire B"),
            ))
            .id();

        let faction_a = to_ai_faction(empire_a);

        let origin = world
            .spawn((
                StarSystem {
                    name: "Origin".into(),
                    is_capital: false,
                    surveyed: false,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
            ))
            .id();

        let target = world
            .spawn((
                StarSystem {
                    name: "Target".into(),
                    is_capital: false,
                    surveyed: false,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([10.0, 0.0, 0.0]),
            ))
            .id();

        // Ship owned by empire_b
        world.spawn((
            Ship {
                name: "B's Ship".into(),
                design_id: "scout".into(),
                hull_id: "corvette".into(),
                modules: vec![],
                owner: Owner::Empire(empire_b),
                sublight_speed: 0.1,
                ftl_range: 5.0,
                ruler_aboard: false,
                home_port: origin,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system: origin },
            CommandQueue::default(),
        ));

        let target_ref = crate::ai::convert::to_ai_system(target);
        let cmd = Command::new(cmd_ids::attack_target(), faction_a, 10)
            .with_param("target_system", CommandValue::System(target_ref));
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();

        app.insert_resource(MoveCount(0));
        app.add_systems(Update, (drain_ai_commands, count_moves).chain());
        app.update();

        let count = app.world().resource::<MoveCount>().0;
        assert_eq!(
            count, 0,
            "empire_b's ship should not be dispatched by empire_a's command"
        );
    }

    #[test]
    fn attack_target_no_crash_with_missing_params() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire = world
            .spawn((
                Empire {
                    name: "Test".into(),
                },
                Faction::new("test", "Test"),
            ))
            .id();

        let faction_id = to_ai_faction(empire);

        let cmd = Command::new(cmd_ids::attack_target(), faction_id, 10);
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();
    }

    /// Helper: create a minimal ShipDesignRegistry with a combat design.
    fn test_design_registry() -> ShipDesignRegistry {
        use crate::ship_design::ShipDesignDefinition;
        let mut registry = ShipDesignRegistry::default();
        registry.insert(ShipDesignDefinition {
            id: "corvette_mk1".into(),
            name: "Corvette Mk1".into(),
            description: String::new(),
            hull_id: "corvette".into(),
            modules: vec![],
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::new(0, 500),
            build_cost_minerals: Amt::units(100),
            build_cost_energy: Amt::units(50),
            build_time: 30,
            hp: 100.0,
            sublight_speed: 0.1,
            ftl_range: 5.0,
            revision: 0,
            is_direct_buildable: true,
        });
        registry
    }

    /// Helper: create a BuildingRegistry with a test mine building.
    fn test_building_registry() -> BuildingRegistry {
        use crate::scripting::building_api::{BuildingDefinition, CapabilityParams};
        use std::collections::HashMap;
        let mut registry = BuildingRegistry::default();
        registry.insert(BuildingDefinition {
            id: "mine".into(),
            name: "Mine".into(),
            description: String::new(),
            minerals_cost: Amt::units(50),
            energy_cost: Amt::units(10),
            build_time: 15,
            maintenance: Amt::ZERO,
            production_bonus_minerals: Amt::units(5),
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers: Vec::new(),
            is_system_building: false,
            capabilities: HashMap::new(),
            upgrade_to: Vec::new(),
            is_direct_buildable: true,
            prerequisites: None,
            on_built: None,
            on_upgraded: None,
            dismantlable: true,
            ship_design_id: None,
            colony_slots: None,
        });
        registry
    }

    #[test]
    fn build_ship_queues_order_at_shipyard_system() {
        use crate::scripting::building_api::{BuildingDefinition, BuildingId, CapabilityParams};
        use std::collections::HashMap;

        let mut app = test_app();
        // Insert design + building registries
        app.insert_resource(test_design_registry());

        // Building registry with shipyard modifier
        let mut breg = BuildingRegistry::default();
        breg.insert(BuildingDefinition {
            id: "shipyard".into(),
            name: "Shipyard".into(),
            description: String::new(),
            minerals_cost: Amt::ZERO,
            energy_cost: Amt::ZERO,
            build_time: 30,
            maintenance: Amt::ZERO,
            production_bonus_minerals: Amt::ZERO,
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers: vec![crate::modifier::ParsedModifier {
                target: "system.shipyard_capacity".into(),
                base_add: 1.0,
                multiplier: 0.0,
                add: 0.0,
            }],
            is_system_building: true,
            capabilities: HashMap::new(),
            upgrade_to: Vec::new(),
            is_direct_buildable: true,
            prerequisites: None,
            on_built: None,
            on_upgraded: None,
            dismantlable: true,
            ship_design_id: Some("station_shipyard_v1".into()),
            colony_slots: None,
        });
        app.insert_resource(breg);

        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let faction_id = to_ai_faction(empire_entity);

        // SystemModifiers with shipyard_capacity seeded so has_shipyard check passes.
        let mut sys_mods = crate::galaxy::SystemModifiers::default();
        sys_mods
            .shipyard_capacity
            .push_modifier(crate::modifier::Modifier {
                id: "test_shipyard".into(),
                label: "Test Shipyard".into(),
                base_add: crate::amount::SignedAmt::units(1),
                multiplier: crate::amount::SignedAmt::ZERO,
                add: crate::amount::SignedAmt::ZERO,
                expires_at: None,
                on_expire_event: None,
            });

        let sys_entity = world
            .spawn((
                StarSystem {
                    name: "Home".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
                Sovereignty {
                    owner: Some(Owner::Empire(empire_entity)),
                    control_score: 1.0,
                },
                BuildQueue::default(),
                sys_mods,
            ))
            .id();

        // Spawn a station ship with shipyard design at that system
        world.spawn((
            Ship {
                name: "Shipyard Station".into(),
                design_id: "station_shipyard_v1".into(),
                hull_id: "station".into(),
                modules: vec![],
                owner: Owner::Empire(empire_entity),
                sublight_speed: 0.0,
                ftl_range: 0.0,
                ruler_aboard: false,
                home_port: sys_entity,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system: sys_entity },
            SlotAssignment(0),
            CommandQueue::default(),
        ));

        // Emit build_ship command
        let cmd = Command::new(cmd_ids::build_ship(), faction_id, 10)
            .with_param("design_id", CommandValue::Str("corvette_mk1".into()));
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();

        // Check that a build order was added
        let queue = app.world().get::<BuildQueue>(sys_entity).unwrap();
        assert_eq!(queue.queue.len(), 1, "should have 1 build order");
        assert_eq!(queue.queue[0].design_id, "corvette_mk1");
    }

    #[test]
    fn research_focus_sets_active_research() {
        use crate::technology::{TechCost, Technology};

        let mut app = test_app();
        let world = app.world_mut();

        let tech_tree = TechTree::from_vec(vec![Technology {
            id: TechId("test_tech".into()),
            name: "Test Tech".into(),
            description: String::new(),
            branch: "test".into(),
            cost: TechCost {
                research: Amt::units(100),
                minerals: Amt::ZERO,
                energy: Amt::ZERO,
            },
            prerequisites: vec![],
            dangerous: false,
        }]);

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
                tech_tree,
                ResearchQueue::default(),
            ))
            .id();

        let faction_id = to_ai_faction(empire_entity);

        let cmd = Command::new(cmd_ids::research_focus(), faction_id, 10)
            .with_param("tech_id", CommandValue::Str("test_tech".into()));
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();

        let rq = app.world().get::<ResearchQueue>(empire_entity).unwrap();
        assert_eq!(
            rq.current,
            Some(TechId("test_tech".into())),
            "should be researching test_tech"
        );
    }

    #[test]
    fn research_focus_auto_picks_available_tech() {
        use crate::technology::{TechCost, Technology};

        let mut app = test_app();
        let world = app.world_mut();

        let tech_tree = TechTree::from_vec(vec![Technology {
            id: TechId("auto_pick_tech".into()),
            name: "Auto Pick".into(),
            description: String::new(),
            branch: "test".into(),
            cost: TechCost {
                research: Amt::units(100),
                minerals: Amt::ZERO,
                energy: Amt::ZERO,
            },
            prerequisites: vec![],
            dangerous: false,
        }]);

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
                tech_tree,
                ResearchQueue::default(),
            ))
            .id();

        let faction_id = to_ai_faction(empire_entity);

        // No tech_id param — should auto-pick
        let cmd = Command::new(cmd_ids::research_focus(), faction_id, 10);
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();

        let rq = app.world().get::<ResearchQueue>(empire_entity).unwrap();
        assert!(rq.current.is_some(), "should have auto-picked a tech");
    }

    #[test]
    fn build_structure_queues_building_at_colony() {
        let mut app = test_app();
        app.insert_resource(test_building_registry());

        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let faction_id = to_ai_faction(empire_entity);

        let sys_entity = world
            .spawn((
                StarSystem {
                    name: "Home".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
                Sovereignty {
                    owner: Some(Owner::Empire(empire_entity)),
                    control_score: 1.0,
                },
            ))
            .id();

        let planet_entity = world
            .spawn(Planet {
                name: "Test Planet".into(),
                planet_type: "terran".into(),
                system: sys_entity,
            })
            .id();

        world.spawn((
            Colony {
                planet: planet_entity,
                growth_rate: 0.01,
            },
            Buildings {
                slots: vec![None, None, None],
            },
            BuildingQueue::default(),
        ));

        let cmd = Command::new(cmd_ids::build_structure(), faction_id, 10)
            .with_param("building_id", CommandValue::Str("mine".into()));
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();

        // Verify a building order was queued
        let mut found = false;
        for (_, _, _, bq) in app
            .world_mut()
            .query::<(Entity, &Colony, &Buildings, &BuildingQueue)>()
            .iter(app.world())
        {
            if !bq.queue.is_empty() {
                assert_eq!(bq.queue[0].building_id.as_str(), "mine");
                assert_eq!(bq.queue[0].target_slot, 0);
                found = true;
            }
        }
        assert!(found, "should have queued a building order");
    }

    #[test]
    fn reposition_dispatches_ships() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let faction_id = to_ai_faction(empire_entity);

        let origin = world
            .spawn((
                StarSystem {
                    name: "Origin".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
            ))
            .id();

        let target = world
            .spawn((
                StarSystem {
                    name: "Target".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([10.0, 0.0, 0.0]),
            ))
            .id();

        let ship_entity = world
            .spawn((
                Ship {
                    name: "NPC Ship".into(),
                    design_id: "corvette".into(),
                    hull_id: "corvette".into(),
                    modules: vec![],
                    owner: Owner::Empire(empire_entity),
                    sublight_speed: 0.1,
                    ftl_range: 5.0,
                    ruler_aboard: false,
                    home_port: origin,
                    design_revision: 0,
                    fleet: None,
                },
                ShipState::InSystem { system: origin },
                CommandQueue::default(),
            ))
            .id();

        let target_ref = crate::ai::convert::to_ai_system(target);
        let ship_ref = crate::ai::convert::to_ai_entity(ship_entity);
        let cmd = Command::new(cmd_ids::reposition(), faction_id, 10)
            .with_param("target_system", CommandValue::System(target_ref))
            .with_param("ship_count", CommandValue::I64(1))
            .with_param("ship_0", CommandValue::Entity(ship_ref));
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.insert_resource(MoveCount(0));
        app.add_systems(Update, (drain_ai_commands, count_moves).chain());
        app.update();

        let count = app.world().resource::<MoveCount>().0;
        assert_eq!(count, 1, "reposition should emit 1 MoveRequested");
    }

    #[test]
    fn blockade_dispatches_ships() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let faction_id = to_ai_faction(empire_entity);

        let origin = world
            .spawn((
                StarSystem {
                    name: "Origin".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
            ))
            .id();

        let target = world
            .spawn((
                StarSystem {
                    name: "Target".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([10.0, 0.0, 0.0]),
            ))
            .id();

        let ship_entity = world
            .spawn((
                Ship {
                    name: "NPC Ship".into(),
                    design_id: "corvette".into(),
                    hull_id: "corvette".into(),
                    modules: vec![],
                    owner: Owner::Empire(empire_entity),
                    sublight_speed: 0.1,
                    ftl_range: 5.0,
                    ruler_aboard: false,
                    home_port: origin,
                    design_revision: 0,
                    fleet: None,
                },
                ShipState::InSystem { system: origin },
                CommandQueue::default(),
            ))
            .id();

        let target_ref = crate::ai::convert::to_ai_system(target);
        let ship_ref = crate::ai::convert::to_ai_entity(ship_entity);
        let cmd = Command::new(cmd_ids::blockade(), faction_id, 10)
            .with_param("target_system", CommandValue::System(target_ref))
            .with_param("ship_count", CommandValue::I64(1))
            .with_param("ship_0", CommandValue::Entity(ship_ref));
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.insert_resource(MoveCount(0));
        app.add_systems(Update, (drain_ai_commands, count_moves).chain());
        app.update();

        let count = app.world().resource::<MoveCount>().0;
        assert_eq!(count, 1, "blockade should emit 1 MoveRequested");
    }

    #[test]
    fn fortify_system_auto_picks_combat_design() {
        use crate::scripting::building_api::{BuildingDefinition, CapabilityParams};
        use std::collections::HashMap;

        let mut app = test_app();
        app.insert_resource(test_design_registry());

        // Building registry with shipyard modifier
        let mut breg = BuildingRegistry::default();
        breg.insert(BuildingDefinition {
            id: "shipyard".into(),
            name: "Shipyard".into(),
            description: String::new(),
            minerals_cost: Amt::ZERO,
            energy_cost: Amt::ZERO,
            build_time: 30,
            maintenance: Amt::ZERO,
            production_bonus_minerals: Amt::ZERO,
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers: vec![crate::modifier::ParsedModifier {
                target: "system.shipyard_capacity".into(),
                base_add: 1.0,
                multiplier: 0.0,
                add: 0.0,
            }],
            is_system_building: true,
            capabilities: HashMap::new(),
            upgrade_to: Vec::new(),
            is_direct_buildable: true,
            prerequisites: None,
            on_built: None,
            on_upgraded: None,
            dismantlable: true,
            ship_design_id: Some("station_shipyard_v1".into()),
            colony_slots: None,
        });
        app.insert_resource(breg);

        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let faction_id = to_ai_faction(empire_entity);

        let mut sys_mods = crate::galaxy::SystemModifiers::default();
        sys_mods
            .shipyard_capacity
            .push_modifier(crate::modifier::Modifier {
                id: "test_shipyard".into(),
                label: "Test Shipyard".into(),
                base_add: crate::amount::SignedAmt::units(1),
                multiplier: crate::amount::SignedAmt::ZERO,
                add: crate::amount::SignedAmt::ZERO,
                expires_at: None,
                on_expire_event: None,
            });

        let sys_entity = world
            .spawn((
                StarSystem {
                    name: "Home".into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
                Sovereignty {
                    owner: Some(Owner::Empire(empire_entity)),
                    control_score: 1.0,
                },
                BuildQueue::default(),
                sys_mods,
            ))
            .id();

        // Shipyard station ship
        world.spawn((
            Ship {
                name: "Shipyard Station".into(),
                design_id: "station_shipyard_v1".into(),
                hull_id: "station".into(),
                modules: vec![],
                owner: Owner::Empire(empire_entity),
                sublight_speed: 0.0,
                ftl_range: 0.0,
                ruler_aboard: false,
                home_port: sys_entity,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system: sys_entity },
            SlotAssignment(0),
            CommandQueue::default(),
        ));

        // No design_id param — should auto-pick combat design
        let cmd = Command::new(cmd_ids::fortify_system(), faction_id, 10);
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();

        let queue = app.world().get::<BuildQueue>(sys_entity).unwrap();
        assert_eq!(queue.queue.len(), 1, "fortify should queue 1 ship");
        assert_eq!(queue.queue[0].design_id, "corvette_mk1");
    }

    // ── retreat tests ────────────────────────────────────────────────

    /// Collect (ship, target) pairs from MoveRequested messages.
    #[derive(Resource, Default, Reflect)]
    #[reflect(Resource)]
    struct MoveTargets(Vec<(Entity, Entity)>);

    fn collect_move_targets(
        mut reader: MessageReader<MoveRequested>,
        mut targets: ResMut<MoveTargets>,
    ) {
        for msg in reader.read() {
            targets.0.push((msg.ship, msg.target));
        }
    }

    /// Helper: spawn a ship owned by `empire` at `system`.
    fn spawn_ship_at(world: &mut World, empire: Entity, system: Entity, name: &str) -> Entity {
        world
            .spawn((
                Ship {
                    name: name.into(),
                    design_id: "scout".into(),
                    hull_id: "corvette".into(),
                    modules: vec![],
                    owner: Owner::Empire(empire),
                    sublight_speed: 0.1,
                    ftl_range: 5.0,
                    ruler_aboard: false,
                    home_port: system,
                    design_revision: 0,
                    fleet: None,
                },
                ShipState::InSystem { system },
                CommandQueue::default(),
            ))
            .id()
    }

    /// Helper: spawn a star system with position and sovereignty.
    fn spawn_system_with_sov(
        world: &mut World,
        name: &str,
        pos: [f64; 3],
        owner: Option<Entity>,
    ) -> Entity {
        let owner = owner.map(|e| Owner::Empire(e));
        world
            .spawn((
                StarSystem {
                    name: name.into(),
                    is_capital: false,
                    surveyed: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from(pos),
                Sovereignty {
                    owner,
                    control_score: 1.0,
                },
            ))
            .id()
    }

    #[test]
    fn retreat_picks_nearest_safe_system() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire = world
            .spawn((Empire { name: "E".into() }, Faction::new("e", "E")))
            .id();

        // Hostile system where ship is located (at origin).
        let hostile_sys = spawn_system_with_sov(world, "Hostile", [0.0, 0.0, 0.0], Some(empire));
        // Near safe system at distance 5.
        let near_safe = spawn_system_with_sov(world, "NearSafe", [3.0, 4.0, 0.0], Some(empire));
        // Far safe system at distance 13.
        let _far_safe = spawn_system_with_sov(world, "FarSafe", [12.0, 5.0, 0.0], Some(empire));

        // Mark hostile_sys as having hostile presence.
        world.spawn((Hostile, AtSystem(hostile_sys)));

        let ship = spawn_ship_at(world, empire, hostile_sys, "Scout");

        // Issue retreat command.
        let faction_id = to_ai_faction(empire);
        let cmd = Command::new(cmd_ids::retreat(), faction_id, 10);
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.init_resource::<MoveTargets>();
        app.add_systems(Update, (drain_ai_commands, collect_move_targets).chain());
        app.update();

        let targets = app.world().resource::<MoveTargets>();
        assert_eq!(targets.0.len(), 1, "should retreat 1 ship");
        assert_eq!(
            targets.0[0],
            (ship, near_safe),
            "should pick nearest safe system"
        );
    }

    #[test]
    fn retreat_distributes_ships_to_nearest_rally() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire = world
            .spawn((Empire { name: "E".into() }, Faction::new("e", "E")))
            .id();

        // Two hostile systems at different locations.
        let hostile_a = spawn_system_with_sov(world, "HostileA", [0.0, 0.0, 0.0], Some(empire));
        let hostile_b = spawn_system_with_sov(world, "HostileB", [20.0, 0.0, 0.0], Some(empire));

        // Two safe systems: one near hostile_a, one near hostile_b.
        let safe_left = spawn_system_with_sov(world, "SafeLeft", [-5.0, 0.0, 0.0], Some(empire));
        let safe_right = spawn_system_with_sov(world, "SafeRight", [25.0, 0.0, 0.0], Some(empire));

        world.spawn((Hostile, AtSystem(hostile_a)));
        world.spawn((Hostile, AtSystem(hostile_b)));

        let ship_a = spawn_ship_at(world, empire, hostile_a, "ShipA");
        let ship_b = spawn_ship_at(world, empire, hostile_b, "ShipB");

        let faction_id = to_ai_faction(empire);
        let cmd = Command::new(cmd_ids::retreat(), faction_id, 10);
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.init_resource::<MoveTargets>();
        app.add_systems(Update, (drain_ai_commands, collect_move_targets).chain());
        app.update();

        let targets = app.world().resource::<MoveTargets>();
        assert_eq!(targets.0.len(), 2, "should retreat 2 ships");

        // Each ship goes to its nearest safe system.
        let ship_a_target = targets
            .0
            .iter()
            .find(|(s, _)| *s == ship_a)
            .map(|(_, t)| *t);
        let ship_b_target = targets
            .0
            .iter()
            .find(|(s, _)| *s == ship_b)
            .map(|(_, t)| *t);
        assert_eq!(
            ship_a_target,
            Some(safe_left),
            "ship_a should go to safe_left"
        );
        assert_eq!(
            ship_b_target,
            Some(safe_right),
            "ship_b should go to safe_right"
        );
    }

    #[test]
    fn retreat_skips_ships_already_in_transit() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire = world
            .spawn((Empire { name: "E".into() }, Faction::new("e", "E")))
            .id();

        let hostile_sys = spawn_system_with_sov(world, "Hostile", [0.0, 0.0, 0.0], Some(empire));
        let _safe_sys = spawn_system_with_sov(world, "Safe", [5.0, 0.0, 0.0], Some(empire));

        world.spawn((Hostile, AtSystem(hostile_sys)));

        // Ship already in FTL transit — should not be re-commanded.
        world.spawn((
            Ship {
                name: "In Transit".into(),
                design_id: "scout".into(),
                hull_id: "corvette".into(),
                modules: vec![],
                owner: Owner::Empire(empire),
                sublight_speed: 0.1,
                ftl_range: 5.0,
                ruler_aboard: false,
                home_port: hostile_sys,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InFTL {
                origin_system: hostile_sys,
                destination_system: _safe_sys,
                departed_at: 5,
                arrival_at: 15,
            },
            CommandQueue::default(),
        ));

        let faction_id = to_ai_faction(empire);
        let cmd = Command::new(cmd_ids::retreat(), faction_id, 10);
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.init_resource::<MoveTargets>();
        app.add_systems(Update, (drain_ai_commands, collect_move_targets).chain());
        app.update();

        let targets = app.world().resource::<MoveTargets>();
        assert_eq!(
            targets.0.len(),
            0,
            "should not retreat ships already in transit"
        );
    }

    #[test]
    fn retreat_falls_back_to_hostile_owned_system() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire = world
            .spawn((Empire { name: "E".into() }, Faction::new("e", "E")))
            .id();

        // All owned systems have hostiles.
        let hostile_a = spawn_system_with_sov(world, "HostileA", [0.0, 0.0, 0.0], Some(empire));
        let hostile_b = spawn_system_with_sov(world, "HostileB", [10.0, 0.0, 0.0], Some(empire));

        world.spawn((Hostile, AtSystem(hostile_a)));
        world.spawn((Hostile, AtSystem(hostile_b)));

        let ship = spawn_ship_at(world, empire, hostile_a, "Scout");

        let faction_id = to_ai_faction(empire);
        let cmd = Command::new(cmd_ids::retreat(), faction_id, 10);
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.init_resource::<MoveTargets>();
        app.add_systems(Update, (drain_ai_commands, collect_move_targets).chain());
        app.update();

        let targets = app.world().resource::<MoveTargets>();
        // Should still retreat — falls back to the nearest owned system (hostile_b,
        // since ship is already at hostile_a).
        assert_eq!(
            targets.0.len(),
            1,
            "should retreat even when all systems are hostile"
        );
        assert_eq!(
            targets.0[0],
            (ship, hostile_b),
            "should pick nearest owned system as fallback"
        );
    }
}
