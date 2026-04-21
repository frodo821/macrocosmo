//! #303 (S-10): Integration tests for sovereignty change detection,
//! cascade, and Lua event dispatch.

mod common;

use bevy::prelude::*;
use common::{
    advance_time, empire_entity, full_test_app, spawn_mock_core_ship, spawn_test_colony,
    spawn_test_system,
};
use macrocosmo::amount::Amt;
use macrocosmo::colony::authority::{SOVEREIGNTY_CHANGED_EVENT, SovereigntyChangeReason};
use macrocosmo::event_system::EventSystem;
use macrocosmo::faction::FactionOwner;
use macrocosmo::galaxy::Sovereignty;
use macrocosmo::player::{Empire, Faction};
use macrocosmo::ship::{Owner, Ship, ShipState};

/// Helper: spawn a second faction entity (non-player) for conquest tests.
fn spawn_rival_faction(world: &mut World) -> Entity {
    world
        .spawn((
            Empire {
                name: "Rival Empire".to_string(),
            },
            Faction::new("rival", "Rival Faction"),
        ))
        .id()
}

// =========================================================================
// Display trait
// =========================================================================

#[test]
fn test_sovereignty_changed_reason_display() {
    assert_eq!(SovereigntyChangeReason::Conquest.to_string(), "conquest");
    assert_eq!(SovereigntyChangeReason::Cession.to_string(), "cession");
    assert_eq!(
        SovereigntyChangeReason::Abandonment.to_string(),
        "abandonment"
    );
    assert_eq!(SovereigntyChangeReason::Secession.to_string(), "secession");
    assert_eq!(SovereigntyChangeReason::Initial.to_string(), "initial");
}

// =========================================================================
// Event detection
// =========================================================================

/// Core deployed in unclaimed system -> event fires with reason=Initial.
#[test]
fn test_initial_sovereignty_fires_event() {
    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "NewClaim",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let empire = empire_entity(app.world_mut());
    spawn_mock_core_ship(app.world_mut(), sys, empire);

    advance_time(&mut app, 1);

    let es = app.world().resource::<EventSystem>();
    let sov_events: Vec<_> = es
        .fired_log
        .iter()
        .filter(|e| e.event_id == SOVEREIGNTY_CHANGED_EVENT && e.payload.is_some())
        .collect();
    assert_eq!(sov_events.len(), 1, "exactly one sovereignty event");
    let ctx = sov_events[0].payload.as_ref().expect("payload attached");
    assert_eq!(ctx.payload_get("reason").as_deref(), Some("initial"));
    assert_eq!(ctx.payload_get("system_name").as_deref(), Some("NewClaim"));
    assert!(ctx.payload_get("new_owner_id").is_some());
    assert!(ctx.payload_get("previous_owner_id").is_none());
}

/// System changes from faction A to B -> event fires with reason=Conquest.
#[test]
fn test_conquest_sovereignty_fires_event() {
    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "Contested",
        [10.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let empire = empire_entity(app.world_mut());
    let core_a = spawn_mock_core_ship(app.world_mut(), sys, empire);

    advance_time(&mut app, 1);

    // Verify initial claim.
    let sov = app.world().get::<Sovereignty>(sys).unwrap();
    assert_eq!(sov.owner, Some(Owner::Empire(empire)));

    // Remove old Core, deploy rival Core.
    app.world_mut().despawn(core_a);
    let rival = spawn_rival_faction(app.world_mut());
    spawn_mock_core_ship(app.world_mut(), sys, rival);

    // Clear event log so we only see the conquest event.
    app.world_mut()
        .resource_mut::<EventSystem>()
        .fired_log
        .clear();

    advance_time(&mut app, 1);

    let es = app.world().resource::<EventSystem>();
    let sov_events: Vec<_> = es
        .fired_log
        .iter()
        .filter(|e| e.event_id == SOVEREIGNTY_CHANGED_EVENT && e.payload.is_some())
        .collect();
    // One abandonment (old owner lost) and then one conquest (new owner gained)
    // actually happens in two steps: despawn core -> update_sovereignty sees None
    // (abandonment), then next tick after rival core placed -> update_sovereignty
    // sees rival (initial for that faction).
    // BUT since we despawned + spawned in the same frame, update_sovereignty
    // runs once and sees the rival directly. The transition is
    // Some(empire) -> Some(rival) = Conquest.
    assert_eq!(sov_events.len(), 1, "exactly one sovereignty event");
    let ctx = sov_events[0].payload.as_ref().expect("payload attached");
    assert_eq!(ctx.payload_get("reason").as_deref(), Some("conquest"));
}

/// Core removed -> event fires with reason=Abandonment.
#[test]
fn test_abandonment_sovereignty_fires_event() {
    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "Abandoned",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let empire = empire_entity(app.world_mut());
    let core = spawn_mock_core_ship(app.world_mut(), sys, empire);

    advance_time(&mut app, 1);

    // Clear initial event.
    app.world_mut()
        .resource_mut::<EventSystem>()
        .fired_log
        .clear();

    app.world_mut().despawn(core);
    advance_time(&mut app, 1);

    let es = app.world().resource::<EventSystem>();
    let sov_events: Vec<_> = es
        .fired_log
        .iter()
        .filter(|e| e.event_id == SOVEREIGNTY_CHANGED_EVENT && e.payload.is_some())
        .collect();
    assert_eq!(sov_events.len(), 1, "exactly one abandonment event");
    let ctx = sov_events[0].payload.as_ref().expect("payload attached");
    assert_eq!(ctx.payload_get("reason").as_deref(), Some("abandonment"));
    assert!(ctx.payload_get("new_owner_id").is_none());
}

/// Same owner across ticks -> no event.
#[test]
fn test_no_event_when_owner_unchanged() {
    let mut app = full_test_app();
    let sys = spawn_test_system(app.world_mut(), "Stable", [0.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());
    spawn_mock_core_ship(app.world_mut(), sys, empire);

    advance_time(&mut app, 1);

    // Clear initial event.
    app.world_mut()
        .resource_mut::<EventSystem>()
        .fired_log
        .clear();

    advance_time(&mut app, 1);
    advance_time(&mut app, 1);

    let es = app.world().resource::<EventSystem>();
    let sov_events: Vec<_> = es
        .fired_log
        .iter()
        .filter(|e| e.event_id == SOVEREIGNTY_CHANGED_EVENT && e.payload.is_some())
        .collect();
    assert_eq!(sov_events.len(), 0, "no events when owner unchanged");
}

// =========================================================================
// Cascade tests
// =========================================================================

/// Sovereignty change updates Colony's FactionOwner.
#[test]
fn test_cascade_colony_faction_owner() {
    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "ColonyTest",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let empire = empire_entity(app.world_mut());
    let core_a = spawn_mock_core_ship(app.world_mut(), sys, empire);

    // Spawn a colony with FactionOwner set to the original empire.
    let colony_entity = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::units(100),
        Amt::units(100),
        vec![],
    );
    app.world_mut()
        .entity_mut(colony_entity)
        .insert(FactionOwner(empire));

    advance_time(&mut app, 1);

    // Now swap sovereignty: despawn old Core, deploy rival Core.
    app.world_mut().despawn(core_a);
    let rival = spawn_rival_faction(app.world_mut());
    spawn_mock_core_ship(app.world_mut(), sys, rival);

    advance_time(&mut app, 1);

    let colony_fo = app.world().get::<FactionOwner>(colony_entity).unwrap();
    assert_eq!(
        colony_fo.0, rival,
        "colony FactionOwner should be updated to rival"
    );
}

/// StarSystem entity's FactionOwner updated.
#[test]
fn test_cascade_system_buildings_faction_owner() {
    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "SysBldgTest",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let empire = empire_entity(app.world_mut());
    let core_a = spawn_mock_core_ship(app.world_mut(), sys, empire);

    // Give the system a FactionOwner.
    app.world_mut().entity_mut(sys).insert(FactionOwner(empire));

    advance_time(&mut app, 1);

    // Swap sovereignty.
    app.world_mut().despawn(core_a);
    let rival = spawn_rival_faction(app.world_mut());
    spawn_mock_core_ship(app.world_mut(), sys, rival);

    advance_time(&mut app, 1);

    let sys_fo = app.world().get::<FactionOwner>(sys).unwrap();
    assert_eq!(
        sys_fo.0, rival,
        "system FactionOwner should be updated to rival"
    );
}

/// Docked ships get new FactionOwner + Ship.owner.
#[test]
fn test_cascade_docked_ships_faction_owner() {
    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "DockedShipTest",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let empire = empire_entity(app.world_mut());
    let core_a = spawn_mock_core_ship(app.world_mut(), sys, empire);

    // Spawn a docked ship.
    let ship_entity = app
        .world_mut()
        .spawn((
            Ship {
                name: "TestShip".to_string(),
                design_id: "corvette".to_string(),
                hull_id: "corvette".to_string(),
                modules: vec![],
                owner: Owner::Empire(empire),
                sublight_speed: 1.0,
                ftl_range: 5.0,
                ruler_aboard: false,
                home_port: sys,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system: sys },
            FactionOwner(empire),
        ))
        .id();

    advance_time(&mut app, 1);

    // Swap sovereignty.
    app.world_mut().despawn(core_a);
    let rival = spawn_rival_faction(app.world_mut());
    spawn_mock_core_ship(app.world_mut(), sys, rival);

    advance_time(&mut app, 1);

    let ship_fo = app.world().get::<FactionOwner>(ship_entity).unwrap();
    assert_eq!(
        ship_fo.0, rival,
        "docked ship FactionOwner should be updated"
    );
    let ship = app.world().get::<Ship>(ship_entity).unwrap();
    assert_eq!(
        ship.owner,
        Owner::Empire(rival),
        "docked ship Ship.owner should be updated"
    );
}

/// Ships in SubLight/FTL state at the system retain original owner.
#[test]
fn test_cascade_skips_in_transit_ships() {
    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "TransitTest",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let sys2 = spawn_test_system(app.world_mut(), "Dest", [100.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());
    let core_a = spawn_mock_core_ship(app.world_mut(), sys, empire);

    // Spawn a ship in SubLight (in-transit).
    let ship_entity = app
        .world_mut()
        .spawn((
            Ship {
                name: "Transit".to_string(),
                design_id: "corvette".to_string(),
                hull_id: "corvette".to_string(),
                modules: vec![],
                owner: Owner::Empire(empire),
                sublight_speed: 1.0,
                ftl_range: 5.0,
                ruler_aboard: false,
                home_port: sys,
                design_revision: 0,
                fleet: None,
            },
            ShipState::SubLight {
                origin: [0.0, 0.0, 0.0],
                destination: [100.0, 0.0, 0.0],
                target_system: Some(sys2),
                departed_at: 0,
                arrival_at: 100,
            },
            FactionOwner(empire),
        ))
        .id();

    advance_time(&mut app, 1);

    // Swap sovereignty.
    app.world_mut().despawn(core_a);
    let rival = spawn_rival_faction(app.world_mut());
    spawn_mock_core_ship(app.world_mut(), sys, rival);

    advance_time(&mut app, 1);

    // In-transit ship should NOT be cascaded.
    let ship_fo = app.world().get::<FactionOwner>(ship_entity).unwrap();
    assert_eq!(
        ship_fo.0, empire,
        "in-transit ship should keep original FactionOwner"
    );
    let ship = app.world().get::<Ship>(ship_entity).unwrap();
    assert_eq!(
        ship.owner,
        Owner::Empire(empire),
        "in-transit ship should keep original Ship.owner"
    );
}

/// When new_owner=None (abandonment), child entities keep previous FactionOwner.
#[test]
fn test_abandonment_preserves_faction_owner() {
    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "AbandonTest",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let empire = empire_entity(app.world_mut());
    let core = spawn_mock_core_ship(app.world_mut(), sys, empire);

    // Spawn colony with FactionOwner.
    let colony_entity = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::units(100),
        Amt::units(100),
        vec![],
    );
    app.world_mut()
        .entity_mut(colony_entity)
        .insert(FactionOwner(empire));

    // Spawn docked ship.
    let ship_entity = app
        .world_mut()
        .spawn((
            Ship {
                name: "Orphan".to_string(),
                design_id: "corvette".to_string(),
                hull_id: "corvette".to_string(),
                modules: vec![],
                owner: Owner::Empire(empire),
                sublight_speed: 1.0,
                ftl_range: 5.0,
                ruler_aboard: false,
                home_port: sys,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system: sys },
            FactionOwner(empire),
        ))
        .id();

    // Give system FactionOwner.
    app.world_mut().entity_mut(sys).insert(FactionOwner(empire));

    advance_time(&mut app, 1);

    // Abandon: despawn Core.
    app.world_mut().despawn(core);
    advance_time(&mut app, 1);

    // All children should retain empire as FactionOwner.
    let colony_fo = app.world().get::<FactionOwner>(colony_entity).unwrap();
    assert_eq!(
        colony_fo.0, empire,
        "colony should keep FactionOwner on abandonment"
    );
    let ship_fo = app.world().get::<FactionOwner>(ship_entity).unwrap();
    assert_eq!(
        ship_fo.0, empire,
        "ship should keep FactionOwner on abandonment"
    );
    let sys_fo = app.world().get::<FactionOwner>(sys).unwrap();
    assert_eq!(
        sys_fo.0, empire,
        "system should keep FactionOwner on abandonment"
    );
}
