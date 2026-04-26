//! Regression: Bug A (2026-04-27 handoff). NPC AI must not double-emit
//! `survey_system` / `colonize_system` commands at the same target while
//! a previous emission is still sitting in the light-speed
//! [`AiCommandOutbox`] (handler hasn't run yet, so no
//! `PendingAssignment` marker exists).
//!
//! Background: Vesk Scout-1 and Scout-2 were observed dispatching to the
//! same system 163 hex apart — `npc_decision_tick` (every 2 ticks) only
//! deduped against `Query<&PendingAssignment>`, which is populated when
//! `drain_ai_commands` runs *after* the light-speed window elapses. A
//! 30+-hex courier delay therefore left a wide window where the policy
//! happily re-fired onto the same target every other tick.
//!
//! Fix: `npc_decision_tick` now also unions in the `survey_system` /
//! `colonize_system` `target_system` params from `AiCommandOutbox.entries`
//! before computing candidate sets. See `npc_decision.rs` (the
//! `outbox_survey_per_empire` / `outbox_colonize_per_empire` precompute
//! and the `pending_survey_targets` extension).
//!
//! These tests pin both directions:
//! - **survey**: with one unsurveyed target and two idle scouts, the
//!   second decision tick during the light-speed window must NOT add a
//!   second `survey_system` to the outbox.
//! - **colonize**: same shape but for colony ships against a colonizable
//!   target with an owned Core in place.

mod common;

use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
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

fn count_outbox_for(app: &App, kind: macrocosmo_ai::CommandKindId, target_system: Entity) -> usize {
    let outbox = app.world().resource::<AiCommandOutbox>();
    outbox
        .entries
        .iter()
        .filter(|entry| {
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
        .count()
}

/// Spawn an AI-controlled empire at `home` (capital, surveyed in its
/// own KnowledgeStore) plus a far-away `frontier` system at `[d, 0, 0]`
/// known only as `Catalogued`.
///
/// `frontier_distance_ly` should be large enough that the resulting
/// light-delay through `AiCommandOutbox` is many ticks — the test wants
/// the outbox entry to *persist* across at least two
/// `npc_decision_tick` runs so it can assert the second tick does not
/// re-fire.
fn setup_far_target(
    app: &mut App,
    name: &str,
    frontier_distance_ly: f64,
    target_surveyed: bool,
) -> (Entity, Entity, Entity) {
    app.insert_resource(AiPlayerMode(true));

    let empire = app
        .world_mut()
        .spawn((
            Empire { name: name.into() },
            PlayerEmpire,
            Faction {
                id: name.to_lowercase(),
                name: name.into(),
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
        target_surveyed,
        false,
    );

    spawn_test_ruler(app.world_mut(), empire, home);

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(home, SystemVisibilityTier::Local);
        vis.set(
            frontier,
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
        if target_surveyed {
            // Required for Rule 3 to consider the frontier as a
            // colonization candidate (`k.data.surveyed && !colonized`).
            store.update(SystemKnowledge {
                system: frontier,
                observed_at: 0,
                received_at: 0,
                data: SystemSnapshot {
                    name: "Frontier".into(),
                    position: [frontier_distance_ly, 0.0, 0.0],
                    surveyed: true,
                    colonized: false,
                    ..Default::default()
                },
                source: ObservationSource::Direct,
            });
        }
    }

    (empire, home, frontier)
}

/// Required by the `colonizable_systems` builder gate (#299): the
/// empire must already have a deployed Core in the target system for
/// Rule 3 to pick it up. Mirrors `place_core_at` in
/// `tests/ai_npc_avoid_hostile_systems.rs`.
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

/// Bug A regression (survey path): with two idle scouts, one
/// frontier target, and a far-away placement (so the light-speed
/// outbox window spans many decision ticks), only ONE
/// `survey_system` command may live in the outbox at any point
/// before the first one arrives.
#[test]
fn outbox_dedups_survey_system_during_light_delay_window() {
    // 5 ly frontier → light delay is many tens of hexadies, more than
    // enough for `mid_cadence=2` `npc_decision_tick` to fire several
    // times during the window.
    let mut app = test_app();
    let (empire, home, frontier) = setup_far_target(&mut app, "Vesk", 5.0, false);

    // Two idle scouts at home — the bug needs more than one candidate
    // ship to manifest (Rule 2 zips ships×targets and would happily
    // dispatch the second scout if dedup misses the in-flight one).
    for i in 0..2 {
        let s = spawn_test_ship(
            app.world_mut(),
            &format!("Scout-{}", i),
            "explorer_mk1",
            home,
            [0.0, 0.0, 0.0],
        );
        app.world_mut()
            .entity_mut(s)
            .get_mut::<Ship>()
            .unwrap()
            .owner = Owner::Empire(empire);
    }

    // First batch: enough ticks for `npc_decision_tick` to fire once
    // (it gates on `clock.elapsed > last_tick`) and the dispatcher to
    // park the command in the outbox. Stay well under the 5-ly light
    // delay so the entry hasn't matured yet.
    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    let after_first = count_outbox_for(&app, cmd_ids::survey_system(), frontier);
    assert_eq!(
        after_first, 1,
        "expected exactly 1 in-flight survey command after first decision \
         tick; got {}",
        after_first,
    );

    // Second batch: drive several more decision ticks while the
    // outbox entry is still in flight. The pre-fix code would let the
    // second scout fire onto the same target — the assertion checks
    // the outbox count stays at 1.
    for _ in 0..6 {
        advance_time(&mut app, 1);
    }

    let after_second = count_outbox_for(&app, cmd_ids::survey_system(), frontier);
    assert_eq!(
        after_second, 1,
        "expected outbox to still hold exactly 1 survey command for the \
         frontier (the light-speed window has not elapsed); got {} — \
         Bug A regression",
        after_second,
    );
}

/// Bug A regression (colonize path): same shape as the survey case
/// but for `colonize_system`. Two colony ships, one colonizable
/// target with an owned Core in place — only one `colonize_system`
/// command may live in the outbox during the light-speed window.
#[test]
fn outbox_dedups_colonize_system_during_light_delay_window() {
    let mut app = test_app();
    // `target_surveyed = true` so Rule 3 considers the frontier a
    // valid colonization candidate.
    let (empire, home, frontier) = setup_far_target(&mut app, "Aurelian", 5.0, true);

    // Satisfy the Core-presence gate (#299).
    place_core_at(app.world_mut(), empire, frontier, [5.0, 0.0, 0.0]);

    // Two idle colony ships at home.
    for i in 0..2 {
        let s = spawn_test_ship(
            app.world_mut(),
            &format!("Colonizer-{}", i),
            "colony_ship_mk1",
            home,
            [0.0, 0.0, 0.0],
        );
        app.world_mut()
            .entity_mut(s)
            .get_mut::<Ship>()
            .unwrap()
            .owner = Owner::Empire(empire);
    }

    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    let after_first = count_outbox_for(&app, cmd_ids::colonize_system(), frontier);
    assert_eq!(
        after_first, 1,
        "expected exactly 1 in-flight colonize command after first decision \
         tick; got {}",
        after_first,
    );

    for _ in 0..6 {
        advance_time(&mut app, 1);
    }

    let after_second = count_outbox_for(&app, cmd_ids::colonize_system(), frontier);
    assert_eq!(
        after_second, 1,
        "expected outbox to still hold exactly 1 colonize command for the \
         frontier (the light-speed window has not elapsed); got {} — \
         Bug A regression",
        after_second,
    );
}
