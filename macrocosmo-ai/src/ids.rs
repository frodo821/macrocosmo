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
