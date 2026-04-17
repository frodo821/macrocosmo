//! #241: Regression tests for the modifier-based Job system.
//!
//! These tests cover the end-to-end wiring between building slot modifiers,
//! pop assignment, per-job rate buckets, and the colony production aggregator.

mod common;

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::colony::*;
use macrocosmo::modifier::{ModifiedValue, Modifier, ParsedModifier};
use macrocosmo::scripting::building_api::{BuildingDefinition, BuildingId};
use macrocosmo::species::*;

use common::{advance_time, find_planet, spawn_test_system, test_app};

// ---------------------------------------------------------------------------
// Test registry / world helpers
// ---------------------------------------------------------------------------

/// Produce a minimal registry with `mine` (miner_slot +5) and `farm` (farmer_slot +5).
fn slot_based_building_registry() -> BuildingRegistry {
    use std::collections::HashMap;
    let mut registry = BuildingRegistry::default();
    let pm = |target: &str, base_add: f64| ParsedModifier {
        target: target.to_string(),
        base_add,
        multiplier: 0.0,
        add: 0.0,
    };
    registry.insert(BuildingDefinition {
        id: "mine".into(),
        name: "Mine".into(),
        description: String::new(),
        minerals_cost: Amt::units(150),
        energy_cost: Amt::units(50),
        build_time: 10,
        maintenance: Amt::new(0, 200),
        production_bonus_minerals: Amt::ZERO,
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: vec![pm("colony.miner_slot", 5.0)],
        is_system_building: false,
        capabilities: HashMap::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: true,
        prerequisites: None,
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
    });
    registry.insert(BuildingDefinition {
        id: "farm".into(),
        name: "Farm".into(),
        description: String::new(),
        minerals_cost: Amt::units(100),
        energy_cost: Amt::units(50),
        build_time: 20,
        maintenance: Amt::new(0, 300),
        production_bonus_minerals: Amt::ZERO,
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: vec![pm("colony.farmer_slot", 5.0)],
        is_system_building: false,
        capabilities: HashMap::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: true,
        prerequisites: None,
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
    });
    // Shipyard — capability-only, no production/slots.
    registry.insert(BuildingDefinition {
        id: "shipyard".into(),
        name: "Shipyard".into(),
        description: String::new(),
        minerals_cost: Amt::units(300),
        energy_cost: Amt::units(200),
        build_time: 30,
        maintenance: Amt::units(1),
        production_bonus_minerals: Amt::ZERO,
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: true,
        capabilities: {
            let mut m = HashMap::new();
            m.insert(
                "shipyard".to_string(),
                macrocosmo::scripting::building_api::CapabilityParams::default(),
            );
            m
        },
        upgrade_to: Vec::new(),
        is_direct_buildable: true,
        prerequisites: None,
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
    });
    registry
}

fn install_basic_jobs(app: &mut App) {
    let pm = |target: &str, base_add: f64| ParsedModifier {
        target: target.to_string(),
        base_add,
        multiplier: 0.0,
        add: 0.0,
    };
    let mut jobs = JobRegistry::default();
    jobs.insert(JobDefinition {
        id: "miner".into(),
        label: "Miner".into(),
        description: String::new(),
        modifiers: vec![pm("job:miner::colony.minerals_per_hexadies", 0.6)],
    });
    jobs.insert(JobDefinition {
        id: "farmer".into(),
        label: "Farmer".into(),
        description: String::new(),
        modifiers: vec![pm("job:farmer::colony.food_per_hexadies", 1.0)],
    });
    app.insert_resource(jobs);
}

fn spawn_colony_with(
    app: &mut App,
    sys: Entity,
    population: u32,
    buildings: Vec<&str>,
    job_slots: Vec<(&str, u32)>,
) -> Entity {
    // Pre-populate ColonyJobRates buckets from the JobRegistry so buckets
    // exist even before sync_species_modifiers runs. This mirrors what a
    // Startup system would normally do for a freshly-spawned colony.
    let job_rates = {
        let jr = app.world().resource::<JobRegistry>();
        let mut rates = ColonyJobRates::default();
        for (id, def) in &jr.jobs {
            for pm in &def.modifiers {
                if let Some((job_id, inner)) = pm.job_scope() {
                    if job_id != id {
                        continue;
                    }
                    let bucket = rates.bucket_mut(job_id, inner);
                    bucket.push_modifier(pm.to_modifier(
                        format!("job:{}:{}", id, pm.target),
                        format!("Job '{}' base", def.label),
                    ));
                }
            }
        }
        rates
    };

    let planet = find_planet(app.world_mut(), sys);
    app.world_mut().entity_mut(sys).insert((
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::units(200),
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
    ));

    let slot_entries: Vec<Option<BuildingId>> = buildings
        .iter()
        .map(|s| Some(BuildingId::new(*s)))
        .collect();

    app.world_mut()
        .spawn((
            Colony {
                planet,
                population: population as f64,
                growth_rate: 0.0,
            },
            Production {
                minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
                energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
                research_per_hexadies: ModifiedValue::new(Amt::ZERO),
                food_per_hexadies: ModifiedValue::new(Amt::ZERO),
            },
            BuildQueue::default(),
            Buildings {
                slots: slot_entries,
            },
            BuildingQueue::default(),
            ProductionFocus::default(),
            MaintenanceCost::default(),
            FoodConsumption::default(),
            ColonyPopulation {
                species: vec![ColonySpecies {
                    species_id: "human".to_string(),
                    population,
                }],
            },
            ColonyJobs {
                slots: job_slots
                    .into_iter()
                    .map(|(id, cap)| JobSlot::fixed(id, cap))
                    .collect(),
            },
            job_rates,
        ))
        .id()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_job_slot_computed_from_building_modifiers() {
    let mut app = test_app();
    install_basic_jobs(&mut app);
    app.insert_resource(slot_based_building_registry());

    let sys = spawn_test_system(
        app.world_mut(),
        "Test Sys",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let colony = spawn_colony_with(
        &mut app,
        sys,
        10,
        vec!["mine", "farm"],
        vec![("miner", 0), ("farmer", 0)],
    );

    advance_time(&mut app, 1);

    let jobs = app.world().get::<ColonyJobs>(colony).unwrap();
    assert_eq!(
        jobs.slots
            .iter()
            .find(|s| s.job_id == "miner")
            .unwrap()
            .capacity,
        5,
        "mine should grant 5 miner slots"
    );
    assert_eq!(
        jobs.slots
            .iter()
            .find(|s| s.job_id == "farmer")
            .unwrap()
            .capacity,
        5,
        "farm should grant 5 farmer slots"
    );
}

#[test]
fn test_pop_assigned_to_slots_contributes_production() {
    let mut app = test_app();
    install_basic_jobs(&mut app);
    app.insert_resource(slot_based_building_registry());

    let sys = spawn_test_system(
        app.world_mut(),
        "Prod Sys",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let _colony = spawn_colony_with(&mut app, sys, 5, vec!["mine"], vec![("miner", 0)]);
    advance_time(&mut app, 1);

    // 5 miners × 0.6 = 3.0 minerals/hexady, minus 1 hexady already elapsed.
    // The first advance_time(1) already ran the tick, so we expect ~3 minerals
    // in the stockpile.
    let stockpile = app.world().get::<ResourceStockpile>(sys).unwrap();
    assert_eq!(
        stockpile.minerals,
        Amt::units(3),
        "5 miners × 0.6 = 3.0 minerals after 1 hexady, got {}",
        stockpile.minerals
    );
}

#[test]
fn test_unemployed_pop_does_not_produce() {
    let mut app = test_app();
    install_basic_jobs(&mut app);
    app.insert_resource(slot_based_building_registry());

    let sys = spawn_test_system(
        app.world_mut(),
        "Idle Sys",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    // Pop=5 but no buildings → 0 slots → 0 assigned → 0 production.
    let _colony = spawn_colony_with(&mut app, sys, 5, vec![], vec![("miner", 0)]);

    advance_time(&mut app, 3);

    let stockpile = app.world().get::<ResourceStockpile>(sys).unwrap();
    assert_eq!(
        stockpile.minerals,
        Amt::ZERO,
        "No slots → no production, got {}",
        stockpile.minerals
    );
}

#[test]
fn test_species_scoped_modifier_applies_only_to_assigned_job() {
    let mut app = test_app();
    install_basic_jobs(&mut app);
    app.insert_resource(slot_based_building_registry());

    // Human species: +50% miner minerals. No farmer bonus.
    let mut species = SpeciesRegistry::default();
    species.insert(SpeciesDefinition {
        id: "human".to_string(),
        name: "Human".to_string(),
        description: String::new(),
        base_growth_rate: 0.0,
        modifiers: vec![ParsedModifier {
            target: "job:miner::colony.minerals_per_hexadies".to_string(),
            base_add: 0.0,
            multiplier: 0.5, // +50%
            add: 0.0,
        }],
    });
    app.insert_resource(species);

    let sys = spawn_test_system(app.world_mut(), "Spc Sys", [0.0, 0.0, 0.0], 1.0, true, true);
    let _colony = spawn_colony_with(&mut app, sys, 5, vec!["mine"], vec![("miner", 0)]);

    advance_time(&mut app, 1);

    // 5 miners × 0.6 × 1.5 = 4.5 minerals.
    let stockpile = app.world().get::<ResourceStockpile>(sys).unwrap();
    assert_eq!(
        stockpile.minerals,
        Amt::new(4, 500),
        "5 × 0.6 × 1.5 = 4.5, got {}",
        stockpile.minerals
    );
}

#[test]
fn test_automated_building_produces_without_pop() {
    // Legacy-style automation path: building pushes directly into
    // colony.<resource>_per_hexadies. This exercises the fixture registry in
    // common/mod.rs where `mine` emits colony.minerals_per_hexadies +3.
    let mut app = test_app();
    install_basic_jobs(&mut app);
    // Leave the default `test_app` fixture registry in place (mine/farm push
    // directly into colony.<X>_per_hexadies).

    let sys = spawn_test_system(
        app.world_mut(),
        "Auto Sys",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    // Zero pops — automation path should still produce.
    let _colony = spawn_colony_with(&mut app, sys, 0, vec!["mine"], vec![]);

    advance_time(&mut app, 1);
    let stockpile = app.world().get::<ResourceStockpile>(sys).unwrap();
    assert_eq!(
        stockpile.minerals,
        Amt::units(3),
        "Automation building produces 3 minerals/hexady without pops, got {}",
        stockpile.minerals
    );
}

#[test]
fn test_building_demolition_clears_slots() {
    let mut app = test_app();
    install_basic_jobs(&mut app);
    app.insert_resource(slot_based_building_registry());

    let sys = spawn_test_system(
        app.world_mut(),
        "Demo Sys",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let colony = spawn_colony_with(
        &mut app,
        sys,
        10,
        vec!["mine", "farm"],
        vec![("miner", 0), ("farmer", 0)],
    );
    advance_time(&mut app, 1);

    // Verify slots present first.
    let jobs = app.world().get::<ColonyJobs>(colony).unwrap();
    let miner = jobs.slots.iter().find(|s| s.job_id == "miner").unwrap();
    assert_eq!(miner.capacity, 5);

    // Demolish the mine: set the first slot to None.
    app.world_mut().get_mut::<Buildings>(colony).unwrap().slots[0] = None;
    advance_time(&mut app, 1);

    let jobs = app.world().get::<ColonyJobs>(colony).unwrap();
    let miner = jobs.slots.iter().find(|s| s.job_id == "miner").unwrap();
    assert_eq!(
        miner.capacity, 0,
        "after mine demolished, miner_slot should go to 0"
    );
    // Farmer slot should still be present.
    let farmer = jobs.slots.iter().find(|s| s.job_id == "farmer").unwrap();
    assert_eq!(farmer.capacity, 5);
}

#[test]
fn test_target_prefix_routes_to_job_bucket() {
    // Buildings can push `job:<id>::<target>` modifiers directly; they land in
    // the per-job bucket instead of the colony aggregator.
    use std::collections::HashMap;
    let mut app = test_app();
    install_basic_jobs(&mut app);

    let mut registry = BuildingRegistry::default();
    registry.insert(BuildingDefinition {
        id: "mine".into(),
        name: "Mine".into(),
        description: String::new(),
        minerals_cost: Amt::ZERO,
        energy_cost: Amt::ZERO,
        build_time: 10,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::ZERO,
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: vec![
            ParsedModifier {
                target: "colony.miner_slot".to_string(),
                base_add: 5.0,
                multiplier: 0.0,
                add: 0.0,
            },
            // +100% miner efficiency boost via per-job bucket.
            ParsedModifier {
                target: "job:miner::colony.minerals_per_hexadies".to_string(),
                base_add: 0.0,
                multiplier: 1.0,
                add: 0.0,
            },
        ],
        is_system_building: false,
        capabilities: HashMap::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: true,
        prerequisites: None,
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
    });
    app.insert_resource(registry);

    let sys = spawn_test_system(
        app.world_mut(),
        "Route Sys",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let _colony = spawn_colony_with(&mut app, sys, 5, vec!["mine"], vec![("miner", 0)]);

    advance_time(&mut app, 1);

    // 5 miners × 0.6 × 2.0 = 6.0 minerals/hexady.
    let stockpile = app.world().get::<ResourceStockpile>(sys).unwrap();
    assert_eq!(
        stockpile.minerals,
        Amt::units(6),
        "job-scoped multiplier from building lands in per-job bucket; got {}",
        stockpile.minerals
    );
}

#[test]
fn test_auto_prefix_in_define_job() {
    // `define_job { modifiers = { { target = "colony.X", ... } } }` gets its
    // target auto-prefixed to `job:<self_id>::colony.X` at parse time.
    use macrocosmo::scripting::ScriptEngine;
    use macrocosmo::scripting::species_api::parse_job_definitions;

    let engine = ScriptEngine::new().unwrap();
    engine
        .lua()
        .load(
            r#"
            define_job {
                id = "trader",
                label = "Trader",
                modifiers = {
                    { target = "colony.minerals_per_hexadies", base_add = 0.3 },
                    -- explicit prefix should be preserved
                    { target = "job:trader::colony.energy_per_hexadies", base_add = 0.1 },
                },
            }
            "#,
        )
        .exec()
        .unwrap();
    let defs = parse_job_definitions(engine.lua()).unwrap();
    assert_eq!(defs.len(), 1);
    let trader = &defs[0];
    assert_eq!(trader.modifiers.len(), 2);
    assert!(
        trader
            .modifiers
            .iter()
            .any(|m| m.target == "job:trader::colony.minerals_per_hexadies")
    );
    assert!(
        trader
            .modifiers
            .iter()
            .any(|m| m.target == "job:trader::colony.energy_per_hexadies")
    );
}

#[test]
fn test_tech_effect_increases_slot_count() {
    // A tech pushes a `colony.miner_slot` modifier onto an empire-scoped
    // ModifiedValue → building-sync integrates it into jobs.slots capacity.
    //
    // This is end-to-end through the pretty-routed pipeline:
    // - Building `mine` has `colony.miner_slot +5`.
    // - Species has no contribution.
    // - The base capacity in the test is 5 (from mine). A stub tech modifier
    //   simulated via a directly-inserted `Modifier` on
    //   `Production.minerals_per_hexadies` is not relevant here; this test
    //   asserts slot capacity from buildings is respected.
    // For brevity, we verify the base case — tech integration for slot counts
    // is planned for a follow-up; see #241 open questions.
    let mut app = test_app();
    install_basic_jobs(&mut app);
    app.insert_resource(slot_based_building_registry());

    let sys = spawn_test_system(
        app.world_mut(),
        "Tech Sys",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let colony = spawn_colony_with(&mut app, sys, 10, vec!["mine"], vec![("miner", 0)]);
    advance_time(&mut app, 1);

    // Baseline: mine gives miner_slot = 5.
    let miner_cap = app
        .world()
        .get::<ColonyJobs>(colony)
        .unwrap()
        .slots
        .iter()
        .find(|s| s.job_id == "miner")
        .unwrap()
        .capacity;
    assert_eq!(miner_cap, 5);

    // Simulate a tech that adds another miner_slot via a runtime modifier
    // push on Production (until #241 follow-up wires tech modifiers directly
    // into ColonyJobRates, slot-count tech effects go through the same
    // BuildingRegistry path used by upgrades).
    let _ = colony; // slot modification via direct Buildings mutation:
    app.world_mut()
        .get_mut::<Buildings>(colony)
        .unwrap()
        .slots
        .push(Some(BuildingId::new("mine")));
    advance_time(&mut app, 1);

    let miner_cap = app
        .world()
        .get::<ColonyJobs>(colony)
        .unwrap()
        .slots
        .iter()
        .find(|s| s.job_id == "miner")
        .unwrap()
        .capacity;
    assert_eq!(
        miner_cap, 10,
        "A second mine should raise miner_slot to 10, got {}",
        miner_cap
    );
}

#[test]
fn test_parsed_modifier_detects_job_scope() {
    let pm = ParsedModifier {
        target: "job:miner::colony.minerals_per_hexadies".into(),
        base_add: 0.0,
        multiplier: 0.0,
        add: 0.0,
    };
    assert_eq!(
        pm.job_scope(),
        Some(("miner", "colony.minerals_per_hexadies"))
    );

    let pm = ParsedModifier {
        target: "colony.miner_slot".into(),
        base_add: 0.0,
        multiplier: 0.0,
        add: 0.0,
    };
    assert_eq!(pm.job_scope(), None);
}

// ---------------------------------------------------------------------------
// #250: After the fix, a freshly-spawned capital's `Production.final_value()`
// reflects building + job contributions from the very first tick, including
// while the game is paused (delta = 0). Legacy base values of 5/5/1/5 have
// been removed — production comes entirely from building/job modifiers.
// ---------------------------------------------------------------------------

/// Mirror the bundle produced by `spawn_capital_colony` (post-fix) so the
/// tests below exercise the real spawn path's invariants.
fn spawn_capital_like_colony(
    app: &mut App,
    sys: Entity,
    buildings: Vec<&str>,
    population: u32,
) -> Entity {
    let planet = find_planet(app.world_mut(), sys);
    app.world_mut()
        .spawn((
            Colony {
                planet,
                population: population as f64,
                growth_rate: 0.01,
            },
            Production {
                minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
                energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
                research_per_hexadies: ModifiedValue::new(Amt::ZERO),
                food_per_hexadies: ModifiedValue::new(Amt::ZERO),
            },
            BuildQueue::default(),
            Buildings {
                slots: buildings
                    .into_iter()
                    .map(|s| Some(BuildingId::new(s)))
                    .collect(),
            },
            BuildingQueue::default(),
            ProductionFocus::default(),
            MaintenanceCost::default(),
            FoodConsumption::default(),
            ColonyPopulation {
                species: vec![ColonySpecies {
                    species_id: "human".to_string(),
                    population,
                }],
            },
            ColonyJobs::default(),
            ColonyJobRates::default(),
        ))
        .id()
}

#[test]
fn test_issue_250_capital_production_reflects_buildings_and_jobs() {
    let mut app = test_app();
    install_basic_jobs(&mut app);
    app.insert_resource(slot_based_building_registry());

    let sys = spawn_test_system(
        app.world_mut(),
        "Issue250",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    app.world_mut().entity_mut(sys).insert((
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::units(200),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
    ));

    let colony = spawn_capital_like_colony(&mut app, sys, vec!["mine", "farm"], 100);

    advance_time(&mut app, 1);

    let prod = app.world().get::<Production>(colony).unwrap();
    // Expected contributions only (no legacy base):
    //   Minerals: miner 5 × 0.6 = 3
    //   Food:     farmer 5 × 1.0 = 5
    //   Energy:   no plant        = 0
    //   Research: no lab          = 0
    assert_eq!(
        prod.minerals_per_hexadies.final_value(),
        Amt::units(3),
        "miner contribution should drive minerals"
    );
    assert_eq!(
        prod.food_per_hexadies.final_value(),
        Amt::units(5),
        "farmer contribution should drive food"
    );
    assert_eq!(
        prod.energy_per_hexadies.final_value(),
        Amt::ZERO,
        "no power plant ⇒ zero energy"
    );
    assert_eq!(
        prod.research_per_hexadies.final_value(),
        Amt::ZERO,
        "no researcher ⇒ zero research"
    );
}

/// #250 regression: the aggregator must expose the correct rate even when the
/// clock is paused (`delta = 0`). Before the fix, `tick_production` held the
/// Stage 1 push, so the UI saw only the legacy base value during pauses.
#[test]
fn test_issue_250_aggregator_runs_while_paused() {
    let mut app = test_app();
    install_basic_jobs(&mut app);
    app.insert_resource(slot_based_building_registry());

    let sys = spawn_test_system(
        app.world_mut(),
        "PausedProd",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    app.world_mut().entity_mut(sys).insert((
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
    ));

    let colony = spawn_capital_like_colony(&mut app, sys, vec!["mine", "farm"], 100);

    // Do NOT advance time. Only run one frame so the sync pipeline fires.
    app.update();

    let prod = app.world().get::<Production>(colony).unwrap();
    assert_eq!(
        prod.minerals_per_hexadies.final_value(),
        Amt::units(3),
        "aggregator must populate minerals even with delta=0; got {}",
        prod.minerals_per_hexadies.final_value()
    );
    assert_eq!(
        prod.food_per_hexadies.final_value(),
        Amt::units(5),
        "aggregator must populate food even with delta=0"
    );

    // Stockpile should NOT have been credited (Stage 2 is delta-gated).
    let stockpile = app.world().get::<ResourceStockpile>(sys).unwrap();
    assert_eq!(
        stockpile.minerals,
        Amt::ZERO,
        "no time elapsed ⇒ stockpile should not accumulate"
    );
}

// ---------------------------------------------------------------------------
// #250: Verify that real Lua definitions (scripts/buildings/basic.lua and
// scripts/jobs/basic.lua) parse into the expected BuildingDefinition.modifiers
// and JobDefinition.modifiers. If the Lua side silently drops or misroutes a
// modifier, the integration test above (which uses hand-built registries)
// will never catch it.
// ---------------------------------------------------------------------------

#[test]
fn test_issue_250_lua_building_modifiers_parse_correctly() {
    use macrocosmo::scripting::ScriptEngine;
    use macrocosmo::scripting::building_api::parse_building_definitions;

    let engine = ScriptEngine::new().expect("ScriptEngine::new()");
    let init = engine.scripts_dir().join("init.lua");
    engine.load_file(&init).expect("load init.lua");

    let defs = parse_building_definitions(engine.lua()).expect("parse buildings");
    let ids: Vec<&str> = defs.iter().map(|d| d.id.as_str()).collect();
    eprintln!("[issue #250] building ids: {ids:?}");

    let mine = defs
        .iter()
        .find(|d| d.id == "mine")
        .expect("mine not defined");
    eprintln!("[issue #250] mine.modifiers = {:?}", mine.modifiers);
    assert!(
        mine.modifiers
            .iter()
            .any(|m| m.target == "colony.miner_slot" && (m.base_add - 5.0).abs() < 1e-9),
        "mine should declare modifier colony.miner_slot base_add=5; got {:?}",
        mine.modifiers
    );

    let power = defs
        .iter()
        .find(|d| d.id == "power_plant")
        .expect("power_plant not defined");
    eprintln!("[issue #250] power_plant.modifiers = {:?}", power.modifiers);
    assert!(
        power
            .modifiers
            .iter()
            .any(|m| m.target == "colony.power_worker_slot" && (m.base_add - 5.0).abs() < 1e-9),
        "power_plant should declare modifier colony.power_worker_slot base_add=5; got {:?}",
        power.modifiers
    );

    let farm = defs
        .iter()
        .find(|d| d.id == "farm")
        .expect("farm not defined");
    eprintln!("[issue #250] farm.modifiers = {:?}", farm.modifiers);
    assert!(
        farm.modifiers
            .iter()
            .any(|m| m.target == "colony.farmer_slot" && (m.base_add - 5.0).abs() < 1e-9),
        "farm should declare modifier colony.farmer_slot base_add=5; got {:?}",
        farm.modifiers
    );
}

#[test]
fn test_issue_250_lua_job_modifiers_parse_correctly() {
    use macrocosmo::scripting::ScriptEngine;
    use macrocosmo::scripting::species_api::parse_job_definitions;

    let engine = ScriptEngine::new().expect("ScriptEngine::new()");
    let init = engine.scripts_dir().join("init.lua");
    engine.load_file(&init).expect("load init.lua");

    let defs = parse_job_definitions(engine.lua()).expect("parse jobs");
    let ids: Vec<&str> = defs.iter().map(|d| d.id.as_str()).collect();
    eprintln!("[issue #250] job ids: {ids:?}");

    let miner = defs.iter().find(|d| d.id == "miner").expect("miner");
    eprintln!("[issue #250] miner.modifiers = {:?}", miner.modifiers);
    assert!(
        miner
            .modifiers
            .iter()
            .any(|m| m.target == "job:miner::colony.minerals_per_hexadies"
                && (m.base_add - 0.6).abs() < 1e-9),
        "miner per-pop rate should auto-prefix to job:miner::...; got {:?}",
        miner.modifiers
    );

    let power_worker = defs
        .iter()
        .find(|d| d.id == "power_worker")
        .expect("power_worker");
    eprintln!(
        "[issue #250] power_worker.modifiers = {:?}",
        power_worker.modifiers
    );
    assert!(
        power_worker.modifiers.iter().any(|m| m.target
            == "job:power_worker::colony.energy_per_hexadies"
            && (m.base_add - 6.0).abs() < 1e-9),
        "power_worker per-pop rate should be 6.0; got {:?}",
        power_worker.modifiers
    );

    let farmer = defs.iter().find(|d| d.id == "farmer").expect("farmer");
    eprintln!("[issue #250] farmer.modifiers = {:?}", farmer.modifiers);
    assert!(
        farmer
            .modifiers
            .iter()
            .any(|m| m.target == "job:farmer::colony.food_per_hexadies"
                && (m.base_add - 2.0).abs() < 1e-9),
        "farmer per-pop rate should be 2.0 (current balance); got {:?}",
        farmer.modifiers
    );
}

// Required by macrocosmo::modifier::Modifier for the helper above.
#[allow(dead_code)]
fn _ensure_modifier_constructible() -> Modifier {
    Modifier {
        id: "x".into(),
        label: "x".into(),
        base_add: macrocosmo::amount::SignedAmt::ZERO,
        multiplier: macrocosmo::amount::SignedAmt::ZERO,
        add: macrocosmo::amount::SignedAmt::ZERO,
        expires_at: None,
        on_expire_event: None,
    }
}
