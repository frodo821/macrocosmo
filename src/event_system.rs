use bevy::prelude::*;
use rand::Rng;
use std::collections::HashMap;

use crate::time_system::{GameClock, HEXADIES_PER_MONTH, HEXADIES_PER_YEAR};

/// Reference to a Lua function stored in the registry.
/// For now, store the registry key as an integer.
#[derive(Clone, Debug)]
pub struct LuaFunctionRef(pub i64);

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

    /// Queue an event to fire at a specific time (bypasses trigger logic).
    pub fn fire_event_delayed(&mut self, event_id: &str, target: Option<Entity>, fires_at: i64) {
        self.pending.push(PendingEvent {
            event_id: event_id.to_string(),
            target,
            fires_at,
        });
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
        });
    }
}

/// Plugin that registers the EventSystem resource and tick_events system.
pub struct EventSystemPlugin;

impl Plugin for EventSystemPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(EventSystem::default()).add_systems(
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
}
