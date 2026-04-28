//! #471: ShortAgent per-region routing regression tests.
//!
//! These tests pin the contract that fleet/colony ShortAgents resolve
//! their `managed_by` to the MidAgent of the *specific* Region whose
//! `member_systems` contains the agent's location, not blindly to the
//! empire's primary Region. The 2-region builder mirrors the one in
//! `tests/ai_per_region_npc_smoke.rs` (kept duplicated locally because
//! the builder is intentionally a one-off — it splices a multi-region
//! empire by hand, which the production spawn pipeline does not).
//!
//! Tests:
//! 1. `fleet_in_region_b_routes_to_mid_b` — a fleet whose flagship sits
//!    in region B gets `managed_by == mid_b`.
//! 2. `colony_in_region_b_routes_to_mid_b` — a colony settled in
//!    region B's `target_b` produces a `ColonizedSystem(target_b)`
//!    ShortAgent with `managed_by == mid_b`.
//! 3. `fleet_movement_across_regions_rehomes_short_agent` — a fleet
//!    that crosses region boundaries has its `managed_by` rehomed by
//!    the per-tick `rehome_fleet_short_agents` system.

mod common;

use bevy::prelude::*;

use macrocosmo::ai::short_agent::{ShortAgent, ShortScope};
use macrocosmo::ai::{MidAgent, core::MidTermState};
use macrocosmo::amount::Amt;
use macrocosmo::colony::{
    BuildQueue, BuildingQueue, Buildings, Colony, ColonyJobRates, FoodConsumption, MaintenanceCost,
    Production, ProductionFocus,
};
use macrocosmo::components::Position;
use macrocosmo::empire::CommsParams;
use macrocosmo::faction::FactionOwner;
use macrocosmo::galaxy::{AtSystem, HomeSystem, Planet, StarSystem, SystemAttributes};
use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, SystemKnowledge, SystemSnapshot, SystemVisibilityMap,
    SystemVisibilityTier,
};
use macrocosmo::player::{Empire, Faction};
use macrocosmo::region::{
    EmpireLongTermState, Region, RegionMembership, RegionRegistry, spawn_initial_region,
};
use macrocosmo::ship::{CoreShip, Owner, Ship};
use macrocosmo::species::{ColonyJobs, ColonyPopulation, ColonySpecies};

use common::{advance_time, spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

#[derive(Debug, Clone, Copy)]
struct TwoRegionLayout {
    empire: Entity,
    #[allow(dead_code)]
    region_a: Entity,
    #[allow(dead_code)]
    region_b: Entity,
    mid_a: Entity,
    mid_b: Entity,
    home_a: Entity,
    target_a: Entity,
    home_b: Entity,
    target_b: Entity,
}

fn place_core_at(world: &mut World, empire: Entity, system: Entity, position: [f64; 3]) -> Entity {
    let pos = Position::from(position);
    world
        .spawn((
            Ship {
                name: "Core".into(),
                design_id: "infrastructure_core_v1".into(),
                hull_id: "infrastructure_core_hull".into(),
                modules: Vec::new(),
                owner: Owner::Empire(empire),
                sublight_speed: 0.0,
                ftl_range: 0.0,
                ruler_aboard: false,
                home_port: system,
                design_revision: 0,
                fleet: None,
            },
            macrocosmo::ship::ShipState::InSystem { system },
            pos,
            macrocosmo::ship::ShipHitpoints {
                hull: 400.0,
                hull_max: 400.0,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            macrocosmo::ship::CommandQueue::default(),
            macrocosmo::ship::Cargo::default(),
            macrocosmo::ship::ShipModifiers::default(),
            macrocosmo::ship::ShipStats::default(),
            macrocosmo::ship::RulesOfEngagement::default(),
            CoreShip,
            AtSystem(system),
            FactionOwner(empire),
        ))
        .id()
}

fn spawn_test_colony_local(world: &mut World, planet: Entity, empire: Entity) -> Entity {
    world
        .spawn((
            Colony {
                planet,
                growth_rate: 0.005,
            },
            Production {
                minerals_per_hexadies: macrocosmo::modifier::ModifiedValue::new(Amt::ZERO),
                energy_per_hexadies: macrocosmo::modifier::ModifiedValue::new(Amt::ZERO),
                research_per_hexadies: macrocosmo::modifier::ModifiedValue::new(Amt::ZERO),
                food_per_hexadies: macrocosmo::modifier::ModifiedValue::new(Amt::ZERO),
            },
            BuildQueue::default(),
            Buildings { slots: vec![] },
            BuildingQueue::default(),
            ProductionFocus::default(),
            MaintenanceCost::default(),
            FoodConsumption::default(),
            ColonyPopulation {
                species: vec![ColonySpecies {
                    species_id: "human".into(),
                    population: 10,
                }],
                growth_accumulator: 0.0,
            },
            ColonyJobs::default(),
            ColonyJobRates::default(),
            FactionOwner(empire),
        ))
        .id()
}

fn spawn_planet_in(world: &mut World, system: Entity, name: &str, pos: [f64; 3]) -> Entity {
    world
        .spawn((
            Planet {
                name: name.into(),
                system,
                planet_type: "default".into(),
            },
            SystemAttributes {
                habitability: 0.7,
                mineral_richness: 0.5,
                energy_potential: 0.5,
                research_potential: 0.5,
                max_building_slots: 4,
            },
            Position::from(pos),
        ))
        .id()
}

/// Build a 2-region NPC empire (no colonies, no fleets). Each test
/// adds the relevant entities to exercise its specific code path.
fn build_two_region_npc_skeleton(app: &mut App) -> TwoRegionLayout {
    let world = app.world_mut();
    if world.get_resource::<RegionRegistry>().is_none() {
        world.insert_resource(RegionRegistry::default());
    }

    let empire = world
        .spawn((
            Empire {
                name: "Two-Region NPC".into(),
            },
            Faction::new("two_region_npc_routing", "Two-Region NPC"),
            KnowledgeStore::default(),
            SystemVisibilityMap::default(),
            CommsParams::default(),
            EmpireLongTermState::default(),
        ))
        .id();

    let home_a = spawn_test_system(world, "HomeA", [0.0, 0.0, 0.0], 1.0, true, true);
    let target_a = spawn_test_system(world, "TargetA", [0.5, 0.0, 0.0], 1.0, true, false);
    let home_b = spawn_test_system(world, "HomeB", [100.0, 0.0, 0.0], 1.0, true, true);
    let target_b = spawn_test_system(world, "TargetB", [100.5, 0.0, 0.0], 1.0, true, false);
    world.entity_mut(empire).insert(HomeSystem(home_a));

    spawn_test_ruler(world, empire, home_a);

    {
        let mut em = world.entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        for sys in [home_a, target_a, home_b, target_b] {
            vis.set(sys, SystemVisibilityTier::Surveyed);
        }
    }
    {
        let mut em = world.entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        let mut record = |sys: Entity, name: &str, pos: [f64; 3], colonized: bool| {
            store.update(SystemKnowledge {
                system: sys,
                observed_at: 0,
                received_at: 0,
                data: SystemSnapshot {
                    name: name.into(),
                    position: pos,
                    surveyed: true,
                    colonized,
                    ..Default::default()
                },
                source: ObservationSource::Direct,
            });
        };
        record(home_a, "HomeA", [0.0, 0.0, 0.0], true);
        record(target_a, "TargetA", [0.5, 0.0, 0.0], false);
        record(home_b, "HomeB", [100.0, 0.0, 0.0], true);
        record(target_b, "TargetB", [100.5, 0.0, 0.0], false);
    }

    place_core_at(world, empire, target_a, [0.5, 0.0, 0.0]);
    place_core_at(world, empire, target_b, [100.5, 0.0, 0.0]);

    // Region A via spawn_initial_region.
    let region_a = spawn_initial_region(world, empire, home_a);
    {
        let mut r = world.get_mut::<Region>(region_a).unwrap();
        r.member_systems.push(target_a);
    }
    world
        .entity_mut(target_a)
        .insert(RegionMembership { region: region_a });

    // Region B hand-spawned (multi-region splits not yet in production).
    let region_b = world
        .spawn(Region {
            empire,
            member_systems: vec![home_b, target_b],
            capital_system: home_b,
            mid_agent: None,
        })
        .id();
    world
        .entity_mut(home_b)
        .insert(RegionMembership { region: region_b });
    world
        .entity_mut(target_b)
        .insert(RegionMembership { region: region_b });
    world
        .resource_mut::<RegionRegistry>()
        .by_empire
        .entry(empire)
        .or_default()
        .push(region_b);

    let mid_a = world
        .spawn(MidAgent {
            region: region_a,
            state: MidTermState::default(),
            auto_managed: true,
        })
        .id();
    let mid_b = world
        .spawn(MidAgent {
            region: region_b,
            state: MidTermState::default(),
            auto_managed: true,
        })
        .id();
    world.get_mut::<Region>(region_a).unwrap().mid_agent = Some(mid_a);
    world.get_mut::<Region>(region_b).unwrap().mid_agent = Some(mid_b);

    TwoRegionLayout {
        empire,
        region_a,
        region_b,
        mid_a,
        mid_b,
        home_a,
        target_a,
        home_b,
        target_b,
    }
}

#[test]
fn fleet_in_region_b_routes_to_mid_b() {
    let mut app = test_app();
    let layout = build_two_region_npc_skeleton(&mut app);

    // Spawn an idle ship in region B's home so the `Added<Fleet>` hook
    // fires once a fleet is auto-created for it.
    let ship = spawn_test_ship(
        app.world_mut(),
        "ColonyB",
        "colony_ship_mk1",
        layout.home_b,
        [100.0, 0.0, 0.0],
    );
    {
        let world = app.world_mut();
        world.entity_mut(ship).get_mut::<Ship>().unwrap().owner = Owner::Empire(layout.empire);
    }

    // One tick: spawn hooks resolve the Mid; rehome runs but is a no-op.
    advance_time(&mut app, 1);

    let fleet = app
        .world()
        .get::<Ship>(ship)
        .and_then(|s| s.fleet)
        .expect("ship must belong to an auto-spawned fleet");
    let agent = app
        .world()
        .get::<ShortAgent>(fleet)
        .expect("ShortAgent must be installed on the fleet");
    assert!(
        matches!(agent.scope, ShortScope::Fleet(f) if f == fleet),
        "ShortAgent.scope should be Fleet({fleet:?}); got {:?}",
        agent.scope,
    );
    assert_eq!(
        agent.managed_by, layout.mid_b,
        "Fleet ShortAgent for a ship in region B must route to mid_b \
         (#471); got {:?}, expected {:?}",
        agent.managed_by, layout.mid_b,
    );
}

#[test]
fn colony_in_region_b_routes_to_mid_b() {
    let mut app = test_app();
    let layout = build_two_region_npc_skeleton(&mut app);

    // Settle a colony in region B's target_b. `target_b` already has
    // `RegionMembership { region: region_b }` from the skeleton, so
    // Tier 1 of the resolver fires.
    let target_b_planet = spawn_planet_in(
        app.world_mut(),
        layout.target_b,
        "TargetB-I",
        [100.5, 0.0, 0.0],
    );
    let _colony = spawn_test_colony_local(app.world_mut(), target_b_planet, layout.empire);

    advance_time(&mut app, 1);

    let mut q = app.world_mut().query::<&ShortAgent>();
    let agents_for_target_b: Vec<ShortAgent> = q
        .iter(app.world())
        .filter(|sa| matches!(sa.scope, ShortScope::ColonizedSystem(s) if s == layout.target_b))
        .cloned()
        .collect();
    assert_eq!(
        agents_for_target_b.len(),
        1,
        "exactly one ColonizedSystem(target_b) ShortAgent should exist; got {}",
        agents_for_target_b.len(),
    );
    assert_eq!(
        agents_for_target_b[0].managed_by, layout.mid_b,
        "Colony ShortAgent for target_b must route to mid_b (#471); \
         got {:?}, expected {:?}",
        agents_for_target_b[0].managed_by, layout.mid_b,
    );
}

#[test]
fn fleet_movement_across_regions_rehomes_short_agent() {
    let mut app = test_app();
    let layout = build_two_region_npc_skeleton(&mut app);

    // Spawn an idle ship in region A's home_a.
    let ship = spawn_test_ship(
        app.world_mut(),
        "Wanderer",
        "colony_ship_mk1",
        layout.home_a,
        [0.0, 0.0, 0.0],
    );
    {
        let world = app.world_mut();
        world.entity_mut(ship).get_mut::<Ship>().unwrap().owner = Owner::Empire(layout.empire);
    }

    advance_time(&mut app, 1);

    let fleet = app
        .world()
        .get::<Ship>(ship)
        .and_then(|s| s.fleet)
        .expect("ship must belong to a fleet");
    let initial = app
        .world()
        .get::<ShortAgent>(fleet)
        .expect("ShortAgent must be installed");
    assert_eq!(
        initial.managed_by, layout.mid_a,
        "Initial managed_by must point at mid_a (ship is in home_a)"
    );

    // Teleport the ship to region B's home_b and update its
    // `RegionMembership`-bearing system. (We mutate `ShipState` directly
    // — the production move pipeline goes through FTL/sublight, but for
    // this test we only care that the rehome system reads
    // `ShipState::InSystem` and resolves the new region.)
    {
        let world = app.world_mut();
        let mut state = world.get_mut::<macrocosmo::ship::ShipState>(ship).unwrap();
        *state = macrocosmo::ship::ShipState::InSystem {
            system: layout.home_b,
        };
    }

    // One more tick → `rehome_fleet_short_agents` picks up the
    // boundary crossing and updates `managed_by`.
    advance_time(&mut app, 1);

    let after = app
        .world()
        .get::<ShortAgent>(fleet)
        .expect("ShortAgent must still exist after move");
    assert_eq!(
        after.managed_by, layout.mid_b,
        "Fleet ShortAgent must rehome to mid_b after flagship moves \
         into region B (#471); got {:?}, expected {:?}",
        after.managed_by, layout.mid_b,
    );
}
