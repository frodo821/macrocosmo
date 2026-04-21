//! Command drain consumer — converts AI bus commands into ECS game actions.
//!
//! Registered under [`AiTickSet::CommandDrain`](super::AiTickSet::CommandDrain).
//! Each tick, drains pending commands from the bus and applies them:
//!
//! - `attack_target` → find idle ships owned by the issuing faction, emit
//!   `MoveRequested` for the target system.
//! - `retreat` → find ships in hostile systems, emit `MoveRequested` back to
//!   the faction's home system (system with most colonies).
//! - `fortify_system`, `reposition`, `blockade` → log only (Phase 1).

use bevy::prelude::*;

use macrocosmo_ai::CommandValue;

use crate::ai::convert::{from_ai_system, to_ai_faction};
use crate::ai::emit::AiBusDrainer;
use crate::ai::schema::ids::command as cmd_ids;
use crate::galaxy::{AtSystem, Hostile, Sovereignty, StarSystem};
use crate::player::{Empire, Faction};
use crate::ship::command_events::{
    ColonizeRequested, CommandId, MoveRequested, NextCommandId, SurveyRequested,
};
use crate::ship::{CommandQueue, Owner, Ship, ShipState};
use crate::time_system::GameClock;

/// Drain AI commands from the bus and apply them to the game world.
pub fn drain_ai_commands(
    mut drainer: AiBusDrainer,
    ships: Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    sovereignty: Query<(Entity, &Sovereignty), With<StarSystem>>,
    hostiles: Query<&AtSystem, With<Hostile>>,
    empires: Query<(Entity, &Faction), With<Empire>>,
    mut move_writer: MessageWriter<MoveRequested>,
    mut survey_writer: Option<MessageWriter<SurveyRequested>>,
    mut colonize_writer: Option<MessageWriter<ColonizeRequested>>,
    mut next_cmd_id: ResMut<NextCommandId>,
    clock: Res<GameClock>,
) {
    let commands = drainer.drain_commands();
    if commands.is_empty() {
        return;
    }

    for cmd in commands {
        let kind_str = cmd.kind.as_str();

        if kind_str == cmd_ids::attack_target().as_str() {
            handle_attack_target(
                &cmd.issuer,
                &cmd.params,
                &ships,
                &empires,
                &mut move_writer,
                &mut next_cmd_id,
                clock.elapsed,
            );
        } else if kind_str == cmd_ids::retreat().as_str() {
            handle_retreat(
                &cmd.issuer,
                &ships,
                &hostiles,
                &sovereignty,
                &empires,
                &mut move_writer,
                &mut next_cmd_id,
                clock.elapsed,
            );
        } else if kind_str == cmd_ids::survey_system().as_str() {
            if let Some(ref mut w) = survey_writer {
                handle_survey_system(
                    &cmd.issuer,
                    &cmd.params,
                    &ships,
                    &empires,
                    w,
                    &mut next_cmd_id,
                    clock.elapsed,
                );
            }
        } else if kind_str == cmd_ids::colonize_system().as_str() {
            if let Some(ref mut w) = colonize_writer {
                handle_colonize_system(
                    &cmd.issuer,
                    &cmd.params,
                    &ships,
                    &empires,
                    w,
                    &mut next_cmd_id,
                    clock.elapsed,
                );
            }
        } else if kind_str == cmd_ids::fortify_system().as_str() {
            info!(
                "AI command fortify_system from faction {:?} (Phase 1: log only)",
                cmd.issuer
            );
        } else if kind_str == cmd_ids::reposition().as_str() {
            info!(
                "AI command reposition from faction {:?} (Phase 1: log only)",
                cmd.issuer
            );
        } else if kind_str == cmd_ids::blockade().as_str() {
            info!(
                "AI command blockade from faction {:?} (Phase 1: log only)",
                cmd.issuer
            );
        } else {
            debug!(
                "AI command '{}' from faction {:?} not handled by drain_ai_commands",
                kind_str, cmd.issuer
            );
        }
    }
}

/// Find the empire entity for a given AI FactionId.
fn find_empire_entity(
    issuer: &macrocosmo_ai::FactionId,
    empires: &Query<(Entity, &Faction), With<Empire>>,
) -> Option<Entity> {
    for (entity, _faction) in empires {
        if to_ai_faction(entity) == *issuer {
            return Some(entity);
        }
    }
    None
}

/// Extract ship entity list from indexed command params (`ship_count`,
/// `ship_0`, `ship_1`, ...).
fn extract_ship_list(params: &macrocosmo_ai::CommandParams) -> Vec<Entity> {
    let count = match params.get("ship_count") {
        Some(CommandValue::I64(n)) => *n as usize,
        _ => return vec![],
    };
    (0..count)
        .filter_map(|i| {
            let key = format!("ship_{i}");
            match params.get(key.as_str()) {
                Some(CommandValue::Entity(r)) => {
                    Some(crate::ai::convert::from_ai_entity(*r))
                }
                _ => None,
            }
        })
        .collect()
}

/// Handle `attack_target`: dispatch the ships specified by the AI policy
/// to the target system. The policy is responsible for ship selection —
/// the consumer only validates that each ship is still eligible.
fn handle_attack_target(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    move_writer: &mut MessageWriter<MoveRequested>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    let target_system = match params.get("target_system") {
        Some(CommandValue::System(sys_ref)) => from_ai_system(*sys_ref),
        _ => {
            warn!("attack_target command missing target_system param");
            return;
        }
    };

    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => {
            warn!("attack_target: no empire found for faction {:?}", issuer);
            return;
        }
    };

    let selected_ships = extract_ship_list(params);
    if selected_ships.is_empty() {
        debug!(
            "attack_target: no ships specified by policy for faction {:?}",
            issuer
        );
        return;
    }

    let mut dispatched = 0;
    for ship_entity in selected_ships {
        let Ok((_, ship, state, queue)) = ships.get(ship_entity) else {
            continue; // Ship despawned since policy decided
        };
        if ship.owner != Owner::Empire(empire_entity) {
            continue;
        }
        if !matches!(state, ShipState::InSystem { .. }) || !queue.commands.is_empty() {
            continue;
        }
        if let ShipState::InSystem { system } = state {
            if *system == target_system {
                continue;
            }
        }

        move_writer.write(MoveRequested {
            command_id: next_cmd_id.allocate(),
            ship: ship_entity,
            target: target_system,
            issued_at: now,
        });
        dispatched += 1;
    }

    if dispatched > 0 {
        info!(
            "attack_target: dispatched {} ships from faction {:?} to system {:?}",
            dispatched, issuer, target_system
        );
    }
}

/// Handle `survey_system`: dispatch the specified survey ship to the target system.
fn handle_survey_system(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    survey_writer: &mut MessageWriter<SurveyRequested>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    let target_system = match params.get("target_system") {
        Some(CommandValue::System(sys_ref)) => from_ai_system(*sys_ref),
        _ => {
            warn!("survey_system command missing target_system param");
            return;
        }
    };

    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => return,
    };

    let selected_ships = extract_ship_list(params);
    let mut dispatched = 0;
    for ship_entity in selected_ships {
        let Ok((_, ship, state, queue)) = ships.get(ship_entity) else {
            continue;
        };
        if ship.owner != Owner::Empire(empire_entity) {
            continue;
        }
        if !matches!(state, ShipState::InSystem { .. }) || !queue.commands.is_empty() {
            continue;
        }

        survey_writer.write(SurveyRequested {
            command_id: next_cmd_id.allocate(),
            ship: ship_entity,
            target_system,
            issued_at: now,
        });
        dispatched += 1;
    }

    if dispatched > 0 {
        info!(
            "survey_system: dispatched {} ships from faction {:?} to system {:?}",
            dispatched, issuer, target_system
        );
    }
}

/// Handle `colonize_system`: dispatch the specified colony ship to the target system.
fn handle_colonize_system(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    colonize_writer: &mut MessageWriter<ColonizeRequested>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    let target_system = match params.get("target_system") {
        Some(CommandValue::System(sys_ref)) => from_ai_system(*sys_ref),
        _ => {
            warn!("colonize_system command missing target_system param");
            return;
        }
    };

    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => return,
    };

    let selected_ships = extract_ship_list(params);
    let mut dispatched = 0;
    for ship_entity in selected_ships {
        let Ok((_, ship, state, queue)) = ships.get(ship_entity) else {
            continue;
        };
        if ship.owner != Owner::Empire(empire_entity) {
            continue;
        }
        if !matches!(state, ShipState::InSystem { .. }) || !queue.commands.is_empty() {
            continue;
        }

        colonize_writer.write(ColonizeRequested {
            command_id: next_cmd_id.allocate(),
            ship: ship_entity,
            target_system,
            planet: None, // Let the handler pick the best planet
            issued_at: now,
        });
        dispatched += 1;
    }

    if dispatched > 0 {
        info!(
            "colonize_system: dispatched {} ships from faction {:?} to system {:?}",
            dispatched, issuer, target_system
        );
    }
}

/// Handle `retreat`: find ships in systems with hostiles and send them
/// back to the faction's home system (system with most colonies).
fn handle_retreat(
    issuer: &macrocosmo_ai::FactionId,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    hostiles: &Query<&AtSystem, With<Hostile>>,
    sovereignty: &Query<(Entity, &Sovereignty), With<StarSystem>>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    move_writer: &mut MessageWriter<MoveRequested>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => return,
    };

    let mut home_system: Option<Entity> = None;
    for (sys_entity, sov) in sovereignty.iter() {
        if sov.owner == Some(Owner::Empire(empire_entity)) {
            home_system = Some(sys_entity);
            break;
        }
    }

    let home = match home_system {
        Some(h) => h,
        None => {
            debug!("retreat: faction {:?} has no home system", issuer);
            return;
        }
    };

    let hostile_systems: std::collections::HashSet<Entity> =
        hostiles.iter().map(|at| at.0).collect();

    let mut retreated = 0;
    for (ship_entity, ship, state, queue) in ships.iter() {
        if ship.owner != Owner::Empire(empire_entity) {
            continue;
        }
        if ship.is_immobile() {
            continue;
        }

        if let ShipState::InSystem { system } = state {
            if hostile_systems.contains(system) && queue.commands.is_empty() && *system != home {
                move_writer.write(MoveRequested {
                    command_id: next_cmd_id.allocate(),
                    ship: ship_entity,
                    target: home,
                    issued_at: now,
                });
                retreated += 1;
            }
        }
    }

    if retreated > 0 {
        info!(
            "retreat: {} ships from faction {:?} retreating to {:?}",
            retreated, issuer, home
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::plugin::AiBusResource;
    use crate::ai::schema;
    use crate::components::Position;
    use crate::time_system::{GameClock, GameSpeed};
    use macrocosmo_ai::{Command, WarningMode};

    #[derive(Resource)]
    struct MoveCount(usize);

    fn count_moves(mut reader: MessageReader<MoveRequested>, mut count: ResMut<MoveCount>) {
        for _msg in reader.read() {
            count.0 += 1;
        }
    }

    /// Minimal app with AI bus and clock for command consumer tests.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GameClock::new(10));
        app.insert_resource(GameSpeed::default());
        app.init_resource::<NextCommandId>();
        app.insert_resource(AiBusResource::with_warning_mode(WarningMode::Silent));
        app.add_message::<MoveRequested>();
        app.add_systems(Startup, schema::declare_all);
        app.update();
        app
    }

    #[test]
    fn attack_target_dispatches_idle_ships() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let faction_id = to_ai_faction(empire_entity);

        let origin_sys = world
            .spawn((
                StarSystem {
                    name: "Origin".into(),
                    is_capital: false,
                    surveyed: false,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
            ))
            .id();

        let target_sys = world
            .spawn((
                StarSystem {
                    name: "Target".into(),
                    is_capital: false,
                    surveyed: false,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([10.0, 0.0, 0.0]),
            ))
            .id();

        let ship_entity = world.spawn((
            Ship {
                name: "NPC Scout".into(),
                design_id: "scout".into(),
                hull_id: "corvette".into(),
                modules: vec![],
                owner: Owner::Empire(empire_entity),
                sublight_speed: 0.1,
                ftl_range: 5.0,
                ruler_aboard: false,
                home_port: origin_sys,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system: origin_sys },
            CommandQueue::default(),
        )).id();

        let target_ref = crate::ai::convert::to_ai_system(target_sys);
        let ship_ref = crate::ai::convert::to_ai_entity(ship_entity);
        let cmd = Command::new(cmd_ids::attack_target(), faction_id, 10)
            .with_param("target_system", CommandValue::System(target_ref))
            .with_param("ship_count", CommandValue::I64(1))
            .with_param("ship_0", CommandValue::Entity(ship_ref));
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        // Add a counting system that reads MoveRequested messages.
        app.insert_resource(MoveCount(0));
        app.add_systems(
            Update,
            (drain_ai_commands, count_moves).chain(),
        );
        app.update();

        let count = app.world().resource::<MoveCount>().0;
        assert_eq!(count, 1, "should emit 1 MoveRequested");
    }

    #[test]
    fn attack_target_skips_ships_not_owned_by_faction() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire_a = world
            .spawn((
                Empire {
                    name: "Empire A".into(),
                },
                Faction::new("empire_a", "Empire A"),
            ))
            .id();

        let empire_b = world
            .spawn((
                Empire {
                    name: "Empire B".into(),
                },
                Faction::new("empire_b", "Empire B"),
            ))
            .id();

        let faction_a = to_ai_faction(empire_a);

        let origin = world
            .spawn((
                StarSystem {
                    name: "Origin".into(),
                    is_capital: false,
                    surveyed: false,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
            ))
            .id();

        let target = world
            .spawn((
                StarSystem {
                    name: "Target".into(),
                    is_capital: false,
                    surveyed: false,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([10.0, 0.0, 0.0]),
            ))
            .id();

        // Ship owned by empire_b
        world.spawn((
            Ship {
                name: "B's Ship".into(),
                design_id: "scout".into(),
                hull_id: "corvette".into(),
                modules: vec![],
                owner: Owner::Empire(empire_b),
                sublight_speed: 0.1,
                ftl_range: 5.0,
                ruler_aboard: false,
                home_port: origin,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system: origin },
            CommandQueue::default(),
        ));

        let target_ref = crate::ai::convert::to_ai_system(target);
        let cmd = Command::new(cmd_ids::attack_target(), faction_a, 10)
            .with_param("target_system", CommandValue::System(target_ref));
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();

        app.insert_resource(MoveCount(0));
        app.add_systems(
            Update,
            (drain_ai_commands, count_moves).chain(),
        );
        app.update();

        let count = app.world().resource::<MoveCount>().0;
        assert_eq!(
            count, 0,
            "empire_b's ship should not be dispatched by empire_a's command"
        );
    }

    #[test]
    fn attack_target_no_crash_with_missing_params() {
        let mut app = test_app();
        let world = app.world_mut();

        let empire = world
            .spawn((
                Empire {
                    name: "Test".into(),
                },
                Faction::new("test", "Test"),
            ))
            .id();

        let faction_id = to_ai_faction(empire);

        let cmd = Command::new(cmd_ids::attack_target(), faction_id, 10);
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();
    }
}
