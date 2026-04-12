use bevy::prelude::*;

/// A modifier attached to a star type, applied to each system of that type at generation.
/// Target strings follow the shared convention (e.g. "system.research_bonus",
/// "ship.shield_regen"). Unknown targets are retained but not applied — they can be
/// wired up later without script changes.
#[derive(Clone, Debug, Default)]
pub struct StarTypeModifier {
    pub target: String,
    pub base_add: f64,
    pub multiplier: f64,
    pub add: f64,
}

/// A star type definition parsed from Lua `define_star_type { ... }` calls.
#[derive(Clone, Debug)]
pub struct StarTypeDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub color: [f32; 3],
    pub planet_lambda: f64,
    pub max_planets: usize,
    pub habitability_bonus: f64,
    pub weight: f64,
    /// Modifiers applied to systems of this star type at galaxy generation.
    pub modifiers: Vec<StarTypeModifier>,
}

/// A planet type definition parsed from Lua `define_planet_type { ... }` calls.
#[derive(Clone, Debug)]
pub struct PlanetTypeDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub base_habitability: f64,
    pub base_slots: usize,
    pub resource_bias: ResourceBias,
    pub weight: f64,
}

/// Resource generation biases for a planet type.
#[derive(Clone, Debug)]
pub struct ResourceBias {
    pub minerals: f64,
    pub energy: f64,
    pub research: f64,
}

/// Registry of all star type definitions loaded from Lua scripts.
#[derive(Resource, Default)]
pub struct StarTypeRegistry {
    pub types: Vec<StarTypeDefinition>,
}

/// Registry of all planet type definitions loaded from Lua scripts.
#[derive(Resource, Default)]
pub struct PlanetTypeRegistry {
    pub types: Vec<PlanetTypeDefinition>,
}

/// Parse star type definitions from the Lua `_star_type_definitions` global table.
pub fn parse_star_types(lua: &mlua::Lua) -> Result<Vec<StarTypeDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_star_type_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let description: String = table.get::<Option<String>>("description")?.unwrap_or_default();

        let color_table: mlua::Table = table.get("color")?;
        let r: f32 = color_table.get("r")?;
        let g: f32 = color_table.get("g")?;
        let b: f32 = color_table.get("b")?;

        let planet_lambda: f64 = table.get("planet_lambda")?;
        let max_planets: usize = table.get("max_planets")?;
        let habitability_bonus: f64 = table.get("habitability_bonus")?;
        let weight: f64 = table.get("weight")?;

        let modifiers = parse_star_type_modifiers(&table)?;

        result.push(StarTypeDefinition {
            id,
            name,
            description,
            color: [r, g, b],
            planet_lambda,
            max_planets,
            habitability_bonus,
            weight,
            modifiers,
        });
    }

    Ok(result)
}

/// Parse the optional `modifiers = { { target = "...", base_add = N, ... }, ... }`
/// array on a star type definition. Returns an empty vec if the field is absent.
fn parse_star_type_modifiers(table: &mlua::Table) -> Result<Vec<StarTypeModifier>, mlua::Error> {
    let mods_value: mlua::Value = table.get("modifiers")?;
    match mods_value {
        mlua::Value::Table(mods_table) => {
            let mut modifiers = Vec::new();
            for pair in mods_table.pairs::<i64, mlua::Table>() {
                let (_, mod_table) = pair?;
                let target: String = mod_table.get("target")?;
                let base_add: f64 = mod_table.get::<Option<f64>>("base_add")?.unwrap_or(0.0);
                let multiplier: f64 = mod_table.get::<Option<f64>>("multiplier")?.unwrap_or(0.0);
                let add: f64 = mod_table.get::<Option<f64>>("add")?.unwrap_or(0.0);
                modifiers.push(StarTypeModifier {
                    target,
                    base_add,
                    multiplier,
                    add,
                });
            }
            Ok(modifiers)
        }
        mlua::Value::Nil => Ok(Vec::new()),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'modifiers' field on star type".to_string(),
        )),
    }
}

/// Parse planet type definitions from the Lua `_planet_type_definitions` global table.
pub fn parse_planet_types(lua: &mlua::Lua) -> Result<Vec<PlanetTypeDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_planet_type_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let description: String = table.get::<Option<String>>("description")?.unwrap_or_default();
        let base_habitability: f64 = table.get("base_habitability")?;
        let base_slots: usize = table.get("base_slots")?;
        let weight: f64 = table.get("weight")?;

        let bias_table: mlua::Table = table.get("resource_bias")?;
        let minerals: f64 = bias_table.get::<Option<f64>>("minerals")?.unwrap_or(0.0);
        let energy: f64 = bias_table.get::<Option<f64>>("energy")?.unwrap_or(0.0);
        let research: f64 = bias_table.get::<Option<f64>>("research")?.unwrap_or(0.0);

        result.push(PlanetTypeDefinition {
            id,
            name,
            description,
            base_habitability,
            base_slots,
            resource_bias: ResourceBias {
                minerals,
                energy,
                research,
            },
            weight,
        });
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_parse_star_types() {
        let engine = ScriptEngine::new().unwrap();

        let star_script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/stars/types.lua");
        engine.load_file(&star_script).unwrap();

        let types = parse_star_types(engine.lua()).unwrap();
        assert_eq!(types.len(), 10);

        // Verify yellow dwarf
        let yellow = types.iter().find(|t| t.id == "yellow_dwarf").unwrap();
        assert_eq!(yellow.name, "Yellow Dwarf");
        assert!((yellow.color[0] - 1.0).abs() < 1e-5);
        assert!((yellow.color[1] - 0.9).abs() < 1e-5);
        assert!((yellow.color[2] - 0.7).abs() < 1e-5);
        assert!((yellow.planet_lambda - 2.5).abs() < 1e-10);
        assert_eq!(yellow.max_planets, 8);
        assert!((yellow.habitability_bonus - 0.0).abs() < 1e-10);
        assert!((yellow.weight - 0.5).abs() < 1e-10);

        // Verify red dwarf
        let red = types.iter().find(|t| t.id == "red_dwarf").unwrap();
        assert_eq!(red.name, "Red Dwarf");
        assert!((red.habitability_bonus - (-0.2)).abs() < 1e-10);
        assert_eq!(red.max_planets, 5);

        // Verify blue giant
        let blue = types.iter().find(|t| t.id == "blue_giant").unwrap();
        assert_eq!(blue.name, "Blue Giant");
        assert!((blue.planet_lambda - 4.0).abs() < 1e-10);
        assert_eq!(blue.max_planets, 12);
    }

    #[test]
    fn test_parse_exotic_star_types() {
        let engine = ScriptEngine::new().unwrap();
        let star_script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/stars/types.lua");
        engine.load_file(&star_script).unwrap();

        let types = parse_star_types(engine.lua()).unwrap();

        // All 5 exotic types must be present and existing types preserved.
        for id in &[
            "yellow_dwarf", "red_dwarf", "blue_giant", "white_dwarf", "orange_giant",
            "neutron_star", "pulsar", "magnetar", "black_hole", "binary_star",
        ] {
            assert!(
                types.iter().any(|t| t.id == *id),
                "Expected star type '{}' to be defined",
                id
            );
        }

        // Neutron star: uninhabitable, energy + research bonuses.
        let neutron = types.iter().find(|t| t.id == "neutron_star").unwrap();
        assert_eq!(neutron.name, "Neutron Star");
        assert!(neutron.habitability_bonus <= -1.0 + 1e-10);
        assert!(!neutron.modifiers.is_empty());
        assert!(neutron
            .modifiers
            .iter()
            .any(|m| m.target == "system.energy_potential" && m.multiplier > 0.0));

        // Pulsar: FTL/comm disruption + anomaly bonus.
        let pulsar = types.iter().find(|t| t.id == "pulsar").unwrap();
        assert!(pulsar
            .modifiers
            .iter()
            .any(|m| m.target == "system.ftl_range" && m.multiplier < 0.0));
        assert!(pulsar
            .modifiers
            .iter()
            .any(|m| m.target == "system.anomaly_chance" && m.multiplier > 0.0));

        // Magnetar: shield-disabling modifiers.
        let magnetar = types.iter().find(|t| t.id == "magnetar").unwrap();
        assert!(magnetar
            .modifiers
            .iter()
            .any(|m| m.target == "ship.shield_max" && m.multiplier < 0.0));
        assert!(magnetar
            .modifiers
            .iter()
            .any(|m| m.target == "ship.shield_regen"));

        // Black hole: no planets, FTL research bonus.
        let bh = types.iter().find(|t| t.id == "black_hole").unwrap();
        assert_eq!(bh.max_planets, 0);
        assert!(bh.planet_lambda <= 1e-10);
        assert!(bh
            .modifiers
            .iter()
            .any(|m| m.target.contains("research") && m.multiplier > 0.0));

        // Binary star: mineral + energy bonuses, habitability penalty.
        let binary = types.iter().find(|t| t.id == "binary_star").unwrap();
        assert!(binary.habitability_bonus < 0.0);
        assert!(binary
            .modifiers
            .iter()
            .any(|m| m.target == "system.mineral_richness" && m.multiplier > 0.0));

        // Vanilla types must still have no modifiers (unchanged).
        let yellow = types.iter().find(|t| t.id == "yellow_dwarf").unwrap();
        assert!(yellow.modifiers.is_empty());
    }

    #[test]
    fn test_parse_star_type_modifiers_empty_when_absent() {
        let lua = mlua::Lua::new();
        let table = lua.create_table().unwrap();
        let result = parse_star_type_modifiers(&table).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_star_type_modifiers_fields() {
        let lua = mlua::Lua::new();
        let table = lua.create_table().unwrap();
        let mods = lua.create_table().unwrap();

        let m1 = lua.create_table().unwrap();
        m1.set("target", "system.research_bonus").unwrap();
        m1.set("multiplier", 0.5f64).unwrap();
        mods.set(1, m1).unwrap();

        let m2 = lua.create_table().unwrap();
        m2.set("target", "ship.shield_max").unwrap();
        m2.set("base_add", -10.0f64).unwrap();
        m2.set("add", 2.0f64).unwrap();
        mods.set(2, m2).unwrap();

        table.set("modifiers", mods).unwrap();

        let parsed = parse_star_type_modifiers(&table).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].target, "system.research_bonus");
        assert!((parsed[0].multiplier - 0.5).abs() < 1e-10);
        assert!((parsed[0].base_add - 0.0).abs() < 1e-10);
        assert_eq!(parsed[1].target, "ship.shield_max");
        assert!((parsed[1].base_add - (-10.0)).abs() < 1e-10);
        assert!((parsed[1].add - 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_parse_planet_types() {
        let engine = ScriptEngine::new().unwrap();

        let planet_script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/planets/types.lua");
        engine.load_file(&planet_script).unwrap();

        let types = parse_planet_types(engine.lua()).unwrap();
        assert_eq!(types.len(), 5);

        // Verify terrestrial
        let terr = types.iter().find(|t| t.id == "terrestrial").unwrap();
        assert_eq!(terr.name, "Terrestrial");
        assert!((terr.base_habitability - 0.7).abs() < 1e-10);
        assert_eq!(terr.base_slots, 4);
        assert!((terr.resource_bias.minerals - 1.0).abs() < 1e-10);
        assert!((terr.resource_bias.energy - 0.8).abs() < 1e-10);
        assert!((terr.resource_bias.research - 0.5).abs() < 1e-10);
        assert!((terr.weight - 0.4).abs() < 1e-10);

        // Verify gas giant
        let gas = types.iter().find(|t| t.id == "gas_giant").unwrap();
        assert_eq!(gas.name, "Gas Giant");
        assert!((gas.base_habitability - 0.0).abs() < 1e-10);
        assert_eq!(gas.base_slots, 0);
        assert!((gas.resource_bias.minerals - 0.0).abs() < 1e-10);

        // Verify ocean world
        let ocean = types.iter().find(|t| t.id == "ocean").unwrap();
        assert_eq!(ocean.name, "Ocean World");
        assert!((ocean.resource_bias.research - 1.2).abs() < 1e-10);
    }

    #[test]
    fn test_poisson_sample() {
        use crate::galaxy::poisson_sample;
        let mut rng = rand::rng();

        // Test with lambda=2.5, max=8
        let n = 10000;
        let mut sum = 0usize;
        let mut all_in_range = true;
        for _ in 0..n {
            let val = poisson_sample(&mut rng, 2.5, 8);
            sum += val;
            if val < 1 || val > 8 {
                all_in_range = false;
            }
        }
        assert!(all_in_range, "All samples should be in [1, 8]");
        let mean = sum as f64 / n as f64;
        // Mean of clamped Poisson(2.5) should be roughly 2.5 (clamping at 1 pushes it slightly up)
        assert!(
            mean > 2.0 && mean < 3.5,
            "Mean {} should be close to 2.5",
            mean
        );

        // Test with lambda=1.0, max=3
        let mut sum = 0usize;
        for _ in 0..n {
            let val = poisson_sample(&mut rng, 1.0, 3);
            sum += val;
            assert!(val >= 1 && val <= 3);
        }
        let mean = sum as f64 / n as f64;
        assert!(
            mean > 1.0 && mean < 2.5,
            "Mean {} should be close to 1.0 (clamped)",
            mean
        );
    }
}
