//! Command drain consumer — converts AI bus commands into ECS game actions.
//!
//! Registered under [`AiTickSet::CommandDrain`](super::AiTickSet::CommandDrain).
//! Each tick, drains pending commands from the bus and applies them:
//!
//! - `attack_target` → find idle ships owned by the issuing faction, queue
//!   `MoveTo` for the target system.
//! - `retreat` → find ships in hostile systems, queue `MoveTo` back to
//!   the faction's home system (system with most colonies).
//! - `fortify_system`, `reposition`, `blockade` → log only (Phase 1).

use bevy::prelude::*;

use macrocosmo_ai::CommandValue;

use crate::ai::convert::{from_ai_system, to_ai_faction};
use crate::ai::emit::AiBusDrainer;
use crate::ai::schema::ids::command as cmd_ids;
use crate::components::Position;
use crate::galaxy::{AtSystem, Hostile, Sovereignty, StarSystem};
use crate::player::{Empire, Faction, PlayerEmpire};
use crate::ship::{CommandQueue, Owner, QueuedCommand, Ship, ShipState};

/// Drain AI commands from the bus and apply them to the game world.
///
/// Phase 1 implementation:
/// - `attack_target`: dispatches idle ships to the target system
/// - `retreat`: sends ships in hostile systems back home
/// - Other commands: logged but not acted on
pub fn drain_ai_commands(
    mut drainer: AiBusDrainer,
    mut ships: Query<(Entity, &Ship, &ShipState, &mut CommandQueue)>,
    sovereignty: Query<(Entity, &Sovereignty), With<StarSystem>>,
    hostiles: Query<&AtSystem, With<Hostile>>,
    positions: Query<&Position, With<StarSystem>>,
    npcs: Query<(Entity, &Faction), (With<Empire>, Without<PlayerEmpire>)>,
) {
    let commands = drainer.drain_commands();
    if commands.is_empty() {
        return;
    }

    for cmd in commands {
        let kind_str = cmd.kind.as_str();

        if kind_str == cmd_ids::attack_target().as_str() {
            handle_attack_target(&cmd.issuer, &cmd.params, &mut ships, &positions, &npcs);
        } else if kind_str == cmd_ids::retreat().as_str() {
            handle_retreat(
                &cmd.issuer,
                &mut ships,
                &hostiles,
                &sovereignty,
                &positions,
                &npcs,
            );
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
    npcs: &Query<(Entity, &Faction), (With<Empire>, Without<PlayerEmpire>)>,
) -> Option<Entity> {
    for (entity, _faction) in npcs {
        if to_ai_faction(entity) == *issuer {
            return Some(entity);
        }
    }
    None
}

/// Handle `attack_target`: find idle ships owned by the faction and queue
/// MoveTo for the target system.
fn handle_attack_target(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    ships: &mut Query<(Entity, &Ship, &ShipState, &mut CommandQueue)>,
    positions: &Query<&Position, With<StarSystem>>,
    npcs: &Query<(Entity, &Faction), (With<Empire>, Without<PlayerEmpire>)>,
) {
    // Extract target_system from params
    let target_system = match params.get("target_system") {
        Some(CommandValue::System(sys_ref)) => from_ai_system(*sys_ref),
        _ => {
            warn!("attack_target command missing target_system param");
            return;
        }
    };

    let empire_entity = match find_empire_entity(issuer, npcs) {
        Some(e) => e,
        None => {
            warn!("attack_target: no empire found for faction {:?}", issuer);
            return;
        }
    };

    // Build a closure for position lookup (needed by CommandQueue::push)
    let pos_lookup =
        |sys: Entity| -> Option<[f64; 3]> { positions.get(sys).ok().map(|p| p.as_array()) };

    let mut dispatched = 0;
    for (_ship_entity, ship, state, mut queue) in ships.iter_mut() {
        // Only dispatch ships owned by this faction
        if ship.owner != Owner::Empire(empire_entity) {
            continue;
        }
        // Only dispatch idle ships (InSystem, no pending commands)
        if !matches!(state, ShipState::InSystem { .. }) || !queue.commands.is_empty() {
            continue;
        }
        // Skip immobile ships
        if ship.is_immobile() {
            continue;
        }

        // Don't send ships already at the target
        if let ShipState::InSystem { system } = state {
            if *system == target_system {
                continue;
            }
        }

        queue.push(
            QueuedCommand::MoveTo {
                system: target_system,
            },
            &pos_lookup,
        );
        dispatched += 1;
    }

    if dispatched > 0 {
        info!(
            "attack_target: dispatched {} ships from faction {:?} to system {:?}",
            dispatched, issuer, target_system
        );
    }
}

/// Handle `retreat`: find ships in systems with hostiles and send them
/// back to the faction's home system (system with most colonies).
fn handle_retreat(
    issuer: &macrocosmo_ai::FactionId,
    ships: &mut Query<(Entity, &Ship, &ShipState, &mut CommandQueue)>,
    hostiles: &Query<&AtSystem, With<Hostile>>,
    sovereignty: &Query<(Entity, &Sovereignty), With<StarSystem>>,
    positions: &Query<&Position, With<StarSystem>>,
    npcs: &Query<(Entity, &Faction), (With<Empire>, Without<PlayerEmpire>)>,
) {
    let empire_entity = match find_empire_entity(issuer, npcs) {
        Some(e) => e,
        None => return,
    };

    // Find home system: system owned by this faction with sovereignty
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

    // Collect systems with hostiles
    let hostile_systems: std::collections::HashSet<Entity> =
        hostiles.iter().map(|at| at.0).collect();

    let pos_lookup =
        |sys: Entity| -> Option<[f64; 3]> { positions.get(sys).ok().map(|p| p.as_array()) };

    let mut retreated = 0;
    for (_ship_entity, ship, state, mut queue) in ships.iter_mut() {
        if ship.owner != Owner::Empire(empire_entity) {
            continue;
        }
        if ship.is_immobile() {
            continue;
        }

        // Only retreat ships in hostile systems
        if let ShipState::InSystem { system } = state {
            if hostile_systems.contains(system) && queue.commands.is_empty() && *system != home {
                queue.push(QueuedCommand::MoveTo { system: home }, &pos_lookup);
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
    use macrocosmo_ai::{Command, FactionId, WarningMode};

    /// Minimal app with AI bus and clock for command consumer tests.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GameClock::new(10));
        app.insert_resource(GameSpeed::default());
        app.insert_resource(AiBusResource::with_warning_mode(WarningMode::Silent));
        app.add_systems(Startup, schema::declare_all);
        app.update();
        app
    }

    #[test]
    fn attack_target_dispatches_idle_ships() {
        let mut app = test_app();
        let world = app.world_mut();

        // Spawn NPC empire
        let empire_entity = world
            .spawn((
                Empire {
                    name: "Test NPC".into(),
                },
                Faction::new("test_npc", "Test NPC"),
            ))
            .id();

        let faction_id = to_ai_faction(empire_entity);

        // Spawn two star systems
        let origin_sys = world
            .spawn((
                StarSystem {
                    name: "Origin".into(),
                    is_capital: false,
                    surveyed: false,
                    star_type: "yellow_dwarf".into(),
                },
                Position {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
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
                Position {
                    x: 10.0,
                    y: 0.0,
                    z: 0.0,
                },
            ))
            .id();

        // Spawn an idle ship at origin
        let _ship_entity = world
            .spawn((
                Ship {
                    name: "NPC Scout".into(),
                    design_id: "scout".into(),
                    hull_id: "corvette".into(),
                    modules: vec![],
                    owner: Owner::Empire(empire_entity),
                    sublight_speed: 0.1,
                    ftl_range: 5.0,
                    player_aboard: false,
                    home_port: origin_sys,
                    design_revision: 0,
                    fleet: None,
                },
                ShipState::InSystem { system: origin_sys },
                CommandQueue::default(),
            ))
            .id();

        // Emit attack_target command on the bus
        let target_ref = crate::ai::convert::to_ai_system(target_sys);
        let cmd = Command::new(cmd_ids::attack_target(), faction_id, 10)
            .with_param("target_system", CommandValue::System(target_ref));
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        // Run the drain system
        app.add_systems(Update, drain_ai_commands);
        app.update();

        // Verify ship has a queued MoveTo command
        let world = app.world_mut();
        let mut ship_query = world.query::<&CommandQueue>();
        let queue = ship_query.iter(world).next().expect("ship should exist");
        assert_eq!(queue.commands.len(), 1, "ship should have 1 queued command");
        match &queue.commands[0] {
            QueuedCommand::MoveTo { system } => {
                assert_eq!(*system, target_sys, "should move to target system");
            }
            other => panic!("expected MoveTo, got {:?}", other),
        }
    }

    #[test]
    fn attack_target_skips_ships_not_owned_by_faction() {
        let mut app = test_app();
        let world = app.world_mut();

        // Spawn two NPC empires
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
                Position {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
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
                Position {
                    x: 10.0,
                    y: 0.0,
                    z: 0.0,
                },
            ))
            .id();

        // Ship owned by empire_b — should NOT be dispatched by empire_a's command
        world.spawn((
            Ship {
                name: "B's Ship".into(),
                design_id: "scout".into(),
                hull_id: "corvette".into(),
                modules: vec![],
                owner: Owner::Empire(empire_b),
                sublight_speed: 0.1,
                ftl_range: 5.0,
                player_aboard: false,
                home_port: origin,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system: origin },
            CommandQueue::default(),
        ));

        // Emit command from empire_a
        let target_ref = crate::ai::convert::to_ai_system(target);
        let cmd = Command::new(cmd_ids::attack_target(), faction_a, 10)
            .with_param("target_system", CommandValue::System(target_ref));
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        app.update();

        // Empire B's ship should NOT have any commands
        let world = app.world_mut();
        let mut ship_query = world.query::<&CommandQueue>();
        let queue = ship_query.iter(world).next().unwrap();
        assert!(
            queue.commands.is_empty(),
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

        // Emit attack_target with NO target_system param
        let cmd = Command::new(cmd_ids::attack_target(), faction_id, 10);
        world.resource_mut::<AiBusResource>().0.emit_command(cmd);

        app.add_systems(Update, drain_ai_commands);
        // Should not panic
        app.update();
    }
}
