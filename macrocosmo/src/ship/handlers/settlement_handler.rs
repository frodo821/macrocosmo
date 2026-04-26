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

use crate::ai::assignments::PendingAssignment;
use crate::faction::FactionOwner;
use crate::galaxy::AtSystem;
use crate::ship::command_events::{
    ColonizeRequested, CommandExecuted, CommandKind, CommandResult, SurveyRequested,
};
use crate::ship::core_deliverable::CoreShip;
use crate::ship::survey::start_survey_with_bonus;
use crate::ship::{CommandQueue, Owner, QueuedCommand, Ship, ShipState};

#[allow(clippy::too_many_arguments)]
pub fn handle_survey_requested(
    mut commands_buf: Commands,
    clock: Res<GameClock>,
    balance: Res<crate::technology::GameBalance>,
    empire_params_q: Query<&crate::technology::GlobalParams, With<crate::player::Empire>>,
    mut reqs: MessageReader<SurveyRequested>,
    mut executed: MessageWriter<CommandExecuted>,
    mut ships: Query<(&Ship, &mut ShipState, &Position, &mut CommandQueue)>,
    systems: Query<(&crate::galaxy::StarSystem, &Position), Without<Ship>>,
    design_registry: Res<ShipDesignRegistry>,
) {
    let survey_range_base = balance.survey_range_ly();
    let survey_duration_base = balance.survey_duration();
    let default_params = crate::technology::GlobalParams::default();

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
            // Round 9 PR #2 Step 4: ship unavailable means despawned or
            // missing required components — Bevy already drops the
            // `PendingAssignment` component with the entity; nothing to do.
            // The sweeper handles the rare "ship still exists but unqueryable"
            // case via `stale_at`.
            continue;
        };

        let global_params = match ship.owner {
            Owner::Empire(e) => empire_params_q.get(e).unwrap_or(&default_params),
            Owner::Neutral => &default_params,
        };

        let docked_system: Option<Entity> = match *state {
            ShipState::InSystem { system } => Some(system),
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
                // Round 9 PR #2 Step 4: terminal Rejected — clear marker so
                // the next AI tick can re-evaluate this ship.
                commands_buf.entity(req.ship).remove::<PendingAssignment>();
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
            // Round 9 PR #2 Step 4: terminal Rejected — clear marker.
            commands_buf.entity(req.ship).remove::<PendingAssignment>();
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
                // Round 9 PR #2 follow-up: do NOT remove `PendingAssignment`
                // here. The marker is the NPC's *decision memory* — it must
                // outlive the dispatch and stay attached until the issuing
                // empire's `KnowledgeStore` reflects the survey completion
                // (success path) or the ship is known lost (failure path).
                // The knowledge-driven `sweep_resolved_survey_assignments`
                // system handles both cases; `sweep_stale_assignments`
                // catches anything pathological via `stale_at`.
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
                // Round 9 PR #2 Step 4: terminal Rejected — clear marker.
                commands_buf.entity(req.ship).remove::<PendingAssignment>();
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
    mut ships: Query<(
        &Ship,
        &mut ShipState,
        &Position,
        &mut CommandQueue,
        Option<&FactionOwner>,
    )>,
    systems: Query<(&crate::galaxy::StarSystem, &Position), Without<Ship>>,
    design_registry: Res<ShipDesignRegistry>,
    cores: Query<(&AtSystem, &FactionOwner), With<CoreShip>>,
) {
    let settling_duration = balance.settling_duration();

    for req in reqs.read() {
        let Ok((ship, mut state, ship_pos, mut queue, ship_faction)) = ships.get_mut(req.ship)
        else {
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

        // #299 (S-5): Resolve the ship's faction for the Core ownership check.
        let ship_faction_entity: Option<Entity> =
            ship_faction.map(|fo| fo.0).or_else(|| match ship.owner {
                Owner::Empire(e) => Some(e),
                Owner::Neutral => None,
            });

        let docked_system: Option<Entity> = match *state {
            ShipState::InSystem { system } => Some(system),
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

        // #299 (S-5): Settle gate — require a Core ship owned by this
        // faction in the target system. Without sovereignty presence,
        // colonization is blocked. Neutral ships (no faction) bypass the
        // gate for backward compatibility with pre-faction test setups.
        if let Some(faction) = ship_faction_entity {
            let has_own_core = cores
                .iter()
                .any(|(at, fo)| at.0 == req.target_system && fo.0 == faction);
            if !has_own_core {
                warn!(
                    "Queue: Ship {} cannot colonize {} — no sovereignty core in target system",
                    ship.name, target_star.name
                );
                executed.write(CommandExecuted {
                    command_id: req.command_id,
                    kind: CommandKind::Colonize,
                    ship: req.ship,
                    result: CommandResult::Rejected {
                        reason: "no sovereignty core in target system".to_string(),
                    },
                    completed_at: clock.elapsed,
                });
                queue.sync_prediction(ship_pos.as_array(), docked_system);
                continue;
            }
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
