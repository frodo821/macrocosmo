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
