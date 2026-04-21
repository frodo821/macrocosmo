//! #215: Read-side facade over [`KnowledgeStore`] that exposes observation
//! freshness (`last_updated`, `age`, `is_stale`) and applies the [`Stale`]
//! overlay on entries older than [`STALE_THRESHOLD_HEXADIES`].
//!
//! Producers continue to write `Direct / Relay / Scout` sources into the
//! store; [`perceived_system`] / [`perceived_fleet`] rewrite the source to
//! [`ObservationSource::Stale`] on read when the entry is too old. The
//! underlying stored entry is not mutated.
//!
//! [`Stale`]: ObservationSource::Stale

use bevy::prelude::Entity;

use super::{
    KnowledgeStore, ObservationSource, STALE_THRESHOLD_HEXADIES, ShipSnapshot, SystemSnapshot,
};

/// Faction identifier alias. Promoted to a newtype once #163 lands.
pub type FactionId = Entity;

/// A snapshot of observed information with freshness metadata.
///
/// `last_updated` is the spec-facing name for the underlying `observed_at`
/// field — same integer-hexadies timebase, different alias.
#[derive(Clone, Debug)]
pub struct PerceivedInfo<T> {
    pub value: T,
    /// Game time (hexadies) at which the observation was made. Equivalent to
    /// the underlying `observed_at` on [`SystemKnowledge`] / [`ShipSnapshot`].
    pub last_updated: i64,
    pub source: ObservationSource,
}

#[allow(dead_code)] // Consumed by integration tests + future UI/AI code (#216/#217).
impl<T> PerceivedInfo<T> {
    /// Age of the observation relative to `now` (hexadies).
    pub fn age(&self, now: i64) -> i64 {
        now - self.last_updated
    }

    /// Whether this observation is considered stale. Returns `true` if the
    /// source was already tagged stale (defensive) or if the age has reached
    /// [`STALE_THRESHOLD_HEXADIES`].
    pub fn is_stale(&self, now: i64) -> bool {
        self.source == ObservationSource::Stale || self.age(now) >= STALE_THRESHOLD_HEXADIES
    }
}

/// Read a system observation from the knowledge store, overlaying the source
/// to [`ObservationSource::Stale`] when the entry exceeds the freshness
/// threshold.
pub fn perceived_system(
    store: &KnowledgeStore,
    system: Entity,
    now: i64,
) -> Option<PerceivedInfo<SystemSnapshot>> {
    let entry = store.get(system)?;
    let age = now - entry.observed_at;
    let source = if age >= STALE_THRESHOLD_HEXADIES {
        ObservationSource::Stale
    } else {
        entry.source
    };
    Some(PerceivedInfo {
        value: entry.data.clone(),
        last_updated: entry.observed_at,
        source,
    })
}

/// Return all ship observations in the store, with the staleness overlay
/// applied per-entry.
///
/// TODO(#163): filter by `_target` faction once `ShipSnapshot` carries owner
/// information. Currently returns every ship snapshot regardless of faction.
#[allow(dead_code)] // Consumed by integration tests + future UI/AI code (#216/#217).
pub fn perceived_fleet(
    store: &KnowledgeStore,
    _target: FactionId,
    now: i64,
) -> Vec<PerceivedInfo<ShipSnapshot>> {
    store
        .iter_ships()
        .map(|(_, snap)| {
            let age = now - snap.observed_at;
            let source = if age >= STALE_THRESHOLD_HEXADIES {
                ObservationSource::Stale
            } else {
                snap.source
            };
            PerceivedInfo {
                value: snap.clone(),
                last_updated: snap.observed_at,
                source,
            }
        })
        .collect()
}
