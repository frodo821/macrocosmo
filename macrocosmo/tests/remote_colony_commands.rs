//! #270: Integration tests for `RemoteCommand::Colony` arrival dispatcher.
//!
//! Each test spawns a `PendingCommand` directly (bypassing UI dispatch —
//! that's covered by later commits) and verifies the arrival handler
//! mutates the target queues correctly when the clock advances past
//! `arrives_at`.

mod common;

use bevy::prelude::*;

use macrocosmo::amount::Amt;
use macrocosmo::colony::{BuildKind, BuildQueue, BuildingQueue, SystemBuildingQueue};
use macrocosmo::communication::{
    self, ColonyCommand, ColonyCommandKind, CommandLog, PendingColonyDispatch,
    PendingColonyDispatches, PendingCommand, RemoteCommand,
};
use macrocosmo::components::Position;
use macrocosmo::player::{Player, PlayerEmpire, StationedAt};
use macrocosmo::scripting::building_api::BuildingId;

use common::{
    advance_time, empire_entity, spawn_test_colony, spawn_test_system_with_planet, test_app,
};

/// Wire `process_pending_commands` into a bare `test_app()` — only this
/// system is needed, no need to pay the cost of `full_test_app()`.
fn build_app() -> App {
    let mut app = test_app();
    app.add_systems(
        Update,
        communication::process_pending_commands
            .after(macrocosmo::time_system::advance_game_time),
    );
    app
}

/// Build an app with the full communication dispatch chain wired in. Used
/// by the tests that exercise the UI-push-to-arrival pipeline (Commit C).
fn build_app_with_dispatch() -> App {
    let mut app = test_app();
    app.init_resource::<PendingColonyDispatches>();
    app.add_systems(
        Update,
        (
            communication::dispatch_pending_colony_commands,
            communication::process_pending_commands,
        )
            .chain()
            .after(macrocosmo::time_system::advance_game_time),
    );
    app
}

fn spawn_pending_colony_command(
    app: &mut App,
    target_system: Entity,
    sent_at: i64,
    arrives_at: i64,
    cmd: ColonyCommand,
) {
    app.world_mut().spawn(PendingCommand {
        target_system,
        command: RemoteCommand::Colony(cmd),
        sent_at,
        arrives_at,
        origin_pos: [0.0, 0.0, 0.0],
        destination_pos: [0.0, 0.0, 0.0],
    });
    // Also register a matching CommandLog entry so the arrival code finds
    // something to mark as arrived. (Not strictly required for dispatch
    // tests — the arrival handler despawns regardless — but mirrors
    // production where `send_remote_command` records the entry.)
    let empire = empire_entity(app.world_mut());
    if let Some(mut log) = app.world_mut().get_mut::<CommandLog>(empire) {
        log.entries.push(macrocosmo::communication::CommandLogEntry {
            description: "test".to_string(),
            sent_at,
            arrives_at,
            arrived: false,
        });
    }
}

/// Position the clock one tick before arrival and advance by exactly 1
/// hexadies, so the resulting `delta` seen by production/build-tick systems
/// is 1. Without the `LastProductionTick` alignment, the tick systems would
/// see `delta = arrives_at - 0` and complete arbitrarily cheap buildings in
/// a single frame — this fixture focuses on the arrival dispatcher's
/// enqueue step, not downstream build completion.
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

// --------------------------------------------------------------------------
// QueueBuilding
// --------------------------------------------------------------------------

#[test]
fn queue_building_planet_arrives_and_enqueues() {
    let mut app = build_app();
    let (sys, planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Target",
        [10.0, 0.0, 0.0],
        1.0,
        true,
    );
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );

    spawn_pending_colony_command(
        &mut app,
        sys,
        0,
        10,
        ColonyCommand {
            target_planet: Some(planet),
            kind: ColonyCommandKind::QueueBuilding {
                building_id: "mine".to_string(),
                target_slot: 1,
            },
        },
    );

    // Before arrival the queue should be empty.
    let bq = app.world().get::<BuildingQueue>(colony).unwrap();
    assert!(bq.queue.is_empty(), "queue should be empty before arrival");

    run_until_arrival(&mut app, 10);

    let bq = app.world().get::<BuildingQueue>(colony).unwrap();
    assert_eq!(bq.queue.len(), 1, "one BuildingOrder should be enqueued");
    assert_eq!(bq.queue[0].target_slot, 1);
    assert_eq!(bq.queue[0].building_id.as_str(), "mine");
}

#[test]
fn queue_building_system_arrives_and_enqueues() {
    let mut app = build_app();
    let (sys, _planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Target",
        [10.0, 0.0, 0.0],
        1.0,
        true,
    );
    // spawn_test_colony also attaches SystemBuildings/SystemBuildingQueue to
    // the system entity.
    let _colony = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::units(1000),
        Amt::units(1000),
        vec![],
    );

    spawn_pending_colony_command(
        &mut app,
        sys,
        0,
        10,
        ColonyCommand {
            target_planet: None,
            kind: ColonyCommandKind::QueueBuilding {
                building_id: "shipyard".to_string(),
                target_slot: 0,
            },
        },
    );

    run_until_arrival(&mut app, 10);

    let sbq = app.world().get::<SystemBuildingQueue>(sys).unwrap();
    assert_eq!(sbq.queue.len(), 1);
    assert_eq!(sbq.queue[0].target_slot, 0);
    assert_eq!(sbq.queue[0].building_id.as_str(), "shipyard");
}

// --------------------------------------------------------------------------
// DemolishBuilding
// --------------------------------------------------------------------------

#[test]
fn demolish_building_planet_arrives_and_enqueues() {
    let mut app = build_app();
    let (sys, planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Target",
        [10.0, 0.0, 0.0],
        1.0,
        true,
    );
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![Some(BuildingId::new("mine")), None, None, None],
    );

    spawn_pending_colony_command(
        &mut app,
        sys,
        0,
        5,
        ColonyCommand {
            target_planet: Some(planet),
            kind: ColonyCommandKind::DemolishBuilding { target_slot: 0 },
        },
    );

    run_until_arrival(&mut app, 5);

    let bq = app.world().get::<BuildingQueue>(colony).unwrap();
    assert_eq!(bq.demolition_queue.len(), 1);
    assert_eq!(bq.demolition_queue[0].target_slot, 0);
    assert_eq!(bq.demolition_queue[0].building_id.as_str(), "mine");
    // Demolition time starts at build_time/2 = 5 (mine: build_time=10). The
    // single tick_building_queue pass that happens within this `advance_time(1)`
    // subtracts `delta=1`, so the observed value is 4.
    assert_eq!(bq.demolition_queue[0].time_remaining, 4);
}

// --------------------------------------------------------------------------
// UpgradeBuilding
// --------------------------------------------------------------------------

#[test]
fn upgrade_building_planet_without_path_warns_and_noops() {
    // The test building registry does not include upgrade_to paths, so a
    // naive upgrade request should warn but not enqueue anything. This
    // verifies the path-lookup branch.
    let mut app = build_app();
    let (sys, planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Target",
        [10.0, 0.0, 0.0],
        1.0,
        true,
    );
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![Some(BuildingId::new("mine")), None, None, None],
    );

    spawn_pending_colony_command(
        &mut app,
        sys,
        0,
        5,
        ColonyCommand {
            target_planet: Some(planet),
            kind: ColonyCommandKind::UpgradeBuilding {
                slot_index: 0,
                target_id: "advanced_mine".to_string(),
            },
        },
    );

    run_until_arrival(&mut app, 5);

    let bq = app.world().get::<BuildingQueue>(colony).unwrap();
    assert!(
        bq.upgrade_queue.is_empty(),
        "upgrade_queue should stay empty when no upgrade_to path matches"
    );
}

// --------------------------------------------------------------------------
// CancelBuildingOrder
// --------------------------------------------------------------------------

#[test]
fn cancel_building_order_planet_removes_matching_slot() {
    let mut app = build_app();
    let (sys, planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Target",
        [10.0, 0.0, 0.0],
        1.0,
        true,
    );
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );

    // Pre-populate the queue with an order for slot 2.
    {
        let mut bq = app.world_mut().get_mut::<BuildingQueue>(colony).unwrap();
        bq.queue.push(macrocosmo::colony::BuildingOrder {
            building_id: BuildingId::new("mine"),
            target_slot: 2,
            minerals_remaining: Amt::units(150),
            energy_remaining: Amt::units(50),
            build_time_remaining: 10,
        });
    }

    spawn_pending_colony_command(
        &mut app,
        sys,
        0,
        5,
        ColonyCommand {
            target_planet: Some(planet),
            kind: ColonyCommandKind::CancelBuildingOrder { target_slot: 2 },
        },
    );

    run_until_arrival(&mut app, 5);

    let bq = app.world().get::<BuildingQueue>(colony).unwrap();
    assert!(
        bq.queue.is_empty(),
        "queue entry matching slot 2 should be removed on arrival"
    );
}

// --------------------------------------------------------------------------
// QueueShipBuild
// --------------------------------------------------------------------------

#[test]
fn queue_ship_build_arrives_and_enqueues() {
    let mut app = build_app();
    let (sys, planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Target",
        [10.0, 0.0, 0.0],
        1.0,
        true,
    );
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(10_000),
        Amt::units(10_000),
        vec![],
    );

    spawn_pending_colony_command(
        &mut app,
        sys,
        0,
        20,
        ColonyCommand {
            target_planet: None,
            kind: ColonyCommandKind::QueueShipBuild {
                host_colony: colony,
                design_id: "explorer_mk1".to_string(),
                build_kind: BuildKind::Ship,
            },
        },
    );

    run_until_arrival(&mut app, 20);

    let bq = app.world().get::<BuildQueue>(colony).unwrap();
    assert_eq!(bq.queue.len(), 1, "ship BuildOrder should be enqueued");
    assert_eq!(bq.queue[0].design_id, "explorer_mk1");
    assert!(matches!(bq.queue[0].kind, BuildKind::Ship));
}

/// #270 Commit H: Deliverable dispatch carries full payload so arrival
/// doesn't need StructureRegistry access. Verify a
/// `QueueDeliverableBuild` lands a `BuildOrder` with
/// `kind: BuildKind::Deliverable { cargo_size }` on the host colony.
#[test]
fn queue_deliverable_build_arrives_and_enqueues() {
    let mut app = build_app();
    let (sys, planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Target",
        [10.0, 0.0, 0.0],
        1.0,
        true,
    );
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(10_000),
        Amt::units(10_000),
        vec![],
    );

    spawn_pending_colony_command(
        &mut app,
        sys,
        0,
        20,
        ColonyCommand {
            target_planet: None,
            kind: ColonyCommandKind::QueueDeliverableBuild {
                host_colony: colony,
                def_id: "sensor_buoy".to_string(),
                display_name: "Sensor Buoy".to_string(),
                cargo_size: 2,
                minerals_cost: Amt::units(100),
                energy_cost: Amt::units(50),
                build_time: 30,
            },
        },
    );

    run_until_arrival(&mut app, 20);

    let bq = app.world().get::<BuildQueue>(colony).unwrap();
    assert_eq!(bq.queue.len(), 1);
    assert_eq!(bq.queue[0].design_id, "sensor_buoy");
    assert!(matches!(
        bq.queue[0].kind,
        BuildKind::Deliverable { cargo_size: 2 }
    ));
    assert_eq!(bq.queue[0].minerals_cost, Amt::units(100));
    assert_eq!(bq.queue[0].build_time_total, 30);
}

// --------------------------------------------------------------------------
// Timing: commands before arrival do NOT fire
// --------------------------------------------------------------------------

#[test]
fn pending_colony_command_not_applied_before_arrival() {
    let mut app = build_app();
    let (sys, planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Target",
        [10.0, 0.0, 0.0],
        1.0,
        true,
    );
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );

    spawn_pending_colony_command(
        &mut app,
        sys,
        0,
        100,
        ColonyCommand {
            target_planet: Some(planet),
            kind: ColonyCommandKind::QueueBuilding {
                building_id: "mine".to_string(),
                target_slot: 0,
            },
        },
    );

    // Advance halfway.
    app.world_mut()
        .resource_mut::<macrocosmo::time_system::GameClock>()
        .elapsed = 50;
    advance_time(&mut app, 1);

    let bq = app.world().get::<BuildingQueue>(colony).unwrap();
    assert!(
        bq.queue.is_empty(),
        "queue must remain empty until arrives_at"
    );

    // The PendingCommand entity should still be alive.
    let alive = app
        .world_mut()
        .query::<&PendingCommand>()
        .iter(app.world())
        .count();
    assert_eq!(alive, 1);
}

// --------------------------------------------------------------------------
// UI dispatch pipeline (Commit C): UI push → send_remote_command → arrival
// --------------------------------------------------------------------------

/// Local dispatch: player is at the target system, so the light-speed delay
/// is zero and the command applies the same frame it's pushed.
#[test]
fn local_dispatch_applies_same_frame() {
    let mut app = build_app_with_dispatch();
    let (sys, planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Home",
        [0.0, 0.0, 0.0],
        1.0,
        true,
    );
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );
    app.world_mut()
        .spawn((Player, StationedAt { system: sys }));

    app.world_mut()
        .resource_mut::<PendingColonyDispatches>()
        .queue
        .push(PendingColonyDispatch {
            target_system: sys,
            command: ColonyCommand {
                target_planet: Some(planet),
                kind: ColonyCommandKind::QueueBuilding {
                    building_id: "mine".to_string(),
                    target_slot: 0,
                },
            },
        });

    // Align LastProductionTick so the build queue tick sees delta=1.
    app.world_mut()
        .resource_mut::<macrocosmo::colony::LastProductionTick>()
        .0 = 0;
    advance_time(&mut app, 1);

    let bq = app
        .world()
        .get::<macrocosmo::colony::BuildingQueue>(colony)
        .unwrap();
    // Either the order is in the queue or already in the slot if cost+time
    // completed in the single tick. Verify progression.
    let present_in_queue = bq.queue.iter().any(|o| o.target_slot == 0);
    let present_in_slot = app
        .world()
        .get::<macrocosmo::colony::Buildings>(colony)
        .map(|b| b.slots.get(0).and_then(|s| s.as_ref()).is_some())
        .unwrap_or(false);
    assert!(
        present_in_queue || present_in_slot,
        "local dispatch should result in queued (or completed) order by next frame"
    );
}

/// Remote dispatch: player is 10 ly away → ~600 hd light delay. Command
/// should NOT apply until the clock advances past `arrives_at`.
#[test]
fn remote_dispatch_delayed_by_light_speed() {
    let mut app = build_app_with_dispatch();
    let (home_sys, _home_planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Home",
        [0.0, 0.0, 0.0],
        1.0,
        true,
    );
    let (target_sys, target_planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Target",
        [10.0, 0.0, 0.0],
        1.0,
        true,
    );
    let target_colony = spawn_test_colony(
        app.world_mut(),
        target_planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );
    app.world_mut()
        .spawn((Player, StationedAt { system: home_sys }));

    app.world_mut()
        .resource_mut::<PendingColonyDispatches>()
        .queue
        .push(PendingColonyDispatch {
            target_system: target_sys,
            command: ColonyCommand {
                target_planet: Some(target_planet),
                kind: ColonyCommandKind::QueueBuilding {
                    building_id: "mine".to_string(),
                    target_slot: 0,
                },
            },
        });

    // Run one frame. Dispatch drains and spawns PendingCommand;
    // process_pending_commands sees arrives_at > clock.elapsed and skips.
    advance_time(&mut app, 1);

    let bq = app
        .world()
        .get::<macrocosmo::colony::BuildingQueue>(target_colony)
        .unwrap();
    assert!(
        bq.queue.is_empty(),
        "remote colony queue should be empty before light delay elapses"
    );

    let pending_count = app
        .world_mut()
        .query::<&PendingCommand>()
        .iter(app.world())
        .count();
    assert_eq!(
        pending_count, 1,
        "exactly one in-flight PendingCommand should exist"
    );

    // Advance past light delay (10 ly => 600 hd). Command was dispatched
    // during the previous `advance_time(1)` so `sent_at = 1` and
    // `arrives_at = 1 + 600 = 601`.
    let arrives_at = 1 + macrocosmo::physics::light_delay_hexadies(10.0);
    run_until_arrival(&mut app, arrives_at);

    let bq = app
        .world()
        .get::<macrocosmo::colony::BuildingQueue>(target_colony)
        .unwrap();
    let present_in_queue = bq.queue.iter().any(|o| o.target_slot == 0);
    let present_in_slot = app
        .world()
        .get::<macrocosmo::colony::Buildings>(target_colony)
        .map(|b| b.slots.get(0).and_then(|s| s.as_ref()).is_some())
        .unwrap_or(false);
    assert!(
        present_in_queue || present_in_slot,
        "remote command should apply once clock reaches arrives_at"
    );
}

/// Remote system-level dispatch (Commit D): `target_planet: None` targets
/// the `SystemBuildingQueue` on the target system.
#[test]
fn remote_system_level_dispatch_delayed() {
    let mut app = build_app_with_dispatch();
    let (home_sys, _home_planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Home",
        [0.0, 0.0, 0.0],
        1.0,
        true,
    );
    let (target_sys, _target_planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Target",
        [10.0, 0.0, 0.0],
        1.0,
        true,
    );
    // spawn_test_colony attaches SystemBuildings/SystemBuildingQueue to the
    // star system entity.
    let _target_colony = spawn_test_colony(
        app.world_mut(),
        target_sys,
        Amt::units(10_000),
        Amt::units(10_000),
        vec![],
    );
    app.world_mut()
        .spawn((Player, StationedAt { system: home_sys }));

    app.world_mut()
        .resource_mut::<PendingColonyDispatches>()
        .queue
        .push(PendingColonyDispatch {
            target_system: target_sys,
            command: ColonyCommand {
                target_planet: None,
                kind: ColonyCommandKind::QueueBuilding {
                    building_id: "shipyard".to_string(),
                    target_slot: 0,
                },
            },
        });

    advance_time(&mut app, 1);

    let sbq = app
        .world()
        .get::<SystemBuildingQueue>(target_sys)
        .unwrap();
    assert!(
        sbq.queue.is_empty(),
        "remote system queue should be empty before light delay"
    );

    let arrives_at = 1 + macrocosmo::physics::light_delay_hexadies(10.0);
    run_until_arrival(&mut app, arrives_at);

    let sbq = app
        .world()
        .get::<SystemBuildingQueue>(target_sys)
        .unwrap();
    let sys_bldgs = app
        .world()
        .get::<macrocosmo::colony::SystemBuildings>(target_sys)
        .unwrap();
    let present = sbq.queue.iter().any(|o| o.target_slot == 0)
        || sys_bldgs
            .slots
            .get(0)
            .and_then(|s| s.as_ref())
            .is_some();
    assert!(
        present,
        "remote system-level command should apply once clock reaches arrives_at"
    );
}

/// Remote ship-build dispatch (Commit E): `QueueShipBuild { host_colony }`
/// targets a specific colony's ship `BuildQueue` after light-speed delay.
#[test]
fn remote_ship_build_dispatch_delayed() {
    let mut app = build_app_with_dispatch();
    let (home_sys, _home_planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Home",
        [0.0, 0.0, 0.0],
        1.0,
        true,
    );
    let (target_sys, target_planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Target",
        [10.0, 0.0, 0.0],
        1.0,
        true,
    );
    let target_colony = spawn_test_colony(
        app.world_mut(),
        target_planet,
        Amt::units(10_000),
        Amt::units(10_000),
        vec![],
    );
    app.world_mut()
        .spawn((Player, StationedAt { system: home_sys }));

    app.world_mut()
        .resource_mut::<PendingColonyDispatches>()
        .queue
        .push(PendingColonyDispatch {
            target_system: target_sys,
            command: ColonyCommand {
                target_planet: None,
                kind: ColonyCommandKind::QueueShipBuild {
                    host_colony: target_colony,
                    design_id: "explorer_mk1".to_string(),
                    build_kind: BuildKind::Ship,
                },
            },
        });

    advance_time(&mut app, 1);

    let bq = app.world().get::<BuildQueue>(target_colony).unwrap();
    assert!(
        bq.queue.is_empty(),
        "remote ship BuildQueue should be empty before light delay"
    );

    let arrives_at = 1 + macrocosmo::physics::light_delay_hexadies(10.0);
    run_until_arrival(&mut app, arrives_at);

    let bq = app.world().get::<BuildQueue>(target_colony).unwrap();
    assert_eq!(
        bq.queue.len(),
        1,
        "remote ship build should arrive and enqueue"
    );
    assert_eq!(bq.queue[0].design_id, "explorer_mk1");
}

// --------------------------------------------------------------------------
// CommandLog arrival marking
// --------------------------------------------------------------------------

#[test]
fn arrival_marks_command_log_entry() {
    let mut app = build_app();
    let (sys, planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Target",
        [10.0, 0.0, 0.0],
        1.0,
        true,
    );
    let _colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );

    spawn_pending_colony_command(
        &mut app,
        sys,
        0,
        10,
        ColonyCommand {
            target_planet: Some(planet),
            kind: ColonyCommandKind::QueueBuilding {
                building_id: "mine".to_string(),
                target_slot: 0,
            },
        },
    );

    run_until_arrival(&mut app, 10);

    // Locate the empire and inspect CommandLog.
    let mut empire_q = app
        .world_mut()
        .query_filtered::<&CommandLog, With<PlayerEmpire>>();
    let log = empire_q.single(app.world()).expect("empire command log");
    assert_eq!(log.entries.len(), 1);
    assert!(log.entries[0].arrived, "entry should be marked arrived");
}

