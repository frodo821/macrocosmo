//! Type conversion helpers between macrocosmo (Bevy) and macrocosmo-ai.
//!
//! The AI core uses opaque numeric newtypes (`FactionId(u32)`,
//! `SystemRef(u64)`, `EntityRef(u64)`) to avoid depending on Bevy. The
//! helpers here translate Bevy `Entity` and engine-owned types into those
//! opaque ids and back again.
//!
//! ## Round-trip properties
//!
//! - `Entity ↔ SystemRef/EntityRef` is lossless: we use
//!   `Entity::to_bits()` / `Entity::from_bits()` which preserves both the
//!   index and the generation.
//! - `Entity → FactionId` is **lossy**: we take only `Entity::index()`.
//!   Faction entities are long-lived (they live for the entire session
//!   and are not despawned/respawned), so the generation bits are
//!   unnecessary and dropping them yields a stable `u32` suitable for
//!   `FactionId`. There is no `from_ai_faction` because the reverse
//!   mapping is not well-defined in general.

use bevy::prelude::Entity;
use macrocosmo_ai::{EntityRef, FactionId, FactionRef, SystemRef, Tick};

use crate::time_system::GameClock;

/// Derive an AI-side [`FactionId`] from a Bevy [`Entity`].
///
/// Uses `Entity::index()` (the `u32` part). Faction entities are
/// session-stable — no despawn/respawn — so the dropped generation bits
/// do not cause collisions within a run.
pub fn to_ai_faction(entity: Entity) -> FactionId {
    // Bevy 0.18's `Entity::index()` returns an `EntityIndex` newtype; call
    // its own `.index()` to retrieve the underlying `u32`.
    FactionId(entity.index().index())
}

/// `FactionRef::Me` — the observing faction in the current context.
pub fn to_ai_faction_ref_me() -> FactionRef {
    FactionRef::Me
}

/// `FactionRef::Other(id)` derived from a Bevy [`Entity`].
pub fn to_ai_faction_ref_other(entity: Entity) -> FactionRef {
    FactionRef::Other(to_ai_faction(entity))
}

/// Convert a star-system [`Entity`] to an AI-side [`SystemRef`]. Lossless.
pub fn to_ai_system(entity: Entity) -> SystemRef {
    SystemRef(entity.to_bits())
}

/// Reverse of [`to_ai_system`]. Lossless as long as the `SystemRef` was
/// produced by [`to_ai_system`].
pub fn from_ai_system(r: SystemRef) -> Entity {
    Entity::from_bits(r.0)
}

/// Convert any game [`Entity`] (ship, colony, structure, …) to an
/// AI-side [`EntityRef`]. Lossless.
pub fn to_ai_entity(entity: Entity) -> EntityRef {
    EntityRef(entity.to_bits())
}

/// Reverse of [`to_ai_entity`]. Lossless as long as the `EntityRef` was
/// produced by [`to_ai_entity`].
pub fn from_ai_entity(r: EntityRef) -> Entity {
    Entity::from_bits(r.0)
}

/// Read the current tick (hexadies) from the [`GameClock`]. The AI crate
/// treats [`Tick`] as an opaque monotonic `i64`; macrocosmo's unit is
/// hexadies.
pub fn tick_from_clock(clock: &GameClock) -> Tick {
    clock.elapsed
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;

    #[test]
    fn entity_roundtrips_via_system_ref() {
        let mut w = World::new();
        let e = w.spawn_empty().id();
        let r = to_ai_system(e);
        assert_eq!(from_ai_system(r), e);
    }

    #[test]
    fn entity_roundtrips_via_entity_ref() {
        let mut w = World::new();
        let e = w.spawn_empty().id();
        let r = to_ai_entity(e);
        assert_eq!(from_ai_entity(r), e);
    }

    #[test]
    fn faction_id_is_entity_index() {
        let mut w = World::new();
        let e = w.spawn_empty().id();
        assert_eq!(to_ai_faction(e), FactionId(e.index().index()));
    }

    #[test]
    fn faction_ref_me_and_other() {
        let mut w = World::new();
        let e = w.spawn_empty().id();
        assert_eq!(to_ai_faction_ref_me(), FactionRef::Me);
        assert_eq!(
            to_ai_faction_ref_other(e),
            FactionRef::Other(FactionId(e.index().index()))
        );
    }

    #[test]
    fn tick_from_clock_matches_elapsed() {
        let clock = GameClock::new(42);
        assert_eq!(tick_from_clock(&clock), 42);
    }
}
