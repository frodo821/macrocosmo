//! #334 Phase 3 (Commit 1): `Scout` handler.
//!
//! Extracted from the `Scout` arm of `command::process_command_queue`
//! (now deleted in Phase 3 Commit 3). Semantics are preserved verbatim:
//! - Target system must still exist.
//! - Ship must have non-zero FTL range (scouts must leap to target).
//! - Ship must carry a `scout` module (`super::scout::ship_has_scout_module`).
//! - If not at target, auto-insert `MoveTo` ahead of a re-queued `Scout`
//!   (Deferred result).
//! - Otherwise transition into `ShipState::Scouting`, origin = home port
//!   (so a ship that auto-moved to target still reports home).
//!
//! Scout lifecycle (`tick_scout_observation`, `process_scout_report`) is
//! untouched — this handler only INITIATES the scout transition.

use bevy::prelude::*;

use crate::components::Position;
use crate::time_system::GameClock;

use crate::ship::command_events::{
    CommandExecuted, CommandKind, CommandResult, ScoutRequested,
};
use crate::ship::{CommandQueue, QueuedCommand, Ship, ShipState};

#[allow(clippy::too_many_arguments)]
pub fn handle_scout_requested(
    clock: Res<GameClock>,
    mut reqs: MessageReader<ScoutRequested>,
    mut executed: MessageWriter<CommandExecuted>,
    mut ships: Query<(&Ship, &mut ShipState, &Position, &mut CommandQueue)>,
    systems: Query<&crate::galaxy::StarSystem, Without<Ship>>,
) {
    for req in reqs.read() {
        let Ok((ship, mut state, ship_pos, mut queue)) = ships.get_mut(req.ship) else {
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::Scout,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "ship unavailable".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };

        let docked_system: Option<Entity> = match *state {
            ShipState::Docked { system } => Some(system),
            ShipState::Loitering { .. } => None,
            _ => {
                executed.write(CommandExecuted {
                    command_id: req.command_id,
                    kind: CommandKind::Scout,
                    ship: req.ship,
                    result: CommandResult::Rejected {
                        reason: "ship not idle".to_string(),
                    },
                    completed_at: clock.elapsed,
                });
                continue;
            }
        };

        // Target system must still exist.
        if systems.get(req.target_system).is_err() {
            warn!("Queued Scout target no longer exists");
            queue.sync_prediction(ship_pos.as_array(), docked_system);
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::Scout,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "target system despawned".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        }

        // Non-FTL ships are disallowed from Scout — scouts must leap to
        // the target.
        if ship.ftl_range <= 0.0 {
            warn!("Scout rejected: ship {} has no FTL capability", ship.name);
            queue.sync_prediction(ship_pos.as_array(), docked_system);
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::Scout,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "ship lacks FTL".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        }

        // Must carry the scout module.
        if !super::super::scout::ship_has_scout_module(ship) {
            warn!("Scout rejected: ship {} lacks a scout module", ship.name);
            queue.sync_prediction(ship_pos.as_array(), docked_system);
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::Scout,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "ship lacks scout module".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        }

        // If not at target, prepend MoveTo and re-queue Scout.
        if docked_system != Some(req.target_system) {
            queue.commands.insert(
                0,
                QueuedCommand::Scout {
                    target_system: req.target_system,
                    observation_duration: req.observation_duration,
                    report_mode: req.report_mode,
                },
            );
            queue.commands.insert(
                0,
                QueuedCommand::MoveTo {
                    system: req.target_system,
                },
            );
            info!(
                "Queue: Ship {} not at Scout target — auto-inserting MoveTo",
                ship.name
            );
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::Scout,
                ship: req.ship,
                result: CommandResult::Deferred,
                completed_at: clock.elapsed,
            });
            continue;
        }

        // #217: origin_system for reporting is the ship's home port — not
        // the current dock. Otherwise a ship that auto-moved to target and
        // started scouting would be "home" already when the report is
        // delivered (bug).
        let origin_system = ship.home_port;
        *state = ShipState::Scouting {
            target_system: req.target_system,
            origin_system,
            started_at: clock.elapsed,
            completes_at: clock.elapsed + req.observation_duration,
            report_mode: req.report_mode,
        };
        info!(
            "Queue: Ship {} began scouting target (duration {} hexadies, mode {:?})",
            ship.name, req.observation_duration, req.report_mode
        );
        queue.sync_prediction(ship_pos.as_array(), Some(req.target_system));
        executed.write(CommandExecuted {
            command_id: req.command_id,
            kind: CommandKind::Scout,
            ship: req.ship,
            result: CommandResult::Ok,
            completed_at: clock.elapsed,
        });
    }
}
