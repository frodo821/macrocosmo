//! #233 — `PerceivedFact` / `KnowledgeFact` pipeline.
//!
//! This is the "notification-producing delta" side of the knowledge system.
//! The existing [`KnowledgeStore`](super::KnowledgeStore) holds a *snapshot*
//! (latest known state per system / ship); this module tracks *events* — one
//! per discrete observable happening — so the notification UI can render a
//! single banner per event (rather than having to diff the snapshot store).
//!
//! Facts travel through [`PendingFactQueue`] with an `arrives_at` timestamp
//! computed from light-speed propagation + optional FTL Comm Relay shortcut.
//! The `notify_from_knowledge_facts` system drains facts whose `arrives_at`
//! is <= `clock.elapsed` and pushes them into the notification queue.
//!
//! See `src/empire/comms.rs` for the [`CommsParams`] component that carries
//! the `empire_relay_inv_latency` / `empire_relay_range` modifiers consumed
//! by the helpers in this module.
//!
//! Several types here are unused by the main `macrocosmo` binary today —
//! they are the consumer surface exposed to (a) the integration tests that
//! exercise the arrival-time math and (b) future callsites that will be
//! wired in follow-up PRs (scout ships, ship-carried fact pipeline, etc.).
//! `#[allow(dead_code)]` is applied to the module to silence the binary-only
//! unused warnings without suppressing genuine dead code elsewhere.

#![allow(dead_code)]

use bevy::prelude::*;

use crate::components::Position;
use crate::deep_space::{
    CapabilityParams, ConstructionPlatform, DeepSpaceStructure, DeliverableRegistry, FTLCommRelay,
    Scrapyard,
};
use crate::empire::comms::CommsParams;
use crate::physics;

use super::ObservationSource;

/// Base FTL multiplier for relay-routed propagation. `relay_delay` at base
/// evaluates to `light_delay / 10`. `empire_relay_inv_latency` modifiers stack
/// additively on top of this base.
pub const FTL_RELAY_BASE_MULTIPLIER: f64 = 10.0;

/// Combat victor designator for [`KnowledgeFact::CombatOutcome`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CombatVictor {
    /// Player-side victory.
    Player,
    /// Hostile-side victory.
    Hostile,
}

/// An observable event that can produce a player-facing notification.
///
/// Facts are *events*, not snapshots. Each fact carries enough context to
/// render a single banner (title + description + priority) without needing
/// to cross-reference the snapshot store.
#[derive(Clone, Debug)]
pub enum KnowledgeFact {
    /// A hostile contact was detected in deep space (#186 pursuit).
    HostileDetected {
        target: Entity,
        detector: Entity,
        target_pos: [f64; 3],
        description: String,
    },
    /// Combat completed at a star system.
    CombatOutcome {
        system: Entity,
        victor: CombatVictor,
        detail: String,
    },
    /// A star system was fully surveyed.
    SurveyComplete {
        system: Entity,
        system_name: String,
        detail: String,
    },
    /// An anomaly was discovered during a survey.
    AnomalyDiscovered {
        system: Entity,
        anomaly_id: String,
        detail: String,
    },
    /// Non-anomaly survey discovery (legacy exploration event).
    SurveyDiscovery {
        system: Entity,
        detail: String,
    },
    /// A ship / structure was built or destroyed.
    StructureBuilt {
        system: Option<Entity>,
        kind: String,
        name: String,
        destroyed: bool,
        detail: String,
    },
    /// A colony was founded at a planet.
    ColonyEstablished {
        system: Entity,
        planet: Entity,
        name: String,
        detail: String,
    },
    /// A colony attempt failed.
    ColonyFailed {
        system: Entity,
        name: String,
        reason: String,
    },
    /// A ship arrived at a system (routine — usually Low priority).
    ShipArrived {
        system: Option<Entity>,
        name: String,
        detail: String,
    },
}

impl KnowledgeFact {
    /// Short banner title for this fact.
    pub fn title(&self) -> &'static str {
        match self {
            KnowledgeFact::HostileDetected { .. } => "Hostile Detected",
            KnowledgeFact::CombatOutcome { victor, .. } => match victor {
                CombatVictor::Player => "Combat Victory",
                CombatVictor::Hostile => "Combat Defeat",
            },
            KnowledgeFact::SurveyComplete { .. } => "Survey Complete",
            KnowledgeFact::AnomalyDiscovered { .. } => "Anomaly Discovered",
            KnowledgeFact::SurveyDiscovery { .. } => "Discovery",
            KnowledgeFact::StructureBuilt { destroyed, .. } => {
                if *destroyed { "Structure Destroyed" } else { "Structure Built" }
            }
            KnowledgeFact::ColonyEstablished { .. } => "Colony Established",
            KnowledgeFact::ColonyFailed { .. } => "Colony Failed",
            KnowledgeFact::ShipArrived { .. } => "Ship Arrived",
        }
    }

    /// Free-form banner description for this fact.
    pub fn description(&self) -> String {
        match self {
            KnowledgeFact::HostileDetected { description, .. } => description.clone(),
            KnowledgeFact::CombatOutcome { detail, .. } => detail.clone(),
            KnowledgeFact::SurveyComplete { detail, .. } => detail.clone(),
            KnowledgeFact::AnomalyDiscovered { detail, .. } => detail.clone(),
            KnowledgeFact::SurveyDiscovery { detail, .. } => detail.clone(),
            KnowledgeFact::StructureBuilt { detail, .. } => detail.clone(),
            KnowledgeFact::ColonyEstablished { detail, .. } => detail.clone(),
            KnowledgeFact::ColonyFailed { reason, name, .. } => {
                format!("Colony '{}' failed: {}", name, reason)
            }
            KnowledgeFact::ShipArrived { detail, .. } => detail.clone(),
        }
    }

    /// Default notification priority for this fact kind.
    pub fn priority(&self) -> crate::notifications::NotificationPriority {
        use crate::notifications::NotificationPriority::*;
        match self {
            KnowledgeFact::HostileDetected { .. } => High,
            KnowledgeFact::CombatOutcome { .. } => High,
            KnowledgeFact::SurveyComplete { .. } => Medium,
            KnowledgeFact::AnomalyDiscovered { .. } => High,
            KnowledgeFact::SurveyDiscovery { .. } => High,
            KnowledgeFact::StructureBuilt { .. } => Low,
            KnowledgeFact::ColonyEstablished { .. } => High,
            KnowledgeFact::ColonyFailed { .. } => High,
            KnowledgeFact::ShipArrived { .. } => Low,
        }
    }

    /// Associated star system (for notification jump-to-system).
    pub fn related_system(&self) -> Option<Entity> {
        match self {
            KnowledgeFact::HostileDetected { .. } => None,
            KnowledgeFact::CombatOutcome { system, .. } => Some(*system),
            KnowledgeFact::SurveyComplete { system, .. } => Some(*system),
            KnowledgeFact::AnomalyDiscovered { system, .. } => Some(*system),
            KnowledgeFact::SurveyDiscovery { system, .. } => Some(*system),
            KnowledgeFact::StructureBuilt { system, .. } => *system,
            KnowledgeFact::ColonyEstablished { system, .. } => Some(*system),
            KnowledgeFact::ColonyFailed { system, .. } => Some(*system),
            KnowledgeFact::ShipArrived { system, .. } => *system,
        }
    }
}

/// A [`KnowledgeFact`] plus the timing + provenance metadata the arrival
/// scheduler needs.
#[derive(Clone, Debug)]
pub struct PerceivedFact {
    pub fact: KnowledgeFact,
    /// Hexadies at which the event actually happened at its origin.
    pub observed_at: i64,
    /// Hexadies at which the notification should surface to the player.
    pub arrives_at: i64,
    /// Which channel produced this observation (Direct / Relay / Scout).
    pub source: ObservationSource,
    /// World-space origin of the event (for future directionality / UI).
    pub origin_pos: [f64; 3],
    /// Optional star system reference (duplicates `fact.related_system()` for
    /// convenience — callers sometimes have the entity but not the fact yet).
    pub related_system: Option<Entity>,
}

/// Resource holding facts waiting for their light-speed / relay arrival time.
///
/// Parallel to (not merged with) [`KnowledgeStore`](super::KnowledgeStore).
/// Responsibility split:
///   - `KnowledgeStore` → "what is the world like right now, from the
///     player's vantage point" (snapshot).
///   - `PendingFactQueue` → "what *happened* that the player will hear about
///     at time T" (delta).
#[derive(Resource, Default)]
pub struct PendingFactQueue {
    pub facts: Vec<PerceivedFact>,
}

impl PendingFactQueue {
    /// Record a new fact. Does not check timing — the scheduler will sort out
    /// arrival ordering on the next `drain_ready` call.
    pub fn record(&mut self, fact: PerceivedFact) {
        self.facts.push(fact);
    }

    /// Drain all facts whose `arrives_at <= now`, returning them in insertion
    /// order. Facts still pending remain in the queue.
    pub fn drain_ready(&mut self, now: i64) -> Vec<PerceivedFact> {
        let mut ready = Vec::new();
        let mut i = 0;
        while i < self.facts.len() {
            if self.facts[i].arrives_at <= now {
                ready.push(self.facts.remove(i));
            } else {
                i += 1;
            }
        }
        ready
    }

    /// How many facts are currently pending (not yet arrived).
    pub fn pending_len(&self) -> usize {
        self.facts.len()
    }
}

/// Snapshot of a single FTL Comm Relay endpoint for arrival-time computation.
///
/// Built once per tick by [`collect_relay_snapshots`] so the arrival-time
/// helpers don't need to touch ECS queries.
#[derive(Clone, Debug)]
pub struct RelaySnapshot {
    pub position: [f64; 3],
    /// Effective range after `empire_relay_range` modifiers. Zero / negative
    /// means "this relay is non-operational for coverage purposes".
    pub range_ly: f64,
    /// Whether this relay has a live partner (i.e. can actually forward).
    pub paired: bool,
}

/// Lightweight snapshot of the empire's relay network. MVP assumption:
/// **all relays belong to one network**; proper multi-network BFS is future
/// work. See issue #233 design notes.
#[derive(Resource, Default, Clone, Debug)]
pub struct RelayNetwork {
    pub relays: Vec<RelaySnapshot>,
}

/// Rebuild [`RelayNetwork`] each tick from live Deep-Space entities.
///
/// Skips entities still in construction (`ConstructionPlatform`) or scrapping
/// (`Scrapyard`). Uses the `empire.comm_relay_range` bonus from [`CommsParams`]
/// on the `PlayerEmpire` entity when computing effective range.
pub fn rebuild_relay_network(
    mut network: ResMut<RelayNetwork>,
    structures: Query<
        (
            Entity,
            &DeepSpaceStructure,
            &Position,
            Option<&FTLCommRelay>,
        ),
        (Without<ConstructionPlatform>, Without<Scrapyard>),
    >,
    registry: Res<DeliverableRegistry>,
    empire_q: Query<&CommsParams, With<crate::player::PlayerEmpire>>,
) {
    let empire_bonus = empire_q
        .iter()
        .next()
        .map(|c| c.empire_relay_range.final_value().to_f64())
        .unwrap_or(0.0);

    network.relays.clear();
    for (_e, structure, pos, relay) in structures.iter() {
        let Some(def) = registry.get(&structure.definition_id) else {
            continue;
        };
        let Some(cap) = def.capabilities.get("ftl_comm_relay") else {
            continue;
        };
        network.relays.push(RelaySnapshot {
            position: pos.as_array(),
            range_ly: effective_relay_range(cap, empire_bonus),
            paired: relay.is_some(),
        });
    }
}

/// Compute the effective range of a relay. Zero-range capabilities (the Lua
/// default) are treated as infinite; otherwise the `empire_relay_range`
/// additive bonus is applied.
pub fn effective_relay_range(cap: &CapabilityParams, empire_range_bonus: f64) -> f64 {
    if cap.range <= 0.0 {
        // A range of zero in Lua means "infinite" — see
        // `relay_knowledge_propagate_system` doc.
        f64::INFINITY
    } else {
        cap.range + empire_range_bonus
    }
}

/// Convert a relay distance into an arrival-delay in hexadies. Applies the
/// base FTL multiplier plus the `empire_relay_inv_latency` bonus.
pub fn relay_delay_hexadies(distance_ly: f64, comms: &CommsParams) -> i64 {
    let base = FTL_RELAY_BASE_MULTIPLIER;
    let bonus = comms.empire_relay_inv_latency.final_value().to_f64();
    let multiplier = base + bonus;
    if multiplier <= 0.0 {
        return physics::light_delay_hexadies(distance_ly);
    }
    let light = physics::light_delay_hexadies(distance_ly) as f64;
    (light / multiplier).floor() as i64
}

/// Find the nearest relay that covers `point`, returning `(position, index)`.
/// A relay "covers" `point` when `point` lies within its effective range, or
/// its range is infinite. Returns `None` if no relay covers `point`.
fn nearest_covering_relay(
    point: [f64; 3],
    relays: &[RelaySnapshot],
) -> Option<(usize, [f64; 3], f64)> {
    let mut best: Option<(usize, [f64; 3], f64)> = None;
    for (i, relay) in relays.iter().enumerate() {
        if !relay.paired {
            continue;
        }
        let dist = physics::distance_ly_arr(point, relay.position);
        let covered = !relay.range_ly.is_finite() || dist <= relay.range_ly;
        if !covered {
            continue;
        }
        if best.is_none() || dist < best.as_ref().unwrap().2 {
            best = Some((i, relay.position, dist));
        }
    }
    best
}

/// Result of the arrival-time computation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrivalPlan {
    pub arrives_at: i64,
    pub source: ObservationSource,
}

/// Compute when a fact observed at `origin` will arrive at `player`.
///
/// Two-stage model:
/// 1. If origin and player are both covered by (any) relay in the network,
///    and at least two relays are paired, the path is
///    `origin → relay_o (light) → relay_p (FTL) → player (light)`. Source is
///    `Relay`.
/// 2. Otherwise fall back to a pure light-speed path (`Direct`).
///
/// **MVP**: single-empire assumption → any pair of covering relays is
/// treated as connected. Proper network BFS is future work (#233 note).
pub fn compute_fact_arrival(
    observed_at: i64,
    origin: [f64; 3],
    player: [f64; 3],
    relays: &[RelaySnapshot],
    comms: &CommsParams,
) -> ArrivalPlan {
    // Pure light-speed fallback.
    let light_distance = physics::distance_ly_arr(origin, player);
    let light_delay = physics::light_delay_hexadies(light_distance);
    let direct = ArrivalPlan {
        arrives_at: observed_at + light_delay,
        source: ObservationSource::Direct,
    };

    // Try relay path.
    let Some((o_idx, relay_o_pos, origin_to_relay_dist)) =
        nearest_covering_relay(origin, relays)
    else {
        return direct;
    };
    let Some((p_idx, relay_p_pos, player_to_relay_dist)) =
        nearest_covering_relay(player, relays)
    else {
        return direct;
    };

    let relay_delay = if o_idx == p_idx {
        // Same relay on both ends — no FTL hop needed.
        0
    } else {
        let relay_distance = physics::distance_ly_arr(relay_o_pos, relay_p_pos);
        relay_delay_hexadies(relay_distance, comms)
    };

    let endpoint_light = physics::light_delay_hexadies(origin_to_relay_dist)
        + physics::light_delay_hexadies(player_to_relay_dist);
    let relay_total = relay_delay + endpoint_light;

    if relay_total < light_delay {
        ArrivalPlan {
            arrives_at: observed_at + relay_total,
            source: ObservationSource::Relay,
        }
    } else {
        direct
    }
}

/// Common helper that funnels both systems-1 (fact) and systems-2 (local)
/// producers through the same decision point. Returns the computed
/// `(arrives_at, source)` so callers can also populate a `GameEvent` if they
/// dual-write.
///
/// - If `player_aboard` or `origin_pos == player_pos`, the event is treated
///   as local and pushed directly into the notification queue (systems-2);
///   the returned `arrives_at == observed_at`, `source = Direct`.
/// - Otherwise the fact is routed through `PendingFactQueue` with an arrival
///   time from [`compute_fact_arrival`].
#[allow(clippy::too_many_arguments)]
pub fn record_fact_or_local(
    fact: KnowledgeFact,
    origin_pos: [f64; 3],
    observed_at: i64,
    player_aboard: bool,
    player_pos: [f64; 3],
    queue: &mut PendingFactQueue,
    notifications: &mut crate::notifications::NotificationQueue,
    relays: &[RelaySnapshot],
    comms: &CommsParams,
) -> (i64, ObservationSource) {
    // Local path: player is on-site so they perceive the event immediately.
    // No light-speed / relay delay applies.
    let is_local = player_aboard || origin_pos == player_pos;

    if is_local {
        notifications.push(
            fact.title().to_string(),
            fact.description(),
            None,
            fact.priority(),
            fact.related_system(),
        );
        return (observed_at, ObservationSource::Direct);
    }

    let plan = compute_fact_arrival(observed_at, origin_pos, player_pos, relays, comms);
    let related_system = fact.related_system();
    queue.record(PerceivedFact {
        fact,
        observed_at,
        arrives_at: plan.arrives_at,
        source: plan.source,
        origin_pos,
        related_system,
    });
    (plan.arrives_at, plan.source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::SignedAmt;
    use crate::modifier::Modifier;

    fn empty_comms() -> CommsParams {
        CommsParams::default()
    }

    #[test]
    fn relay_delay_base_multiplier() {
        // 1 LY light delay = 60 hd; base multiplier 10 → 6 hd.
        assert_eq!(relay_delay_hexadies(1.0, &empty_comms()), 6);
    }

    #[test]
    fn relay_delay_with_inv_latency_bonus() {
        // multiplier 10 + 5 = 15 → 60 / 15 = 4 hd.
        let mut comms = empty_comms();
        comms.empire_relay_inv_latency.push_modifier(Modifier {
            id: "test:inv_latency".into(),
            label: "Test".into(),
            base_add: SignedAmt::from_f64(5.0),
            multiplier: SignedAmt::ZERO,
            add: SignedAmt::ZERO,
            expires_at: None,
            on_expire_event: None,
        });
        assert_eq!(relay_delay_hexadies(1.0, &comms), 4);
    }

    #[test]
    fn compute_arrival_no_relays_is_light_speed() {
        let origin = [0.0, 0.0, 0.0];
        let player = [10.0, 0.0, 0.0];
        let plan = compute_fact_arrival(0, origin, player, &[], &empty_comms());
        assert_eq!(plan.source, ObservationSource::Direct);
        assert_eq!(plan.arrives_at, 600); // 10 ly × 60 hd/ly
    }

    #[test]
    fn compute_arrival_relay_is_faster() {
        // Both origin and player under coverage of separate, paired relays.
        let origin = [0.0, 0.0, 0.0];
        let player = [50.0, 0.0, 0.0];
        let relay_o = RelaySnapshot {
            position: [1.0, 0.0, 0.0],
            range_ly: 5.0,
            paired: true,
        };
        let relay_p = RelaySnapshot {
            position: [49.0, 0.0, 0.0],
            range_ly: 5.0,
            paired: true,
        };
        let relays = vec![relay_o, relay_p];

        let plan = compute_fact_arrival(0, origin, player, &relays, &empty_comms());
        assert_eq!(plan.source, ObservationSource::Relay);
        // Light endpoint o→relay_o = 60 hd, player→relay_p = 60 hd.
        // Relay hop 48 ly → light 2880 hd / 10 = 288 hd.
        // Total 60 + 288 + 60 = 408 hd; direct light = 3000 hd.
        assert!(plan.arrives_at < 3000);
    }

    #[test]
    fn compute_arrival_falls_back_when_unpaired() {
        let origin = [0.0, 0.0, 0.0];
        let player = [50.0, 0.0, 0.0];
        let relay_o = RelaySnapshot {
            position: [1.0, 0.0, 0.0],
            range_ly: 5.0,
            paired: false, // unpaired → skipped
        };
        let relays = vec![relay_o];

        let plan = compute_fact_arrival(0, origin, player, &relays, &empty_comms());
        assert_eq!(plan.source, ObservationSource::Direct);
        assert_eq!(plan.arrives_at, 3000);
    }

    #[test]
    fn pending_queue_drain_ready_respects_arrival_time() {
        let mut q = PendingFactQueue::default();
        q.record(PerceivedFact {
            fact: KnowledgeFact::SurveyComplete {
                system: Entity::PLACEHOLDER,
                system_name: "A".into(),
                detail: "A".into(),
            },
            observed_at: 0,
            arrives_at: 100,
            source: ObservationSource::Direct,
            origin_pos: [0.0; 3],
            related_system: None,
        });
        q.record(PerceivedFact {
            fact: KnowledgeFact::SurveyComplete {
                system: Entity::PLACEHOLDER,
                system_name: "B".into(),
                detail: "B".into(),
            },
            observed_at: 0,
            arrives_at: 200,
            source: ObservationSource::Direct,
            origin_pos: [0.0; 3],
            related_system: None,
        });

        let drained = q.drain_ready(150);
        assert_eq!(drained.len(), 1);
        assert_eq!(q.pending_len(), 1);

        let drained = q.drain_ready(250);
        assert_eq!(drained.len(), 1);
        assert_eq!(q.pending_len(), 0);
    }

    #[test]
    fn record_fact_or_local_bypasses_queue_when_player_aboard() {
        let mut queue = PendingFactQueue::default();
        let mut notifs = crate::notifications::NotificationQueue::new();
        let comms = empty_comms();
        let fact = KnowledgeFact::CombatOutcome {
            system: Entity::PLACEHOLDER,
            victor: CombatVictor::Player,
            detail: "On-site victory".into(),
        };
        let (arrives_at, src) = record_fact_or_local(
            fact,
            [100.0, 0.0, 0.0],
            50,
            true, // player aboard
            [0.0, 0.0, 0.0],
            &mut queue,
            &mut notifs,
            &[],
            &comms,
        );
        assert_eq!(arrives_at, 50);
        assert_eq!(src, ObservationSource::Direct);
        assert_eq!(queue.pending_len(), 0);
        assert_eq!(notifs.items.len(), 1);
    }

    #[test]
    fn record_fact_or_local_queues_remote_event() {
        let mut queue = PendingFactQueue::default();
        let mut notifs = crate::notifications::NotificationQueue::new();
        let comms = empty_comms();
        let fact = KnowledgeFact::HostileDetected {
            target: Entity::PLACEHOLDER,
            detector: Entity::PLACEHOLDER,
            target_pos: [50.0, 0.0, 0.0],
            description: "Hostile".into(),
        };
        let (arrives_at, src) = record_fact_or_local(
            fact,
            [50.0, 0.0, 0.0],
            0,
            false,
            [0.0, 0.0, 0.0],
            &mut queue,
            &mut notifs,
            &[],
            &comms,
        );
        assert_eq!(src, ObservationSource::Direct);
        assert_eq!(arrives_at, 50 * 60); // 50 ly × 60 hd
        assert_eq!(queue.pending_len(), 1);
        assert_eq!(notifs.items.len(), 0);
    }
}
