use bevy::prelude::*;

use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::ship_design::ShipDesignRegistry;
use crate::time_system::GameClock;

use super::movement::{PortParams, start_ftl_travel_full, start_sublight_travel_with_bonus};
use super::survey::start_survey_with_bonus;
use super::{CommandQueue, PendingShipCommand, Ship, ShipCommand, ShipState};

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
            ShipState::InSystem { system } => system,
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
//
// #334 Phase 3 (Commit 3): the legacy dispatch loop in this module has
// been **deleted**. Every `QueuedCommand` variant is now emitted by
// `super::dispatcher::dispatch_queued_commands` and consumed by a
// focused handler under `super::handlers`. The corresponding scheduler
// hooks in `ShipPlugin` / `test_app` / `full_test_app` were retargeted
// to anchor on the last handler in the dispatcher chain.
