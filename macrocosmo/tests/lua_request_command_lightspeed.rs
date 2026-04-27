//! #461 regression: `gs:request_command` honours light-speed delay.
//!
//! Before this PR, `apply::request_command` wrote the typed
//! `*Requested` message immediately regardless of issuer-target
//! distance — Lua scripts could remote-control any ship instantly,
//! bypassing the same physics gate that throttles colony commands
//! through `send_remote_command` (#268) and AI commands through the
//! `AiCommandOutbox` (Round 9).
//!
//! These tests pin the corrected contract:
//!
//! 1. `request_command_local_emits_immediately` — issuer (PlayerEmpire
//!    Ruler) at the same position as the target ship → typed message
//!    fires on the same call. Legacy behaviour preserved.
//! 2. `request_command_remote_holds_until_light_delay_elapses` — Ruler
//!    several light-years from the target ship → no `*Requested` yet,
//!    a `PendingScriptedCommand` entity is queued, and only after the
//!    drainer system runs at `clock.elapsed >= arrives_at` does the
//!    typed message land.
//! 3. `request_command_remote_silent_before_arrival` — companion to
//!    (2): explicitly assert the queue gate by stepping the drainer
//!    *before* arrival and confirming zero `*Requested` events.

use bevy::ecs::message::Messages;
use bevy::prelude::*;
use macrocosmo::components::Position;
use macrocosmo::physics::light_delay_hexadies;
use macrocosmo::player::{Empire, EmpireRuler, PlayerEmpire, Ruler, StationedAt};
use macrocosmo::scripting::ScriptEngine;
use macrocosmo::scripting::gamestate_scope::apply::{
    self, ParsedRequest, PendingScriptedCommand, dispatch_pending_scripted_commands,
};
use macrocosmo::scripting::gamestate_scope::{GamestateMode, dispatch_with_gamestate};
use macrocosmo::ship::command_events::{MoveRequested, NextCommandId};
use macrocosmo::time_system::GameClock;

/// Build a minimal world wired with everything `apply::request_command`
/// + `dispatch_pending_scripted_commands` need: clock, command id
/// allocator, message queues, plus a Player empire whose Ruler is
/// stationed at `home_pos` and a target ship at `ship_pos`.
fn make_world(home_pos: [f64; 3], ship_pos: [f64; 3], clock_elapsed: i64) -> (World, Entity) {
    let mut world = World::new();
    world.insert_resource(GameClock::new(clock_elapsed));
    world.insert_resource(NextCommandId::default());
    // Seed only the message queues the test exercises (Move + Survey
    // are the two we touch here). Other typed messages can be added if
    // future variants are tested.
    world.insert_resource(Messages::<MoveRequested>::default());
    world.insert_resource(Messages::<macrocosmo::ship::command_events::SurveyRequested>::default());
    // ScriptEngine isn't used by `apply::request_command` directly but
    // some unrelated systems probe for it; not required here.
    let _ = ScriptEngine::new(); // sanity construct, not stored

    // Spawn the home star system at `home_pos` so StationedAt has a
    // resolvable Position. We don't need StarSystem semantics for this
    // test — just the Position component the issuer-position lookup
    // walks through.
    let home_system = world.spawn(Position::from(home_pos)).id();

    // Spawn the player empire entity + the Ruler entity, then link
    // them via `EmpireRuler` exactly like `spawn_ruler_for_empire` does.
    let empire = world
        .spawn((Empire { name: "P".into() }, PlayerEmpire))
        .id();
    let ruler = world
        .spawn((
            Ruler {
                name: "R".into(),
                empire,
            },
            StationedAt {
                system: home_system,
            },
        ))
        .id();
    world.entity_mut(empire).insert(EmpireRuler(ruler));

    // Target ship at `ship_pos` — only the Position is needed for the
    // light-delay calculation (the `*Requested` handler is not run in
    // this test).
    let ship = world.spawn(Position::from(ship_pos)).id();

    (world, ship)
}

fn count_move_requested(world: &World) -> usize {
    let msgs = world.resource::<Messages<MoveRequested>>();
    msgs.iter_current_update_messages().count()
}

fn count_pending(world: &mut World) -> usize {
    let mut q = world.query::<&PendingScriptedCommand>();
    q.iter(world).count()
}

/// Local case: Ruler stationed at the same position as the target
/// ship → `apply::request_command` must emit `MoveRequested` synchronously
/// (no `PendingScriptedCommand` queued).
#[test]
fn request_command_local_emits_immediately() {
    let pos = [0.0, 0.0, 0.0];
    let (mut world, ship) = make_world(pos, pos, 100);

    // Use a target system entity with a Position; Move's `target` is a
    // system entity (not a ship) — the field type doesn't affect the
    // light-delay path which keys on `parsed_request_ship` (= ship).
    let target_system = world.spawn(Position::from([0.0, 0.0, 0.0])).id();

    let req = ParsedRequest::Move {
        ship,
        target: target_system,
    };
    let id = apply::request_command(&mut world, req).expect("local request_command must succeed");
    assert!(id > 0);

    // Local path: typed message emitted immediately, no pending queue.
    assert_eq!(
        count_move_requested(&world),
        1,
        "local issuer must emit MoveRequested synchronously"
    );
    assert_eq!(
        count_pending(&mut world),
        0,
        "local issuer must NOT spawn a PendingScriptedCommand"
    );
}

/// Remote case (silent before arrival): Ruler 5 ly from the ship →
/// `request_command` queues a `PendingScriptedCommand`. Running the
/// drainer at the same tick (or any tick before `arrives_at`) must
/// keep the typed message queue empty.
#[test]
fn request_command_remote_silent_before_arrival() {
    let distance_ly = 5.0;
    let (mut world, ship) = make_world([0.0, 0.0, 0.0], [distance_ly, 0.0, 0.0], 100);
    let target_system = world.spawn(Position::from([distance_ly, 0.0, 0.0])).id();

    let req = ParsedRequest::Move {
        ship,
        target: target_system,
    };
    let _id = apply::request_command(&mut world, req).expect("remote request_command must succeed");

    // Remote path: PendingScriptedCommand queued, no typed message yet.
    assert_eq!(
        count_move_requested(&world),
        0,
        "remote issuer must NOT emit MoveRequested synchronously"
    );
    assert_eq!(
        count_pending(&mut world),
        1,
        "remote issuer must enqueue exactly one PendingScriptedCommand"
    );

    // Drainer at clock = sent_at: arrival not yet due, must stay silent.
    dispatch_pending_scripted_commands(&mut world);
    assert_eq!(
        count_move_requested(&world),
        0,
        "drainer fired BEFORE arrival must not emit"
    );
    assert_eq!(
        count_pending(&mut world),
        1,
        "drainer must keep the entry until arrival"
    );

    // Step clock to one tick BEFORE arrival; still silent.
    let delay = light_delay_hexadies(distance_ly);
    world.resource_mut::<GameClock>().elapsed = 100 + delay - 1;
    dispatch_pending_scripted_commands(&mut world);
    assert_eq!(
        count_move_requested(&world),
        0,
        "drainer one tick before arrival must not emit"
    );
}

/// Remote case (arrival): step the clock past `arrives_at`, run the
/// drainer, and confirm the typed message lands plus the queue empties.
#[test]
fn request_command_remote_emits_after_arrival() {
    let distance_ly = 5.0;
    let (mut world, ship) = make_world([0.0, 0.0, 0.0], [distance_ly, 0.0, 0.0], 100);
    let target_system = world.spawn(Position::from([distance_ly, 0.0, 0.0])).id();

    let req = ParsedRequest::Move {
        ship,
        target: target_system,
    };
    let id = apply::request_command(&mut world, req).expect("remote request_command must succeed");

    // Sanity: queued, not emitted yet.
    assert_eq!(count_pending(&mut world), 1);
    assert_eq!(count_move_requested(&world), 0);

    // Advance to arrival tick and drain.
    let delay = light_delay_hexadies(distance_ly);
    world.resource_mut::<GameClock>().elapsed = 100 + delay;
    dispatch_pending_scripted_commands(&mut world);

    // Typed message must now be emitted, queue drained.
    assert_eq!(
        count_move_requested(&world),
        1,
        "drainer at arrival tick must emit exactly one MoveRequested"
    );
    assert_eq!(
        count_pending(&mut world),
        0,
        "drainer must despawn the entry after emit"
    );

    // The emitted message carries the original command_id and an
    // `issued_at` equal to the original sent_at (not the arrival tick).
    let msgs = world.resource::<Messages<MoveRequested>>();
    let mut cursor = msgs.get_cursor();
    let all: Vec<&MoveRequested> = cursor.read(msgs).collect();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].command_id.0, id, "command_id preserved across delay");
    assert_eq!(
        all[0].issued_at, 100,
        "issued_at must reflect the original send tick, not arrival"
    );
}

/// Lua-driven end-to-end: invoke `gs:request_command` via the scope
/// closure with a remote PlayerEmpire vantage. The call must NOT emit
/// the typed message synchronously; it must enqueue a
/// `PendingScriptedCommand`. After advancing the clock past arrival,
/// running the drainer must release the message.
#[test]
fn lua_request_command_remote_routes_through_pending_queue() {
    let distance_ly = 5.0;
    let (mut world, ship) = make_world([0.0, 0.0, 0.0], [distance_ly, 0.0, 0.0], 100);
    // We need a ScriptEngine resource for the scope dispatch.
    world.insert_resource(ScriptEngine::new().expect("script engine"));
    let target_system = world.spawn(Position::from([distance_ly, 0.0, 0.0])).id();

    // Drive the call through the Lua scope closure exactly as a real
    // event handler would.
    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();
        lua.globals().set("_ship", ship.to_bits()).unwrap();
        lua.globals()
            .set("_target", target_system.to_bits())
            .unwrap();
        let payload = lua.create_table().unwrap();
        dispatch_with_gamestate(lua, world, &payload, GamestateMode::ReadWrite, |lua, p| {
            lua.globals().set("_evt", p.clone())?;
            lua.load(
                r#"
                _id = _evt.gamestate:request_command("move", {
                    ship = _ship, target = _target,
                })
                "#,
            )
            .exec()?;
            Ok(())
        })
        .unwrap();
    });

    // Synchronous emit must NOT have happened — Lua call queued a
    // PendingScriptedCommand instead.
    assert_eq!(
        count_move_requested(&world),
        0,
        "Lua-driven remote request must not emit synchronously"
    );
    assert_eq!(
        count_pending(&mut world),
        1,
        "Lua-driven remote request must enqueue a PendingScriptedCommand"
    );

    // Advance to arrival and drain.
    let delay = light_delay_hexadies(distance_ly);
    world.resource_mut::<GameClock>().elapsed = 100 + delay;
    dispatch_pending_scripted_commands(&mut world);
    assert_eq!(
        count_move_requested(&world),
        1,
        "drainer must release the queued message after light delay"
    );
}
