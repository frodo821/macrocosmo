//! #291: Fleet system transit events — `macrocosmo:fleet_system_entered`
//! and `macrocosmo:fleet_system_left`.
//!
//! This module provides:
//! - [`LastDockedSystem`] — per-ship tracking component recording the most
//!   recent star system the ship was in. Used to detect departures from
//!   star systems (vs departures from deep-space Loitering).
//! - [`FleetTransitCtx`] — typed [`EventContext`] payload for Lua handlers.
//! - Helper function [`fire_fleet_transit`] called from movement systems
//!   and departure callsites.
//!
//! # Design
//!
//! **Arrivals** are fired directly from `process_ftl_travel` and
//! `sublight_movement_system` at the point of state transition. The mode
//! (FTL/sublight) is known at the callsite.
//!
//! **Departures** are fired from `detect_fleet_departures`, a system that
//! runs after movement and command processing. It uses `Changed<ShipState>`
//! to find ships that transitioned to `InFTL` or `SubLight` and checks
//! `LastDockedSystem` to determine if they left a star system (vs deep-space).
//!
//! Events are enqueued on [`EventSystem`] via `fire_event_with_payload`
//! (queue-only, never sync-dispatched).

use std::borrow::Cow;

use bevy::prelude::*;
use mlua::prelude::*;

use crate::event_system::{
    EventContext, EventSystem, FLEET_SYSTEM_ENTERED_EVENT, FLEET_SYSTEM_LEFT_EVENT,
};
use crate::time_system::GameClock;

use super::{Ship, ShipState};

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// Tracks the most recent star system a ship was in (`InSystem`).
/// Initialised when a ship spawns in `InSystem { system }` and updated
/// whenever the ship transitions into `InSystem`. Used by
/// [`detect_fleet_departures`] to determine whether a departure from
/// a star system occurred (as opposed to departing from deep-space
/// Loitering, which should NOT fire `fleet_system_left`).
#[derive(Component, Debug, Clone, Copy)]
pub struct LastDockedSystem(pub Option<Entity>);

// ---------------------------------------------------------------------------
// FleetTransitCtx — typed EventContext payload
// ---------------------------------------------------------------------------

/// Travel mode discriminator for fleet transit events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitMode {
    Ftl,
    Sublight,
}

impl TransitMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            TransitMode::Ftl => "ftl",
            TransitMode::Sublight => "sublight",
        }
    }
}

/// Direction discriminator for fleet transit events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitDirection {
    Entered,
    Left,
}

/// Typed [`EventContext`] payload for `macrocosmo:fleet_system_entered`
/// and `macrocosmo:fleet_system_left`.
///
/// Carries entity ids for `system` and `fleet`; the full SystemView /
/// FleetView Lua tables are built by `enrich_fleet_transit_payload`
/// in `dispatch_event_handlers` before handlers are invoked.
#[derive(Debug, Clone)]
pub struct FleetTransitCtx {
    pub direction: TransitDirection,
    pub date: i64,
    pub mode: TransitMode,
    pub system_entity: Entity,
    pub fleet_entity: Entity,
}

impl EventContext for FleetTransitCtx {
    fn event_id(&self) -> &str {
        match self.direction {
            TransitDirection::Entered => FLEET_SYSTEM_ENTERED_EVENT,
            TransitDirection::Left => FLEET_SYSTEM_LEFT_EVENT,
        }
    }

    fn to_lua_table(&self, lua: &Lua) -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        t.set("event_id", self.event_id())?;
        t.set("date", self.date)?;
        t.set("mode", self.mode.as_str())?;
        t.set("system_entity", self.system_entity.to_bits())?;
        t.set("fleet_entity", self.fleet_entity.to_bits())?;
        Ok(t)
    }

    fn payload_get(&self, key: &str) -> Option<Cow<'_, str>> {
        match key {
            "date" => Some(Cow::Owned(self.date.to_string())),
            "mode" => Some(Cow::Borrowed(self.mode.as_str())),
            "system_entity" => Some(Cow::Owned(self.system_entity.to_bits().to_string())),
            "fleet_entity" => Some(Cow::Owned(self.fleet_entity.to_bits().to_string())),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Helper — fire a fleet transit event
// ---------------------------------------------------------------------------

/// Fire a fleet transit event on the EventSystem. Called from movement
/// systems (for arrivals) and from [`detect_fleet_departures`] (for
/// departures).
pub fn fire_fleet_transit(
    event_system: &mut EventSystem,
    direction: TransitDirection,
    date: i64,
    mode: TransitMode,
    system_entity: Entity,
    fleet_entity: Entity,
) {
    event_system.fire_event_with_payload(
        Some(system_entity),
        date,
        Box::new(FleetTransitCtx {
            direction,
            date,
            mode,
            system_entity,
            fleet_entity,
        }),
    );
}

// ---------------------------------------------------------------------------
// detect_fleet_departures — Bevy system
// ---------------------------------------------------------------------------

/// Detects fleet departures from star systems and fires
/// `macrocosmo:fleet_system_left`.
///
/// Runs after movement systems and command handlers. Uses
/// `Changed<ShipState>` to find ships that transitioned to `InFTL` or
/// `SubLight`. If `LastDockedSystem` is `Some(system)`, the ship just
/// departed that system. Clears `LastDockedSystem` afterwards.
///
/// Also updates `LastDockedSystem` when a ship transitions to `InSystem`
/// through non-travel means (e.g. Surveying/Settling → InSystem).
pub fn detect_fleet_departures(
    clock: Res<GameClock>,
    mut event_system: ResMut<EventSystem>,
    mut query: Query<(&Ship, &ShipState, &mut LastDockedSystem), Changed<ShipState>>,
) {
    for (ship, state, mut last_docked) in query.iter_mut() {
        let fleet_entity = match ship.fleet {
            Some(f) => f,
            None => continue,
        };

        match state {
            ShipState::InFTL { origin_system, .. } => {
                if last_docked.0.is_some() {
                    fire_fleet_transit(
                        &mut event_system,
                        TransitDirection::Left,
                        clock.elapsed,
                        TransitMode::Ftl,
                        *origin_system,
                        fleet_entity,
                    );
                    last_docked.0 = None;
                }
            }
            ShipState::SubLight { .. } => {
                if let Some(system) = last_docked.0 {
                    fire_fleet_transit(
                        &mut event_system,
                        TransitDirection::Left,
                        clock.elapsed,
                        TransitMode::Sublight,
                        system,
                        fleet_entity,
                    );
                    last_docked.0 = None;
                }
            }
            // Non-travel arrival (Surveying → InSystem, etc.): update
            // tracking but don't fire transit event.
            ShipState::InSystem { system } => {
                last_docked.0 = Some(*system);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transit_ctx_event_id_matches_direction() {
        let entered = FleetTransitCtx {
            direction: TransitDirection::Entered,
            date: 42,
            mode: TransitMode::Ftl,
            system_entity: Entity::from_raw_u32(1).unwrap(),
            fleet_entity: Entity::from_raw_u32(2).unwrap(),
        };
        assert_eq!(entered.event_id(), FLEET_SYSTEM_ENTERED_EVENT);

        let left = FleetTransitCtx {
            direction: TransitDirection::Left,
            date: 42,
            mode: TransitMode::Sublight,
            system_entity: Entity::from_raw_u32(1).unwrap(),
            fleet_entity: Entity::from_raw_u32(2).unwrap(),
        };
        assert_eq!(left.event_id(), FLEET_SYSTEM_LEFT_EVENT);
    }

    #[test]
    fn transit_ctx_payload_get() {
        let ctx = FleetTransitCtx {
            direction: TransitDirection::Entered,
            date: 100,
            mode: TransitMode::Ftl,
            system_entity: Entity::from_raw_u32(5).unwrap(),
            fleet_entity: Entity::from_raw_u32(10).unwrap(),
        };
        assert_eq!(ctx.payload_get("date").as_deref(), Some("100"));
        assert_eq!(ctx.payload_get("mode").as_deref(), Some("ftl"));
        assert!(ctx.payload_get("nonexistent").is_none());
    }

    #[test]
    fn transit_mode_str() {
        assert_eq!(TransitMode::Ftl.as_str(), "ftl");
        assert_eq!(TransitMode::Sublight.as_str(), "sublight");
    }
}
