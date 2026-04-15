use bevy::prelude::*;

use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::ship_design::ShipDesignRegistry;
use crate::time_system::GameClock;

use super::movement::{PortParams, start_ftl_travel_full, start_sublight_travel_with_bonus};
use super::routing;
use super::survey::start_survey_with_bonus;
use super::{
    CommandQueue, PendingShipCommand, QueuedCommand, RulesOfEngagement, Ship, ShipCommand,
    ShipState,
};

// --- Pending ship command processing (#33) ---

/// Processes pending ship commands that have arrived after communication delay.
/// #45: Uses GlobalParams for tech bonuses
/// #46: Checks for port facility at origin system
pub fn process_pending_ship_commands(
    mut commands: Commands,
    clock: Res<GameClock>,
    empire_params_q: Query<&crate::technology::GlobalParams, With<crate::player::PlayerEmpire>>,
    balance: Res<crate::technology::GameBalance>,
    pending: Query<(Entity, &PendingShipCommand)>,
    mut ships: Query<(&mut Ship, &mut ShipState, &Position)>,
    mut command_queues: Query<&mut CommandQueue>,
    systems: Query<(&StarSystem, &Position), Without<Ship>>,
    system_buildings: Query<&crate::colony::SystemBuildings>,
    _planets: Query<&crate::galaxy::Planet>,
    design_registry: Res<ShipDesignRegistry>,
    building_registry: Res<crate::colony::BuildingRegistry>,
) {
    let Ok(global_params) = empire_params_q.single() else {
        return;
    };
    let base_ftl_speed = balance.initial_ftl_speed_c();
    let settling_duration = balance.settling_duration();
    let survey_range = balance.survey_range_ly();
    let survey_duration = balance.survey_duration();
    for (cmd_entity, pending_cmd) in &pending {
        if clock.elapsed < pending_cmd.arrives_at {
            continue;
        }

        let Ok((ship, mut state, ship_pos)) = ships.get_mut(pending_cmd.ship) else {
            commands.entity(cmd_entity).despawn();
            continue;
        };

        // EnqueueCommand works regardless of ship state — it just adds to the queue
        if let ShipCommand::EnqueueCommand(queued_cmd) = &pending_cmd.command {
            if let Ok(mut queue) = command_queues.get_mut(pending_cmd.ship) {
                info!(
                    "Delayed queue command arrived for {}: {:?}",
                    ship.name, queued_cmd,
                );
                let sys_query = &systems;
                queue.push(queued_cmd.clone(), &|sys| {
                    sys_query.get(sys).ok().map(|(_, pos)| pos.as_array())
                });
            }
            commands.entity(cmd_entity).despawn();
            continue;
        }

        let docked_system = match *state {
            ShipState::Docked { system } => system,
            _ => {
                info!(
                    "Remote command for {} discarded: ship is no longer docked",
                    ship.name,
                );
                commands.entity(cmd_entity).despawn();
                continue;
            }
        };

        match &pending_cmd.command {
            ShipCommand::MoveTo { destination } => {
                let dest = *destination;
                let Ok((dest_star, dest_pos)) = systems.get(dest) else {
                    commands.entity(cmd_entity).despawn();
                    continue;
                };
                let Ok((_, origin_pos)) = systems.get(docked_system) else {
                    commands.entity(cmd_entity).despawn();
                    continue;
                };
                // Try FTL first, fall back to sublight
                let port_params = system_buildings
                    .get(docked_system)
                    .map(|sb| PortParams::from_system_buildings(sb, &building_registry))
                    .unwrap_or(PortParams::NONE);
                match start_ftl_travel_full(
                    &mut state,
                    &ship,
                    docked_system,
                    dest,
                    origin_pos,
                    dest_pos,
                    clock.elapsed,
                    global_params.ftl_range_bonus,
                    global_params.ftl_speed_multiplier,
                    port_params,
                    base_ftl_speed,
                ) {
                    Ok(()) => {
                        info!(
                            "Remote move command executed: {} FTL jumping to {}",
                            ship.name, dest_star.name,
                        );
                    }
                    Err(_) => {
                        // Fall back to sublight. #296: silently drop immobile
                        // ships — the remote MoveTo cannot be fulfilled.
                        match start_sublight_travel_with_bonus(
                            &mut state,
                            ship_pos,
                            &ship,
                            *dest_pos,
                            Some(dest),
                            clock.elapsed,
                            global_params.sublight_speed_bonus,
                        ) {
                            Ok(()) => {
                                info!(
                                    "Remote move command executed: {} sub-light to {}",
                                    ship.name, dest_star.name,
                                );
                            }
                            Err(e) => {
                                info!("Remote move command rejected for {}: {}", ship.name, e,);
                            }
                        }
                    }
                }
            }
            ShipCommand::Survey { target } => {
                let tgt = *target;
                let Ok((tgt_star, tgt_pos)) = systems.get(tgt) else {
                    commands.entity(cmd_entity).despawn();
                    continue;
                };
                match start_survey_with_bonus(
                    &mut state,
                    &ship,
                    tgt,
                    ship_pos,
                    tgt_pos,
                    clock.elapsed,
                    global_params.survey_range_bonus,
                    &design_registry,
                    survey_range,
                    survey_duration,
                ) {
                    Ok(()) => {
                        info!(
                            "Remote survey command executed: {} surveying {}",
                            ship.name, tgt_star.name,
                        );
                    }
                    Err(e) => {
                        info!("Remote survey command for {} failed: {}", ship.name, e,);
                    }
                }
            }
            ShipCommand::Colonize => {
                if !design_registry.can_colonize(&ship.design_id) {
                    info!(
                        "Remote colonize command for {} failed: not a colony ship",
                        ship.name,
                    );
                } else {
                    *state = ShipState::Settling {
                        system: docked_system,
                        planet: None,
                        started_at: clock.elapsed,
                        completes_at: clock.elapsed + settling_duration,
                    };
                    info!(
                        "Remote colonize command executed: {} settling at docked system",
                        ship.name,
                    );
                }
            }
            ShipCommand::SetROE { roe } => {
                let roe_val = *roe;
                info!(
                    "Remote ROE command executed: {} set to {:?}",
                    ship.name, roe_val,
                );
                // Use try_insert: ship may have been despawned by combat
                commands.entity(pending_cmd.ship).try_insert(roe_val);
            }
            ShipCommand::EnqueueCommand(_) => unreachable!("handled above"),
        }

        commands.entity(cmd_entity).despawn();
    }
}

// --- Command queue processing (#34) ---

/// #45: Uses GlobalParams for tech bonuses
/// #46: Checks for port facility at origin system
/// #108: Unified MoveTo with auto-route planning (FTL chain > FTL direct > SubLight)
/// #128: Async A* mixed route planning (FTL/sublight)
#[allow(clippy::too_many_arguments)]
pub fn process_command_queue(
    // #334 Phase 1: `Commands` / KnowledgeStore / FactionRelations /
    // port-params / FTL blocker params are preserved at the SystemParam
    // surface for Phase 2/3 migrations (Survey / Colonize / Scout handlers
    // continue to need some of them) even though this tick the MoveTo
    // path no longer consults them. Underscored locals silence the
    // "unused" warnings without touching the signature.
    mut _commands: Commands,
    clock: Res<GameClock>,
    empire_params_q: Query<&crate::technology::GlobalParams, With<crate::player::PlayerEmpire>>,
    balance: Res<crate::technology::GameBalance>,
    empire_knowledge_q: Query<&crate::knowledge::KnowledgeStore, With<crate::player::PlayerEmpire>>,
    mut ships: Query<
        (
            Entity,
            &Ship,
            &mut ShipState,
            &mut CommandQueue,
            &Position,
            Option<&RulesOfEngagement>,
        ),
        Without<routing::PendingRoute>,
    >,
    systems: Query<(Entity, &StarSystem, &Position), Without<Ship>>,
    _system_buildings: Query<&crate::colony::SystemBuildings>,
    _planets: Query<&crate::galaxy::Planet>,
    hostiles_q: Query<
        (&crate::galaxy::AtSystem, &crate::faction::FactionOwner),
        With<crate::galaxy::Hostile>,
    >,
    _relations: Res<crate::faction::FactionRelations>,
    mut _pending_count: ResMut<routing::RouteCalculationsPending>,
    design_registry: Res<ShipDesignRegistry>,
    _building_registry: Res<crate::colony::BuildingRegistry>,
    regions: Query<&crate::galaxy::ForbiddenRegion>,
) {
    let Ok(_global_params) = empire_params_q.single() else {
        return;
    };
    let _base_ftl_speed = balance.initial_ftl_speed_c();
    let _ftl_blockers = routing::collect_ftl_blockers(&regions);
    let _settling_duration = balance.settling_duration();
    let _survey_range_base = balance.survey_range_ly();
    let _survey_duration_base = balance.survey_duration();
    let _empire_knowledge = empire_knowledge_q.single().ok();
    let _hostile_faction_map: std::collections::HashMap<Entity, Entity> = hostiles_q
        .iter()
        .map(|(at_system, owner)| (at_system.0, owner.0))
        .collect();
    let _ = &design_registry;
    for (_entity, ship, mut state, mut queue, ship_pos, _roe) in ships.iter_mut() {
        // #185: Process queue when ship is Docked OR Loitering (current command finished).
        let docked_system: Option<Entity> = match *state {
            ShipState::Docked { system } => Some(system),
            ShipState::Loitering { .. } => None,
            _ => continue,
        };

        if queue.commands.is_empty() {
            continue;
        }

        // Peek at the next command without consuming it yet.
        let next = &queue.commands[0];

        match next {
            // #334 Phase 1: MoveTo / MoveToCoordinates are handled by
            // `super::dispatcher::dispatch_queued_commands` +
            // `super::handlers::move_handler::{handle_move_requested,
            // handle_move_to_coordinates_requested}`. The dispatcher
            // normally pops these before we run, but Phase 2 handlers
            // (e.g. `handle_deploy_deliverable_requested`) can auto-inject
            // a MoveTo/MoveToCoordinates at the queue head in the SAME
            // tick as the dispatcher runs. Leave it untouched here so the
            // next-tick dispatcher picks it up.
            QueuedCommand::MoveTo { .. } | QueuedCommand::MoveToCoordinates { .. } => {
                // Skip — dispatcher will process next tick.
            }
            QueuedCommand::Scout { .. } => {
                // #217: Consume and dispatch synchronously. If not at the
                // target yet, auto-insert a MoveTo and retry; else transition
                // into ShipState::Scouting.
                let next = queue.commands.remove(0);
                let QueuedCommand::Scout {
                    target_system,
                    observation_duration,
                    report_mode,
                } = next
                else {
                    unreachable!("outer match guarantees Scout variant");
                };
                let Ok((_target_entity, _target_star, _target_pos)) = systems.get(target_system)
                else {
                    warn!("Queued Scout target no longer exists");
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                };
                // Non-FTL ships are disallowed from Scout — scouts must
                // leap to the target. Reject early with a warning.
                if ship.ftl_range <= 0.0 {
                    warn!("Scout rejected: ship {} has no FTL capability", ship.name);
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                }
                // Must carry the scout module.
                if !super::scout::ship_has_scout_module(ship) {
                    warn!("Scout rejected: ship {} lacks a scout module", ship.name);
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                }
                // If not at target, prepend a MoveTo and re-queue Scout.
                if docked_system != Some(target_system) {
                    queue.commands.insert(
                        0,
                        QueuedCommand::Scout {
                            target_system,
                            observation_duration,
                            report_mode,
                        },
                    );
                    queue.commands.insert(
                        0,
                        QueuedCommand::MoveTo {
                            system: target_system,
                        },
                    );
                    info!(
                        "Queue: Ship {} not at Scout target — auto-inserting MoveTo",
                        ship.name
                    );
                    continue;
                }
                // #217: origin_system for reporting is the ship's home port
                // — not the current dock. Otherwise a ship that auto-moved
                // to target and started scouting would be "home" already
                // when the report is delivered (bug).
                let origin_system = ship.home_port;
                *state = ShipState::Scouting {
                    target_system,
                    origin_system,
                    started_at: clock.elapsed,
                    completes_at: clock.elapsed + observation_duration,
                    report_mode,
                };
                info!(
                    "Queue: Ship {} began scouting target (duration {} hexadies, mode {:?})",
                    ship.name, observation_duration, report_mode
                );
                queue.sync_prediction(ship_pos.as_array(), Some(target_system));
            }
            // #334 Phase 2 (Commit 4): Survey / Colonize migrated to
            // `handlers::settlement_handler`. Exhaustive match requires the
            // arms stay listed; they're unreachable under Phase 2 schedule
            // but we silently skip (same guard as MoveTo/MoveToCoordinates
            // above) to let any handler-injected retry survive the tick.
            QueuedCommand::Survey { .. }
            | QueuedCommand::Colonize { .. }
            | QueuedCommand::LoadDeliverable { .. }
            | QueuedCommand::DeployDeliverable { .. }
            | QueuedCommand::TransferToStructure { .. }
            | QueuedCommand::LoadFromScrapyard { .. } => {
                // no-op; handled by dispatcher + handler pipeline. Leave
                // at the queue head so the next-tick dispatcher picks up
                // any retry re-injected by a handler this tick.
            }
        }
    }
}
