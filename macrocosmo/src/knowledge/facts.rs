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

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::collections::HashMap;

use crate::components::Position;
use crate::deep_space::{
    CapabilityParams, ConstructionPlatform, DeepSpaceStructure, DeliverableRegistry, FTLCommRelay,
    Scrapyard,
};
use crate::empire::comms::CommsParams;
use crate::notifications::NotificationQueue;
use crate::physics;

use super::ObservationSource;

/// #249: Global event identifier used to dedupe notification banners when the
/// same world happening is surfaced through both the legacy `GameEvent` flow
/// and the `KnowledgeFact` pipeline (dual-write transition), and also to
/// dedupe multiple facts that originate from a single logical event (e.g.
/// per-ship `CombatDefeat` + all-ships-wiped `CombatDefeat`).
///
/// Allocated by [`NextEventId`] via [`NextEventId::allocate`]. Copy semantics
/// so it's cheap to pass into both a `GameEvent` and a `KnowledgeFact`.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default, bevy::reflect::Reflect,
)]
pub struct EventId(pub u64);

/// Monotonic counter resource that hands out fresh [`EventId`]s. Ids start at
/// 1 so that `EventId::default()` (which returns 0) can represent "no id yet"
/// when useful.
#[derive(Resource, Debug, Default, Reflect)]
#[reflect(Resource)]
pub struct NextEventId {
    counter: u64,
}

impl NextEventId {
    pub fn allocate(&mut self) -> EventId {
        self.counter = self.counter.wrapping_add(1);
        EventId(self.counter)
    }

    pub fn peek(&self) -> u64 {
        self.counter
    }
}

/// Set of [`EventId`]s that have already surfaced a notification banner.
///
/// Consumed by both [`crate::notifications::auto_notify_from_events`] and
/// [`crate::notifications::notify_from_knowledge_facts`] so that a dual-write
/// (legacy `GameEvent` + `KnowledgeFact`) only produces **one** banner per
/// underlying world happening. Populated on the first successful push and
/// checked before every subsequent push for the same `EventId`.
///
/// ## State machine (tri-state)
///
/// Each tracked id is in one of three states:
///
/// | map state          | meaning                                           | `try_notify` |
/// |--------------------|---------------------------------------------------|--------------|
/// | not present        | 未使用 or already closed (treated as notified)    | returns `false` (skip push) |
/// | `Some(false)`      | registered, banner not yet pushed                 | returns `true`, sets to `true` |
/// | `Some(true)`       | banner already pushed                             | returns `false` (skip push) |
///
/// The "missing == treated as notified" rule is the safety net: closing an id
/// too early can never cause a duplicate banner, only a (silently) suppressed
/// one. This lets us aggressively close ids to keep memory bounded.
///
/// ## Lifecycle
///
/// 1. [`Self::register`] when a new id is allocated for a dual-write
///    (typically by [`FactSysParam::allocate_event_id`]).
/// 2. [`Self::try_notify`] from each banner push path; the first one wins.
/// 3. [`sweep_notified_event_ids`] runs once per frame after both notify
///    systems have finished and removes every entry that reached `true` —
///    those ids will not produce another banner, so the memory is freed.
///
/// Entries that stay `false` across many ticks (registered but neither path
/// has surfaced a banner — typically because the fact is still in flight in
/// [`PendingFactQueue`]) remain until they reach `true` or are explicitly
/// closed via [`Self::close`].
#[derive(Resource, Debug, Default, Reflect)]
#[reflect(Resource)]
pub struct NotifiedEventIds {
    notified: HashMap<EventId, bool>,
}

impl NotifiedEventIds {
    /// Mark an id as live (not yet notified). Idempotent — re-registering an
    /// already-notified id leaves it `true`.
    pub fn register(&mut self, id: EventId) {
        self.notified.entry(id).or_insert(false);
    }

    /// Atomically claim the first banner push for this id.
    ///
    /// Returns `true` (and flips the entry to `true`) only if the id is
    /// currently registered as `false`. Any other state — missing or already
    /// `true` — returns `false` so the caller skips the push.
    pub fn try_notify(&mut self, id: EventId) -> bool {
        match self.notified.get_mut(&id) {
            Some(slot) if !*slot => {
                *slot = true;
                true
            }
            _ => false,
        }
    }

    /// Explicitly remove an id. After this any future [`Self::try_notify`]
    /// returns `false` (missing == "treated as notified").
    pub fn close(&mut self, id: EventId) {
        self.notified.remove(&id);
    }

    /// Drop every entry that has reached `true`. Intended to run once per
    /// frame after both notify systems via [`sweep_notified_event_ids`];
    /// bounds the map size at "ids registered this tick that haven't been
    /// notified yet".
    pub fn sweep_notified(&mut self) {
        self.notified.retain(|_, notified| !*notified);
    }

    /// True when the id is currently tracked (in either state). Mostly useful
    /// for diagnostics / tests.
    pub fn contains(&self, id: EventId) -> bool {
        self.notified.contains_key(&id)
    }

    pub fn len(&self) -> usize {
        self.notified.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notified.is_empty()
    }
}

/// #249: System that runs once per frame after both notify paths and frees
/// every notified id. Without it the map grows unbounded over a session.
pub fn sweep_notified_event_ids(mut notified: ResMut<NotifiedEventIds>) {
    notified.sweep_notified();
}

/// Base FTL multiplier for relay-routed propagation. `relay_delay` at base
/// evaluates to `light_delay / 10`. `empire_relay_inv_latency` modifiers stack
/// additively on top of this base.
pub const FTL_RELAY_BASE_MULTIPLIER: f64 = 10.0;

/// Combat victor designator for [`KnowledgeFact::CombatOutcome`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, bevy::reflect::Reflect)]
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
///
/// #249: Every variant carries an optional `event_id`. When set, the banner
/// push path looks up [`NotifiedEventIds`] and drops the push if the id has
/// already fired — this dedupes dual-written events between the legacy
/// `GameEvent` flow and the fact pipeline, and multi-fact events (per-ship +
/// wipe CombatDefeat). Scout-only facts with no `GameEvent` counterpart keep
/// `event_id = None`.
#[derive(Clone, Debug, bevy::reflect::Reflect)]
pub enum KnowledgeFact {
    /// A hostile contact was detected in deep space (#186 pursuit).
    HostileDetected {
        event_id: Option<EventId>,
        target: Entity,
        detector: Entity,
        target_pos: [f64; 3],
        description: String,
    },
    /// Combat completed at a star system.
    CombatOutcome {
        event_id: Option<EventId>,
        system: Entity,
        victor: CombatVictor,
        detail: String,
    },
    /// A star system was fully surveyed.
    SurveyComplete {
        event_id: Option<EventId>,
        system: Entity,
        system_name: String,
        detail: String,
    },
    /// An anomaly was discovered during a survey.
    AnomalyDiscovered {
        event_id: Option<EventId>,
        system: Entity,
        anomaly_id: String,
        detail: String,
    },
    /// Non-anomaly survey discovery (legacy exploration event).
    SurveyDiscovery {
        event_id: Option<EventId>,
        system: Entity,
        detail: String,
    },
    /// A ship / structure was built or destroyed.
    StructureBuilt {
        event_id: Option<EventId>,
        system: Option<Entity>,
        kind: String,
        name: String,
        destroyed: bool,
        detail: String,
    },
    /// A colony was founded at a planet.
    ColonyEstablished {
        event_id: Option<EventId>,
        system: Entity,
        planet: Entity,
        name: String,
        detail: String,
    },
    /// A colony attempt failed.
    ColonyFailed {
        event_id: Option<EventId>,
        system: Entity,
        name: String,
        reason: String,
    },
    /// A ship arrived at a system (routine — usually Low priority).
    ShipArrived {
        event_id: Option<EventId>,
        system: Option<Entity>,
        name: String,
        detail: String,
    },
    /// #463: An Infrastructure Core was conquered (hull dropped to 1.0). The
    /// `conquered_by` faction is the attacker; `original_owner` is the
    /// defender whose Core entered the lock. Empires receive this fact via
    /// light-speed propagation from the conquered system, mirroring the
    /// observation contract used by combat / settlement events.
    CoreConquered {
        event_id: Option<EventId>,
        system: Entity,
        conquered_by: Entity,
        original_owner: Entity,
        detail: String,
    },
    /// #472: A ship has been destroyed. Per-faction observation of the
    /// destruction, paired with the audit-only [`crate::events::GameEvent::ShipDestroyed`]
    /// fired at the destruction site. Empires learn about the loss after the
    /// light-speed (or relay-shortened) delay from the destruction position
    /// to their viewer — mirroring the [`KnowledgeFact::CoreConquered`]
    /// contract codified by #463.
    ShipDestroyed {
        event_id: Option<EventId>,
        /// Last known star system (the destruction site). `None` for
        /// deep-space destructions with no associated system.
        system: Option<Entity>,
        ship_name: String,
        /// Hexadies tick at which the ship was actually destroyed at the
        /// origin. Distinct from `PerceivedFact::observed_at` in that this
        /// stays useful to subscribers even after the fact has propagated.
        destroyed_at: i64,
        detail: String,
    },
    /// #472: A ship has not returned by expected time and is presumed
    /// missing — an empire-side epistemic state with no omniscient audit
    /// counterpart (`event_id` is therefore always `None`). Emitted once
    /// per empire whose grace window has elapsed before destruction light
    /// arrives. The matching [`KnowledgeFact::ShipDestroyed`] supersedes
    /// this fact when the destruction light eventually reaches the empire.
    ShipMissing {
        event_id: Option<EventId>,
        system: Option<Entity>,
        ship_name: String,
        detail: String,
    },
    /// #351 (K-2): Lua-defined knowledge kind. The payload is captured as a
    /// [`PayloadSnapshot`](super::payload::PayloadSnapshot) so the fact
    /// survives being queued without keeping Lua references alive.
    Scripted {
        event_id: Option<EventId>,
        kind_id: String,
        origin_system: Option<Entity>,
        payload_snapshot: super::payload::PayloadSnapshot,
        recorded_at: i64,
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
                if *destroyed {
                    "Structure Destroyed"
                } else {
                    "Structure Built"
                }
            }
            KnowledgeFact::ColonyEstablished { .. } => "Colony Established",
            KnowledgeFact::ColonyFailed { .. } => "Colony Failed",
            KnowledgeFact::ShipArrived { .. } => "Ship Arrived",
            KnowledgeFact::CoreConquered { .. } => "Core Conquered",
            KnowledgeFact::ShipDestroyed { .. } => "Ship Destroyed",
            KnowledgeFact::ShipMissing { .. } => "Ship Missing",
            KnowledgeFact::Scripted { .. } => "Knowledge",
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
            KnowledgeFact::CoreConquered { detail, .. } => detail.clone(),
            KnowledgeFact::ShipDestroyed { detail, .. } => detail.clone(),
            KnowledgeFact::ShipMissing { detail, .. } => detail.clone(),
            KnowledgeFact::Scripted {
                kind_id,
                payload_snapshot,
                ..
            } => payload_snapshot
                .fields
                .get("detail")
                .and_then(|v| match v {
                    super::payload::PayloadValue::String(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| kind_id.clone()),
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
            KnowledgeFact::CoreConquered { .. } => High,
            KnowledgeFact::ShipDestroyed { .. } => High,
            KnowledgeFact::ShipMissing { .. } => High,
            KnowledgeFact::Scripted { .. } => Medium,
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
            KnowledgeFact::CoreConquered { system, .. } => Some(*system),
            KnowledgeFact::ShipDestroyed { system, .. } => *system,
            KnowledgeFact::ShipMissing { system, .. } => *system,
            KnowledgeFact::Scripted { origin_system, .. } => *origin_system,
        }
    }

    /// #249: The [`EventId`] attached to this fact, if any. Used by the
    /// banner push path to dedupe dual-writes and multi-fact events.
    pub fn event_id(&self) -> Option<EventId> {
        match self {
            KnowledgeFact::HostileDetected { event_id, .. }
            | KnowledgeFact::CombatOutcome { event_id, .. }
            | KnowledgeFact::SurveyComplete { event_id, .. }
            | KnowledgeFact::AnomalyDiscovered { event_id, .. }
            | KnowledgeFact::SurveyDiscovery { event_id, .. }
            | KnowledgeFact::StructureBuilt { event_id, .. }
            | KnowledgeFact::ColonyEstablished { event_id, .. }
            | KnowledgeFact::ColonyFailed { event_id, .. }
            | KnowledgeFact::ShipArrived { event_id, .. }
            | KnowledgeFact::CoreConquered { event_id, .. }
            | KnowledgeFact::ShipDestroyed { event_id, .. }
            | KnowledgeFact::ShipMissing { event_id, .. }
            | KnowledgeFact::Scripted { event_id, .. } => *event_id,
        }
    }

    /// #354 (K-5): `core:*` kind id for each built-in variant. Returns
    /// `None` for [`KnowledgeFact::Scripted`] — a Lua-origin fact
    /// already carries its own kind id in the `kind_id` field.
    ///
    /// The mapping must stay in sync with
    /// [`crate::knowledge::kind_registry::CORE_KIND_IDS`] — the unit
    /// tests in `facts::tests::core_kind_id_mapping_matches_registry`
    /// enforce this at CI time.
    pub fn core_kind_id(&self) -> Option<&'static str> {
        match self {
            KnowledgeFact::HostileDetected { .. } => Some("core:hostile_detected"),
            KnowledgeFact::CombatOutcome { .. } => Some("core:combat_outcome"),
            KnowledgeFact::SurveyComplete { .. } => Some("core:survey_complete"),
            KnowledgeFact::AnomalyDiscovered { .. } => Some("core:anomaly_discovered"),
            KnowledgeFact::SurveyDiscovery { .. } => Some("core:survey_discovery"),
            KnowledgeFact::StructureBuilt { .. } => Some("core:structure_built"),
            KnowledgeFact::ColonyEstablished { .. } => Some("core:colony_established"),
            KnowledgeFact::ColonyFailed { .. } => Some("core:colony_failed"),
            KnowledgeFact::ShipArrived { .. } => Some("core:ship_arrived"),
            KnowledgeFact::CoreConquered { .. } => Some("core:core_conquered"),
            KnowledgeFact::ShipDestroyed { .. } => Some("core:ship_destroyed"),
            KnowledgeFact::ShipMissing { .. } => Some("core:ship_missing"),
            KnowledgeFact::Scripted { .. } => None,
        }
    }

    /// #354 (K-5): Flatten a core variant into a payload snapshot that
    /// K-3 `<core:*>@recorded` / K-4 `<core:*>@observed` subscribers see
    /// under `e.payload`. Returns `None` for [`KnowledgeFact::Scripted`]
    /// — Lua-origin facts already carry their snapshot directly.
    ///
    /// Field mapping mirrors
    /// [`crate::knowledge::kind_registry::core_kind_catalog`]:
    /// * `Entity` values become [`PayloadValue::Entity`]
    /// * `[f64; 3]` positions are flattened to `*_x/_y/_z` numbers
    /// * `CombatVictor` becomes a `"player" | "hostile"` string
    /// * `bool` becomes [`PayloadValue::Boolean`]
    /// * `event_id` is **not** emitted — it lives in the Rust-internal
    ///   `NotifiedEventIds` map and is not part of the observable
    ///   `e.payload` surface (see plan-349 §3.5 field-rationale note).
    ///
    /// Optional `Entity` fields (`system: Option<Entity>` on
    /// `StructureBuilt` / `ShipArrived`) are only inserted when `Some`.
    pub fn to_core_payload_snapshot(&self) -> Option<super::payload::PayloadSnapshot> {
        use super::payload::{PayloadSnapshot, PayloadValue};
        use std::collections::HashMap;

        let mut fields: HashMap<String, PayloadValue> = HashMap::new();
        match self {
            KnowledgeFact::HostileDetected {
                target,
                detector,
                target_pos,
                description,
                ..
            } => {
                fields.insert("target".into(), PayloadValue::Entity(target.to_bits()));
                fields.insert("detector".into(), PayloadValue::Entity(detector.to_bits()));
                fields.insert("target_pos_x".into(), PayloadValue::Number(target_pos[0]));
                fields.insert("target_pos_y".into(), PayloadValue::Number(target_pos[1]));
                fields.insert("target_pos_z".into(), PayloadValue::Number(target_pos[2]));
                fields.insert(
                    "description".into(),
                    PayloadValue::String(description.clone()),
                );
            }
            KnowledgeFact::CombatOutcome {
                system,
                victor,
                detail,
                ..
            } => {
                fields.insert("system".into(), PayloadValue::Entity(system.to_bits()));
                fields.insert(
                    "victor".into(),
                    PayloadValue::String(
                        match victor {
                            CombatVictor::Player => "player",
                            CombatVictor::Hostile => "hostile",
                        }
                        .to_string(),
                    ),
                );
                fields.insert("detail".into(), PayloadValue::String(detail.clone()));
            }
            KnowledgeFact::SurveyComplete {
                system,
                system_name,
                detail,
                ..
            } => {
                fields.insert("system".into(), PayloadValue::Entity(system.to_bits()));
                fields.insert(
                    "system_name".into(),
                    PayloadValue::String(system_name.clone()),
                );
                fields.insert("detail".into(), PayloadValue::String(detail.clone()));
            }
            KnowledgeFact::AnomalyDiscovered {
                system,
                anomaly_id,
                detail,
                ..
            } => {
                fields.insert("system".into(), PayloadValue::Entity(system.to_bits()));
                fields.insert(
                    "anomaly_id".into(),
                    PayloadValue::String(anomaly_id.clone()),
                );
                fields.insert("detail".into(), PayloadValue::String(detail.clone()));
            }
            KnowledgeFact::SurveyDiscovery { system, detail, .. } => {
                fields.insert("system".into(), PayloadValue::Entity(system.to_bits()));
                fields.insert("detail".into(), PayloadValue::String(detail.clone()));
            }
            KnowledgeFact::StructureBuilt {
                system,
                kind,
                name,
                destroyed,
                detail,
                ..
            } => {
                if let Some(s) = system {
                    fields.insert("system".into(), PayloadValue::Entity(s.to_bits()));
                }
                fields.insert("kind".into(), PayloadValue::String(kind.clone()));
                fields.insert("name".into(), PayloadValue::String(name.clone()));
                fields.insert("destroyed".into(), PayloadValue::Boolean(*destroyed));
                fields.insert("detail".into(), PayloadValue::String(detail.clone()));
            }
            KnowledgeFact::ColonyEstablished {
                system,
                planet,
                name,
                detail,
                ..
            } => {
                fields.insert("system".into(), PayloadValue::Entity(system.to_bits()));
                fields.insert("planet".into(), PayloadValue::Entity(planet.to_bits()));
                fields.insert("name".into(), PayloadValue::String(name.clone()));
                fields.insert("detail".into(), PayloadValue::String(detail.clone()));
            }
            KnowledgeFact::ColonyFailed {
                system,
                name,
                reason,
                ..
            } => {
                fields.insert("system".into(), PayloadValue::Entity(system.to_bits()));
                fields.insert("name".into(), PayloadValue::String(name.clone()));
                fields.insert("reason".into(), PayloadValue::String(reason.clone()));
            }
            KnowledgeFact::ShipArrived {
                system,
                name,
                detail,
                ..
            } => {
                if let Some(s) = system {
                    fields.insert("system".into(), PayloadValue::Entity(s.to_bits()));
                }
                fields.insert("name".into(), PayloadValue::String(name.clone()));
                fields.insert("detail".into(), PayloadValue::String(detail.clone()));
            }
            KnowledgeFact::CoreConquered {
                system,
                conquered_by,
                original_owner,
                detail,
                ..
            } => {
                fields.insert("system".into(), PayloadValue::Entity(system.to_bits()));
                fields.insert(
                    "conquered_by".into(),
                    PayloadValue::Entity(conquered_by.to_bits()),
                );
                fields.insert(
                    "original_owner".into(),
                    PayloadValue::Entity(original_owner.to_bits()),
                );
                fields.insert("detail".into(), PayloadValue::String(detail.clone()));
            }
            KnowledgeFact::ShipDestroyed {
                system,
                ship_name,
                destroyed_at,
                detail,
                ..
            } => {
                if let Some(s) = system {
                    fields.insert("system".into(), PayloadValue::Entity(s.to_bits()));
                }
                fields.insert("ship_name".into(), PayloadValue::String(ship_name.clone()));
                fields.insert(
                    "destroyed_at".into(),
                    PayloadValue::Number(*destroyed_at as f64),
                );
                fields.insert("detail".into(), PayloadValue::String(detail.clone()));
            }
            KnowledgeFact::ShipMissing {
                system,
                ship_name,
                detail,
                ..
            } => {
                if let Some(s) = system {
                    fields.insert("system".into(), PayloadValue::Entity(s.to_bits()));
                }
                fields.insert("ship_name".into(), PayloadValue::String(ship_name.clone()));
                fields.insert("detail".into(), PayloadValue::String(detail.clone()));
            }
            KnowledgeFact::Scripted { .. } => return None,
        }
        Some(PayloadSnapshot { fields })
    }

    /// #354 (K-5): Related star system for a core variant, matching the
    /// `origin_system` metadata slot in `<core:*>@recorded` events. For
    /// variants that already carry `Option<Entity>` (`StructureBuilt`,
    /// `ShipArrived`) this forwards the optional field; other variants
    /// provide their concrete system via `Some(_)`. Identical to
    /// [`Self::related_system`] today but kept as a separate hook so
    /// future core variants with a distinct "origin" vs "related"
    /// semantic can diverge cleanly.
    pub fn core_origin_system(&self) -> Option<Entity> {
        self.related_system()
    }
}

/// A [`KnowledgeFact`] plus the timing + provenance metadata the arrival
/// scheduler needs.
#[derive(Clone, Debug, bevy::reflect::Reflect)]
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

/// Holds facts waiting for their light-speed / relay arrival time.
///
/// Parallel to (not merged with) [`KnowledgeStore`](super::KnowledgeStore).
/// Responsibility split:
///   - `KnowledgeStore` → "what is the world like right now, from this
///     empire's vantage point" (snapshot).
///   - `PendingFactQueue` → "what *happened* that this empire will hear
///     about at time T" (delta).
///
/// Round 9 PR #1 Step 2: `PendingFactQueue` is now both a `Resource`
/// (legacy player-empire-only queue, drained by
/// `dispatch_knowledge_observed`) **and** a `Component` attached to
/// every `Empire` entity. Step 3 migrates production callsites from
/// the Resource path to per-empire Components via
/// [`FactSysParam::record_for`]; until then the Component is added on
/// every empire spawn but stays empty for NPC empires (the player
/// empire continues to receive both). Once all callsites are migrated
/// the Resource derive will be removed in a follow-up PR.
#[derive(Resource, Component, Default, Reflect)]
#[reflect(Resource, Component)]
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

    /// #353 (K-4): Drain **only** `KnowledgeFact::Scripted` facts whose
    /// `arrives_at <= now`, leaving core variants in place for the legacy
    /// `notify_from_knowledge_facts` path (banner drain).
    ///
    /// #354 K-5 status: this partitioned drain is **no longer used** by
    /// the production pipeline — `dispatch_knowledge_observed` now
    /// drains the whole queue via `drain_ready()` and handles both
    /// core + scripted variants in a single pass. The partitioned
    /// helper is retained for any future callsites that deliberately
    /// want the Scripted-only subset (plus the K-4 unit tests that
    /// exercise its ordering invariant).
    pub fn drain_ready_scripted(&mut self, now: i64) -> Vec<PerceivedFact> {
        let mut ready = Vec::new();
        let mut i = 0;
        while i < self.facts.len() {
            let pf = &self.facts[i];
            if matches!(pf.fact, KnowledgeFact::Scripted { .. }) && pf.arrives_at <= now {
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
#[derive(Clone, Debug, bevy::reflect::Reflect)]
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
#[derive(Resource, Default, Clone, Debug, Reflect)]
#[reflect(Resource)]
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
    empire_q: Query<&CommsParams, With<crate::player::Empire>>,
) {
    // Use the max empire relay range bonus across all empires for the global
    // relay network. TODO(#418): per-empire relay networks.
    let empire_bonus = empire_q
        .iter()
        .map(|c| c.empire_relay_range.final_value().to_f64())
        .fold(0.0_f64, f64::max);

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
    let Some((o_idx, relay_o_pos, origin_to_relay_dist)) = nearest_covering_relay(origin, relays)
    else {
        return direct;
    };
    let Some((p_idx, relay_p_pos, player_to_relay_dist)) = nearest_covering_relay(player, relays)
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
/// - If `ruler_aboard` or `origin_pos == player_pos`, the event is treated
///   as local and pushed directly into the notification queue (systems-2);
///   the returned `arrives_at == observed_at`, `source = Direct`.
/// - Otherwise the fact is routed through `PendingFactQueue` with an arrival
///   time from [`compute_fact_arrival`].
///
/// #249: If the fact carries an `EventId`, [`NotifiedEventIds`] is checked on
/// the local path and updated on first push so a later banner for the same id
/// (e.g. from `auto_notify_from_events` or a sibling fact) is suppressed.
#[allow(clippy::too_many_arguments)]
pub fn record_fact_or_local(
    fact: KnowledgeFact,
    origin_pos: [f64; 3],
    observed_at: i64,
    ruler_aboard: bool,
    player_pos: [f64; 3],
    queue: &mut PendingFactQueue,
    notifications: &mut crate::notifications::NotificationQueue,
    notified_ids: &mut NotifiedEventIds,
    relays: &[RelaySnapshot],
    comms: &CommsParams,
) -> (i64, ObservationSource) {
    // Local path: player is on-site so they perceive the event immediately.
    // No light-speed / relay delay applies.
    let is_local = ruler_aboard || origin_pos == player_pos;

    if is_local {
        // Dedupe: only push if this id is registered AND hasn't notified yet.
        // Facts without an event_id (e.g. scout-only) always push.
        let should_push = match fact.event_id() {
            Some(id) => notified_ids.try_notify(id),
            None => true,
        };
        if should_push {
            notifications.push(
                fact.title().to_string(),
                fact.description(),
                None,
                fact.priority(),
                fact.related_system(),
            );
        }
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

/// #249: Minimal snapshot of the player's observation vantage point. Built
/// once per callsite from the system's existing queries; passed by reference
/// to [`record_world_event_fact`] so the helper can make the
/// local-vs-remote decision without pulling Positions itself.
#[derive(Clone, Copy, Debug)]
pub struct PlayerVantage {
    pub player_pos: [f64; 3],
    pub ruler_aboard: bool,
}

/// Round 9 PR #1: Per-faction observation vantage point for the
/// multi-empire knowledge propagation pipeline.
///
/// Equivalent to [`PlayerVantage`] but tagged with the empire entity that
/// owns the vantage so [`FactSysParam::record_for`] can route a single
/// world event into multiple empires' [`PendingFactQueue`]s in one call.
///
/// `ref_pos` is the world-space position the empire uses as its
/// light-speed reference — typically the empire's
/// [`crate::player::EmpireViewerSystem`] position, which mirrors the
/// Ruler's `StationedAt` system.
///
/// `ruler_aboard` shortcircuits the queue for that empire's ruler when
/// they are physically present at the event origin (mirrors the legacy
/// "player is on-site" path in [`record_fact_or_local`]).
#[derive(Clone, Copy, Debug)]
pub struct FactionVantage {
    /// The empire entity this vantage describes.
    pub faction: Entity,
    /// World-space position used for light-speed delay calculation.
    pub ref_pos: [f64; 3],
    /// `true` when the faction's Ruler is aboard a ship — the local
    /// path bypasses the queue when origin position matches `ref_pos`
    /// or the ruler is mobile.
    pub ruler_aboard: bool,
}

impl FactionVantage {
    /// Convert a legacy [`PlayerVantage`] into a [`FactionVantage`]
    /// targeting the given empire entity. Used by the back-compat
    /// adapter so existing callsites keep working until they are
    /// migrated in PR #1 Step 3.
    pub fn from_player(faction: Entity, vantage: &PlayerVantage) -> Self {
        Self {
            faction,
            ref_pos: vantage.player_pos,
            ruler_aboard: vantage.ruler_aboard,
        }
    }
}

/// #249: SystemParam bundle that groups the six resources / queries that every
/// fact-writing callsite needs. Keeps the parameter count of the host system
/// under Bevy's 16-param limit while avoiding a re-query of `Position` (the
/// callsite supplies the vantage via [`PlayerVantage`]).
///
/// #354 K-5: [`FactSysParam::record`] now also pushes a
/// [`PendingKnowledgeRecord`](crate::scripting::knowledge_dispatch::PendingKnowledgeRecord)
/// mirroring the fact into the `core:*` kind so the K-2 pipeline can
/// fire `<core:*>@recorded` subscribers on the next Update tick. This
/// does **not** change the legacy banner / `PendingFactQueue` path —
/// both flows are produced from the same `record()` call.
#[derive(SystemParam)]
pub struct FactSysParam<'w, 's> {
    pub fact_queue: ResMut<'w, PendingFactQueue>,
    pub notifications: ResMut<'w, NotificationQueue>,
    pub notified_ids: ResMut<'w, NotifiedEventIds>,
    pub next_event_id: ResMut<'w, NextEventId>,
    pub relay_network: Res<'w, RelayNetwork>,
    pub empire_comms: Query<'w, 's, &'static CommsParams, With<crate::player::Empire>>,
    /// #354 K-5: core variant -> `PendingKnowledgeRecord` sink. Optional
    /// so callsites that construct a `FactSysParam` outside
    /// `ScriptingPlugin` (tests / observer-mode headless apps) don't
    /// crash when the resource is absent.
    pub pending_records:
        Option<ResMut<'w, crate::scripting::knowledge_dispatch::PendingKnowledgeRecords>>,
}

impl<'w, 's> FactSysParam<'w, 's> {
    /// Allocate a fresh [`EventId`] AND register it with [`NotifiedEventIds`]
    /// so subsequent banner pushes (from either the legacy event flow or the
    /// fact pipeline) dedupe against each other. The first push wins; the
    /// rest are silently suppressed.
    pub fn allocate_event_id(&mut self) -> EventId {
        let id = self.next_event_id.allocate();
        self.notified_ids.register(id);
        id
    }

    /// Snapshot the player empire's [`CommsParams`], falling back to defaults
    /// when no `PlayerEmpire` exists (e.g. observer mode pre-spawn).
    pub fn comms(&self) -> CommsParams {
        self.empire_comms.iter().next().cloned().unwrap_or_default()
    }

    /// Borrow the active relay snapshots.
    pub fn relays(&self) -> &[RelaySnapshot] {
        &self.relay_network.relays
    }

    /// Canonical dual-write entry point for world-event callsites.
    /// Encapsulates the comms / relays lookup so a callsite reduces to:
    ///
    /// ```ignore
    /// let id = fact_sys.allocate_event_id();
    /// events.write(GameEvent { id, ... });
    /// fact_sys.record(
    ///     KnowledgeFact::SomeVariant { event_id: Some(id), .. },
    ///     origin_pos,
    ///     clock.elapsed,
    ///     &vantage,
    /// );
    /// ```
    ///
    /// #354 K-5: after the legacy `record_fact_or_local` call, a core
    /// variant also pushes a `PendingKnowledgeRecord` so the K-2
    /// `dispatch_knowledge_recorded` system can fire
    /// `<core:*>@recorded` subscribers. Scripted variants are ignored
    /// by the core push (they already come in through
    /// `gs:record_knowledge`).
    ///
    /// Round 9 PR #1: This is the legacy player-only entry point.
    /// It now wraps the supplied [`PlayerVantage`] in a single-element
    /// [`FactionVantage`] slice and forwards to [`Self::record_for`].
    /// The faction id used for the wrap is the first `PlayerEmpire` (or
    /// `Entity::PLACEHOLDER` if none — typical observer / headless
    /// startup before empires spawn).
    ///
    /// **Deprecated**: all production callsites in macrocosmo have been
    /// migrated to [`Self::record_for`] in Step 3. This adapter remains
    /// only so the `notification_knowledge_pipeline` integration tests
    /// keep working without a full `FactionVantageQueries` setup.
    /// Remove once those tests migrate to per-faction wiring.
    #[deprecated(
        since = "0.3.1",
        note = "Round 9 PR #1: use `record_for(...)` with `FactionVantageQueries::collect()` instead. See `src/knowledge/mod.rs::collect_faction_vantages`."
    )]
    pub fn record(
        &mut self,
        fact: KnowledgeFact,
        origin_pos: [f64; 3],
        observed_at: i64,
        vantage: &PlayerVantage,
    ) -> (i64, ObservationSource) {
        // Resolve a "best guess" faction entity for the legacy single
        // vantage. If no empire is present, fall back to `Entity::PLACEHOLDER`
        // — `record_for` only uses the faction id for routing in Step 2,
        // and Step 1 (this commit) still routes into the shared queue, so
        // the placeholder is harmless.
        let faction = self
            .empire_comms
            .iter()
            .next()
            .map(|_| Entity::PLACEHOLDER)
            .unwrap_or(Entity::PLACEHOLDER);
        let fv = [FactionVantage::from_player(faction, vantage)];
        self.record_for(fact, &fv, origin_pos, observed_at)
    }

    /// Round 9 PR #1: Per-faction multi-vantage record entry point.
    ///
    /// Routes a single world event into the [`PendingFactQueue`] once per
    /// supplied [`FactionVantage`], so observer mode + NPC empires both
    /// accumulate fact records in their own knowledge pipeline.
    ///
    /// Behaviour notes:
    /// * Each vantage gets an independent arrival-time computation
    ///   ([`compute_fact_arrival`]) keyed on `vantage.ref_pos`.
    /// * The same `EventId` is shared across all vantage pushes — the
    ///   tri-state [`NotifiedEventIds`] map deduplicates the eventual
    ///   banner so the player sees one notification per logical event.
    /// * The local-vs-remote shortcut (`origin == ref_pos` or
    ///   `ruler_aboard`) is decided per vantage. Only the FIRST vantage
    ///   that takes the local path produces a notification banner — the
    ///   rest still record into the queue with a 0-delay so their
    ///   `@observed` subscribers fire on the next tick.
    /// * The `(arrives_at, source)` return value is the result for the
    ///   FIRST vantage (legacy single-vantage callers preserve their
    ///   existing observation contract). Multi-vantage callers that need
    ///   per-vantage arrival info can compute it themselves via
    ///   [`compute_fact_arrival`].
    /// * With an empty `vantages` slice this is a no-op (returns
    ///   `(observed_at, Direct)`); the K-5 core record push is also
    ///   suppressed since there is no observer to dispatch to.
    ///
    /// Step 1 implementation note: the queue is still a single
    /// [`Resource`] today. Step 2 of PR #1 moves it to a per-empire
    /// [`Component`] and the routing here switches to per-empire queue
    /// lookup; the API is stable across that migration.
    pub fn record_for(
        &mut self,
        fact: KnowledgeFact,
        vantages: &[FactionVantage],
        origin_pos: [f64; 3],
        observed_at: i64,
    ) -> (i64, ObservationSource) {
        // No vantages → nothing observes the event; treat as a no-op.
        // Avoids double-pushing when the caller supplies an empty list
        // (e.g. observer mode pre-empire-spawn).
        if vantages.is_empty() {
            return (observed_at, ObservationSource::Direct);
        }

        // Snapshot immutable inputs once, before borrowing ResMut fields.
        let comms = self.empire_comms.iter().next().cloned().unwrap_or_default();
        let relays = self.relay_network.relays.clone();

        // K-5 core mirror: push the pending `core:*` record exactly once
        // regardless of vantage count. Subscribers are observer-empire
        // agnostic at the @recorded stage; @observed already iterates
        // every observer downstream.
        if let (Some(kind_id), Some(snapshot)) =
            (fact.core_kind_id(), fact.to_core_payload_snapshot())
            && let Some(records) = self.pending_records.as_mut()
        {
            records.push(
                crate::scripting::knowledge_dispatch::PendingKnowledgeRecord {
                    kind_id: kind_id.to_string(),
                    origin_system: fact.core_origin_system(),
                    payload_snapshot: snapshot,
                    recorded_at: observed_at,
                },
            );
        }

        let mut first_result: Option<(i64, ObservationSource)> = None;
        for (i, v) in vantages.iter().enumerate() {
            let player_vantage = PlayerVantage {
                player_pos: v.ref_pos,
                ruler_aboard: v.ruler_aboard,
            };
            // Cloning the fact per vantage is required because each push
            // captures it by move into the queue. Cost is dominated by
            // String fields; multi-vantage scenarios are rare today
            // (player + 0-2 NPCs) so this is acceptable.
            let f = fact.clone();
            let result = record_fact_or_local(
                f,
                origin_pos,
                observed_at,
                player_vantage.ruler_aboard,
                player_vantage.player_pos,
                &mut self.fact_queue,
                &mut self.notifications,
                &mut self.notified_ids,
                &relays,
                &comms,
            );
            if i == 0 {
                first_result = Some(result);
            }
        }
        first_result.unwrap_or((observed_at, ObservationSource::Direct))
    }
}

/// #249: Canonical entry point for world-event callsites. Combines
/// [`record_fact_or_local`] with a [`PlayerVantage`] and a [`FactSysParam`],
/// so callsites reduce to a single call (plus whatever `GameEvent` write they
/// dual-produce).
///
/// Returns `(arrives_at, source)` from the underlying scheduler for callers
/// that want to log the propagation path.
#[allow(clippy::too_many_arguments)]
pub fn record_world_event_fact(
    fact: KnowledgeFact,
    origin_pos: [f64; 3],
    observed_at: i64,
    vantage: &PlayerVantage,
    queue: &mut PendingFactQueue,
    notifications: &mut NotificationQueue,
    notified_ids: &mut NotifiedEventIds,
    relays: &[RelaySnapshot],
    comms: &CommsParams,
) -> (i64, ObservationSource) {
    record_fact_or_local(
        fact,
        origin_pos,
        observed_at,
        vantage.ruler_aboard,
        vantage.player_pos,
        queue,
        notifications,
        notified_ids,
        relays,
        comms,
    )
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
                event_id: None,
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
                event_id: None,
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
    fn record_fact_or_local_bypasses_queue_when_ruler_aboard() {
        let mut queue = PendingFactQueue::default();
        let mut notifs = crate::notifications::NotificationQueue::new();
        let mut notified = NotifiedEventIds::default();
        let comms = empty_comms();
        let fact = KnowledgeFact::CombatOutcome {
            event_id: None,
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
            &mut notified,
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
        let mut notified = NotifiedEventIds::default();
        let comms = empty_comms();
        let fact = KnowledgeFact::HostileDetected {
            event_id: None,
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
            &mut notified,
            &[],
            &comms,
        );
        assert_eq!(src, ObservationSource::Direct);
        assert_eq!(arrives_at, 50 * 60); // 50 ly × 60 hd
        assert_eq!(queue.pending_len(), 1);
        assert_eq!(notifs.items.len(), 0);
    }

    #[test]
    fn record_fact_or_local_dedupes_by_event_id_on_local_path() {
        let mut queue = PendingFactQueue::default();
        let mut notifs = crate::notifications::NotificationQueue::new();
        let mut notified = NotifiedEventIds::default();
        let comms = empty_comms();
        let eid = EventId(42);
        // Tri-state NotifiedEventIds: register before the first push.
        // Production callsites get this for free via
        // `FactSysParam::allocate_event_id`.
        notified.register(eid);

        let fact1 = KnowledgeFact::CombatOutcome {
            event_id: Some(eid),
            system: Entity::PLACEHOLDER,
            victor: CombatVictor::Player,
            detail: "first".into(),
        };
        record_fact_or_local(
            fact1,
            [0.0, 0.0, 0.0],
            0,
            false,
            [0.0, 0.0, 0.0],
            &mut queue,
            &mut notifs,
            &mut notified,
            &[],
            &comms,
        );
        assert_eq!(notifs.items.len(), 1);

        // Same id again — must NOT produce a second banner.
        let fact2 = KnowledgeFact::CombatOutcome {
            event_id: Some(eid),
            system: Entity::PLACEHOLDER,
            victor: CombatVictor::Player,
            detail: "second".into(),
        };
        record_fact_or_local(
            fact2,
            [0.0, 0.0, 0.0],
            0,
            false,
            [0.0, 0.0, 0.0],
            &mut queue,
            &mut notifs,
            &mut notified,
            &[],
            &comms,
        );
        assert_eq!(notifs.items.len(), 1, "dedupe must suppress second banner");
    }

    // #353 K-4: `drain_ready_scripted` splits Scripted vs non-Scripted
    // facts so `dispatch_knowledge_observed` consumes the Scripted subset
    // and `notify_from_knowledge_facts` handles the remainder.
    #[test]
    fn drain_ready_scripted_separates_scripted_from_core() {
        let mut q = PendingFactQueue::default();
        // Core variant (SurveyComplete) arriving at t=100.
        q.record(PerceivedFact {
            fact: KnowledgeFact::SurveyComplete {
                event_id: None,
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
        // Scripted variant arriving at t=100.
        q.record(PerceivedFact {
            fact: KnowledgeFact::Scripted {
                event_id: None,
                kind_id: "test:kind".into(),
                origin_system: None,
                payload_snapshot: super::super::payload::PayloadSnapshot::default(),
                recorded_at: 0,
            },
            observed_at: 0,
            arrives_at: 100,
            source: ObservationSource::Direct,
            origin_pos: [0.0; 3],
            related_system: None,
        });
        // Scripted variant arriving at t=200 (not yet ready).
        q.record(PerceivedFact {
            fact: KnowledgeFact::Scripted {
                event_id: None,
                kind_id: "test:kind".into(),
                origin_system: None,
                payload_snapshot: super::super::payload::PayloadSnapshot::default(),
                recorded_at: 0,
            },
            observed_at: 0,
            arrives_at: 200,
            source: ObservationSource::Direct,
            origin_pos: [0.0; 3],
            related_system: None,
        });

        // At t=150, drain_ready_scripted should return exactly 1 Scripted
        // fact (the one with arrives_at=100) and leave the core + future
        // Scripted in place.
        let ready = q.drain_ready_scripted(150);
        assert_eq!(ready.len(), 1);
        assert!(matches!(ready[0].fact, KnowledgeFact::Scripted { .. }));
        assert_eq!(q.pending_len(), 2);

        // drain_ready (legacy) now picks up only the core variant at t=150.
        let core = q.drain_ready(150);
        assert_eq!(core.len(), 1);
        assert!(matches!(core[0].fact, KnowledgeFact::SurveyComplete { .. }));

        // One future Scripted fact remains.
        assert_eq!(q.pending_len(), 1);
    }

    // --- #354 K-5: core variant -> kind id + payload converter ---

    /// `core_kind_id` returns a `core:*` id for every built-in variant
    /// and `None` for `Scripted`. The set must match `CORE_KIND_IDS`.
    #[test]
    fn core_kind_id_mapping_matches_registry() {
        use crate::knowledge::kind_registry::CORE_KIND_IDS;
        use std::collections::HashSet;

        // Build one sample of each variant so we can interrogate `core_kind_id`.
        let samples: Vec<KnowledgeFact> = vec![
            KnowledgeFact::HostileDetected {
                event_id: None,
                target: Entity::PLACEHOLDER,
                detector: Entity::PLACEHOLDER,
                target_pos: [0.0; 3],
                description: "".into(),
            },
            KnowledgeFact::CombatOutcome {
                event_id: None,
                system: Entity::PLACEHOLDER,
                victor: CombatVictor::Player,
                detail: "".into(),
            },
            KnowledgeFact::SurveyComplete {
                event_id: None,
                system: Entity::PLACEHOLDER,
                system_name: "".into(),
                detail: "".into(),
            },
            KnowledgeFact::AnomalyDiscovered {
                event_id: None,
                system: Entity::PLACEHOLDER,
                anomaly_id: "".into(),
                detail: "".into(),
            },
            KnowledgeFact::SurveyDiscovery {
                event_id: None,
                system: Entity::PLACEHOLDER,
                detail: "".into(),
            },
            KnowledgeFact::StructureBuilt {
                event_id: None,
                system: None,
                kind: "".into(),
                name: "".into(),
                destroyed: false,
                detail: "".into(),
            },
            KnowledgeFact::ColonyEstablished {
                event_id: None,
                system: Entity::PLACEHOLDER,
                planet: Entity::PLACEHOLDER,
                name: "".into(),
                detail: "".into(),
            },
            KnowledgeFact::ColonyFailed {
                event_id: None,
                system: Entity::PLACEHOLDER,
                name: "".into(),
                reason: "".into(),
            },
            KnowledgeFact::ShipArrived {
                event_id: None,
                system: None,
                name: "".into(),
                detail: "".into(),
            },
            KnowledgeFact::CoreConquered {
                event_id: None,
                system: Entity::PLACEHOLDER,
                conquered_by: Entity::PLACEHOLDER,
                original_owner: Entity::PLACEHOLDER,
                detail: "".into(),
            },
            KnowledgeFact::ShipDestroyed {
                event_id: None,
                system: Some(Entity::PLACEHOLDER),
                ship_name: "".into(),
                destroyed_at: 0,
                detail: "".into(),
            },
            KnowledgeFact::ShipMissing {
                event_id: None,
                system: Some(Entity::PLACEHOLDER),
                ship_name: "".into(),
                detail: "".into(),
            },
        ];

        let seen: HashSet<&'static str> = samples
            .iter()
            .map(|f| f.core_kind_id().expect("built-in variant has core kind id"))
            .collect();
        let expected: HashSet<&'static str> = CORE_KIND_IDS.iter().copied().collect();
        assert_eq!(
            seen, expected,
            "KnowledgeFact::core_kind_id must enumerate the same set as CORE_KIND_IDS"
        );

        // Scripted returns None.
        let scripted = KnowledgeFact::Scripted {
            event_id: None,
            kind_id: "mod:x".into(),
            origin_system: None,
            payload_snapshot: super::super::payload::PayloadSnapshot::default(),
            recorded_at: 0,
        };
        assert!(scripted.core_kind_id().is_none());
    }

    /// Converter drops `event_id`, flattens position + CombatVictor,
    /// and covers every schema field declared by the registry.
    #[test]
    fn core_payload_snapshot_contains_expected_fields_hostile() {
        use super::super::payload::PayloadValue;
        let fact = KnowledgeFact::HostileDetected {
            event_id: Some(EventId(7)),
            target: Entity::from_bits(100),
            detector: Entity::from_bits(200),
            target_pos: [1.0, 2.5, -3.25],
            description: "Saw pirate".into(),
        };
        let snap = fact.to_core_payload_snapshot().unwrap();
        // `event_id` not present.
        assert!(!snap.fields.contains_key("event_id"));
        // Fields.
        assert!(
            matches!(snap.fields.get("target"), Some(PayloadValue::Entity(bits)) if *bits == 100)
        );
        assert!(
            matches!(snap.fields.get("detector"), Some(PayloadValue::Entity(bits)) if *bits == 200)
        );
        assert!(
            matches!(snap.fields.get("target_pos_x"), Some(PayloadValue::Number(n)) if (*n - 1.0).abs() < f64::EPSILON)
        );
        assert!(
            matches!(snap.fields.get("target_pos_y"), Some(PayloadValue::Number(n)) if (*n - 2.5).abs() < f64::EPSILON)
        );
        assert!(
            matches!(snap.fields.get("target_pos_z"), Some(PayloadValue::Number(n)) if (*n + 3.25).abs() < f64::EPSILON)
        );
        assert!(
            matches!(snap.fields.get("description"), Some(PayloadValue::String(s)) if s == "Saw pirate")
        );
    }

    #[test]
    fn core_payload_snapshot_combat_victor_flattens_to_string() {
        use super::super::payload::PayloadValue;
        for (v, expected) in [
            (CombatVictor::Player, "player"),
            (CombatVictor::Hostile, "hostile"),
        ] {
            let fact = KnowledgeFact::CombatOutcome {
                event_id: None,
                system: Entity::PLACEHOLDER,
                victor: v,
                detail: "d".into(),
            };
            let snap = fact.to_core_payload_snapshot().unwrap();
            assert!(
                matches!(snap.fields.get("victor"), Some(PayloadValue::String(s)) if s == expected),
                "victor={:?} should flatten to '{}'",
                v,
                expected
            );
        }
    }

    #[test]
    fn core_payload_snapshot_skips_system_when_none() {
        // StructureBuilt / ShipArrived carry Option<Entity>; when None, the
        // converter must omit the `system` field (not emit `Nil`) so the
        // schema check upstream doesn't mis-type.
        let fact = KnowledgeFact::StructureBuilt {
            event_id: None,
            system: None,
            kind: "platform".into(),
            name: "Beacon".into(),
            destroyed: false,
            detail: "".into(),
        };
        let snap = fact.to_core_payload_snapshot().unwrap();
        assert!(!snap.fields.contains_key("system"));

        let fact_with = KnowledgeFact::StructureBuilt {
            event_id: None,
            system: Some(Entity::from_bits(42)),
            kind: "".into(),
            name: "".into(),
            destroyed: false,
            detail: "".into(),
        };
        let snap_with = fact_with.to_core_payload_snapshot().unwrap();
        assert!(snap_with.fields.contains_key("system"));
    }

    #[test]
    fn core_payload_snapshot_scripted_returns_none() {
        let fact = KnowledgeFact::Scripted {
            event_id: None,
            kind_id: "mod:x".into(),
            origin_system: None,
            payload_snapshot: super::super::payload::PayloadSnapshot::default(),
            recorded_at: 0,
        };
        assert!(fact.to_core_payload_snapshot().is_none());
    }

    /// Every schema field declared by `core_kind_catalog` must be
    /// emitted by the converter for its matching variant (when the
    /// source field is not an `Option::None`). Catches future drift
    /// between the schema list and the converter.
    #[test]
    fn core_payload_schema_matches_converter_output() {
        use crate::knowledge::kind_registry::core_kind_catalog;

        // Build one "populated" sample for each kind id so Option fields
        // resolve to Some(_).
        let samples: Vec<(&str, KnowledgeFact)> = vec![
            (
                "core:hostile_detected",
                KnowledgeFact::HostileDetected {
                    event_id: None,
                    target: Entity::from_bits(1),
                    detector: Entity::from_bits(2),
                    target_pos: [0.0; 3],
                    description: "".into(),
                },
            ),
            (
                "core:combat_outcome",
                KnowledgeFact::CombatOutcome {
                    event_id: None,
                    system: Entity::from_bits(1),
                    victor: CombatVictor::Player,
                    detail: "".into(),
                },
            ),
            (
                "core:survey_complete",
                KnowledgeFact::SurveyComplete {
                    event_id: None,
                    system: Entity::from_bits(1),
                    system_name: "".into(),
                    detail: "".into(),
                },
            ),
            (
                "core:anomaly_discovered",
                KnowledgeFact::AnomalyDiscovered {
                    event_id: None,
                    system: Entity::from_bits(1),
                    anomaly_id: "".into(),
                    detail: "".into(),
                },
            ),
            (
                "core:survey_discovery",
                KnowledgeFact::SurveyDiscovery {
                    event_id: None,
                    system: Entity::from_bits(1),
                    detail: "".into(),
                },
            ),
            (
                "core:structure_built",
                KnowledgeFact::StructureBuilt {
                    event_id: None,
                    system: Some(Entity::from_bits(1)),
                    kind: "".into(),
                    name: "".into(),
                    destroyed: false,
                    detail: "".into(),
                },
            ),
            (
                "core:colony_established",
                KnowledgeFact::ColonyEstablished {
                    event_id: None,
                    system: Entity::from_bits(1),
                    planet: Entity::from_bits(2),
                    name: "".into(),
                    detail: "".into(),
                },
            ),
            (
                "core:colony_failed",
                KnowledgeFact::ColonyFailed {
                    event_id: None,
                    system: Entity::from_bits(1),
                    name: "".into(),
                    reason: "".into(),
                },
            ),
            (
                "core:ship_arrived",
                KnowledgeFact::ShipArrived {
                    event_id: None,
                    system: Some(Entity::from_bits(1)),
                    name: "".into(),
                    detail: "".into(),
                },
            ),
            (
                "core:core_conquered",
                KnowledgeFact::CoreConquered {
                    event_id: None,
                    system: Entity::from_bits(1),
                    conquered_by: Entity::from_bits(2),
                    original_owner: Entity::from_bits(3),
                    detail: "".into(),
                },
            ),
            (
                "core:ship_destroyed",
                KnowledgeFact::ShipDestroyed {
                    event_id: None,
                    system: Some(Entity::from_bits(1)),
                    ship_name: "".into(),
                    destroyed_at: 0,
                    detail: "".into(),
                },
            ),
            (
                "core:ship_missing",
                KnowledgeFact::ShipMissing {
                    event_id: None,
                    system: Some(Entity::from_bits(1)),
                    ship_name: "".into(),
                    detail: "".into(),
                },
            ),
        ];

        for (id, fact) in samples {
            let snap = fact
                .to_core_payload_snapshot()
                .unwrap_or_else(|| panic!("no snapshot for {id}"));
            let schema = core_kind_catalog()
                .iter()
                .find(|(k, _)| *k == id)
                .unwrap_or_else(|| panic!("no schema for {id}"))
                .1;
            for (field_name, _) in schema {
                assert!(
                    snap.fields.contains_key(*field_name),
                    "kind '{id}': converter missing field '{field_name}' declared in schema"
                );
            }
        }
    }

    #[test]
    fn notified_event_ids_state_machine() {
        // Tri-state semantics: missing == treated as already-notified;
        // Some(false) == registered, first push wins; Some(true) == notified.
        let mut notified = NotifiedEventIds::default();
        let id = EventId(7);

        // Missing → try_notify returns false (no push), state still missing.
        assert!(!notified.try_notify(id));
        assert!(!notified.contains(id));

        // After register: try_notify wins exactly once.
        notified.register(id);
        assert!(notified.try_notify(id), "first push must succeed");
        assert!(!notified.try_notify(id), "second push must be suppressed");

        // sweep_notified frees the entry; re-registering returns to false.
        notified.sweep_notified();
        assert!(!notified.contains(id));
        notified.register(id);
        assert!(notified.try_notify(id), "post-sweep re-register works");

        // Explicit close removes the entry too.
        notified.close(id);
        assert!(!notified.contains(id));
    }

    // ----------------------------------------------------------------
    // Round 9 PR #1 Step 1: `FactSysParam::record_for` multi-vantage
    // routing tests. The queue is still a single Resource at this
    // step, so verifying behaviour amounts to "N vantages produce N
    // queue entries with the correct per-vantage arrival times".
    // ----------------------------------------------------------------

    use bevy::ecs::system::SystemState;
    use bevy::prelude::App;

    /// Build a minimal `App` with the resources `FactSysParam` needs.
    /// No empire entity is spawned — caller decides whether to add
    /// `CommsParams`-bearing entities.
    fn make_facts_app() -> App {
        let mut app = App::new();
        app.init_resource::<PendingFactQueue>()
            .init_resource::<NotifiedEventIds>()
            .init_resource::<NextEventId>()
            .init_resource::<RelayNetwork>()
            .insert_resource(NotificationQueue::new());
        app
    }

    fn survey_fact() -> KnowledgeFact {
        KnowledgeFact::SurveyComplete {
            event_id: None,
            system: Entity::PLACEHOLDER,
            system_name: "Vega".into(),
            detail: "Vega surveyed".into(),
        }
    }

    #[test]
    fn record_for_zero_vantages_is_noop() {
        let mut app = make_facts_app();
        let mut state: SystemState<FactSysParam> = SystemState::new(app.world_mut());
        let mut fact_sys = state.get_mut(app.world_mut());

        let result = fact_sys.record_for(survey_fact(), &[], [10.0, 0.0, 0.0], 100);
        // No-op contract: `(observed_at, Direct)` and queue stays empty.
        assert_eq!(result.0, 100);
        assert_eq!(result.1, ObservationSource::Direct);

        state.apply(app.world_mut());
        let queue = app.world().resource::<PendingFactQueue>();
        assert_eq!(queue.pending_len(), 0);
        let notifs = app.world().resource::<NotificationQueue>();
        assert!(notifs.items.is_empty());
    }

    #[test]
    fn record_for_one_vantage_matches_legacy_record() {
        let mut app = make_facts_app();
        let mut state: SystemState<FactSysParam> = SystemState::new(app.world_mut());
        let mut fact_sys = state.get_mut(app.world_mut());

        let v = FactionVantage {
            faction: Entity::PLACEHOLDER,
            ref_pos: [0.0, 0.0, 0.0],
            ruler_aboard: false,
        };
        // Origin 50ly away → 50 * 60 = 3000 hd light delay.
        let result = fact_sys.record_for(survey_fact(), &[v], [50.0, 0.0, 0.0], 0);
        assert_eq!(result.0, 3000);
        assert_eq!(result.1, ObservationSource::Direct);

        state.apply(app.world_mut());
        let queue = app.world().resource::<PendingFactQueue>();
        assert_eq!(queue.pending_len(), 1);
        assert_eq!(queue.facts[0].arrives_at, 3000);
    }

    #[test]
    fn record_for_two_vantages_pushes_per_empire_arrival_times() {
        let mut app = make_facts_app();
        let mut state: SystemState<FactSysParam> = SystemState::new(app.world_mut());
        let mut fact_sys = state.get_mut(app.world_mut());

        // Two vantages at different positions — they should observe the
        // event at different times (50 ly vs 10 ly).
        let near = FactionVantage {
            faction: Entity::PLACEHOLDER,
            ref_pos: [40.0, 0.0, 0.0], // 10 ly from origin
            ruler_aboard: false,
        };
        let far = FactionVantage {
            faction: Entity::PLACEHOLDER,
            ref_pos: [0.0, 0.0, 0.0], // 50 ly from origin
            ruler_aboard: false,
        };
        let result = fact_sys.record_for(survey_fact(), &[near, far], [50.0, 0.0, 0.0], 0);
        // Return value reflects the FIRST vantage (`near`).
        assert_eq!(result.0, 600); // 10 * 60
        assert_eq!(result.1, ObservationSource::Direct);

        state.apply(app.world_mut());
        let queue = app.world().resource::<PendingFactQueue>();
        assert_eq!(queue.pending_len(), 2);
        // Both arrival times present — order matches vantage order.
        assert_eq!(queue.facts[0].arrives_at, 600);
        assert_eq!(queue.facts[1].arrives_at, 3000);
    }

    #[test]
    fn record_for_local_path_when_ruler_aboard_skips_queue() {
        let mut app = make_facts_app();
        let mut state: SystemState<FactSysParam> = SystemState::new(app.world_mut());
        let mut fact_sys = state.get_mut(app.world_mut());

        // Ruler aboard ⇒ legacy local path: no queue push, banner instead.
        // Use a CombatOutcome (Medium / High priority) so the banner
        // actually surfaces (SurveyComplete is also Medium).
        let v = FactionVantage {
            faction: Entity::PLACEHOLDER,
            ref_pos: [0.0, 0.0, 0.0],
            ruler_aboard: true,
        };
        let fact = KnowledgeFact::CombatOutcome {
            event_id: None,
            system: Entity::PLACEHOLDER,
            victor: CombatVictor::Player,
            detail: "On-site victory".into(),
        };
        let result = fact_sys.record_for(fact, &[v], [200.0, 0.0, 0.0], 50);
        // Local path returns `(observed_at, Direct)`.
        assert_eq!(result.0, 50);
        assert_eq!(result.1, ObservationSource::Direct);

        state.apply(app.world_mut());
        let queue = app.world().resource::<PendingFactQueue>();
        assert_eq!(queue.pending_len(), 0);
        let notifs = app.world().resource::<NotificationQueue>();
        assert_eq!(notifs.items.len(), 1);
    }
}
