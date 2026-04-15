use bevy::prelude::*;
use mlua::Lua;

use crate::event_system::EventSystem;
use crate::scripting::ScriptEngine;
use crate::time_system::GameClock;

/// Execute all registered on_game_start handlers **without** a gamestate
/// payload. Retained for unit tests that drive the hook directly without
/// spinning up a Bevy `World`. Production startup now goes through
/// [`run_on_game_start_with_gamestate`].
pub fn run_on_game_start(lua: &Lua) -> Result<(), mlua::Error> {
    run_handlers(lua, "_on_game_start_handlers")
}

/// Execute all registered on_game_load handlers **without** a gamestate
/// payload. Retained for unit tests (see [`run_on_game_start`] for the
/// same rationale). Production load paths should use
/// [`run_on_game_load_with_gamestate`].
pub fn run_on_game_load(lua: &Lua) -> Result<(), mlua::Error> {
    run_handlers(lua, "_on_game_load_handlers")
}

/// Execute all registered on_scripts_loaded handlers.
///
/// This hook runs with `()` as the argument — per plan §5.3, static
/// validation hooks do not receive a live gamestate (no world mutation
/// is expected; effect declaration stays in the scope-outer context via
/// `EffectScope`).
pub fn run_on_scripts_loaded(lua: &Lua) -> Result<(), mlua::Error> {
    run_handlers(lua, "_on_scripts_loaded_handlers")
}

/// Execute all `_on_game_start_handlers` with a live `ReadWrite`
/// `gamestate` attached to the payload. Each handler receives a Lua
/// table (`payload`) whose `gamestate` field exposes the
/// `gs:push_*_modifier` / `gs:set_flag` setter surface — see
/// `scripting::gamestate_scope::dispatch_with_gamestate`.
///
/// The gamestate is rebuilt per-handler so each callback sees a fresh
/// borrow; mutations performed by handler N are live for handler N+1.
pub fn run_on_game_start_with_gamestate(
    lua: &Lua,
    world: &mut World,
) -> Result<(), mlua::Error> {
    run_handlers_with_gamestate(lua, world, "_on_game_start_handlers")
}

/// Execute all `_on_game_load_handlers` with a live `ReadWrite`
/// gamestate attached to the payload. See [`run_on_game_start_with_gamestate`].
pub fn run_on_game_load_with_gamestate(
    lua: &Lua,
    world: &mut World,
) -> Result<(), mlua::Error> {
    run_handlers_with_gamestate(lua, world, "_on_game_load_handlers")
}

fn run_handlers(lua: &Lua, table_name: &str) -> Result<(), mlua::Error> {
    let handlers: mlua::Table = lua.globals().get(table_name)?;
    for i in 1..=handlers.len()? {
        let func: mlua::Function = handlers.get(i)?;
        func.call::<()>(())?;
    }
    Ok(())
}

fn run_handlers_with_gamestate(
    lua: &Lua,
    world: &mut World,
    table_name: &str,
) -> Result<(), mlua::Error> {
    use crate::scripting::gamestate_scope::{GamestateMode, dispatch_with_gamestate};
    let handlers: mlua::Table = lua.globals().get(table_name)?;
    let len = handlers.len()?;
    for i in 1..=len {
        let func: mlua::Function = handlers.get(i)?;
        let payload = lua.create_table()?;
        dispatch_with_gamestate(
            lua,
            world,
            &payload,
            GamestateMode::ReadWrite,
            |_lua, p| func.call::<()>(p.clone()),
        )?;
    }
    Ok(())
}


/// Startup system that runs lifecycle hooks after all scripts have been loaded.
/// Scripts are loaded by `load_all_scripts`; this system only executes callbacks.
/// Runs on_scripts_loaded and on_game_start hooks (on_game_load is reserved for
/// save/load which is not yet implemented).
///
/// #332 Phase B: promoted to an exclusive `&mut World` system so
/// `on_game_start` handlers receive a live `ReadWrite` gamestate (they
/// may call `gs:push_empire_modifier`, `gs:set_flag`, etc. to mutate
/// world state directly). `on_scripts_loaded` keeps the legacy
/// no-argument call because it's a static-validation hook (plan §5.3).
///
/// Phase B3 removed the `_pending_flags` drain that used to run here;
/// lifecycle flag mutation now goes through the gamestate setter,
/// which updates `GameFlags` / `ScopedFlags` synchronously.
pub fn run_lifecycle_hooks(world: &mut World) {
    crate::prof_span!("run_lifecycle_hooks");

    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();

        // Run on_scripts_loaded hooks (no gamestate — static validation only)
        match run_on_scripts_loaded(lua) {
            Ok(()) => info!("on_scripts_loaded hooks executed"),
            Err(e) => warn!("on_scripts_loaded hook error: {e}"),
        }

        // Run on_game_start hooks with ReadWrite gamestate.
        match run_on_game_start_with_gamestate(lua, world) {
            Ok(()) => info!("on_game_start hooks executed"),
            Err(e) => warn!("on_game_start hook error: {e}"),
        }
    });
}

/// Per-tick system that drains `_pending_script_events` from Lua and
/// forwards them to the Rust `EventSystem`.
pub fn drain_script_events(
    engine: Res<ScriptEngine>,
    mut event_system: ResMut<EventSystem>,
    clock: Res<GameClock>,
) {
    let lua = engine.lua();
    let Ok(events) = lua.globals().get::<mlua::Table>("_pending_script_events") else {
        return;
    };
    let Ok(len) = events.len() else {
        return;
    };
    if len == 0 {
        return;
    }
    crate::prof_span!("drain_script_events");

    for i in 1..=len {
        let Ok(entry) = events.get::<mlua::Table>(i) else {
            continue;
        };
        let Ok(event_id) = entry.get::<String>("event_id") else {
            continue;
        };
        let target: Option<u64> = entry.get("target").ok();
        // target is an Entity index — for now fire without target entity mapping
        let _ = target; // Reserved for future entity-target mapping
        event_system.fire_event(&event_id, None, clock.elapsed);
    }

    // Clear the table by replacing it with a fresh one
    if let Ok(new_table) = lua.create_table() {
        let _ = lua.globals().set("_pending_script_events", new_table);
    }
}

/// Exclusive Bevy system that runs immediately before `tick_events` and
/// filters out periodic / MTTH events whose `fire_condition` Lua callback
/// returns `false`. This is #263's wiring of the previously-ignored
/// `fire_condition` field.
///
/// Strategy:
/// * For each periodic event whose interval is due this tick, call its
///   `fire_condition` with the live `event.gamestate` snapshot. If the
///   callback returns a falsy value, bump `last_fired` to `now` so
///   `tick_events` treats it as "just fired" and skips it.
/// * For each pending MTTH event whose `fires_at <= now`, call its
///   `fire_condition`. If falsy, drop the entry from the pending queue so
///   `tick_events` never logs it as fired.
///
/// Callback errors are logged and treated as `true` (fire anyway) so that
/// a broken script can't silently disable an event definition without
/// leaving a warning in the log.
pub fn evaluate_fire_conditions(world: &mut World) {
    // Fast path: nothing to filter.
    let (has_periodic_or_pending, now) = {
        let clock = world
            .get_resource::<crate::time_system::GameClock>()
            .map(|c| c.elapsed)
            .unwrap_or(0);
        let es = world.get_resource::<EventSystem>();
        let have = es
            .map(|e| {
                let has_pending = !e.pending.is_empty();
                let has_periodic = e.definitions.values().any(|d| {
                    matches!(
                        d.trigger,
                        crate::event_system::EventTrigger::Periodic { .. }
                    )
                });
                has_pending || has_periodic
            })
            .unwrap_or(false);
        (have, clock)
    };
    if !has_periodic_or_pending {
        return;
    }

    // Collect decisions while ScriptEngine is out of the world.
    struct Decision {
        event_id: String,
        fire: bool,
    }

    // Snapshot the set of events we need to decide on (avoid borrowing
    // both EventSystem and &mut World at the same time while calling Lua).
    struct PendingDecision {
        kind: DecisionKind,
        event_id: String,
        fire_fn: Option<crate::event_system::LuaFunctionRef>,
    }
    #[derive(Clone, Copy)]
    enum DecisionKind {
        Periodic,
        /// Pending MTTH entry — the full `pending` vector is re-scanned by
        /// event id during the apply phase, so we don't need the index
        /// captured at snapshot time.
        PendingMtth,
    }

    let pending_decisions: Vec<PendingDecision> = {
        let Some(es) = world.get_resource::<EventSystem>() else {
            return;
        };
        let mut out = Vec::new();

        // Periodic events due this tick
        for (id, def) in &es.definitions {
            if let crate::event_system::EventTrigger::Periodic {
                interval_hexadies,
                last_fired,
                fire_condition,
                max_times,
                times_triggered,
            } = &def.trigger
            {
                if let Some(max) = max_times {
                    if *times_triggered >= *max {
                        continue;
                    }
                }
                if now - *last_fired >= *interval_hexadies && fire_condition.is_some() {
                    // #320: Only queue suppression checks for periodics that
                    // actually declare a fire_condition. Without one,
                    // `evaluate_fire_conditions` would build a gamestate
                    // snapshot just to answer "yes, fire" for every tick —
                    // wasted Lua work (and aux-stack pressure) since the
                    // actual fire is driven by `event_system::tick_events`
                    // and `evaluate_fire_conditions` only ever *suppresses*
                    // events that return false. Symmetric with the MTTH
                    // branch below, which already filters None out.
                    out.push(PendingDecision {
                        kind: DecisionKind::Periodic,
                        event_id: id.clone(),
                        fire_fn: fire_condition.clone(),
                    });
                }
            }
        }

        // Pending MTTH events whose time has come
        for pe in &es.pending {
            if pe.fires_at > now {
                continue;
            }
            // Look up the MTTH fire_condition on the event definition.
            let fire_fn = es
                .definitions
                .get(&pe.event_id)
                .and_then(|d| match &d.trigger {
                    crate::event_system::EventTrigger::Mtth { fire_condition, .. } => {
                        fire_condition.clone()
                    }
                    _ => None,
                });
            if fire_fn.is_some() {
                out.push(PendingDecision {
                    kind: DecisionKind::PendingMtth,
                    event_id: pe.event_id.clone(),
                    fire_fn,
                });
            }
        }
        out
    };

    if pending_decisions.is_empty() {
        return;
    }

    // Invoke each fire_condition with the live gamestate.
    //
    // #332: the gamestate is now built from scope closures (Option B),
    // not a snapshot. Closures expire when `dispatch_with_gamestate`
    // returns, so there is no snapshot residue to gc — the #320
    // `lua.gc_collect()` is therefore unnecessary here.
    //
    // `fire_condition` runs under `GamestateMode::ReadOnly`; setters
    // are not exposed, matching the spec that fire_condition must be
    // side-effect free.
    let mut decisions: Vec<Decision> = Vec::with_capacity(pending_decisions.len());
    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();
        let payload = match lua.create_table() {
            Ok(t) => t,
            Err(_) => return,
        };
        use crate::scripting::gamestate_scope::{GamestateMode, dispatch_with_gamestate};
        let dispatch_result = dispatch_with_gamestate(
            lua,
            world,
            &payload,
            GamestateMode::ReadOnly,
            |lua_inner, p| {
                for pd in &pending_decisions {
                    let fire = match &pd.fire_fn {
                        None => true,
                        Some(fref) => match fref.get(lua_inner) {
                            Ok(Some(func)) => match func.call::<mlua::Value>(p.clone()) {
                                Ok(mlua::Value::Boolean(b)) => b,
                                Ok(mlua::Value::Nil) => false,
                                Ok(_) => true, // truthy non-boolean
                                Err(e) => {
                                    warn!(
                                        "fire_condition for '{}' errored: {e}; firing anyway",
                                        pd.event_id
                                    );
                                    true
                                }
                            },
                            Ok(None) => true,
                            Err(e) => {
                                warn!(
                                    "fire_condition lookup for '{}' failed: {e}; firing anyway",
                                    pd.event_id
                                );
                                true
                            }
                        },
                    };
                    decisions.push(Decision {
                        event_id: pd.event_id.clone(),
                        fire,
                    });
                    let _ = pd.kind;
                }
                Ok(())
            },
        );
        if let Err(e) = dispatch_result {
            warn!("evaluate_fire_conditions: dispatch_with_gamestate failed: {e}");
        }
    });

    // Apply decisions: for suppressed periodic events, bump last_fired; for
    // suppressed pending MTTH events, remove them from the queue. We re-scan
    // the EventSystem here because indices may have shifted since the
    // snapshot.
    let mut es = world.resource_mut::<EventSystem>();
    let suppress_ids: std::collections::HashSet<String> = decisions
        .iter()
        .filter_map(|d| {
            if !d.fire {
                Some(d.event_id.clone())
            } else {
                None
            }
        })
        .collect();
    if suppress_ids.is_empty() {
        return;
    }

    // Suppress periodics by advancing their timer.
    for (id, def) in es.definitions.iter_mut() {
        if !suppress_ids.contains(id) {
            continue;
        }
        if let crate::event_system::EventTrigger::Periodic { last_fired, .. } = &mut def.trigger {
            *last_fired = now;
        }
    }

    // Suppress pending MTTH entries whose event id is in the suppress set
    // and whose fires_at <= now.
    es.pending
        .retain(|pe| !(pe.fires_at <= now && suppress_ids.contains(&pe.event_id)));
}

/// Per-tick **exclusive** system that dispatches recently fired events from
/// `EventSystem.fired_log` to Lua handlers — both:
/// * the `on(event_id, filter, fn)` bus handlers (stored in `_event_handlers`), and
/// * the `on_trigger` callback on the event definition itself (stored in the
///   Lua `_event_definitions` table).
///
/// The system runs with exclusive `&mut World` access so that `event.gamestate`
/// (a read-only world snapshot, #263) can be built inline and attached to the
/// payload table before any Lua callback is invoked. `ScriptEngine` is
/// re-acquired via `world.resource_scope` so we can hold both the Lua engine
/// and `&mut World` at the same time.
pub fn dispatch_event_handlers(world: &mut World) {
    // Fast path: nothing fired, skip world scope dance.
    let has_events = world
        .get_resource::<EventSystem>()
        .map(|es| !es.fired_log.is_empty())
        .unwrap_or(false);
    if !has_events {
        return;
    }

    // Drain fired log (mutable borrow scoped).
    let fired_events: Vec<crate::event_system::FiredEvent> = {
        let mut es = world.resource_mut::<EventSystem>();
        es.fired_log.drain(..).collect()
    };

    // Borrow ScriptEngine out of the world so that we can use &mut World to
    // build the gamestate scope for each dispatched event. `resource_scope`
    // temporarily removes the resource, giving us a &mut World that excludes
    // it; we restore it when the closure returns.
    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();
        for fired in &fired_events {
            // #288: Build the Lua payload table via the EventContext trait
            // when one is attached; otherwise fall back to an empty table
            // tagged with just `event_id` (the historical no-payload shape).
            let payload_table = match &fired.payload {
                Some(ctx) => match ctx.to_lua_table(lua) {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(
                            "dispatch_event_handlers: ctx.to_lua_table failed for '{}': {e}",
                            fired.event_id
                        );
                        continue;
                    }
                },
                None => match lua.create_table() {
                    Ok(t) => {
                        let _ = t.set("event_id", fired.event_id.as_str());
                        t
                    }
                    Err(e) => {
                        warn!("dispatch_event_handlers: failed to create payload table: {e}");
                        continue;
                    }
                },
            };

            // #332: attach a live gamestate via Option B scope closures.
            // Event callbacks may mutate the world via setter methods
            // (`ReadWrite` mode); the resulting borrow is released per
            // closure invocation. There is no snapshot residue, so the
            // old `lua.gc_collect()` aux-stack fix from #320 is no
            // longer needed.
            use crate::scripting::gamestate_scope::{GamestateMode, dispatch_with_gamestate};
            let event_id = fired.event_id.clone();
            let payload_clone = fired.payload.clone();
            let dispatch_result = dispatch_with_gamestate(
                lua,
                world,
                &payload_table,
                GamestateMode::ReadWrite,
                |lua_inner, pt| {
                    dispatch_bus_handlers(lua_inner, &event_id, payload_clone.as_deref(), pt);
                    dispatch_on_trigger(lua_inner, &event_id, pt);
                    Ok(())
                },
            );
            if let Err(e) = dispatch_result {
                warn!(
                    "dispatch_event_handlers: dispatch_with_gamestate failed for '{}': {e}",
                    fired.event_id
                );
            }
        }
    });
}

/// Re-implementation of `EventBus::fire` that reuses a caller-built
/// payload table (so `event.gamestate` is shared across all handlers for
/// a single fire).
///
/// #288: `ctx` replaces the old `&HashMap<String, String>` payload —
/// structural filters (`on(id, filter, fn)`) are evaluated via
/// `EventContext::payload_get`, wire-compatible with the pre-#288
/// HashMap flow for `LuaDefinedEventContext`. When `ctx` is `None` (an
/// event fired without a payload), every non-empty filter fails, which
/// matches the prior behaviour of an empty HashMap.
fn dispatch_bus_handlers(
    lua: &mlua::Lua,
    event_id: &str,
    ctx: Option<&dyn crate::event_system::EventContext>,
    payload_table: &mlua::Table,
) {
    let Ok(handlers) = lua.globals().get::<mlua::Table>("_event_handlers") else {
        return;
    };
    let len = handlers.len().unwrap_or(0);
    if len == 0 {
        return;
    }
    for i in 1..=len {
        let Ok(entry) = handlers.get::<mlua::Table>(i) else {
            continue;
        };
        let Ok(eid) = entry.get::<String>("event_id") else {
            continue;
        };
        if eid != event_id {
            continue;
        }
        // Structural filter
        if let Ok(filter) = entry.get::<mlua::Table>("filter") {
            let mut matches = true;
            for pair in filter.pairs::<String, String>() {
                if let Ok((k, v)) = pair {
                    let got = ctx.and_then(|c| c.payload_get(&k));
                    match got {
                        Some(pv) if pv.as_ref() == v.as_str() => {}
                        _ => {
                            matches = false;
                            break;
                        }
                    }
                }
            }
            if !matches {
                continue;
            }
        }
        if let Ok(func) = entry.get::<mlua::Function>("func") {
            if let Err(e) = func.call::<()>(payload_table.clone()) {
                warn!("EventBus handler error for {}: {}", event_id, e);
            }
        }
    }
}

/// Dispatch the `on_trigger` callback declared on an event definition
/// (`_event_definitions[i].on_trigger`). Unlike bus handlers (`on(id, fn)`),
/// `on_trigger` is defined at `define_event { ... on_trigger = fn }` time and
/// is keyed by the event definition's `id`.
fn dispatch_on_trigger(lua: &mlua::Lua, event_id: &str, payload_table: &mlua::Table) {
    let Ok(defs) = lua.globals().get::<mlua::Table>("_event_definitions") else {
        return;
    };
    let Ok(len) = defs.len() else { return };
    for i in 1..=len {
        let Ok(def) = defs.get::<mlua::Table>(i) else {
            continue;
        };
        let Ok(id) = def.get::<String>("id") else {
            continue;
        };
        if id != event_id {
            continue;
        }
        match def.get::<mlua::Value>("on_trigger") {
            Ok(mlua::Value::Function(f)) => {
                if let Err(e) = f.call::<()>(payload_table.clone()) {
                    warn!("on_trigger error for event '{}': {}", event_id, e);
                }
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::condition::ScopedFlags;
    use crate::player::PlayerEmpire;
    use crate::scripting::ScriptEngine;
    use crate::technology::GameFlags;
    use std::collections::HashMap;

    #[test]
    fn test_on_game_start_handler_called() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on_game_start(function()
                _test_game_start_called = true
            end)
            "#,
        )
        .exec()
        .unwrap();

        run_on_game_start(lua).unwrap();

        let called: bool = lua.globals().get("_test_game_start_called").unwrap();
        assert!(called);
    }

    #[test]
    fn test_multiple_handlers() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            _test_order = {}
            on_game_start(function()
                table.insert(_test_order, "first")
            end)
            on_game_start(function()
                table.insert(_test_order, "second")
            end)
            "#,
        )
        .exec()
        .unwrap();

        run_on_game_start(lua).unwrap();

        let order: mlua::Table = lua.globals().get("_test_order").unwrap();
        let first: String = order.get(1).unwrap();
        let second: String = order.get(2).unwrap();
        assert_eq!(first, "first");
        assert_eq!(second, "second");
        assert_eq!(order.len().unwrap(), 2);
    }

    #[test]
    fn test_on_scripts_loaded() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on_scripts_loaded(function()
                _test_scripts_loaded_called = true
            end)
            "#,
        )
        .exec()
        .unwrap();

        run_on_scripts_loaded(lua).unwrap();

        let called: bool = lua.globals().get("_test_scripts_loaded_called").unwrap();
        assert!(called);
    }

    #[test]
    fn test_on_game_load() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on_game_load(function()
                _test_game_load_called = true
            end)
            "#,
        )
        .exec()
        .unwrap();

        run_on_game_load(lua).unwrap();

        let called: bool = lua.globals().get("_test_game_load_called").unwrap();
        assert!(called);
    }

    #[test]
    fn test_lifecycle_hooks_fire_events() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on_game_start(function()
                fire_event("test_event")
            end)
            "#,
        )
        .exec()
        .unwrap();

        run_on_game_start(lua).unwrap();

        // Verify _pending_script_events has the event
        let events: mlua::Table = lua.globals().get("_pending_script_events").unwrap();
        assert_eq!(events.len().unwrap(), 1);
        let entry: mlua::Table = events.get(1).unwrap();
        let event_id: String = entry.get("event_id").unwrap();
        assert_eq!(event_id, "test_event");
    }

    #[test]
    fn test_no_handlers_is_ok() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // Should not error when no handlers are registered
        run_on_game_start(lua).unwrap();
        run_on_game_load(lua).unwrap();
        run_on_scripts_loaded(lua).unwrap();
    }

    // #332-B4: removed `test_drain_pending_flags` /
    // `test_drain_pending_flags_empty` — the `drain_pending_flags`
    // helper and its companion `set_flag(name)` global are gone, so
    // the tests have nothing to exercise.

    /// CRITICAL #2: Verify fire_event from Lua queues into _pending_script_events.
    #[test]
    fn test_drain_script_events_integration() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // Simulate fire_event from Lua side
        lua.load(r#"fire_event("test_event")"#).exec().unwrap();

        // Read _pending_script_events and verify the event was queued
        let events: mlua::Table = lua.globals().get("_pending_script_events").unwrap();
        assert_eq!(events.len().unwrap(), 1);
        let entry: mlua::Table = events.get(1).unwrap();
        let event_id: String = entry.get("event_id").unwrap();
        assert_eq!(event_id, "test_event");
    }

    /// CRITICAL #2: Verify fire_event with target parameter.
    #[test]
    fn test_drain_script_events_with_target() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(r#"fire_event("targeted_event", 42)"#)
            .exec()
            .unwrap();

        let events: mlua::Table = lua.globals().get("_pending_script_events").unwrap();
        assert_eq!(events.len().unwrap(), 1);
        let entry: mlua::Table = events.get(1).unwrap();
        let event_id: String = entry.get("event_id").unwrap();
        assert_eq!(event_id, "targeted_event");
        let target: u64 = entry.get("target").unwrap();
        assert_eq!(target, 42);
    }

    /// CRITICAL #2: Verify multiple fire_event calls accumulate.
    #[test]
    fn test_drain_script_events_multiple() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            fire_event("event_a")
            fire_event("event_b")
            fire_event("event_c")
        "#,
        )
        .exec()
        .unwrap();

        let events: mlua::Table = lua.globals().get("_pending_script_events").unwrap();
        assert_eq!(events.len().unwrap(), 3);

        let e1: mlua::Table = events.get(1).unwrap();
        assert_eq!(e1.get::<String>("event_id").unwrap(), "event_a");
        let e2: mlua::Table = events.get(2).unwrap();
        assert_eq!(e2.get::<String>("event_id").unwrap(), "event_b");
        let e3: mlua::Table = events.get(3).unwrap();
        assert_eq!(e3.get::<String>("event_id").unwrap(), "event_c");
    }

    // ------------- #263 dispatch + gamestate integration tests -------------

    use crate::event_system::FiredEvent;

    /// Build a minimal world with ScriptEngine + EventSystem + a player empire
    /// and clock, suitable for exercising `dispatch_event_handlers`.
    fn make_world() -> World {
        let mut world = World::new();
        world.insert_resource(crate::time_system::GameClock::new(42));
        world.insert_resource(EventSystem::default());
        world.insert_resource(ScriptEngine::new().unwrap());
        // Spawn a player empire so gamestate snapshot has something to show.
        let mut tree = crate::technology::TechTree::default();
        tree.researched
            .insert(crate::technology::TechId("tech_a".to_string()));
        let mut flags = GameFlags::default();
        flags.set("fa");
        world.spawn((
            crate::player::Empire { name: "E".into() },
            PlayerEmpire,
            tree,
            flags,
            ScopedFlags::default(),
            crate::technology::EmpireModifiers::default(),
        ));
        world
    }

    #[test]
    fn test_dispatch_attaches_gamestate_to_bus_handler() {
        let mut world = make_world();

        // Register a Lua `on` handler that records gamestate.clock.now.
        // #332: uses the method-based Option B API.
        {
            let engine = world.resource::<ScriptEngine>();
            engine
                .lua()
                .load(
                    r#"
                    _captured_now = -1
                    _captured_empire_name = nil
                    on("macrocosmo:test", function(evt)
                        _captured_now = evt.gamestate.clock.now
                        _captured_empire_name = evt.gamestate:player_empire().name
                    end)
                    "#,
                )
                .exec()
                .unwrap();
        }

        // Fire via fired_log directly
        {
            let mut es = world.resource_mut::<EventSystem>();
            es.fired_log.push(FiredEvent {
                event_id: "macrocosmo:test".to_string(),
                target: None,
                fired_at: 42,
                payload: None,
            });
        }

        dispatch_event_handlers(&mut world);

        let engine = world.resource::<ScriptEngine>();
        let now: i64 = engine.lua().globals().get("_captured_now").unwrap();
        assert_eq!(now, 42, "event.gamestate.clock.now must match GameClock");
        let name: String = engine.lua().globals().get("_captured_empire_name").unwrap();
        assert_eq!(name, "E");
    }

    #[test]
    fn test_dispatch_invokes_on_trigger_with_gamestate() {
        let mut world = make_world();
        {
            let engine = world.resource::<ScriptEngine>();
            engine
                .lua()
                .load(
                    r#"
                    _trigger_called = false
                    _trigger_has_tech = false
                    define_event {
                        id = "harvest_ended",
                        name = "Harvest Ended",
                        on_trigger = function(evt)
                            _trigger_called = true
                            -- #332 Option B: player_empire is a method, techs is a plain table
                            _trigger_has_tech = evt.gamestate:player_empire().techs.tech_a == true
                        end,
                    }
                    "#,
                )
                .exec()
                .unwrap();
        }

        {
            let mut es = world.resource_mut::<EventSystem>();
            es.fired_log.push(FiredEvent {
                event_id: "harvest_ended".to_string(),
                target: None,
                fired_at: 42,
                payload: None,
            });
        }

        dispatch_event_handlers(&mut world);

        let engine = world.resource::<ScriptEngine>();
        let called: bool = engine.lua().globals().get("_trigger_called").unwrap();
        assert!(called, "on_trigger must fire when event_id matches");
        let has_tech: bool = engine.lua().globals().get("_trigger_has_tech").unwrap();
        assert!(
            has_tech,
            "gamestate techs lookup must work inside on_trigger"
        );
    }

    /// #332 Option B: event callbacks can mutate world state live via
    /// setter methods. This test verifies that scribbling on the
    /// gamestate table directly is *tolerated* (plain Lua tables, no
    /// seal), while the canonical setter path `push_*_modifier` / etc.
    /// is what actually mutates World. We run a trivial setter in a
    /// handler and confirm no error escapes.
    #[test]
    fn test_dispatch_handler_can_mutate_via_setter() {
        let mut world = make_world();
        {
            let engine = world.resource::<ScriptEngine>();
            engine
                .lua()
                .load(
                    r#"
                    _setter_ok = false
                    on("macrocosmo:mutate", function(evt)
                        local pe = evt.gamestate:player_empire()
                        -- Unknown targets surface a RuntimeError; any
                        -- recognised target proves setter wiring works.
                        -- Wrap in pcall so the test distinguishes
                        -- "setter reached Rust side" from "error".
                        local ok, err = pcall(function()
                            evt.gamestate:push_empire_modifier(
                                pe.id,
                                "empire.population_growth",
                                { id = "live_test", add = 0.5 }
                            )
                        end)
                        _setter_ok = ok
                        _setter_err = err and tostring(err) or nil
                    end)
                    "#,
                )
                .exec()
                .unwrap();
        }
        {
            let mut es = world.resource_mut::<EventSystem>();
            es.fired_log.push(FiredEvent {
                event_id: "macrocosmo:mutate".to_string(),
                target: None,
                fired_at: 42,
                payload: None,
            });
        }

        dispatch_event_handlers(&mut world);

        let engine = world.resource::<ScriptEngine>();
        let ok: bool = engine.lua().globals().get("_setter_ok").unwrap();
        let err: Option<String> = engine.lua().globals().get("_setter_err").ok();
        assert!(
            ok,
            "setter should succeed under ReadWrite mode, got err: {err:?}"
        );
    }

    #[test]
    fn test_dispatch_empty_fired_log_is_noop() {
        let mut world = make_world();
        // No events fired. Should not panic, should not touch Lua state.
        dispatch_event_handlers(&mut world);
        // Sanity: resource still there.
        assert!(world.get_resource::<EventSystem>().is_some());
        assert!(world.get_resource::<ScriptEngine>().is_some());
    }

    #[test]
    fn test_fire_condition_suppresses_periodic_event() {
        use crate::event_system::{EventDefinition, EventTrigger, LuaFunctionRef};

        let mut world = make_world();
        // Register a periodic event in Lua so the fire_condition function is
        // real (RegistryKey-backed), then register the same trigger in the
        // Rust EventSystem.
        let fref = {
            let engine = world.resource::<ScriptEngine>();
            let lua = engine.lua();
            // Condition returns false: event should be suppressed.
            let f: mlua::Function = lua
                .load(r#"return function(evt) return false end"#)
                .eval()
                .unwrap();
            LuaFunctionRef::from_function(lua, f).unwrap()
        };
        {
            let mut es = world.resource_mut::<EventSystem>();
            es.register(EventDefinition {
                id: "periodic_censored".to_string(),
                name: "Periodic Censored".to_string(),
                description: "Periodic event whose fire_condition returns false.".to_string(),
                trigger: EventTrigger::Periodic {
                    interval_hexadies: 10,
                    last_fired: 0,
                    fire_condition: Some(fref),
                    max_times: None,
                    times_triggered: 0,
                },
            });
        }

        // At clock=42 (from make_world), interval=10 since last_fired=0 is due.
        // evaluate_fire_conditions must suppress by bumping last_fired to now=42.
        evaluate_fire_conditions(&mut world);

        let es = world.resource::<EventSystem>();
        let def = es.definitions.get("periodic_censored").unwrap();
        match &def.trigger {
            EventTrigger::Periodic { last_fired, .. } => {
                assert_eq!(
                    *last_fired, 42,
                    "suppressed periodic event should have last_fired bumped to now"
                );
            }
            _ => panic!("expected Periodic trigger"),
        }
    }

    #[test]
    fn test_fire_condition_allows_periodic_event() {
        use crate::event_system::{EventDefinition, EventTrigger, LuaFunctionRef};

        let mut world = make_world();
        let fref = {
            let engine = world.resource::<ScriptEngine>();
            let lua = engine.lua();
            let f: mlua::Function = lua
                .load(r#"return function(evt) return evt.gamestate.player_empire.techs.tech_a end"#)
                .eval()
                .unwrap();
            LuaFunctionRef::from_function(lua, f).unwrap()
        };
        {
            let mut es = world.resource_mut::<EventSystem>();
            es.register(EventDefinition {
                id: "periodic_gated".to_string(),
                name: "Gated".to_string(),
                description: "Fires when tech_a is researched.".to_string(),
                trigger: EventTrigger::Periodic {
                    interval_hexadies: 10,
                    last_fired: 0,
                    fire_condition: Some(fref),
                    max_times: None,
                    times_triggered: 0,
                },
            });
        }

        evaluate_fire_conditions(&mut world);

        let es = world.resource::<EventSystem>();
        let def = es.definitions.get("periodic_gated").unwrap();
        match &def.trigger {
            EventTrigger::Periodic { last_fired, .. } => {
                assert_eq!(
                    *last_fired, 0,
                    "event should not be suppressed when fire_condition returns truthy"
                );
            }
            _ => panic!("expected Periodic trigger"),
        }
    }

    #[test]
    fn test_fire_condition_suppresses_pending_mtth_event() {
        use crate::event_system::{EventDefinition, EventTrigger, LuaFunctionRef, PendingEvent};

        let mut world = make_world();
        let fref = {
            let engine = world.resource::<ScriptEngine>();
            let lua = engine.lua();
            let f: mlua::Function = lua
                .load(r#"return function(evt) return false end"#)
                .eval()
                .unwrap();
            LuaFunctionRef::from_function(lua, f).unwrap()
        };
        {
            let mut es = world.resource_mut::<EventSystem>();
            es.register(EventDefinition {
                id: "mtth_filtered".to_string(),
                name: "Filtered MTTH".to_string(),
                description: "".into(),
                trigger: EventTrigger::Mtth {
                    mean_hexadies: 100,
                    fire_condition: Some(fref),
                    max_times: None,
                    times_triggered: 0,
                },
            });
            // Pending entry whose fires_at <= now (42).
            es.pending.push(PendingEvent {
                event_id: "mtth_filtered".to_string(),
                target: None,
                fires_at: 10,
            });
        }

        evaluate_fire_conditions(&mut world);

        let es = world.resource::<EventSystem>();
        assert!(
            es.pending.is_empty(),
            "suppressed MTTH event must be dropped from pending queue"
        );
    }

    #[test]
    fn test_existing_event_scripts_still_work() {
        // Regression: a pre-#263 event script that doesn't touch gamestate
        // must still receive payload fields as before.
        let mut world = make_world();
        {
            let engine = world.resource::<ScriptEngine>();
            engine
                .lua()
                .load(
                    r#"
                    _old_style_cause = nil
                    on("macrocosmo:building_lost", { cause = "combat" }, function(evt)
                        _old_style_cause = evt.cause
                    end)
                    "#,
                )
                .exec()
                .unwrap();
        }
        {
            let mut es = world.resource_mut::<EventSystem>();
            let mut payload = HashMap::new();
            payload.insert("cause".to_string(), "combat".to_string());
            payload.insert("building_id".to_string(), "mine".to_string());
            let ctx = crate::event_system::LuaDefinedEventContext::new(
                "macrocosmo:building_lost",
                payload,
            );
            es.fired_log.push(FiredEvent {
                event_id: "macrocosmo:building_lost".to_string(),
                target: None,
                fired_at: 1,
                payload: Some(std::sync::Arc::new(ctx)),
            });
        }

        dispatch_event_handlers(&mut world);

        let engine = world.resource::<ScriptEngine>();
        let cause: String = engine.lua().globals().get("_old_style_cause").unwrap();
        assert_eq!(cause, "combat");
    }
}
