//! #186 Phase 1 — Aggressive ROE hostile detection.
//!
//! When a ship with [`RulesOfEngagement::Aggressive`] is in a deep-space state
//! ([`ShipState::SubLight`] or [`ShipState::Loitering`]), this system scans for
//! other ships belonging to hostile factions that are also in a deep-space
//! state and within the detection range. For each newly-detected hostile, it
//! fires a [`GameEventKind::HostileDetected`] event (which
//! [`auto_notify_from_events`](crate::notifications::auto_notify_from_events)
//! maps to a high-priority notification banner).
//!
//! Scope note: this is detection only. Interception orbit calculation
//! ([`PursueTarget`] command) and deep-space ship-vs-ship combat are deferred
//! to Phase 2 / Phase 3.
//!
//! ## Detection rules
//!
//! - Self ship must be in `SubLight` or `Loitering` (FTL ships have no sensor
//!   contact in this phase — see #120).
//! - Self ship must be `RulesOfEngagement::Aggressive`.
//! - Self ship must have `Owner::Empire(faction)` (neutrals have no diplomatic
//!   identity; never detect).
//! - Target ship must also be in `SubLight` or `Loitering`.
//! - Target ship's faction (via `Owner::Empire` or `FactionOwner`) must be
//!   hostile to self under [`FactionView::can_attack_aggressive`].
//! - Euclidean distance ≤ [`DEFAULT_DETECTION_RANGE_LY`].
//!
//! ## Duplicate-notification suppression
//!
//! Each detector carries a [`DetectedHostiles`] component (attached lazily on
//! first detection) mapping `target entity → last detected-at hexadies`.
//! A target is suppressed if re-detected within
//! [`DETECTION_COOLDOWN_HEXADIES`] of the last entry. Stale entries are
//! pruned on access.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::faction::{FactionOwner, FactionRelations};
use crate::knowledge::{FactSysParam, FactionVantageQueries, KnowledgeFact};
use crate::physics;
use crate::time_system::GameClock;

use super::{Owner, RulesOfEngagement, Ship, ShipState};

/// Default sensor range for Phase 1 Aggressive detection, in light-years.
/// Intentionally a small constant — module-driven sensor ranges are future
/// work.
pub const DEFAULT_DETECTION_RANGE_LY: f64 = 3.0;

/// Cooldown during which a repeat detection of the *same* target entity does
/// not re-fire its `HostileDetected` event. One hexadies-year keeps the
/// notification stream readable without hiding genuinely new contacts.
pub const DETECTION_COOLDOWN_HEXADIES: i64 = 60;

/// Per-ship record of recently-detected hostile targets, used for de-duplication
/// of `HostileDetected` events under the Aggressive ROE (#186 Phase 1).
///
/// Keys are the target's [`Entity`]. Values are the game-clock hexadies at
/// which the detection was last recorded. Entries older than
/// [`DETECTION_COOLDOWN_HEXADIES`] are pruned lazily.
#[derive(Component, Default, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct DetectedHostiles {
    pub entries: HashMap<Entity, i64>,
}

impl DetectedHostiles {
    /// Returns `true` if `target` was detected within `cooldown` hexadies of
    /// `now`.
    pub fn is_on_cooldown(&self, target: Entity, now: i64, cooldown: i64) -> bool {
        self.entries
            .get(&target)
            .is_some_and(|last| now - *last < cooldown)
    }

    /// Record a detection of `target` at `now`.
    pub fn record(&mut self, target: Entity, now: i64) {
        self.entries.insert(target, now);
    }

    /// Remove entries older than `cooldown` hexadies relative to `now`.
    pub fn prune(&mut self, now: i64, cooldown: i64) {
        self.entries.retain(|_, last| now - *last < cooldown);
    }
}

/// Compute a ship's current world-space position as a `[f64; 3]`, given its
/// state. Mirrors the logic in [`crate::knowledge::propagate_knowledge`] so
/// detection operates on the ship's *actual* location, not the origin/system.
/// Returns `None` for states that are not currently in realspace (e.g. FTL).
fn ship_position(
    state: &ShipState,
    positions: &Query<&crate::components::Position>,
    now: i64,
) -> Option<[f64; 3]> {
    match state {
        ShipState::InSystem { system } => positions.get(*system).ok().map(|p| p.as_array()),
        ShipState::Surveying { target_system, .. } => {
            positions.get(*target_system).ok().map(|p| p.as_array())
        }
        ShipState::Settling { system, .. } => positions.get(*system).ok().map(|p| p.as_array()),
        ShipState::Refitting { system, .. } => positions.get(*system).ok().map(|p| p.as_array()),
        // FTL ships are not detectable in Phase 1 (see #120). Returning `None`
        // causes them to be skipped both as detector and target.
        ShipState::InFTL { .. } => None,
        ShipState::SubLight {
            origin,
            destination,
            departed_at,
            arrival_at,
            ..
        } => {
            let total = (*arrival_at - *departed_at) as f64;
            let elapsed = (now - *departed_at) as f64;
            let t = if total > 0.0 {
                (elapsed / total).clamp(0.0, 1.0)
            } else {
                1.0
            };
            Some([
                origin[0] + (destination[0] - origin[0]) * t,
                origin[1] + (destination[1] - origin[1]) * t,
                origin[2] + (destination[2] - origin[2]) * t,
            ])
        }
        ShipState::Loitering { position } => Some(*position),
        // #217: Scouting ships orbit the target system.
        ShipState::Scouting { target_system, .. } => {
            positions.get(*target_system).ok().map(|p| p.as_array())
        }
    }
}

/// Whether a ship state is considered "deep-space detectable" for Phase 1.
/// Only `SubLight` and `Loitering` count — docked/surveying ships are handled
/// by the in-system combat path (`resolve_combat`), and FTL ships are beyond
/// baseline sensors.
fn is_deep_space(state: &ShipState) -> bool {
    matches!(
        state,
        ShipState::SubLight { .. } | ShipState::Loitering { .. }
    )
}

/// Resolve a ship's faction entity. Ship owners store this as
/// `Owner::Empire(faction_entity)` (see combat.rs §168); non-empire ships
/// (pirates, space creatures) carry it via a `FactionOwner` component.
/// Returns `None` for wholly unaffiliated ships (`Owner::Neutral` + no
/// `FactionOwner`).
fn resolve_ship_faction(owner: &Owner, faction_owner: Option<&FactionOwner>) -> Option<Entity> {
    match owner {
        Owner::Empire(e) => Some(*e),
        Owner::Neutral => faction_owner.map(|f| f.0),
    }
}

/// #186 Phase 1 — Scan for hostile contacts and fire `HostileDetected` events.
///
/// Runs every tick. Aggressive ships in deep-space scan every other ship in
/// deep-space; pairs whose factions satisfy `can_attack_aggressive` and are
/// within [`DEFAULT_DETECTION_RANGE_LY`] produce an event (+ notification via
/// `auto_notify_from_events`). Duplicate events within
/// [`DETECTION_COOLDOWN_HEXADIES`] are suppressed per detector via
/// [`DetectedHostiles`].
pub fn detect_hostiles_system(
    mut commands: Commands,
    clock: Res<GameClock>,
    relations: Res<FactionRelations>,
    positions: Query<&Position>,
    ships: Query<(
        Entity,
        &Ship,
        &ShipState,
        Option<&RulesOfEngagement>,
        Option<&FactionOwner>,
    )>,
    mut detected: Query<&mut DetectedHostiles>,
    mut events: MessageWriter<GameEvent>,
    mut fact_sys: FactSysParam,
    // Round 9 PR #1 Step 3: per-faction routing.
    vantage_q: FactionVantageQueries,
) {
    let now = clock.elapsed;
    let vantages = vantage_q.collect();

    // Snapshot every ship's (position, faction, name, deep-space) once so we
    // can cross-reference them in O(n²) without re-borrowing the query.
    struct Snapshot {
        entity: Entity,
        faction: Option<Entity>,
        position: [f64; 3],
        name: String,
    }
    let snapshots: Vec<Snapshot> = ships
        .iter()
        .filter_map(|(entity, ship, state, _roe, fowner)| {
            if !is_deep_space(state) {
                return None;
            }
            let position = ship_position(state, &positions, now)?;
            let faction = resolve_ship_faction(&ship.owner, fowner);
            Some(Snapshot {
                entity,
                faction,
                position,
                name: ship.name.clone(),
            })
        })
        .collect();

    if snapshots.len() < 2 {
        return;
    }

    // For each detector (Aggressive + has faction + deep-space) scan the rest.
    // We collect the pending detections first, then mutate DetectedHostiles in
    // a second pass to avoid holding both a read and a write borrow on the
    // ships/detected queries simultaneously.
    struct Detection {
        detector: Entity,
        detector_name: String,
        target: Entity,
        target_name: String,
        target_pos: [f64; 3],
    }
    let mut pending: Vec<Detection> = Vec::new();

    for (detector_entity, ship, state, roe, fowner) in ships.iter() {
        let roe = roe.copied().unwrap_or_default();
        if roe != RulesOfEngagement::Aggressive {
            continue;
        }
        if !is_deep_space(state) {
            continue;
        }
        // #296 (S-3): Immobile ships (Infrastructure Cores) can never
        // intercept a hostile target, so they are excluded from the
        // detector loop entirely. Defence-in-depth: a Core always sits in
        // ShipState::InSystem and would already be filtered by is_deep_space,
        // but if a future feature ever loiters one in deep space the
        // pursuit pipeline must remain coherent.
        if ship.is_immobile() {
            continue;
        }
        let Some(detector_faction) = resolve_ship_faction(&ship.owner, fowner) else {
            continue;
        };
        let Some(detector_pos) = ship_position(state, &positions, now) else {
            continue;
        };

        for target in &snapshots {
            if target.entity == detector_entity {
                continue;
            }
            let Some(target_faction) = target.faction else {
                continue;
            };
            if target_faction == detector_faction {
                continue;
            }
            // Hostile gate: must be allowed to attack under Aggressive ROE.
            if !relations
                .get_or_default(detector_faction, target_faction)
                .can_attack_aggressive()
            {
                continue;
            }
            let dist = physics::distance_ly_arr(detector_pos, target.position);
            if dist > DEFAULT_DETECTION_RANGE_LY {
                continue;
            }
            pending.push(Detection {
                detector: detector_entity,
                detector_name: ship.name.clone(),
                target: target.entity,
                target_name: target.name.clone(),
                target_pos: target.position,
            });
        }
    }

    for det in pending {
        // Look up (or ensure) the DetectedHostiles component on the detector,
        // checking cooldown before firing the event + notification.
        let should_notify = if let Ok(mut dh) = detected.get_mut(det.detector) {
            dh.prune(now, DETECTION_COOLDOWN_HEXADIES);
            if dh.is_on_cooldown(det.target, now, DETECTION_COOLDOWN_HEXADIES) {
                false
            } else {
                dh.record(det.target, now);
                true
            }
        } else {
            // First detection for this ship — create the component with the
            // current entry. We use try_insert so concurrent despawns don't
            // panic.
            let mut dh = DetectedHostiles::default();
            dh.record(det.target, now);
            commands.entity(det.detector).try_insert(dh);
            true
        };

        if should_notify {
            let description = format!(
                "{} detected hostile {} at ({:.2}, {:.2}, {:.2})",
                det.detector_name,
                det.target_name,
                det.target_pos[0],
                det.target_pos[1],
                det.target_pos[2],
            );
            // #249 + Round 9 Step 3: shared EventId between the legacy
            // GameEvent and the paired KnowledgeFact (NotifiedEventIds
            // dedupe), routed through `record_for` so every empire sees
            // it (light-delayed per their vantage). FactSysParam handles
            // both the EventId allocation/registration and the relay /
            // light-speed math.
            let event_id = fact_sys.allocate_event_id();
            // EventLog + auto_pause still receive the raw event (the notification
            // path is the one gaining light-speed delay).
            events.write(GameEvent {
                id: event_id,
                timestamp: now,
                kind: GameEventKind::HostileDetected,
                description: description.clone(),
                related_system: None,
            });

            // #233: Routing through `record_for` respects the
            // light-speed contract (50 ly detections used to alert the
            // player instantly) and also enables relay-accelerated
            // propagation when coverage exists.
            let fact = KnowledgeFact::HostileDetected {
                event_id: Some(event_id),
                target: det.target,
                detector: det.detector,
                target_pos: det.target_pos,
                description,
            };
            fact_sys.record_for(fact, &vantages, det.target_pos, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;

    #[test]
    fn detected_hostiles_cooldown_and_prune() {
        let mut world = World::new();
        let target = world.spawn_empty().id();
        let mut dh = DetectedHostiles::default();
        assert!(!dh.is_on_cooldown(target, 0, 60));
        dh.record(target, 100);
        assert!(dh.is_on_cooldown(target, 100, 60));
        assert!(dh.is_on_cooldown(target, 159, 60));
        assert!(!dh.is_on_cooldown(target, 160, 60));

        // Pruning removes stale entries.
        dh.prune(200, 60);
        assert!(dh.entries.is_empty());
    }

    #[test]
    fn is_deep_space_classification() {
        let mut world = World::new();
        let sys = world.spawn_empty().id();
        assert!(!is_deep_space(&ShipState::InSystem { system: sys }));
        assert!(!is_deep_space(&ShipState::InFTL {
            origin_system: sys,
            destination_system: sys,
            departed_at: 0,
            arrival_at: 100,
        }));
        assert!(!is_deep_space(&ShipState::Surveying {
            target_system: sys,
            started_at: 0,
            completes_at: 100,
        }));
        assert!(is_deep_space(&ShipState::SubLight {
            origin: [0.0; 3],
            destination: [1.0, 0.0, 0.0],
            target_system: None,
            departed_at: 0,
            arrival_at: 60,
        }));
        assert!(is_deep_space(&ShipState::Loitering {
            position: [2.0, 0.0, 0.0],
        }));
    }

    #[test]
    fn resolve_ship_faction_prefers_empire_then_faction_owner() {
        let mut world = World::new();
        let empire = world.spawn_empty().id();
        let other_faction = world.spawn_empty().id();
        let fowner = FactionOwner(other_faction);

        // Empire variant always wins.
        assert_eq!(
            resolve_ship_faction(&Owner::Empire(empire), Some(&fowner)),
            Some(empire)
        );
        // Neutral + FactionOwner → FactionOwner.
        assert_eq!(
            resolve_ship_faction(&Owner::Neutral, Some(&fowner)),
            Some(other_faction)
        );
        // Neutral + no FactionOwner → None.
        assert_eq!(resolve_ship_faction(&Owner::Neutral, None), None);
    }
}
