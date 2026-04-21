/// #393: Regression tests for territory visualization.
///
/// The territory system resolves colony -> planet -> star system -> position
/// to populate GPU colony data. If any link in this chain is broken,
/// the territory overlay silently produces no output.
mod common;

use bevy::prelude::*;
use common::{spawn_test_colony, spawn_test_system, test_app};
use macrocosmo::amount::Amt;
use macrocosmo::colony::Colony;
use macrocosmo::components::Position;
use macrocosmo::galaxy::Planet;

/// Verify that every Colony entity resolves through planet -> system -> position
/// without hitting a broken link. This is the chain that `sync_territory_material`
/// follows; if any link fails, that colony is silently skipped and territory
/// disappears.
#[test]
fn colony_planet_system_position_chain_resolves() {
    let mut app = test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "Territory Test",
        [10.0, 5.0, 0.0],
        0.8,
        true,
        true,
    );
    spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::units(100),
        Amt::units(100),
        vec![],
    );

    // Advance one tick so deferred commands are applied
    app.update();

    // Now walk every colony through the chain
    let world = app.world_mut();
    let mut colony_count = 0;
    let mut q = world.query::<&Colony>();
    // Collect colony data first to avoid borrow conflict
    let colonies: Vec<Entity> = q.iter(world).map(|c| c.planet).collect();
    for planet_entity in &colonies {
        let planet = world.get::<Planet>(*planet_entity).unwrap_or_else(|| {
            panic!(
                "Colony's planet {:?} has no Planet component",
                planet_entity
            )
        });
        let system = planet.system;
        let _pos = world
            .get::<Position>(system)
            .unwrap_or_else(|| panic!("Planet's system {:?} has no Position component", system));
        colony_count += 1;
    }

    assert!(
        colony_count > 0,
        "Expected at least one colony in the world"
    );
}
