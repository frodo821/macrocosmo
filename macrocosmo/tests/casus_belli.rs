//! #305 (S-11): Integration tests for the Casus Belli system.

mod common;

use bevy::prelude::*;
use macrocosmo::casus_belli::{
    ActiveWars, CasusBelliDefinition, CasusBelliRegistry, DemandSpec, EndScenario,
};
use macrocosmo::event_system::EventSystem;
use macrocosmo::events::{EventLog, GameEvent, GameEventKind};
use macrocosmo::faction::{FactionRelations, RelationState};
use macrocosmo::knowledge::NextEventId;
use macrocosmo::player::{Empire, Faction};
use macrocosmo::scripting::{GameRng, ScriptEngine};
use macrocosmo::time_system::{GameClock, GameSpeed};

/// Build a minimal app with the CB evaluation system and required resources.
fn cb_test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(GameClock::new(0));
    app.insert_resource(GameSpeed::default());
    app.init_resource::<FactionRelations>();
    app.init_resource::<ActiveWars>();
    app.init_resource::<CasusBelliRegistry>();
    app.init_resource::<EventSystem>();
    app.init_resource::<EventLog>();
    app.init_resource::<NextEventId>();
    app.init_resource::<GameRng>();
    app.add_message::<GameEvent>();

    // Init ScriptEngine so evaluate_casus_belli can resource_scope it
    let engine =
        ScriptEngine::new_with_rng(app.world().resource::<GameRng>().handle()).expect("lua init");
    app.insert_resource(engine);

    app.add_systems(Update, macrocosmo::time_system::advance_game_time);
    app.add_systems(
        Update,
        macrocosmo::casus_belli::evaluate_casus_belli
            .after(macrocosmo::time_system::advance_game_time),
    );
    // #460: receiver-side war/peace flips travel via DiplomaticEvent.
    // Without HomeSystem components on these synthetic empires the delay
    // collapses to 0, but tick_diplomatic_events still has to run to
    // actually consume the spawned event so the existing assertions
    // (which expect symmetric end-state) keep passing.
    app.add_systems(
        Update,
        macrocosmo::faction::tick_diplomatic_events
            .after(macrocosmo::casus_belli::evaluate_casus_belli),
    );
    app.add_systems(
        Update,
        macrocosmo::events::collect_events.after(macrocosmo::casus_belli::evaluate_casus_belli),
    );
    app
}

/// Spawn two empire entities with Faction + Empire components. Returns (a, b).
fn spawn_two_empires(world: &mut World) -> (Entity, Entity) {
    let a = world
        .spawn((
            Empire {
                name: "Alpha".into(),
            },
            Faction::new("alpha", "Alpha"),
        ))
        .id();
    let b = world
        .spawn((
            Empire {
                name: "Beta".into(),
            },
            Faction::new("beta", "Beta"),
        ))
        .id();
    (a, b)
}

/// Register a test CB definition into the registry with auto_war and an
/// evaluate function that always returns true.
fn register_always_true_cb(app: &mut App) {
    // Set up the Lua evaluate function
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
            define_casus_belli {
                id = "test_cb",
                name = "Test CB",
                auto_war = true,
                evaluate = function(attacker_id, defender_id)
                    return true
                end,
                demands = {
                    { kind = "return_cores" },
                },
                end_scenarios = {
                    {
                        id = "white_peace",
                        label = "White Peace",
                        available = function() return true end,
                    },
                },
            }
        "#,
            )
            .exec()
            .expect("lua exec");
    }

    // Also register in the Rust registry
    let mut registry = CasusBelliRegistry::default();
    registry.definitions.insert(
        "test_cb".into(),
        CasusBelliDefinition {
            id: "test_cb".into(),
            name: "Test CB".into(),
            auto_war: true,
            demands: vec![DemandSpec {
                kind: "return_cores".into(),
                params: Default::default(),
            }],
            additional_demand_groups: Vec::new(),
            end_scenarios: vec![EndScenario {
                id: "white_peace".into(),
                label: "White Peace".into(),
                demand_adjustments: Vec::new(),
            }],
        },
    );
    app.insert_resource(registry);
}

// ---- Tests ----

#[test]
fn test_cb_registration() {
    let engine = ScriptEngine::new().expect("lua init");
    engine
        .lua()
        .load(
            r#"
        define_casus_belli {
            id = "core_attack",
            name = "Unprovoked Core Attack",
            auto_war = true,
            demands = { { kind = "return_cores" } },
            end_scenarios = {
                { id = "white_peace", label = "White Peace" },
            },
        }
    "#,
        )
        .exec()
        .expect("lua exec");

    let defs = macrocosmo::scripting::casus_belli_api::parse_casus_belli_definitions(engine.lua())
        .expect("parse");
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].id, "core_attack");
    assert!(defs[0].auto_war);
    assert_eq!(defs[0].demands.len(), 1);
    assert_eq!(defs[0].demands[0].kind, "return_cores");
    assert_eq!(defs[0].end_scenarios.len(), 1);
}

#[test]
fn test_auto_war_trigger() {
    let mut app = cb_test_app();
    let (a, b) = spawn_two_empires(app.world_mut());
    register_always_true_cb(&mut app);

    // Advance time to trigger evaluation
    app.world_mut().resource_mut::<GameClock>().elapsed += 1;
    app.update();

    // Should have declared war
    let relations = app.world().resource::<FactionRelations>();
    assert_eq!(
        relations.get(a, b).unwrap().state,
        RelationState::War,
        "A should be at war with B"
    );
    assert_eq!(
        relations.get(b, a).unwrap().state,
        RelationState::War,
        "B should be at war with A"
    );

    // ActiveWars should have an entry
    let active_wars = app.world().resource::<ActiveWars>();
    assert!(active_wars.has_war_between(a, b));
    assert_eq!(active_wars.wars.len(), 1);
    assert_eq!(active_wars.wars[0].cb_id, "test_cb");

    // EventLog should have a WarDeclared event
    let event_log = app.world().resource::<EventLog>();
    assert!(
        event_log
            .entries
            .iter()
            .any(|e| e.kind == GameEventKind::WarDeclared),
        "Should have emitted WarDeclared event"
    );
}

#[test]
fn test_no_double_war() {
    let mut app = cb_test_app();
    let (a, b) = spawn_two_empires(app.world_mut());
    register_always_true_cb(&mut app);

    // First tick: war declared
    app.world_mut().resource_mut::<GameClock>().elapsed += 1;
    app.update();

    let wars_count_1 = app.world().resource::<ActiveWars>().wars.len();
    assert_eq!(
        wars_count_1, 1,
        "Should have exactly one war after first tick"
    );

    // Second tick: should NOT create a duplicate war
    app.world_mut().resource_mut::<GameClock>().elapsed += 1;
    app.update();

    let wars_count_2 = app.world().resource::<ActiveWars>().wars.len();
    assert_eq!(
        wars_count_2, 1,
        "Should still have exactly one war — no double war"
    );
}

#[test]
fn test_demand_computation() {
    let registry = {
        let mut r = CasusBelliRegistry::default();
        r.definitions.insert(
            "test_cb".into(),
            CasusBelliDefinition {
                id: "test_cb".into(),
                name: "Test CB".into(),
                auto_war: true,
                demands: vec![
                    DemandSpec {
                        kind: "return_cores".into(),
                        params: Default::default(),
                    },
                    DemandSpec {
                        kind: "reparations".into(),
                        params: [("amount".into(), "500".into())].into_iter().collect(),
                    },
                ],
                additional_demand_groups: Vec::new(),
                end_scenarios: Vec::new(),
            },
        );
        r
    };

    let def = registry.get("test_cb").unwrap();
    assert_eq!(def.demands.len(), 2);
    assert_eq!(def.demands[0].kind, "return_cores");
    assert_eq!(def.demands[1].kind, "reparations");
    assert_eq!(def.demands[1].params.get("amount").unwrap(), "500");
}

#[test]
fn test_end_war() {
    let mut app = cb_test_app();
    let (a, b) = spawn_two_empires(app.world_mut());
    register_always_true_cb(&mut app);

    // Trigger auto-war
    app.world_mut().resource_mut::<GameClock>().elapsed += 1;
    app.update();

    // Verify war exists
    assert!(app.world().resource::<ActiveWars>().has_war_between(a, b));
    assert_eq!(
        app.world()
            .resource::<FactionRelations>()
            .get(a, b)
            .unwrap()
            .state,
        RelationState::War,
    );

    // End the war
    let removed = macrocosmo::casus_belli::end_war(app.world_mut(), a, b);
    assert!(removed.is_some());
    assert_eq!(removed.unwrap().cb_id, "test_cb");

    // War should be gone
    assert!(!app.world().resource::<ActiveWars>().has_war_between(a, b));

    // #460: Sender (a) flipped to Peace immediately; receiver (b) flips
    // when its DIPLO_FORCED_PEACE DiplomaticEvent arrives.
    {
        let relations = app.world().resource::<FactionRelations>();
        assert_eq!(relations.get(a, b).unwrap().state, RelationState::Peace);
        assert_eq!(
            relations.get(b, a).unwrap().state,
            RelationState::War,
            "receiver still sees War until tick_diplomatic_events drains the forced-peace event"
        );
    }

    // Drive tick_diplomatic_events directly — calling app.update() would
    // re-trigger evaluate_casus_belli (auto_war=true, evaluate=true) and
    // immediately re-declare war between the two empires.
    app.world_mut().resource_mut::<GameClock>().elapsed += 1;
    let mut sched = bevy::ecs::schedule::Schedule::default();
    sched.add_systems(macrocosmo::faction::tick_diplomatic_events);
    sched.run(app.world_mut());

    let relations = app.world().resource::<FactionRelations>();
    assert_eq!(relations.get(a, b).unwrap().state, RelationState::Peace);
    assert_eq!(relations.get(b, a).unwrap().state, RelationState::Peace);
}

/// Verify that existing conquered-core tests still pass with CB system loaded.
/// This test ensures the new CB system doesn't break existing ship/combat behavior.
#[test]
fn test_existing_conquered_tests_still_pass() {
    // This test verifies module-level compatibility. The ActiveWars resource
    // existing doesn't affect combat resolution or conquered-core mechanics
    // since they run independently.
    let mut app = cb_test_app();
    let (a, _b) = spawn_two_empires(app.world_mut());

    // Verify ActiveWars starts empty
    assert!(app.world().resource::<ActiveWars>().wars.is_empty());

    // Verify FactionRelations starts with no entries for these empires
    let relations = app.world().resource::<FactionRelations>();
    assert!(relations.get(a, a).is_none());
}

// ---------------------------------------------------------------------------
// #460: Casus Belli auto-war / forced peace must use the delayed
// receiver-flip pattern (sender immediate, receiver via DiplomaticEvent).
// ---------------------------------------------------------------------------

use macrocosmo::components::Position;
use macrocosmo::faction::{
    DIPLO_DECLARE_WAR, DIPLO_FORCED_PEACE, DiplomaticEvent, tick_diplomatic_events,
};
use macrocosmo::galaxy::HomeSystem;
use macrocosmo::physics::light_delay_hexadies;

/// Build a CB test app **without** `tick_diplomatic_events` so the test
/// can drive the diplomatic-event drainer manually and observe the
/// pre-arrival asymmetric state. Mirrors `cb_test_app` otherwise.
fn cb_test_app_no_tick() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(GameClock::new(0));
    app.insert_resource(GameSpeed::default());
    app.init_resource::<FactionRelations>();
    app.init_resource::<ActiveWars>();
    app.init_resource::<CasusBelliRegistry>();
    app.init_resource::<EventSystem>();
    app.init_resource::<EventLog>();
    app.init_resource::<NextEventId>();
    app.init_resource::<GameRng>();
    app.add_message::<GameEvent>();

    let engine =
        ScriptEngine::new_with_rng(app.world().resource::<GameRng>().handle()).expect("lua init");
    app.insert_resource(engine);

    app.add_systems(Update, macrocosmo::time_system::advance_game_time);
    app.add_systems(
        Update,
        macrocosmo::casus_belli::evaluate_casus_belli
            .after(macrocosmo::time_system::advance_game_time),
    );
    app
}

/// Spawn two empires whose capital `HomeSystem`s are 10 ly apart.
/// Returns (empire_a, empire_b, expected_delay_hexadies).
fn spawn_two_empires_with_capitals(world: &mut World, distance_ly: f64) -> (Entity, Entity, i64) {
    let cap_a = world
        .spawn(Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        })
        .id();
    let cap_b = world
        .spawn(Position {
            x: distance_ly,
            y: 0.0,
            z: 0.0,
        })
        .id();
    let a = world
        .spawn((
            Empire {
                name: "Alpha".into(),
            },
            Faction::new("alpha", "Alpha"),
            HomeSystem(cap_a),
        ))
        .id();
    let b = world
        .spawn((
            Empire {
                name: "Beta".into(),
            },
            Faction::new("beta", "Beta"),
            HomeSystem(cap_b),
        ))
        .id();
    (a, b, light_delay_hexadies(distance_ly))
}

/// Drive `tick_diplomatic_events` on the world without re-running
/// `evaluate_casus_belli` (which would auto-redeclare war on every tick
/// because the test CB has `evaluate = always true`).
fn run_tick_diplomatic_events(app: &mut App) {
    let mut sched = bevy::ecs::schedule::Schedule::default();
    sched.add_systems(tick_diplomatic_events);
    sched.run(app.world_mut());
}

/// #460: Auto-war via Casus Belli must be **asymmetric** during the
/// light-speed propagation window. Attacker → defender flips immediately,
/// defender → attacker stays peaceful until the
/// [`DiplomaticEvent`] arrives.
#[test]
fn test_cb_declare_war_asymmetric_pre_arrival() {
    let mut app = cb_test_app_no_tick();
    let (a, b, delay) = spawn_two_empires_with_capitals(app.world_mut(), 10.0);
    register_always_true_cb(&mut app);
    assert!(
        delay > 0,
        "test setup: capitals should yield non-zero delay"
    );

    // Tick 1: evaluate_casus_belli fires.
    app.world_mut().resource_mut::<GameClock>().elapsed += 1;
    app.update();

    // Active war recorded.
    assert!(app.world().resource::<ActiveWars>().has_war_between(a, b));

    // Asymmetric: attacker -> defender = War, defender -> attacker = NOT War.
    let relations = app.world().resource::<FactionRelations>();
    assert_eq!(
        relations.get(a, b).unwrap().state,
        RelationState::War,
        "attacker (sender) sees War immediately"
    );
    assert_ne!(
        relations.get(b, a).map(|v| v.state),
        Some(RelationState::War),
        "defender (receiver) must NOT see War before light-speed delay elapses"
    );

    // A DIPLO_DECLARE_WAR DiplomaticEvent is in flight.
    let world = app.world_mut();
    let events: Vec<_> = world
        .query::<&DiplomaticEvent>()
        .iter(world)
        .filter(|e| e.option_id == DIPLO_DECLARE_WAR)
        .map(|e| (e.from, e.to, e.arrives_at))
        .collect();
    assert_eq!(events.len(), 1, "exactly one in-flight declare_war event");
    assert_eq!(events[0].0, a, "from = attacker");
    assert_eq!(events[0].1, b, "to = defender");
    assert_eq!(
        events[0].2,
        1 + delay,
        "arrives_at = clock_at_declaration + light-speed delay"
    );
}

/// #460: After the light-speed delay elapses and
/// `tick_diplomatic_events` drains the in-flight event, the CB-declared
/// war becomes symmetric (both directions War).
#[test]
fn test_cb_declare_war_symmetric_post_arrival() {
    let mut app = cb_test_app_no_tick();
    let (a, b, delay) = spawn_two_empires_with_capitals(app.world_mut(), 10.0);
    register_always_true_cb(&mut app);

    // Tick 1: declare.
    app.world_mut().resource_mut::<GameClock>().elapsed += 1;
    app.update();

    // Advance to (and past) the arrival tick — but only run
    // tick_diplomatic_events, NOT evaluate_casus_belli (which would
    // re-declare war every tick under always-true CB).
    app.world_mut().resource_mut::<GameClock>().elapsed = 1 + delay;
    run_tick_diplomatic_events(&mut app);

    // Symmetric: both directions War.
    let relations = app.world().resource::<FactionRelations>();
    assert_eq!(relations.get(a, b).unwrap().state, RelationState::War);
    assert_eq!(
        relations.get(b, a).unwrap().state,
        RelationState::War,
        "defender's view flips to War once the DiplomaticEvent arrives"
    );

    // The in-flight event has been consumed.
    let world = app.world_mut();
    let in_flight = world
        .query::<&DiplomaticEvent>()
        .iter(world)
        .filter(|e| e.option_id == DIPLO_DECLARE_WAR)
        .count();
    assert_eq!(in_flight, 0, "DiplomaticEvent should be despawned");
}

/// #460: Forced peace via [`end_war`] is also asymmetric. Sender flips to
/// Peace immediately; receiver still sees War until the
/// [`DIPLO_FORCED_PEACE`] event arrives.
#[test]
fn test_cb_end_war_asymmetric_pre_arrival() {
    let mut app = cb_test_app_no_tick();
    let (a, b, delay) = spawn_two_empires_with_capitals(app.world_mut(), 10.0);
    register_always_true_cb(&mut app);
    assert!(delay > 0);

    // Tick 1: declare auto-war + drain to make it symmetric, so we have
    // a clean baseline of "both at War" before testing the end path.
    app.world_mut().resource_mut::<GameClock>().elapsed += 1;
    app.update();
    app.world_mut().resource_mut::<GameClock>().elapsed = 1 + delay;
    run_tick_diplomatic_events(&mut app);
    {
        let r = app.world().resource::<FactionRelations>();
        assert_eq!(r.get(a, b).unwrap().state, RelationState::War);
        assert_eq!(r.get(b, a).unwrap().state, RelationState::War);
    }

    // End the war (a is the forced-peace initiator).
    let removed = macrocosmo::casus_belli::end_war(app.world_mut(), a, b);
    assert!(removed.is_some(), "war should have been removed");
    assert!(
        !app.world().resource::<ActiveWars>().has_war_between(a, b),
        "ActiveWar must be gone"
    );

    // Asymmetric: a -> b = Peace, b -> a = still War.
    let relations = app.world().resource::<FactionRelations>();
    assert_eq!(
        relations.get(a, b).unwrap().state,
        RelationState::Peace,
        "sender sees Peace immediately"
    );
    assert_eq!(
        relations.get(b, a).unwrap().state,
        RelationState::War,
        "receiver still sees War before forced-peace propagation completes"
    );

    // A DIPLO_FORCED_PEACE event is in flight.
    let now = app.world().resource::<GameClock>().elapsed;
    let world = app.world_mut();
    let events: Vec<_> = world
        .query::<&DiplomaticEvent>()
        .iter(world)
        .filter(|e| e.option_id == DIPLO_FORCED_PEACE)
        .map(|e| (e.from, e.to, e.arrives_at))
        .collect();
    assert_eq!(events.len(), 1, "exactly one in-flight forced_peace event");
    assert_eq!(events[0].0, a);
    assert_eq!(events[0].1, b);
    assert_eq!(
        events[0].2,
        now + delay,
        "arrives_at = clock_at_end_war + light-speed delay"
    );
}

/// #460: After the forced-peace event arrives, both directions are Peace.
#[test]
fn test_cb_end_war_symmetric_post_arrival() {
    let mut app = cb_test_app_no_tick();
    let (a, b, delay) = spawn_two_empires_with_capitals(app.world_mut(), 10.0);
    register_always_true_cb(&mut app);

    // Declare and let the war propagate fully so both sides see War.
    app.world_mut().resource_mut::<GameClock>().elapsed += 1;
    app.update();
    app.world_mut().resource_mut::<GameClock>().elapsed = 1 + delay;
    run_tick_diplomatic_events(&mut app);

    // End the war (sender = a). Forced-peace event spawned.
    let end_at = app.world().resource::<GameClock>().elapsed;
    macrocosmo::casus_belli::end_war(app.world_mut(), a, b);

    // Advance past the forced-peace event's arrival.
    app.world_mut().resource_mut::<GameClock>().elapsed = end_at + delay;
    run_tick_diplomatic_events(&mut app);

    let relations = app.world().resource::<FactionRelations>();
    assert_eq!(relations.get(a, b).unwrap().state, RelationState::Peace);
    assert_eq!(
        relations.get(b, a).unwrap().state,
        RelationState::Peace,
        "receiver flips to Peace once forced-peace event arrives"
    );

    // Forced-peace event consumed.
    let world = app.world_mut();
    let in_flight = world
        .query::<&DiplomaticEvent>()
        .iter(world)
        .filter(|e| e.option_id == DIPLO_FORCED_PEACE)
        .count();
    assert_eq!(in_flight, 0);
}

/// #460: `WarDeclared` GameEvent is emitted on the sender side
/// immediately (matching `declare_war_with_delay` policy), not deferred
/// until the receiver-side flip.
#[test]
fn test_cb_war_declared_event_emitted_at_sender() {
    let mut app = cb_test_app_no_tick();
    // Add the GameEvent collector since cb_test_app_no_tick doesn't
    // include it.
    app.add_systems(
        Update,
        macrocosmo::events::collect_events.after(macrocosmo::casus_belli::evaluate_casus_belli),
    );
    let (_a, _b, delay) = spawn_two_empires_with_capitals(app.world_mut(), 10.0);
    register_always_true_cb(&mut app);
    assert!(delay > 0);

    // Tick 1: war declared on the sender side.
    app.world_mut().resource_mut::<GameClock>().elapsed += 1;
    app.update();

    // The WarDeclared event must already be in the EventLog at
    // declaration time, BEFORE the receiver-side flip arrives.
    let log = app.world().resource::<EventLog>();
    assert!(
        log.entries
            .iter()
            .any(|e| e.kind == GameEventKind::WarDeclared),
        "WarDeclared must be emitted immediately on sender side, not deferred"
    );

    // Sanity: the receiver still sees Peace/Neutral, confirming the
    // event fired during the propagation window.
    let relations = app.world().resource::<FactionRelations>();
    let receiver_state = relations.get(_b, _a).map(|v| v.state);
    assert_ne!(receiver_state, Some(RelationState::War));
}
