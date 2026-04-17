//! #296 (S-3): Integration tests for the Infrastructure Core Deliverable
//! lifecycle — Lua definitions round-trip into the registries, deliver →
//! spawn pipeline produces a `CoreShip`, sovereignty derivation follows it,
//! and save/load preserves the marker.

mod common;

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use common::{advance_time, empire_entity, full_test_app, spawn_test_system};
use macrocosmo::amount::Amt;
use macrocosmo::components::Position;
use macrocosmo::deep_space::StructureRegistry;
use macrocosmo::faction::FactionOwner;
use macrocosmo::galaxy::{
    AtSystem, INNER_ORBIT_OFFSET_LY, Sovereignty, system_inner_orbit_position,
};
use macrocosmo::ship::command_events::{CommandId, CoreDeployRequested};
use macrocosmo::ship::core_deliverable::handle_core_deploy_requested;
use macrocosmo::ship::{CoreShip, Owner};
use macrocosmo::ship_design::ShipDesignRegistry;

#[test]
fn lua_loads_infrastructure_core_deliverable_and_design() {
    use macrocosmo::scripting::ScriptEngine;
    use macrocosmo::scripting::structure_api::parse_structure_definitions;

    let engine = ScriptEngine::new().expect("ScriptEngine::new");
    let init = engine.scripts_dir().join("init.lua");
    engine.load_file(&init).expect("load init.lua");

    let defs = parse_structure_definitions(engine.lua()).expect("parse structures");
    let core = defs
        .iter()
        .find(|d| d.id == "infrastructure_core")
        .expect("infrastructure_core should parse");
    let meta = core
        .deliverable
        .as_ref()
        .expect("core is shipyard-buildable");
    assert_eq!(
        meta.spawns_as_ship.as_deref(),
        Some("infrastructure_core_v1"),
        "spawns_as_ship must point at the immobile design"
    );
}

#[test]
fn spawn_core_helper_attaches_core_ship_and_at_system() {
    // No Lua — use the direct helper + a design registry containing the
    // immobile infrastructure_core_v1. This exercises the spawn path in
    // isolation from the Lua pipeline.
    let mut app = full_test_app();
    let sys = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());

    // Inject a minimal immobile design into the ShipDesignRegistry.
    {
        let mut reg = app.world_mut().resource_mut::<ShipDesignRegistry>();
        reg.insert(macrocosmo::ship_design::ShipDesignDefinition {
            id: "infrastructure_core_v1".to_string(),
            name: "Infrastructure Core".to_string(),
            description: String::new(),
            hull_id: "infrastructure_core_hull".to_string(),
            modules: Vec::new(),
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::units(2),
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 0,
            hp: 400.0,
            sublight_speed: 0.0,
            ftl_range: 0.0,
            revision: 0,
        });
    }

    // Wrap the helper in a one-shot system. The spawned entity id travels
    // back out through an `Arc<Mutex<_>>` since run_system_once can't return
    // arbitrary values directly.
    let entity: std::sync::Arc<std::sync::Mutex<Option<Entity>>> = Default::default();
    let entity_out = entity.clone();
    let owner = Owner::Empire(empire);
    let sys_captured = sys;
    app.world_mut()
        .run_system_once(move |mut cmds: Commands, reg: Res<ShipDesignRegistry>| {
            let e = macrocosmo::ship::spawn_core_ship_from_deliverable(
                &mut cmds,
                "infrastructure_core_v1",
                "Core Alpha".to_string(),
                sys_captured,
                Position::from([0.05, 0.0, 0.0]),
                owner,
                &reg,
            );
            *entity_out.lock().unwrap() = Some(e);
        })
        .expect("run helper");
    // Commands are applied at the next app.update()
    app.update();

    let e = entity.lock().unwrap().expect("core spawned");
    assert!(
        app.world().get::<CoreShip>(e).is_some(),
        "CoreShip marker must be attached"
    );
    let at = app
        .world()
        .get::<AtSystem>(e)
        .expect("AtSystem must be attached");
    assert_eq!(at.0, sys);
    let fo = app
        .world()
        .get::<FactionOwner>(e)
        .expect("empire-owned Core must carry FactionOwner");
    assert_eq!(fo.0, empire);
}

#[test]
fn pending_core_deploys_resolves_single_ticket() {
    let mut app = full_test_app();
    let sys = spawn_test_system(app.world_mut(), "Target", [5.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());

    {
        let mut reg = app.world_mut().resource_mut::<ShipDesignRegistry>();
        reg.insert(macrocosmo::ship_design::ShipDesignDefinition {
            id: "infrastructure_core_v1".to_string(),
            name: "Infrastructure Core".to_string(),
            description: String::new(),
            hull_id: "infrastructure_core_hull".to_string(),
            modules: Vec::new(),
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::units(2),
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 0,
            hp: 400.0,
            sublight_speed: 0.0,
            ftl_range: 0.0,
            revision: 0,
        });
    }

    // Enqueue a CoreDeployRequested message directly.
    let deployer = app.world_mut().spawn_empty().id();
    let pos = system_inner_orbit_position(sys, app.world());
    assert!((pos[0] - (5.0 + INNER_ORBIT_OFFSET_LY)).abs() < 1e-9);
    {
        app.world_mut()
            .init_resource::<macrocosmo::scripting::GameRng>();
        let mut msgs = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<CoreDeployRequested>>();
        msgs.write(CoreDeployRequested {
            command_id: CommandId(101),
            deployer,
            target_system: sys,
            deploy_pos: pos,
            faction_owner: Some(empire),
            owner: Owner::Empire(empire),
            design_id: "infrastructure_core_v1".to_string(),
            submitted_at: 0,
        });
    }

    // Run the handler and apply commands.
    app.world_mut()
        .run_system_once(handle_core_deploy_requested)
        .expect("run core handler");
    app.update();

    // Exactly one CoreShip should now exist in sys.
    let mut q = app.world_mut().query::<(Entity, &AtSystem, &CoreShip)>();
    let cores: Vec<_> = q.iter(app.world()).collect();
    assert_eq!(cores.len(), 1);
    assert_eq!(cores[0].1.0, sys);
}

#[test]
fn pending_core_deploys_discards_duplicate_on_owned_system() {
    let mut app = full_test_app();
    let sys = spawn_test_system(app.world_mut(), "Held", [0.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());

    {
        let mut reg = app.world_mut().resource_mut::<ShipDesignRegistry>();
        reg.insert(macrocosmo::ship_design::ShipDesignDefinition {
            id: "infrastructure_core_v1".to_string(),
            name: "Infrastructure Core".to_string(),
            description: String::new(),
            hull_id: "infrastructure_core_hull".to_string(),
            modules: Vec::new(),
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::units(2),
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 0,
            hp: 400.0,
            sublight_speed: 0.0,
            ftl_range: 0.0,
            revision: 0,
        });
    }

    // Pre-populate a Core in the system.
    let _pre_existing = app
        .world_mut()
        .spawn((CoreShip, AtSystem(sys), FactionOwner(empire)))
        .id();

    let deployer = app.world_mut().spawn_empty().id();
    let pos = [INNER_ORBIT_OFFSET_LY, 0.0, 0.0];
    {
        app.world_mut()
            .init_resource::<macrocosmo::scripting::GameRng>();
        let mut msgs = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<CoreDeployRequested>>();
        msgs.write(CoreDeployRequested {
            command_id: CommandId(102),
            deployer,
            target_system: sys,
            deploy_pos: pos,
            faction_owner: Some(empire),
            owner: Owner::Empire(empire),
            design_id: "infrastructure_core_v1".to_string(),
            submitted_at: 0,
        });
    }

    app.world_mut()
        .run_system_once(handle_core_deploy_requested)
        .expect("run core handler");
    app.update();

    // Still only one Core in sys (the pre-existing).
    let mut q = app.world_mut().query::<(Entity, &AtSystem, &CoreShip)>();
    let cores: Vec<_> = q
        .iter(app.world())
        .filter(|(_, at, _)| at.0 == sys)
        .collect();
    assert_eq!(cores.len(), 1, "duplicate deploy must not spawn a 2nd Core");
}

#[test]
fn pending_core_deploys_same_tick_tie_break_picks_one() {
    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "Contested",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let empire_a = empire_entity(app.world_mut());
    // Mock faction B as a second empire entity.
    let empire_b = app.world_mut().spawn_empty().id();

    {
        let mut reg = app.world_mut().resource_mut::<ShipDesignRegistry>();
        reg.insert(macrocosmo::ship_design::ShipDesignDefinition {
            id: "infrastructure_core_v1".to_string(),
            name: "Infrastructure Core".to_string(),
            description: String::new(),
            hull_id: "infrastructure_core_hull".to_string(),
            modules: Vec::new(),
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::units(2),
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 0,
            hp: 400.0,
            sublight_speed: 0.0,
            ftl_range: 0.0,
            revision: 0,
        });
    }

    let deployer_a = app.world_mut().spawn_empty().id();
    let deployer_b = app.world_mut().spawn_empty().id();
    let pos = [INNER_ORBIT_OFFSET_LY, 0.0, 0.0];
    {
        app.world_mut()
            .init_resource::<macrocosmo::scripting::GameRng>();
        let mut msgs = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<CoreDeployRequested>>();
        for (i, (deployer, faction)) in [(deployer_a, empire_a), (deployer_b, empire_b)]
            .into_iter()
            .enumerate()
        {
            msgs.write(CoreDeployRequested {
                command_id: CommandId(200 + i as u64),
                deployer,
                target_system: sys,
                deploy_pos: pos,
                faction_owner: Some(faction),
                owner: Owner::Empire(faction),
                design_id: "infrastructure_core_v1".to_string(),
                submitted_at: 0,
            });
        }
    }

    app.world_mut()
        .run_system_once(handle_core_deploy_requested)
        .expect("run core handler");
    app.update();

    // Exactly one Core in sys, regardless of which ticket won.
    let mut q = app.world_mut().query::<(Entity, &AtSystem, &CoreShip)>();
    let cores: Vec<_> = q
        .iter(app.world())
        .filter(|(_, at, _)| at.0 == sys)
        .collect();
    assert_eq!(cores.len(), 1);
}

#[test]
fn pending_core_deploys_preserves_across_different_systems() {
    let mut app = full_test_app();
    let sys_a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, false);
    let sys_b = spawn_test_system(app.world_mut(), "B", [10.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());

    {
        let mut reg = app.world_mut().resource_mut::<ShipDesignRegistry>();
        reg.insert(macrocosmo::ship_design::ShipDesignDefinition {
            id: "infrastructure_core_v1".to_string(),
            name: "Infrastructure Core".to_string(),
            description: String::new(),
            hull_id: "infrastructure_core_hull".to_string(),
            modules: Vec::new(),
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::units(2),
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 0,
            hp: 400.0,
            sublight_speed: 0.0,
            ftl_range: 0.0,
            revision: 0,
        });
    }

    let deployer_a = app.world_mut().spawn_empty().id();
    let deployer_b = app.world_mut().spawn_empty().id();
    {
        app.world_mut()
            .init_resource::<macrocosmo::scripting::GameRng>();
        // Collect positions before taking `&mut` on the Messages resource,
        // since `system_inner_orbit_position` borrows `&World`.
        let pos_a = system_inner_orbit_position(sys_a, app.world());
        let pos_b = system_inner_orbit_position(sys_b, app.world());
        let mut msgs = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<CoreDeployRequested>>();
        for (i, (deployer, target, pos)) in [(deployer_a, sys_a, pos_a), (deployer_b, sys_b, pos_b)]
            .into_iter()
            .enumerate()
        {
            msgs.write(CoreDeployRequested {
                command_id: CommandId(300 + i as u64),
                deployer,
                target_system: target,
                deploy_pos: pos,
                faction_owner: Some(empire),
                owner: Owner::Empire(empire),
                design_id: "infrastructure_core_v1".to_string(),
                submitted_at: 0,
            });
        }
    }

    app.world_mut()
        .run_system_once(handle_core_deploy_requested)
        .expect("run core handler");
    app.update();

    let mut q = app.world_mut().query::<(Entity, &AtSystem, &CoreShip)>();
    let cores: Vec<_> = q.iter(app.world()).collect();
    assert_eq!(cores.len(), 2);
    let mut systems: Vec<Entity> = cores.iter().map(|(_, at, _)| at.0).collect();
    systems.sort();
    let mut expected = vec![sys_a, sys_b];
    expected.sort();
    assert_eq!(systems, expected);
}

/// End-to-end: a ship in deep space at a system's inner-orbit position with
/// an `infrastructure_core` cargo item executes `DeployDeliverable` → the
/// deliverable_ops processor enqueues a CoreDeployTicket → resolve_core_deploys
/// spawns a CoreShip in the target system at the canonical inner-orbit
/// coordinate.
/// #296: Once an Infrastructure Core ship is spawned, queueing a `MoveTo`
/// against it must be a no-op — the immobile-gate in process_command_queue
/// drops the command without attempting routing. The Core's state stays
/// `Docked`.
#[test]
fn process_command_queue_drops_movetto_on_immobile_ship() {
    use macrocosmo::ship::{CommandQueue, QueuedCommand};

    let mut app = full_test_app();
    let sys_a = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, false);
    let sys_b = spawn_test_system(app.world_mut(), "Far", [3.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());

    {
        let mut reg = app.world_mut().resource_mut::<ShipDesignRegistry>();
        reg.insert(macrocosmo::ship_design::ShipDesignDefinition {
            id: "infrastructure_core_v1".to_string(),
            name: "Infrastructure Core".to_string(),
            description: String::new(),
            hull_id: "infrastructure_core_hull".to_string(),
            modules: Vec::new(),
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::units(2),
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 0,
            hp: 400.0,
            sublight_speed: 0.0,
            ftl_range: 0.0,
            revision: 0,
        });
    }

    let entity_holder: std::sync::Arc<std::sync::Mutex<Option<Entity>>> = Default::default();
    let entity_out = entity_holder.clone();
    let owner = Owner::Empire(empire);
    let sys_captured = sys_a;
    app.world_mut()
        .run_system_once(move |mut cmds: Commands, reg: Res<ShipDesignRegistry>| {
            let e = macrocosmo::ship::spawn_core_ship_from_deliverable(
                &mut cmds,
                "infrastructure_core_v1",
                "Core".to_string(),
                sys_captured,
                Position::from([INNER_ORBIT_OFFSET_LY, 0.0, 0.0]),
                owner,
                &reg,
            );
            *entity_out.lock().unwrap() = Some(e);
        })
        .expect("run helper");
    app.update();
    let core = entity_holder.lock().unwrap().expect("core spawned");

    // Queue a MoveTo to sys_b on the Core's CommandQueue.
    {
        let mut q = app
            .world_mut()
            .get_mut::<CommandQueue>(core)
            .expect("Core has CommandQueue");
        q.commands.push(QueuedCommand::MoveTo { system: sys_b });
    }

    // Pump the schedule.
    advance_time(&mut app, 1);
    advance_time(&mut app, 1);

    // The command must be dropped — queue back to empty.
    let q = app
        .world()
        .get::<CommandQueue>(core)
        .expect("Core still has CommandQueue");
    assert!(q.commands.is_empty(), "MoveTo dropped on immobile ship");
    // State remains Docked at sys_a.
    let st = app
        .world()
        .get::<macrocosmo::ship::ShipState>(core)
        .expect("ShipState");
    assert!(
        matches!(st, macrocosmo::ship::ShipState::Docked { system } if *system == sys_a),
        "Core must remain Docked at home system"
    );
}

#[test]
fn end_to_end_deploy_spawns_core_at_inner_orbit() {
    use macrocosmo::deep_space::{
        CapabilityParams, DeliverableMetadata, ResourceCost, StructureDefinition, StructureRegistry,
    };
    use macrocosmo::ship::{Cargo, CargoItem, CommandQueue, QueuedCommand, ShipState};

    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "DeployHere",
        [3.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let empire = empire_entity(app.world_mut());

    // Register the immobile ship design.
    {
        let mut reg = app.world_mut().resource_mut::<ShipDesignRegistry>();
        reg.insert(macrocosmo::ship_design::ShipDesignDefinition {
            id: "infrastructure_core_v1".to_string(),
            name: "Infrastructure Core".to_string(),
            description: String::new(),
            hull_id: "infrastructure_core_hull".to_string(),
            modules: Vec::new(),
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::units(2),
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 0,
            hp: 400.0,
            sublight_speed: 0.0,
            ftl_range: 0.0,
            revision: 0,
        });
    }
    // Register the deliverable with `spawns_as_ship`.
    {
        let mut reg = app.world_mut().resource_mut::<StructureRegistry>();
        reg.insert(StructureDefinition {
            id: "infrastructure_core".to_string(),
            name: "Infrastructure Core".to_string(),
            description: String::new(),
            max_hp: 400.0,
            energy_drain: Amt::milli(200),
            capabilities: std::collections::HashMap::<String, CapabilityParams>::new(),
            prerequisites: None,
            deliverable: Some(DeliverableMetadata {
                cost: ResourceCost {
                    minerals: Amt::units(600),
                    energy: Amt::units(400),
                },
                build_time: 120,
                cargo_size: 5,
                scrap_refund: 0.25,
                spawns_as_ship: Some("infrastructure_core_v1".to_string()),
            }),
            upgrade_to: Vec::new(),
            upgrade_from: None,
            on_built: None,
            on_upgraded: None,
        });
    }

    // Spawn a courier ship loitering at the system's inner-orbit position
    // with the Core in cargo and a `DeployDeliverable` queued.
    let inner_orbit = system_inner_orbit_position(sys, app.world());
    let ship_design_id = "courier".to_string();
    {
        let mut reg = app.world_mut().resource_mut::<ShipDesignRegistry>();
        if reg.get(&ship_design_id).is_none() {
            reg.insert(macrocosmo::ship_design::ShipDesignDefinition {
                id: ship_design_id.clone(),
                name: "Courier".to_string(),
                description: String::new(),
                hull_id: "corvette".to_string(),
                modules: Vec::new(),
                can_survey: false,
                can_colonize: false,
                maintenance: Amt::ZERO,
                build_cost_minerals: Amt::ZERO,
                build_cost_energy: Amt::ZERO,
                build_time: 0,
                hp: 50.0,
                sublight_speed: 0.5,
                ftl_range: 10.0,
                revision: 0,
            });
        }
    }

    let courier = {
        let cargo = Cargo {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            items: vec![CargoItem::Deliverable {
                definition_id: "infrastructure_core".to_string(),
            }],
        };
        let queue = CommandQueue {
            commands: vec![QueuedCommand::DeployDeliverable {
                position: inner_orbit,
                item_index: 0,
            }],
            ..Default::default()
        };
        // Loitering at inner_orbit, owned by `empire`.
        app.world_mut()
            .spawn((
                macrocosmo::ship::Ship {
                    name: "Test Courier".to_string(),
                    design_id: ship_design_id.clone(),
                    hull_id: "corvette".to_string(),
                    modules: Vec::new(),
                    owner: Owner::Empire(empire),
                    sublight_speed: 0.5,
                    ftl_range: 10.0,
                    player_aboard: false,
                    home_port: sys,
                    design_revision: 0,
                    fleet: None,
                },
                ShipState::Loitering {
                    position: inner_orbit,
                },
                Position::from(inner_orbit),
                queue,
                cargo,
                macrocosmo::ship::ShipHitpoints {
                    hull: 50.0,
                    hull_max: 50.0,
                    armor: 0.0,
                    armor_max: 0.0,
                    shield: 0.0,
                    shield_max: 0.0,
                    shield_regen: 0.0,
                },
                macrocosmo::ship::ShipModifiers::default(),
                macrocosmo::ship::ShipStats::default(),
                macrocosmo::ship::RulesOfEngagement::default(),
                FactionOwner(empire),
            ))
            .id()
    };

    // Pump one tick — process_deliverable_commands enqueues the ticket;
    // the ShipPlugin would then run resolve_core_deploys. In test_app, both
    // are registered on Update, so a single update should suffice.
    advance_time(&mut app, 1);

    // Tick again so commands from both stages are flushed.
    advance_time(&mut app, 1);

    // Find the Core ship.
    let mut q = app
        .world_mut()
        .query::<(Entity, &CoreShip, &AtSystem, &Position)>();
    let cores: Vec<_> = q
        .iter(app.world())
        .filter(|(_, _, at, _)| at.0 == sys)
        .collect();
    assert_eq!(cores.len(), 1, "exactly one Core spawned");
    let (_, _, _, pos) = cores[0];
    let expected = inner_orbit;
    assert!(
        (pos.x - expected[0]).abs() < 1e-9
            && (pos.y - expected[1]).abs() < 1e-9
            && (pos.z - expected[2]).abs() < 1e-9,
        "Core position must match system_inner_orbit_position"
    );
    // Cargo on the deployer is consumed.
    let cargo = app
        .world()
        .get::<Cargo>(courier)
        .expect("deployer keeps Cargo component");
    assert!(cargo.items.is_empty(), "deployer cargo consumed");
}

/// #296: CoreShip marker survives save/load. Without this the `system_owner`
/// query (`With<CoreShip>`) would silently drop sovereignty after a load.
///
/// The deeper round-trip + `update_sovereignty` re-derive coverage lives in
/// `tests/save_load.rs::test_save_load_sovereignty_derived_cache_regression`
/// (which now spawns its Core ship with the CoreShip marker too).
#[test]
fn savebag_default_omits_core_ship_marker() {
    use macrocosmo::persistence::savebag::SavedComponentBag;
    let bag = SavedComponentBag::default();
    assert!(
        bag.core_ship.is_none(),
        "default bag MUST decode core_ship as None to keep legacy save \
         compatibility (no SAVE_VERSION bump)"
    );
}

#[test]
fn core_deploy_sets_system_sovereignty() {
    // End-to-end: deploy a Core → update_sovereignty writes Some(Empire).
    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "SovHome",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let empire = empire_entity(app.world_mut());

    {
        let mut reg = app.world_mut().resource_mut::<ShipDesignRegistry>();
        reg.insert(macrocosmo::ship_design::ShipDesignDefinition {
            id: "infrastructure_core_v1".to_string(),
            name: "Infrastructure Core".to_string(),
            description: String::new(),
            hull_id: "infrastructure_core_hull".to_string(),
            modules: Vec::new(),
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::units(2),
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 0,
            hp: 400.0,
            sublight_speed: 0.0,
            ftl_range: 0.0,
            revision: 0,
        });
    }

    let deployer = app.world_mut().spawn_empty().id();
    let pos = system_inner_orbit_position(sys, app.world());
    {
        app.world_mut()
            .init_resource::<macrocosmo::scripting::GameRng>();
        let mut msgs = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<CoreDeployRequested>>();
        msgs.write(CoreDeployRequested {
            command_id: CommandId(400),
            deployer,
            target_system: sys,
            deploy_pos: pos,
            faction_owner: Some(empire),
            owner: Owner::Empire(empire),
            design_id: "infrastructure_core_v1".to_string(),
            submitted_at: 0,
        });
    }

    app.world_mut()
        .run_system_once(handle_core_deploy_requested)
        .expect("run core handler");
    app.update();
    // Let update_sovereignty run.
    advance_time(&mut app, 1);

    let sov = app.world().get::<Sovereignty>(sys).expect("Sovereignty");
    assert_eq!(sov.owner, Some(Owner::Empire(empire)));
    assert!(sov.control_score > 0.0);
}

// ---------------------------------------------------------------------------
// #300 (S-6): Defense Fleet auto-composition tests
// ---------------------------------------------------------------------------

use macrocosmo::ship::defense_fleet::{DefenseFleet, join_defense_fleet};
use macrocosmo::ship::{Fleet, FleetMembers, Ship};

/// Helper: insert the immobile design into the registry. Avoids repeating
/// the same block in every test.
fn insert_core_design(app: &mut bevy::prelude::App) {
    let mut reg = app.world_mut().resource_mut::<ShipDesignRegistry>();
    reg.insert(macrocosmo::ship_design::ShipDesignDefinition {
        id: "infrastructure_core_v1".to_string(),
        name: "Infrastructure Core".to_string(),
        description: String::new(),
        hull_id: "infrastructure_core_hull".to_string(),
        modules: Vec::new(),
        can_survey: false,
        can_colonize: false,
        maintenance: Amt::units(2),
        build_cost_minerals: Amt::ZERO,
        build_cost_energy: Amt::ZERO,
        build_time: 0,
        hp: 400.0,
        sublight_speed: 0.0,
        ftl_range: 0.0,
        revision: 0,
    });
}

/// Deploy a Core via the message-driven handler and verify that a Defense
/// Fleet entity is created with the Core as its sole member.
#[test]
fn core_deploy_creates_defense_fleet() {
    let mut app = full_test_app();
    let sys = spawn_test_system(app.world_mut(), "DFHome", [0.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());
    insert_core_design(&mut app);

    let deployer = app.world_mut().spawn_empty().id();
    let pos = system_inner_orbit_position(sys, app.world());
    {
        app.world_mut()
            .init_resource::<macrocosmo::scripting::GameRng>();
        let mut msgs = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<CoreDeployRequested>>();
        msgs.write(CoreDeployRequested {
            command_id: CommandId(500),
            deployer,
            target_system: sys,
            deploy_pos: pos,
            faction_owner: Some(empire),
            owner: Owner::Empire(empire),
            design_id: "infrastructure_core_v1".to_string(),
            submitted_at: 0,
        });
    }

    app.world_mut()
        .run_system_once(handle_core_deploy_requested)
        .expect("run core handler");
    // Two updates: one for Commands from the handler, one for the queued
    // world-closure that creates the Defense Fleet.
    app.update();
    app.update();

    // Find the Core ship.
    let mut core_q = app.world_mut().query::<(Entity, &CoreShip, &Ship)>();
    let cores: Vec<_> = core_q.iter(app.world()).collect();
    assert_eq!(cores.len(), 1, "exactly one Core ship");
    let (core_entity, _, core_ship) = cores[0];

    // The Core's fleet must carry the DefenseFleet marker.
    let fleet_entity = core_ship
        .fleet
        .expect("Core ship must have a fleet backref");
    let df = app
        .world()
        .get::<DefenseFleet>(fleet_entity)
        .expect("fleet must carry DefenseFleet marker");
    assert_eq!(df.system, sys, "DefenseFleet.system must match target");

    // Fleet must have the Core as sole member and flagship.
    let members = app
        .world()
        .get::<FleetMembers>(fleet_entity)
        .expect("fleet must have FleetMembers");
    assert!(members.contains(core_entity));
    assert_eq!(members.len(), 1);
    let fleet = app
        .world()
        .get::<Fleet>(fleet_entity)
        .expect("fleet must have Fleet component");
    assert_eq!(fleet.flagship, Some(core_entity));
}

/// After Defense Fleet creation, the old auto-generated single-ship fleet
/// should be pruned (empty → despawned by `prune_empty_fleets`).
#[test]
fn defense_fleet_old_single_ship_fleet_pruned() {
    use macrocosmo::ship::fleet::prune_empty_fleets;

    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "PruneTest",
        [1.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let empire = empire_entity(app.world_mut());
    insert_core_design(&mut app);

    let deployer = app.world_mut().spawn_empty().id();
    let pos = system_inner_orbit_position(sys, app.world());
    {
        app.world_mut()
            .init_resource::<macrocosmo::scripting::GameRng>();
        let mut msgs = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<CoreDeployRequested>>();
        msgs.write(CoreDeployRequested {
            command_id: CommandId(501),
            deployer,
            target_system: sys,
            deploy_pos: pos,
            faction_owner: Some(empire),
            owner: Owner::Empire(empire),
            design_id: "infrastructure_core_v1".to_string(),
            submitted_at: 0,
        });
    }

    app.world_mut()
        .run_system_once(handle_core_deploy_requested)
        .expect("run core handler");
    app.update();
    app.update();

    // Run prune_empty_fleets to clean up the orphaned auto-fleet.
    app.world_mut()
        .run_system_once(prune_empty_fleets)
        .expect("prune");
    app.update();

    // Count fleets: only the Defense Fleet should remain (no orphan).
    let mut fleet_q = app.world_mut().query::<(Entity, &Fleet)>();
    let fleets: Vec<_> = fleet_q.iter(app.world()).collect();
    // There should be exactly one fleet that is a Defense Fleet.
    let defense_fleets: Vec<_> = fleets
        .iter()
        .filter(|(e, _)| app.world().get::<DefenseFleet>(*e).is_some())
        .collect();
    assert_eq!(defense_fleets.len(), 1, "exactly one Defense Fleet");

    // All remaining fleets that are NOT Defense Fleets should have
    // non-empty members (i.e. the orphan was pruned).
    for (e, _) in &fleets {
        if app.world().get::<DefenseFleet>(*e).is_some() {
            continue;
        }
        let m = app.world().get::<FleetMembers>(*e);
        assert!(
            m.map_or(true, |m| !m.is_empty()),
            "orphan fleet {:?} should have been pruned",
            e,
        );
    }
}

/// Destroying the Core ship causes the Defense Fleet to be pruned (via the
/// standard prune_empty_fleets path).
#[test]
fn core_destroy_prunes_defense_fleet() {
    use macrocosmo::ship::fleet::prune_empty_fleets;

    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "DestroyTest",
        [2.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let empire = empire_entity(app.world_mut());
    insert_core_design(&mut app);

    let deployer = app.world_mut().spawn_empty().id();
    let pos = system_inner_orbit_position(sys, app.world());
    {
        app.world_mut()
            .init_resource::<macrocosmo::scripting::GameRng>();
        let mut msgs = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<CoreDeployRequested>>();
        msgs.write(CoreDeployRequested {
            command_id: CommandId(502),
            deployer,
            target_system: sys,
            deploy_pos: pos,
            faction_owner: Some(empire),
            owner: Owner::Empire(empire),
            design_id: "infrastructure_core_v1".to_string(),
            submitted_at: 0,
        });
    }

    app.world_mut()
        .run_system_once(handle_core_deploy_requested)
        .expect("run core handler");
    app.update();
    app.update();

    // Find the Core ship and its Defense Fleet.
    let mut core_q = app.world_mut().query::<(Entity, &CoreShip, &Ship)>();
    let (core_entity, _, core_ship) = core_q.iter(app.world()).next().expect("Core exists");
    let defense_fleet_entity = core_ship.fleet.expect("Core has fleet");

    // Destroy the Core.
    app.world_mut().despawn(core_entity);

    // Run prune.
    app.world_mut()
        .run_system_once(prune_empty_fleets)
        .expect("prune");
    app.update();

    // Defense Fleet should be gone.
    assert!(
        app.world().get_entity(defense_fleet_entity).is_err(),
        "Defense Fleet must be despawned after Core is destroyed"
    );
}

/// Save/load round-trip preserves the DefenseFleet component.
#[test]
fn defense_fleet_save_load_round_trip() {
    use macrocosmo::persistence::savebag::{SavedComponentBag, SavedDefenseFleet};

    let system = Entity::from_raw_u32(42).unwrap();
    let df = DefenseFleet { system };
    let saved = SavedDefenseFleet::from_live(&df);
    assert_eq!(saved.system_bits, system.to_bits());

    // Round-trip through serde.
    let json = serde_json::to_string(&saved).expect("serialize");
    let restored: SavedDefenseFleet = serde_json::from_str(&json).expect("deserialize");

    let mut map = macrocosmo::persistence::remap::EntityMap::new();
    map.insert(system.to_bits(), system);
    let live = restored.into_live(&map);
    assert_eq!(live.system, system);
}

/// The `join_defense_fleet` helper successfully adds a ship to an existing
/// Defense Fleet.
#[test]
fn join_defense_fleet_helper_adds_ship() {
    use macrocosmo::ship::fleet::create_fleet;

    let mut world = bevy::ecs::world::World::new();
    let system = world.spawn_empty().id();

    // Spawn a Core-like ship as initial member.
    let core = world
        .spawn(Ship {
            name: "Core".into(),
            design_id: "core_v1".into(),
            hull_id: "hull".into(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.0,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
            fleet: None,
        })
        .id();
    let fleet = create_fleet(&mut world, "Defense Fleet".into(), vec![core], Some(core));
    world.entity_mut(fleet).insert(DefenseFleet { system });

    // Spawn a new defense platform ship.
    let platform = world
        .spawn(Ship {
            name: "Platform".into(),
            design_id: "platform_v1".into(),
            hull_id: "hull".into(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.0,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
            fleet: None,
        })
        .id();

    let ok = join_defense_fleet(&mut world, platform, system);
    assert!(ok, "join_defense_fleet should return true");

    let members = world.get::<FleetMembers>(fleet).unwrap();
    assert_eq!(members.len(), 2);
    assert!(members.contains(platform));
    assert_eq!(world.get::<Ship>(platform).unwrap().fleet, Some(fleet));
}
