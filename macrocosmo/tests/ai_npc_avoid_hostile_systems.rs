//! Regression: Bug B (2026-04-27 handoff). NPC AI must not redispatch
//! survey or colonize commands at systems already known-hostile in its
//! own `KnowledgeStore` (`SystemSnapshot::has_hostile = true`). Without
//! this filter, an empire that lost a scout to a hostile system would
//! immediately redispatch the next available scout / colonizer to the
//! same death trap, looping forever.
//!
//! See `docs/session-handoff-2026-04-27-ai-decomposition.md` Bug B and
//! `ai/npc_decision.rs::npc_decision_tick` (the `colonizable_systems`
//! and `candidates` builders).

mod common;

use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
use macrocosmo::ai::assignments::{AssignmentKind, AssignmentTarget, PendingAssignment};
use macrocosmo::ai::command_outbox::AiCommandOutbox;
use macrocosmo::ai::schema::ids::command as cmd_ids;
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

/// Spawn an AI empire whose KnowledgeStore already records `target` as
/// known-hostile (e.g. a previous scout died there and the destruction
/// fact propagated back). `home` is known surveyed + colonized so
/// `npc_decision_tick` doesn't accidentally treat it as a candidate.
fn setup_hostile_known_scenario(app: &mut App, target_surveyed: bool) -> (Entity, Entity, Entity) {
    app.insert_resource(AiPlayerMode(true));

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Aurelian".into(),
            },
            PlayerEmpire,
            Faction {
                id: "aurelian".into(),
                name: "Aurelian".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            SystemVisibilityMap::default(),
            KnowledgeStore::default(),
        ))
        .id();

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    // Place the hostile target close enough that route planning is
    // trivial — we don't actually want any ship to *reach* it; the test
    // is solely about command emission.
    let target = spawn_test_system(
        app.world_mut(),
        "Hostile-072",
        [0.5, 0.0, 0.0],
        1.0,
        target_surveyed,
        false,
    );

    spawn_test_ruler(app.world_mut(), empire, home);

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(home, SystemVisibilityTier::Local);
        // Catalogued is enough for both Rule 2 (survey) and Rule 3
        // (colonize) to consider this system. The hostile filter — not
        // visibility — is what should keep it out.
        vis.set(
            target,
            if target_surveyed {
                SystemVisibilityTier::Surveyed
            } else {
                SystemVisibilityTier::Catalogued
            },
        );
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
        store.update(SystemKnowledge {
            system: target,
            observed_at: 0,
            received_at: 0,
            data: SystemSnapshot {
                name: "Hostile-072".into(),
                position: [0.5, 0.0, 0.0],
                surveyed: target_surveyed,
                colonized: false,
                has_hostile: true,
                ..Default::default()
            },
            source: ObservationSource::Direct,
        });
    }

    (empire, home, target)
}

fn outbox_has_command_for(
    app: &App,
    kind: macrocosmo_ai::CommandKindId,
    target_system: Entity,
) -> bool {
    let outbox = app.world().resource::<AiCommandOutbox>();
    outbox.entries.iter().any(|entry| {
        let cmd = &entry.command;
        if cmd.kind != kind {
            return false;
        }
        match cmd.params.get("target_system") {
            Some(macrocosmo_ai::CommandValue::System(sys_id)) => {
                target_system.to_bits() == sys_id.0
            }
            _ => false,
        }
    })
}

fn pending_survey_targets(app: &mut App, empire: Entity) -> Vec<Entity> {
    let mut q = app.world_mut().query::<&PendingAssignment>();
    q.iter(app.world())
        .filter(|pa| pa.faction == empire && pa.kind == AssignmentKind::Survey)
        .filter_map(|pa| match pa.target {
            AssignmentTarget::System(e) => Some(e),
        })
        .collect()
}

/// Place a `CoreShip` at `system` for `empire` so the colonize-Core gate
/// (`#299` / `npc_decision_tick::core_systems_per_empire`) is satisfied.
/// Without this, Rule 3 would skip the candidate for an unrelated reason
/// and the test would pass even if the hostile filter was missing.
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

/// Test 1: a system known-hostile-but-unsurveyed must not draw a
/// `survey_system` command nor a `PendingAssignment::Survey` marker.
/// The pre-fix code path runs the candidate filter without checking
/// `has_hostile`, so an Aurelian-style "lost a scout, send another"
/// loop would re-emit a survey order tick after tick.
#[test]
fn ai_does_not_dispatch_survey_to_known_hostile_system() {
    let mut app = test_app();
    let (empire, home, target) = setup_hostile_known_scenario(&mut app, false);

    // Park a scout at home so the AI has the means to dispatch one.
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

    // Drive a handful of decision ticks. The bug manifests on the very
    // first tick where Rule 2 runs, but a few ticks bracket
    // light-speed / dispatch ordering without lingering long enough for
    // anything else to interfere.
    for _ in 0..5 {
        advance_time(&mut app, 1);
    }

    assert!(
        !outbox_has_command_for(&app, cmd_ids::survey_system(), target),
        "AI emitted survey_system for a system flagged has_hostile=true \
         in its own KnowledgeStore — Bug B regression (the lost-scout \
         loop)."
    );

    let pending = pending_survey_targets(&mut app, empire);
    assert!(
        !pending.contains(&target),
        "PendingAssignment::Survey points at a known-hostile system: \
         {:?} (target {:?}) — Bug B regression.",
        pending,
        target,
    );
}

/// Test 2: a known-hostile system that is otherwise a valid colonization
/// candidate (surveyed, uncolonized, own Core present) must be excluded
/// by the hostile filter. Without the fix, Rule 3 would happily ferry
/// settlers into the meat grinder.
#[test]
fn ai_does_not_dispatch_colonize_to_known_hostile_system() {
    let mut app = test_app();
    // `target_surveyed = true` so without the hostile filter Rule 3
    // would otherwise consider this a perfectly good candidate.
    let (empire, home, target) = setup_hostile_known_scenario(&mut app, true);

    // Satisfy the Core-presence gate so the test isolates the hostile
    // filter and not the (unrelated) Core requirement.
    place_core_at(app.world_mut(), empire, target, [0.5, 0.0, 0.0]);

    // Park an idle colonizer at home so Rule 3 has a ship to dispatch.
    // Without this, the absence of an emitted command is meaningless —
    // Rule 3 short-circuits when `idle_colonizers.is_empty()`.
    let colonizer = spawn_test_ship(
        app.world_mut(),
        "Colonizer-1",
        "colony_ship_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(colonizer)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    for _ in 0..5 {
        advance_time(&mut app, 1);
    }

    assert!(
        !outbox_has_command_for(&app, cmd_ids::colonize_system(), target),
        "AI emitted colonize_system for a system flagged has_hostile=true \
         in its own KnowledgeStore — Bug B regression (settlers into the \
         meat grinder)."
    );
}
