//! #448 PR2c parity test for Layered vs Legacy AI policy.
//!
//! Asserts that on a one-tick fixture, [`AiPolicyMode::Legacy`] and
//! [`AiPolicyMode::Layered`] emit the **same** canonical Command set.
//! Today (PR2c) only Rules 1 and 5a are ported; PR2d adds the rest.
//! Each fixture below is built so **only** Rules 1 and/or 5a fire in
//! Legacy mode — any other rule firing would diverge from Layered's
//! still-empty branch and break parity.
//!
//! Canonical comparison goes through [`CanonicalCommand`] which sorts
//! the param map before serialising — `CommandParams` is an
//! `AHashMap` whose iteration order varies, so a naive `Vec<Command>`
//! comparison was flaky under the issuer-supplied parameter ordering.
//! The `BTreeSet` wrapper additionally absorbs same-tick re-emission
//! (irrelevant for parity) and order-of-emission changes between
//! rules (also irrelevant).

mod common;

use std::collections::BTreeSet;

use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
use macrocosmo::ai::AiTickSet;
use macrocosmo::ai::mid_adapter::AiPolicyMode;
use macrocosmo::ai::plugin::AiBusResource;
use macrocosmo::colony::BuildingId;
use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, SystemKnowledge, SystemSnapshot, SystemVisibilityMap,
    SystemVisibilityTier,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{Owner, Ship};

use common::{
    advance_time, spawn_mock_core_ship, spawn_test_colony, spawn_test_ruler, spawn_test_ship,
    spawn_test_system, test_app,
};

/// Canonical projection of [`macrocosmo_ai::Command`] for parity
/// comparison. `kind` and `issuer` round-trip directly; the param
/// map is serialized after sorting by key so equality is order-free.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CanonicalCommand {
    kind: String,
    issuer: u32,
    /// Sorted `(key, Debug-of-value)` pairs serialised as a single
    /// string. `Debug` on `CommandValue` is stable enough for parity
    /// — every variant prints its inner payload deterministically.
    params_canonical: String,
}

impl CanonicalCommand {
    fn from(cmd: &macrocosmo_ai::Command) -> Self {
        let mut sorted: Vec<(String, String)> = cmd
            .params
            .iter()
            .map(|(k, v)| (k.to_string(), format!("{:?}", v)))
            .collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            kind: cmd.kind.as_str().to_string(),
            issuer: cmd.issuer.0,
            params_canonical: format!("{:?}", sorted),
        }
    }
}

/// Cumulative capture of every command the bus saw in pending state
/// between `npc_decision_tick` (which emits) and
/// `dispatch_ai_pending_commands` (which moves them to the outbox).
/// Inserted at test setup time and read at the end of the run. This
/// is the only stable observation point for parity comparison: the
/// outbox empties as commands mature, and `drain_commands` removes
/// them from the bus, so any later inspection sees a moving target.
#[derive(Resource, Default)]
struct CapturedCommands(Vec<macrocosmo_ai::Command>);

fn snapshot_pending_commands(bus: Res<AiBusResource>, mut captured: ResMut<CapturedCommands>) {
    for cmd in bus.0.pending_commands() {
        captured.0.push(cmd.clone());
    }
}

/// Run `fixture` in a fresh `App` under `mode`, advance `ticks`
/// hexadies, and return the canonical set of every command the
/// policy emitted (across all ticks). Wires a probe system between
/// `npc_decision_tick` and `dispatch_ai_pending_commands` so the
/// observation sees every command exactly once before it leaves the
/// bus.
fn run_for_ticks<F>(mode: AiPolicyMode, ticks: u32, fixture: F) -> BTreeSet<CanonicalCommand>
where
    F: FnOnce(&mut App),
{
    let mut app = test_app();
    app.insert_resource(mode);
    app.init_resource::<CapturedCommands>();
    // Probe after npc_decision_tick (and the orchestrator) but before
    // dispatch — the dispatch step drains `bus.pending_commands` into
    // the outbox so this is the last frame-stable read site.
    app.add_systems(
        Update,
        snapshot_pending_commands
            .after(macrocosmo::ai::npc_decision::npc_decision_tick)
            .before(macrocosmo::ai::command_outbox::dispatch_ai_pending_commands)
            .in_set(AiTickSet::Reason),
    );
    fixture(&mut app);
    for _ in 0..ticks {
        advance_time(&mut app, 1);
    }
    let captured = app.world().resource::<CapturedCommands>();
    captured.0.iter().map(CanonicalCommand::from).collect()
}

/// Mark an empire as an `AiControlled` PlayerEmpire and spawn home +
/// hostile systems. Returns `(empire, home, hostile_target)`. The
/// hostile target is recorded in the empire's KnowledgeStore as
/// `surveyed=true, has_hostile=true` so Rule 1 picks it up.
fn setup_rule_1_scenario(app: &mut App) -> (Entity, Entity, Entity) {
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
    // Close target — distance only matters for command-outbox light
    // delay, and we want the `attack_target` entry to arrive in the
    // outbox during the test window regardless of mode.
    let target = spawn_test_system(
        app.world_mut(),
        "Hostile-072",
        [0.5, 0.0, 0.0],
        1.0,
        true,
        false,
    );

    spawn_test_ruler(app.world_mut(), empire, home);

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(home, SystemVisibilityTier::Local);
        vis.set(target, SystemVisibilityTier::Surveyed);
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
                surveyed: true,
                colonized: false,
                has_hostile: true,
                ..Default::default()
            },
            source: ObservationSource::Direct,
        });
    }

    // Idle combat ship at home. We spawn an explorer (so the helper
    // succeeds — `patrol_corvette` isn't in the test registry) then
    // mutate its design id to one the registry doesn't know. That
    // makes `can_survey == can_colonize == false` in
    // `npc_decision_tick`'s `ShipInfo` builder, so `is_combat = true`
    // (sublight/ftl from the explorer hull keep it non-immobile).
    // Same shape that legacy + layered both observe.
    let combat = spawn_test_ship(
        app.world_mut(),
        "Corvette-1",
        "explorer_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    {
        let mut s = app.world_mut().entity_mut(combat);
        let mut ship = s.get_mut::<Ship>().unwrap();
        ship.owner = Owner::Empire(empire);
        // Unknown design id → registry returns None → Rule 1 sees a
        // combat-classified ship.
        ship.design_id = "patrol_corvette_test".to_string();
    }

    (empire, home, target)
}

/// Spawn an empire with one colony + a deployed Core at home, no
/// shipyard, no idle ships. Drives Rule 5a in Legacy mode, identity
/// in Layered.
fn setup_rule_5a_scenario(app: &mut App) -> (Entity, Entity) {
    use macrocosmo::amount::Amt;

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

    // Colony with all four planet slots filled with mines. This
    // forces `free_building_slots == 0` so Rule 5b stays silent —
    // only Rule 5a (system-level shipyard) is left to fire. The
    // helper auto-attaches `FactionOwner(empire)` so the colony
    // counts toward `colony_count`.
    let _colony = spawn_test_colony(
        app.world_mut(),
        home,
        Amt::units(1000),
        Amt::units(1000),
        vec![
            Some(BuildingId("mine".to_string())),
            Some(BuildingId("mine".to_string())),
            Some(BuildingId("mine".to_string())),
            Some(BuildingId("mine".to_string())),
        ],
    );

    // Core at home so `systems_with_core >= 1.0` — required by
    // Rule 5a's #370 gate. Mirrors the working fixture in
    // `tests/ai_player_e2e.rs::ai_builds_shipyard_when_can_build_zero`.
    spawn_mock_core_ship(app.world_mut(), home, empire);

    // `AiCommandOutbox` resolves the issuer's home capital via
    // `HomeSystem` (or `is_capital`); the test helper makes systems
    // with `is_capital = false`, so we must tag the empire
    // explicitly. Without this, the `build_structure` command
    // (spatial-less, addresses the capital) would be dropped at
    // dispatch time with a warn!.
    app.world_mut()
        .entity_mut(empire)
        .insert(macrocosmo::galaxy::HomeSystem(home));
    spawn_test_ruler(app.world_mut(), empire, home);

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(home, SystemVisibilityTier::Local);
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

    (empire, home)
}

#[test]
fn rule_1_attack_hostile_parity() {
    // 3 ticks: target is 0.5 ly away, so the `attack_target` outbox
    // entry matures within one decision cycle.
    let legacy = run_for_ticks(AiPolicyMode::Legacy, 3, |app| {
        setup_rule_1_scenario(app);
    });
    let layered = run_for_ticks(AiPolicyMode::Layered, 3, |app| {
        setup_rule_1_scenario(app);
    });

    // Sanity: Legacy must produce *something* in this fixture.
    // Without this guard a future regression that silences both
    // modes would still pass via vacuous equality.
    assert!(
        legacy.iter().any(|c| c.kind == "attack_target"),
        "Rule 1 fixture broken: Legacy must emit attack_target",
    );

    assert_eq!(
        legacy, layered,
        "Rule 1 (attack_target + move_ruler) parity broken: \
         Legacy = {:?}, Layered = {:?}",
        legacy, layered,
    );
}

#[test]
fn rule_5a_build_shipyard_parity() {
    // 15 ticks mirrors `ai_player_e2e::ai_builds_shipyard_when_can_build_zero`:
    // `build_structure` is spatial-less so origin == destination == capital
    // (zero light delay), but the schedule still needs one full Update for
    // dispatch and one for process to release the entry through the outbox.
    let legacy = run_for_ticks(AiPolicyMode::Legacy, 15, |app| {
        setup_rule_5a_scenario(app);
    });
    let layered = run_for_ticks(AiPolicyMode::Layered, 15, |app| {
        setup_rule_5a_scenario(app);
    });

    // Sanity: legacy must emit a `build_structure` shipyard. If this
    // fixture stops triggering Rule 5a (e.g. emitter changes,
    // metrics renamed) the parity assertion below would still match
    // a vacuous empty-vs-empty case — so guard explicitly.
    assert!(
        legacy.iter().any(|c| c.kind == "build_structure"),
        "Rule 5a fixture broken: Legacy must emit build_structure(shipyard); \
         got {:?}",
        legacy,
    );

    assert_eq!(
        legacy, layered,
        "Rule 5a (build_structure shipyard) parity broken: \
         Legacy = {:?}, Layered = {:?}",
        legacy, layered,
    );
}
