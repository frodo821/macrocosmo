//! #334 Phase 2 (Commit 4): `Survey` / `Colonize` handlers.
//!
//! Extracted from the `Survey` / `Colonize` arms of
//! `command::process_command_queue`. Semantics are preserved verbatim:
//! - If the ship is not at the target system, auto-insert a `MoveTo` ahead
//!   of the command (Deferred).
//! - If at target, call `start_survey_with_bonus` / transition into
//!   `ShipState::Settling` (Ok) or reject with a warning (Rejected).
//!
//! The settlement tick system (`process_settling`) and the survey tick
//! system (`process_surveys`) remain unchanged — this handler only
//! INITIATES the action; lifecycle progression runs elsewhere.

use bevy::prelude::*;

use crate::components::Position;
use crate::ship_design::ShipDesignRegistry;
use crate::time_system::GameClock;

use crate::ship::command_events::{
    ColonizeRequested, CommandExecuted, CommandKind, CommandResult, SurveyRequested,
};
use crate::ship::survey::start_survey_with_bonus;
use crate::ship::{CommandQueue, QueuedCommand, Ship, ShipState};

#[allow(clippy::too_many_arguments)]
pub fn handle_survey_requested(
    clock: Res<GameClock>,
    balance: Res<crate::technology::GameBalance>,
    empire_params_q: Query<&crate::technology::GlobalParams, With<crate::player::PlayerEmpire>>,
    mut reqs: MessageReader<SurveyRequested>,
    mut executed: MessageWriter<CommandExecuted>,
    mut ships: Query<(&Ship, &mut ShipState, &Position, &mut CommandQueue)>,
    systems: Query<(&crate::galaxy::StarSystem, &Position), Without<Ship>>,
    design_registry: Res<ShipDesignRegistry>,
) {
    let Ok(global_params) = empire_params_q.single() else {
        for _ in reqs.read() {}
        return;
    };
    let survey_range_base = balance.survey_range_ly();
    let survey_duration_base = balance.survey_duration();

    for req in reqs.read() {
        let Ok((ship, mut state, ship_pos, mut queue)) = ships.get_mut(req.ship) else {
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::Survey,
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
                    kind: CommandKind::Survey,
                    ship: req.ship,
                    result: CommandResult::Rejected {
                        reason: "ship not idle".to_string(),
                    },
                    completed_at: clock.elapsed,
                });
                continue;
            }
        };

        let Ok((target_star, target_pos)) = systems.get(req.target_system) else {
            warn!("Queued Survey target no longer exists");
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::Survey,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "target system despawned".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };

        // Auto-insert a MoveTo if not at target. Re-queue Survey after it.
        if docked_system != Some(req.target_system) {
            queue.commands.insert(
                0,
                QueuedCommand::Survey {
                    system: req.target_system,
                },
            );
            queue.commands.insert(
                0,
                QueuedCommand::MoveTo {
                    system: req.target_system,
                },
            );
            info!(
                "Queue: Ship {} not at target, auto-inserting move before survey of {}",
                ship.name, target_star.name
            );
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::Survey,
                ship: req.ship,
                result: CommandResult::Deferred,
                completed_at: clock.elapsed,
            });
            continue;
        }

        let origin = Position::from(ship_pos.as_array());
        match start_survey_with_bonus(
            &mut state,
            ship,
            req.target_system,
            &origin,
            target_pos,
            clock.elapsed,
            global_params.survey_range_bonus,
            &design_registry,
            survey_range_base,
            survey_duration_base,
        ) {
            Ok(()) => {
                info!("Queue: Ship {} surveying {}", ship.name, target_star.name);
                executed.write(CommandExecuted {
                    command_id: req.command_id,
                    kind: CommandKind::Survey,
                    ship: req.ship,
                    result: CommandResult::Ok,
                    completed_at: clock.elapsed,
                });
            }
            Err(e) => {
                warn!("Queue: Survey failed for {}: {}", ship.name, e);
                executed.write(CommandExecuted {
                    command_id: req.command_id,
                    kind: CommandKind::Survey,
                    ship: req.ship,
                    result: CommandResult::Rejected {
                        reason: format!("survey start failed: {}", e),
                    },
                    completed_at: clock.elapsed,
                });
            }
        }
        queue.sync_prediction(ship_pos.as_array(), docked_system);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn handle_colonize_requested(
    clock: Res<GameClock>,
    balance: Res<crate::technology::GameBalance>,
    mut reqs: MessageReader<ColonizeRequested>,
    mut executed: MessageWriter<CommandExecuted>,
    mut ships: Query<(&Ship, &mut ShipState, &Position, &mut CommandQueue)>,
    systems: Query<(&crate::galaxy::StarSystem, &Position), Without<Ship>>,
    design_registry: Res<ShipDesignRegistry>,
) {
    let settling_duration = balance.settling_duration();

    for req in reqs.read() {
        let Ok((ship, mut state, ship_pos, mut queue)) = ships.get_mut(req.ship) else {
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::Colonize,
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
                    kind: CommandKind::Colonize,
                    ship: req.ship,
                    result: CommandResult::Rejected {
                        reason: "ship not idle".to_string(),
                    },
                    completed_at: clock.elapsed,
                });
                continue;
            }
        };

        let Ok((target_star, _target_pos)) = systems.get(req.target_system) else {
            warn!("Queued Colonize target no longer exists");
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::Colonize,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "target system despawned".to_string(),
                },
                completed_at: clock.elapsed,
            });
            continue;
        };

        if docked_system != Some(req.target_system) {
            queue.commands.insert(
                0,
                QueuedCommand::Colonize {
                    system: req.target_system,
                    planet: req.planet,
                },
            );
            queue.commands.insert(
                0,
                QueuedCommand::MoveTo {
                    system: req.target_system,
                },
            );
            info!(
                "Queue: Ship {} not at target, auto-inserting move before colonize of {}",
                ship.name, target_star.name
            );
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::Colonize,
                ship: req.ship,
                result: CommandResult::Deferred,
                completed_at: clock.elapsed,
            });
            continue;
        }

        if !design_registry.can_colonize(&ship.design_id) {
            warn!(
                "Queue: Ship {} cannot colonize (not a colony ship)",
                ship.name
            );
            executed.write(CommandExecuted {
                command_id: req.command_id,
                kind: CommandKind::Colonize,
                ship: req.ship,
                result: CommandResult::Rejected {
                    reason: "not a colony ship".to_string(),
                },
                completed_at: clock.elapsed,
            });
            queue.sync_prediction(ship_pos.as_array(), docked_system);
            continue;
        }

        let docked_sys = docked_system.expect("colonize already required docked");
        *state = ShipState::Settling {
            system: docked_sys,
            planet: req.planet,
            started_at: clock.elapsed,
            completes_at: clock.elapsed + settling_duration,
        };
        info!("Queue: Ship {} colonizing {}", ship.name, target_star.name);
        executed.write(CommandExecuted {
            command_id: req.command_id,
            kind: CommandKind::Colonize,
            ship: req.ship,
            result: CommandResult::Ok,
            completed_at: clock.elapsed,
        });
        queue.sync_prediction(ship_pos.as_array(), docked_system);
    }
}
