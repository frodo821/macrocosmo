//! Hotfix-3 regression tests — AI resource starvation + Rule 6
//! fleet-composition semantics.
//!
//! Two bugs were observed on a long-run brp QA session:
//!
//! 1. **Infinite `explorer_mk1` build loop** — Rule 6's first branch
//!    fires when `comp.survey_count == 0 && has_unsurveyed_targets`.
//!    Pre-hotfix `survey_count` was derived from
//!    `NpcContext.ships`, whose builder filters on
//!    `info.system.is_some()`. The moment an explorer transitioned
//!    to `ShipState::Surveying` its `system` collapsed to `None`,
//!    the count went back to 0, and Rule 6 re-emitted `build_ship
//!    explorer_mk1` every Reason tick. Each cycle paid the full
//!    explorer maintenance + build cost, eventually bankrupting the
//!    starter empire.
//!
//! 2. **AI re-queues identical building order every tick** — the brp
//!    QA report observed a single colony with **165 stacked `mine`
//!    orders**. Rule 5b emits the same `build_structure` proposal
//!    every Reason tick once production is short; the per-tick
//!    dedup that absorbs system-building re-emissions
//!    (`handle_build_structure` system branch, line ~736) did not
//!    have a planet-building counterpart, so every emit found a
//!    free slot and pushed another order.
//!
//! Both bugs share a deeper root cause — **no resource gate** on
//! the build rules. Even with the dedup fixed, an empire with
//! stockpile == 0 would still emit, queue, and starve. The hotfix
//! adds a soft `current_stockpile >= cost` gate to every build
//! emission (Rules 5a / 5b / 6 / 3.5), permitting deficit spending
//! (revenue < expense) but refusing to stack orders an empty
//! stockpile cannot fund.
//!
//! This integration test pins the consumer-side planet-building
//! dedup behaviour via the production `drain_ai_commands`
//! pipeline. Pure-logic gate tests (no Bevy app required) live as
//! `#[test]`s alongside `MidStanceAgent::decide` in
//! `src/ai/mid_stance.rs::tests` — they cover Rule 6 fleet-census
//! semantics, Rule 5a stockpile gate, and the deficit-spending
//! escape hatch.

mod common;

use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
use macrocosmo::ai::plugin::AiBusResource;
use macrocosmo::ai::schema::ids::command as cmd_ids;
use macrocosmo::amount::Amt;
use macrocosmo::colony::building_queue::BuildingQueue;
use macrocosmo::faction::FactionOwner;
use macrocosmo::knowledge::{KnowledgeStore, SystemVisibilityMap};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo_ai::{Command, CommandValue};

use common::{
    advance_time, spawn_mock_core_ship, spawn_test_colony, spawn_test_ruler, spawn_test_system,
    test_app,
};

/// Mirror of `to_ai_faction(empire)` (private to the ai module). We
/// rebuild the conversion here so the test can craft a `Command`
/// whose `issuer.0 == empire.index()` without pulling in private
/// internals.
fn faction_id_for(empire: Entity) -> macrocosmo_ai::FactionId {
    macrocosmo_ai::FactionId(empire.index().index())
}

/// Spawn a minimal NPC empire with one colonised, sovereign system
/// and a non-zero stockpile. Returns `(empire, system)`.
///
/// **Why `AiPlayerMode(true)`**: lets the empire be `PlayerEmpire`
/// while still routing through the AI command pipeline. The test
/// directly emits commands onto the bus rather than relying on the
/// Mid rule logic, so the player flag is purely an authorisation
/// switch.
fn spawn_empire_with_colony(app: &mut App) -> (Entity, Entity) {
    app.insert_resource(AiPlayerMode(true));

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Stockpile Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "stockpile_test".into(),
                name: "Stockpile Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            KnowledgeStore::default(),
            SystemVisibilityMap::default(),
        ))
        .id();

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    app.world_mut()
        .entity_mut(empire)
        .insert(macrocosmo::galaxy::HomeSystem(home));
    spawn_test_ruler(app.world_mut(), empire, home);

    // Colony with 4 free slots and a fat stockpile (so the test can
    // observe the dedup independent of the stockpile gate).
    let colony = spawn_test_colony(
        app.world_mut(),
        home,
        Amt::units(10_000),
        Amt::units(10_000),
        vec![None, None, None, None],
    );
    // Re-stamp colony owner (helper picks the first empire it finds,
    // which can be a different auto-spawned test empire).
    app.world_mut()
        .entity_mut(colony)
        .insert(FactionOwner(empire));

    // Sovereignty is set by `update_sovereignty` from CoreShip → AtSystem
    // → FactionOwner. Spawning a mock Core ship at home routes the
    // ownership through the production path; without it the per-tick
    // `update_sovereignty` resets the manual stamp back to None and
    // `handle_build_structure`'s owned-systems gate drops the order.
    spawn_mock_core_ship(app.world_mut(), home, empire);

    (empire, home)
}

/// Emit a `build_structure(building_id = "mine")` directly onto the
/// bus, addressed to `empire`. Returns the issued command for log /
/// assertion.
fn emit_build_mine(world: &mut World, empire: Entity, now: i64) -> Command {
    let cmd = Command::new(cmd_ids::build_structure(), faction_id_for(empire), now)
        .with_param("building_id", CommandValue::Str("mine".into()));
    world
        .resource_mut::<AiBusResource>()
        .0
        .emit_command(cmd.clone());
    cmd
}

/// Hotfix-3: planet-building dedup at the colony level. Emitting the
/// same `build_structure(mine)` twice within one tick must result in
/// **one** outstanding `BuildingQueue` order, not two. Pre-hotfix the
/// dedup branch only fired for system buildings (shipyard / port /
/// lab); planet buildings (mine / farm / power_plant) stacked
/// unbounded, leading the brp QA report's 165× mine stack.
#[test]
fn planet_building_dedup_at_colony_collapses_duplicate_emits() {
    let mut app = test_app();
    let (empire, _home) = spawn_empire_with_colony(&mut app);

    // First tick triggers Startup (registers AI schema kinds, builds
    // resources). Without this the direct `emit_command` call below
    // would be dropped with an "undeclared command kind" warning
    // because `schema::declare_all` runs in Startup.
    advance_time(&mut app, 1);

    // Two identical `mine` commands in the same tick.
    emit_build_mine(app.world_mut(), empire, 1);
    emit_build_mine(app.world_mut(), empire, 1);

    // Drive the AI bus dispatch + handler chain. Origin == destination
    // (Ruler at home, build_structure resolves to capital == home) so
    // light-speed delay is zero — five Update cycles is enough to
    // clear `emit → outbox → drain → handler`.
    for _ in 0..5 {
        advance_time(&mut app, 1);
    }

    let mut found_queues = Vec::new();
    {
        let mut q = app.world_mut().query::<(&BuildingQueue, &FactionOwner)>();
        for (queue, owner) in q.iter(app.world()) {
            if owner.0 != empire {
                continue;
            }
            let mines = queue
                .queue
                .iter()
                .filter(|o| o.building_id.as_str() == "mine")
                .count();
            found_queues.push(mines);
        }
    }
    let total_mines: usize = found_queues.iter().sum();
    assert_eq!(
        total_mines, 1,
        "Hotfix-3 planet dedup: two identical `mine` emits must collapse to one queue order, got per-colony counts {:?}",
        found_queues,
    );
}

/// Hotfix-3: dedup still kicks in across multiple ticks. The
/// per-tick stockpile gate suppresses fresh emits when the order is
/// in-flight, but if the AI logic side fails to gate (e.g. legacy
/// code path, future regression), the consumer-side dedup is the
/// last line of defense. Pin three back-to-back emits in three
/// separate ticks → still one queue order.
#[test]
fn planet_building_dedup_survives_multi_tick_re_emission() {
    let mut app = test_app();
    let (empire, _home) = spawn_empire_with_colony(&mut app);

    // Startup tick first so AI schema is declared (see comment in
    // `planet_building_dedup_at_colony_collapses_duplicate_emits`).
    advance_time(&mut app, 1);

    // Tick 1 emit → drive dispatch → tick 2 emit → … → tick 3 emit.
    emit_build_mine(app.world_mut(), empire, 1);
    advance_time(&mut app, 1);
    emit_build_mine(app.world_mut(), empire, 2);
    advance_time(&mut app, 1);
    emit_build_mine(app.world_mut(), empire, 3);
    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    let mut total_mines = 0usize;
    {
        let mut q = app.world_mut().query::<(&BuildingQueue, &FactionOwner)>();
        for (queue, owner) in q.iter(app.world()) {
            if owner.0 != empire {
                continue;
            }
            total_mines += queue
                .queue
                .iter()
                .filter(|o| o.building_id.as_str() == "mine")
                .count();
        }
    }
    assert_eq!(
        total_mines, 1,
        "Hotfix-3: three sequential `mine` emits across separate ticks must still result in 1 queue order",
    );
}

/// Hotfix-3 sanity: dedup is keyed on `building_id`, NOT on order
/// arrival ordering. A different building (farm) emitted after a
/// mine must be allowed through — the per-tick stockpile gate is
/// the only thing that throttles cross-building emits, and we
/// model an empire with enough stockpile to fund both.
#[test]
fn planet_building_dedup_does_not_block_different_building_id() {
    let mut app = test_app();
    let (empire, _home) = spawn_empire_with_colony(&mut app);

    // Startup tick first so AI schema is declared.
    advance_time(&mut app, 1);

    emit_build_mine(app.world_mut(), empire, 1);
    let farm_cmd = Command::new(cmd_ids::build_structure(), faction_id_for(empire), 1)
        .with_param("building_id", CommandValue::Str("farm".into()));
    app.world_mut()
        .resource_mut::<AiBusResource>()
        .0
        .emit_command(farm_cmd);

    for _ in 0..5 {
        advance_time(&mut app, 1);
    }

    let (mut mines, mut farms) = (0usize, 0usize);
    {
        let mut q = app.world_mut().query::<(&BuildingQueue, &FactionOwner)>();
        for (queue, owner) in q.iter(app.world()) {
            if owner.0 != empire {
                continue;
            }
            for o in &queue.queue {
                match o.building_id.as_str() {
                    "mine" => mines += 1,
                    "farm" => farms += 1,
                    _ => {}
                }
            }
        }
    }
    assert_eq!(mines, 1, "one mine queued");
    assert_eq!(farms, 1, "farm not blocked by mine's dedup entry");
}
