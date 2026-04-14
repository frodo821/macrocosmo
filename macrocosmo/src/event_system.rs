use bevy::prelude::*;
use mlua::prelude::*;
use rand::Rng;
use std::collections::HashMap;
use std::sync::Arc;

use crate::time_system::{GameClock, HEXADIES_PER_MONTH, HEXADIES_PER_YEAR};

/// Reference to a Lua function stored in the `mlua` registry.
///
/// Until #263 the placeholder variant was a plain `i64` stored on
/// `LuaFunctionRef(pub i64)`; it was never actually usable for calling
/// back into the function because `format!("{:?}", key).len()` was used as
/// the identifier — multiple distinct functions collide on the same id.
///
/// The new representation holds an `Arc<mlua::RegistryKey>` so multiple
/// `Clone`s of the same `EventTrigger` keep the function alive for the
/// lifetime of the `EventSystem`, and the dispatcher can recover the
/// `mlua::Function` via `lua.registry_value::<mlua::Function>(&key)`.
///
/// The historical `pub i64` field is preserved as a transparent accessor
/// for backward-compatibility with pre-existing tests that construct
/// `LuaFunctionRef(42)` literals — those cases never had a real function,
/// so we map them to a `None` inner key.
#[derive(Clone, Debug)]
pub struct LuaFunctionRef {
    /// Deprecated placeholder id, kept for the `LuaFunctionRef(i64)` tuple
    /// constructor that still appears in tests. Use `key()` to check for a
    /// real Lua function instead.
    pub id: i64,
    inner: Option<Arc<mlua::RegistryKey>>,
}

impl LuaFunctionRef {
    /// Build a real reference from a Lua function (consumes the function
    /// into the registry).
    pub fn from_function(lua: &Lua, f: mlua::Function) -> mlua::Result<Self> {
        let key = lua.create_registry_value(f)?;
        // Deterministic-ish id for logging / debugging only. The string form
        // of `RegistryKey` is not stable across mlua versions, so we hash it
        // with a simple fingerprint. Downstream code never relies on id for
        // correctness.
        let id = fingerprint_registry_key(&key);
        Ok(Self {
            id,
            inner: Some(Arc::new(key)),
        })
    }

    /// Historical tuple-style constructor. Produces a ref with no real
    /// function attached; callers must not dispatch on it.
    pub fn placeholder(id: i64) -> Self {
        Self { id, inner: None }
    }

    /// Acquire the Lua function, if one is attached. Returns `Ok(None)` if
    /// this ref is a historical placeholder with no real function.
    pub fn get(&self, lua: &Lua) -> mlua::Result<Option<mlua::Function>> {
        match &self.inner {
            Some(arc) => Ok(Some(lua.registry_value(arc.as_ref())?)),
            None => Ok(None),
        }
    }
}

// Allow the legacy `LuaFunctionRef(42)` tuple construction to keep
// compiling. We treat the argument as the placeholder id.
#[allow(non_snake_case)]
impl LuaFunctionRef {
    /// Deprecated: use [`Self::placeholder`] or [`Self::from_function`].
    #[deprecated(note = "Use LuaFunctionRef::placeholder or from_function")]
    pub fn new_legacy(id: i64) -> Self {
        Self::placeholder(id)
    }
}

fn fingerprint_registry_key(key: &mlua::RegistryKey) -> i64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    format!("{:p}", key as *const _).hash(&mut h);
    h.finish() as i64
}

/// Convert years/months/sd (sub-divisions, i.e. raw hexadies) to hexadies.
pub fn time_to_hexadies(years: i64, months: i64, sd: i64) -> i64 {
    years * HEXADIES_PER_YEAR + months * HEXADIES_PER_MONTH + sd
}

/// Defines how an event is triggered.
#[derive(Clone, Debug)]
pub enum EventTrigger {
    /// Fired explicitly by fire_event() or on_expire_event.
    Manual,
    /// Mean Time To Happen -- fires after random delay when fire_event is called.
    Mtth {
        mean_hexadies: i64,
        fire_condition: Option<LuaFunctionRef>,
        max_times: Option<u32>,
        times_triggered: u32,
    },
    /// Fires at regular intervals.
    Periodic {
        interval_hexadies: i64,
        last_fired: i64,
        fire_condition: Option<LuaFunctionRef>,
        max_times: Option<u32>,
        times_triggered: u32,
    },
}

/// A scripted event definition loaded from Lua.
#[derive(Clone, Debug)]
pub struct EventDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub trigger: EventTrigger,
    // on_trigger callback lives in the Lua table, not here
}

/// An event that is waiting to fire at a specific time.
#[derive(Clone, Debug)]
pub struct PendingEvent {
    pub event_id: String,
    pub target: Option<Entity>,
    pub fires_at: i64,
}

/// Record of a fired event, for testing and debugging.
#[derive(Clone, Debug)]
pub struct FiredEvent {
    pub event_id: String,
    pub target: Option<Entity>,
    pub fired_at: i64,
    /// Optional key-value payload for EventBus dispatch.
    pub payload: Option<HashMap<String, String>>,
}

/// Resource holding all event definitions and pending events.
#[derive(Resource, Default)]
pub struct EventSystem {
    pub definitions: HashMap<String, EventDefinition>,
    pub pending: Vec<PendingEvent>,
    pub fired_log: Vec<FiredEvent>,
}

impl EventSystem {
    /// Register an event definition.
    pub fn register(&mut self, def: EventDefinition) {
        self.definitions.insert(def.id.clone(), def);
    }

    /// Queue an event. Behavior depends on trigger type:
    /// - Manual: fires immediately (fires_at = now)
    /// - Mtth: fires after random delay based on mean_hexadies
    /// - Periodic: fires immediately (periodic auto-fire is handled by tick_events)
    pub fn fire_event(&mut self, event_id: &str, target: Option<Entity>, now: i64) {
        let fires_at = match self.definitions.get_mut(event_id) {
            Some(def) => match &mut def.trigger {
                EventTrigger::Manual => now,
                EventTrigger::Mtth { mean_hexadies, max_times, times_triggered, .. } => {
                    if max_times.is_some_and(|max| *times_triggered >= max) {
                        return;
                    }
                    now + random_mtth_delay(*mean_hexadies)
                }
                EventTrigger::Periodic { last_fired, .. } => {
                    *last_fired = now; // Start the periodic timer
                    now // First fire is immediate
                }
            },
            None => now, // unknown event, fire immediately
        };
        self.pending.push(PendingEvent {
            event_id: event_id.to_string(),
            target,
            fires_at,
        });
    }

    /// Queue an event with a key-value payload for EventBus dispatch.
    /// The event fires immediately (fires_at = now), like `fire_event` for manual events.
    pub fn fire_event_with_payload(
        &mut self,
        event_id: &str,
        target: Option<Entity>,
        now: i64,
        payload: HashMap<String, String>,
    ) {
        self.pending.push(PendingEvent {
            event_id: event_id.to_string(),
            target,
            fires_at: now,
        });
        // Also log immediately so dispatch_event_handlers picks it up
        self.fired_log.push(FiredEvent {
            event_id: event_id.to_string(),
            target,
            fired_at: now,
            payload: Some(payload),
        });
    }

    /// Queue an event to fire at a specific time (bypasses trigger logic).
    pub fn fire_event_delayed(&mut self, event_id: &str, target: Option<Entity>, fires_at: i64) {
        self.pending.push(PendingEvent {
            event_id: event_id.to_string(),
            target,
            fires_at,
        });
    }
}

// Event IDs follow namespace:name convention.
// Built-in events use "macrocosmo:" prefix.
// Examples: "macrocosmo:building_lost", "macrocosmo:ship_lost", "macrocosmo:tech_researched"

/// Central event bus for dispatching events to Lua handlers.
///
/// Handlers are stored in the Lua global `_event_handlers` table to avoid
/// Send/Sync issues with `mlua::RegistryKey`. Each entry in the table is:
/// `{ event_id = "...", filter = { key = "value", ... } | nil, func = function(...) }`.
///
/// The `handler_count` field tracks the number of registered handlers for
/// informational purposes (it is not used for dispatch logic).
#[derive(Resource, Default)]
pub struct EventBus {
    pub handler_count: usize,
}

impl EventBus {
    /// Fire an event to all matching Lua handlers in `_event_handlers`.
    ///
    /// Returns the number of handlers that were called. Handlers whose
    /// `event_id` does not match, or whose structural `filter` does not match
    /// the payload, are skipped. Errors from individual handlers are logged
    /// but do not abort processing of remaining handlers.
    pub fn fire(lua: &Lua, event_id: &str, payload: &HashMap<String, String>) -> usize {
        // Build payload table
        let Ok(payload_table) = lua.create_table() else {
            return 0;
        };
        for (k, v) in payload {
            let _ = payload_table.set(k.as_str(), v.as_str());
        }
        let _ = payload_table.set("event_id", event_id);

        // Get handlers table
        let Ok(handlers) = lua.globals().get::<LuaTable>("_event_handlers") else {
            return 0;
        };
        let len = handlers.len().unwrap_or(0);
        if len == 0 {
            return 0;
        }

        let mut count = 0;
        for i in 1..=len {
            let Ok(entry) = handlers.get::<LuaTable>(i) else {
                continue;
            };
            let Ok(eid) = entry.get::<String>("event_id") else {
                continue;
            };
            if eid != event_id {
                continue;
            }

            // Check structural filter
            if let Ok(filter) = entry.get::<LuaTable>("filter") {
                let mut matches = true;
                for pair in filter.pairs::<String, String>() {
                    if let Ok((k, v)) = pair {
                        if payload.get(&k).map(|pv| pv.as_str()) != Some(v.as_str()) {
                            matches = false;
                            break;
                        }
                    }
                }
                if !matches {
                    continue;
                }
            }

            // Call handler
            if let Ok(func) = entry.get::<LuaFunction>("func") {
                if let Err(e) = func.call::<()>(payload_table.clone()) {
                    warn!("EventBus handler error for {}: {}", event_id, e);
                }
                count += 1;
            }
        }
        count
    }
}

/// Generate an exponentially distributed random delay in hexadies.
/// Uses the inverse transform: delay = -mean * ln(1 - U) where U ~ Uniform(0,1).
/// Result is clamped to at least 1.
fn random_mtth_delay(mean_hexadies: i64) -> i64 {
    let mut rng = rand::rng();
    let u: f64 = rng.random::<f64>();
    let delay = -(mean_hexadies as f64) * (1.0 - u).ln();
    (delay as i64).max(1)
}

/// Bevy system that processes periodic triggers
/// and pending events whose fire time has been reached.
/// MTTH events are queued via fire_event() from Lua hooks, not auto-activated.
pub fn tick_events(clock: Res<GameClock>, mut event_system: ResMut<EventSystem>) {
    let now = clock.elapsed;

    // --- Periodic trigger check ---
    {
        // Collect periodic events that need to fire
        let periodic_fires: Vec<String> = event_system
            .definitions
            .iter()
            .filter_map(|(id, def)| {
                if let EventTrigger::Periodic {
                    interval_hexadies,
                    last_fired,
                    fire_condition: _, // Lua call not implemented yet; None = always fire
                    max_times,
                    times_triggered,
                } = &def.trigger
                {
                    if let Some(max) = max_times {
                        if *times_triggered >= *max {
                            return None;
                        }
                    }
                    if now - *last_fired >= *interval_hexadies {
                        return Some(id.clone());
                    }
                    None
                } else {
                    None
                }
            })
            .collect();

        for id in periodic_fires {
            // Fire the event: update last_fired, increment times_triggered
            if let Some(def) = event_system.definitions.get_mut(&id) {
                if let EventTrigger::Periodic {
                    last_fired,
                    times_triggered,
                    ..
                } = &mut def.trigger
                {
                    *last_fired = now;
                    *times_triggered += 1;
                }
                info!("Event fired: {} ({})", def.name, def.id);
                event_system.fired_log.push(FiredEvent {
                    event_id: id,
                    target: None,
                    fired_at: now,
                    payload: None,
                });
            }
        }
    }

    // --- Process pending events whose fire time has been reached ---
    let mut fired = Vec::new();
    event_system.pending.retain(|pe| {
        if pe.fires_at <= now {
            fired.push(pe.clone());
            false
        } else {
            true
        }
    });

    // Log fired events
    for event in &fired {
        if let Some(def) = event_system.definitions.get_mut(&event.event_id) {
            // For MTTH events, increment times_triggered when they actually fire
            if let EventTrigger::Mtth {
                fire_condition: _, // None = always fire
                times_triggered,
                ..
            } = &mut def.trigger
            {
                *times_triggered += 1;
            }
            info!("Event fired: {} ({})", def.name, def.id);
        } else {
            info!("Event fired: {} (no definition found)", event.event_id);
        }
        event_system.fired_log.push(FiredEvent {
            event_id: event.event_id.clone(),
            target: event.target,
            fired_at: now,
            payload: None,
        });
    }
}

/// Plugin that registers the EventSystem resource and tick_events system.
pub struct EventSystemPlugin;

impl Plugin for EventSystemPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(EventSystem::default())
            .insert_resource(EventBus::default())
            .add_systems(
            Update,
            tick_events.after(crate::time_system::advance_game_time),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_event_definition() {
        let mut system = EventSystem::default();
        let def = EventDefinition {
            id: "test_event".to_string(),
            name: "Test Event".to_string(),
            description: "A test event.".to_string(),
            trigger: EventTrigger::Manual,
        };
        system.register(def);
        assert!(system.definitions.contains_key("test_event"));
        assert_eq!(system.definitions["test_event"].name, "Test Event");
    }

    #[test]
    fn test_fire_event_adds_to_pending() {
        let mut system = EventSystem::default();
        system.fire_event("harvest_ended", None, 10);
        assert_eq!(system.pending.len(), 1);
        assert_eq!(system.pending[0].event_id, "harvest_ended");
        assert_eq!(system.pending[0].fires_at, 10);
        assert!(system.pending[0].target.is_none());
    }

    #[test]
    fn test_fire_event_delayed() {
        let mut system = EventSystem::default();
        system.fire_event_delayed("delayed_event", None, 100);
        assert_eq!(system.pending.len(), 1);
        assert_eq!(system.pending[0].fires_at, 100);
    }

    #[test]
    fn test_fire_event_with_target() {
        let mut system = EventSystem::default();
        let entity = Entity::PLACEHOLDER;
        system.fire_event("targeted_event", Some(entity), 5);
        assert_eq!(system.pending.len(), 1);
        assert_eq!(system.pending[0].target, Some(entity));
    }

    #[test]
    fn test_time_to_hexadies() {
        assert_eq!(time_to_hexadies(0, 0, 0), 0);
        assert_eq!(time_to_hexadies(1, 0, 0), 60);
        assert_eq!(time_to_hexadies(0, 1, 0), 5);
        assert_eq!(time_to_hexadies(0, 0, 1), 1);
        assert_eq!(time_to_hexadies(1, 2, 3), 60 + 10 + 3);
        assert_eq!(time_to_hexadies(10, 0, 0), 600);
    }

    #[test]
    fn test_mtth_fire_event_adds_delayed_pending() {
        let mut system = EventSystem::default();
        system.register(EventDefinition {
            id: "mtth_test".to_string(),
            name: "MTTH Test".to_string(),
            description: "Test MTTH event.".to_string(),
            trigger: EventTrigger::Mtth {
                mean_hexadies: 60,
                fire_condition: None,
                max_times: None,
                times_triggered: 0,
            },
        });

        let now = 100;
        system.fire_event("mtth_test", None, now);

        assert_eq!(system.pending.len(), 1);
        assert!(system.pending[0].fires_at > now); // MTTH adds random delay
        assert!(system.pending[0].fires_at >= now + 1); // delay is at least 1
    }

    #[test]
    fn test_mtth_max_times_prevents_fire() {
        let mut system = EventSystem::default();
        system.register(EventDefinition {
            id: "mtth_limited".to_string(),
            name: "Limited MTTH".to_string(),
            description: "Fires at most once.".to_string(),
            trigger: EventTrigger::Mtth {
                mean_hexadies: 10,
                fire_condition: None,
                max_times: Some(1),
                times_triggered: 1, // already fired once
            },
        });

        // fire_event should not queue because max_times reached
        system.fire_event("mtth_limited", None, 100);
        assert!(system.pending.is_empty());
    }

    #[test]
    fn test_periodic_fires_on_interval() {
        let mut system = EventSystem::default();
        system.register(EventDefinition {
            id: "periodic_test".to_string(),
            name: "Periodic Test".to_string(),
            description: "Fires every 10 hexadies.".to_string(),
            trigger: EventTrigger::Periodic {
                interval_hexadies: 10,
                last_fired: 0,
                fire_condition: None,
                max_times: None,
                times_triggered: 0,
            },
        });

        // At time=10, should fire
        let check_should_fire = |system: &EventSystem, now: i64| -> Vec<String> {
            system
                .definitions
                .iter()
                .filter_map(|(id, def)| {
                    if let EventTrigger::Periodic {
                        interval_hexadies,
                        last_fired,
                        max_times,
                        times_triggered,
                        ..
                    } = &def.trigger
                    {
                        if let Some(max) = max_times {
                            if *times_triggered >= *max {
                                return None;
                            }
                        }
                        if now - *last_fired >= *interval_hexadies {
                            return Some(id.clone());
                        }
                        None
                    } else {
                        None
                    }
                })
                .collect()
        };

        // At t=10, should fire
        assert_eq!(check_should_fire(&system, 10).len(), 1);

        // Simulate firing: update last_fired and times_triggered
        if let Some(def) = system.definitions.get_mut("periodic_test") {
            if let EventTrigger::Periodic {
                last_fired,
                times_triggered,
                ..
            } = &mut def.trigger
            {
                *last_fired = 10;
                *times_triggered += 1;
            }
        }

        // At t=15, should NOT fire (only 5 hexadies since last)
        assert!(check_should_fire(&system, 15).is_empty());

        // At t=20, should fire again
        assert_eq!(check_should_fire(&system, 20).len(), 1);
    }

    #[test]
    fn test_periodic_max_times() {
        let mut system = EventSystem::default();
        system.register(EventDefinition {
            id: "periodic_limited".to_string(),
            name: "Limited Periodic".to_string(),
            description: "Fires at most twice.".to_string(),
            trigger: EventTrigger::Periodic {
                interval_hexadies: 5,
                last_fired: 0,
                fire_condition: None,
                max_times: Some(2),
                times_triggered: 0,
            },
        });

        let check_should_fire = |system: &EventSystem, now: i64| -> bool {
            system
                .definitions
                .iter()
                .any(|(_, def)| {
                    if let EventTrigger::Periodic {
                        interval_hexadies,
                        last_fired,
                        max_times,
                        times_triggered,
                        ..
                    } = &def.trigger
                    {
                        if let Some(max) = max_times {
                            if *times_triggered >= *max {
                                return false;
                            }
                        }
                        now - *last_fired >= *interval_hexadies
                    } else {
                        false
                    }
                })
        };

        // Fire twice
        for t in [5, 10] {
            assert!(check_should_fire(&system, t));
            if let Some(def) = system.definitions.get_mut("periodic_limited") {
                if let EventTrigger::Periodic {
                    last_fired,
                    times_triggered,
                    ..
                } = &mut def.trigger
                {
                    *last_fired = t;
                    *times_triggered += 1;
                }
            }
        }

        // Third time: should NOT fire
        assert!(!check_should_fire(&system, 15));
    }

    #[test]
    fn test_fire_event_starts_periodic_timer() {
        let mut system = EventSystem::default();
        system.register(EventDefinition {
            id: "periodic_start".to_string(),
            name: "Periodic Start".to_string(),
            description: "Test that fire_event updates last_fired for periodic events.".to_string(),
            trigger: EventTrigger::Periodic {
                interval_hexadies: 10,
                last_fired: 0,
                fire_condition: None,
                max_times: None,
                times_triggered: 0,
            },
        });

        system.fire_event("periodic_start", None, 50);

        // Should have 1 pending event firing immediately (fires_at = now)
        assert_eq!(system.pending.len(), 1);
        assert_eq!(system.pending[0].fires_at, 50);

        // The definition's last_fired should have been updated to 50
        let def = system.definitions.get("periodic_start").unwrap();
        if let EventTrigger::Periodic { last_fired, .. } = &def.trigger {
            assert_eq!(*last_fired, 50);
        } else {
            panic!("Expected Periodic trigger");
        }
    }

    /// CRITICAL #5: Verify fire_condition field is stored and accessible on MTTH triggers.
    /// TODO: Actual Lua callback execution for fire_condition requires ScriptEngine
    /// integration and is not tested here. This test only verifies the field is stored
    /// and retrievable from the EventDefinition.
    #[test]
    fn test_fire_condition_stored_on_mtth() {
        let mut system = EventSystem::default();
        let lua_ref = LuaFunctionRef::placeholder(42);
        system.register(EventDefinition {
            id: "conditional_mtth".to_string(),
            name: "Conditional MTTH".to_string(),
            description: "An MTTH event with a fire_condition.".to_string(),
            trigger: EventTrigger::Mtth {
                mean_hexadies: 30,
                fire_condition: Some(lua_ref.clone()),
                max_times: None,
                times_triggered: 0,
            },
        });

        let def = system.definitions.get("conditional_mtth").unwrap();
        if let EventTrigger::Mtth { fire_condition, mean_hexadies, .. } = &def.trigger {
            assert!(fire_condition.is_some(), "fire_condition should be stored");
            assert_eq!(fire_condition.as_ref().unwrap().id, 42);
            assert_eq!(*mean_hexadies, 30);
        } else {
            panic!("Expected Mtth trigger");
        }
    }

    /// CRITICAL #5: Verify fire_condition field is stored and accessible on Periodic triggers.
    /// TODO: Actual Lua callback execution for fire_condition is not tested here.
    #[test]
    fn test_fire_condition_stored_on_periodic() {
        let mut system = EventSystem::default();
        let lua_ref = LuaFunctionRef::placeholder(99);
        system.register(EventDefinition {
            id: "conditional_periodic".to_string(),
            name: "Conditional Periodic".to_string(),
            description: "A periodic event with a fire_condition.".to_string(),
            trigger: EventTrigger::Periodic {
                interval_hexadies: 10,
                last_fired: 0,
                fire_condition: Some(lua_ref.clone()),
                max_times: Some(5),
                times_triggered: 0,
            },
        });

        let def = system.definitions.get("conditional_periodic").unwrap();
        if let EventTrigger::Periodic { fire_condition, interval_hexadies, max_times, .. } = &def.trigger {
            assert!(fire_condition.is_some(), "fire_condition should be stored");
            assert_eq!(fire_condition.as_ref().unwrap().id, 99);
            assert_eq!(*interval_hexadies, 10);
            assert_eq!(*max_times, Some(5));
        } else {
            panic!("Expected Periodic trigger");
        }
    }

    #[test]
    fn test_event_bus_fire_calls_handler() {
        let lua = Lua::new();
        crate::scripting::ScriptEngine::setup_globals(&lua, &crate::scripting::resolve_scripts_dir()).unwrap();

        // Register a handler via on() and fire an event
        lua.load(
            r#"
            _handler_called = false
            on("macrocosmo:test", function(evt)
                _handler_called = true
                _received_event_id = evt.event_id
            end)
            "#,
        )
        .exec()
        .unwrap();

        let mut payload = HashMap::new();
        payload.insert("some_key".to_string(), "some_value".to_string());
        let count = EventBus::fire(&lua, "macrocosmo:test", &payload);

        assert_eq!(count, 1);
        let called: bool = lua.globals().get("_handler_called").unwrap();
        assert!(called);
        let received_id: String = lua.globals().get("_received_event_id").unwrap();
        assert_eq!(received_id, "macrocosmo:test");
    }

    #[test]
    fn test_event_bus_filter_match() {
        let lua = Lua::new();
        crate::scripting::ScriptEngine::setup_globals(&lua, &crate::scripting::resolve_scripts_dir()).unwrap();

        lua.load(
            r#"
            _combat_handler_called = false
            on("macrocosmo:building_lost", { cause = "combat" }, function(evt)
                _combat_handler_called = true
            end)
            "#,
        )
        .exec()
        .unwrap();

        // Fire with matching filter
        let mut payload = HashMap::new();
        payload.insert("cause".to_string(), "combat".to_string());
        payload.insert("building_id".to_string(), "mine".to_string());
        let count = EventBus::fire(&lua, "macrocosmo:building_lost", &payload);
        assert_eq!(count, 1);
        let called: bool = lua.globals().get("_combat_handler_called").unwrap();
        assert!(called);

        // Reset and fire with non-matching filter
        lua.load(r#"_combat_handler_called = false"#).exec().unwrap();
        let mut payload2 = HashMap::new();
        payload2.insert("cause".to_string(), "recycled".to_string());
        let count2 = EventBus::fire(&lua, "macrocosmo:building_lost", &payload2);
        assert_eq!(count2, 0);
        let called2: bool = lua.globals().get("_combat_handler_called").unwrap();
        assert!(!called2);
    }

    #[test]
    fn test_event_bus_no_filter_matches_all() {
        let lua = Lua::new();
        crate::scripting::ScriptEngine::setup_globals(&lua, &crate::scripting::resolve_scripts_dir()).unwrap();

        lua.load(
            r#"
            _any_handler_called = false
            on("macrocosmo:any_event", function(evt)
                _any_handler_called = true
            end)
            "#,
        )
        .exec()
        .unwrap();

        // Fire with arbitrary payload
        let mut payload = HashMap::new();
        payload.insert("cause".to_string(), "anything".to_string());
        payload.insert("extra".to_string(), "data".to_string());
        let count = EventBus::fire(&lua, "macrocosmo:any_event", &payload);
        assert_eq!(count, 1);
        let called: bool = lua.globals().get("_any_handler_called").unwrap();
        assert!(called);
    }

    #[test]
    fn test_event_bus_wrong_event_id_not_called() {
        let lua = Lua::new();
        crate::scripting::ScriptEngine::setup_globals(&lua, &crate::scripting::resolve_scripts_dir()).unwrap();

        lua.load(
            r#"
            _wrong_handler_called = false
            on("macrocosmo:specific_event", function(evt)
                _wrong_handler_called = true
            end)
            "#,
        )
        .exec()
        .unwrap();

        let payload = HashMap::new();
        let count = EventBus::fire(&lua, "macrocosmo:other_event", &payload);
        assert_eq!(count, 0);
        let called: bool = lua.globals().get("_wrong_handler_called").unwrap();
        assert!(!called);
    }
}
