//! #449 PR2c: `ShortAgent` spawn hook coverage.
//!
//! Asserts the per-Fleet / per-ColonizedSystem agent shape lands as
//! expected:
//!
//! 1. Empire with one fleet → exactly one `ShortAgent { Fleet(_) }`
//!    whose `managed_by` is the empire's MidAgent entity.
//! 2. Establishing a colony in a previously-unowned system grows
//!    `Region.member_systems` (and inserts `RegionMembership`) and
//!    spawns a `ShortAgent { ColonizedSystem(_) }` for that system.
//! 3. The colony-side agent is idempotent — a second colony in the
//!    same system reuses the existing agent.

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
use macrocosmo::galaxy::{HomeSystem, Planet, StarSystem, SystemAttributes};
use macrocosmo::knowledge::{KnowledgeStore, SystemVisibilityMap};
use macrocosmo::player::{Empire, Faction};
use macrocosmo::region::{Region, RegionMembership, RegionRegistry, spawn_initial_region};
use macrocosmo::ship::{Owner, Ship};
use macrocosmo::species::{ColonyJobs, ColonyPopulation, ColonySpecies};

use common::{spawn_test_ship, spawn_test_system_with_planet, test_app};

/// Spawn an NPC empire (no `PlayerEmpire`) with the AI integration
/// component soup the spawn hooks expect (`KnowledgeStore`,
/// `SystemVisibilityMap`, `CommsParams`).
fn spawn_npc_empire(world: &mut World) -> Entity {
    world
        .spawn((
            Empire {
                name: "PR2c spawn-test NPC".into(),
            },
            Faction::new("npc_short_agent_test", "PR2c spawn-test NPC"),
            KnowledgeStore::default(),
            SystemVisibilityMap::default(),
            CommsParams::default(),
        ))
        .id()
}

/// Install `Region` + `MidAgent` for an empire whose `HomeSystem` is
/// `home`. Mirrors the production
/// `setup::spawn_initial_region_for_faction` minus the Lua faction
/// resolve so the test does not need `GameSetupPlugin`.
fn install_region_and_mid_agent(app: &mut App, empire: Entity, home: Entity) -> (Entity, Entity) {
    let world = app.world_mut();
    if world.get_resource::<RegionRegistry>().is_none() {
        world.insert_resource(RegionRegistry::default());
    }
    let region = spawn_initial_region(world, empire, home);
    let mid = world
        .spawn(MidAgent {
            region,
            state: MidTermState::default(),
            auto_managed: true,
        })
        .id();
    if let Some(mut r) = world.get_mut::<Region>(region) {
        r.mid_agent = Some(mid);
    }
    (region, mid)
}

/// Tag an existing ship with `Owner::Empire` + `FactionOwner` so the
/// `Added<Fleet>` spawn hook resolves it to a real empire.
fn tag_ship_owner(world: &mut World, ship: Entity, empire: Entity) {
    if let Some(mut s) = world.get_mut::<Ship>(ship) {
        s.owner = Owner::Empire(empire);
    }
    world.entity_mut(ship).insert(FactionOwner(empire));
}

/// Helper: spawn a colony entity attached to `planet` owned by `empire`.
/// Mirrors the `colony::colonization` / `ship::settlement` spawn shape
/// in miniature; we only need the components the spawn hooks read
/// (`Colony.planet` for system resolution, `FactionOwner` for
/// ownership).
fn spawn_test_colony(world: &mut World, planet: Entity, empire: Entity) -> Entity {
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

#[test]
fn empire_owned_fleet_gets_short_agent_with_correct_managed_by() {
    let mut app = test_app();

    // Empire + home system + region + MidAgent.
    let empire = spawn_npc_empire(app.world_mut());
    let (home, _planet) =
        spawn_test_system_with_planet(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true);
    app.world_mut().entity_mut(empire).insert(HomeSystem(home));
    let (_region, mid) = install_region_and_mid_agent(&mut app, empire, home);

    // Spawn the courier ship → 1-ship Fleet. Tag it with Owner::Empire
    // so `spawn_short_agent_for_new_fleets` resolves the empire.
    let ship = spawn_test_ship(
        app.world_mut(),
        "Courier",
        "courier_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    tag_ship_owner(app.world_mut(), ship, empire);

    // First Update fires the `Added<Fleet>` hook.
    app.update();

    let fleet = app
        .world()
        .get::<Ship>(ship)
        .and_then(|s| s.fleet)
        .expect("ship must have a fleet");
    let agent = app
        .world()
        .get::<ShortAgent>(fleet)
        .expect("ShortAgent must be installed on courier fleet");
    assert!(
        matches!(agent.scope, ShortScope::Fleet(f) if f == fleet),
        "ShortAgent.scope should be Fleet({fleet:?}); got {:?}",
        agent.scope,
    );
    assert_eq!(
        agent.managed_by, mid,
        "ShortAgent.managed_by should point at the empire's MidAgent"
    );
    // NPC empire (no `PlayerEmpire` marker) → auto_managed = true.
    assert!(agent.auto_managed, "NPC fleets should auto_managed = true");
}

#[test]
fn establishing_colony_grows_region_and_spawns_colonized_system_short_agent() {
    let mut app = test_app();

    // Empire + home + region + MidAgent. Home is the only initial
    // region member.
    let empire = spawn_npc_empire(app.world_mut());
    let (home, _home_planet) =
        spawn_test_system_with_planet(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true);
    app.world_mut().entity_mut(empire).insert(HomeSystem(home));
    let (region, _mid) = install_region_and_mid_agent(&mut app, empire, home);

    // Spawn a NEW system + planet that the empire does NOT yet hold.
    let (new_sys, new_planet) =
        spawn_test_system_with_planet(app.world_mut(), "Frontier", [10.0, 0.0, 0.0], 0.8, false);

    // Sanity: the new system has no RegionMembership before the colony
    // spawn.
    assert!(
        app.world().get::<RegionMembership>(new_sys).is_none(),
        "new system should not yet belong to any region"
    );

    // Colony: settle in the new system. Drives the
    // `Added<Colony>` hook on the next Update.
    let _colony = spawn_test_colony(app.world_mut(), new_planet, empire);

    app.update();

    // Region grew to include `new_sys`.
    let region_comp = app
        .world()
        .get::<Region>(region)
        .expect("Region entity must still exist");
    assert!(
        region_comp.member_systems.contains(&home) && region_comp.member_systems.contains(&new_sys),
        "Region.member_systems should now contain both the home and \
         the new system; got {:?}",
        region_comp.member_systems,
    );
    // Reverse index installed.
    let membership = app
        .world()
        .get::<RegionMembership>(new_sys)
        .expect("new system should now carry a RegionMembership");
    assert_eq!(
        membership.region, region,
        "new system's RegionMembership.region should match the empire's region"
    );

    // Exactly one ColonizedSystem ShortAgent for `new_sys`.
    let mut q = app.world_mut().query::<&ShortAgent>();
    let colonized_for_new: Vec<_> = q
        .iter(app.world())
        .filter(|sa| matches!(sa.scope, ShortScope::ColonizedSystem(s) if s == new_sys))
        .collect();
    assert_eq!(
        colonized_for_new.len(),
        1,
        "exactly one ShortAgent ColonizedSystem(new_sys) should exist",
    );
}

#[test]
fn second_colony_in_same_system_reuses_existing_short_agent() {
    let mut app = test_app();

    let empire = spawn_npc_empire(app.world_mut());
    let (home, home_planet) =
        spawn_test_system_with_planet(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true);
    app.world_mut().entity_mut(empire).insert(HomeSystem(home));
    install_region_and_mid_agent(&mut app, empire, home);

    // The home system has no Colony yet — establish one. Note: the
    // home system already has `RegionMembership` (installed by
    // `spawn_initial_region` in the helper), so this exercises the
    // existing-region branch of `spawn_short_agent_for_new_colonies`.
    spawn_test_colony(app.world_mut(), home_planet, empire);
    app.update();

    let mut q = app.world_mut().query::<&ShortAgent>();
    let agents_for_home: Vec<_> = q
        .iter(app.world())
        .filter(|sa| matches!(sa.scope, ShortScope::ColonizedSystem(s) if s == home))
        .collect();
    assert_eq!(
        agents_for_home.len(),
        1,
        "first colony should produce exactly one ColonizedSystem agent",
    );

    // Second colony in the same system: spawn another planet attached
    // to `home` so the resolution `Colony.planet → Planet.system` lands
    // on the same system.
    // NOTE: `spawn_test_system_with_planet` always returns a fresh
    // (system, planet) pair; we instead spawn a Planet attached to the
    // existing `home` directly.
    let extra_planet = app
        .world_mut()
        .spawn((
            Planet {
                name: "Home II".into(),
                system: home,
                planet_type: "default".into(),
            },
            SystemAttributes {
                habitability: 0.5,
                mineral_richness: 0.5,
                energy_potential: 0.5,
                research_potential: 0.5,
                max_building_slots: 4,
            },
            Position::from([0.0, 0.0, 0.0]),
        ))
        .id();
    spawn_test_colony(app.world_mut(), extra_planet, empire);
    app.update();

    let mut q = app.world_mut().query::<&ShortAgent>();
    let agents_for_home: Vec<_> = q
        .iter(app.world())
        .filter(|sa| matches!(sa.scope, ShortScope::ColonizedSystem(s) if s == home))
        .collect();
    assert_eq!(
        agents_for_home.len(),
        1,
        "second colony in the same system should reuse the existing \
         ColonizedSystem agent (idempotent spawn hook)",
    );

    // Sanity: the empire's StarSystem marker on `home` is unchanged.
    assert!(
        app.world().get::<StarSystem>(home).is_some(),
        "home system entity should still carry StarSystem component",
    );
}
