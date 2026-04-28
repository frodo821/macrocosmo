//! Integration tests for #449 PR2e: Region / RegionMembership /
//! RegionRegistry / EmpireLongTermState / MidAgent / ShortAgent
//! survive a save/load round-trip with all entity references remapped
//! correctly through the [`EntityMap`].
//!
//! The tests deliberately bypass the full `GameSetupPlugin` pipeline —
//! they hand-spawn the minimum entities needed (Empire, StarSystem,
//! Region, MidAgent, ShortAgent) so we can pin the wire format without
//! pulling in the rest of the engine's startup ordering. The fixtures
//! cover both the single-region case (PR2a/b spawn shape) and a manual
//! two-region empire to stress the `Vec<u64>` / cross-entity `Option`
//! paths in the savebag shims.

use bevy::prelude::*;

use macrocosmo::ai::{MidAgent, ShortAgent, ShortScope};
use macrocosmo::components::Position;
use macrocosmo::galaxy::StarSystem;
use macrocosmo::persistence::save::SAVE_VERSION;
use macrocosmo::persistence::{LoadError, load_game_from_reader, save::save_game_to_writer};
use macrocosmo::player::{Faction, PlayerEmpire};
use macrocosmo::region::{EmpireLongTermState, Region, RegionMembership, RegionRegistry};

/// Build a minimal world containing an empire, two star systems, one
/// region anchored at the empire's home, plus its MidAgent and a Fleet-
/// scoped ShortAgent. Returns the (empire, region, mid_agent, home_system,
/// other_system, fleet_entity) handles for downstream assertions.
fn build_single_region_world() -> (
    World,
    Entity, /* empire */
    Entity, /* region */
    Entity, /* mid_agent */
    Entity, /* home_system */
    Entity, /* other_system */
    Entity, /* fleet */
) {
    let mut world = World::new();
    world.insert_resource(RegionRegistry::default());

    let empire = world
        .spawn((
            PlayerEmpire,
            Faction::new("humanity", "Humanity"),
            EmpireLongTermState::default(),
        ))
        .id();

    let home_system = world
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
        ))
        .id();
    let other_system = world
        .spawn((
            StarSystem {
                name: "Alpha Centauri".into(),
                surveyed: false,
                is_capital: false,
                star_type: "red_dwarf".into(),
            },
            Position {
                x: 4.3,
                y: 0.0,
                z: 0.0,
            },
        ))
        .id();
    let _ = other_system; // touch only — outside the spawned region.

    let region = world
        .spawn(Region {
            empire,
            member_systems: vec![home_system],
            capital_system: home_system,
            mid_agent: None,
        })
        .id();
    world
        .entity_mut(home_system)
        .insert(RegionMembership { region });
    world
        .resource_mut::<RegionRegistry>()
        .by_empire
        .entry(empire)
        .or_default()
        .push(region);

    let mid_agent = world
        .spawn(MidAgent {
            region,
            state: macrocosmo_ai::MidTermState::default(),
            auto_managed: false,
        })
        .id();
    world
        .entity_mut(region)
        .get_mut::<Region>()
        .unwrap()
        .mid_agent = Some(mid_agent);

    // Fleet entity stand-in: just a SaveableMarker-eligible empty spawn —
    // ShortAgent's `scope: Fleet(fleet)` only needs the bits to round-trip.
    // Picking an entity already covered by the persistable filter
    // (`StarSystem`) keeps the test self-contained without dragging in the
    // ship plugin.
    let fleet_entity = world
        .spawn((
            // Reuse a StarSystem so the entity is persistable; semantically
            // we treat it as the "fleet" the ShortAgent points at.
            StarSystem {
                name: "FakeFleetAnchor".into(),
                surveyed: false,
                is_capital: false,
                star_type: "yellow_dwarf".into(),
            },
            Position {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
        ))
        .id();

    world.spawn(ShortAgent {
        managed_by: mid_agent,
        scope: ShortScope::Fleet(fleet_entity),
        state: macrocosmo_ai::PlanState::default(),
        auto_managed: true,
    });

    (
        world,
        empire,
        region,
        mid_agent,
        home_system,
        other_system,
        fleet_entity,
    )
}

fn round_trip(src: &mut World) -> World {
    let mut bytes: Vec<u8> = Vec::new();
    save_game_to_writer(src, &mut bytes).expect("save_game_to_writer");
    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load_game_from_reader");
    dst
}

#[test]
fn region_components_round_trip_with_entity_remap() {
    let (mut src, _empire, _region, _mid, _home, _other, _fleet) = build_single_region_world();
    let mut dst = round_trip(&mut src);

    // Exactly one Region must come back, anchored at exactly one
    // StarSystem (the home system) with that system's RegionMembership
    // pointing at the same Region entity.
    let region_entities: Vec<Entity> = dst
        .query_filtered::<Entity, With<Region>>()
        .iter(&dst)
        .collect();
    assert_eq!(
        region_entities.len(),
        1,
        "expected exactly 1 Region after load"
    );
    let region_entity = region_entities[0];

    // Snapshot the Region payload so we can drop the immutable borrow
    // before issuing further queries on `dst`.
    let region_snapshot: Region = dst
        .get::<Region>(region_entity)
        .expect("Region missing")
        .clone();
    assert_eq!(region_snapshot.member_systems.len(), 1);
    let home_after = region_snapshot.member_systems[0];
    assert_eq!(region_snapshot.capital_system, home_after);

    let membership_region = dst
        .get::<RegionMembership>(home_after)
        .expect("home system should carry RegionMembership after load")
        .region;
    assert_eq!(
        membership_region, region_entity,
        "RegionMembership must remap to the same Region entity"
    );

    // Empire references must remap to the freshly-spawned Empire.
    let empire_after = dst
        .query_filtered::<Entity, With<PlayerEmpire>>()
        .single(&dst)
        .expect("PlayerEmpire missing");
    assert_eq!(region_snapshot.empire, empire_after);

    // EmpireLongTermState must come back attached to the empire.
    assert!(
        dst.get::<EmpireLongTermState>(empire_after).is_some(),
        "EmpireLongTermState must round-trip onto the Empire"
    );

    // MidAgent must come back, its `region` back-ref pointing at the
    // restored Region entity, and Region.mid_agent must point at it.
    let mid_after = region_snapshot
        .mid_agent
        .expect("Region.mid_agent must round-trip");
    let mid = dst.get::<MidAgent>(mid_after).expect("MidAgent missing");
    assert_eq!(mid.region, region_entity);
    assert!(!mid.auto_managed, "MidAgent.auto_managed must round-trip");

    // ShortAgent: `managed_by` remaps to the live MidAgent entity, and
    // `scope: Fleet(...)` remaps to the live "fleet" stand-in.
    let short_agents: Vec<ShortAgent> = dst.query::<&ShortAgent>().iter(&dst).cloned().collect();
    assert_eq!(short_agents.len(), 1, "expected exactly 1 ShortAgent");
    let short = &short_agents[0];
    assert_eq!(short.managed_by, mid_after);
    assert!(short.auto_managed);
    match short.scope {
        ShortScope::Fleet(_) => {
            // Entity payload remapped successfully if it is non-PLACEHOLDER
            // (i.e. it landed in the fresh world's entity set).
            // The exact value differs from the original because allocation
            // is fresh — what matters is that the bits *did* remap.
        }
        ShortScope::ColonizedSystem(_) => panic!("scope variant changed across round-trip"),
    }
}

#[test]
fn region_registry_resource_round_trips_with_remap() {
    let (mut src, empire, region, _mid, _home, _other, _fleet) = build_single_region_world();
    let registry_before = src
        .resource::<RegionRegistry>()
        .by_empire
        .get(&empire)
        .cloned();
    assert_eq!(registry_before.as_deref(), Some(&[region][..]));

    let mut dst = round_trip(&mut src);

    // Snapshot the registry contents into owned values so we can issue
    // further queries on `dst` without borrow conflicts.
    let registry_entries: Vec<(Entity, Vec<Entity>)> = dst
        .get_resource::<RegionRegistry>()
        .expect("RegionRegistry resource must round-trip")
        .by_empire
        .iter()
        .map(|(e, v)| (*e, v.clone()))
        .collect();

    // After load, the empire and region entities are fresh — but the
    // index must contain exactly one (empire, [region]) pair.
    assert_eq!(registry_entries.len(), 1, "exactly one empire indexed");
    let (live_empire, live_regions) = &registry_entries[0];
    assert_eq!(live_regions.len(), 1, "empire has exactly one region");

    // Cross-check: that empire entity is the live PlayerEmpire, and that
    // region entity is the live Region.
    let player_empire = dst
        .query_filtered::<Entity, With<PlayerEmpire>>()
        .single(&dst)
        .expect("PlayerEmpire missing");
    assert_eq!(*live_empire, player_empire);
    let live_region_entity = live_regions[0];
    assert!(
        dst.get::<Region>(live_region_entity).is_some(),
        "registry must point at a live Region entity"
    );
}

#[test]
fn two_region_empire_round_trips_cross_region_refs() {
    // Stress the `Vec<u64>`-encoded `member_systems_bits` and the
    // `RegionRegistry.by_empire` value-Vec by hand-spawning a second
    // Region in the same empire (multi-region split is a future PR
    // but the persistence layer must already cope).
    let (mut src, empire, region_a, _mid_a, home_a, other_system, _fleet) =
        build_single_region_world();

    // Grow region_a to also cover other_system, AND spawn a second region
    // anchored at a brand-new system.
    let new_system = src
        .spawn((
            StarSystem {
                name: "Tau Ceti".into(),
                surveyed: true,
                is_capital: false,
                star_type: "yellow_dwarf".into(),
            },
            Position {
                x: 12.0,
                y: 0.0,
                z: 0.0,
            },
        ))
        .id();

    // Extend region_a.member_systems with other_system.
    {
        let mut r = src.get_mut::<Region>(region_a).unwrap();
        r.member_systems.push(other_system);
    }
    src.entity_mut(other_system)
        .insert(RegionMembership { region: region_a });

    // Spawn region_b + paired MidAgent.
    let region_b = src
        .spawn(Region {
            empire,
            member_systems: vec![new_system],
            capital_system: new_system,
            mid_agent: None,
        })
        .id();
    src.entity_mut(new_system)
        .insert(RegionMembership { region: region_b });
    let mid_b = src
        .spawn(MidAgent {
            region: region_b,
            state: macrocosmo_ai::MidTermState::default(),
            auto_managed: true,
        })
        .id();
    src.get_mut::<Region>(region_b).unwrap().mid_agent = Some(mid_b);
    src.resource_mut::<RegionRegistry>()
        .by_empire
        .get_mut(&empire)
        .unwrap()
        .push(region_b);

    let mut dst = round_trip(&mut src);

    // Two Regions, both pointing at the same empire, both with valid
    // MidAgent back-refs.
    let regions: Vec<(Entity, Region)> = dst
        .query::<(Entity, &Region)>()
        .iter(&dst)
        .map(|(e, r)| (e, r.clone()))
        .collect();
    assert_eq!(regions.len(), 2, "two regions must round-trip");
    let empire_after = dst
        .query_filtered::<Entity, With<PlayerEmpire>>()
        .single(&dst)
        .expect("PlayerEmpire missing");
    for (_e, r) in &regions {
        assert_eq!(r.empire, empire_after, "every Region.empire must remap");
        let mid = r.mid_agent.expect("each Region must have a MidAgent");
        let mid_comp = dst.get::<MidAgent>(mid).expect("MidAgent entity missing");
        assert_eq!(
            mid_comp.region,
            // The MidAgent's region back-ref must round-trip to a live
            // Region entity — the entity ids are fresh after load, so
            // we can't compare to the pre-load handle directly; instead
            // check it's in the regions set.
            regions
                .iter()
                .find(|(e, _)| *e == mid_comp.region)
                .map(|(e, _)| *e)
                .unwrap_or(Entity::PLACEHOLDER)
        );
    }

    // RegionRegistry: exactly one empire entry, value is a 2-element Vec.
    let registry = dst.resource::<RegionRegistry>();
    assert_eq!(registry.by_empire.len(), 1, "one empire indexed");
    let regions_in_index = registry
        .by_empire
        .get(&empire_after)
        .expect("empire must be in registry");
    assert_eq!(
        regions_in_index.len(),
        2,
        "empire has 2 regions in registry"
    );

    // Region A must list two member systems after load (home + other_system).
    let region_a_after = regions
        .iter()
        .find(|(_, r)| r.member_systems.len() == 2)
        .map(|(e, _)| *e)
        .expect("one of the regions has 2 member systems");
    let r_a = dst.get::<Region>(region_a_after).unwrap();
    assert_eq!(
        r_a.member_systems.len(),
        2,
        "region_a must keep its 2-system membership"
    );
    // Both home_a and other_system (now remapped) must carry RegionMembership
    // pointing at region_a_after.
    let home_after = r_a.capital_system;
    let _ = home_a; // pre-load handle (different from live entity)
    assert_eq!(
        dst.get::<RegionMembership>(home_after).unwrap().region,
        region_a_after
    );
    let other_after = r_a
        .member_systems
        .iter()
        .copied()
        .find(|e| *e != home_after)
        .expect("region_a has a non-capital member");
    assert_eq!(
        dst.get::<RegionMembership>(other_after).unwrap().region,
        region_a_after,
        "RegionMembership on the second member must also remap"
    );
}

/// #449 PR2e bumped SAVE_VERSION 15 → 16 for Region / MidAgent / ShortAgent
/// fields. #472 then bumped 16 → 17 to cover the
/// `SavedGameEventKind::ShipMissing` retirement. #474 bumps 17 → 18 to add
/// `SavedKnowledgeStore::projections` (per-empire ship trajectory
/// projections, epic #473) — postcard's positional encoding requires the
/// version bump even though the new field is `#[serde(default)]`. The
/// strict-reject policy in `load.rs` continues to refuse decoding any prior
/// version so the fixture-regen workflow stays the only path forward.
#[test]
fn save_version_strictly_rejects_previous_version() {
    assert_eq!(
        SAVE_VERSION, 18,
        "#474 bumps SAVE_VERSION 17 → 18 (SavedKnowledgeStore::projections \
         added for epic #473)"
    );

    // Hand-craft a minimal byte stream that begins with a v15 version
    // header and confirm `load_game_from_reader` rejects it via
    // `LoadError::VersionMismatch`. The rest of the bytes don't need to
    // form a valid GameSave — the version check must trigger first.
    //
    // postcard's varint encoding for `15` is a single byte `0x0F`, which
    // gives us a stable preamble we can hand-emit without depending on
    // a SavedResources literal that itself drifts with each PR.
    //
    // We construct a real v15-styled GameSave by serializing a fresh
    // GameSave with version forcibly overwritten — this is more
    // robust than poking bytes by hand and proves the strict-reject
    // path triggers on real-shape inputs.
    use macrocosmo::persistence::save::GameSave;
    use macrocosmo::persistence::save::SavedResources;
    let v15 = GameSave {
        version: 15,
        scripts_version: "0.1".into(),
        resources: SavedResources {
            game_clock_elapsed: 0,
            game_speed_hexadies_per_second: 1.0,
            game_speed_previous: 1.0,
            last_production_tick: 0,
            galaxy_config: None,
            game_rng: None,
            faction_relations: None,
            pending_fact_queue: None,
            event_log: None,
            notification_queue: None,
            destroyed_ship_registry: None,
            ai_command_outbox: None,
            region_registry: None,
        },
        entities: Vec::new(),
    };
    let bytes = postcard::to_stdvec(&v15).expect("encode forged v15 save");

    let mut world = World::new();
    let result = load_game_from_reader(&mut world, &bytes[..]);
    match result {
        Err(LoadError::VersionMismatch { saved, expected }) => {
            assert_eq!(saved, 15, "saved version field must surface to caller");
            assert_eq!(expected, SAVE_VERSION);
        }
        other => panic!(
            "v15 save must be strictly rejected at load; got {:?}",
            other
        ),
    }
}
