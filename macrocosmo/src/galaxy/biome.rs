//! #335: Biome component + `define_biome` Lua API.
//!
//! Replaces the `PlanetView.biome = planet_type` placeholder introduced by
//! PR #329 (β Lua View types, #289 §11 "Out of scope" item 1) with a real
//! biome concept: every Planet entity carries a [`Biome`] component whose
//! id resolves to a [`BiomeDefinition`] in the [`BiomeRegistry`].
//!
//! # Scope
//!
//! This issue is *definition + component plumbing only*. It deliberately does
//! not touch production / habitability / colonisation gates or terraforming
//! transitions — those land in separate issues.
//!
//! # Fallback
//!
//! If a `PlanetTypeDefinition` has no `default_biome` — or if the referenced
//! biome id is absent from the registry — the planet gets [`DEFAULT_BIOME_ID`]
//! (`"default"`). Scripts are free to `define_biome { id = "default", ... }`
//! to customise the fallback's display name; if they don't, an implicit
//! fallback is registered automatically at load time so `PlanetView.biome`
//! is always resolvable against the registry.
//!
//! # Save compatibility
//!
//! Biome component is persisted via a new `biome: Option<SavedBiome>` field
//! on `SavedComponentBag`; `SAVE_VERSION` bumps in lockstep (2 → 3) because
//! postcard wire format is sequential.

use bevy::prelude::*;
use std::collections::HashMap;

/// Id used when a planet_type has no `default_biome` reference (or it fails
/// to resolve against the [`BiomeRegistry`]).
pub const DEFAULT_BIOME_ID: &str = "default";

/// A biome definition parsed from Lua `define_biome { ... }` calls.
#[derive(Clone, Debug, bevy::reflect::Reflect)]
pub struct BiomeDefinition {
    pub id: String,
    pub display_name: String,
    pub description: String,
}

/// Registry of all biome definitions loaded from Lua scripts.
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct BiomeRegistry {
    pub biomes: HashMap<String, BiomeDefinition>,
}

impl BiomeRegistry {
    pub fn get(&self, id: &str) -> Option<&BiomeDefinition> {
        self.biomes.get(id)
    }

    /// Insert a definition into the registry. Used by `load_biome_registry`
    /// when parsing Lua and by tests when priming a registry directly.
    pub fn insert(&mut self, def: BiomeDefinition) {
        self.biomes.insert(def.id.clone(), def);
    }

    /// Ensure a "default" biome is always available so that planets with
    /// no explicit default_biome can resolve to something.
    pub fn ensure_default(&mut self) {
        if !self.biomes.contains_key(DEFAULT_BIOME_ID) {
            self.insert(BiomeDefinition {
                id: DEFAULT_BIOME_ID.to_string(),
                display_name: "Default".to_string(),
                description: String::new(),
            });
        }
    }
}

/// Component attached to every Planet entity recording its current biome.
///
/// For pre-alpha this is purely descriptive — no derived production or
/// habitability bonuses (those are explicitly out of scope; see #335).
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component)]
pub struct Biome {
    pub id: String,
}

impl Biome {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

/// Resolve the biome id for a planet_type, honouring `default_biome` if
/// present on the definition, or returning [`DEFAULT_BIOME_ID`] otherwise.
///
/// The returned id is *not* validated against [`BiomeRegistry`] — the
/// caller is expected to check with [`BiomeRegistry::get`] and fall back
/// to [`DEFAULT_BIOME_ID`] if resolution fails.
pub fn resolve_default_biome_id(default_biome: Option<&str>) -> String {
    default_biome
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_BIOME_ID)
        .to_string()
}

/// Given a planet_type's `default_biome` field and the current
/// [`BiomeRegistry`], return the biome id to attach. Falls back to
/// [`DEFAULT_BIOME_ID`] when the referenced biome is not registered.
pub fn resolve_biome_id(default_biome: Option<&str>, registry: &BiomeRegistry) -> String {
    let candidate = resolve_default_biome_id(default_biome);
    if registry.biomes.contains_key(&candidate) {
        candidate
    } else {
        DEFAULT_BIOME_ID.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_default_biome_id_uses_fallback_when_none() {
        assert_eq!(resolve_default_biome_id(None), DEFAULT_BIOME_ID);
        assert_eq!(resolve_default_biome_id(Some("")), DEFAULT_BIOME_ID);
        assert_eq!(resolve_default_biome_id(Some("temperate")), "temperate");
    }

    #[test]
    fn resolve_biome_id_falls_back_when_unknown() {
        let mut reg = BiomeRegistry::default();
        reg.insert(BiomeDefinition {
            id: "temperate".into(),
            display_name: "Temperate".into(),
            description: String::new(),
        });
        reg.ensure_default();

        assert_eq!(resolve_biome_id(Some("temperate"), &reg), "temperate");
        assert_eq!(resolve_biome_id(Some("unknown"), &reg), DEFAULT_BIOME_ID);
        assert_eq!(resolve_biome_id(None, &reg), DEFAULT_BIOME_ID);
    }

    #[test]
    fn ensure_default_is_idempotent_and_does_not_clobber() {
        let mut reg = BiomeRegistry::default();
        reg.insert(BiomeDefinition {
            id: DEFAULT_BIOME_ID.into(),
            display_name: "Custom Default".into(),
            description: "Script-provided fallback".into(),
        });
        reg.ensure_default();
        let d = reg.get(DEFAULT_BIOME_ID).unwrap();
        assert_eq!(d.display_name, "Custom Default");
        assert_eq!(d.description, "Script-provided fallback");
    }
}
