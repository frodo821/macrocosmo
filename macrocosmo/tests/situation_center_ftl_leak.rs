//! #491 PR-4: The Situation Center "Ship Operations" tab must not leak
//! the realtime ECS [`ShipState`] for own-empire ships. The classifier
//! routes through the viewing empire's [`KnowledgeStore`] — projections
//! for own ships, snapshots for foreign ships — exactly the way the
//! outline tree does (#487, `outline_tree_ftl_leak.rs`).
//!
//! These tests pin the **classifier** layer
//! (`ShipOperationsTab::collect` / `badge`) by spawning ships +
//! projections / snapshots and asserting the produced
//! [`crate::ui::situation_center::Event`] tree.

mod common;

use bevy::prelude::*;

use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, ShipProjection, ShipSnapshot, ShipSnapshotState,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::fleet::{Fleet, FleetMembers};
use macrocosmo::ship::{
    Cargo, CommandQueue, Owner, RulesOfEngagement, Ship, ShipHitpoints, ShipModifiers, ShipState,
};
use macrocosmo::ui::situation_center::{EventKind, OngoingTab, ShipOperationsTab};

use common::{spawn_test_system, test_app};

// ---------------------------------------------------------------------------
// Helpers (mirror the outline_tree_ftl_leak fixtures)
// ---------------------------------------------------------------------------

fn spawn_minimal_player_empire(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "ship_ops_test".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            KnowledgeStore::default(),
            macrocosmo::knowledge::SystemVisibilityMap::default(),
        ))
        .id()
}

fn spawn_foreign_empire(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            Empire {
                name: "Foreign".into(),
            },
            Faction {
                id: "foreign".into(),
                name: "Foreign".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            KnowledgeStore::default(),
            macrocosmo::knowledge::SystemVisibilityMap::default(),
        ))
        .id()
}

/// Hand-rolled ship spawn — avoids pulling in the design registry. Only
/// the `Ship`/`ShipState`/owner Components matter for the classifier.
fn spawn_test_ship(world: &mut World, name: &str, owner: Entity, system: Entity) -> Entity {
    let ship_entity = world.spawn_empty().id();
    let fleet_entity = world.spawn_empty().id();
    world.entity_mut(ship_entity).insert((
        Ship {
            name: name.into(),
            design_id: "explorer_mk1".into(),
            hull_id: "frigate".into(),
            modules: Vec::new(),
            owner: Owner::Empire(owner),
            sublight_speed: 1.0,
            ftl_range: 5.0,
            ruler_aboard: false,
            home_port: system,
            design_revision: 0,
            fleet: Some(fleet_entity),
        },
        ShipState::InSystem { system },
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
        ShipModifiers::default(),
        macrocosmo::ship::ShipStats::default(),
        RulesOfEngagement::default(),
    ));
    world.entity_mut(fleet_entity).insert((
        Fleet {
            name: name.into(),
            flagship: Some(ship_entity),
        },
        FleetMembers(vec![ship_entity]),
    ));
    ship_entity
}

fn set_ship_state(world: &mut World, ship: Entity, new_state: ShipState) {
    *world.get_mut::<ShipState>(ship).unwrap() = new_state;
}

/// Recursively flatten an event tree to all leaves with a non-`None` source.
/// Returns `(label, kind)` tuples. Used for assertions that don't care
/// about the per-category roll-up structure.
fn flatten_ship_leaves(
    events: &[macrocosmo::ui::situation_center::Event],
) -> Vec<(String, EventKind)> {
    let mut out = Vec::new();
    for e in events {
        if e.children.is_empty() {
            if matches!(
                e.source,
                macrocosmo::ui::situation_center::EventSource::Ship(_)
            ) {
                out.push((e.label.clone(), e.kind));
            }
        } else {
            out.extend(flatten_ship_leaves(&e.children));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 1. FTL leak regression — own ship, projection InSystem, realtime SubLight
// ---------------------------------------------------------------------------

/// At dispatcher tick T+1 after dispatching a sublight move at T, the
/// realtime ECS state has the ship in `SubLight` — but the projection
/// still says `InSystem`. The Ship Operations tab must classify the
/// ship as `Other` ("docked"), NOT `Travel` ("sublight transit").
#[test]
fn ship_ops_tab_classifier_uses_projection() {
    let mut app = test_app();
    let empire = spawn_minimal_player_empire(&mut app);
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [50.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );

    let ship = spawn_test_ship(app.world_mut(), "Explorer-1", empire, home);

    // Projection: ship still believed at home (= dispatcher's tick has
    // not yet reached the ship from its POV).
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: Some(10),
            expected_return_at: Some(20),
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            intended_state: Some(ShipSnapshotState::InTransitSubLight),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(5),
        });
    }

    // Realtime ECS advances ahead of the projection (the FTL leak window).
    set_ship_state(
        app.world_mut(),
        ship,
        ShipState::SubLight {
            origin: [0.0, 0.0, 0.0],
            destination: [50.0, 0.0, 0.0],
            target_system: Some(frontier),
            departed_at: 0,
            arrival_at: 50,
        },
    );

    let tab = ShipOperationsTab;
    let events = tab.collect(app.world());
    let leaves = flatten_ship_leaves(&events);

    assert_eq!(leaves.len(), 1, "exactly one ship leaf expected");
    let (label, kind) = &leaves[0];
    assert_eq!(
        *kind,
        EventKind::Other,
        "FTL leak regression: projection says InSystem → Other category, got {:?} (label={})",
        kind,
        label
    );
    assert!(
        label.contains("docked"),
        "leaf must read 'docked', not 'sublight transit'. label={}",
        label
    );
    assert!(
        !label.contains("sublight") && !label.contains("FTL"),
        "leaf must not surface realtime transit terms. label={}",
        label
    );
}

// ---------------------------------------------------------------------------
// 2. InTransitFTL classifier — distinct label
// ---------------------------------------------------------------------------

/// Once `poll_pending_routes` upgrades the projection to `InTransitFTL`,
/// the classifier must surface "in FTL" — distinct from sublight
/// transit so the player can see the FTL/SubLight distinction.
#[test]
fn ship_ops_tab_classifies_intransit_ftl_distinctly() {
    let mut app = test_app();
    let empire = spawn_minimal_player_empire(&mut app);
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [50.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );

    let ship = spawn_test_ship(app.world_mut(), "FTL-Cruiser", empire, home);

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: Some(10),
            expected_return_at: Some(20),
            projected_state: ShipSnapshotState::InTransitFTL,
            projected_system: Some(frontier),
            intended_state: Some(ShipSnapshotState::Surveying),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(10),
        });
    }
    // Realtime: actually in FTL (= confluent with projection).
    set_ship_state(
        app.world_mut(),
        ship,
        ShipState::InFTL {
            origin_system: home,
            destination_system: frontier,
            departed_at: 0,
            arrival_at: 10,
        },
    );

    let tab = ShipOperationsTab;
    let events = tab.collect(app.world());
    let leaves = flatten_ship_leaves(&events);

    assert_eq!(leaves.len(), 1);
    let (label, kind) = &leaves[0];
    assert_eq!(*kind, EventKind::Travel);
    assert!(
        label.contains("in FTL"),
        "InTransitFTL must surface 'in FTL', got {:?}",
        label
    );
    assert!(
        !label.contains("sublight"),
        "FTL ship must not be labelled sublight, got {:?}",
        label
    );

    // ETA lifted from projection.expected_arrival_at.
    let leaf = events
        .iter()
        .flat_map(|e| e.children.iter())
        .next()
        .expect("travel leaf");
    assert_eq!(leaf.eta, Some(10), "ETA must come from projection");
}

// ---------------------------------------------------------------------------
// 3. Foreign ship — uses snapshot
// ---------------------------------------------------------------------------

/// Foreign ships flow through `ship_snapshots`. The classifier must
/// surface the snapshot's `last_known_state`, not the realtime ECS.
#[test]
fn ship_ops_tab_foreign_ship_uses_snapshot() {
    let mut app = test_app();
    let viewing = spawn_minimal_player_empire(&mut app);
    let foreign = spawn_foreign_empire(&mut app);

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let foreign_sys = spawn_test_system(
        app.world_mut(),
        "ForeignSys",
        [100.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );

    let foreign_ship = spawn_test_ship(app.world_mut(), "EnemyScout", foreign, foreign_sys);

    // Viewing empire's snapshot says "surveying".
    {
        let mut em = app.world_mut().entity_mut(viewing);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_ship(ShipSnapshot {
            entity: foreign_ship,
            name: "EnemyScout".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::Surveying,
            last_known_system: Some(foreign_sys),
            observed_at: 0,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });
    }

    // Realtime moves on — snapshot must dominate.
    set_ship_state(
        app.world_mut(),
        foreign_ship,
        ShipState::InSystem { system: home },
    );

    let tab = ShipOperationsTab;
    let events = tab.collect(app.world());
    let leaves = flatten_ship_leaves(&events);

    assert_eq!(leaves.len(), 1, "foreign ship must surface via snapshot");
    let (label, kind) = &leaves[0];
    assert_eq!(*kind, EventKind::Survey);
    assert!(
        label.contains("surveying"),
        "snapshot says Surveying, got {:?}",
        label
    );
}

// ---------------------------------------------------------------------------
// 4. Destroyed projection — filtered out
// ---------------------------------------------------------------------------

/// `Destroyed` and `Missing` are non-actionable terminal states — the
/// classifier filters them out so they don't inflate the badge count.
#[test]
fn ship_ops_tab_destroyed_ship_filtered_out() {
    let mut app = test_app();
    let empire = spawn_minimal_player_empire(&mut app);
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);

    let ship = spawn_test_ship(app.world_mut(), "Doomed", empire, home);

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: None,
            expected_return_at: None,
            projected_state: ShipSnapshotState::Destroyed,
            projected_system: None,
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
    }

    let tab = ShipOperationsTab;
    let events = tab.collect(app.world());
    let leaves = flatten_ship_leaves(&events);

    assert!(
        leaves.is_empty(),
        "Destroyed projection must not surface in Ship Operations tab. Leaves: {:?}",
        leaves
    );

    // Badge must also drop the Destroyed ship — total count = 0 → no badge.
    assert!(
        tab.badge(app.world()).is_none(),
        "badge must be None when only ship is Destroyed"
    );
}

// ---------------------------------------------------------------------------
// 5. Mixed fleet — summary reflects projection-mediated view
// ---------------------------------------------------------------------------

/// Three ships across own/foreign/destroyed paths. The summary
/// (= badge counts) must match the projection-/snapshot-mediated
/// classification, not the realtime ECS state.
#[test]
fn ship_ops_tab_summary_reflects_projection() {
    let mut app = test_app();
    let player = spawn_minimal_player_empire(&mut app);
    let foreign = spawn_foreign_empire(&mut app);

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [50.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let foreign_sys = spawn_test_system(
        app.world_mut(),
        "ForeignSys",
        [100.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );

    // Own ship #1: projection says InTransitFTL → Travel.
    let own_ftl = spawn_test_ship(app.world_mut(), "Own-FTL", player, home);
    // Own ship #2: projection says InSystem (= docked, despite realtime SubLight).
    let own_docked = spawn_test_ship(app.world_mut(), "Own-Docked", player, home);
    // Own ship #3: Destroyed projection — filtered out.
    let own_destroyed = spawn_test_ship(app.world_mut(), "Own-Doomed", player, home);
    // Foreign ship: snapshot says Surveying → Survey.
    let foreign_ship = spawn_test_ship(app.world_mut(), "Foreign-Scout", foreign, foreign_sys);

    {
        let mut em = app.world_mut().entity_mut(player);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: own_ftl,
            dispatched_at: 0,
            expected_arrival_at: Some(10),
            expected_return_at: Some(20),
            projected_state: ShipSnapshotState::InTransitFTL,
            projected_system: Some(frontier),
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
        store.update_projection(ShipProjection {
            entity: own_docked,
            dispatched_at: 0,
            expected_arrival_at: None,
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
        store.update_projection(ShipProjection {
            entity: own_destroyed,
            dispatched_at: 0,
            expected_arrival_at: None,
            expected_return_at: None,
            projected_state: ShipSnapshotState::Destroyed,
            projected_system: None,
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
        store.update_ship(ShipSnapshot {
            entity: foreign_ship,
            name: "Foreign-Scout".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::Surveying,
            last_known_system: Some(foreign_sys),
            observed_at: 0,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });
    }

    // Realtime ECS leaks — own-docked is actually SubLight, own-ftl
    // realtime says InSystem (= reconcile lag). Classifier must read
    // projection.
    set_ship_state(
        app.world_mut(),
        own_docked,
        ShipState::SubLight {
            origin: [0.0, 0.0, 0.0],
            destination: [50.0, 0.0, 0.0],
            target_system: Some(frontier),
            departed_at: 0,
            arrival_at: 50,
        },
    );

    let tab = ShipOperationsTab;
    let events = tab.collect(app.world());
    let leaves = flatten_ship_leaves(&events);

    // 4 ships spawned, 1 destroyed → 3 visible leaves.
    assert_eq!(
        leaves.len(),
        3,
        "expect 3 leaves (FTL + docked + foreign survey); destroyed filtered. Got: {:?}",
        leaves
    );

    let by_kind: Vec<EventKind> = leaves.iter().map(|(_, k)| *k).collect();
    let travel_count = by_kind.iter().filter(|k| **k == EventKind::Travel).count();
    let survey_count = by_kind.iter().filter(|k| **k == EventKind::Survey).count();
    let other_count = by_kind.iter().filter(|k| **k == EventKind::Other).count();
    assert_eq!(travel_count, 1, "1 Travel leaf (Own-FTL via projection)");
    assert_eq!(
        survey_count, 1,
        "1 Survey leaf (Foreign-Scout via snapshot)"
    );
    assert_eq!(
        other_count, 1,
        "1 Other leaf (Own-Docked via projection InSystem)"
    );

    let badge = tab.badge(app.world()).expect("badge");
    assert_eq!(badge.count, 3);
}
