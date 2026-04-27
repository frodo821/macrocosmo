//! Strongly-typed ID newtypes for the AI bus.
//!
//! String-based IDs use `Arc<str>` for cheap clones and stable hashing.
//! Numeric IDs are opaque newtypes — the integration layer in `macrocosmo/src/ai/`
//! (#203) is responsible for translating these to/from engine types
//! (`macrocosmo::faction::FactionId`, Bevy `Entity`, etc.).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Opaque faction identifier. The integration layer maps this to the engine's
/// canonical faction identifier (e.g., a `u32` derived from a `FactionTypeDefinition`
/// or an `Entity` index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FactionId(pub u32);

impl From<u32> for FactionId {
    fn from(v: u32) -> Self {
        Self(v)
    }
}

/// Reference to a faction: either the observing faction itself, or a specific
/// other faction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FactionRef {
    /// "self" in the observing context.
    Me,
    Other(FactionId),
}

impl From<FactionId> for FactionRef {
    fn from(id: FactionId) -> Self {
        FactionRef::Other(id)
    }
}

/// Opaque handle to a star system. Integration layer maps this to Bevy `Entity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SystemRef(pub u64);

/// Opaque handle to an arbitrary game entity (ship, colony, structure, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityRef(pub u64);

/// Helper: declare an `Arc<str>`-backed string newtype used for topic IDs and
/// similar identifiers. Derives the common traits and provides `From<&str>`,
/// `From<String>`, `Deref<Target = str>`.
macro_rules! arc_str_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(Arc<str>);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(Arc::from(s))
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(Arc::from(s.into_boxed_str()))
            }
        }

        impl From<Arc<str>> for $name {
            fn from(s: Arc<str>) -> Self {
                Self(s)
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

arc_str_id! {
    /// Identifier for a metric topic (e.g. `"fleet_readiness"`).
    MetricId
}

arc_str_id! {
    /// Identifier for a command kind (e.g. `"attack_target"`).
    CommandKindId
}

arc_str_id! {
    /// Identifier for an evidence kind (e.g. `"hostile_engagement"`).
    EvidenceKindId
}

arc_str_id! {
    /// Identifier for an objective (e.g. `"defensive_posture"`).
    ObjectiveId
}

arc_str_id! {
    /// Identifier for an intent (e.g. `"attack_target"`, `"expand_to_system"`).
    ///
    /// Intents are concrete action dispatches derived from an Objective; in
    /// Assessment they are keyed for per-intent precondition summaries.
    IntentId
}

arc_str_id! {
    /// Open-kind identifier for an intent kind (e.g. `"pursue_metric"`,
    /// `"fortify"`, `"steer_crisis"`). Game / scenario layer defines its
    /// own vocabulary; `macrocosmo-ai` passes these through without
    /// interpreting them.
    IntentKindId
}

arc_str_id! {
    /// Open-kind address an intent is destined for (e.g. `"faction"`,
    /// `"sector:alpha"`, `"fleet:42"`). Resolved to a concrete Mid-term
    /// agent instance by the integration layer.
    IntentTargetRef
}

arc_str_id! {
    /// Open-kind delivery-mechanism hint (e.g. `"urgent"`, `"routine"`,
    /// `"best_effort"`). `IntentDispatcher` impls may honor or ignore.
    DeliveryHintId
}

arc_str_id! {
    /// Open-kind context label for a short-term agent instance (e.g.
    /// `"fleet:42"`, `"colony:sol"`, `"faction"`). Lets multiple short
    /// agents within the same faction address themselves without macrocosmo-ai
    /// knowing the physical model.
    ShortContext
}

arc_str_id! {
    /// Open-kind label for a Mid-term agent stance extension. Used as
    /// the payload of `Stance::Custom` so Lua / scenario layers can
    /// register stances beyond the four core variants without touching
    /// the enum. The four core variants stay typed so the orchestrator
    /// (and tests) can `match` exhaustively on the common case.
    StanceId
}

arc_str_id! {
    /// Placeholder identifier for the region a Mid-term agent is bound
    /// to. Today every faction has a single empire-wide Mid agent so
    /// `MidTermState::region_id` is always `None`; the type exists to
    /// reserve the binding shape for the multi-Mid split landing in
    /// #449.
    RegionId
}

arc_str_id! {
    /// Placeholder for one axis of a faction's victory decomposition
    /// (e.g. `"economic"`, `"military"`). Used as the key shape of
    /// `LongTermState::victory_progress`; concrete axis schema TBD in
    /// #449.
    VictoryAxisId
}

arc_str_id! {
    /// Placeholder tag for the empire-level campaign phase carried in
    /// `LongTermState::current_campaign_phase`. Free-form today;
    /// promotion to a typed enum is deferred to #449 (mirrors the
    /// `Stance` extension hook in the Mid layer).
    CampaignPhase
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_str_id_equality_across_constructors() {
        let a = MetricId::from("fleet_readiness");
        let b = MetricId::from(String::from("fleet_readiness"));
        assert_eq!(a, b);
        assert_eq!(a.as_str(), "fleet_readiness");
        assert_eq!(&*a, "fleet_readiness");
    }

    #[test]
    fn faction_ref_from_faction_id() {
        let fid = FactionId(7);
        let r: FactionRef = fid.into();
        assert_eq!(r, FactionRef::Other(FactionId(7)));
    }

    #[test]
    fn ids_are_hashable_in_maps() {
        use std::collections::HashMap;
        let mut m: HashMap<MetricId, i32> = HashMap::new();
        m.insert(MetricId::from("a"), 1);
        m.insert(MetricId::from("b"), 2);
        assert_eq!(m.get(&MetricId::from("a")), Some(&1));
    }
}
