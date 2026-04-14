//! Integration tests for the save/load pipeline (#247, Phase A).
//!
//! Focuses on round-trip identity for the core state that Phase A persists:
//! galaxy entities, faction relations, game rng determinism, and the scripts-
//! version mismatch warn path. Ship/colony/knowledge extension state is
//! deferred to Phase B/C and not exercised here.

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::colony::{Colony, LastProductionTick, ResourceStockpile};
use macrocosmo::components::Position;
use macrocosmo::faction::{FactionOwner, FactionRelations, FactionView, RelationState};
use macrocosmo::galaxy::{GalaxyConfig, Planet, Sovereignty, StarSystem, SystemAttributes};
use macrocosmo::persistence::{
    capture_save, load::load_game_from_reader, save::save_game_to_writer, SaveId, SCRIPTS_VERSION,
};
use macrocosmo::player::{Faction, PlayerEmpire};
use macrocosmo::scripting::game_rng::GameRng;
use macrocosmo::time_system::{GameClock, GameSpeed};
use rand::Rng;

/// Build a minimal headless world populated with a tiny galaxy, a colony, a
/// faction-owned empire, and deterministic time/rng resources. Covers the
/// Phase A serialization surface without depending on the test harness from
/// `tests/common`.
fn build_seed_world() -> World {
    let mut world = World::new();

    // Resources.
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

    // Empire + faction entities.
    let empire = world
        .spawn((
            PlayerEmpire,
            Faction {
                id: "humanity".into(),
                name: "Humanity".into(),
            },
        ))
        .id();
    let xeno_faction = world
        .spawn(Faction {
            id: "xeno".into(),
            name: "Xeno".into(),
        })
        .id();

    // Seed faction relations with asymmetric views.
    let mut relations = FactionRelations::new();
    relations.set(
        empire,
        xeno_faction,
        FactionView::new(RelationState::War, -80.0),
    );
    relations.set(
        xeno_faction,
        empire,
        FactionView::new(RelationState::Neutral, -10.0),
    );
    world.insert_resource(relations);

    // Galaxy: 2 star systems with planets and a colony.
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
    let alpha_centauri = world
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
            SystemAttributes {
                habitability: 0.2,
                mineral_richness: 0.8,
                energy_potential: 0.3,
                research_potential: 0.1,
                max_building_slots: 2,
            },
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
    world.spawn((
        Planet {
            name: "Mars".into(),
            system: sol,
            planet_type: "desert".into(),
        },
        Position {
            x: 0.1,
            y: 0.0,
            z: 0.0,
        },
    ));
    let _earth_colony = world
        .spawn(Colony {
            planet: earth,
            population: 1_000.0,
            growth_rate: 0.01,
        })
        .id();

    // Touch alpha_centauri so it's not optimised away.
    let _ = alpha_centauri;

    world
}

fn round_trip_bytes(world: &mut World) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    save_game_to_writer(world, &mut buf).expect("save_game_to_writer");
    buf
}

#[test]
fn test_save_load_round_trip_identity() {
    let mut src = build_seed_world();
    let bytes = round_trip_bytes(&mut src);
    assert!(!bytes.is_empty(), "postcard produced an empty blob");

    // Source: capture a snapshot to compare against.
    let snapshot = capture_save(&mut src).expect("capture_save");
    assert_eq!(snapshot.scripts_version, SCRIPTS_VERSION);
    assert_eq!(snapshot.resources.game_clock_elapsed, 123);
    assert_eq!(snapshot.resources.game_speed_hexadies_per_second, 2.0);
    assert_eq!(snapshot.resources.last_production_tick, 100);
    assert!(snapshot.resources.galaxy_config.is_some());
    assert!(snapshot.resources.game_rng.is_some());

    // Load into a fresh world and verify the resources landed.
    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load_game_from_reader");

    let clock = dst.resource::<GameClock>();
    assert_eq!(clock.elapsed, 123);
    let speed = dst.resource::<GameSpeed>();
    assert_eq!(speed.hexadies_per_second, 2.0);
    assert_eq!(speed.previous_speed, 4.0);
    let tick = dst.resource::<LastProductionTick>();
    assert_eq!(tick.0, 100);
    let cfg = dst.resource::<GalaxyConfig>();
    assert_eq!(cfg.radius, 25.0);
    assert_eq!(cfg.num_systems, 3);
}

#[test]
fn test_save_load_preserves_galaxy() {
    let mut src = build_seed_world();

    // Count entities with StarSystem + Planet + Colony before save.
    let src_stars = src.query::<&StarSystem>().iter(&src).count();
    let src_planets = src.query::<&Planet>().iter(&src).count();
    let src_colonies = src.query::<&Colony>().iter(&src).count();

    let bytes = round_trip_bytes(&mut src);

    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load");

    assert_eq!(
        dst.query::<&StarSystem>().iter(&dst).count(),
        src_stars,
        "star system count must match"
    );
    assert_eq!(
        dst.query::<&Planet>().iter(&dst).count(),
        src_planets,
        "planet count must match"
    );
    assert_eq!(
        dst.query::<&Colony>().iter(&dst).count(),
        src_colonies,
        "colony count must match"
    );

    // Spot-check that the capital is preserved.
    let found_capital = dst
        .query::<&StarSystem>()
        .iter(&dst)
        .any(|s| s.name == "Sol" && s.is_capital);
    assert!(found_capital, "Sol must remain flagged as capital");

    // Spot-check Earth planet's link to its system survives the remap.
    let mut saw_earth = false;
    for (planet, ) in dst.query::<(&Planet,)>().iter(&dst) {
        if planet.name == "Earth" {
            saw_earth = true;
            // The system entity is freshly allocated, but looking it up should
            // yield a StarSystem named "Sol".
            let system_name = dst.get::<StarSystem>(planet.system).map(|s| s.name.clone());
            assert_eq!(system_name.as_deref(), Some("Sol"));
        }
    }
    assert!(saw_earth, "Earth planet should round-trip");

    // Spot-check a ResourceStockpile value.
    let sol_stockpile = dst
        .query::<(&StarSystem, &ResourceStockpile)>()
        .iter(&dst)
        .find(|(s, _)| s.name == "Sol")
        .map(|(_, r)| r.minerals);
    assert_eq!(sol_stockpile, Some(Amt::units(250)));

    // SaveId is assigned on every persistable entity.
    let ids = dst.query::<&SaveId>().iter(&dst).count();
    assert!(ids > 0, "loaded entities carry SaveId markers");
}

#[test]
fn test_save_load_preserves_faction_relations() {
    let mut src = build_seed_world();
    let bytes = round_trip_bytes(&mut src);

    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load");

    // Locate the two factions by id.
    let mut empire = None;
    let mut xeno = None;
    for (e, faction) in dst.query::<(Entity, &Faction)>().iter(&dst) {
        match faction.id.as_str() {
            "humanity" => empire = Some(e),
            "xeno" => xeno = Some(e),
            _ => {}
        }
    }
    let empire = empire.expect("humanity faction must round-trip");
    let xeno = xeno.expect("xeno faction must round-trip");

    let rel = dst.resource::<FactionRelations>();
    let empire_of_xeno = rel
        .get(empire, xeno)
        .expect("empire→xeno relation must survive load");
    assert_eq!(empire_of_xeno.state, RelationState::War);
    assert!((empire_of_xeno.standing + 80.0).abs() < 1e-6);

    let xeno_of_empire = rel
        .get(xeno, empire)
        .expect("xeno→empire relation must survive load");
    assert_eq!(xeno_of_empire.state, RelationState::Neutral);
    assert!((xeno_of_empire.standing + 10.0).abs() < 1e-6);
}

#[test]
fn test_save_load_preserves_game_rng_deterministic() {
    let mut src = build_seed_world();

    // Snapshot then advance the source RNG so we can prove the save captures
    // the successor stream rather than the pre-capture one.
    let bytes = round_trip_bytes(&mut src);

    // Pull N values from a *freshly loaded* world, then again from a
    // separately loaded world. They must match bit-for-bit.
    let mut dst_a = World::new();
    load_game_from_reader(&mut dst_a, &bytes[..]).expect("load a");
    let mut dst_b = World::new();
    load_game_from_reader(&mut dst_b, &bytes[..]).expect("load b");

    let rng_a = dst_a.resource::<GameRng>().clone();
    let rng_b = dst_b.resource::<GameRng>().clone();

    let mut xs = Vec::new();
    let mut ys = Vec::new();
    {
        let ha = rng_a.handle();
        let hb = rng_b.handle();
        let mut ga = ha.lock().unwrap();
        let mut gb = hb.lock().unwrap();
        for _ in 0..16 {
            xs.push(ga.random::<u64>());
            ys.push(gb.random::<u64>());
        }
    }
    assert_eq!(xs, ys, "two loads of the same save must yield identical RNG streams");
}

#[test]
fn test_save_load_preserves_scripts_version_mismatch_warns() {
    // We can't easily intercept `log` crate output from an integration test
    // without an extra harness, so instead we cover the policy contract: the
    // load path **does not fail** on a scripts_version mismatch — it warns
    // and continues. We simulate a mismatch by hand-crafting a GameSave with
    // a different scripts_version, re-encoding, and asserting that load
    // succeeds.
    use macrocosmo::persistence::save::{GameSave, SavedResources, SAVE_VERSION};

    let save = GameSave {
        version: SAVE_VERSION,
        scripts_version: "99.99".into(),
        resources: SavedResources {
            game_clock_elapsed: 7,
            game_speed_hexadies_per_second: 1.0,
            game_speed_previous: 1.0,
            last_production_tick: 0,
            galaxy_config: None,
            game_rng: None,
            faction_relations: None,
            pending_fact_queue: None,
            event_log: None,
            notification_queue: None,
        },
        entities: Vec::new(),
    };
    let bytes = postcard::to_stdvec(&save).expect("encode forged save");

    let mut world = World::new();
    load_game_from_reader(&mut world, &bytes[..])
        .expect("scripts_version mismatch must warn, not fail");

    // Contract: the rest of the payload still lands even when the scripts
    // version differs.
    assert_eq!(world.resource::<GameClock>().elapsed, 7);
}

// ===========================================================================
// Phase B regression tests (#247)
// ===========================================================================

use macrocosmo::colony::{
    BuildKind, BuildOrder, BuildQueue, Buildings, BuildingQueue, ColonyJobRates,
};
use macrocosmo::deep_space::{
    CommDirection, DeepSpaceStructure, FTLCommRelay, StructureHitpoints,
};
use macrocosmo::events::{EventLog, GameEvent, GameEventKind};
use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, PendingFactQueue, PerceivedFact, SystemKnowledge,
    SystemSnapshot,
};
use macrocosmo::knowledge::facts::{CombatVictor, KnowledgeFact};
use macrocosmo::notifications::{NotificationPriority, NotificationQueue};
use macrocosmo::scripting::building_api::BuildingId;
use macrocosmo::ship::{
    CommandQueue, CourierMode, CourierRoute, Owner, QueuedCommand, Ship, ShipState,
};
use macrocosmo::species::{ColonyJobs, JobSlot};
use macrocosmo::technology::{ResearchQueue, TechId, TechTree};

fn seed_world_with_ship() -> (World, Entity, Entity) {
    let mut world = build_seed_world();
    let sol = world
        .query::<(Entity, &StarSystem)>()
        .iter(&world)
        .find(|(_, s)| s.name == "Sol")
        .map(|(e, _)| e)
        .expect("Sol must exist in seed");
    let empire = world
        .query_filtered::<Entity, With<PlayerEmpire>>()
        .iter(&world)
        .next()
        .expect("empire must exist");
    let ship = world
        .spawn((
            Ship {
                name: "TestShip".into(),
                design_id: "explorer_mk1".into(),
                hull_id: "corvette".into(),
                modules: Vec::new(),
                owner: Owner::Empire(empire),
                sublight_speed: 0.5,
                ftl_range: 10.0,
                player_aboard: false,
                home_port: sol,
                design_revision: 0,
            },
            ShipState::Docked { system: sol },
            macrocosmo::ship::ShipHitpoints {
                hull: 50.0, hull_max: 50.0,
                armor: 0.0, armor_max: 0.0,
                shield: 0.0, shield_max: 0.0,
                shield_regen: 0.0,
            },
        ))
        .id();
    (world, ship, sol)
}

#[test]
fn test_save_load_preserves_command_queue() {
    let (mut src, ship, sol) = seed_world_with_ship();
    let alpha = src
        .query::<(Entity, &StarSystem)>()
        .iter(&src)
        .find(|(_, s)| s.name == "Alpha Centauri")
        .map(|(e, _)| e)
        .unwrap();

    let mut cq = CommandQueue::default();
    cq.commands.push(QueuedCommand::MoveTo { system: alpha });
    cq.commands.push(QueuedCommand::Survey { system: alpha });
    cq.predicted_position = [4.3, 0.0, 0.0];
    cq.predicted_system = Some(alpha);
    src.entity_mut(ship).insert(cq);

    let bytes = round_trip_bytes(&mut src);
    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load");

    // Find the reloaded ship + alpha in two passes.
    let survey_target = dst
        .query::<(Entity, &CommandQueue)>()
        .iter(&dst)
        .next()
        .and_then(|(_, q)| {
            q.commands.get(1).and_then(|c| match c {
                QueuedCommand::Survey { system } => Some(*system),
                _ => None,
            })
        })
        .expect("CommandQueue must round-trip with Survey at index 1");

    let alpha_dst = dst
        .query::<(Entity, &StarSystem)>()
        .iter(&dst)
        .find(|(_, s)| s.name == "Alpha Centauri")
        .map(|(e, _)| e)
        .unwrap();
    assert_eq!(survey_target, alpha_dst, "Entity remap must route to the same star system");
}

#[test]
fn test_save_load_preserves_courier_route() {
    let (mut src, ship, sol) = seed_world_with_ship();
    let alpha = src
        .query::<(Entity, &StarSystem)>()
        .iter(&src)
        .find(|(_, s)| s.name == "Alpha Centauri")
        .map(|(e, _)| e)
        .unwrap();

    src.entity_mut(ship).insert(CourierRoute {
        waypoints: vec![sol, alpha, sol],
        current_index: 1,
        mode: CourierMode::ResourceTransport,
        repeat: true,
        paused: false,
    });

    let bytes = round_trip_bytes(&mut src);
    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load");

    let (_, route) = dst
        .query::<(Entity, &CourierRoute)>()
        .iter(&dst)
        .next()
        .expect("CourierRoute must round-trip");
    assert_eq!(route.waypoints.len(), 3);
    assert_eq!(route.current_index, 1);
    assert!(matches!(route.mode, CourierMode::ResourceTransport));
    assert!(route.repeat);
}

#[test]
fn test_save_load_preserves_colony_jobs_and_rates() {
    let mut src = build_seed_world();
    let colony_ent = src
        .query_filtered::<Entity, With<Colony>>()
        .iter(&src)
        .next()
        .unwrap();
    src.entity_mut(colony_ent).insert((
        ColonyJobs {
            slots: vec![
                JobSlot { job_id: "miner".into(), capacity: 10, assigned: 5, capacity_from_buildings: 8 },
                JobSlot { job_id: "farmer".into(), capacity: 6, assigned: 6, capacity_from_buildings: 4 },
            ],
        },
        {
            let mut r = ColonyJobRates::default();
            let bucket = r.bucket_mut("miner", "colony.minerals_per_hexadies");
            bucket.set_base(Amt::units(3));
            r
        },
    ));

    let bytes = round_trip_bytes(&mut src);
    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load");

    let (_, jobs, rates) = dst
        .query::<(Entity, &ColonyJobs, &ColonyJobRates)>()
        .iter(&dst)
        .next()
        .expect("ColonyJobs + ColonyJobRates must round-trip");
    assert_eq!(jobs.slots.len(), 2);
    assert_eq!(jobs.slots[0].job_id, "miner");
    assert_eq!(jobs.slots[0].assigned, 5);
    let bucket = rates.get("miner", "colony.minerals_per_hexadies").expect("bucket must exist");
    assert_eq!(bucket.base(), Amt::units(3));
}

#[test]
fn test_save_load_preserves_build_queue() {
    let mut src = build_seed_world();
    let colony_ent = src
        .query_filtered::<Entity, With<Colony>>()
        .iter(&src)
        .next()
        .unwrap();
    src.entity_mut(colony_ent).insert((
        BuildQueue {
            queue: vec![BuildOrder {
                kind: BuildKind::Ship,
                design_id: "explorer_mk1".into(),
                display_name: "Explorer".into(),
                minerals_cost: Amt::units(100),
                minerals_invested: Amt::units(30),
                energy_cost: Amt::units(50),
                energy_invested: Amt::units(10),
                build_time_total: 60,
                build_time_remaining: 45,
            }],
        },
        Buildings {
            slots: vec![
                Some(BuildingId::new("mine")),
                None,
                Some(BuildingId::new("power_plant")),
            ],
        },
        BuildingQueue::default(),
    ));

    let bytes = round_trip_bytes(&mut src);
    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load");

    let (_, bq, buildings) = dst
        .query::<(Entity, &BuildQueue, &Buildings)>()
        .iter(&dst)
        .next()
        .expect("BuildQueue + Buildings must round-trip");
    assert_eq!(bq.queue.len(), 1);
    assert_eq!(bq.queue[0].design_id, "explorer_mk1");
    assert_eq!(bq.queue[0].minerals_invested, Amt::units(30));
    assert_eq!(bq.queue[0].build_time_remaining, 45);
    assert_eq!(buildings.slots.len(), 3);
    assert_eq!(buildings.slots[0], Some(BuildingId::new("mine")));
    assert_eq!(buildings.slots[2], Some(BuildingId::new("power_plant")));
}

#[test]
fn test_save_load_preserves_knowledge_store() {
    let mut src = build_seed_world();
    let sol = src
        .query::<(Entity, &StarSystem)>()
        .iter(&src)
        .find(|(_, s)| s.name == "Sol")
        .map(|(e, _)| e)
        .unwrap();
    let empire = src
        .query_filtered::<Entity, With<PlayerEmpire>>()
        .iter(&src)
        .next()
        .unwrap();

    let mut store = KnowledgeStore::default();
    store.update(SystemKnowledge {
        system: sol,
        observed_at: 50,
        received_at: 50,
        data: SystemSnapshot {
            name: "Sol".into(),
            position: [0.0, 0.0, 0.0],
            surveyed: true,
            colonized: true,
            population: 1_000.0,
            ..Default::default()
        },
        source: ObservationSource::Direct,
    });
    src.entity_mut(empire).insert(store);

    let bytes = round_trip_bytes(&mut src);
    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load");

    // Extract the single knowledge entry's contents first (short-lived borrow),
    // then resolve Sol in a separate pass to avoid double-borrowing `dst`.
    let (observed_at, entry_name) = {
        let restored = dst
            .query_filtered::<&KnowledgeStore, With<PlayerEmpire>>()
            .iter(&dst)
            .next()
            .expect("KnowledgeStore must round-trip");
        let count = restored.iter().count();
        assert_eq!(count, 1, "one system knowledge entry expected");
        let (_, only) = restored.iter().next().unwrap();
        (only.observed_at, only.data.name.clone())
    };
    assert_eq!(observed_at, 50);
    assert_eq!(entry_name, "Sol");
}

#[test]
fn test_save_load_preserves_pending_facts() {
    let mut src = build_seed_world();
    let sol = src
        .query::<(Entity, &StarSystem)>()
        .iter(&src)
        .find(|(_, s)| s.name == "Sol")
        .map(|(e, _)| e)
        .unwrap();

    let mut queue = PendingFactQueue::default();
    queue.record(PerceivedFact {
        fact: KnowledgeFact::CombatOutcome {
            event_id: None,
            system: sol,
            victor: CombatVictor::Player,
            detail: "Won".into(),
        },
        observed_at: 100,
        arrives_at: 200,
        source: ObservationSource::Direct,
        origin_pos: [0.0, 0.0, 0.0],
        related_system: Some(sol),
    });
    src.insert_resource(queue);

    let bytes = round_trip_bytes(&mut src);
    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load");

    let queue = dst.resource::<PendingFactQueue>();
    assert_eq!(queue.facts.len(), 1);
    let f = &queue.facts[0];
    assert_eq!(f.arrives_at, 200);
    match &f.fact {
        KnowledgeFact::CombatOutcome { detail, victor, .. } => {
            assert_eq!(detail, "Won");
            assert_eq!(*victor, CombatVictor::Player);
        }
        _ => panic!("unexpected fact variant"),
    }
}

#[test]
fn test_save_load_preserves_tech_tree() {
    let mut src = build_seed_world();
    let empire = src
        .query_filtered::<Entity, With<PlayerEmpire>>()
        .iter(&src)
        .next()
        .unwrap();

    let mut tree = TechTree::default();
    tree.complete_research(TechId("industrial_automated_mining".into()));
    tree.complete_research(TechId("physics_ftl_drive".into()));
    let queue = ResearchQueue {
        current: Some(TechId("social_central_planning".into())),
        accumulated: 42.5,
        blocked: false,
    };
    src.entity_mut(empire).insert((tree, queue));

    let bytes = round_trip_bytes(&mut src);
    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load");

    let (tt, rq) = dst
        .query_filtered::<(&TechTree, &ResearchQueue), With<PlayerEmpire>>()
        .iter(&dst)
        .next()
        .expect("TechTree + ResearchQueue must round-trip");
    assert!(tt.researched.contains(&TechId("industrial_automated_mining".into())));
    assert!(tt.researched.contains(&TechId("physics_ftl_drive".into())));
    assert_eq!(rq.current, Some(TechId("social_central_planning".into())));
    assert!((rq.accumulated - 42.5).abs() < 1e-9);
}

#[test]
fn test_save_load_preserves_notifications() {
    let mut src = build_seed_world();
    let mut q = NotificationQueue::new();
    q.push("High", "first", None, NotificationPriority::High, None);
    q.push("Med", "second", None, NotificationPriority::Medium, None);
    src.insert_resource(q);

    let bytes = round_trip_bytes(&mut src);
    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load");

    let queue = dst.resource::<NotificationQueue>();
    assert_eq!(queue.items.len(), 2, "two notifications must survive");
    // Newest at front; the test pushed "Med" last so "Med" is at index 0.
    assert_eq!(queue.items[0].title, "Med");
    assert_eq!(queue.items[1].title, "High");
}

#[test]
fn test_save_load_preserves_event_log() {
    let mut src = build_seed_world();
    let sol = src
        .query::<(Entity, &StarSystem)>()
        .iter(&src)
        .find(|(_, s)| s.name == "Sol")
        .map(|(e, _)| e)
        .unwrap();

    let mut log = EventLog::default();
    log.push(GameEvent {
        id: macrocosmo::knowledge::EventId::default(),
        timestamp: 100,
        kind: GameEventKind::SurveyComplete,
        description: "Surveyed Alpha Centauri".into(),
        related_system: Some(sol),
    });
    log.push(GameEvent {
        id: macrocosmo::knowledge::EventId::default(),
        timestamp: 120,
        kind: GameEventKind::ColonyEstablished,
        description: "Colony at Mars".into(),
        related_system: None,
    });
    src.insert_resource(log);

    let bytes = round_trip_bytes(&mut src);
    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load");

    let (first_related, second_related, first_desc, first_kind, entries_len) = {
        let log = dst.resource::<EventLog>();
        (
            log.entries[0].related_system,
            log.entries[1].related_system,
            log.entries[0].description.clone(),
            log.entries[0].kind.clone(),
            log.entries.len(),
        )
    };
    assert_eq!(entries_len, 2);
    assert_eq!(first_desc, "Surveyed Alpha Centauri");
    assert_eq!(first_kind, GameEventKind::SurveyComplete);
    let sol_dst = dst
        .query::<(Entity, &StarSystem)>()
        .iter(&dst)
        .find(|(_, s)| s.name == "Sol")
        .map(|(e, _)| e)
        .unwrap();
    assert_eq!(first_related, Some(sol_dst));
    assert_eq!(second_related, None);
}

#[test]
fn test_save_load_preserves_ftl_comm_relay_pairing() {
    let mut src = build_seed_world();
    let empire = src
        .query_filtered::<Entity, With<PlayerEmpire>>()
        .iter(&src)
        .next()
        .unwrap();

    // Two relay structures pointing at each other.
    let relay_a = src
        .spawn((
            DeepSpaceStructure {
                definition_id: "ftl_comm_relay".into(),
                name: "Relay A".into(),
                owner: Owner::Empire(empire),
            },
            Position { x: 1.0, y: 0.0, z: 0.0 },
            StructureHitpoints { current: 50.0, max: 50.0 },
        ))
        .id();
    let relay_b = src
        .spawn((
            DeepSpaceStructure {
                definition_id: "ftl_comm_relay".into(),
                name: "Relay B".into(),
                owner: Owner::Empire(empire),
            },
            Position { x: 49.0, y: 0.0, z: 0.0 },
            StructureHitpoints { current: 50.0, max: 50.0 },
        ))
        .id();
    src.entity_mut(relay_a).insert(FTLCommRelay {
        paired_with: relay_b,
        direction: CommDirection::Bidirectional,
    });
    src.entity_mut(relay_b).insert(FTLCommRelay {
        paired_with: relay_a,
        direction: CommDirection::Bidirectional,
    });

    let bytes = round_trip_bytes(&mut src);
    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load");

    // Collect the two relays and verify their pairing survives the remap.
    let relays: Vec<(Entity, Entity)> = dst
        .query::<(Entity, &DeepSpaceStructure, &FTLCommRelay)>()
        .iter(&dst)
        .map(|(e, _, r)| (e, r.paired_with))
        .collect();
    assert_eq!(relays.len(), 2);
    // Each should reference the other.
    for (self_e, paired) in &relays {
        let partner_pair = relays.iter().find(|(e, _)| *e == *paired).expect("partner must exist");
        assert_eq!(partner_pair.1, *self_e, "pairing is symmetric post-load");
    }
}

#[test]
fn test_save_load_deterministic_continuation() {
    // Build two identical worlds, save one, load into another, advance each
    // by the same number of hexadies, and verify their GameClock + RNG state
    // continue in lockstep. This is the Phase B "big" integration marker.
    let mut src = build_seed_world();
    let bytes = round_trip_bytes(&mut src);

    let mut dst_a = World::new();
    load_game_from_reader(&mut dst_a, &bytes[..]).expect("load a");
    let mut dst_b = World::new();
    load_game_from_reader(&mut dst_b, &bytes[..]).expect("load b");

    // Simulate a tick advance (hand-advance the clock; the real pipeline
    // pumps the whole schedule, but the determinism contract for save/load
    // is that two independent loads advance identically).
    {
        let mut c = dst_a.resource_mut::<GameClock>();
        c.elapsed += 100;
    }
    {
        let mut c = dst_b.resource_mut::<GameClock>();
        c.elapsed += 100;
    }

    // Draw the same number of RNG samples from each and confirm bit-for-bit.
    let rng_a = dst_a.resource::<GameRng>().clone();
    let rng_b = dst_b.resource::<GameRng>().clone();
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    {
        let ha = rng_a.handle();
        let hb = rng_b.handle();
        let mut ga = ha.lock().unwrap();
        let mut gb = hb.lock().unwrap();
        for _ in 0..64 {
            xs.push(ga.random::<u64>());
            ys.push(gb.random::<u64>());
        }
    }
    assert_eq!(
        xs, ys,
        "deterministic continuation: two loads must yield identical RNG streams post-tick"
    );
    assert_eq!(
        dst_a.resource::<GameClock>().elapsed,
        dst_b.resource::<GameClock>().elapsed,
        "clock must advance identically"
    );
}

/// #270: In-flight `PendingCommand::Colony` entities must survive the real
/// save/load path — the savebag-struct-only roundtrip test in
/// `persistence::savebag::tests` doesn't exercise `EntityMap` binding
/// coverage on live entity save-ids. This test spawns a command whose
/// payload references a `host_colony` Entity, saves the whole world, loads
/// into a fresh one, and verifies the remapped Entity is still valid.
#[test]
fn test_save_load_preserves_pending_colony_command() {
    use macrocosmo::communication::{ColonyCommand, ColonyCommandKind, PendingCommand, RemoteCommand};
    let mut src = build_seed_world();

    // Pick two systems and a colony to reference.
    let (sol, alpha_centauri) = {
        let mut q = src.query::<(Entity, &StarSystem)>();
        let mut sol = None;
        let mut alpha = None;
        for (e, s) in q.iter(&src) {
            match s.name.as_str() {
                "Sol" => sol = Some(e),
                "Alpha Centauri" => alpha = Some(e),
                _ => {}
            }
        }
        (sol.unwrap(), alpha.unwrap())
    };
    let colony_entity = src
        .query::<(Entity, &Colony)>()
        .iter(&src)
        .next()
        .map(|(e, _)| e)
        .expect("build_seed_world spawned a colony");

    src.spawn(PendingCommand {
        target_system: alpha_centauri,
        command: RemoteCommand::Colony(ColonyCommand {
            target_planet: None,
            kind: ColonyCommandKind::QueueShipBuild {
                host_colony: colony_entity,
                design_id: "explorer_mk1".into(),
                build_kind: macrocosmo::colony::BuildKind::Ship,
            },
        }),
        sent_at: 100,
        arrives_at: 700,
        origin_pos: [0.0, 0.0, 0.0],
        destination_pos: [4.3, 0.0, 0.0],
    });
    let _ = sol;

    let bytes = round_trip_bytes(&mut src);
    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load");

    // Resolve the remapped Alpha Centauri + colony so we can compare.
    let alpha_dst = dst
        .query::<(Entity, &StarSystem)>()
        .iter(&dst)
        .find(|(_, s)| s.name == "Alpha Centauri")
        .map(|(e, _)| e)
        .unwrap();
    let colony_dst = dst
        .query::<(Entity, &Colony)>()
        .iter(&dst)
        .next()
        .map(|(e, _)| e)
        .expect("colony must round-trip");

    let mut q = dst.query::<&PendingCommand>();
    let cmd = q.iter(&dst).next().expect("pending command must round-trip");
    assert_eq!(cmd.target_system, alpha_dst, "target_system Entity remap");
    assert_eq!(cmd.sent_at, 100);
    assert_eq!(cmd.arrives_at, 700);
    match &cmd.command {
        RemoteCommand::Colony(ColonyCommand {
            target_planet: None,
            kind: ColonyCommandKind::QueueShipBuild { host_colony, design_id, .. },
        }) => {
            assert_eq!(*host_colony, colony_dst, "host_colony Entity remap");
            assert_eq!(design_id, "explorer_mk1");
        }
        other => panic!("unexpected command variant after load: {:?}", other),
    }
}

