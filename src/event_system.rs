use bevy::prelude::*;
use std::collections::HashMap;

use crate::time_system::GameClock;

/// Reference to a Lua function stored in the registry.
/// For now, store the registry key as an integer.
#[derive(Clone, Debug)]
pub struct LuaFunctionRef(pub i64);

/// Defines how an event is triggered.
#[derive(Clone, Debug)]
pub enum EventTrigger {
    /// Fired explicitly by fire_event() or on_expire_event.
    Manual,
    /// Mean Time To Happen -- activated periodically, fires after random delay.
    Mtth {
        mean_hexadies: i64,
        activate_condition: Option<LuaFunctionRef>,
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

/// Resource holding all event definitions and pending events.
#[derive(Resource, Default)]
pub struct EventSystem {
    pub definitions: HashMap<String, EventDefinition>,
    pub pending: Vec<PendingEvent>,
}

impl EventSystem {
    /// Register an event definition.
    pub fn register(&mut self, def: EventDefinition) {
        self.definitions.insert(def.id.clone(), def);
    }

    /// Queue a manual event to fire immediately (next tick).
    pub fn fire_event(&mut self, event_id: &str, target: Option<Entity>, now: i64) {
        self.pending.push(PendingEvent {
            event_id: event_id.to_string(),
            target,
            fires_at: now,
        });
    }

    /// Queue an event to fire after a delay.
    pub fn fire_event_delayed(&mut self, event_id: &str, target: Option<Entity>, fires_at: i64) {
        self.pending.push(PendingEvent {
            event_id: event_id.to_string(),
            target,
            fires_at,
        });
    }
}

/// Bevy system that processes pending events whose fire time has been reached.
/// Lua callback execution will come in a follow-up.
pub fn tick_events(clock: Res<GameClock>, mut event_system: ResMut<EventSystem>) {
    let now = clock.elapsed;

    // Collect events that are ready to fire
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
        if let Some(def) = event_system.definitions.get(&event.event_id) {
            info!("Event fired: {} ({})", def.name, def.id);
        } else {
            info!("Event fired: {} (no definition found)", event.event_id);
        }
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
}
