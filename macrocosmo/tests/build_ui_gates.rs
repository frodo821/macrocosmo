//! #436 + #437: Regression tests for the two build-UI gate bugs.
//!
//! - #437: `BuildingDefinition.prerequisites` is honoured both in the UI
//!   filter (`available_planet_buildings` / `available_system_buildings`)
//!   *and* in the arrival-side handler (`apply_building_command`). Without
//!   both, a player could bypass the gate by dispatching a raw
//!   `PendingColonyDispatch` (scripted bot, remote RPC, etc.), or by
//!   researching a tech after dispatching and expecting the filter to
//!   catch up.
//!
//! - #436: Ownership-based UI gating is covered indirectly via the
//!   `prerequisites_satisfied` helper — the UI layer additionally calls
//!   an `is_own_system` check that lives above the helper boundary. The
//!   UI-level gate itself is not exercised under egui in tests (see
//!   `CLAUDE.md` — egui systems are excluded from headless tests); we
//!   instead test the boundary primitive that both UI and backend share.

mod common;

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use macrocosmo::amount::Amt;
use macrocosmo::colony::{BuildingQueue, SystemBuildingQueue};
use macrocosmo::communication::{
    self, BuildingKind, BuildingScope, ColonyCommand, CommandLog, PendingCommand, RemoteCommand,
};
use macrocosmo::condition::{Condition, ConditionAtom, EvalContext, ScopedFlags};
use macrocosmo::scripting::building_api::{BuildingDefinition, BuildingId, BuildingRegistry};
use macrocosmo::technology::{GameFlags, TechTree};

use common::{
    advance_time, empire_entity, spawn_test_colony, spawn_test_system_with_planet, test_app,
};

// --------------------------------------------------------------------------
// Shared helpers
// --------------------------------------------------------------------------

/// Build an app with the minimal remote-command processing pipeline wired
/// in. Matches the shape used in `remote_colony_commands.rs`.
fn build_app() -> App {
    let mut app = test_app();
    app.add_systems(
        Update,
        communication::process_pending_commands.after(macrocosmo::time_system::advance_game_time),
    );
    app
}

/// Spawn a `PendingCommand` directly — bypasses the dispatcher so the
/// test can simulate arrival without setting up full light-speed
/// geometry. Mirrors the helper in `remote_colony_commands.rs`.
fn spawn_pending(app: &mut App, target_system: Entity, arrives_at: i64, cmd: RemoteCommand) {
    app.world_mut().spawn(PendingCommand {
        id: macrocosmo::ship::command_events::CommandId::ZERO,
        target_system,
        command: cmd,
        sent_at: 0,
        arrives_at,
        origin_pos: [0.0, 0.0, 0.0],
        destination_pos: [0.0, 0.0, 0.0],
    });
    // Give the arrival handler a CommandLog entry to mark arrived —
    // unnecessary for the queue assertions but mirrors production.
    let empire = empire_entity(app.world_mut());
    if let Some(mut log) = app.world_mut().get_mut::<CommandLog>(empire) {
        log.entries
            .push(communication::CommandLogEntry::new_pending(
                "test".to_string(),
                0,
                arrives_at,
            ));
    }
}

/// Step the clock from 0 to `arrives_at` in one frame, keeping
/// `LastProductionTick` pinned so `tick_build_queue` sees `delta = 1`
/// (same trick as `remote_colony_commands::run_until_arrival`). Without
/// the pin, a long fast-forward would let the build-tick loop complete
/// arbitrarily cheap orders before our post-arrival assertion.
fn run_until_arrival(app: &mut App, arrives_at: i64) {
    let pre = arrives_at - 1;
    app.world_mut()
        .resource_mut::<macrocosmo::time_system::GameClock>()
        .elapsed = pre;
    app.world_mut()
        .resource_mut::<macrocosmo::colony::LastProductionTick>()
        .0 = pre;
    advance_time(app, 1);
}

/// Install a custom BuildingRegistry holding a `gated_mine` (planet-level)
/// and `gated_shipyard` (system-level, upgrade from `shipyard`) whose
/// `prerequisites` require tech `"warp_drive"`. Plus the standard `mine`
/// and `shipyard` so existing `spawn_test_colony` helpers keep working.
fn install_gated_registry(world: &mut World) {
    let mut registry = BuildingRegistry::default();
    // Planet-level gated building.
    registry.insert(BuildingDefinition {
        id: "gated_mine".into(),
        name: "Gated Mine".into(),
        description: String::new(),
        minerals_cost: Amt::units(100),
        energy_cost: Amt::units(50),
        build_time: 10,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::units(5),
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: false,
        capabilities: HashMap::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: true,
        prerequisites: Some(Condition::Atom(ConditionAtom::has_tech("warp_drive"))),
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
        ship_design_id: None,
        colony_slots: None,
    });
    // System-level gated building.
    registry.insert(BuildingDefinition {
        id: "gated_shipyard".into(),
        name: "Gated Shipyard".into(),
        description: String::new(),
        minerals_cost: Amt::units(500),
        energy_cost: Amt::units(300),
        build_time: 40,
        maintenance: Amt::new(0, 500),
        production_bonus_minerals: Amt::ZERO,
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: true,
        capabilities: HashMap::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: true,
        prerequisites: Some(Condition::Atom(ConditionAtom::has_tech("warp_drive"))),
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
        ship_design_id: None,
        colony_slots: None,
    });
    // Un-gated fallback so the existing test `mine` / `shipyard` ids keep
    // working for spawning helpers used in other tests inside this module.
    registry.insert(BuildingDefinition {
        id: "mine".into(),
        name: "Mine".into(),
        description: String::new(),
        minerals_cost: Amt::units(100),
        energy_cost: Amt::units(50),
        build_time: 10,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::units(5),
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: false,
        capabilities: HashMap::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: true,
        prerequisites: None,
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
        ship_design_id: None,
        colony_slots: None,
    });
    world.insert_resource(registry);
}

/// Mark `tech_id` as researched on the test empire's `TechTree`.
fn research_tech(world: &mut World, empire: Entity, tech_id: &str) {
    if let Some(mut tree) = world.get_mut::<TechTree>(empire) {
        tree.researched
            .insert(macrocosmo::technology::TechId(tech_id.to_string()));
    }
}

// --------------------------------------------------------------------------
// #437 — UI filter helpers (pure functions over BuildingRegistry + EvalContext)
// --------------------------------------------------------------------------

#[test]
fn available_planet_buildings_filters_by_prerequisites() {
    let mut registry = BuildingRegistry::default();
    registry.insert(BuildingDefinition {
        id: "locked".into(),
        name: "Locked".into(),
        description: String::new(),
        minerals_cost: Amt::units(10),
        energy_cost: Amt::units(10),
        build_time: 1,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::ZERO,
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: false,
        capabilities: HashMap::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: true,
        prerequisites: Some(Condition::Atom(ConditionAtom::has_tech("future_tech"))),
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
        ship_design_id: None,
        colony_slots: None,
    });
    registry.insert(BuildingDefinition {
        id: "unlocked".into(),
        name: "Unlocked".into(),
        description: String::new(),
        minerals_cost: Amt::units(10),
        energy_cost: Amt::units(10),
        build_time: 1,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::ZERO,
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: false,
        capabilities: HashMap::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: true,
        prerequisites: None,
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
        ship_design_id: None,
        colony_slots: None,
    });

    let empty: HashSet<String> = HashSet::new();
    let ctx_without = EvalContext::flat(&empty, &empty, &empty, &empty);

    let with_tech: HashSet<String> = ["future_tech".to_string()].into_iter().collect();
    let ctx_with = EvalContext::flat(&with_tech, &empty, &empty, &empty);

    // Without the tech, only `unlocked` appears.
    let filtered_without = registry.available_planet_buildings(&ctx_without);
    let ids_without: Vec<&str> = filtered_without.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(
        ids_without,
        vec!["unlocked"],
        "gated building must be hidden when prerequisites unsatisfied"
    );

    // With the tech, both appear.
    let filtered_with = registry.available_planet_buildings(&ctx_with);
    let mut ids_with: Vec<&str> = filtered_with.iter().map(|d| d.id.as_str()).collect();
    ids_with.sort();
    assert_eq!(
        ids_with,
        vec!["locked", "unlocked"],
        "gated building must appear once prerequisites are satisfied"
    );
}

#[test]
fn available_system_buildings_filters_by_prerequisites() {
    let mut registry = BuildingRegistry::default();
    registry.insert(BuildingDefinition {
        id: "advanced_port".into(),
        name: "Advanced Port".into(),
        description: String::new(),
        minerals_cost: Amt::units(100),
        energy_cost: Amt::units(100),
        build_time: 10,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::ZERO,
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: true,
        capabilities: HashMap::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: true,
        prerequisites: Some(Condition::Atom(ConditionAtom::has_tech("jump_drive"))),
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
        ship_design_id: None,
        colony_slots: None,
    });

    let empty: HashSet<String> = HashSet::new();
    let ctx_without = EvalContext::flat(&empty, &empty, &empty, &empty);
    let with_tech: HashSet<String> = ["jump_drive".to_string()].into_iter().collect();
    let ctx_with = EvalContext::flat(&with_tech, &empty, &empty, &empty);

    assert!(
        registry.available_system_buildings(&ctx_without).is_empty(),
        "gated system building must be hidden without the tech"
    );
    assert_eq!(
        registry.available_system_buildings(&ctx_with).len(),
        1,
        "gated system building must appear with the tech"
    );
}

#[test]
fn prerequisites_satisfied_handles_all_three_cases() {
    let mut registry = BuildingRegistry::default();
    // No prerequisites — always satisfied.
    registry.insert(BuildingDefinition {
        id: "free".into(),
        name: "Free".into(),
        description: String::new(),
        minerals_cost: Amt::ZERO,
        energy_cost: Amt::ZERO,
        build_time: 1,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::ZERO,
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: false,
        capabilities: HashMap::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: true,
        prerequisites: None,
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
        ship_design_id: None,
        colony_slots: None,
    });
    registry.insert(BuildingDefinition {
        id: "gated".into(),
        name: "Gated".into(),
        description: String::new(),
        minerals_cost: Amt::ZERO,
        energy_cost: Amt::ZERO,
        build_time: 1,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::ZERO,
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: false,
        capabilities: HashMap::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: true,
        prerequisites: Some(Condition::Atom(ConditionAtom::has_tech("some_tech"))),
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
        ship_design_id: None,
        colony_slots: None,
    });

    let empty: HashSet<String> = HashSet::new();
    let ctx = EvalContext::flat(&empty, &empty, &empty, &empty);

    assert!(
        registry.prerequisites_satisfied("free", &ctx),
        "no-prerequisite building should be buildable"
    );
    assert!(
        !registry.prerequisites_satisfied("gated", &ctx),
        "gated building must fail when tech missing"
    );
    assert!(
        !registry.prerequisites_satisfied("unknown", &ctx),
        "unknown id must never be buildable"
    );
}

// --------------------------------------------------------------------------
// #437 — Arrival-side validation
// --------------------------------------------------------------------------

#[test]
fn queue_building_rejected_when_prerequisites_unmet_on_arrival() {
    // The UI filter can be bypassed (scripted/remote dispatch). The
    // arrival handler MUST re-check `prerequisites` against the empire's
    // current tech state — otherwise light-speed delay becomes a free
    // tech-gate skip.
    let mut app = build_app();
    install_gated_registry(app.world_mut());

    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1_000),
        Amt::units(1_000),
        vec![None, None, None, None],
    );

    // Dispatch a `Queue` command for the gated building WITHOUT researching
    // the tech. This simulates an attacker bypassing the UI filter.
    spawn_pending(
        &mut app,
        sys,
        10,
        RemoteCommand::Colony(ColonyCommand {
            scope: BuildingScope::Planet(planet),
            kind: BuildingKind::Queue {
                building_id: "gated_mine".to_string(),
                target_slot: 0,
            },
        }),
    );

    run_until_arrival(&mut app, 10);

    let bq = app.world().get::<BuildingQueue>(colony).unwrap();
    assert!(
        bq.queue.is_empty(),
        "prerequisites-unmet Queue command must be rejected at arrival, not enqueued"
    );
}

#[test]
fn queue_building_accepted_when_prerequisites_met_on_arrival() {
    // Counterpart to the rejection case: once the tech is researched the
    // same command succeeds. Guards against the validation being too
    // aggressive (e.g. rejecting every gated building unconditionally).
    let mut app = build_app();
    install_gated_registry(app.world_mut());

    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1_000),
        Amt::units(1_000),
        vec![None, None, None, None],
    );

    let empire = empire_entity(app.world_mut());
    research_tech(app.world_mut(), empire, "warp_drive");

    spawn_pending(
        &mut app,
        sys,
        10,
        RemoteCommand::Colony(ColonyCommand {
            scope: BuildingScope::Planet(planet),
            kind: BuildingKind::Queue {
                building_id: "gated_mine".to_string(),
                target_slot: 0,
            },
        }),
    );

    run_until_arrival(&mut app, 10);

    let bq = app.world().get::<BuildingQueue>(colony).unwrap();
    assert_eq!(
        bq.queue.len(),
        1,
        "prerequisites-satisfied Queue command must enqueue normally"
    );
    assert_eq!(bq.queue[0].building_id.as_str(), "gated_mine");
}

#[test]
fn queue_system_building_rejected_when_prerequisites_unmet() {
    // Same gate for system-level buildings. Uses a gated system
    // building + a CoreShip so the #370 core check doesn't mask the
    // prerequisite rejection.
    let mut app = build_app();
    install_gated_registry(app.world_mut());

    let (sys, _planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let _colony = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::units(1_000),
        Amt::units(1_000),
        vec![],
    );
    // #370: system buildings require a CoreShip in the target system.
    let empire = empire_entity(app.world_mut());
    app.world_mut().spawn((
        macrocosmo::ship::CoreShip,
        macrocosmo::galaxy::AtSystem(sys),
        macrocosmo::faction::FactionOwner(empire),
    ));

    spawn_pending(
        &mut app,
        sys,
        10,
        RemoteCommand::Colony(ColonyCommand {
            scope: BuildingScope::System,
            kind: BuildingKind::Queue {
                building_id: "gated_shipyard".to_string(),
                target_slot: 0,
            },
        }),
    );

    run_until_arrival(&mut app, 10);

    let sbq = app.world().get::<SystemBuildingQueue>(sys).unwrap();
    assert!(
        sbq.queue.is_empty(),
        "prerequisites-unmet system-level Queue command must be rejected"
    );
}

#[test]
fn upgrade_building_rejected_when_target_prerequisites_unmet() {
    // Upgrades re-use `BuildingDefinition.prerequisites` on the target id.
    // Without the gate, an un-upgradeable tech lock could be skipped by
    // whichever source building holds a matching `upgrade_to` path.
    let mut app = build_app();

    // Custom registry with a mine that upgrades to a gated advanced_mine.
    let mut registry = BuildingRegistry::default();
    registry.insert(BuildingDefinition {
        id: "basic_mine".into(),
        name: "Basic Mine".into(),
        description: String::new(),
        minerals_cost: Amt::units(100),
        energy_cost: Amt::units(50),
        build_time: 10,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::units(5),
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: false,
        capabilities: HashMap::new(),
        upgrade_to: vec![macrocosmo::scripting::building_api::UpgradePath {
            target_id: "advanced_mine".to_string(),
            cost_minerals: Amt::units(200),
            cost_energy: Amt::units(100),
            build_time: Some(5),
        }],
        is_direct_buildable: true,
        prerequisites: None,
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
        ship_design_id: None,
        colony_slots: None,
    });
    registry.insert(BuildingDefinition {
        id: "advanced_mine".into(),
        name: "Advanced Mine".into(),
        description: String::new(),
        minerals_cost: Amt::units(200),
        energy_cost: Amt::units(100),
        build_time: 10,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::units(10),
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: false,
        capabilities: HashMap::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: false,
        prerequisites: Some(Condition::Atom(ConditionAtom::has_tech("deep_core_mining"))),
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
        ship_design_id: None,
        colony_slots: None,
    });
    app.world_mut().insert_resource(registry);

    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(10_000),
        Amt::units(10_000),
        vec![Some(BuildingId::new("basic_mine")), None, None, None],
    );

    spawn_pending(
        &mut app,
        sys,
        5,
        RemoteCommand::Colony(ColonyCommand {
            scope: BuildingScope::Planet(planet),
            kind: BuildingKind::Upgrade {
                slot_index: 0,
                target_id: "advanced_mine".to_string(),
            },
        }),
    );

    run_until_arrival(&mut app, 5);

    let bq = app.world().get::<BuildingQueue>(colony).unwrap();
    assert!(
        bq.upgrade_queue.is_empty(),
        "upgrade with unmet target prerequisites must be rejected"
    );
}

// --------------------------------------------------------------------------
// #437 — Game-flag-based prerequisites flow through the same pipeline
// --------------------------------------------------------------------------

#[test]
fn queue_building_respects_empire_flag_prerequisites() {
    // `prerequisites` can reference flags, not just techs. The arrival
    // handler pulls both `GameFlags` and `ScopedFlags` into the EvalContext
    // (union); this exercises that union so a future refactor can't
    // accidentally drop one of the two sources.
    let mut app = build_app();

    let mut registry = BuildingRegistry::default();
    registry.insert(BuildingDefinition {
        id: "flag_gated".into(),
        name: "Flag Gated".into(),
        description: String::new(),
        minerals_cost: Amt::units(10),
        energy_cost: Amt::units(10),
        // Long build_time + expensive enough to not auto-complete on the
        // same frame the arrival handler enqueues. Otherwise
        // tick_building_queue can remove the order before the assert.
        build_time: 50,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::ZERO,
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: false,
        capabilities: HashMap::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: true,
        prerequisites: Some(Condition::Atom(ConditionAtom::has_flag("golden_age"))),
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
        ship_design_id: None,
        colony_slots: None,
    });
    app.world_mut().insert_resource(registry);

    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1_000),
        Amt::units(1_000),
        vec![None, None, None, None],
    );

    // Set the flag on the empire. Using `GameFlags` (the legacy flat
    // source) rather than `ScopedFlags` — both paths flow into the same
    // union inside `process_pending_commands`. The companion negative
    // test covers the "no flag set anywhere" case.
    let empire = empire_entity(app.world_mut());
    if let Some(mut gf) = app.world_mut().get_mut::<GameFlags>(empire) {
        gf.set("golden_age");
    } else {
        panic!("test empire is missing GameFlags component");
    }

    spawn_pending(
        &mut app,
        sys,
        10,
        RemoteCommand::Colony(ColonyCommand {
            scope: BuildingScope::Planet(planet),
            kind: BuildingKind::Queue {
                building_id: "flag_gated".to_string(),
                target_slot: 0,
            },
        }),
    );

    run_until_arrival(&mut app, 10);

    let bq = app.world().get::<BuildingQueue>(colony).unwrap();
    assert_eq!(
        bq.queue.len(),
        1,
        "flag-gated building must enqueue when the flag is set"
    );
}

#[test]
fn queue_building_rejected_without_required_flag() {
    // Flag NOT set — same command must be rejected.
    let mut app = build_app();

    let mut registry = BuildingRegistry::default();
    registry.insert(BuildingDefinition {
        id: "flag_gated".into(),
        name: "Flag Gated".into(),
        description: String::new(),
        minerals_cost: Amt::units(10),
        energy_cost: Amt::units(10),
        // Long build_time + expensive enough to not auto-complete on the
        // same frame the arrival handler enqueues. Otherwise
        // tick_building_queue can remove the order before the assert.
        build_time: 50,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::ZERO,
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: false,
        capabilities: HashMap::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: true,
        prerequisites: Some(Condition::Atom(ConditionAtom::has_flag("golden_age"))),
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
        ship_design_id: None,
        colony_slots: None,
    });
    app.world_mut().insert_resource(registry);

    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1_000),
        Amt::units(1_000),
        vec![None, None, None, None],
    );

    // Make sure flags are clean.
    let empire = empire_entity(app.world_mut());
    app.world_mut()
        .entity_mut(empire)
        .insert((GameFlags::default(), ScopedFlags::default()));

    spawn_pending(
        &mut app,
        sys,
        10,
        RemoteCommand::Colony(ColonyCommand {
            scope: BuildingScope::Planet(planet),
            kind: BuildingKind::Queue {
                building_id: "flag_gated".to_string(),
                target_slot: 0,
            },
        }),
    );

    run_until_arrival(&mut app, 10);

    let bq = app.world().get::<BuildingQueue>(colony).unwrap();
    assert!(
        bq.queue.is_empty(),
        "flag-gated building must be rejected when the flag is missing"
    );
}
