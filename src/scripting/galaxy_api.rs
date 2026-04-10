use bevy::prelude::*;

/// A star type definition parsed from Lua `define_star_type { ... }` calls.
#[derive(Clone, Debug)]
pub struct StarTypeDefinition {
    pub id: String,
    pub name: String,
    pub color: [f32; 3],
    pub planet_lambda: f64,
    pub max_planets: usize,
    pub habitability_bonus: f64,
    pub weight: f64,
}

/// A planet type definition parsed from Lua `define_planet_type { ... }` calls.
#[derive(Clone, Debug)]
pub struct PlanetTypeDefinition {
    pub id: String,
    pub name: String,
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

        let color_table: mlua::Table = table.get("color")?;
        let r: f32 = color_table.get("r")?;
        let g: f32 = color_table.get("g")?;
        let b: f32 = color_table.get("b")?;

        let planet_lambda: f64 = table.get("planet_lambda")?;
        let max_planets: usize = table.get("max_planets")?;
        let habitability_bonus: f64 = table.get("habitability_bonus")?;
        let weight: f64 = table.get("weight")?;

        result.push(StarTypeDefinition {
            id,
            name,
            color: [r, g, b],
            planet_lambda,
            max_planets,
            habitability_bonus,
            weight,
        });
    }

    Ok(result)
}

/// Parse planet type definitions from the Lua `_planet_type_definitions` global table.
pub fn parse_planet_types(lua: &mlua::Lua) -> Result<Vec<PlanetTypeDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_planet_type_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
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
        assert_eq!(types.len(), 5);

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
