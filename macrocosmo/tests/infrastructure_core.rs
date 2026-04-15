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
use macrocosmo::ship::core_deliverable::{
    CoreDeployTicket, PendingCoreDeploys, resolve_core_deploys,
};
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

    // Enqueue a ticket directly.
    let deployer = app.world_mut().spawn_empty().id();
    let pos = system_inner_orbit_position(sys, app.world());
    assert!((pos[0] - (5.0 + INNER_ORBIT_OFFSET_LY)).abs() < 1e-9);
    {
        app.world_mut().init_resource::<PendingCoreDeploys>();
        app.world_mut()
            .init_resource::<macrocosmo::scripting::GameRng>();
        let mut pending = app.world_mut().resource_mut::<PendingCoreDeploys>();
        pending.tickets.push(CoreDeployTicket {
            deployer,
            target_system: sys,
            deploy_pos: pos,
            faction_owner: Some(empire),
            owner: Owner::Empire(empire),
            design_id: "infrastructure_core_v1".to_string(),
            cargo_item_index: 0,
            submitted_at: 0,
        });
    }

    // Run the resolver and apply commands.
    app.world_mut()
        .run_system_once(resolve_core_deploys)
        .expect("run resolver");
    app.update();

    // Exactly one CoreShip should now exist in sys.
    let mut q = app.world_mut().query::<(Entity, &AtSystem, &CoreShip)>();
    let cores: Vec<_> = q.iter(app.world()).collect();
    assert_eq!(cores.len(), 1);
    assert_eq!(cores[0].1.0, sys);

    // Pending queue should be empty.
    let pending = app.world().resource::<PendingCoreDeploys>();
    assert!(pending.tickets.is_empty());
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
        app.world_mut().init_resource::<PendingCoreDeploys>();
        app.world_mut()
            .init_resource::<macrocosmo::scripting::GameRng>();
        let mut pending = app.world_mut().resource_mut::<PendingCoreDeploys>();
        pending.tickets.push(CoreDeployTicket {
            deployer,
            target_system: sys,
            deploy_pos: pos,
            faction_owner: Some(empire),
            owner: Owner::Empire(empire),
            design_id: "infrastructure_core_v1".to_string(),
            cargo_item_index: 0,
            submitted_at: 0,
        });
    }

    app.world_mut()
        .run_system_once(resolve_core_deploys)
        .expect("run resolver");
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
        app.world_mut().init_resource::<PendingCoreDeploys>();
        app.world_mut()
            .init_resource::<macrocosmo::scripting::GameRng>();
        let mut pending = app.world_mut().resource_mut::<PendingCoreDeploys>();
        for (deployer, faction) in [(deployer_a, empire_a), (deployer_b, empire_b)] {
            pending.tickets.push(CoreDeployTicket {
                deployer,
                target_system: sys,
                deploy_pos: pos,
                faction_owner: Some(faction),
                owner: Owner::Empire(faction),
                design_id: "infrastructure_core_v1".to_string(),
                cargo_item_index: 0,
                submitted_at: 0,
            });
        }
    }

    app.world_mut()
        .run_system_once(resolve_core_deploys)
        .expect("run resolver");
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
        app.world_mut().init_resource::<PendingCoreDeploys>();
        app.world_mut()
            .init_resource::<macrocosmo::scripting::GameRng>();
        // Collect positions before taking `&mut` on the resource, since
        // `system_inner_orbit_position` borrows `&World`.
        let pos_a = system_inner_orbit_position(sys_a, app.world());
        let pos_b = system_inner_orbit_position(sys_b, app.world());
        let mut pending = app.world_mut().resource_mut::<PendingCoreDeploys>();
        for (deployer, target, pos) in [(deployer_a, sys_a, pos_a), (deployer_b, sys_b, pos_b)] {
            pending.tickets.push(CoreDeployTicket {
                deployer,
                target_system: target,
                deploy_pos: pos,
                faction_owner: Some(empire),
                owner: Owner::Empire(empire),
                design_id: "infrastructure_core_v1".to_string(),
                cargo_item_index: 0,
                submitted_at: 0,
            });
        }
    }

    app.world_mut()
        .run_system_once(resolve_core_deploys)
        .expect("run resolver");
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
        CapabilityParams, DeliverableMetadata, ResourceCost, StructureDefinition,
        StructureRegistry,
    };
    use macrocosmo::ship::{Cargo, CargoItem, CommandQueue, QueuedCommand, ShipState};

    let mut app = full_test_app();
    let sys =
        spawn_test_system(app.world_mut(), "DeployHere", [3.0, 0.0, 0.0], 1.0, true, false);
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
    let cores: Vec<_> = q.iter(app.world()).filter(|(_, _, at, _)| at.0 == sys).collect();
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
        app.world_mut().init_resource::<PendingCoreDeploys>();
        app.world_mut()
            .init_resource::<macrocosmo::scripting::GameRng>();
        let mut pending = app.world_mut().resource_mut::<PendingCoreDeploys>();
        pending.tickets.push(CoreDeployTicket {
            deployer,
            target_system: sys,
            deploy_pos: pos,
            faction_owner: Some(empire),
            owner: Owner::Empire(empire),
            design_id: "infrastructure_core_v1".to_string(),
            cargo_item_index: 0,
            submitted_at: 0,
        });
    }

    app.world_mut()
        .run_system_once(resolve_core_deploys)
        .expect("run resolver");
    app.update();
    // Let update_sovereignty run.
    advance_time(&mut app, 1);

    let sov = app.world().get::<Sovereignty>(sys).expect("Sovereignty");
    assert_eq!(sov.owner, Some(Owner::Empire(empire)));
    assert!(sov.control_score > 0.0);
}
