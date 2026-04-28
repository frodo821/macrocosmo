//! Sentinel for #449 PR2d: the cutover that moves Rules 2 (survey)
//! and 5b (slot fill) from `MidStanceAgent` to `ShortStanceAgent`.
//!
//! Existence of this file is load-bearing: pre-PR2d these emits came
//! from the Mid layer (Round 11 PR3a / PR3b shape). Post-PR2d they
//! come from per-Fleet / per-ColonizedSystem `ShortAgent`s. The wire
//! shape (Command kind / params / issuer) **must** stay identical so
//! every downstream test that asserts against the AI bus (and every
//! handler that drains it) keeps observing the same behaviour. Each
//! test below pins one half of the cutover against a minimal world
//! and walks the world enough ticks for the AI tick cadence + the
//! light-speed outbox to release the command for inspection.

mod common;

use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
use macrocosmo::ai::command_outbox::AiCommandOutbox;
use macrocosmo::ai::schema::ids::command as cmd_ids;
use macrocosmo::amount::Amt;
use macrocosmo::faction::FactionOwner;
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

/// Find the first command in the outbox whose `kind` matches.
fn find_outbox(app: &App, kind: macrocosmo_ai::CommandKindId) -> Option<macrocosmo_ai::Command> {
    let outbox = app.world().resource::<AiCommandOutbox>();
    outbox
        .entries
        .iter()
        .find(|entry| entry.command.kind == kind)
        .map(|entry| entry.command.clone())
}

// ---- Rule 2 (survey) — Fleet-scope ShortAgent ---------------------------

/// Short cutover sentinel: with one idle surveyor + one unsurveyed
/// frontier target, the AI must emit exactly one `survey_system`
/// command, with the same kind / params / issuer the deleted
/// Mid-side Rule 2 produced. The path is now per-fleet ShortAgent;
/// the wire shape is byte-for-byte identical.
#[test]
fn short_emits_survey_system_after_cutover() {
    let mut app = test_app();
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
    // Far enough that the outbox keeps the entry around long enough
    // for us to inspect, while still being reachable in a single
    // sublight leg via the `plan_ftl_route` fallback.
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [5.0, 0.0, 0.0],
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

    // Walk a few ticks: `npc_decision_tick` populates the per-empire
    // scratch, `run_short_agents` reads it, the dispatcher parks the
    // command in the outbox.
    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    let cmd = find_outbox(&app, cmd_ids::survey_system()).expect("Short must emit survey_system");
    assert_eq!(
        cmd.kind.as_str(),
        "survey_system",
        "wire kind must match the deleted Mid Rule 2"
    );
    // issuer is FactionId derived from the empire entity.
    assert_eq!(
        cmd.issuer,
        macrocosmo_ai::FactionId(empire.index().index()),
        "issuer must remain the empire FactionId",
    );
    match cmd.params.get("target_system") {
        Some(macrocosmo_ai::CommandValue::System(sys)) => {
            assert_eq!(sys.0, frontier.to_bits(), "target_system param");
        }
        other => panic!("expected target_system = SystemRef; got {:?}", other),
    }
    match cmd.params.get("ship_count") {
        Some(macrocosmo_ai::CommandValue::I64(n)) => assert_eq!(*n, 1, "ship_count"),
        other => panic!("expected ship_count=1; got {:?}", other),
    }
    match cmd.params.get("ship_0") {
        Some(macrocosmo_ai::CommandValue::Entity(eref)) => {
            assert_eq!(eref.0, scout.to_bits(), "ship_0 = scout");
        }
        other => panic!("expected ship_0 entity; got {:?}", other),
    }
}

// ---- Rule 5b (slot fill) — ColonizedSystem-scope ShortAgent --------------

/// Short cutover sentinel: one empire with one colonized system and
/// `free_building_slots > 0` must emit exactly the same
/// `build_structure(<id>)` command the deleted Mid-side Rule 5b
/// produced. The path is now per-colony ShortAgent; the wire shape
/// stays identical.
///
/// We exercise the `mine` branch (energy / food production both
/// non-negative). The other branches (`power_plant` for
/// `net_production_energy < 0`, `farm` for negative food) are unit-
/// tested on stub adapters in `ai::short_stance::tests` — production
/// metric flow runs through unsigned `Amt`s and cannot produce a
/// negative value, so an integration sentinel for those branches
/// would have to fight the metric emitter that re-publishes the
/// non-negative rate every tick.
#[test]
fn short_emits_build_structure_mine_after_cutover() {
    let mut app = test_app();
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
    // Mark home as the empire's HomeSystem so the outbox can resolve
    // a faction-wide command's origin position to the capital. Same
    // pattern `tests/ai_player_e2e.rs::ai_builds_shipyard_*` uses.
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

    // Spawn a colony with at least one free building slot. The empty
    // slot vector ensures `free_building_slots > 0`.
    let colony = spawn_test_colony(
        app.world_mut(),
        home,
        Amt::units(1_000),
        Amt::units(1_000),
        vec![None, None, None, None],
    );
    // Re-tag the colony with this empire's ownership: the
    // `spawn_test_colony` helper picks the first `With<Empire>`
    // entity in the world (which `test_app()` auto-creates), so the
    // default `FactionOwner` would point to the wrong empire in a
    // multi-empire setup.
    app.world_mut()
        .entity_mut(colony)
        .insert(FactionOwner(empire));
    // Plant a Core ship at home so `update_sovereignty` flips the
    // system's `Sovereignty.owner = Some(Empire(empire))`. The
    // `handle_build_structure` consumer rejects `mine` orders for
    // systems whose Sovereignty doesn't already cover the issuer
    // empire — without the Core, the planet-side
    // `BuildingQueue` stays empty even though the AI emitted the
    // command correctly.
    spawn_mock_core_ship(app.world_mut(), home, empire);

    // Direct-inject `free_building_slots > 0` on every tick so the
    // empire emitter (which won't see a non-zero `max_building_slots`
    // from `spawn_test_colony`'s default Buildings) doesn't immediately
    // re-publish 0 over our injected value. `net_production_energy`
    // and `net_production_food` stay non-negative, so the rule's
    // three-branch chain falls through to `mine`.
    let fid = macrocosmo_ai::FactionId(empire.index().index());
    for _ in 0..15 {
        {
            let now = app
                .world()
                .resource::<macrocosmo::time_system::GameClock>()
                .elapsed;
            let mut bus = app
                .world_mut()
                .resource_mut::<macrocosmo::ai::AiBusResource>();
            bus.0.emit(
                &macrocosmo::ai::schema::ids::metric::for_faction("free_building_slots", fid),
                4.0,
                now,
            );
        }
        advance_time(&mut app, 1);
    }

    // The build_structure command's destination resolves to the
    // empire's capital (no `target_system` param), and the test sets
    // origin == destination via `HomeSystem(home)` + ruler at home,
    // so light delay collapses to 0 and the command matures within
    // the same tick it was dispatched. `dispatch_ai_pending_commands`
    // pushes it onto the outbox; `process_ai_pending_commands` and
    // `drain_ai_commands` consume it the same tick. So we observe
    // the wire shape via the colony's `BuildingQueue` (the planet-
    // building handler's drop-off — `mine` is a planet building, not
    // a system building) instead of the transient outbox entry.
    let queue = app
        .world()
        .get::<macrocosmo::colony::BuildingQueue>(colony)
        .expect("colony must carry BuildingQueue");
    let queued: Vec<&str> = queue
        .queue
        .iter()
        .map(|order| order.building_id.as_str())
        .collect();
    assert!(
        queued.iter().any(|id| *id == "mine"),
        "Short (ColonizedSystem) Rule 5b mine branch must enqueue mine; \
         queued ids: {:?}",
        queued,
    );
    // Sanity: every command on the bus journey carried the right
    // FactionId. Check the outbox pre-process (it may be empty if
    // every command matured this tick) — if the entry is there, the
    // issuer must match the empire.
    let outbox = app.world().resource::<AiCommandOutbox>();
    for entry in &outbox.entries {
        if entry.command.kind == cmd_ids::build_structure() {
            assert_eq!(
                entry.command.issuer,
                macrocosmo_ai::FactionId(empire.index().index()),
                "outbox build_structure entry must carry empire FactionId"
            );
        }
    }
}
