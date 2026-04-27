//! #448 PR2b: [`AiPolicyMode`] gate smoke test.
//!
//! Confirms `Legacy` mode (default) emits commands as today, and
//! `Layered` mode (opt-in) emits zero commands (noop scaffold).
//! Both modes share the same `npc_decision_tick` system path; the
//! gate is a single `match` inside the per-empire loop.
//!
//! PR2c/2d will fill the Layered branch with rule ports while a
//! parity test (`tests/ai_layered_parity.rs`, future) keeps Legacy
//! and Layered in lock-step. Until then, switching to Layered must
//! make the AI fall silent — that's the property this file pins.

mod common;

use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
use macrocosmo::ai::command_outbox::AiCommandOutbox;
use macrocosmo::ai::mid_adapter::AiPolicyMode;
use macrocosmo::components::Position;
use macrocosmo::faction::FactionOwner;
use macrocosmo::galaxy::AtSystem;
use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, SystemKnowledge, SystemSnapshot, SystemVisibilityMap,
    SystemVisibilityTier,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{CoreShip, Owner, Ship};

use common::{advance_time, spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

/// Build a minimal AI-controlled empire with one idle scout and an
/// unsurveyed catalogued frontier — Legacy `SimpleNpcPolicy` Rule 2
/// should fire a `survey_system` here on the first decision tick.
/// The same scenario under Layered mode must produce zero commands.
fn setup_survey_scenario(app: &mut App, frontier_distance_ly: f64) -> (Entity, Entity, Entity) {
    app.insert_resource(AiPlayerMode(true));

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Vesk".into(),
            },
            PlayerEmpire,
            Faction {
                id: "vesk".into(),
                name: "Vesk".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            SystemVisibilityMap::default(),
            KnowledgeStore::default(),
        ))
        .id();

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [frontier_distance_ly, 0.0, 0.0],
        1.0,
        false,
        false,
    );

    spawn_test_ruler(app.world_mut(), empire, home);

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(home, SystemVisibilityTier::Local);
        vis.set(frontier, SystemVisibilityTier::Catalogued);
    }

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update(SystemKnowledge {
            system: home,
            observed_at: 0,
            received_at: 0,
            data: SystemSnapshot {
                name: "Home".into(),
                position: [0.0, 0.0, 0.0],
                surveyed: true,
                colonized: true,
                ..Default::default()
            },
            source: ObservationSource::Direct,
        });
    }

    // One idle scout at home — Rule 2 needs an idle survey-capable
    // ship before it will emit `survey_system`.
    let scout = spawn_test_ship(
        app.world_mut(),
        "Scout-1",
        "explorer_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(scout)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    // Place a Core at home so per-empire bookkeeping has somewhere
    // to root, mirroring the production setup. Not strictly needed
    // for the survey rule but keeps the test scenario realistic.
    let _core = place_core_at(app.world_mut(), empire, home, [0.0, 0.0, 0.0]);

    (empire, home, frontier)
}

/// Mirror of the helper in `tests/ai_npc_outbox_dedup.rs` — spawn a
/// `CoreShip` for `empire` at `system`. Kept inline to avoid
/// cross-test coupling.
fn place_core_at(world: &mut World, empire: Entity, system: Entity, position: [f64; 3]) -> Entity {
    let pos = Position::from(position);
    world
        .spawn((
            Ship {
                name: "Core".to_string(),
                design_id: "infrastructure_core_v1".to_string(),
                hull_id: "infrastructure_core_hull".to_string(),
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

fn outbox_command_count(app: &App) -> usize {
    app.world().resource::<AiCommandOutbox>().entries.len()
}

/// Default mode is `Legacy` — `SimpleNpcPolicy` Rule 2 fires a
/// `survey_system` at the catalogued frontier, dropping at least
/// one entry into the light-speed outbox.
#[test]
fn legacy_mode_emits_commands_today() {
    let mut app = test_app();
    // No explicit `AiPolicyMode` insert — `Default` (= `Legacy`) is
    // exactly what the production game ships with.
    let _ = setup_survey_scenario(&mut app, 5.0);

    // Confirm the resource is present and at its default before
    // exercising any tick — guards against an accidental future
    // refactor that silently drops the `init_resource` call.
    assert_eq!(
        *app.world().resource::<AiPolicyMode>(),
        AiPolicyMode::Legacy,
        "AiPolicyMode default must be Legacy",
    );

    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    assert!(
        outbox_command_count(&app) >= 1,
        "Legacy mode must emit at least one command in the survey \
         scenario; got {} — gate may have flipped or default \
         changed",
        outbox_command_count(&app),
    );
}

/// Layered mode is the PR2b noop — same scenario, zero commands.
/// The point of this test is the **gate**, not the rules: when
/// PR2c starts porting Rule 1 (attack), this test will fail and
/// the porting agent will move it under a `parity` harness instead.
#[test]
fn layered_mode_emits_commands_after_rule_ports() {
    // PR2b shipped Layered = noop; PR2c+2d+3a have since ported
    // Rules 1/2/3/5a/6/7/8 to `MidStanceAgent`. The flag-flip
    // gate is still valid — this test now confirms Layered also
    // emits on the survey scenario (Rule 2 fires post-PR3a). The
    // file is slated for deletion in #448 PR3d once the flag
    // itself goes away.
    let mut app = test_app();
    let _ = setup_survey_scenario(&mut app, 5.0);

    app.insert_resource(AiPolicyMode::Layered);

    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    assert!(
        outbox_command_count(&app) > 0,
        "Layered mode should now emit commands via MidStanceAgent's \
         Rule 2 port. Got 0 — likely a regression in the survey \
         port or adapter wiring."
    );
}
