//! #334: command-queue dispatcher.
//!
//! Iterates every ship's `CommandQueue`, peeks the head command, performs
//! **lightweight validation only** (ship is Docked/Loitering, target exists,
//! ship not immobile), emits the corresponding
//! [`CommandRequested`](super::command_events) message, and pops the
//! queue. Phase 3 completes the migration — every `QueuedCommand`
//! variant now has a matching request arm here and a handler under
//! `super::handlers`. There are no legacy fallthroughs.
//!
//! **No state mutation beyond `CommandQueue::commands.remove(0)` and
//! message emit** — all semantic effects (starting FTL travel, spawning
//! route tasks, flipping `ShipState`) happen in the downstream handler
//! systems that read the message. This keeps the dispatcher's query set
//! tiny (well below Bevy's 16-param cap) and frees each handler to hold
//! only the queries *it* needs (plan §2.2, §2.3).

use bevy::prelude::*;

use super::command_events::{
    ColonizeRequested, CommandId, DeployDeliverableRequested, LoadDeliverableRequested,
    LoadFromScrapyardRequested, MoveRequested, MoveToCoordinatesRequested, NextCommandId,
    ScoutRequested, SurveyRequested, TransferToStructureRequested,
};
use super::routing::PendingRoute;
use super::{CommandQueue, QueuedCommand, Ship, ShipState};
use crate::communication::{CommandLog, CommandLogEntry};
use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::player::PlayerEmpire;
use crate::time_system::GameClock;

/// Lightweight dispatcher: validates + emits `CommandRequested` messages.
///
/// As of #334 Phase 3 every `QueuedCommand` variant is handled here and
/// consumed by a focused handler under `super::handlers`.
#[allow(clippy::too_many_arguments)]
pub fn dispatch_queued_commands(
    clock: Res<GameClock>,
    mut next_id: ResMut<NextCommandId>,
    // Ships not already mid-route. `PendingRoute` means a MoveTo is already
    // being resolved asynchronously; skip those to preserve the 1-in-flight
    // invariant from the legacy code.
    mut ships: Query<
        (Entity, &Ship, &ShipState, &Position, &mut CommandQueue),
        Without<PendingRoute>,
    >,
    // Read-only target lookup. Name not used here but the filter ensures we
    // only match star-system entities.
    systems: Query<Entity, With<StarSystem>>,
    // Typed message writers — one per Phase-1/2 request variant.
    mut move_req: MessageWriter<MoveRequested>,
    mut move_xy_req: MessageWriter<MoveToCoordinatesRequested>,
    mut load_req: MessageWriter<LoadDeliverableRequested>,
    mut deploy_req: MessageWriter<DeployDeliverableRequested>,
    mut transfer_req: MessageWriter<TransferToStructureRequested>,
    mut scrap_req: MessageWriter<LoadFromScrapyardRequested>,
    mut survey_req: MessageWriter<SurveyRequested>,
    mut colonize_req: MessageWriter<ColonizeRequested>,
    mut scout_req: MessageWriter<ScoutRequested>,
    // #334 Phase 1: append a `Dispatched` entry to the player empire's
    // CommandLog on each successful validation. The bridge system
    // `bridge_command_executed_to_log` finalizes via `CommandId` match.
    // Optional — observer-mode apps without a `PlayerEmpire` skip logging.
    mut command_log_q: Query<&mut CommandLog, With<PlayerEmpire>>,
) {
    let mut command_log = command_log_q.single_mut().ok();
    for (ship_entity, ship, state, ship_pos, mut queue) in ships.iter_mut() {
        // Only ships in a state that can accept a new command get dispatched.
        // The legacy code consumed queue items for ships that were Docked or
        // Loitering; mid-travel / mid-survey / mid-settling ships have to
        // finish the current action first. Preserve that exactly.
        let (is_docked, docked_system): (bool, Option<Entity>) = match *state {
            ShipState::InSystem { system } => (true, Some(system)),
            ShipState::Loitering { .. } => (false, None),
            _ => continue,
        };

        if queue.commands.is_empty() {
            continue;
        }

        // Peek the head command. We only mutate the queue if this command
        // is a Phase-1 migrated variant AND passes dispatcher validation.
        match &queue.commands[0] {
            QueuedCommand::MoveTo { system: target } => {
                let target = *target;

                // Target system must still exist.
                if systems.get(target).is_err() {
                    warn!(
                        "dispatch: MoveTo target {:?} no longer exists (ship {})",
                        target, ship.name
                    );
                    queue.commands.remove(0);
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                }

                // Already at target — drop the no-op.
                if is_docked && docked_system == Some(target) {
                    queue.commands.remove(0);
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                }

                // Immobile ships (Cores, etc.) can never satisfy a MoveTo.
                // Drop with info-level log; UI guard should already prevent
                // this but belt-and-braces per plan §3.1.
                if ship.is_immobile() {
                    info!(
                        "dispatch: dropping MoveTo on immobile ship {} (no propulsion)",
                        ship.name
                    );
                    queue.commands.remove(0);
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                }

                // Validation passed → emit and pop.
                let command_id: CommandId = next_id.allocate();
                queue.commands.remove(0);
                move_req.write(MoveRequested {
                    command_id,
                    ship: ship_entity,
                    target,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!("{} → MoveTo {:?}", ship.name, target),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} MoveRequested -> {:?} (cmd {})",
                    ship.name, target, command_id.0
                );
            }
            QueuedCommand::MoveToCoordinates { target } => {
                let target_arr = *target;
                // Immobile ships cannot MoveToCoordinates either.
                if ship.is_immobile() {
                    info!(
                        "dispatch: dropping MoveToCoordinates on immobile ship {}",
                        ship.name
                    );
                    queue.commands.remove(0);
                    queue.sync_prediction(ship_pos.as_array(), docked_system);
                    continue;
                }

                let command_id = next_id.allocate();
                queue.commands.remove(0);
                move_xy_req.write(MoveToCoordinatesRequested {
                    command_id,
                    ship: ship_entity,
                    target: target_arr,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!(
                            "{} → MoveToCoordinates ({:.2},{:.2},{:.2})",
                            ship.name, target_arr[0], target_arr[1], target_arr[2]
                        ),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} MoveToCoordinatesRequested -> ({:.2},{:.2},{:.2}) (cmd {})",
                    ship.name, target_arr[0], target_arr[1], target_arr[2], command_id.0
                );
            }
            QueuedCommand::LoadDeliverable {
                system,
                stockpile_index,
            } => {
                let system = *system;
                let stockpile_index = *stockpile_index;
                let command_id = next_id.allocate();
                queue.commands.remove(0);
                load_req.write(LoadDeliverableRequested {
                    command_id,
                    ship: ship_entity,
                    system,
                    stockpile_index,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!("{} → LoadDeliverable [{}]", ship.name, stockpile_index),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} LoadDeliverableRequested system={:?} idx={} (cmd {})",
                    ship.name, system, stockpile_index, command_id.0
                );
            }
            QueuedCommand::DeployDeliverable {
                position,
                item_index,
            } => {
                let position = *position;
                let item_index = *item_index;
                let command_id = next_id.allocate();
                queue.commands.remove(0);
                deploy_req.write(DeployDeliverableRequested {
                    command_id,
                    ship: ship_entity,
                    position,
                    item_index,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!(
                            "{} → DeployDeliverable [{}] at ({:.2},{:.2},{:.2})",
                            ship.name, item_index, position[0], position[1], position[2]
                        ),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} DeployDeliverableRequested idx={} (cmd {})",
                    ship.name, item_index, command_id.0
                );
            }
            QueuedCommand::TransferToStructure {
                structure,
                minerals,
                energy,
            } => {
                let structure = *structure;
                let minerals = *minerals;
                let energy = *energy;
                let command_id = next_id.allocate();
                queue.commands.remove(0);
                transfer_req.write(TransferToStructureRequested {
                    command_id,
                    ship: ship_entity,
                    structure,
                    minerals,
                    energy,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!(
                            "{} → TransferToStructure {:?} ({}m/{}e)",
                            ship.name,
                            structure,
                            minerals.to_f64(),
                            energy.to_f64()
                        ),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} TransferToStructureRequested -> {:?} (cmd {})",
                    ship.name, structure, command_id.0
                );
            }
            QueuedCommand::LoadFromScrapyard { structure } => {
                let structure = *structure;
                let command_id = next_id.allocate();
                queue.commands.remove(0);
                scrap_req.write(LoadFromScrapyardRequested {
                    command_id,
                    ship: ship_entity,
                    structure,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!("{} → LoadFromScrapyard {:?}", ship.name, structure),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} LoadFromScrapyardRequested -> {:?} (cmd {})",
                    ship.name, structure, command_id.0
                );
            }
            QueuedCommand::Survey { system: target } => {
                let target = *target;
                let command_id = next_id.allocate();
                queue.commands.remove(0);
                survey_req.write(SurveyRequested {
                    command_id,
                    ship: ship_entity,
                    target_system: target,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!("{} → Survey {:?}", ship.name, target),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} SurveyRequested -> {:?} (cmd {})",
                    ship.name, target, command_id.0
                );
            }
            QueuedCommand::Colonize {
                system: target,
                planet,
            } => {
                let target = *target;
                let planet = *planet;
                let command_id = next_id.allocate();
                queue.commands.remove(0);
                colonize_req.write(ColonizeRequested {
                    command_id,
                    ship: ship_entity,
                    target_system: target,
                    planet,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!("{} → Colonize {:?}", ship.name, target),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} ColonizeRequested -> {:?} (cmd {})",
                    ship.name, target, command_id.0
                );
            }
            QueuedCommand::Scout {
                target_system,
                observation_duration,
                report_mode,
            } => {
                let target_system = *target_system;
                let observation_duration = *observation_duration;
                let report_mode = *report_mode;
                let command_id = next_id.allocate();
                queue.commands.remove(0);
                scout_req.write(ScoutRequested {
                    command_id,
                    ship: ship_entity,
                    target_system,
                    observation_duration,
                    report_mode,
                    issued_at: clock.elapsed,
                });
                if let Some(log) = command_log.as_mut() {
                    log.entries.push(CommandLogEntry::new_dispatched(
                        format!("{} → Scout {:?}", ship.name, target_system),
                        clock.elapsed,
                        command_id,
                    ));
                }
                info!(
                    "dispatch: ship {} ScoutRequested -> {:?} (cmd {})",
                    ship.name, target_system, command_id.0
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::Amt;
    use crate::ship::command_events::CommandEventsPlugin;
    use crate::ship::{Owner, ShipHitpoints, ShipModifiers, ShipStats};
    use bevy::MinimalPlugins;
    use bevy::ecs::message::Messages;

    fn dummy_home_port(world: &mut World) -> Entity {
        // Spawn a harmless placeholder entity just so `Ship.home_port`
        // references a valid Entity id. The dispatcher never resolves it.
        world.spawn_empty().id()
    }

    fn spawn_test_ship(
        world: &mut World,
        pos: [f64; 3],
        docked_system: Option<Entity>,
        sublight_speed: f64,
        ftl_range: f64,
    ) -> Entity {
        let home_port = dummy_home_port(world);
        let state = match docked_system {
            Some(system) => ShipState::InSystem { system },
            None => ShipState::Loitering { position: pos },
        };
        world
            .spawn((
                Ship {
                    name: "T".into(),
                    design_id: "test".into(),
                    hull_id: "hull".into(),
                    modules: vec![],
                    owner: Owner::Neutral,
                    sublight_speed,
                    ftl_range,
                    ruler_aboard: false,
                    home_port,
                    design_revision: 0,
                    fleet: None,
                },
                state,
                Position::from(pos),
                CommandQueue::default(),
                crate::ship::Cargo::default(),
                ShipHitpoints {
                    hull: 10.0,
                    hull_max: 10.0,
                    armor: 0.0,
                    armor_max: 0.0,
                    shield: 0.0,
                    shield_max: 0.0,
                    shield_regen: 0.0,
                },
                ShipModifiers::default(),
                ShipStats::default(),
            ))
            .id()
    }

    fn spawn_test_system(world: &mut World, pos: [f64; 3]) -> Entity {
        world
            .spawn((
                StarSystem {
                    name: "S".into(),
                    surveyed: true,
                    is_capital: false,
                    star_type: "g2v".into(),
                },
                Position::from(pos),
            ))
            .id()
    }

    fn make_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GameClock::new(0));
        app.add_plugins(CommandEventsPlugin);
        app.add_systems(Update, dispatch_queued_commands);
        app
    }

    #[test]
    fn dispatches_move_to_emits_request_and_pops_queue() {
        let mut app = make_app();
        let target = spawn_test_system(app.world_mut(), [5.0, 0.0, 0.0]);
        let origin = spawn_test_system(app.world_mut(), [0.0, 0.0, 0.0]);
        let ship = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.5, 10.0);
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
            q.commands.push(QueuedCommand::MoveTo { system: target });
        }
        app.update();

        // Message emitted with matching ship + target
        let messages = app.world().resource::<Messages<MoveRequested>>();
        let mut cursor = messages.get_cursor();
        let all: Vec<&MoveRequested> = cursor.read(messages).collect();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].ship, ship);
        assert_eq!(all[0].target, target);
        assert_ne!(all[0].command_id, CommandId::ZERO);

        // Queue is now empty — command popped.
        let q = app.world().get::<CommandQueue>(ship).unwrap();
        assert!(q.commands.is_empty());
    }

    #[test]
    fn dispatcher_rejects_move_to_for_immobile_ship() {
        let mut app = make_app();
        let target = spawn_test_system(app.world_mut(), [5.0, 0.0, 0.0]);
        let origin = spawn_test_system(app.world_mut(), [0.0, 0.0, 0.0]);
        // Immobile: 0 sublight, 0 ftl_range.
        let ship = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.0, 0.0);
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
            q.commands.push(QueuedCommand::MoveTo { system: target });
        }
        app.update();

        // No message, queue cleared.
        let messages = app.world().resource::<Messages<MoveRequested>>();
        let mut cursor = messages.get_cursor();
        assert_eq!(cursor.read(messages).count(), 0);
        let q = app.world().get::<CommandQueue>(ship).unwrap();
        assert!(q.commands.is_empty());
    }

    #[test]
    fn dispatcher_drops_already_at_target() {
        let mut app = make_app();
        let origin = spawn_test_system(app.world_mut(), [0.0, 0.0, 0.0]);
        let ship = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.5, 10.0);
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
            q.commands.push(QueuedCommand::MoveTo { system: origin });
        }
        app.update();

        // Already at target → queue cleared, no message.
        let messages = app.world().resource::<Messages<MoveRequested>>();
        let mut cursor = messages.get_cursor();
        assert_eq!(cursor.read(messages).count(), 0);
        let q = app.world().get::<CommandQueue>(ship).unwrap();
        assert!(q.commands.is_empty());
    }

    #[test]
    fn dispatcher_drops_nonexistent_target() {
        let mut app = make_app();
        let origin = spawn_test_system(app.world_mut(), [0.0, 0.0, 0.0]);
        let phantom = Entity::from_raw_u32(9999).unwrap();
        let ship = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.5, 10.0);
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
            q.commands.push(QueuedCommand::MoveTo { system: phantom });
        }
        app.update();

        let messages = app.world().resource::<Messages<MoveRequested>>();
        let mut cursor = messages.get_cursor();
        assert_eq!(cursor.read(messages).count(), 0);
        let q = app.world().get::<CommandQueue>(ship).unwrap();
        assert!(q.commands.is_empty());
    }

    #[test]
    fn dispatches_move_to_coordinates_and_pops() {
        let mut app = make_app();
        let origin = spawn_test_system(app.world_mut(), [0.0, 0.0, 0.0]);
        let ship = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.5, 10.0);
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
            q.commands.push(QueuedCommand::MoveToCoordinates {
                target: [3.0, 4.0, 0.0],
            });
        }
        app.update();

        let messages = app
            .world()
            .resource::<Messages<MoveToCoordinatesRequested>>();
        let mut cursor = messages.get_cursor();
        let all: Vec<&MoveToCoordinatesRequested> = cursor.read(messages).collect();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].target, [3.0, 4.0, 0.0]);
        let q = app.world().get::<CommandQueue>(ship).unwrap();
        assert!(q.commands.is_empty());
    }

    #[test]
    fn dispatches_scout_emits_request_and_pops_queue() {
        // #334 Phase 3 (Commit 1): Scout migrated to the handler pipeline.
        // The dispatcher now emits `ScoutRequested` and pops the queue —
        // there are no remaining non-migrated variants.
        use crate::ship::ReportMode;
        let mut app = make_app();
        let target = spawn_test_system(app.world_mut(), [5.0, 0.0, 0.0]);
        let origin = spawn_test_system(app.world_mut(), [0.0, 0.0, 0.0]);
        let ship = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.5, 10.0);
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
            q.commands.push(QueuedCommand::Scout {
                target_system: target,
                observation_duration: 10,
                report_mode: ReportMode::Return,
            });
        }
        app.update();

        let messages = app.world().resource::<Messages<ScoutRequested>>();
        let mut cursor = messages.get_cursor();
        let all: Vec<&ScoutRequested> = cursor.read(messages).collect();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].ship, ship);
        assert_eq!(all[0].target_system, target);
        assert_eq!(all[0].observation_duration, 10);
        assert!(matches!(all[0].report_mode, ReportMode::Return));
        assert_ne!(all[0].command_id, CommandId::ZERO);

        // Queue is now empty — command popped.
        let q = app.world().get::<CommandQueue>(ship).unwrap();
        assert!(q.commands.is_empty());
    }

    #[test]
    fn dispatcher_fifo_across_multiple_ships() {
        // Plan §6: verify per-ship FIFO + cross-ship emit order.
        let mut app = make_app();
        let origin = spawn_test_system(app.world_mut(), [0.0, 0.0, 0.0]);
        let t1 = spawn_test_system(app.world_mut(), [5.0, 0.0, 0.0]);
        let t2 = spawn_test_system(app.world_mut(), [6.0, 0.0, 0.0]);
        let ship_a = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.5, 10.0);
        let ship_b = spawn_test_ship(app.world_mut(), [0.0, 0.0, 0.0], Some(origin), 0.5, 10.0);
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship_a).unwrap();
            q.commands.push(QueuedCommand::MoveTo { system: t1 });
        }
        {
            let mut q = app.world_mut().get_mut::<CommandQueue>(ship_b).unwrap();
            q.commands.push(QueuedCommand::MoveTo { system: t2 });
        }
        app.update();

        let messages = app.world().resource::<Messages<MoveRequested>>();
        let mut cursor = messages.get_cursor();
        let all: Vec<&MoveRequested> = cursor.read(messages).collect();
        assert_eq!(all.len(), 2);
        // Command ids must be strictly monotonic.
        assert!(all[0].command_id < all[1].command_id);
    }
}
