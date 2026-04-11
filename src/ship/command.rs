use bevy::prelude::*;

use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::ship_design::ShipDesignRegistry;
use crate::time_system::GameClock;

use super::movement::{
    start_ftl_travel_with_bonus, start_sublight_travel_with_bonus, PORT_FTL_RANGE_BONUS_LY,
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
                let origin_has_port = system_buildings.get(docked_system).is_ok_and(|sb| sb.has_port());
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
                    origin_has_port,
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
    mut ships: Query<(Entity, &Ship, &mut ShipState, &mut CommandQueue, &Position), Without<routing::PendingRoute>>,
    systems: Query<(Entity, &StarSystem, &Position), Without<Ship>>,
    system_buildings: Query<&crate::colony::SystemBuildings>,
    _planets: Query<&crate::galaxy::Planet>,
    mut pending_count: ResMut<routing::RouteCalculationsPending>,
    design_registry: Res<ShipDesignRegistry>,
) {
    let Ok(global_params) = empire_params_q.single() else {
        return;
    };
    for (entity, ship, mut state, mut queue, ship_pos) in ships.iter_mut() {
        // Only process queue when ship is Docked (current command finished)
        let ShipState::Docked { system: docked_system } = *state else {
            continue;
        };

        if queue.commands.is_empty() {
            continue;
        }

        // Peek at the next command without consuming it yet.
        let next = &queue.commands[0];

        match next {
            QueuedCommand::MoveTo { system: target } => {
                let target = *target;
                let Ok((_target_entity, _target_star, _target_pos)) = systems.get(target) else {
                    warn!("Queued MoveTo target no longer exists");
                    queue.commands.remove(0);
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    continue;
                };

                // Already at target?
                if docked_system == target {
                    queue.commands.remove(0);
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    continue;
                }

                let Ok((_, _, origin_pos)) = systems.get(docked_system) else {
                    warn!("Queue: Origin system no longer exists");
                    queue.commands.remove(0);
                    queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    continue;
                };

                let origin_has_port = system_buildings.get(docked_system).is_ok_and(|sb| sb.has_port());
                let port_range_bonus = if origin_has_port { PORT_FTL_RANGE_BONUS_LY } else { 0.0 };
                let effective_ftl_range = if ship.ftl_range > 0.0 {
                    ship.ftl_range + global_params.ftl_range_bonus + port_range_bonus
                } else {
                    0.0
                };
                let effective_ftl_speed = INITIAL_FTL_SPEED_C * global_params.ftl_speed_multiplier;
                let effective_sublight_speed = ship.sublight_speed + global_params.sublight_speed_bonus;

                // Spawn async route computation task.
                let snapshots = routing::collect_route_snapshots(&systems);
                let task = routing::spawn_route_task(
                    origin_pos.as_array(),
                    target,
                    effective_ftl_range,
                    effective_sublight_speed,
                    effective_ftl_speed,
                    snapshots,
                );
                commands.entity(entity).insert(routing::PendingRoute {
                    task,
                    target_system: target,
                });
                pending_count.count += 1;
                info!("Queue: Ship {} spawned async route to target", ship.name);
                // Do NOT consume the MoveTo — poll_pending_routes will handle it.
            }
            QueuedCommand::Survey { .. } | QueuedCommand::Colonize { .. } => {
                // Consume the command and process synchronously.
                let next = queue.commands.remove(0);
                match next {
                    QueuedCommand::Survey { system: target } => {
                        let Ok((_target_entity, target_star, target_pos)) = systems.get(target) else {
                            warn!("Queued Survey target no longer exists");
                            queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                            continue;
                        };
                        // #101: If not docked at the target system, auto-insert a move command
                        if docked_system != target {
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
                        queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    }
                    QueuedCommand::Colonize { system: target, planet } => {
                        let Ok((_target_entity, target_star, _target_pos)) = systems.get(target) else {
                            warn!("Queued Colonize target no longer exists");
                            queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                            continue;
                        };
                        if docked_system != target {
                            queue.commands.insert(0, QueuedCommand::Colonize { system: target, planet });
                            queue.commands.insert(0, QueuedCommand::MoveTo { system: target });
                            info!("Queue: Ship {} not at target, auto-inserting move before colonize of {}", ship.name, target_star.name);
                            continue;
                        }
                        if !design_registry.can_colonize(&ship.design_id) {
                            warn!("Queue: Ship {} cannot colonize (not a colony ship)", ship.name);
                            queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                            continue;
                        }
                        *state = ShipState::Settling {
                            system: docked_system,
                            planet,
                            started_at: clock.elapsed,
                            completes_at: clock.elapsed + SETTLING_DURATION_HEXADIES,
                        };
                        info!(
                            "Queue: Ship {} colonizing {}",
                            ship.name, target_star.name
                        );
                        queue.sync_prediction(ship_pos.as_array(), Some(docked_system));
                    }
                    _ => {} // MoveTo handled above
                }
            }
        }
    }
}
