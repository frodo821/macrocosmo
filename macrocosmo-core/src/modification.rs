//! Shared modification contracts.
//!
//! A `Modifier` is a numeric adjustment. A `ModificationSource` is the domain
//! object or rule that explains why tags and numeric projections are active.

use std::collections::HashSet;

use crate::expr::{BoolExpr, ModifierProjection};
use crate::modified_value::Modifier;

pub type ModificationSourceId = String;
pub type ModificationId = String;
pub type ModificationTag = String;

/// Domain object or rule that can produce modification rules.
///
/// This is intentionally object-safe so game-specific types such as
/// Technology, Building, Policy, or overpopulation rules can implement it
/// without being enumerated in `macrocosmo-core`.
pub trait ModificationSource {
    fn modification_source_id(&self) -> &str;
    fn modification_source_label(&self) -> &str;
    fn collect_modification_rules(&self, sink: &mut dyn ModificationRuleSink);
}

pub trait ModificationRuleSink {
    fn push_rule(&mut self, rule: ModificationRule);
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub struct ModificationSourceRef {
    pub id: ModificationSourceId,
    pub label: String,
    pub kind: Option<String>,
}

/// A scope-level active modification instance.
///
/// `flags` are condition-visible tags granted by the active source. `modifiers`
/// are the current numeric projections of the same source. This is intentionally
/// data-only so UI and game hosts can share the same explanation model.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub struct Modification {
    pub id: ModificationId,
    pub label: String,
    pub source: ModificationSourceRef,
    pub flags: HashSet<ModificationTag>,
    pub modifiers: Vec<Modifier>,
}

impl Modification {
    pub fn grants_flag(&self, flag: &str) -> bool {
        self.flags.contains(flag)
    }

    pub fn has_modifier(&self, id: &str) -> bool {
        self.modifiers.iter().any(|m| m.id == id)
    }

    pub fn matches_modification(&self, id: &str) -> bool {
        self.id == id || self.source.id == id
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModificationRule {
    pub id: ModificationId,
    pub label: String,
    pub source: ModificationSourceRef,
    pub when: Option<BoolExpr>,
    pub tags: Vec<ModificationTag>,
    pub projections: Vec<ModifierProjection>,
}
