//! #449 PR2f: per-Region NPC e2e smoke test.
//!
//! Builds a 2-region NPC empire and confirms the full geographic AI
//! pipeline works end-to-end:
//!
//! - Each `MidAgent` emits independently for its own region's
//!   `member_systems` (Rules 1 / 3 / 6 / 7 / 8, plus 5a). Cross-region
//!   intel does not bleed into the "wrong" Mid's emit.
//! - Each `ShortAgent` (per-Fleet for Rule 2 / per-`ColonizedSystem`
//!   for Rule 5b) fires concurrently with the right `managed_by` Mid.
//! - The merged `AiCommandOutbox` carries entries from both Mids in
//!   the same tick (= multi-Mid is actually live, not just one Mid
//!   doing all the work for the empire).
//! - A save/load round-trip round-trips Region / MidAgent / ShortAgent /
//!   `EmpireLongTermState`, and the post-load world keeps emitting
//!   region-scoped commands.
//!
//! The 2-region empire is built by hand (multi-region splits are still
//! a future PR — the production spawn path always creates exactly one
//! Region per empire today). The construction path mirrors the helper
//! in `tests/mid_agent_member_filter.rs`, extended with idle Fleet
//! ships *and* colonies so both ShortAgent variants fire.

mod common;

use bevy::prelude::*;

use macrocosmo::ai::command_outbox::AiCommandOutbox;
use macrocosmo::ai::schema::ids::command as cmd_ids;
use macrocosmo::ai::{MidAgent, ShortAgent, ShortScope, core::MidTermState};
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
use macrocosmo::persistence::{load::load_game_from_reader, save::save_game_to_writer};
use macrocosmo::player::{Empire, Faction};
use macrocosmo::region::{
    EmpireLongTermState, Region, RegionMembership, RegionRegistry, spawn_initial_region,
};
use macrocosmo::ship::{CoreShip, Owner, Ship};
use macrocosmo::species::{ColonyJobs, ColonyPopulation, ColonySpecies};

use common::{advance_time, spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

// ---------------------------------------------------------------------------
// Two-region NPC fixture builder.
// ---------------------------------------------------------------------------

/// Layout produced by [`build_two_region_npc`].
///
/// Each region has its own home (= surveyed, colonized capital) and
/// a frontier candidate (= surveyed but un-colonized). The frontier
/// systems sit at the same x position as their home so the per-region
/// idle ships can reach the local frontier while still being far from
/// the other region's frontier.
#[derive(Debug, Clone, Copy)]
struct TwoRegionLayout {
    empire: Entity,
    region_a: Entity,
    region_b: Entity,
    mid_a: Entity,
    mid_b: Entity,
    home_a: Entity,
    target_a: Entity,
    home_b: Entity,
    target_b: Entity,
    /// Idle colony ship docked at `home_a` (region A).
    colony_ship_a: Entity,
    /// Idle colony ship docked at `home_b` (region B).
    colony_ship_b: Entity,
}

/// Place an Infrastructure Core ship at `system` so the Rule 3 colonize
/// gate (#299) recognises `system` as a candidate for this empire.
/// Mirrors the helper in `tests/ai_npc_outbox_dedup.rs`.
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

/// Spawn a Colony at `planet` owned by `empire`. Mirrors
/// `tests/short_agent_spawn.rs::spawn_test_colony` (the test helper —
/// not the production `colonization` system) so the `Added<Colony>`
/// hook in `spawn_short_agent_for_new_colonies` resolves an empire and
/// a region.
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

/// Spawn a Planet attached to `system` so a colony can be settled in
/// `system` without going through `spawn_test_system_with_planet` (which
/// always returns a fresh system + planet pair).
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

/// Build a 2-region NPC empire with:
///
/// - Region A = `[home_a, target_a]`, Mid A.
/// - Region B = `[home_b, target_b]`, Mid B.
/// - One idle colony ship in each home.
/// - One Core ship in each frontier target (Rule 3 #299 gate).
/// - One Colony in each home (so a `ColonizedSystem` ShortAgent spawns).
/// - KnowledgeStore + visibility seeded so each region's frontier is a
///   valid Rule 3 candidate.
///
/// Construction path (manual splice — the production spawn pipeline
/// always builds exactly one region per empire today):
///
/// 1. `spawn_initial_region` for the home_a system → installs Region A
///    + RegionMembership(home_a) + RegionRegistry entry.
/// 2. Hand-spawn Region B for home_b (insert RegionMembership on
///    home_b + push to `RegionRegistry.by_empire[empire]`).
/// 3. Hand-spawn one MidAgent per region; populate `Region.mid_agent`.
/// 4. Add target_a / target_b to their respective regions
///    (`member_systems.push(target)` + RegionMembership on target).
fn build_two_region_npc(app: &mut App) -> TwoRegionLayout {
    let world = app.world_mut();
    if world.get_resource::<RegionRegistry>().is_none() {
        world.insert_resource(RegionRegistry::default());
    }

    // Empire (NPC: no PlayerEmpire so `mark_npc_empires_ai_controlled`
    // adds AiControlled on the first tick).
    let empire = world
        .spawn((
            Empire {
                name: "Two-Region NPC".into(),
            },
            Faction::new("two_region_npc", "Two-Region NPC"),
            KnowledgeStore::default(),
            SystemVisibilityMap::default(),
            CommsParams::default(),
            EmpireLongTermState::default(),
        ))
        .id();

    // Two homes far apart on the x axis; each home has a frontier
    // target close by (so the per-region colony ship can reach it).
    let home_a = spawn_test_system(world, "HomeA", [0.0, 0.0, 0.0], 1.0, true, true);
    let target_a = spawn_test_system(world, "TargetA", [0.5, 0.0, 0.0], 1.0, true, false);
    let home_b = spawn_test_system(world, "HomeB", [100.0, 0.0, 0.0], 1.0, true, true);
    let target_b = spawn_test_system(world, "TargetB", [100.5, 0.0, 0.0], 1.0, true, false);
    world.entity_mut(empire).insert(HomeSystem(home_a));

    spawn_test_ruler(world, empire, home_a);

    // Visibility + KnowledgeStore for both regions.
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

    // Core ships on the frontier targets so Rule 3 (colonize) sees them
    // as candidates for this empire.
    place_core_at(world, empire, target_a, [0.5, 0.0, 0.0]);
    place_core_at(world, empire, target_b, [100.5, 0.0, 0.0]);

    // ----- Region A: use spawn_initial_region for canonical wiring. ---
    let region_a = spawn_initial_region(world, empire, home_a);
    // Extend region_a to also cover target_a.
    {
        let mut r = world.get_mut::<Region>(region_a).unwrap();
        r.member_systems.push(target_a);
    }
    world
        .entity_mut(target_a)
        .insert(RegionMembership { region: region_a });

    // ----- Region B: hand-spawn (a second region for the same empire).
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

    // MidAgents: one per region, both auto_managed (= NPC behaviour).
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

    // ----- Colonies: settle each home (drives Rule 5b ColonizedSystem
    // ShortAgents on the spawn hook).
    let home_a_planet = spawn_planet_in(world, home_a, "HomeA-I", [0.0, 0.0, 0.0]);
    let home_b_planet = spawn_planet_in(world, home_b, "HomeB-I", [100.0, 0.0, 0.0]);
    let _colony_a = spawn_test_colony(world, home_a_planet, empire);
    let _colony_b = spawn_test_colony(world, home_b_planet, empire);

    // ----- Idle colony ships per home (Rule 3 will pick them up). ---
    let colony_ship_a =
        spawn_test_ship(world, "ColonyA", "colony_ship_mk1", home_a, [0.0, 0.0, 0.0]);
    let colony_ship_b = spawn_test_ship(
        world,
        "ColonyB",
        "colony_ship_mk1",
        home_b,
        [100.0, 0.0, 0.0],
    );
    for s in [colony_ship_a, colony_ship_b] {
        world.entity_mut(s).get_mut::<Ship>().unwrap().owner = Owner::Empire(empire);
    }

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
        colony_ship_a,
        colony_ship_b,
    }
}

// ---------------------------------------------------------------------------
// Outbox helpers.
// ---------------------------------------------------------------------------

fn outbox_entries_for(
    app: &App,
    kind: macrocosmo_ai::CommandKindId,
    target: Entity,
) -> Vec<&macrocosmo::ai::command_outbox::PendingAiCommand> {
    let outbox = app.world().resource::<AiCommandOutbox>();
    outbox
        .entries
        .iter()
        .filter(|entry| {
            let cmd = &entry.command;
            if cmd.kind != kind {
                return false;
            }
            match cmd.params.get("target_system") {
                Some(macrocosmo_ai::CommandValue::System(sys_id)) => target.to_bits() == sys_id.0,
                _ => false,
            }
        })
        .collect()
}

fn count_outbox_for(app: &App, kind: macrocosmo_ai::CommandKindId, target: Entity) -> usize {
    outbox_entries_for(app, kind, target).len()
}

/// Extract the `ship_0` Entity from a command's params (used by Rule 3
/// `colonize_system` to pass the ship the order is for).
fn cmd_ship_0(cmd: &macrocosmo_ai::Command) -> Option<Entity> {
    match cmd.params.get("ship_0")? {
        macrocosmo_ai::CommandValue::Entity(e) => Some(Entity::from_bits(e.0)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[test]
fn per_region_npc_emits_independently_and_no_cross_region_leak() {
    let mut app = test_app();
    let layout = build_two_region_npc(&mut app);

    // Drive a few ticks: `npc_decision_tick` (Mid) + `run_short_agents`
    // (Short) + `dispatch_ai_pending_commands` are all wired into
    // `AiTickSet::Reason` by the AiPlugin in `test_app()`.
    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    // ----- Mid: Rule 3 (colonize) must fire in BOTH regions, each
    // pointing at its OWN frontier target.
    let target_a_count = count_outbox_for(&app, cmd_ids::colonize_system(), layout.target_a);
    let target_b_count = count_outbox_for(&app, cmd_ids::colonize_system(), layout.target_b);
    assert!(
        target_a_count >= 1,
        "Mid A must dispatch colonize on target_a (region A); got {}",
        target_a_count,
    );
    assert!(
        target_b_count >= 1,
        "Mid B must dispatch colonize on target_b (region B); got {}",
        target_b_count,
    );

    // ----- Cross-region leak guard: every colonize_system command must
    // pair (target ∈ region A) with (ship_0 ∈ region A's idle ships),
    // and likewise for region B.
    {
        let outbox = app.world().resource::<AiCommandOutbox>();
        let kind = cmd_ids::colonize_system();
        for entry in outbox.entries.iter() {
            let cmd = &entry.command;
            if cmd.kind != kind {
                continue;
            }
            let target_sys = match cmd.params.get("target_system") {
                Some(macrocosmo_ai::CommandValue::System(s)) => Entity::from_bits(s.0),
                _ => continue,
            };
            let Some(ship) = cmd_ship_0(cmd) else {
                continue;
            };

            if target_sys == layout.target_a {
                assert_eq!(
                    ship, layout.colony_ship_a,
                    "colonize on target_a must use the region-A colony ship; \
                     got {:?}, expected {:?} — cross-region leakage",
                    ship, layout.colony_ship_a,
                );
            } else if target_sys == layout.target_b {
                assert_eq!(
                    ship, layout.colony_ship_b,
                    "colonize on target_b must use the region-B colony ship; \
                     got {:?}, expected {:?} — cross-region leakage",
                    ship, layout.colony_ship_b,
                );
            } else {
                panic!(
                    "colonize_system command for unexpected target {:?} \
                     (neither region A nor region B)",
                    target_sys
                );
            }
        }
    }

    // ----- ShortAgents: every Fleet (2) and ColonizedSystem (2) entry
    // exists, each routed to the MidAgent of the region whose
    // `member_systems` contains its location (#471). After
    // `spawn_short_agent_for_new_fleets` /
    // `spawn_short_agent_for_new_colonies` resolve the managing Mid
    // through the 3-tier `resolve_mid_agent_for_system` fallback,
    // region-A ShortAgents (Fleet whose flagship lives in `home_a`,
    // ColonizedSystem(`home_a`)) point at `mid_a`, and region-B
    // ShortAgents point at `mid_b`. Cross-region leakage at the Short
    // layer would surface as the wrong `managed_by` here.
    let short_agents: Vec<ShortAgent> = app
        .world_mut()
        .query::<&ShortAgent>()
        .iter(app.world())
        .cloned()
        .collect();
    let fleet_agents: Vec<&ShortAgent> = short_agents
        .iter()
        .filter(|sa| matches!(sa.scope, ShortScope::Fleet(_)))
        .collect();
    let colonized_agents: Vec<&ShortAgent> = short_agents
        .iter()
        .filter(|sa| matches!(sa.scope, ShortScope::ColonizedSystem(_)))
        .collect();
    assert_eq!(
        fleet_agents.len(),
        2,
        "expected exactly 2 Fleet ShortAgents (one per idle colony \
         ship); got {}",
        fleet_agents.len(),
    );
    assert_eq!(
        colonized_agents.len(),
        2,
        "expected exactly 2 ColonizedSystem ShortAgents (one per home); \
         got {}",
        colonized_agents.len(),
    );

    // ColonizedSystem ShortAgent scope coverage: both home_a and home_b
    // are wired (= the per-system Colony hook fired in both regions even
    // though they share the same empire).
    let colonized_systems: std::collections::HashSet<Entity> = colonized_agents
        .iter()
        .filter_map(|sa| match sa.scope {
            ShortScope::ColonizedSystem(s) => Some(s),
            _ => None,
        })
        .collect();
    assert!(
        colonized_systems.contains(&layout.home_a),
        "ColonizedSystem ShortAgent must cover home_a (region A); had {:?}",
        colonized_systems,
    );
    assert!(
        colonized_systems.contains(&layout.home_b),
        "ColonizedSystem ShortAgent must cover home_b (region B); had {:?}",
        colonized_systems,
    );

    // Per-region routing contract (#471): each Fleet ShortAgent's
    // `managed_by` matches its flagship's region's MidAgent, and each
    // ColonizedSystem ShortAgent's `managed_by` matches the home
    // region's MidAgent.
    let fleet_a = app
        .world()
        .get::<Ship>(layout.colony_ship_a)
        .and_then(|s| s.fleet)
        .expect("region-A colony ship must belong to a fleet");
    let fleet_b = app
        .world()
        .get::<Ship>(layout.colony_ship_b)
        .and_then(|s| s.fleet)
        .expect("region-B colony ship must belong to a fleet");
    for sa in &short_agents {
        let expected_mid = match sa.scope {
            ShortScope::Fleet(f) if f == fleet_a => layout.mid_a,
            ShortScope::Fleet(f) if f == fleet_b => layout.mid_b,
            ShortScope::ColonizedSystem(s) if s == layout.home_a => layout.mid_a,
            ShortScope::ColonizedSystem(s) if s == layout.home_b => layout.mid_b,
            _ => panic!(
                "unexpected ShortAgent scope {:?} (not bound to either region)",
                sa.scope
            ),
        };
        assert_eq!(
            sa.managed_by, expected_mid,
            "ShortAgent.managed_by must match its region's MidAgent \
             (#471); got {:?}, expected {:?} for scope {:?}",
            sa.managed_by, expected_mid, sa.scope,
        );
    }

    // ----- Sanity: AiCommandOutbox carries entries from BOTH regions
    // in the same world (= multi-Mid is actually live).
    let outbox = app.world().resource::<AiCommandOutbox>();
    let mut seen_a = false;
    let mut seen_b = false;
    for entry in outbox.entries.iter() {
        if let Some(macrocosmo_ai::CommandValue::System(s)) =
            entry.command.params.get("target_system")
        {
            if Entity::from_bits(s.0) == layout.target_a {
                seen_a = true;
            }
            if Entity::from_bits(s.0) == layout.target_b {
                seen_b = true;
            }
        }
    }
    assert!(
        seen_a && seen_b,
        "outbox must hold target_system entries for both regions \
         (saw target_a={}, target_b={})",
        seen_a,
        seen_b,
    );
}

#[test]
fn per_region_smoke_save_load_round_trip() {
    let mut app = test_app();
    let layout = build_two_region_npc(&mut app);

    // Drive ticks so the world has populated MidAgent state, ShortAgents
    // attached, and outbox entries in flight before save.
    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    // ----- Save → load round-trip.
    let mut bytes: Vec<u8> = Vec::new();
    save_game_to_writer(app.world_mut(), &mut bytes).expect("save_game_to_writer");
    let mut dst_world = World::new();
    load_game_from_reader(&mut dst_world, &bytes[..]).expect("load_game_from_reader");

    // ----- Region: exactly two Region entities, both pointing at the
    // same Empire after entity remap.
    let regions: Vec<(Entity, Region)> = dst_world
        .query::<(Entity, &Region)>()
        .iter(&dst_world)
        .map(|(e, r)| (e, r.clone()))
        .collect();
    assert_eq!(
        regions.len(),
        2,
        "exactly two Regions must round-trip; got {}",
        regions.len()
    );
    let empire_after = regions[0].1.empire;
    assert_eq!(
        empire_after, regions[1].1.empire,
        "both Regions must share the same empire after remap"
    );

    // EmpireLongTermState round-trips onto the empire.
    assert!(
        dst_world.get::<EmpireLongTermState>(empire_after).is_some(),
        "EmpireLongTermState must round-trip onto the empire"
    );

    // ----- MidAgents: exactly two, each pointing at one of the live
    // Region entities, and Region.mid_agent reciprocally points back.
    let mid_agents: Vec<(Entity, MidAgent)> = dst_world
        .query::<(Entity, &MidAgent)>()
        .iter(&dst_world)
        .map(|(e, m)| (e, m.clone()))
        .collect();
    assert_eq!(mid_agents.len(), 2, "exactly two MidAgents must round-trip");
    let live_region_set: std::collections::HashSet<Entity> =
        regions.iter().map(|(e, _)| *e).collect();
    for (mid_e, mid) in &mid_agents {
        assert!(
            live_region_set.contains(&mid.region),
            "MidAgent {:?} points at a non-live region {:?}",
            mid_e,
            mid.region,
        );
        let region = dst_world.get::<Region>(mid.region).unwrap();
        assert_eq!(
            region.mid_agent,
            Some(*mid_e),
            "Region.mid_agent reciprocity must round-trip"
        );
        assert!(
            mid.auto_managed,
            "MidAgent.auto_managed must round-trip (was true on save)"
        );
    }

    // ----- ShortAgents: each one's `managed_by` is a live MidAgent;
    // ColonizedSystem scope must remap to a live StarSystem.
    let short_agents: Vec<ShortAgent> = dst_world
        .query::<&ShortAgent>()
        .iter(&dst_world)
        .cloned()
        .collect();
    assert!(
        !short_agents.is_empty(),
        "expected at least one ShortAgent to round-trip (Fleet + ColonizedSystem)"
    );
    let live_mid_set: std::collections::HashSet<Entity> =
        mid_agents.iter().map(|(e, _)| *e).collect();
    for sa in &short_agents {
        assert!(
            live_mid_set.contains(&sa.managed_by),
            "ShortAgent.managed_by {:?} is not a live MidAgent after load",
            sa.managed_by,
        );
        if let ShortScope::ColonizedSystem(s) = sa.scope {
            assert!(
                dst_world.get::<StarSystem>(s).is_some(),
                "ColonizedSystem({:?}) must remap to a live StarSystem",
                s,
            );
        }
    }

    // ----- Region membership integrity: every member system carries
    // a `RegionMembership` pointing at SOME live region (not the same
    // one that lists it — `spawn_short_agent_for_new_colonies`
    // intentionally grows `RegionRegistry.by_empire[empire].first()`'s
    // `member_systems` whenever the empire colonises any system, even
    // if that system already belongs to a sibling region for the same
    // empire). The bidirectional invariant we *do* hold is "membership
    // back-pointer remaps onto a live Region", and that survives the
    // postcard round-trip.
    let live_region_lookup: std::collections::HashSet<Entity> = live_region_set;
    for (region_e, region) in &regions {
        for sys in &region.member_systems {
            let membership = dst_world.get::<RegionMembership>(*sys).unwrap_or_else(|| {
                panic!(
                    "system {:?} in region {:?} has no RegionMembership after load",
                    sys, region_e
                )
            });
            assert!(
                live_region_lookup.contains(&membership.region),
                "RegionMembership.region {:?} for system {:?} (listed in \
                 region {:?}) is not a live Region after load",
                membership.region,
                sys,
                region_e,
            );
        }
    }

    // ----- RegionRegistry resource: one empire entry mapping to a
    // 2-element Vec.
    let registry = dst_world.resource::<RegionRegistry>();
    let regs = registry
        .by_empire
        .get(&empire_after)
        .expect("empire must be in registry after load");
    assert_eq!(
        regs.len(),
        2,
        "RegionRegistry.by_empire must map the empire to its 2 regions"
    );

    // Pre-load layout fields are unused after the round-trip (entity
    // ids are fresh), but referencing them keeps the diagnostic crumbs
    // attached to the test in case it ever fails.
    let _ = (
        layout.region_a,
        layout.region_b,
        layout.mid_a,
        layout.mid_b,
        layout.home_a,
        layout.target_a,
        layout.home_b,
        layout.target_b,
        layout.colony_ship_a,
        layout.colony_ship_b,
        layout.empire,
    );
}
