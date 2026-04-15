use bevy::prelude::*;

use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::ship_design::ShipDesignRegistry;
use crate::time_system::GameClock;

use super::movement::{
    start_ftl_travel_full, start_sublight_travel_with_bonus, PortParams,
};
use super::survey::start_survey_with_bonus;
use super::{
    CommandQueue, QueuedCommand, PendingShipCommand, ShipCommand, RulesOfEngagement,
    Ship, ShipState,
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
                let port_params = system_buildings.get(docked_system)
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
                                info!(
                                    "Remote move command rejected for {}: {}",
                                    ship.name, e,
                                );
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
                match start_survey_with_bonus(&mut state, &ship, tgt, ship_pos, tgt_pos, clock.elapsed, global_params.survey_range_bonus, &design_registry, survey_range, survey_duration) {
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
pub fn process_command_queue(
    mut commands: Commands,
    clock: Res<GameClock>,
    empire_params_q: Query<&crate::technology::GlobalParams, With<crate::player::PlayerEmpire>>,
    balance: Res<crate::technology::GameBalance>,
    // #187: Player empire's KnowledgeStore used for Retreat-route avoidance.
    empire_knowledge_q: Query<&crate::knowledge::KnowledgeStore, With<crate::player::PlayerEmpire>>,
    mut ships: Query<(Entity, &Ship, &mut ShipState, &mut CommandQueue, &Position, Option<&RulesOfEngagement>), Without<routing::PendingRoute>>,
    systems: Query<(Entity, &StarSystem, &Position), Without<Ship>>,
    system_buildings: Query<&crate::colony::SystemBuildings>,
    _planets: Query<&crate::galaxy::Planet>,
    // #187/#293: Hostile garrisons + faction ownership keyed by star system.
    hostiles_q: Query<
        (&crate::galaxy::AtSystem, &crate::faction::FactionOwner),
        With<crate::galaxy::Hostile>,
    >,
    relations: Res<crate::faction::FactionRelations>,
    mut pending_count: ResMut<routing::RouteCalculationsPending>,
    design_registry: Res<ShipDesignRegistry>,
    building_registry: Res<crate::colony::BuildingRegistry>,
    // #145: Forbidden regions that block FTL travel.
    regions: Query<&crate::galaxy::ForbiddenRegion>,
) {
    let Ok(global_params) = empire_params_q.single() else {
        return;
    };
    let base_ftl_speed = balance.initial_ftl_speed_c();
    // #145: snapshot FTL-blocking regions once per tick; cheap since the
    // region set is small (single-digit count) at MVP scope.
    let ftl_blockers = routing::collect_ftl_blockers(&regions);
    let settling_duration = balance.settling_duration();
    let survey_range_base = balance.survey_range_ly();
    let survey_duration_base = balance.survey_duration();
    let empire_knowledge = empire_knowledge_q.single().ok();
    // #187/#293: Build the hostile system → hostile faction map once per tick.
    let hostile_faction_map: std::collections::HashMap<Entity, Entity> = hostiles_q
        .iter()
        .map(|(at_system, owner)| (at_system.0, owner.0))
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
                    // #296: immobile ships can't move sublight either —
                    // consume the command with a warning.
                    if let Err(e) = start_sublight_travel_with_bonus(
                        &mut state,
                        ship_pos,
                        ship,
                        Position::from(target_pos.as_array()),
                        Some(target),
                        clock.elapsed,
                        global_params.sublight_speed_bonus,
                    ) {
                        warn!(
                            "Queue: Loitering ship {} cannot sublight to target: {}",
                            ship.name, e
                        );
                    } else {
                        queue.sync_prediction(target_pos.as_array(), Some(target));
                        info!("Queue: Loitering ship {} sublight to target", ship.name);
                    }
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
                let effective_ftl_speed = base_ftl_speed * global_params.ftl_speed_multiplier;
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
                let task = routing::spawn_route_task_full(
                    origin_pos_arr,
                    target,
                    effective_ftl_range,
                    effective_sublight_speed,
                    effective_ftl_speed,
                    snapshots,
                    roe,
                    ftl_blockers.clone(),
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
                // #296: immobile ships cannot leave their docking system.
                match start_sublight_travel_with_bonus(
                    &mut state,
                    ship_pos,
                    ship,
                    Position::from(target_arr),
                    None, // no target system → arrival transitions to Loitering
                    clock.elapsed,
                    global_params.sublight_speed_bonus,
                ) {
                    Ok(()) => {
                        queue.sync_prediction(target_arr, None);
                        info!(
                            "Queue: Ship {} sublight to deep-space coordinates ({:.2},{:.2},{:.2})",
                            ship.name, target_arr[0], target_arr[1], target_arr[2]
                        );
                    }
                    Err(e) => {
                        warn!(
                            "Queue: Ship {} cannot MoveToCoordinates: {}",
                            ship.name, e
                        );
                    }
                }
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
                let Ok((_target_entity, _target_star, _target_pos)) =
                    systems.get(target_system)
                else {
                    warn!("Queued Scout target no longer exists");
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                };
                // Non-FTL ships are disallowed from Scout — scouts must
                // leap to the target. Reject early with a warning.
                if ship.ftl_range <= 0.0 {
                    warn!(
                        "Scout rejected: ship {} has no FTL capability",
                        ship.name
                    );
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                }
                // Must carry the scout module.
                if !super::scout::ship_has_scout_module(ship) {
                    warn!(
                        "Scout rejected: ship {} lacks a scout module",
                        ship.name
                    );
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
                            survey_range_base,
                            survey_duration_base,
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
                            completes_at: clock.elapsed + settling_duration,
                        };
                        info!(
                            "Queue: Ship {} colonizing {}",
                            ship.name, target_star.name
                        );
                        queue.sync_prediction(ship_pos.as_array(), docked_system);
                    }
                    QueuedCommand::MoveToCoordinates { .. } | QueuedCommand::MoveTo { .. } => {} // handled above
                    // #217: Scout is handled by its own outer arm.
                    QueuedCommand::Scout { .. } => {}
                    // #223: Deliverable-side commands are handled by
                    // `super::deliverable_ops::process_deliverable_commands`.
                    // This arm is unreachable via the outer match, but the
                    // exhaustive compiler still needs the variants enumerated.
                    QueuedCommand::LoadDeliverable { .. }
                    | QueuedCommand::DeployDeliverable { .. }
                    | QueuedCommand::TransferToStructure { .. }
                    | QueuedCommand::LoadFromScrapyard { .. } => {
                        // handled by deliverable_ops
                    }
                }
            }
            // #223: Deliverable-side commands are processed by
            // `super::deliverable_ops::process_deliverable_commands`, which
            // runs as its own system. We skip them here and leave them at
            // the head of the queue.
            QueuedCommand::LoadDeliverable { .. }
            | QueuedCommand::DeployDeliverable { .. }
            | QueuedCommand::TransferToStructure { .. }
            | QueuedCommand::LoadFromScrapyard { .. } => {
                // no-op; the deliverable ops system consumes these.
            }
        }
    }
}
