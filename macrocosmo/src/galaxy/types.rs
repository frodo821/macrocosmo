use bevy::prelude::*;

use crate::scripting::galaxy_api::{
    BiomeLuaDefinition, PlanetTypeDefinition, PlanetTypeRegistry, ResourceBias, StarTypeDefinition,
    StarTypeRegistry,
};

use super::biome::{BiomeDefinition, BiomeRegistry};

/// Hardcoded fallback star types when no Lua definitions are loaded.
pub(crate) fn default_star_types() -> Vec<StarTypeDefinition> {
    vec![StarTypeDefinition {
        id: "default".to_string(),
        name: "Star".to_string(),
        description: String::new(),
        color: [1.0, 1.0, 0.9],
        planet_lambda: 2.0,
        max_planets: 3,
        habitability_bonus: 0.0,
        weight: 1.0,
        modifiers: Vec::new(),
    }]
}

/// Hardcoded fallback planet types when no Lua definitions are loaded.
pub(crate) fn default_planet_types() -> Vec<PlanetTypeDefinition> {
    vec![PlanetTypeDefinition {
        id: "default".to_string(),
        name: "Planet".to_string(),
        description: String::new(),
        base_habitability: 0.5,
        base_slots: 4,
        resource_bias: ResourceBias {
            minerals: 1.0,
            energy: 1.0,
            research: 1.0,
        },
        weight: 1.0,
        default_biome: None,
    }]
}

/// #335: Startup system that parses biome definitions from the
/// `_biome_definitions` Lua accumulator into [`BiomeRegistry`]. Runs after
/// `load_all_scripts`. Always ensures a `"default"` biome exists so planet
/// lookups fall back cleanly.
pub fn load_biome_registry(
    engine: Res<crate::scripting::ScriptEngine>,
    mut registry: ResMut<BiomeRegistry>,
) {
    match crate::scripting::galaxy_api::parse_biomes(engine.lua()) {
        Ok(biomes) => {
            let count = biomes.len();
            for b in biomes {
                let BiomeLuaDefinition {
                    id,
                    display_name,
                    description,
                } = b;
                registry.insert(BiomeDefinition {
                    id,
                    display_name,
                    description,
                });
            }
            info!("Loaded {} biome definitions from Lua", count);
        }
        Err(e) => {
            warn!("Failed to parse biome definitions: {e}");
        }
    }
    registry.ensure_default();
}

/// Startup system that parses star and planet type definitions from Lua accumulators.
/// Scripts are loaded by `load_all_scripts`; this system only parses the results.
pub fn load_galaxy_types(
    engine: Res<crate::scripting::ScriptEngine>,
    mut star_registry: ResMut<StarTypeRegistry>,
    mut planet_registry: ResMut<PlanetTypeRegistry>,
) {
    match crate::scripting::galaxy_api::parse_star_types(engine.lua()) {
        Ok(types) => {
            info!("Loaded {} star type definitions", types.len());
            star_registry.types = types;
        }
        Err(e) => {
            warn!("Failed to parse star types: {e}");
        }
    }

    match crate::scripting::galaxy_api::parse_planet_types(engine.lua()) {
        Ok(types) => {
            info!("Loaded {} planet type definitions", types.len());
            planet_registry.types = types;
        }
        Err(e) => {
            warn!("Failed to parse planet types: {e}");
        }
    }
}
