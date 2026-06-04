use std::collections::HashSet;

use bevy::prelude::{Component, ReflectComponent};
use bevy::reflect::Reflect;

pub use macrocosmo_core::modified_value::{ModifiedValue, Modifier};
pub use macrocosmo_core::{
    CachedValue, Modification, ModificationSource, ModificationSourceRef, ParsedModifier,
    ScopedModifiers,
};

const LEGACY_FLAG_SOURCE_ID: &str = "legacy.scoped_flag";
const LEGACY_FLAG_SOURCE_LABEL: &str = "Scoped flag";
const LEGACY_FLAG_MODIFICATION_PREFIX: &str = "legacy.scoped_flag:";

/// Scope-local modifications visible to condition evaluation.
///
/// This replaces the conceptual role of `ScopedFlags`: boolean flags are now
/// just one projection of active scope modifications. Legacy flag mutation is
/// represented as synthetic `Modification` entries so `Modification.flags`
/// remains the single source of truth.
#[derive(Component, Default, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct ScopedModifications {
    pub modifications: Vec<Modification>,
}

impl ScopedModifications {
    pub fn set(&mut self, flag: &str) {
        let modification = Modification {
            id: legacy_flag_modification_id(flag),
            label: flag.to_string(),
            source: ModificationSourceRef {
                id: LEGACY_FLAG_SOURCE_ID.to_string(),
                label: LEGACY_FLAG_SOURCE_LABEL.to_string(),
                kind: Some("legacy_flag".to_string()),
            },
            flags: HashSet::from([flag.to_string()]),
            modifiers: Vec::new(),
        };
        self.push_modification(modification);
    }

    pub fn unset(&mut self, flag: &str) -> bool {
        self.pop_modification(&legacy_flag_modification_id(flag))
            .is_some()
    }

    pub fn check(&self, flag: &str) -> bool {
        self.modifications.iter().any(|m| m.grants_flag(flag))
    }

    pub fn iter_flags(&self) -> impl Iterator<Item = &String> {
        self.modifications
            .iter()
            .flat_map(|modification| modification.flags.iter())
    }

    pub fn flag_set(&self) -> HashSet<String> {
        self.iter_flags().cloned().collect()
    }

    pub fn push_modification(&mut self, modification: Modification) {
        if let Some(existing) = self
            .modifications
            .iter_mut()
            .find(|existing| existing.id == modification.id)
        {
            *existing = modification;
        } else {
            self.modifications.push(modification);
        }
    }

    pub fn pop_modification(&mut self, id: &str) -> Option<Modification> {
        let pos = self.modifications.iter().position(|m| m.id == id)?;
        Some(self.modifications.remove(pos))
    }

    pub fn has_modification(&self, id: &str) -> bool {
        self.modifications
            .iter()
            .any(|m| m.matches_modification(id))
    }

    pub fn has_projected_modifier(&self, id: &str) -> bool {
        self.modifications.iter().any(|m| m.has_modifier(id))
    }

    pub fn active_modifier_ids(&self) -> HashSet<String> {
        self.modifications
            .iter()
            .flat_map(|m| m.modifiers.iter().map(|modifier| modifier.id.clone()))
            .collect()
    }

    pub fn active_modification_ids(&self) -> HashSet<String> {
        self.modifications
            .iter()
            .flat_map(|m| [m.id.clone(), m.source.id.clone()])
            .collect()
    }
}

fn legacy_flag_modification_id(flag: &str) -> String {
    format!("{LEGACY_FLAG_MODIFICATION_PREFIX}{flag}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modification_with_flag(id: &str, flag: &str) -> Modification {
        Modification {
            id: id.to_string(),
            label: id.to_string(),
            source: ModificationSourceRef {
                id: format!("source:{id}"),
                label: format!("Source {id}"),
                kind: None,
            },
            flags: HashSet::from([flag.to_string()]),
            modifiers: Vec::new(),
        }
    }

    #[test]
    fn scoped_modifications_flags_are_derived_from_modifications() {
        let mut scope = ScopedModifications::default();
        scope.push_modification(modification_with_flag("policy.a", "mobilized"));

        assert!(scope.check("mobilized"));
        assert_eq!(scope.flag_set(), HashSet::from(["mobilized".to_string()]));
    }

    #[test]
    fn scoped_modifications_legacy_set_creates_modification() {
        let mut scope = ScopedModifications::default();
        scope.set("first_contact");

        assert!(scope.check("first_contact"));
        assert!(scope.has_modification("legacy.scoped_flag:first_contact"));
        assert_eq!(scope.modifications.len(), 1);
        assert_eq!(
            scope.modifications[0].flags,
            HashSet::from(["first_contact".to_string()])
        );
    }

    #[test]
    fn scoped_modifications_unset_removes_only_legacy_flag_modification() {
        let mut scope = ScopedModifications::default();
        scope.set("shared_flag");
        scope.push_modification(modification_with_flag("technology.a", "shared_flag"));

        assert!(scope.unset("shared_flag"));
        assert!(scope.check("shared_flag"));
        assert!(!scope.has_modification("legacy.scoped_flag:shared_flag"));
        assert!(scope.has_modification("technology.a"));
    }
}
