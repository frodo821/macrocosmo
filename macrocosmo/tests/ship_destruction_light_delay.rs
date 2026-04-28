//! #435 / #472: Regression tests — ship destruction perception path.
//!
//! Before #435 the `ShipDestroyed` `GameEvent` and the `CombatDefeat` "All
//! ships destroyed by X" event were written immediately at the destruction
//! tick, regardless of how far the destruction site was from the player.
//! #435 deferred the event emission to per-empire light-arrival time.
//!
//! #472 split the contract again to match the #463 `CoreConquered` template:
//!
//! * `GameEvent::ShipDestroyed` is the **omniscient audit-only** record —
//!   one immediate fire at the destruction site (no light-speed gating).
//! * `KnowledgeFact::ShipDestroyed` carries the per-empire delayed
//!   observation; it is routed via `FactSysParam::record_for(...)` so each
//!   empire's `PendingFactQueue` arrival respects light-speed (or the relay
//!   shortcut) from the destruction site to that empire's viewer.
//! * The `KnowledgeStore` snapshot transition stays per-empire delayed —
//!   the ghost flips to `Destroyed` only once light has arrived for that
//!   specific empire.
//!
//! These tests pin the post-#472 behaviour:
//!
//! * The `KnowledgeStore` snapshot must still respect light-speed delay
//!   (no early ghost-to-Destroyed transition for distant empires).
//! * `CombatDefeat` (different code path) still respects light-speed delay
//!   via `DelayedCombatEventQueue`.
//! * For a destruction at the SAME system as the player, both the audit
//!   `GameEvent` and the on-site snapshot transition fire on the same tick.

mod common;

use bevy::prelude::*;
use macrocosmo::components::Position;
use macrocosmo::events::{EventLog, GameEventKind};
use macrocosmo::galaxy::StarSystem;
use macrocosmo::knowledge::{KnowledgeStore, ShipSnapshotState};
use macrocosmo::physics::light_delay_hexadies;
use macrocosmo::player::*;
use macrocosmo::ship::*;

use common::{
    advance_time, empire_entity, set_empire_viewer_system, spawn_test_system,
    test_app_with_event_log,
};

/// Spawn a capital star system. Helper separate from `spawn_test_system`
/// because combat tests need `is_capital=true` for the PlayerRespawn lookup
/// to succeed inside `resolve_combat`.
fn spawn_capital(world: &mut World) -> Entity {
    let sys = world
        .spawn((
            StarSystem {
                name: "Capital".into(),
                surveyed: true,
                is_capital: true,
                star_type: "default".into(),
            },
            Position::from([0.0, 0.0, 0.0]),
            macrocosmo::galaxy::Sovereignty::default(),
            macrocosmo::technology::TechKnowledge::default(),
            macrocosmo::galaxy::SystemModifiers::default(),
        ))
        .id();
    world.spawn((
        macrocosmo::galaxy::Planet {
            name: "Capital I".into(),
            system: sys,
            planet_type: "default".into(),
        },
        macrocosmo::galaxy::SystemAttributes {
            habitability: 0.7,
            mineral_richness: 0.5,
            energy_potential: 0.5,
            research_potential: 0.5,
            max_building_slots: 4,
        },
        Position::from([0.0, 0.0, 0.0]),
    ));
    sys
}

/// Spawn a doomed ship: hull is 0.01 so the first combat tick reduces it to
/// zero. Returns the ship entity.
fn spawn_doomed_ship(world: &mut World, name: &str, system: Entity, pos: [f64; 3]) -> Entity {
    world
        .spawn((
            Ship {
                name: name.into(),
                design_id: "explorer_mk1".into(),
                hull_id: "corvette".into(),
                modules: Vec::new(),
                owner: Owner::Neutral,
                sublight_speed: 0.75,
                ftl_range: 10.0,
                ruler_aboard: false,
                home_port: system,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system },
            Position::from(pos),
            ShipHitpoints {
                hull: 0.01,
                hull_max: 50.0,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            ShipModifiers::default(),
            CommandQueue::default(),
            Cargo::default(),
        ))
        .id()
}

#[test]
fn remote_ship_destruction_snapshot_respects_light_delay() {
    let mut app = test_app_with_event_log();

    let capital = spawn_capital(app.world_mut());

    // Place the combat site 10 LY away. Light-delay hexadies ≈ 600.
    let remote_pos = [10.0, 0.0, 0.0];
    let remote = spawn_test_system(app.world_mut(), "Doom-Zone", remote_pos, 0.7, true, false);

    // Put the player and empire viewer at the capital (origin).
    app.world_mut()
        .spawn((Player, StationedAt { system: capital }));
    let empire = empire_entity(app.world_mut());
    set_empire_viewer_system(app.world_mut(), empire, capital);

    // Doomed ship + strong hostile at the remote system — first tick destroys.
    let ship_entity = spawn_doomed_ship(app.world_mut(), "Far-Away-1", remote, remote_pos);
    let _ = common::spawn_raw_hostile(
        app.world_mut(),
        remote,
        1000.0,
        1000.0,
        100.0,
        0.0,
        "space_creature",
    );

    // Tick 1 hexadies — combat resolves and the ship is despawned.
    advance_time(&mut app, 1);
    app.update(); // drain Messages into EventLog

    // Sanity: the live entity is gone (destruction happened).
    assert!(
        app.world().get_entity(ship_entity).is_err(),
        "Ship should be despawned immediately at destruction tick"
    );

    // #472: `GameEvent::ShipDestroyed` is now an omniscient audit record and
    // fires immediately at the destruction site (no light-speed gating).
    // The per-empire delayed perception flows through `KnowledgeFact` +
    // `KnowledgeStore` instead — verified below.
    {
        let log = app.world().resource::<EventLog>();
        let ship_destroyed = log
            .entries
            .iter()
            .find(|e| e.kind == GameEventKind::ShipDestroyed);
        assert!(
            ship_destroyed.is_some(),
            "#472: GameEvent::ShipDestroyed is an immediate audit fire at the \
             destruction site (omniscient channel). EventLog: {:?}",
            log.entries
                .iter()
                .map(|e| &e.description)
                .collect::<Vec<_>>()
        );
    }

    // Light-speed delay still governs the per-empire `KnowledgeStore`
    // snapshot transition — the ghost must NOT be flipped to Destroyed yet.
    {
        let empire = empire_entity(app.world_mut());
        let store = app.world().get::<KnowledgeStore>(empire).unwrap();
        let snap = store.get_ship(ship_entity);
        if let Some(snap) = snap {
            assert_ne!(
                snap.last_known_state,
                ShipSnapshotState::Destroyed,
                "Ghost must NOT be in Destroyed state before light arrives"
            );
        }
    }

    // Advance past the light delay. 10 LY = 600 hexadies.
    let delay = light_delay_hexadies(10.0);
    assert_eq!(delay, 600, "sanity: 10 LY is 600 hexadies of light delay");
    advance_time(&mut app, delay);
    app.update();

    // The KnowledgeStore snapshot must now have transitioned to Destroyed
    // for this empire.
    {
        let empire = empire_entity(app.world_mut());
        let store = app.world().get::<KnowledgeStore>(empire).unwrap();
        let snap = store
            .get_ship(ship_entity)
            .expect("Snapshot must exist after destruction is observed");
        assert_eq!(
            snap.last_known_state,
            ShipSnapshotState::Destroyed,
            "Snapshot must transition to Destroyed once light arrives"
        );
    }
}

#[test]
fn remote_combat_defeat_event_respects_light_delay() {
    let mut app = test_app_with_event_log();

    let capital = spawn_capital(app.world_mut());

    // 10 LY away → delay = 600 hexadies.
    let remote_pos = [10.0, 0.0, 0.0];
    let remote = spawn_test_system(app.world_mut(), "Doom-Zone-2", remote_pos, 0.7, true, false);

    app.world_mut()
        .spawn((Player, StationedAt { system: capital }));
    let empire = empire_entity(app.world_mut());
    set_empire_viewer_system(app.world_mut(), empire, capital);

    // Spawn exactly one ship at the remote system; once it dies, all-ships
    // destroyed → CombatDefeat path fires.
    let _ = spawn_doomed_ship(app.world_mut(), "Lone-Ship", remote, remote_pos);
    let _ = common::spawn_raw_hostile(
        app.world_mut(),
        remote,
        1000.0,
        1000.0,
        100.0,
        0.0,
        "space_creature",
    );

    advance_time(&mut app, 1);
    app.update();

    // Immediately after destruction, no CombatDefeat in the event log yet.
    {
        let log = app.world().resource::<EventLog>();
        let combat_defeat_count = log
            .entries
            .iter()
            .filter(|e| e.kind == GameEventKind::CombatDefeat)
            .count();
        assert_eq!(
            combat_defeat_count,
            0,
            "CombatDefeat event must NOT fire at the destruction tick for \
             a remote combat. EventLog: {:?}",
            log.entries
                .iter()
                .map(|e| &e.description)
                .collect::<Vec<_>>()
        );
    }

    // Advance past the light delay.
    advance_time(&mut app, light_delay_hexadies(10.0));
    app.update();

    // Now CombatDefeat should have fired.
    {
        let log = app.world().resource::<EventLog>();
        let combat_defeat = log
            .entries
            .iter()
            .find(|e| e.kind == GameEventKind::CombatDefeat);
        assert!(
            combat_defeat.is_some(),
            "CombatDefeat event must fire after light-speed delay. \
             EventLog: {:?}",
            log.entries
                .iter()
                .map(|e| &e.description)
                .collect::<Vec<_>>()
        );
        assert!(
            combat_defeat
                .unwrap()
                .description
                .contains("All ships destroyed"),
            "CombatDefeat description should match the canonical text"
        );
    }
}

#[test]
fn local_ship_destruction_event_fires_immediately() {
    // When the player is at the same system as the combat site, the light
    // delay is zero and the event should fire the moment the ship is
    // destroyed. This guard catches over-aggressive gating that would
    // suppress events even for local combat.
    let mut app = test_app_with_event_log();

    let capital = spawn_capital(app.world_mut());

    app.world_mut()
        .spawn((Player, StationedAt { system: capital }));
    let empire = empire_entity(app.world_mut());
    set_empire_viewer_system(app.world_mut(), empire, capital);

    let ship_entity = spawn_doomed_ship(app.world_mut(), "Home-Guard", capital, [0.0, 0.0, 0.0]);
    let _ = common::spawn_raw_hostile(
        app.world_mut(),
        capital,
        1000.0,
        1000.0,
        100.0,
        0.0,
        "space_creature",
    );

    advance_time(&mut app, 1);
    app.update();

    // Entity despawned.
    assert!(
        app.world().get_entity(ship_entity).is_err(),
        "Ship should be despawned at destruction tick"
    );

    // Event fires immediately — light delay is 0 since player == destruction site.
    {
        let log = app.world().resource::<EventLog>();
        let ship_destroyed = log
            .entries
            .iter()
            .find(|e| e.kind == GameEventKind::ShipDestroyed);
        assert!(
            ship_destroyed.is_some(),
            "ShipDestroyed must fire immediately for local combat (delay=0). \
             EventLog: {:?}",
            log.entries
                .iter()
                .map(|e| &e.description)
                .collect::<Vec<_>>()
        );
    }
}
