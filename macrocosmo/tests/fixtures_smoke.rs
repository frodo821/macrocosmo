//! Integration tests for the committed save fixtures (#247).
//!
//! Pairs with `common::fixture::load_fixture` and
//! `tests/fixtures/*.bin`. The committed binary pins the on-disk postcard
//! format: any incompatible change to `SAVE_VERSION`, `GameSave`, or
//! `SavedComponentBag` will fail CI here.
//!
//! To regenerate the fixtures after an intentional format bump, run:
//!
//! ```bash
//! cargo test -p macrocosmo --test fixtures_smoke \
//!     regenerate_minimal_game_fixture -- --ignored
//! ```

#![allow(dead_code)]

mod common;

use bevy::prelude::*;
use common::fixture::{fixtures_dir, load_fixture};
use macrocosmo::amount::Amt;
use macrocosmo::colony::{Colony, LastProductionTick, ResourceStockpile};
use macrocosmo::components::Position;
use macrocosmo::faction::{FactionOwner, FactionRelations, FactionView, RelationState};
use macrocosmo::galaxy::{GalaxyConfig, Planet, Sovereignty, StarSystem, SystemAttributes};
use macrocosmo::persistence::save::save_game_to_writer;
use macrocosmo::player::{Faction, PlayerEmpire};
use macrocosmo::scripting::game_rng::GameRng;
use macrocosmo::time_system::{GameClock, GameSpeed};

/// The path (relative to `tests/fixtures/`) of the canonical minimal save.
const MINIMAL_GAME_FIXTURE: &str = "minimal_game.bin";

/// Replica of `save_load::build_seed_world` kept in sync so the regenerator
/// produces the same canonical fixture without depending on that module's
/// private `fn`. Touches one entity per savebag category relevant to
/// format-stability testing.
fn build_seed_world() -> World {
    let mut world = World::new();
    world.insert_resource(GameClock::new(123));
    world.insert_resource(GameSpeed {
        hexadies_per_second: 2.0,
        previous_speed: 4.0,
    });
    world.insert_resource(LastProductionTick(100));
    world.insert_resource(GalaxyConfig {
        radius: 25.0,
        num_systems: 3,
    });
    world.insert_resource(GameRng::from_seed(42));

    let empire = world
        .spawn((PlayerEmpire, Faction::new("humanity", "Humanity")))
        .id();
    let xeno = world.spawn(Faction::new("xeno", "Xeno")).id();
    let mut relations = FactionRelations::new();
    relations.set(empire, xeno, FactionView::new(RelationState::War, -80.0));
    relations.set(
        xeno,
        empire,
        FactionView::new(RelationState::Neutral, -10.0),
    );
    world.insert_resource(relations);

    let sol = world
        .spawn((
            StarSystem {
                name: "Sol".into(),
                surveyed: true,
                is_capital: true,
                star_type: "yellow_dwarf".into(),
            },
            Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            SystemAttributes {
                habitability: 0.9,
                mineral_richness: 0.5,
                energy_potential: 0.6,
                research_potential: 0.7,
                max_building_slots: 4,
            },
            Sovereignty {
                owner: None,
                control_score: 0.0,
            },
            ResourceStockpile {
                minerals: Amt::units(250),
                energy: Amt::units(100),
                research: Amt::units(5),
                food: Amt::units(80),
                authority: Amt::units(1000),
            },
            FactionOwner(empire),
        ))
        .id();
    let earth = world
        .spawn((
            Planet {
                name: "Earth".into(),
                system: sol,
                planet_type: "terrestrial".into(),
            },
            Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
        ))
        .id();
    world.spawn(Colony {
        planet: earth,
        population: 1_000.0,
        growth_rate: 0.01,
    });
    world
}

/// Smoke test: the committed `minimal_game.bin` decodes cleanly, lands in
/// a fresh App, and carries the expected canonical field values from
/// `build_seed_world`. This is the format-stability guard.
#[test]
fn load_minimal_game_fixture_smoke() {
    let app = load_fixture(MINIMAL_GAME_FIXTURE);
    assert_eq!(app.world().resource::<GameClock>().elapsed, 123);
    assert_eq!(app.world().resource::<GameSpeed>().hexadies_per_second, 2.0);
    assert_eq!(app.world().resource::<LastProductionTick>().0, 100);

    let cfg = app.world().resource::<GalaxyConfig>();
    assert_eq!(cfg.radius, 25.0);
    assert_eq!(cfg.num_systems, 3);

    // Entity contents: Sol + Earth must survive round-trip.
    let mut app = app;
    let world_mut = app.world_mut();
    let sol_exists = world_mut
        .query::<&StarSystem>()
        .iter(world_mut)
        .any(|s| s.name == "Sol" && s.is_capital);
    assert!(sol_exists, "Sol (capital) must round-trip");
    let earth_exists = world_mut
        .query::<&Planet>()
        .iter(world_mut)
        .any(|p| p.name == "Earth");
    assert!(earth_exists, "Earth planet must round-trip");
}

/// Maintenance-only: regenerate the committed `minimal_game.bin` from
/// `build_seed_world`. Marked `#[ignore]` so normal test runs do not
/// rewrite the fixture — run explicitly with `-- --ignored` when
/// intentionally bumping `SAVE_VERSION` or `SavedComponentBag`.
#[test]
#[ignore = "writes tests/fixtures/minimal_game.bin; run with --ignored"]
fn regenerate_minimal_game_fixture() {
    let mut world = build_seed_world();
    let mut bytes: Vec<u8> = Vec::new();
    save_game_to_writer(&mut world, &mut bytes).expect("save_game_to_writer");
    let out = fixtures_dir().join(MINIMAL_GAME_FIXTURE);
    std::fs::create_dir_all(fixtures_dir()).expect("create fixtures dir");
    std::fs::write(&out, &bytes).expect("write fixture");
    eprintln!("Regenerated {} ({} bytes)", out.display(), bytes.len());
}
