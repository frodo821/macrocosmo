//! #186 Phase 1 — Integration tests for Aggressive ROE hostile detection.

mod common;

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::components::Position;
use macrocosmo::events::{EventLog, GameEventKind};
use macrocosmo::faction::{FactionOwner, FactionRelations, FactionView, RelationState};
use macrocosmo::notifications::{NotificationPriority, NotificationQueue};
use macrocosmo::player::{Faction, PlayerEmpire};
use macrocosmo::ship::*;

use common::{advance_time, test_app_with_event_log};

/// Build a minimal two-faction setup and return their entities.
/// `player_faction` is the PlayerEmpire (spawned by `test_app`).
/// `enemy_faction` is a spawned Faction entity with hostile relations.
fn setup_two_factions(app: &mut App) -> (Entity, Entity) {
    let player_faction = {
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<PlayerEmpire>>();
        q.iter(app.world()).next().expect("player empire must exist")
    };
    let enemy_faction = app
        .world_mut()
        .spawn(Faction {
            id: "enemy_test_faction".into(),
            name: "Enemy Test Faction".into(),
        })
        .id();

    // Seed hostile Neutral + -100 relations in both directions.
    {
        let mut rel = app.world_mut().resource_mut::<FactionRelations>();
        rel.set(
            player_faction,
            enemy_faction,
            FactionView::new(RelationState::Neutral, -100.0),
        );
        rel.set(
            enemy_faction,
            player_faction,
            FactionView::new(RelationState::Neutral, -100.0),
        );
    }

    (player_faction, enemy_faction)
}

fn default_hp() -> ShipHitpoints {
    ShipHitpoints {
        hull: 50.0,
        hull_max: 50.0,
        armor: 0.0,
        armor_max: 0.0,
        shield: 0.0,
        shield_max: 0.0,
        shield_regen: 0.0,
    }
}

fn spawn_sublight_ship(
    app: &mut App,
    name: &str,
    owner: Owner,
    faction_owner: Option<FactionOwner>,
    origin: [f64; 3],
    destination: [f64; 3],
    roe: RulesOfEngagement,
    arrival_at: i64,
) -> Entity {
    let mut e = app.world_mut().spawn((
        Ship {
            name: name.into(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".into(),
            modules: Vec::new(),
            owner,
            sublight_speed: 0.75,
            ftl_range: 10.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
            fleet: None,
        },
        ShipState::SubLight {
            origin,
            destination,
            target_system: None,
            departed_at: 0,
            arrival_at,
        },
        Position::from(origin),
        default_hp(),
        ShipModifiers::default(),
        ShipStats::default(),
        CommandQueue::default(),
        Cargo::default(),
        roe,
    ));
    if let Some(fo) = faction_owner {
        e.insert(fo);
    }
    e.id()
}

fn spawn_loitering_ship(
    app: &mut App,
    name: &str,
    owner: Owner,
    faction_owner: Option<FactionOwner>,
    position: [f64; 3],
    roe: RulesOfEngagement,
) -> Entity {
    let mut e = app.world_mut().spawn((
        Ship {
            name: name.into(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".into(),
            modules: Vec::new(),
            owner,
            sublight_speed: 0.75,
            ftl_range: 10.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
            fleet: None,
        },
        ShipState::Loitering { position },
        Position::from(position),
        default_hp(),
        ShipModifiers::default(),
        ShipStats::default(),
        CommandQueue::default(),
        Cargo::default(),
        roe,
    ));
    if let Some(fo) = faction_owner {
        e.insert(fo);
    }
    e.id()
}

fn count_hostile_detected(app: &App) -> usize {
    app.world()
        .resource::<EventLog>()
        .entries
        .iter()
        .filter(|e| e.kind == GameEventKind::HostileDetected)
        .count()
}

// Register the notifications plumbing (queue + auto_notify + fact drain) on
// top of `test_app_with_event_log` so we can assert notifications too.
//
// #233: `HostileDetected` now surfaces through `PendingFactQueue`, so the
// pursuit tests that assert on banners need both pipelines registered plus
// a `Player` entity so `detect_hostiles_system` can compute an arrival time.
fn test_app_with_notifications() -> App {
    let mut app = test_app_with_event_log();
    app.add_systems(
        Update,
        (
            macrocosmo::notifications::auto_notify_from_events,
            macrocosmo::notifications::notify_from_knowledge_facts,
        )
            .after(macrocosmo::ship::pursuit::detect_hostiles_system),
    );
    // Spawn a minimal Player + capital system at origin so the new #233 fact
    // pipeline has a target coordinate. Place the player at the same origin
    // as the detector so local notifications surface instantly.
    let system = app
        .world_mut()
        .spawn(Position::from([0.0, 0.0, 0.0]))
        .id();
    app.world_mut().spawn((
        macrocosmo::player::Player,
        macrocosmo::player::StationedAt { system },
    ));
    app
}

// --------- Positive detection scenarios ---------

#[test]
fn aggressive_sublight_detects_hostile_sublight_in_range() {
    let mut app = test_app_with_notifications();
    let (player_f, enemy_f) = setup_two_factions(&mut app);

    // Detector: player Aggressive, sublight, near origin.
    spawn_sublight_ship(
        &mut app,
        "Scout",
        Owner::Empire(player_f),
        None,
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        RulesOfEngagement::Aggressive,
        240,
    );
    // Target: enemy SubLight, 1 ly away at game start.
    // Defensive to ensure only the Scout detects (one-way), not mutual detection.
    spawn_sublight_ship(
        &mut app,
        "Raider",
        Owner::Empire(enemy_f),
        None,
        [1.0, 0.0, 0.0],
        [5.0, 0.0, 0.0],
        RulesOfEngagement::Defensive,
        400,
    );

    // Detection is immediate (GameEvent + EventLog); the notification is
    // routed through PendingFactQueue and surfaces after the light delay
    // between the target (1 ly away) and the player at origin (~60 hd).
    advance_time(&mut app, 1);
    assert_eq!(count_hostile_detected(&app), 1);
    // Fact queue should have the HostileDetected fact scheduled for arrival.
    assert_eq!(
        app.world()
            .resource::<macrocosmo::knowledge::PendingFactQueue>()
            .pending_len(),
        1,
        "HostileDetected must be recorded in PendingFactQueue"
    );

    // Advance past the light-speed arrival to drain the fact. Target is ~1 ly
    // from player and drifting in SubLight, so actual delay is slightly over
    // 60 hd (distance accumulates). 120 hd is a comfortable margin.
    advance_time(&mut app, 120);

    let q = app.world().resource::<NotificationQueue>();
    let notif = q
        .items
        .iter()
        .find(|n| n.title == "Hostile Detected")
        .expect("HostileDetected must produce a banner after light delay");
    assert_eq!(notif.priority, NotificationPriority::High);
    assert!(notif.description.contains("Scout"));
    assert!(notif.description.contains("Raider"));
}

#[test]
fn aggressive_loitering_detects_hostile_sublight_in_range() {
    let mut app = test_app_with_notifications();
    let (player_f, enemy_f) = setup_two_factions(&mut app);

    spawn_loitering_ship(
        &mut app,
        "Picket",
        Owner::Empire(player_f),
        None,
        [0.0, 0.0, 0.0],
        RulesOfEngagement::Aggressive,
    );
    spawn_sublight_ship(
        &mut app,
        "Enemy-SL",
        Owner::Empire(enemy_f),
        None,
        [1.5, 0.0, 0.0],
        [5.0, 0.0, 0.0],
        RulesOfEngagement::Defensive,
        400,
    );

    advance_time(&mut app, 1);
    assert_eq!(count_hostile_detected(&app), 1);
}

#[test]
fn aggressive_detects_hostile_via_faction_owner_on_neutral_owner() {
    // Target: Owner::Neutral but carries FactionOwner (mirrors the
    // HostilePresence migration pattern for non-empire ships).
    let mut app = test_app_with_notifications();
    let (player_f, enemy_f) = setup_two_factions(&mut app);

    spawn_loitering_ship(
        &mut app,
        "Patrol",
        Owner::Empire(player_f),
        None,
        [0.0, 0.0, 0.0],
        RulesOfEngagement::Aggressive,
    );
    spawn_loitering_ship(
        &mut app,
        "Beast",
        Owner::Neutral,
        Some(FactionOwner(enemy_f)),
        [2.0, 0.0, 0.0],
        RulesOfEngagement::Defensive,
    );

    advance_time(&mut app, 1);
    assert_eq!(count_hostile_detected(&app), 1);
}

// --------- Negative (must NOT detect) scenarios ---------

#[test]
fn ftl_ships_are_invisible_to_detector() {
    let mut app = test_app_with_notifications();
    let (player_f, enemy_f) = setup_two_factions(&mut app);
    let sys_a = app.world_mut().spawn(Position::from([0.0, 0.0, 0.0])).id();
    let sys_b = app.world_mut().spawn(Position::from([5.0, 0.0, 0.0])).id();

    // Detector is itself in FTL — it should not detect anything.
    app.world_mut().spawn((
        Ship {
            name: "FtlPatrol".into(),
            design_id: "explorer_mk1".into(),
            hull_id: "corvette".into(),
            modules: Vec::new(),
            owner: Owner::Empire(player_f),
            sublight_speed: 0.75,
            ftl_range: 10.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
            fleet: None,
        },
        ShipState::InFTL {
            origin_system: sys_a,
            destination_system: sys_b,
            departed_at: 0,
            arrival_at: 60,
        },
        Position::from([0.0, 0.0, 0.0]),
        default_hp(),
        ShipModifiers::default(),
        ShipStats::default(),
        CommandQueue::default(),
        Cargo::default(),
        RulesOfEngagement::Aggressive,
    ));
    // And an enemy in FTL — also invisible when the detector is also Aggressive
    // and deep-space, so add one of each to test both directions.
    spawn_loitering_ship(
        &mut app,
        "LoiterFriendly",
        Owner::Empire(player_f),
        None,
        [0.0, 0.0, 0.0],
        RulesOfEngagement::Aggressive,
    );
    app.world_mut().spawn((
        Ship {
            name: "EnemyFtl".into(),
            design_id: "explorer_mk1".into(),
            hull_id: "corvette".into(),
            modules: Vec::new(),
            owner: Owner::Empire(enemy_f),
            sublight_speed: 0.75,
            ftl_range: 10.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
            fleet: None,
        },
        ShipState::InFTL {
            origin_system: sys_a,
            destination_system: sys_b,
            departed_at: 0,
            arrival_at: 60,
        },
        Position::from([1.0, 0.0, 0.0]),
        default_hp(),
        ShipModifiers::default(),
        ShipStats::default(),
        CommandQueue::default(),
        Cargo::default(),
        RulesOfEngagement::Defensive,
    ));

    advance_time(&mut app, 1);
    assert_eq!(
        count_hostile_detected(&app),
        0,
        "FTL ships must not participate in Phase 1 detection"
    );
}

#[test]
fn defensive_and_retreat_detectors_do_not_fire() {
    let mut app = test_app_with_notifications();
    let (player_f, enemy_f) = setup_two_factions(&mut app);

    spawn_loitering_ship(
        &mut app,
        "Defender",
        Owner::Empire(player_f),
        None,
        [0.0, 0.0, 0.0],
        RulesOfEngagement::Defensive,
    );
    spawn_loitering_ship(
        &mut app,
        "Runner",
        Owner::Empire(player_f),
        None,
        [0.1, 0.0, 0.0],
        RulesOfEngagement::Retreat,
    );
    spawn_loitering_ship(
        &mut app,
        "Enemy",
        Owner::Empire(enemy_f),
        None,
        [0.5, 0.0, 0.0],
        RulesOfEngagement::Defensive,
    );

    advance_time(&mut app, 1);
    assert_eq!(count_hostile_detected(&app), 0);
}

#[test]
fn non_hostile_standing_is_not_detected() {
    // Two factions with Neutral + standing = 0  → can_attack_aggressive = false.
    let mut app = test_app_with_notifications();
    let player_f = {
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<PlayerEmpire>>();
        q.iter(app.world()).next().unwrap()
    };
    let other_f = app
        .world_mut()
        .spawn(Faction {
            id: "friendly_neutral".into(),
            name: "Friendly".into(),
        })
        .id();
    // Leave relations empty → get_or_default returns Neutral/0 → hostile check fails.

    spawn_loitering_ship(
        &mut app,
        "Patrol",
        Owner::Empire(player_f),
        None,
        [0.0, 0.0, 0.0],
        RulesOfEngagement::Aggressive,
    );
    spawn_loitering_ship(
        &mut app,
        "NeutralShip",
        Owner::Empire(other_f),
        None,
        [1.0, 0.0, 0.0],
        RulesOfEngagement::Defensive,
    );

    advance_time(&mut app, 1);
    assert_eq!(count_hostile_detected(&app), 0);
}

#[test]
fn out_of_range_target_is_not_detected() {
    let mut app = test_app_with_notifications();
    let (player_f, enemy_f) = setup_two_factions(&mut app);

    spawn_loitering_ship(
        &mut app,
        "Patrol",
        Owner::Empire(player_f),
        None,
        [0.0, 0.0, 0.0],
        RulesOfEngagement::Aggressive,
    );
    // Well beyond DEFAULT_DETECTION_RANGE_LY = 3.0 ly.
    spawn_loitering_ship(
        &mut app,
        "DistantEnemy",
        Owner::Empire(enemy_f),
        None,
        [10.0, 0.0, 0.0],
        RulesOfEngagement::Defensive,
    );

    advance_time(&mut app, 1);
    assert_eq!(count_hostile_detected(&app), 0);
}

// --------- Duplicate suppression ---------

#[test]
fn duplicate_detection_is_suppressed_within_cooldown() {
    let mut app = test_app_with_notifications();
    let (player_f, enemy_f) = setup_two_factions(&mut app);

    let detector = spawn_loitering_ship(
        &mut app,
        "Patrol",
        Owner::Empire(player_f),
        None,
        [0.0, 0.0, 0.0],
        RulesOfEngagement::Aggressive,
    );
    spawn_loitering_ship(
        &mut app,
        "Raider",
        Owner::Empire(enemy_f),
        None,
        [1.5, 0.0, 0.0],
        RulesOfEngagement::Defensive,
    );

    // First tick: fires.
    advance_time(&mut app, 1);
    assert_eq!(count_hostile_detected(&app), 1);

    // Repeated ticks within the cooldown must not produce additional events.
    for _ in 0..20 {
        advance_time(&mut app, 1);
    }
    assert_eq!(
        count_hostile_detected(&app),
        1,
        "within cooldown, repeat detections must not fire new events"
    );
    // The detector accumulated a DetectedHostiles component.
    assert!(
        app.world().get::<pursuit::DetectedHostiles>(detector).is_some(),
        "detector must be tagged with DetectedHostiles"
    );

    // After cooldown elapses, a re-detection fires again.
    advance_time(&mut app, pursuit::DETECTION_COOLDOWN_HEXADIES);
    assert!(
        count_hostile_detected(&app) >= 2,
        "post-cooldown detection must fire again"
    );
}

// Sanity: silence `Amt` unused import warning in some builds.
#[allow(dead_code)]
fn _unused_amt() -> Amt {
    Amt::ZERO
}
