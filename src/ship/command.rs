use bevy::prelude::*;

use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::ship_design::ShipDesignRegistry;
use crate::time_system::GameClock;

use super::movement::{
    start_ftl_travel_with_bonus, start_sublight_travel_with_bonus, PortParams,
};
use super::survey::start_survey_with_bonus;
use super::settlement::SETTLING_DURATION_HEXADIES;
use super::{
    CommandQueue, QueuedCommand, PendingShipCommand, ShipCommand, RulesOfEngagement,
    Ship, ShipState, INITIAL_FTL_SPEED_C,
};
use super::routing;

// --- Pending ship command processing (#33) ---

/// Processes pending ship commands that have arrived after communication delay.
/// #45: Uses GlobalParams for tech bonuses
/// #46: Checks for port facility at origin system
pub fn process_pending_ship_commands(
    mut commands: Commands,
    clock: Res<GameClock>,
    empire_params_q: Query<&crate::technology::GlobalParams, With<crate::player::PlayerEmpire>>,
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
                let port_params = system_buildings.get(docked_system)
                    .map(|sb| PortParams::from_system_buildings(sb, &building_registry))
                    .unwrap_or(PortParams::NONE);
                match start_ftl_travel_with_bonus(
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
                ) {
                    Ok(()) => {
                        info!(
                            "Remote move command executed: {} FTL jumping to {}",
                            ship.name, dest_star.name,
                        );
                    }
                    Err(_) => {
                        // Fall back to sublight
                        start_sublight_travel_with_bonus(
                            &mut state,
                            ship_pos,
                            &ship,
                            *dest_pos,
                            Some(dest),
                            clock.elapsed,
                            global_params.sublight_speed_bonus,
                        );
                        info!(
                            "Remote move command executed: {} sub-light to {}",
                            ship.name, dest_star.name,
                        );
                    }
                }
            }
            ShipCommand::Survey { target } => {
                let tgt = *target;
                let Ok((tgt_star, tgt_pos)) = systems.get(tgt) else {
                    commands.entity(cmd_entity).despawn();
                    continue;
                };
                match start_survey_with_bonus(&mut state, &ship, tgt, ship_pos, tgt_pos, clock.elapsed, global_params.survey_range_bonus, &design_registry) {
                    Ok(()) => {
                        info!(
                            "Remote survey command executed: {} surveying {}",
                            ship.name, tgt_star.name,
                        );
                    }
                    Err(e) => {
                        info!(
                            "Remote survey command for {} failed: {}",
                            ship.name, e,
                        );
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
                        completes_at: clock.elapsed + SETTLING_DURATION_HEXADIES,
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
pub fn process_command_queue(
    mut commands: Commands,
    clock: Res<GameClock>,
    empire_params_q: Query<&crate::technology::GlobalParams, With<crate::player::PlayerEmpire>>,
    // #187: Player empire's KnowledgeStore used for Retreat-route avoidance.
    empire_knowledge_q: Query<&crate::knowledge::KnowledgeStore, With<crate::player::PlayerEmpire>>,
    mut ships: Query<(Entity, &Ship, &mut ShipState, &mut CommandQueue, &Position, Option<&RulesOfEngagement>), Without<routing::PendingRoute>>,
    systems: Query<(Entity, &StarSystem, &Position), Without<Ship>>,
    system_buildings: Query<&crate::colony::SystemBuildings>,
    _planets: Query<&crate::galaxy::Planet>,
    // #187: Hostile garrisons + faction ownership keyed by star system.
    hostiles_q: Query<(&crate::galaxy::HostilePresence, &crate::faction::FactionOwner)>,
    relations: Res<crate::faction::FactionRelations>,
    mut pending_count: ResMut<routing::RouteCalculationsPending>,
    design_registry: Res<ShipDesignRegistry>,
    building_registry: Res<crate::colony::BuildingRegistry>,
) {
    let Ok(global_params) = empire_params_q.single() else {
        return;
    };
    let empire_knowledge = empire_knowledge_q.single().ok();
    // #187: Build the hostile system → hostile faction map once per tick.
    let hostile_faction_map: std::collections::HashMap<Entity, Entity> = hostiles_q
        .iter()
        .map(|(h, owner)| (h.system, owner.0))
        .collect();
    for (entity, ship, mut state, mut queue, ship_pos, roe) in ships.iter_mut() {
        // #187: ROE defaults to Defensive when absent (matches Ship::default spawn).
        let roe = roe.copied().unwrap_or_default();
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
            QueuedCommand::MoveTo { system: target } => {
                let target = *target;
                let Ok((_target_entity, _target_star, target_pos)) = systems.get(target) else {
                    warn!("Queued MoveTo target no longer exists");
                    queue.commands.remove(0);
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                };

                // Already at target (only possible when docked)?
                if docked_system == Some(target) {
                    queue.commands.remove(0);
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                }

                // For loitering ships, fall back to a direct sublight move (no FTL chain
                // planning from deep space).
                if docked_system.is_none() {
                    queue.commands.remove(0);
                    start_sublight_travel_with_bonus(
                        &mut state,
                        ship_pos,
                        ship,
                        Position::from(target_pos.as_array()),
                        Some(target),
                        clock.elapsed,
                        global_params.sublight_speed_bonus,
                    );
                    queue.sync_prediction(target_pos.as_array(), Some(target));
                    info!("Queue: Loitering ship {} sublight to target", ship.name);
                    continue;
                }

                // Docked: use the system's position as origin for FTL route planning.
                let docked_sys_for_origin = docked_system.expect("docked branch");
                let Ok((_, _, origin_pos)) = systems.get(docked_sys_for_origin) else {
                    warn!("Queue: Origin system no longer exists");
                    queue.commands.remove(0);
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                };
                let origin_pos_arr = origin_pos.as_array();

                // From here we know docked_system.is_some(); only docked ships use the
                // FTL route planner.
                let docked_sys = docked_system.expect("loitering branch handled above");
                let port_params = system_buildings.get(docked_sys)
                    .map(|sb| PortParams::from_system_buildings(sb, &building_registry))
                    .unwrap_or(PortParams::NONE);
                let effective_ftl_range = if ship.ftl_range > 0.0 {
                    ship.ftl_range + global_params.ftl_range_bonus + port_params.ftl_range_bonus
                } else {
                    0.0
                };
                let effective_ftl_speed = INITIAL_FTL_SPEED_C * global_params.ftl_speed_multiplier;
                let effective_sublight_speed = ship.sublight_speed + global_params.sublight_speed_bonus;

                // Spawn async route computation task.
                // #187: Feed per-ship ROE + KnowledgeStore-derived hostile info.
                let ship_faction = match ship.owner {
                    super::Owner::Empire(f) => Some(f),
                    super::Owner::Neutral => None,
                };
                let snapshots = routing::collect_route_snapshots(
                    &systems,
                    empire_knowledge,
                    &relations,
                    ship_faction,
                    &hostile_faction_map,
                );
                let task = routing::spawn_route_task_with_roe(
                    origin_pos_arr,
                    target,
                    effective_ftl_range,
                    effective_sublight_speed,
                    effective_ftl_speed,
                    snapshots,
                    roe,
                );
                commands.entity(entity).insert(routing::PendingRoute {
                    task,
                    target_system: target,
                });
                pending_count.count += 1;
                info!("Queue: Ship {} spawned async route to target", ship.name);
                // Do NOT consume the MoveTo — poll_pending_routes will handle it.
            }
            QueuedCommand::MoveToCoordinates { target } => {
                // #185: Move sublight to deep-space coordinates and loiter on arrival.
                let target_arr = *target;
                queue.commands.remove(0);
                start_sublight_travel_with_bonus(
                    &mut state,
                    ship_pos,
                    ship,
                    Position::from(target_arr),
                    None, // no target system → arrival transitions to Loitering
                    clock.elapsed,
                    global_params.sublight_speed_bonus,
                );
                queue.sync_prediction(target_arr, None);
                info!(
                    "Queue: Ship {} sublight to deep-space coordinates ({:.2},{:.2},{:.2})",
                    ship.name, target_arr[0], target_arr[1], target_arr[2]
                );
            }
            QueuedCommand::Survey { .. } | QueuedCommand::Colonize { .. } => {
                // Consume the command and process synchronously.
                let next = queue.commands.remove(0);
                match next {
                    QueuedCommand::Survey { system: target } => {
                        let Ok((_target_entity, target_star, target_pos)) = systems.get(target) else {
                            warn!("Queued Survey target no longer exists");
                            queue.sync_prediction(ship_pos.as_array(), docked_system);
                            continue;
                        };
                        // #101: If not docked at the target system, auto-insert a move command.
                        // #185: Loitering ships also need to move first.
                        if docked_system != Some(target) {
                            queue.commands.insert(0, QueuedCommand::Survey { system: target });
                            queue.commands.insert(0, QueuedCommand::MoveTo { system: target });
                            info!("Queue: Ship {} not at target, auto-inserting move before survey of {}", ship.name, target_star.name);
                            continue;
                        }
                        let origin = Position::from(ship_pos.as_array());
                        match start_survey_with_bonus(
                            &mut state,
                            ship,
                            target,
                            &origin,
                            target_pos,
                            clock.elapsed,
                            global_params.survey_range_bonus,
                            &design_registry,
                        ) {
                            Ok(()) => {
                                info!(
                                    "Queue: Ship {} surveying {}",
                                    ship.name, target_star.name
                                );
                            }
                            Err(e) => {
                                warn!("Queue: Survey failed for {}: {}", ship.name, e);
                            }
                        }
                        queue.sync_prediction(ship_pos.as_array(), docked_system);
                    }
                    QueuedCommand::Colonize { system: target, planet } => {
                        let Ok((_target_entity, target_star, _target_pos)) = systems.get(target) else {
                            warn!("Queued Colonize target no longer exists");
                            queue.sync_prediction(ship_pos.as_array(), docked_system);
                            continue;
                        };
                        if docked_system != Some(target) {
                            queue.commands.insert(0, QueuedCommand::Colonize { system: target, planet });
                            queue.commands.insert(0, QueuedCommand::MoveTo { system: target });
                            info!("Queue: Ship {} not at target, auto-inserting move before colonize of {}", ship.name, target_star.name);
                            continue;
                        }
                        if !design_registry.can_colonize(&ship.design_id) {
                            warn!("Queue: Ship {} cannot colonize (not a colony ship)", ship.name);
                            queue.sync_prediction(ship_pos.as_array(), docked_system);
                            continue;
                        }
                        let docked_sys = docked_system.expect("survey/colonize already required docked");
                        *state = ShipState::Settling {
                            system: docked_sys,
                            planet,
                            started_at: clock.elapsed,
                            completes_at: clock.elapsed + SETTLING_DURATION_HEXADIES,
                        };
                        info!(
                            "Queue: Ship {} colonizing {}",
                            ship.name, target_star.name
                        );
                        queue.sync_prediction(ship_pos.as_array(), docked_system);
                    }
                    QueuedCommand::MoveToCoordinates { .. } | QueuedCommand::MoveTo { .. } => {} // handled above
                }
            }
        }
    }
}
