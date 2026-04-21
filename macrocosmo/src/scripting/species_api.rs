use bevy::log::warn;

use crate::scripting::modifier_api::parse_parsed_modifiers;
use crate::species::{JobDefinition, SpeciesDefinition};

/// Parse species definitions from the Lua `_species_definitions` global table.
pub fn parse_species_definitions(lua: &mlua::Lua) -> Result<Vec<SpeciesDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_species_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let description: String = table
            .get::<Option<String>>("description")?
            .unwrap_or_default();
        let growth_rate: f64 = table.get::<Option<f64>>("growth_rate")?.unwrap_or(0.01);

        // #241: Legacy `job_bonuses` field is warn-then-ignored. Use `modifiers` with
        // job-scoped targets instead, e.g. `job:miner::colony.minerals_per_hexadies`.
        if matches!(
            table.get::<mlua::Value>("job_bonuses")?,
            mlua::Value::Table(_)
        ) {
            warn!(
                "Species '{}' uses legacy `job_bonuses` field; ignored. Migrate to modifiers \
                 with target = \"job:<job_id>::colony.<resource>_per_hexadies\" (#241).",
                id
            );
        }

        // New: modifiers array.
        let modifiers = parse_parsed_modifiers(&table, "modifiers", None)?;

        result.push(SpeciesDefinition {
            id,
            name,
            description,
            base_growth_rate: growth_rate,
            modifiers,
        });
    }

    Ok(result)
}

/// Parse job definitions from the Lua `_job_definitions` global table.
pub fn parse_job_definitions(lua: &mlua::Lua) -> Result<Vec<JobDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_job_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let label: String = table.get("label")?;
        let description: String = table
            .get::<Option<String>>("description")?
            .unwrap_or_default();

        // #241: Legacy `base_output` field is warn-then-ignored. Use `modifiers`
        // instead, e.g. `{ target = "colony.minerals_per_hexadies", base_add = 0.6 }`.
        if matches!(
            table.get::<mlua::Value>("base_output")?,
            mlua::Value::Table(_)
        ) {
            warn!(
                "Job '{}' uses legacy `base_output` field; ignored. Migrate to modifiers \
                 with target = \"colony.<resource>_per_hexadies\" (#241).",
                id
            );
        }

        // New: modifiers array, with auto-prefix applied (`colony.x` →
        // `job:<id>::colony.x` when the Lua author didn't write the prefix).
        let modifiers = parse_parsed_modifiers(&table, "modifiers", Some(&id))?;

        result.push(JobDefinition {
            id,
            label,
            description,
            modifiers,
        });
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_parse_species_definitions() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_species {
                id = "human",
                name = "Human",
                growth_rate = 0.01,
                modifiers = {
                    { target = "job:researcher::colony.research_per_hexadies", multiplier = 0.1 },
                },
            }
            define_species {
                id = "alien",
                name = "Alien",
                growth_rate = 0.02,
                modifiers = {
                    { target = "job:miner::colony.minerals_per_hexadies", multiplier = 0.2 },
                },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_species_definitions(lua).unwrap();
        assert_eq!(defs.len(), 2);

        // Human
        assert_eq!(defs[0].id, "human");
        assert_eq!(defs[0].name, "Human");
        assert!((defs[0].base_growth_rate - 0.01).abs() < 1e-10);
        assert_eq!(defs[0].modifiers.len(), 1);
        assert_eq!(
            defs[0].modifiers[0].target,
            "job:researcher::colony.research_per_hexadies"
        );
        assert!((defs[0].modifiers[0].multiplier - 0.1).abs() < 1e-10);

        // Alien
        assert_eq!(defs[1].id, "alien");
        assert!((defs[1].base_growth_rate - 0.02).abs() < 1e-10);
        assert_eq!(defs[1].modifiers.len(), 1);
        assert_eq!(
            defs[1].modifiers[0].target,
            "job:miner::colony.minerals_per_hexadies"
        );
    }

    #[test]
    fn test_parse_species_minimal() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_species {
                id = "basic",
                name = "Basic Species",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_species_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "basic");
        assert_eq!(defs[0].name, "Basic Species");
        assert!((defs[0].base_growth_rate - 0.01).abs() < 1e-10); // default
        assert!(defs[0].modifiers.is_empty());
    }

    #[test]
    fn test_parse_job_definitions() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_job {
                id = "miner",
                label = "Miner",
                modifiers = { { target = "colony.minerals_per_hexadies", base_add = 0.6 } },
            }
            define_job {
                id = "farmer",
                label = "Farmer",
                modifiers = { { target = "colony.food_per_hexadies", base_add = 1.0 } },
            }
            define_job {
                id = "researcher",
                label = "Researcher",
                modifiers = { { target = "colony.research_per_hexadies", base_add = 0.5 } },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_job_definitions(lua).unwrap();
        assert_eq!(defs.len(), 3);

        // Miner — prefix-less `colony.x` auto-prefixes with `job:miner::`
        assert_eq!(defs[0].id, "miner");
        assert_eq!(defs[0].label, "Miner");
        assert_eq!(defs[0].modifiers.len(), 1);
        assert_eq!(
            defs[0].modifiers[0].target,
            "job:miner::colony.minerals_per_hexadies"
        );
        assert!((defs[0].modifiers[0].base_add - 0.6).abs() < 1e-10);

        // Farmer
        assert_eq!(defs[1].id, "farmer");
        assert_eq!(
            defs[1].modifiers[0].target,
            "job:farmer::colony.food_per_hexadies"
        );
        assert!((defs[1].modifiers[0].base_add - 1.0).abs() < 1e-10);

        // Researcher
        assert_eq!(defs[2].id, "researcher");
        assert_eq!(
            defs[2].modifiers[0].target,
            "job:researcher::colony.research_per_hexadies"
        );
    }

    #[test]
    fn test_parse_job_minimal() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_job {
                id = "idle",
                label = "Idle",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_job_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "idle");
        assert_eq!(defs[0].label, "Idle");
        assert!(defs[0].modifiers.is_empty());
    }

    /// Test loading species definitions from the actual scripts/species/human.lua file.
    #[test]
    fn test_species_registry_loaded_from_lua() {
        let engine = ScriptEngine::new().unwrap();

        let species_script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/species/human.lua");
        if !species_script.exists() {
            panic!(
                "scripts/species/human.lua not found at {:?}",
                species_script
            );
        }
        engine.load_file(&species_script).unwrap();

        let defs = parse_species_definitions(engine.lua()).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "human");
        assert_eq!(defs[0].name, "Human");
        assert!((defs[0].base_growth_rate - 0.01).abs() < 1e-10);
        // #241: species uses modifiers instead of job_bonuses
        // Human should have at least one species-wide modifier.
        assert!(
            !defs[0].modifiers.is_empty(),
            "expected human species to define modifiers"
        );
    }

    /// Test loading job definitions from the actual scripts/jobs/basic.lua file.
    #[test]
    fn test_job_registry_loaded_from_lua() {
        let engine = ScriptEngine::new().unwrap();

        let jobs_script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/jobs/basic.lua");
        if !jobs_script.exists() {
            panic!("scripts/jobs/basic.lua not found at {:?}", jobs_script);
        }
        engine.load_file(&jobs_script).unwrap();

        let defs = parse_job_definitions(engine.lua()).unwrap();
        assert!(defs.len() >= 4, "Expected at least 4 job definitions");

        let ids: Vec<&str> = defs.iter().map(|d| d.id.as_str()).collect();
        assert!(ids.contains(&"miner"));
        assert!(ids.contains(&"farmer"));
        assert!(ids.contains(&"researcher"));
        assert!(ids.contains(&"power_worker"));
    }
}
