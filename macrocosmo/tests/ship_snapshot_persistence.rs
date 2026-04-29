//! #491 (D-H-4): Postcard round-trip pins for the
//! `ShipSnapshotState::InTransitSubLight` / `InTransitFTL` split.
//!
//! `SAVE_VERSION` was bumped 19 → 20 specifically because postcard's
//! positional enum tag encoding makes adding a new enum variant a
//! breaking change. These tests pin the v20 wire format by:
//!
//! 1. Building a minimal world with a `PlayerEmpire` whose
//!    `KnowledgeStore` carries snapshots / projections at every
//!    transit variant.
//! 2. Serialising via `save_game_to_writer` (postcard) and
//!    deserialising into a fresh `World`.
//! 3. Asserting that `last_known_state` / `intended_state` /
//!    `projected_state` come back **unchanged** — the FTL marker must
//!    not collapse to the SubLight tag (or vice-versa) across a
//!    round-trip.
//!
//! Tests are kept narrow on purpose: they cover only the new transit
//! variants, since `region_persistence.rs::save_version_strictly_rejects_previous_version`
//! already covers the version-bump strict-reject path.

mod common;

use bevy::prelude::*;

use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, ShipProjection, ShipSnapshot, ShipSnapshotState,
    SystemVisibilityMap,
};
use macrocosmo::persistence::{load::load_game_from_reader, save::save_game_to_writer};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};

fn spawn_minimal_empire(world: &mut World) -> Entity {
    world
        .spawn((
            Empire {
                name: "TransitTest".into(),
            },
            PlayerEmpire,
            Faction {
                id: "snapshot_persistence".into(),
                name: "TransitTest".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            SystemVisibilityMap::default(),
            KnowledgeStore::default(),
        ))
        .id()
}

fn round_trip(src: &mut World) -> World {
    let mut bytes: Vec<u8> = Vec::new();
    save_game_to_writer(src, &mut bytes).expect("save_game_to_writer");
    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load_game_from_reader");
    dst
}

fn dst_empire(dst: &mut World) -> Entity {
    dst.query_filtered::<Entity, With<PlayerEmpire>>()
        .single(dst)
        .expect("PlayerEmpire must round-trip")
}

/// `ShipSnapshot.last_known_state` round-trips for both transit
/// variants. We spawn real Ship entities so the entity bits round-trip
/// through `EntityMap` cleanly (without that, both snapshots collapse
/// onto `Entity::PLACEHOLDER` post-load and only one survives the
/// `HashMap` re-keying).
#[test]
fn ship_snapshot_state_round_trips_through_postcard_v20() {
    use macrocosmo::ship::{Cargo, CommandQueue, Owner, RulesOfEngagement, Ship, ShipHitpoints};

    let mut src = World::new();
    let empire = spawn_minimal_empire(&mut src);

    // Spawn dummy Ship entities so they are persistable (the savebag
    // filter includes `With<Ship>`) and their entity bits land in the
    // EntityMap on load. We do not care about their realtime state —
    // this test only pins the **snapshot** wire format.
    let home = src
        .spawn(macrocosmo::galaxy::StarSystem {
            name: "Home".into(),
            surveyed: true,
            is_capital: true,
            star_type: "yellow_dwarf".into(),
        })
        .insert(macrocosmo::components::Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        })
        .id();
    let ship_sublight = src
        .spawn((
            Ship {
                name: "SubLightShip".into(),
                design_id: "explorer_mk1".into(),
                hull_id: "frigate".into(),
                modules: Vec::new(),
                owner: Owner::Empire(empire),
                sublight_speed: 1.0,
                ftl_range: 5.0,
                ruler_aboard: false,
                home_port: home,
                design_revision: 0,
                fleet: None,
            },
            macrocosmo::ship::ShipState::InSystem { system: home },
            ShipHitpoints {
                hull: 90.0,
                hull_max: 100.0,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            CommandQueue::default(),
            Cargo::default(),
            macrocosmo::ship::ShipModifiers::default(),
            macrocosmo::ship::ShipStats::default(),
            RulesOfEngagement::default(),
            macrocosmo::components::Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
        ))
        .id();
    let ship_ftl = src
        .spawn((
            Ship {
                name: "FtlShip".into(),
                design_id: "explorer_mk1".into(),
                hull_id: "frigate".into(),
                modules: Vec::new(),
                owner: Owner::Empire(empire),
                sublight_speed: 1.0,
                ftl_range: 5.0,
                ruler_aboard: false,
                home_port: home,
                design_revision: 0,
                fleet: None,
            },
            macrocosmo::ship::ShipState::InSystem { system: home },
            ShipHitpoints {
                hull: 80.0,
                hull_max: 100.0,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            CommandQueue::default(),
            Cargo::default(),
            macrocosmo::ship::ShipModifiers::default(),
            macrocosmo::ship::ShipStats::default(),
            RulesOfEngagement::default(),
            macrocosmo::components::Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
        ))
        .id();

    {
        let mut em = src.entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_ship(ShipSnapshot {
            entity: ship_sublight,
            name: "SubLightShip".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::InTransitSubLight,
            last_known_system: None,
            observed_at: 7,
            hp: 90.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });
        store.update_ship(ShipSnapshot {
            entity: ship_ftl,
            name: "FtlShip".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::InTransitFTL,
            last_known_system: None,
            observed_at: 11,
            hp: 80.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });
    }

    let mut dst = round_trip(&mut src);
    let dst_empire = dst_empire(&mut dst);
    let store = dst
        .entity(dst_empire)
        .get::<KnowledgeStore>()
        .expect("loaded empire has KnowledgeStore");

    // Find each snapshot by name (entity ids are remapped through
    // EntityMap on load — they will not match the source bits).
    let snaps: Vec<&ShipSnapshot> = store.iter_ships().map(|(_, s)| s).collect();
    let sublight = snaps
        .iter()
        .find(|s| s.name == "SubLightShip")
        .expect("SubLightShip must round-trip");
    let ftl = snaps
        .iter()
        .find(|s| s.name == "FtlShip")
        .expect("FtlShip must round-trip");

    assert_eq!(
        sublight.last_known_state,
        ShipSnapshotState::InTransitSubLight,
        "InTransitSubLight tag must survive postcard v20 round-trip"
    );
    assert_eq!(
        ftl.last_known_state,
        ShipSnapshotState::InTransitFTL,
        "InTransitFTL tag must survive postcard v20 round-trip"
    );
    assert_eq!(sublight.observed_at, 7);
    assert_eq!(ftl.observed_at, 11);
}

/// `ShipProjection.intended_state` and `projected_state` round-trip
/// for both transit variants. Pins that the projection layer also
/// preserves the FTL tag — without this the dispatcher's intent-side
/// upgrade (poll_pending_routes) would silently corrupt across save.
///
/// Like the snapshot test above, we spawn real Ship entities so the
/// entity-keyed `HashMap<Entity, ShipProjection>` round-trips two
/// distinct projections cleanly.
#[test]
fn ship_projection_intended_state_round_trips_ftl_variant() {
    use macrocosmo::ship::{Cargo, CommandQueue, Owner, RulesOfEngagement, Ship, ShipHitpoints};

    let mut src = World::new();
    let empire = spawn_minimal_empire(&mut src);

    let home = src
        .spawn(macrocosmo::galaxy::StarSystem {
            name: "Home".into(),
            surveyed: true,
            is_capital: true,
            star_type: "yellow_dwarf".into(),
        })
        .insert(macrocosmo::components::Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        })
        .id();
    let spawn_dummy_ship = |world: &mut World, name: &str| -> Entity {
        world
            .spawn((
                Ship {
                    name: name.into(),
                    design_id: "explorer_mk1".into(),
                    hull_id: "frigate".into(),
                    modules: Vec::new(),
                    owner: Owner::Empire(empire),
                    sublight_speed: 1.0,
                    ftl_range: 5.0,
                    ruler_aboard: false,
                    home_port: home,
                    design_revision: 0,
                    fleet: None,
                },
                macrocosmo::ship::ShipState::InSystem { system: home },
                ShipHitpoints {
                    hull: 100.0,
                    hull_max: 100.0,
                    armor: 0.0,
                    armor_max: 0.0,
                    shield: 0.0,
                    shield_max: 0.0,
                    shield_regen: 0.0,
                },
                CommandQueue::default(),
                Cargo::default(),
                macrocosmo::ship::ShipModifiers::default(),
                macrocosmo::ship::ShipStats::default(),
                RulesOfEngagement::default(),
                macrocosmo::components::Position {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
            ))
            .id()
    };
    let ship_sublight = spawn_dummy_ship(&mut src, "SubLightShip");
    let ship_ftl = spawn_dummy_ship(&mut src, "FtlShip");

    {
        let mut em = src.entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship_sublight,
            dispatched_at: 100,
            expected_arrival_at: Some(150),
            expected_return_at: None,
            projected_state: ShipSnapshotState::InTransitSubLight,
            projected_system: None,
            intended_state: Some(ShipSnapshotState::InTransitSubLight),
            intended_system: None,
            intended_takes_effect_at: Some(110),
        });
        store.update_projection(ShipProjection {
            entity: ship_ftl,
            dispatched_at: 200,
            expected_arrival_at: Some(220),
            expected_return_at: None,
            projected_state: ShipSnapshotState::InTransitFTL,
            projected_system: None,
            intended_state: Some(ShipSnapshotState::InTransitFTL),
            intended_system: None,
            intended_takes_effect_at: Some(205),
        });
    }

    let mut dst = round_trip(&mut src);
    let dst_empire = dst_empire(&mut dst);
    let store = dst
        .entity(dst_empire)
        .get::<KnowledgeStore>()
        .expect("loaded empire has KnowledgeStore");

    // Two projections, distinguished by `dispatched_at` (entity bits
    // are remapped on load — `dispatched_at` is preserved as a plain
    // i64 and is the easiest stable handle).
    let projections: Vec<&ShipProjection> = store.iter_projections().map(|(_, p)| p).collect();
    let sublight = projections
        .iter()
        .find(|p| p.dispatched_at == 100)
        .expect("SubLight projection (dispatched_at=100) must round-trip");
    let ftl = projections
        .iter()
        .find(|p| p.dispatched_at == 200)
        .expect("FTL projection (dispatched_at=200) must round-trip");

    assert_eq!(
        sublight.projected_state,
        ShipSnapshotState::InTransitSubLight,
        "InTransitSubLight projected_state must survive round-trip"
    );
    assert_eq!(
        sublight.intended_state,
        Some(ShipSnapshotState::InTransitSubLight),
        "InTransitSubLight intended_state must survive round-trip"
    );
    assert_eq!(
        ftl.projected_state,
        ShipSnapshotState::InTransitFTL,
        "InTransitFTL projected_state must survive round-trip"
    );
    assert_eq!(
        ftl.intended_state,
        Some(ShipSnapshotState::InTransitFTL),
        "InTransitFTL intended_state must survive round-trip"
    );

    // Spot-check unrelated fields preserved.
    assert_eq!(sublight.expected_arrival_at, Some(150));
    assert_eq!(ftl.expected_arrival_at, Some(220));
    assert_eq!(sublight.intended_takes_effect_at, Some(110));
    assert_eq!(ftl.intended_takes_effect_at, Some(205));
}
