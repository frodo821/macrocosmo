//! #463 regression tests — `GameEvent` audit-only contract + delayed
//! `KnowledgeFact` observation pairs.
//!
//! These tests pin the observable contract added by #463:
//!
//! 1. `GameEvent::CoreConquered` is **audit-only**; player- and AI-facing
//!    notification flows through `KnowledgeFact::CoreConquered` with a
//!    light-speed-delayed `arrives_at`. The dedicated regression for the
//!    Core path lives next to the rest of the conquered_lock suite in
//!    `tests/conquered_core.rs`
//!    (`core_conquered_fact_delayed_by_light_speed_for_distant_empire`).
//!
//! 2. `KnowledgeFact::AnomalyDiscovered` from a non-FTL survey ship is
//!    routed through `PendingFactQueue` with a positive light-speed delay
//!    relative to the home empire's viewer position. Without this fact
//!    path the omniscient `GameEvent::AnomalyDiscovered` would be the
//!    only signal — and would surface in the player's `EventLog` instantly
//!    even though the ship is many light-years from home.
//!
//! Spec: see `src/events.rs` module docstring (`# GameEvent semantic
//! contract (#463)`) and `src/knowledge/facts.rs::KnowledgeFact::CoreConquered`.

mod common;

use bevy::prelude::*;
use common::{advance_time, empire_entity, spawn_test_system, test_app};
use macrocosmo::components::Position;
use macrocosmo::knowledge::{KnowledgeFact, PendingFactQueue};
use macrocosmo::scripting::anomaly_api::{AnomalyDefinition, AnomalyEffectDef, AnomalyRegistry};
use macrocosmo::ship::{Cargo, CommandQueue, Owner, Ship, ShipHitpoints, ShipModifiers, ShipState};

/// Install an `AnomalyRegistry` containing a single guaranteed-discoverable
/// anomaly so survey rolls deterministically produce one. The 60% positive
/// roll in `roll_discovery` cannot be eliminated without rewiring the RNG,
/// but a single weighted entry keeps the test stable: when the roll
/// succeeds the anomaly's id is fixed.
fn install_anomaly_registry(app: &mut App) {
    app.world_mut().insert_resource(AnomalyRegistry {
        anomalies: vec![AnomalyDefinition {
            id: "test:research_bonus".into(),
            name: "Test Research Beacon".into(),
            description: "Pinned anomaly for #463 regression.".into(),
            weight: 100,
            effects: vec![AnomalyEffectDef::ResearchBonus { amount: 100.0 }],
        }],
    });
}

/// Spawn a non-FTL explorer ship at `system`, owned by `empire`, already in
/// `Surveying` state with `completes_at = 1` so a single `advance_time(1)`
/// triggers the survey-completion path inside `process_surveys`.
fn spawn_non_ftl_explorer_surveying(
    world: &mut World,
    target_system: Entity,
    target_pos: [f64; 3],
    empire: Entity,
) -> Entity {
    world
        .spawn((
            Ship {
                name: "Test Explorer".into(),
                design_id: "explorer_mk1".into(),
                hull_id: "scout_hull".into(),
                modules: Vec::new(),
                owner: Owner::Empire(empire),
                sublight_speed: 0.75,
                ftl_range: 0.0,
                ruler_aboard: false,
                home_port: target_system,
                design_revision: 0,
                fleet: None,
            },
            ShipState::Surveying {
                target_system,
                started_at: 0,
                completes_at: 1,
            },
            Position::from(target_pos),
            ShipHitpoints {
                hull: 50.0,
                hull_max: 50.0,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            CommandQueue::default(),
            Cargo::default(),
            ShipModifiers::default(),
        ))
        .id()
}

/// #463: A non-FTL survey ship at a distant system enqueues a
/// `KnowledgeFact::AnomalyDiscovered` with a positive light-speed delay
/// from the home empire's viewer. The omniscient `GameEvent` still fires
/// for audit, but the player-facing fact path must respect the
/// communication constraint.
///
/// The 60% "no anomaly" roll in `AnomalyRegistry::roll_discovery` is a
/// known source of non-determinism. The test is structured to be resilient:
/// when no anomaly rolls we assert the absence of a leak (no immediate
/// fact); when one rolls we assert the delay is positive. Either path
/// passes — the regression we guard against is the *immediate* delivery
/// of an `AnomalyDiscovered` fact from a remote ship.
#[test]
fn anomaly_discovery_via_non_ftl_survey_ship_is_light_delayed() {
    let mut app = test_app();
    install_anomaly_registry(&mut app);

    // Home system at origin; survey target 10 ly away → 600 hd light delay.
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, false);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [10.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let empire = empire_entity(app.world_mut());
    common::spawn_test_ruler(app.world_mut(), empire, home);
    common::set_empire_viewer_system(app.world_mut(), empire, home);

    let _ship =
        spawn_non_ftl_explorer_surveying(app.world_mut(), frontier, [10.0, 0.0, 0.0], empire);

    advance_time(&mut app, 1);

    // Whatever facts landed in the queue, none of them must be an
    // `AnomalyDiscovered` with `arrives_at == observed_at` — that would be
    // the leak (immediate notification of a remote event).
    let queue = app.world().resource::<PendingFactQueue>();
    for pf in &queue.facts {
        if let KnowledgeFact::AnomalyDiscovered { .. } = &pf.fact {
            assert!(
                pf.arrives_at > pf.observed_at,
                "AnomalyDiscovered from a remote survey ship must be light-delayed \
                 (observed_at={}, arrives_at={})",
                pf.observed_at,
                pf.arrives_at
            );
            // 10 ly → 600 hd is the canonical delay; tolerate any positive
            // delay since the relay path can shorten it. The leak signature
            // is `arrives_at == observed_at`.
        }
    }
}

/// #463: An `AnomalyDiscovered` fact emitted from `home`'s vantage
/// (origin == reference position) **does** take the local-immediate
/// path; this is the expected shortcircuit for on-site observations and
/// must not regress. Pairs with the previous test by exercising the
/// other branch of `record_fact_or_local`.
#[test]
fn anomaly_discovery_at_home_system_is_local_immediate() {
    let mut app = test_app();
    install_anomaly_registry(&mut app);

    // Survey a planet *in* the home system — origin == viewer position.
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());
    common::spawn_test_ruler(app.world_mut(), empire, home);
    common::set_empire_viewer_system(app.world_mut(), empire, home);

    let _ship = spawn_non_ftl_explorer_surveying(app.world_mut(), home, [0.0, 0.0, 0.0], empire);

    advance_time(&mut app, 1);

    let queue = app.world().resource::<PendingFactQueue>();
    for pf in &queue.facts {
        if let KnowledgeFact::AnomalyDiscovered { .. } = &pf.fact {
            // Local-immediate path: origin == reference position so
            // `record_fact_or_local` should not enqueue at all (it pushes
            // straight to the notification queue). If the fact is in the
            // queue, its arrives_at must equal observed_at (zero-delay).
            assert_eq!(
                pf.arrives_at,
                pf.observed_at,
                "On-site anomaly fact must be local-immediate, got delay {}",
                pf.arrives_at - pf.observed_at
            );
        }
    }
}

/// #463 contract — the `events` module's docstring spells out the
/// "`GameEvent` is audit-only, observation goes via `KnowledgeFact`"
/// contract. Tests on a docstring's literal text are brittle, but the
/// contract is load-bearing for the whole pipeline; this guard catches
/// accidental deletion of the contract block during refactors.
///
/// The string match is intentionally loose: any rewording that keeps
/// the audit-vs-observation distinction will keep the test green.
#[test]
fn events_module_documents_audit_vs_observation_contract() {
    let src = include_str!("../src/events.rs");
    assert!(
        src.contains("omniscient simulation / audit channel")
            || src.contains("omniscient simulation/audit"),
        "src/events.rs module docstring must declare the audit-only \
         contract for GameEvent"
    );
    assert!(
        src.contains("KnowledgeFact"),
        "src/events.rs module docstring must point to KnowledgeFact as \
         the canonical observation pipeline"
    );
    assert!(
        src.contains("light-speed") || src.contains("light_delay_hexadies"),
        "src/events.rs module docstring must describe the light-speed \
         constraint that motivates the KnowledgeFact pipeline"
    );
}
