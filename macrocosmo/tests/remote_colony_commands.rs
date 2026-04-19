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
    self, BuildingKind, BuildingScope, ColonyCommand, CommandLog, MAX_DISPATCH_RETRY_FRAMES,
    PendingColonyDispatch, PendingColonyDispatches, PendingCommand, RemoteCommand,
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
        communication::process_pending_commands.after(macrocosmo::time_system::advance_game_time),
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

fn spawn_pending_remote_command(
    app: &mut App,
    target_system: Entity,
    sent_at: i64,
    arrives_at: i64,
    cmd: RemoteCommand,
) {
    app.world_mut().spawn(PendingCommand {
        id: macrocosmo::ship::command_events::CommandId::ZERO,
        target_system,
        command: cmd,
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
        log.entries
            .push(macrocosmo::communication::CommandLogEntry::new_pending(
                "test".to_string(),
                sent_at,
                arrives_at,
            ));
    }
}

/// Jump the clock to the frame of arrival. Tests here fast-forward from
/// t=0 to t=arrives_at (which may be hundreds of hd for remote commands),
/// so we also pin `LastProductionTick` to keep the build-tick `delta` at
/// 1 — otherwise tick_building_queue would "catch up" 600 iterations in
/// one frame and complete arbitrarily cheap orders before the assertions
/// run. This is about delta management during fast-forward, not the
/// arrival-vs-consumption ordering invariant (which is enforced in the
/// production schedule via `tick_building_queue.after(process_pending_commands)`).
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
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );

    spawn_pending_remote_command(
        &mut app,
        sys,
        0,
        10,
        RemoteCommand::Colony(ColonyCommand {
            scope: BuildingScope::Planet(planet),
            kind: BuildingKind::Queue {
                building_id: "mine".to_string(),
                target_slot: 1,
            },
        }),
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
    let (sys, _planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    // spawn_test_colony also attaches SystemBuildings/SystemBuildingQueue to
    // the system entity.
    let _colony = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::units(1000),
        Amt::units(1000),
        vec![],
    );
    // #370: System building enqueue requires a Core ship in the system.
    let empire = empire_entity(app.world_mut());
    app.world_mut().spawn((
        macrocosmo::ship::CoreShip,
        macrocosmo::galaxy::AtSystem(sys),
        macrocosmo::faction::FactionOwner(empire),
    ));

    spawn_pending_remote_command(
        &mut app,
        sys,
        0,
        10,
        RemoteCommand::Colony(ColonyCommand {
            scope: BuildingScope::System,
            kind: BuildingKind::Queue {
                building_id: "shipyard".to_string(),
                target_slot: 0,
            },
        }),
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
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![Some(BuildingId::new("mine")), None, None, None],
    );

    spawn_pending_remote_command(
        &mut app,
        sys,
        0,
        5,
        RemoteCommand::Colony(ColonyCommand {
            scope: BuildingScope::Planet(planet),
            kind: BuildingKind::Demolish { target_slot: 0 },
        }),
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
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![Some(BuildingId::new("mine")), None, None, None],
    );

    spawn_pending_remote_command(
        &mut app,
        sys,
        0,
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
        "upgrade_queue should stay empty when no upgrade_to path matches"
    );
}

// --------------------------------------------------------------------------
// QueueShipBuild
// --------------------------------------------------------------------------

#[test]
fn queue_ship_build_arrives_and_enqueues() {
    let mut app = build_app();
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(10_000),
        Amt::units(10_000),
        vec![],
    );

    spawn_pending_remote_command(
        &mut app,
        sys,
        0,
        20,
        RemoteCommand::ShipBuild {
            host_colony: colony,
            design_id: "explorer_mk1".to_string(),
            build_kind: BuildKind::Ship,
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
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(10_000),
        Amt::units(10_000),
        vec![],
    );

    spawn_pending_remote_command(
        &mut app,
        sys,
        0,
        20,
        RemoteCommand::DeliverableBuild {
            host_colony: colony,
            def_id: "sensor_buoy".to_string(),
            display_name: "Sensor Buoy".to_string(),
            cargo_size: 2,
            minerals_cost: Amt::units(100),
            energy_cost: Amt::units(50),
            build_time: 30,
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
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );

    spawn_pending_remote_command(
        &mut app,
        sys,
        0,
        100,
        RemoteCommand::Colony(ColonyCommand {
            scope: BuildingScope::Planet(planet),
            kind: BuildingKind::Queue {
                building_id: "mine".to_string(),
                target_slot: 0,
            },
        }),
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
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );
    app.world_mut().spawn((Player, StationedAt { system: sys }));

    app.world_mut()
        .resource_mut::<PendingColonyDispatches>()
        .queue
        .push(PendingColonyDispatch {
            target_system: sys,
            command: RemoteCommand::Colony(ColonyCommand {
                scope: BuildingScope::Planet(planet),
                kind: BuildingKind::Queue {
                    building_id: "mine".to_string(),
                    target_slot: 0,
                },
            }),
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
    let (home_sys, _home_planet) =
        spawn_test_system_with_planet(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true);
    let (target_sys, target_planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
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
            command: RemoteCommand::Colony(ColonyCommand {
                scope: BuildingScope::Planet(target_planet),
                kind: BuildingKind::Queue {
                    building_id: "mine".to_string(),
                    target_slot: 0,
                },
            }),
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

/// Remote system-level dispatch (Commit D): `scope: BuildingScope::System` targets
/// the `SystemBuildingQueue` on the target system.
#[test]
fn remote_system_level_dispatch_delayed() {
    let mut app = build_app_with_dispatch();
    let (home_sys, _home_planet) =
        spawn_test_system_with_planet(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true);
    let (target_sys, _target_planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    // spawn_test_colony attaches SystemBuildings/SystemBuildingQueue to the
    // star system entity.
    let _target_colony = spawn_test_colony(
        app.world_mut(),
        target_sys,
        Amt::units(10_000),
        Amt::units(10_000),
        vec![],
    );
    // #370: System building enqueue requires a Core ship in the target system.
    let empire = empire_entity(app.world_mut());
    app.world_mut().spawn((
        macrocosmo::ship::CoreShip,
        macrocosmo::galaxy::AtSystem(target_sys),
        macrocosmo::faction::FactionOwner(empire),
    ));
    app.world_mut()
        .spawn((Player, StationedAt { system: home_sys }));

    app.world_mut()
        .resource_mut::<PendingColonyDispatches>()
        .queue
        .push(PendingColonyDispatch {
            target_system: target_sys,
            command: RemoteCommand::Colony(ColonyCommand {
                scope: BuildingScope::System,
                kind: BuildingKind::Queue {
                    building_id: "shipyard".to_string(),
                    target_slot: 0,
                },
            }),
        });

    advance_time(&mut app, 1);

    let sbq = app.world().get::<SystemBuildingQueue>(target_sys).unwrap();
    assert!(
        sbq.queue.is_empty(),
        "remote system queue should be empty before light delay"
    );

    let arrives_at = 1 + macrocosmo::physics::light_delay_hexadies(10.0);
    run_until_arrival(&mut app, arrives_at);

    let sbq = app.world().get::<SystemBuildingQueue>(target_sys).unwrap();
    let sys_bldgs = app
        .world()
        .get::<macrocosmo::colony::SystemBuildings>(target_sys)
        .unwrap();
    let present = sbq.queue.iter().any(|o| o.target_slot == 0)
        || sys_bldgs.slots.get(0).and_then(|s| s.as_ref()).is_some();
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
    let (home_sys, _home_planet) =
        spawn_test_system_with_planet(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true);
    let (target_sys, target_planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
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
            command: RemoteCommand::ShipBuild {
                host_colony: target_colony,
                design_id: "explorer_mk1".to_string(),
                build_kind: BuildKind::Ship,
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
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let _colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );

    spawn_pending_remote_command(
        &mut app,
        sys,
        0,
        10,
        RemoteCommand::Colony(ColonyCommand {
            scope: BuildingScope::Planet(planet),
            kind: BuildingKind::Queue {
                building_id: "mine".to_string(),
                target_slot: 0,
            },
        }),
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

// --------------------------------------------------------------------------
// #275: Cancel commands — order_id based
// --------------------------------------------------------------------------

/// Helper that seeds a planet-level building order on a colony's
/// BuildingQueue via the public `push_build_order` helper so the returned
/// `order_id` matches what the cancel command will reference.
fn seed_building_order(app: &mut App, colony: Entity) -> u64 {
    use macrocosmo::colony::BuildingOrder;
    let mut bq = app
        .world_mut()
        .get_mut::<BuildingQueue>(colony)
        .expect("colony has BuildingQueue");
    bq.push_build_order(BuildingOrder {
        order_id: 0,
        building_id: BuildingId::new("mine"),
        target_slot: 1,
        minerals_remaining: Amt::units(100),
        energy_remaining: Amt::units(50),
        build_time_remaining: 50,
    })
}

/// #275 Phase 1: `BuildingQueue::push_build_order` auto-assigns ids so
/// sequentially pushed orders get distinct ids starting at 1.
#[test]
fn push_order_assigns_unique_incrementing_ids() {
    let mut app = build_app();
    let (_sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "S", [0.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );
    let id1 = seed_building_order(&mut app, colony);
    let id2 = seed_building_order(&mut app, colony);
    let id3 = seed_building_order(&mut app, colony);
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
    assert_eq!(id3, 3);
    let bq = app.world().get::<BuildingQueue>(colony).unwrap();
    assert_eq!(bq.queue[0].order_id, 1);
    assert_eq!(bq.queue[1].order_id, 2);
    assert_eq!(bq.queue[2].order_id, 3);
    assert_eq!(bq.next_order_id, 4);
}

/// #275 Phase 2: `CancelBuildingOrder` at zero distance (local) applies
/// the same frame it's dispatched — mirrors the Queue path.
#[test]
fn local_cancel_building_order_applies_same_frame() {
    let mut app = build_app_with_dispatch();
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );
    app.world_mut().spawn((Player, StationedAt { system: sys }));
    let order_id = seed_building_order(&mut app, colony);

    app.world_mut()
        .resource_mut::<PendingColonyDispatches>()
        .queue
        .push(PendingColonyDispatch {
            target_system: sys,
            command: RemoteCommand::CancelBuildingOrder { order_id },
        });

    // Align LastProductionTick so delta=1 (no giant catch-up).
    app.world_mut()
        .resource_mut::<macrocosmo::colony::LastProductionTick>()
        .0 = 0;
    advance_time(&mut app, 1);

    let bq = app.world().get::<BuildingQueue>(colony).unwrap();
    assert!(
        bq.queue.is_empty(),
        "local cancel should remove the order same frame"
    );
}

/// #275 Phase 2: remote cancel is delayed by light-speed. Order stays
/// enqueued until arrival.
#[test]
fn remote_cancel_building_order_delayed() {
    let mut app = build_app_with_dispatch();
    let (home_sys, _home_planet) =
        spawn_test_system_with_planet(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true);
    let (target_sys, target_planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let target_colony = spawn_test_colony(
        app.world_mut(),
        target_planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );
    app.world_mut()
        .spawn((Player, StationedAt { system: home_sys }));
    let order_id = seed_building_order(&mut app, target_colony);

    app.world_mut()
        .resource_mut::<PendingColonyDispatches>()
        .queue
        .push(PendingColonyDispatch {
            target_system: target_sys,
            command: RemoteCommand::CancelBuildingOrder { order_id },
        });

    advance_time(&mut app, 1);
    // Order should still be present before light-delay elapses.
    let bq = app.world().get::<BuildingQueue>(target_colony).unwrap();
    assert_eq!(bq.queue.len(), 1, "cancel should not apply before arrival");

    let arrives_at = 1 + macrocosmo::physics::light_delay_hexadies(10.0);
    run_until_arrival(&mut app, arrives_at);

    let bq = app.world().get::<BuildingQueue>(target_colony).unwrap();
    assert!(
        bq.queue.is_empty(),
        "remote cancel should remove order once light delay elapses"
    );
}

/// #275 Phase 2: Cancel against a non-existent order_id logs a warn but
/// does not panic / affect unrelated orders. Simulates the race where
/// the order completed (or was cancelled by a previous dispatch) during
/// the light-speed trip.
#[test]
fn cancel_missing_order_is_noop_warn() {
    let mut app = build_app();
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );
    // Seed one real order whose id we will NOT cancel.
    let _real = seed_building_order(&mut app, colony);

    spawn_pending_remote_command(
        &mut app,
        sys,
        0,
        5,
        RemoteCommand::CancelBuildingOrder { order_id: 9999 },
    );

    run_until_arrival(&mut app, 5);

    let bq = app.world().get::<BuildingQueue>(colony).unwrap();
    assert_eq!(
        bq.queue.len(),
        1,
        "unrelated order must stay put when cancel misses"
    );
}

/// #275 Phase 2 (regression): `CancelBuildingOrder` must not cross
/// system boundaries. `order_id` counters are per-queue so the same
/// numeric id can exist independently on different systems; a cancel
/// dispatched for system A should only scan colonies in system A.
#[test]
fn cancel_scoped_to_target_system_does_not_affect_other_systems() {
    let mut app = build_app();
    let (sys_a, planet_a) =
        spawn_test_system_with_planet(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true);
    let (sys_b, planet_b) =
        spawn_test_system_with_planet(app.world_mut(), "B", [20.0, 0.0, 0.0], 1.0, true);
    let colony_a = spawn_test_colony(
        app.world_mut(),
        planet_a,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );
    let colony_b = spawn_test_colony(
        app.world_mut(),
        planet_b,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );
    // Seed one order on each colony. Both will get order_id=1 because
    // each BuildingQueue has its own counter.
    let id_a = seed_building_order(&mut app, colony_a);
    let id_b = seed_building_order(&mut app, colony_b);
    assert_eq!(id_a, id_b, "pre-condition: per-queue counters collide");

    // Dispatch a cancel to system B for order_id=1. System A's order
    // must stay put.
    spawn_pending_remote_command(
        &mut app,
        sys_b,
        0,
        5,
        RemoteCommand::CancelBuildingOrder { order_id: 1 },
    );
    run_until_arrival(&mut app, 5);

    let bq_a = app.world().get::<BuildingQueue>(colony_a).unwrap();
    let bq_b = app.world().get::<BuildingQueue>(colony_b).unwrap();
    assert_eq!(
        bq_a.queue.len(),
        1,
        "system A order must not be cancelled by a dispatch to system B"
    );
    assert!(
        bq_b.queue.is_empty(),
        "system B order should have been cancelled"
    );
    let _ = sys_a; // suppress unused
}

/// #275 Phase 2: `CancelBuildingOrder` also targets the system-level
/// `SystemBuildingQueue` when the id lives there (scope derived at
/// arrival, not at send time).
#[test]
fn cancel_system_level_order() {
    let mut app = build_app();
    let (sys, _planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    // spawn_test_colony attaches SystemBuildings/SystemBuildingQueue.
    let _colony = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::units(1000),
        Amt::units(1000),
        vec![],
    );
    use macrocosmo::colony::BuildingOrder;
    let sys_order_id = {
        let mut sbq = app.world_mut().get_mut::<SystemBuildingQueue>(sys).unwrap();
        sbq.push_build_order(BuildingOrder {
            order_id: 0,
            building_id: BuildingId::new("shipyard"),
            target_slot: 0,
            minerals_remaining: Amt::units(200),
            energy_remaining: Amt::units(100),
            build_time_remaining: 30,
        })
    };

    spawn_pending_remote_command(
        &mut app,
        sys,
        0,
        5,
        RemoteCommand::CancelBuildingOrder {
            order_id: sys_order_id,
        },
    );
    run_until_arrival(&mut app, 5);

    let sbq = app.world().get::<SystemBuildingQueue>(sys).unwrap();
    assert!(
        sbq.queue.is_empty(),
        "system-level order should be cancelled"
    );
}

/// #275 Phase 2: `CancelShipOrder { host_colony, order_id }` removes a
/// pending ship BuildOrder from the specified colony's BuildQueue.
#[test]
fn cancel_ship_order_removes_from_host_colony_queue() {
    let mut app = build_app();
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(10_000),
        Amt::units(10_000),
        vec![],
    );
    // Seed a ship order and grab its id.
    let ship_order_id = {
        use macrocosmo::colony::BuildOrder;
        let mut bq = app.world_mut().get_mut::<BuildQueue>(colony).unwrap();
        bq.push_order(BuildOrder {
            order_id: 0,
            kind: BuildKind::Ship,
            design_id: "explorer_mk1".to_string(),
            display_name: "Explorer".to_string(),
            minerals_cost: Amt::units(500),
            minerals_invested: Amt::ZERO,
            energy_cost: Amt::units(300),
            energy_invested: Amt::ZERO,
            build_time_total: 60,
            build_time_remaining: 60,
        })
    };

    spawn_pending_remote_command(
        &mut app,
        sys,
        0,
        5,
        RemoteCommand::CancelShipOrder {
            host_colony: colony,
            order_id: ship_order_id,
        },
    );
    run_until_arrival(&mut app, 5);

    let bq = app.world().get::<BuildQueue>(colony).unwrap();
    assert!(bq.queue.is_empty(), "ship order should be cancelled");
}

/// #275 Phase 2: Cancelling a ship order with an unknown id is a warn+
/// noop — queue unchanged. Simulates the race where the order already
/// completed (producing a Ship entity) before the cancel arrived.
#[test]
fn cancel_ship_order_with_missing_id_is_noop() {
    let mut app = build_app();
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(10_000),
        Amt::units(10_000),
        vec![],
    );
    // Seed a ship order whose id we will NOT cancel.
    let _real = {
        use macrocosmo::colony::BuildOrder;
        let mut bq = app.world_mut().get_mut::<BuildQueue>(colony).unwrap();
        bq.push_order(BuildOrder {
            order_id: 0,
            kind: BuildKind::Ship,
            design_id: "explorer_mk1".to_string(),
            display_name: "Explorer".to_string(),
            minerals_cost: Amt::units(500),
            minerals_invested: Amt::ZERO,
            energy_cost: Amt::units(300),
            energy_invested: Amt::ZERO,
            build_time_total: 60,
            build_time_remaining: 60,
        })
    };

    spawn_pending_remote_command(
        &mut app,
        sys,
        0,
        5,
        RemoteCommand::CancelShipOrder {
            host_colony: colony,
            order_id: 9999,
        },
    );
    run_until_arrival(&mut app, 5);

    let bq = app.world().get::<BuildQueue>(colony).unwrap();
    assert_eq!(bq.queue.len(), 1, "unrelated ship order must stay put");
}

/// #275 Phase 1: `order_id` and `next_order_id` survive save/load.
#[test]
fn order_id_preserved_across_save_load() {
    use macrocosmo::colony::{BuildOrder, BuildQueue as LiveBuildQueue};
    use macrocosmo::persistence::savebag::{SavedBuildOrder, SavedBuildQueue};

    let live = LiveBuildQueue {
        queue: vec![
            BuildOrder {
                order_id: 7,
                kind: BuildKind::Ship,
                design_id: "x".into(),
                display_name: "x".into(),
                minerals_cost: Amt::units(1),
                minerals_invested: Amt::ZERO,
                energy_cost: Amt::units(1),
                energy_invested: Amt::ZERO,
                build_time_total: 10,
                build_time_remaining: 10,
            },
            BuildOrder {
                order_id: 9,
                kind: BuildKind::Ship,
                design_id: "y".into(),
                display_name: "y".into(),
                minerals_cost: Amt::units(1),
                minerals_invested: Amt::ZERO,
                energy_cost: Amt::units(1),
                energy_invested: Amt::ZERO,
                build_time_total: 10,
                build_time_remaining: 10,
            },
        ],
        next_order_id: 10,
    };
    let saved = SavedBuildQueue::from_live(&live);
    assert_eq!(saved.next_order_id, 10);
    assert_eq!(saved.queue[0].order_id, 7);
    assert_eq!(saved.queue[1].order_id, 9);
    let restored: LiveBuildQueue = saved.into_live();
    assert_eq!(restored.next_order_id, 10);
    assert_eq!(restored.queue[0].order_id, 7);
    assert_eq!(restored.queue[1].order_id, 9);

    // Serde round-trip (bincode) covers the #[serde(default)] paths too.
    let one = SavedBuildOrder::from_live(&BuildOrder {
        order_id: 42,
        kind: BuildKind::Ship,
        design_id: "z".into(),
        display_name: "z".into(),
        minerals_cost: Amt::units(1),
        minerals_invested: Amt::ZERO,
        energy_cost: Amt::units(1),
        energy_invested: Amt::ZERO,
        build_time_total: 1,
        build_time_remaining: 1,
    });
    let json = serde_json::to_string(&one).expect("serialize");
    let back: SavedBuildOrder = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.order_id, 42);
}

// #276: Observer mode / transient empire unavailability
// --------------------------------------------------------------------------

/// Regression for #276: when `PlayerEmpire` is absent (observer mode,
/// load/teardown), the dispatcher must retain the queue instead of
/// clearing it, so queued UI clicks are not silently lost.
#[test]
fn dispatch_preserves_queue_when_empire_absent() {
    let mut app = build_app_with_dispatch();
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let _colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );

    // Despawn the empire to simulate observer / teardown mode.
    let empire = empire_entity(app.world_mut());
    app.world_mut().entity_mut(empire).despawn();

    // Push a click into the queue.
    app.world_mut()
        .resource_mut::<PendingColonyDispatches>()
        .queue
        .push(PendingColonyDispatch {
            target_system: sys,
            command: RemoteCommand::Colony(ColonyCommand {
                scope: BuildingScope::Planet(planet),
                kind: BuildingKind::Queue {
                    building_id: "mine".to_string(),
                    target_slot: 0,
                },
            }),
        });

    // Run a frame with no empire. The queue must be retained.
    advance_time(&mut app, 1);

    let q = app.world().resource::<PendingColonyDispatches>();
    assert_eq!(
        q.queue.len(),
        1,
        "queue must be preserved while PlayerEmpire is absent"
    );
    assert!(q.retry_frames >= 1, "retry_frames should have incremented");

    // No PendingCommand should have been spawned (nothing dispatched).
    let pending_count = app
        .world_mut()
        .query::<&PendingCommand>()
        .iter(app.world())
        .count();
    assert_eq!(
        pending_count, 0,
        "no PendingCommand should be spawned while empire is absent"
    );
}

/// Regression for #276: once the empire reappears, the retained queue
/// should be dispatched on the next frame and `retry_frames` reset.
#[test]
fn dispatch_resumes_after_empire_reappears() {
    let mut app = build_app_with_dispatch();
    let (home_sys, _home_planet) =
        spawn_test_system_with_planet(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true);
    let (target_sys, target_planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let _target_colony = spawn_test_colony(
        app.world_mut(),
        target_planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );

    // Despawn empire; push click; advance one frame (queue retained).
    let empire = empire_entity(app.world_mut());
    app.world_mut().entity_mut(empire).despawn();

    app.world_mut()
        .resource_mut::<PendingColonyDispatches>()
        .queue
        .push(PendingColonyDispatch {
            target_system: target_sys,
            command: RemoteCommand::Colony(ColonyCommand {
                scope: BuildingScope::Planet(target_planet),
                kind: BuildingKind::Queue {
                    building_id: "mine".to_string(),
                    target_slot: 0,
                },
            }),
        });
    advance_time(&mut app, 1);
    assert_eq!(
        app.world()
            .resource::<PendingColonyDispatches>()
            .queue
            .len(),
        1,
        "queue retained while empire absent"
    );

    // Restore the empire + a player stationed somewhere with a resolvable
    // origin, then run another frame — the retained command should drain
    // into a PendingCommand and the retry counter should reset.
    common::spawn_test_empire(app.world_mut());
    app.world_mut()
        .spawn((Player, StationedAt { system: home_sys }));

    advance_time(&mut app, 1);

    let q = app.world().resource::<PendingColonyDispatches>();
    assert!(
        q.queue.is_empty(),
        "queue should drain once empire + player origin are available"
    );
    assert_eq!(
        q.retry_frames, 0,
        "retry_frames should reset after successful dispatch"
    );

    let pending_count = app
        .world_mut()
        .query::<&PendingCommand>()
        .iter(app.world())
        .count();
    assert_eq!(
        pending_count, 1,
        "retained command should have produced a PendingCommand"
    );
}

/// Regression for #276: after `MAX_DISPATCH_RETRY_FRAMES` consecutive
/// failed frames, the queue is dropped so long observation sessions do
/// not accumulate unbounded state.
#[test]
fn dispatch_drops_queue_after_max_retry_frames() {
    let mut app = build_app_with_dispatch();
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [10.0, 0.0, 0.0], 1.0, true);
    let _colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(1000),
        Amt::units(1000),
        vec![None, None, None, None],
    );

    let empire = empire_entity(app.world_mut());
    app.world_mut().entity_mut(empire).despawn();

    app.world_mut()
        .resource_mut::<PendingColonyDispatches>()
        .queue
        .push(PendingColonyDispatch {
            target_system: sys,
            command: RemoteCommand::Colony(ColonyCommand {
                scope: BuildingScope::Planet(planet),
                kind: BuildingKind::Queue {
                    building_id: "mine".to_string(),
                    target_slot: 0,
                },
            }),
        });

    // Pre-seed retry_frames to just below the threshold so we don't need
    // to pump 300 frames in a test.
    app.world_mut()
        .resource_mut::<PendingColonyDispatches>()
        .retry_frames = MAX_DISPATCH_RETRY_FRAMES - 1;

    advance_time(&mut app, 1);

    let q = app.world().resource::<PendingColonyDispatches>();
    assert!(
        q.queue.is_empty(),
        "queue should be dropped once retry window is exhausted"
    );
    assert_eq!(
        q.retry_frames, 0,
        "retry_frames should reset after dropping the queue"
    );
}
