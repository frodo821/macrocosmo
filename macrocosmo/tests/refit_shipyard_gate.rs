//! Regression test for the refit shipyard gate (`apply_design_refit`).
//!
//! `apply_design_refit` previously only checked that the ship was docked at
//! the target system; it did not require the system to host a shipyard. New
//! ship construction has always required a shipyard
//! (`colony/building_queue.rs:425-432`), so refit was inconsistent with
//! construction. This test pins the new behavior: refit at a shipyard-less
//! system is a no-op.

mod common;

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use macrocosmo::amount::Amt;
use macrocosmo::components::Position;
use macrocosmo::galaxy::{StarSystem, SystemModifiers};
use macrocosmo::modifier::Modifier;
use macrocosmo::ship::{
    Cargo, CommandQueue, EquippedModule, Owner, RulesOfEngagement, Ship, ShipHitpoints,
    ShipModifiers, ShipState, ShipStats,
};
use macrocosmo::ship_design::{
    DesignSlotAssignment, HullDefinition, HullRegistry, HullSlot, ModuleDefinition, ModuleRegistry,
    ModuleSize, ShipDesignDefinition, ShipDesignRegistry,
};

use common::test_app;

/// Install a minimal hull / module / design fixture. Mirrors
/// `tests/ship.rs::install_refit_fixture` but local to this file so the
/// regression remains self-contained.
fn install_refit_fixture(app: &mut App) {
    let mut hulls = HullRegistry::default();
    hulls.insert(HullDefinition {
        id: "corvette".into(),
        name: "Corvette".into(),
        description: String::new(),
        base_hp: 50.0,
        base_speed: 0.75,
        base_evasion: 30.0,
        slots: vec![HullSlot {
            slot_type: "weapon".into(),
            count: 1,
            max_size: ModuleSize::Large,
        }],
        build_cost_minerals: Amt::units(200),
        build_cost_energy: Amt::units(100),
        build_time: 60,
        maintenance: Amt::new(0, 500),
        modifiers: vec![],
        prerequisites: None,
        size: 1,
        is_capital: false,
    });

    let mut modules = ModuleRegistry::default();
    let mk = |id: &str, mineral: u64, energy: u64| ModuleDefinition {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        slot_type: "weapon".into(),
        modifiers: vec![],
        weapon: None,
        cost_minerals: Amt::units(mineral),
        cost_energy: Amt::units(energy),
        prerequisites: None,
        upgrade_to: Vec::new(),
        build_time: 0,
        power_cost: 0,
        power_output: 0,
        size: ModuleSize::Small,
    };
    modules.insert(mk("laser_mk1", 50, 20));
    modules.insert(mk("laser_mk2", 80, 30));

    let mut designs = ShipDesignRegistry::default();
    designs.insert(ShipDesignDefinition {
        id: "rev_test".into(),
        name: "Rev Test".into(),
        description: String::new(),
        hull_id: "corvette".into(),
        modules: vec![DesignSlotAssignment {
            slot_type: "weapon".into(),
            module_id: "laser_mk1".into(),
        }],
        can_survey: false,
        can_colonize: false,
        maintenance: Amt::new(0, 500),
        build_cost_minerals: Amt::units(200),
        build_cost_energy: Amt::units(100),
        build_time: 60,
        hp: 50.0,
        sublight_speed: 0.75,
        ftl_range: 0.0,
        revision: 0,
        is_direct_buildable: true,
    });

    app.insert_resource(hulls);
    app.insert_resource(modules);
    app.insert_resource(designs);
}

/// Spawn a ship docked at `system` at revision `design_revision`.
fn spawn_test_ship(world: &mut World, system: Entity, design_revision: u64) -> Entity {
    world
        .spawn((
            Ship {
                name: "Test".to_string(),
                design_id: "rev_test".to_string(),
                hull_id: "corvette".to_string(),
                modules: vec![EquippedModule {
                    slot_type: "weapon".into(),
                    module_id: "laser_mk1".into(),
                }],
                owner: Owner::Neutral,
                sublight_speed: 0.75,
                ftl_range: 0.0,
                ruler_aboard: false,
                home_port: system,
                design_revision,
                fleet: None,
            },
            ShipState::InSystem { system },
            Position::from([0.0, 0.0, 0.0]),
            ShipHitpoints {
                hull: 50.0,
                hull_max: 50.0,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            CommandQueue::default(),
            Cargo::default(),
            ShipModifiers::default(),
            ShipStats::default(),
            RulesOfEngagement::default(),
        ))
        .id()
}

/// Attach a `SystemModifiers` component to `sys` with the given shipyard
/// capacity (>0 means "has shipyard").
fn install_system_modifiers(world: &mut World, sys: Entity, shipyard_capacity_units: u64) {
    let mut mods = SystemModifiers::default();
    if shipyard_capacity_units > 0 {
        mods.shipyard_capacity.push_modifier(Modifier {
            id: "test_shipyard".into(),
            label: "test shipyard".into(),
            base_add: macrocosmo::amount::SignedAmt::from_amt(Amt::units(shipyard_capacity_units)),
            multiplier: macrocosmo::amount::SignedAmt::ZERO,
            add: macrocosmo::amount::SignedAmt::ZERO,
            expires_at: None,
            on_expire_event: None,
        });
    }
    world.entity_mut(sys).insert(mods);
}

/// Bump the registered design's revision so the ship is "behind" and would
/// otherwise be eligible for refit.
fn bump_design_revision(app: &mut App) -> u64 {
    let mut r = app.world_mut().resource_mut::<ShipDesignRegistry>();
    let mut d = r.get("rev_test").unwrap().clone();
    d.modules = vec![DesignSlotAssignment {
        slot_type: "weapon".into(),
        module_id: "laser_mk2".into(),
    }];
    r.upsert_edited(d)
}

#[test]
fn refit_no_op_when_system_lacks_shipyard() {
    let mut app = test_app();
    install_refit_fixture(&mut app);

    let sys = app
        .world_mut()
        .spawn((
            StarSystem {
                name: "S".into(),
                star_type: "g_main".into(),
                is_capital: false,
                surveyed: true,
            },
            Position::from([0.0, 0.0, 0.0]),
        ))
        .id();
    // No shipyard at this system.
    install_system_modifiers(app.world_mut(), sys, 0);

    let ship = spawn_test_ship(app.world_mut(), sys, 0);

    // Bump the design so the ship is refit-eligible apart from the shipyard
    // gate.
    let new_rev = bump_design_revision(&mut app);
    assert_eq!(new_rev, 1);

    // Capture pre-state.
    let before_rev = app.world().get::<Ship>(ship).unwrap().design_revision;
    let before_modules: Vec<(String, String)> = app
        .world()
        .get::<Ship>(ship)
        .unwrap()
        .modules
        .iter()
        .map(|m| (m.slot_type.clone(), m.module_id.clone()))
        .collect();

    // Invoke `apply_design_refit` via a one-shot system. Without a shipyard
    // the function should early-return; the ship must remain `InSystem`
    // with its original revision and modules.
    let now = 0i64;
    app.world_mut()
        .run_system_once(
            move |mut ships_query: Query<
                (
                    Entity,
                    &mut Ship,
                    &mut ShipState,
                    Option<&mut Cargo>,
                    &ShipHitpoints,
                    Option<&macrocosmo::ship::SurveyData>,
                ),
                Without<macrocosmo::colony::SlotAssignment>,
            >,
                  mut stockpiles: Query<
                (
                    &mut macrocosmo::colony::ResourceStockpile,
                    Option<&macrocosmo::colony::ResourceCapacity>,
                ),
                With<StarSystem>,
            >,
                  sys_mods_q: Query<&'static SystemModifiers>,
                  design_registry: Res<ShipDesignRegistry>,
                  hull_registry: Res<HullRegistry>,
                  module_registry: Res<ModuleRegistry>| {
                macrocosmo::ui::apply_design_refit(
                    ship,
                    sys,
                    &mut ships_query,
                    &mut stockpiles,
                    &sys_mods_q,
                    &design_registry,
                    &hull_registry,
                    &module_registry,
                    now,
                );
            },
        )
        .expect("run apply_design_refit");

    // Ship state should be unchanged: still InSystem, same revision, same modules.
    assert!(
        matches!(
            app.world().get::<ShipState>(ship),
            Some(ShipState::InSystem { system: s }) if *s == sys
        ),
        "ship must remain InSystem (no shipyard ⇒ refit no-op)"
    );
    let after = app.world().get::<Ship>(ship).unwrap();
    assert_eq!(
        after.design_revision, before_rev,
        "design_revision must not advance without a shipyard"
    );
    let after_modules: Vec<(String, String)> = after
        .modules
        .iter()
        .map(|m| (m.slot_type.clone(), m.module_id.clone()))
        .collect();
    assert_eq!(
        after_modules, before_modules,
        "modules must not change without a shipyard"
    );
}

#[test]
fn refit_proceeds_when_system_has_shipyard() {
    // Positive control: same setup but with a shipyard, the refit must engage
    // (state transitions to Refitting).
    let mut app = test_app();
    install_refit_fixture(&mut app);

    let sys = app
        .world_mut()
        .spawn((
            StarSystem {
                name: "S".into(),
                star_type: "g_main".into(),
                is_capital: false,
                surveyed: true,
            },
            Position::from([0.0, 0.0, 0.0]),
            macrocosmo::colony::ResourceStockpile {
                minerals: Amt::units(1000),
                energy: Amt::units(1000),
                research: Amt::ZERO,
                food: Amt::ZERO,
                authority: Amt::ZERO,
            },
        ))
        .id();
    install_system_modifiers(app.world_mut(), sys, 1);

    let ship = spawn_test_ship(app.world_mut(), sys, 0);

    let new_rev = bump_design_revision(&mut app);
    assert_eq!(new_rev, 1);

    let now = 0i64;
    app.world_mut()
        .run_system_once(
            move |mut ships_query: Query<
                (
                    Entity,
                    &mut Ship,
                    &mut ShipState,
                    Option<&mut Cargo>,
                    &ShipHitpoints,
                    Option<&macrocosmo::ship::SurveyData>,
                ),
                Without<macrocosmo::colony::SlotAssignment>,
            >,
                  mut stockpiles: Query<
                (
                    &mut macrocosmo::colony::ResourceStockpile,
                    Option<&macrocosmo::colony::ResourceCapacity>,
                ),
                With<StarSystem>,
            >,
                  sys_mods_q: Query<&'static SystemModifiers>,
                  design_registry: Res<ShipDesignRegistry>,
                  hull_registry: Res<HullRegistry>,
                  module_registry: Res<ModuleRegistry>| {
                macrocosmo::ui::apply_design_refit(
                    ship,
                    sys,
                    &mut ships_query,
                    &mut stockpiles,
                    &sys_mods_q,
                    &design_registry,
                    &hull_registry,
                    &module_registry,
                    now,
                );
            },
        )
        .expect("run apply_design_refit");

    assert!(
        matches!(
            app.world().get::<ShipState>(ship),
            Some(ShipState::Refitting { system: s, .. }) if *s == sys
        ),
        "ship must transition to Refitting when shipyard present"
    );
}
