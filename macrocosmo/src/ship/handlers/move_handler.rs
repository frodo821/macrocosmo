//! #334 Phase 1: MoveTo / MoveToCoordinates handler systems.
//!
//! `handle_move_requested` consumes `MoveRequested` messages and executes
//! the same FTL-chain routing logic as the legacy `process_command_queue`
//! MoveTo arm (see `docs/plan-334-command-dispatch-event-driven.md` §2.3).
//! The async route task is spawned via [`routing::spawn_route_task_full`];
//! `poll_pending_routes` finalizes the terminal `CommandExecuted` when the
//! route resolves.
//!
//! `handle_move_to_coordinates_requested` consumes `MoveToCoordinatesRequested`
//! and runs a synchronous sublight-to-deep-space move, emitting a terminal
//! `CommandExecuted` immediately on success/failure.

use bevy::prelude::*;

use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::ship_design::ShipDesignRegistry;
use crate::time_system::GameClock;

use crate::ship::command_events::{
    CommandExecuted, CommandKind, CommandResult, MoveRequested, MoveToCoordinatesRequested,
};
use crate::ship::movement::{PortParams, start_sublight_travel_with_bonus};
use crate::ship::routing;
use crate::ship::{CommandQueue, RulesOfEngagement, Ship, ShipState};

/// Handles `MoveRequested`. Runs **before** the legacy `process_command_queue`
/// (see `ShipPlugin` schedule) so that spawning `PendingRoute` correctly
/// excludes the ship from the legacy system's `Without<PendingRoute>`
/// filter for this tick.
///
/// On success (async route spawned): emits nothing — `poll_pending_routes`
/// will emit the terminal `CommandExecuted { result: Ok/Rejected }` when
/// the route resolves. (Plan §3.3 `Deferred` semantics are carried
/// implicitly by the presence of `PendingRoute`.)
///
/// On sync failure (e.g. target despawned between dispatcher and handler,
/// or Loitering sublight-fallback fails): emits `CommandExecuted` with
/// `CommandResult::Rejected`.
#[allow(clippy::too_many_arguments)]
pub fn handle_move_requested(
    mut commands: Commands,
    clock: Res<GameClock>,
    mut reqs: MessageReader<MoveRequested>,
    empire_params_q: Query<&crate::technology::GlobalParams, With<crate::player::PlayerEmpire>>,
    balance: Res<crate::technology::GameBalance>,
    empire_knowledge_q: Query<&crate::knowledge::KnowledgeStore, With<crate::player::PlayerEmpire>>,
    // Same shape as the legacy `process_command_queue` ship query, minus
    // `&mut CommandQueue` (we don't touch the queue from the handler — the
    // dispatcher already popped the MoveTo).
    mut ships: Query<
        (&Ship, &mut ShipState, &Position, Option<&RulesOfEngagement>),
        Without<routing::PendingRoute>,
    >,
    systems: Query<(Entity, &StarSystem, &Position), Without<Ship>>,
    system_buildings: Query<&crate::colony::SystemBuildings>,
    hostiles_q: Query<
        (&crate::galaxy::AtSystem, &crate::faction::FactionOwner),
        With<crate::galaxy::Hostile>,
    >,
    relations: Res<crate::faction::FactionRelations>,
    mut pending_count: ResMut<routing::RouteCalculationsPending>,
    design_registry: Res<ShipDesignRegistry>,
    building_registry: Res<crate::colony::BuildingRegistry>,
    regions: Query<&crate::galaxy::ForbiddenRegion>,
    mut executed: MessageWriter<CommandExecuted>,
) {
    // Early exit when no MoveRequested messages this tick.
    let Some(global_params) = empire_params_q.single().ok() else {
        // No empire → drain without action (avoids eating messages) but
        // also don't emit terminals since there's nobody to hear them.
        for _ in reqs.read() {}
        return;
    };
    let _ = &design_registry; // kept for symmetry with legacy; future uses
    let base_ftl_speed = balance.initial_ftl_speed_c();
    let ftl_blockers = routing::collect_ftl_blockers(&regions);
    let empire_knowledge = empire_knowledge_q.single().ok();
    let hostile_faction_map: std::collections::HashMap<Entity, Entity> = hostiles_q
        .iter()
        .map(|(at_system, owner)| (at_system.0, owner.0))
        .collect();

    for req in reqs.read() {
        let ship_entity = req.ship;
        let target = req.target;

        let Ok((ship, mut state, ship_pos, roe)) = ships.get_mut(ship_entity) else {
            // Ship despawned or already has PendingRoute (another
            // dispatcher tick raced us). Reject.
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::Move,
                ship: ship_entity,
                result: CommandResult::Rejected {
                    reason: "ship unavailable for move".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };
        let roe = roe.copied().unwrap_or_default();

        // The dispatcher already validated that the target exists; re-check
        // here to guard against a same-tick despawn race (plan §3.2).
        let Ok((_, _target_star, target_pos)) = systems.get(target) else {
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::Move,
                ship: ship_entity,
                result: CommandResult::Rejected {
                    reason: "target system despawned".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };

        // Determine whether the ship is Docked or Loitering; other states
        // should never arrive here (dispatcher guards them) but we reject
        // gracefully if so.
        let docked_system: Option<Entity> = match *state {
            ShipState::InSystem { system } => Some(system),
            ShipState::Loitering { .. } => None,
            _ => {
                executed.write(CommandExecuted {
                    command_id: req.command_id,
                    kind: CommandKind::Move,
                    ship: ship_entity,
                    result: CommandResult::Rejected {
                        reason: "ship not idle".to_string(),
                    },
                    completed_at: clock.elapsed,
                });
                continue;
            }
        };

        // Loitering ship: direct sublight, no FTL route planner.
        if docked_system.is_none() {
            match start_sublight_travel_with_bonus(
                &mut state,
                ship_pos,
                ship,
                Position::from(target_pos.as_array()),
                Some(target),
                clock.elapsed,
                global_params.sublight_speed_bonus,
            ) {
                Ok(()) => {
                    info!(
                        "handle_move: Loitering ship {} sublight to target (cmd {})",
                        ship.name, req.command_id.0
                    );
                    executed.write(CommandExecuted {
                        command_id: req.command_id,
                        kind: CommandKind::Move,
                        ship: ship_entity,
                        result: CommandResult::Ok,
                        completed_at: clock.elapsed,
                    });
                }
                Err(e) => {
                    warn!(
                        "handle_move: Loitering ship {} cannot sublight to target: {}",
                        ship.name, e
                    );
                    executed.write(CommandExecuted {
                        command_id: req.command_id,
                        kind: CommandKind::Move,
                        ship: ship_entity,
                        result: CommandResult::Rejected {
                            reason: format!("sublight start failed: {}", e),
                        },
                        completed_at: clock.elapsed,
                    });
                }
            }
            continue;
        }

        // Docked: spawn async route planner (FTL chain / hybrid / sublight).
        let docked_sys = docked_system.expect("docked branch");
        let Ok((_, _, origin_pos)) = systems.get(docked_sys) else {
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::Move,
                ship: ship_entity,
                result: CommandResult::Rejected {
                    reason: "origin system lost".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };
        let origin_pos_arr = origin_pos.as_array();
        let port_params = system_buildings
            .get(docked_sys)
            .map(|sb| PortParams::from_system_buildings(sb, &building_registry))
            .unwrap_or(PortParams::NONE);
        let effective_ftl_range = if ship.ftl_range > 0.0 {
            ship.ftl_range + global_params.ftl_range_bonus + port_params.ftl_range_bonus
        } else {
            0.0
        };
        let effective_ftl_speed = base_ftl_speed * global_params.ftl_speed_multiplier;
        let effective_sublight_speed = ship.sublight_speed + global_params.sublight_speed_bonus;

        let ship_faction = match ship.owner {
            crate::ship::Owner::Empire(f) => Some(f),
            crate::ship::Owner::Neutral => None,
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
        commands.entity(ship_entity).insert(routing::PendingRoute {
            task,
            target_system: target,
            command_id: Some(req.command_id),
        });
        pending_count.count += 1;
        info!(
            "handle_move: ship {} spawned async route to target (cmd {})",
            ship.name, req.command_id.0
        );
        // Terminal `CommandExecuted` (Ok or Rejected) is emitted later by
        // `poll_pending_routes` once the async task resolves.
    }
}

/// Handles `MoveToCoordinatesRequested` — synchronous sublight to a deep-space coord.
#[allow(clippy::too_many_arguments)]
pub fn handle_move_to_coordinates_requested(
    clock: Res<GameClock>,
    mut reqs: MessageReader<MoveToCoordinatesRequested>,
    empire_params_q: Query<&crate::technology::GlobalParams, With<crate::player::PlayerEmpire>>,
    mut ships: Query<
        (&Ship, &mut ShipState, &Position, &mut CommandQueue),
        Without<routing::PendingRoute>,
    >,
    mut executed: MessageWriter<CommandExecuted>,
) {
    let Ok(global_params) = empire_params_q.single() else {
        for _ in reqs.read() {}
        return;
    };

    for req in reqs.read() {
        let Ok((ship, mut state, ship_pos, mut queue)) = ships.get_mut(req.ship) else {
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::MoveToCoordinates,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "ship unavailable".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };

        match start_sublight_travel_with_bonus(
            &mut state,
            ship_pos,
            ship,
            Position::from(req.target),
            None,
            clock.elapsed,
            global_params.sublight_speed_bonus,
        ) {
            Ok(()) => {
                queue.sync_prediction(req.target, None);
                info!(
                    "handle_move_xy: ship {} sublight to ({:.2},{:.2},{:.2}) (cmd {})",
                    ship.name, req.target[0], req.target[1], req.target[2], req.command_id.0,
                );
                executed.write(CommandExecuted {
                    command_id: req.command_id,
                    kind: CommandKind::MoveToCoordinates,
                    ship: req.ship,
                    result: CommandResult::Ok,
                    completed_at: clock.elapsed,
                });
            }
            Err(e) => {
                warn!(
                    "handle_move_xy: ship {} cannot MoveToCoordinates: {}",
                    ship.name, e
                );
                executed.write(CommandExecuted {
                    command_id: req.command_id,
                    kind: CommandKind::MoveToCoordinates,
                    ship: req.ship,
                    result: CommandResult::Rejected {
                        reason: format!("sublight start failed: {}", e),
                    },
                    completed_at: clock.elapsed,
                });
            }
        }
    }
}
