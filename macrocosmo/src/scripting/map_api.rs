//! #182: Predefined star systems + Map Type Lua API.
//!
//! Adds two orthogonal Lua definitions on top of the #181 galaxy-generation
//! hook framework:
//!
//! - `define_predefined_system { ... }` registers a fixed star system that
//!   a `define_map_type` generator can spawn verbatim via
//!   `ctx:spawn_predefined_system(id)`.
//! - `define_map_type { id, name, description, generator = function(ctx) ... end }`
//!   registers a named map layout. `set_active_map_type(id)` marks one as
//!   active; when `generate_galaxy` runs, the active map type's generator
//!   is used in place of `on_galaxy_generate_empty` / the default spiral.
//!
//! Both registries are keyed by id and live in separate Bevy resources:
//! - [`PredefinedSystemRegistry`] — parsed from `_predefined_system_definitions`
//! - [`MapTypeRegistry`] — parsed from `_map_type_definitions`, plus a
//!   `current: Option<String>` that the Lua `set_active_map_type` helper
//!   writes into `_active_map_type` for the Rust side to read.
//!
//! The actual hookup into `generate_galaxy` lives in `galaxy/generation.rs`
//! and the Lua ctx method on `GalaxyGenerateCtx` in `galaxy_gen_ctx.rs`.

use bevy::prelude::*;
use std::collections::HashMap;

/// Optional per-attribute overrides for a predefined planet. All fields are
/// optional; missing fields fall back to the planet type's defaults when the
/// system is spawned (mirrors the #181 `on_initialize_system` override path).
#[derive(Clone, Debug, Default, PartialEq, bevy::reflect::Reflect)]
pub struct PlanetAttributesSpec {
    pub habitability: Option<f64>,
    pub mineral_richness: Option<f64>,
    pub energy_potential: Option<f64>,
    pub research_potential: Option<f64>,
    pub max_building_slots: Option<u8>,
}

/// A planet inside a `define_predefined_system { ... }` block.
#[derive(Clone, Debug, bevy::reflect::Reflect)]
pub struct PredefinedPlanetSpec {
    pub name: String,
    pub planet_type_id: String,
    pub attrs: PlanetAttributesSpec,
}

/// A predefined star system definition (e.g. "sol" with Mercury/Venus/Earth).
#[derive(Clone, Debug, bevy::reflect::Reflect)]
pub struct PredefinedSystemDefinition {
    pub id: String,
    pub name: String,
    pub position: [f64; 3],
    pub star_type_id: String,
    pub planets: Vec<PredefinedPlanetSpec>,
    /// Optional hint used by the default `on_choose_capitals` path: if a
    /// predefined system carries a `capital_for_faction`, its spawn in
    /// Phase A is tagged so Phase B can prefer it as that faction's capital.
    pub capital_for_faction: Option<String>,
}

/// Registry of all predefined system definitions loaded from Lua.
#[derive(Resource, Default, Debug, Reflect)]
#[reflect(Resource)]
pub struct PredefinedSystemRegistry {
    pub systems: HashMap<String, PredefinedSystemDefinition>,
}

/// A map type definition. The generator callback itself is stored Lua-side
/// in the `_map_type_definitions` accumulator (since mlua `Function` is not
/// `Send`); this struct only carries the metadata + a `has_generator` flag
/// so Rust code can decide whether to dispatch to the Lua callback.
#[derive(Clone, Debug, bevy::reflect::Reflect)]
pub struct MapTypeDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub has_generator: bool,
}

/// Registry of map type definitions + currently active id (set by
/// `set_active_map_type` from Lua).
#[derive(Resource, Default, Debug, Reflect)]
#[reflect(Resource)]
pub struct MapTypeRegistry {
    pub types: HashMap<String, MapTypeDefinition>,
    /// Id of the map type currently selected by Lua via `set_active_map_type`.
    /// When `None`, `generate_galaxy` falls back to the standard pipeline
    /// (on_galaxy_generate_empty hook or the spiral default).
    pub current: Option<String>,
}

/// Parse `_predefined_system_definitions` into a map keyed by `id`.
pub fn parse_predefined_systems(
    lua: &mlua::Lua,
) -> Result<Vec<PredefinedSystemDefinition>, mlua::Error> {
    use super::helpers::extract_id_from_lua_value;

    let defs: mlua::Table = match lua
        .globals()
        .get::<mlua::Value>("_predefined_system_definitions")?
    {
        mlua::Value::Table(t) => t,
        _ => return Ok(Vec::new()),
    };

    let mut out = Vec::new();
    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;
        let id: String = table.get("id")?;
        let name: String = table
            .get::<Option<String>>("name")?
            .unwrap_or_else(|| id.clone());

        let position = parse_position(&table)?;

        // star_type may be string or ref table
        let star_raw: mlua::Value = table.get("star_type")?;
        let star_type_id = extract_id_from_lua_value(&star_raw)?;

        // planets array, optional (may be omitted for star-only systems).
        let mut planets = Vec::new();
        if let Ok(list) = table.get::<mlua::Table>("planets") {
            for ppair in list.pairs::<i64, mlua::Table>() {
                let (_, ptable) = ppair?;
                let pname: String = ptable.get("name")?;
                let ptype_raw: mlua::Value = ptable.get("type")?;
                let ptype_id = extract_id_from_lua_value(&ptype_raw)?;
                let attrs = parse_planet_attrs(&ptable)?;
                planets.push(PredefinedPlanetSpec {
                    name: pname,
                    planet_type_id: ptype_id,
                    attrs,
                });
            }
        }

        let capital_for_faction: Option<String> =
            match table.get::<mlua::Value>("capital_for_faction")? {
                mlua::Value::Nil => None,
                v => Some(extract_id_from_lua_value(&v)?),
            };

        out.push(PredefinedSystemDefinition {
            id,
            name,
            position,
            star_type_id,
            planets,
            capital_for_faction,
        });
    }

    Ok(out)
}

/// Parse `_map_type_definitions` into metadata rows. The `generator` Lua
/// function is left in place in the accumulator — `galaxy/generation.rs`
/// retrieves it by id at dispatch time (matching how #181 `last_registered_hook`
/// works).
pub fn parse_map_types(lua: &mlua::Lua) -> Result<Vec<MapTypeDefinition>, mlua::Error> {
    let defs: mlua::Table = match lua.globals().get::<mlua::Value>("_map_type_definitions")? {
        mlua::Value::Table(t) => t,
        _ => return Ok(Vec::new()),
    };

    let mut out = Vec::new();
    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;
        let id: String = table.get("id")?;
        let name: String = table
            .get::<Option<String>>("name")?
            .unwrap_or_else(|| id.clone());
        let description: String = table
            .get::<Option<String>>("description")?
            .unwrap_or_default();
        let has_generator = matches!(
            table.get::<mlua::Value>("generator")?,
            mlua::Value::Function(_)
        );
        out.push(MapTypeDefinition {
            id,
            name,
            description,
            has_generator,
        });
    }
    Ok(out)
}

/// Look up a map type's `generator` function on the Lua side by id. Returns
/// `None` if no such map type is defined, if it has no generator, or if the
/// accumulator itself is missing.
pub fn lookup_map_type_generator(
    lua: &mlua::Lua,
    id: &str,
) -> Result<Option<mlua::Function>, mlua::Error> {
    let defs: mlua::Table = match lua.globals().get::<mlua::Value>("_map_type_definitions")? {
        mlua::Value::Table(t) => t,
        _ => return Ok(None),
    };
    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;
        let mt_id: String = match table.get::<Option<String>>("id")? {
            Some(s) => s,
            None => continue,
        };
        if mt_id == id {
            if let mlua::Value::Function(f) = table.get::<mlua::Value>("generator")? {
                return Ok(Some(f));
            }
            return Ok(None);
        }
    }
    Ok(None)
}

/// Read the global `_active_map_type` string (written by `set_active_map_type`).
pub fn read_active_map_type(lua: &mlua::Lua) -> Option<String> {
    match lua.globals().get::<mlua::Value>("_active_map_type") {
        Ok(mlua::Value::String(s)) => s.to_str().ok().map(|s| s.to_string()),
        _ => None,
    }
}

fn parse_position(table: &mlua::Table) -> Result<[f64; 3], mlua::Error> {
    let pos: mlua::Table = table.get("position")?;
    if let Ok(x) = pos.get::<f64>(1) {
        let y: f64 = pos.get(2)?;
        let z: f64 = pos.get::<Option<f64>>(3)?.unwrap_or(0.0);
        return Ok([x, y, z]);
    }
    let x: f64 = pos.get("x")?;
    let y: f64 = pos.get("y")?;
    let z: f64 = pos.get::<Option<f64>>("z")?.unwrap_or(0.0);
    Ok([x, y, z])
}

fn parse_planet_attrs(table: &mlua::Table) -> Result<PlanetAttributesSpec, mlua::Error> {
    let mut spec = PlanetAttributesSpec::default();
    if let Ok(v) = table.get::<f64>("habitability") {
        spec.habitability = Some(v);
    }
    if let Ok(v) = table.get::<f64>("mineral_richness") {
        spec.mineral_richness = Some(v);
    }
    if let Ok(v) = table.get::<f64>("energy_potential") {
        spec.energy_potential = Some(v);
    }
    if let Ok(v) = table.get::<f64>("research_potential") {
        spec.research_potential = Some(v);
    }
    if let Ok(v) = table.get::<u32>("max_building_slots") {
        spec.max_building_slots = Some(v.min(u8::MAX as u32) as u8);
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    fn load(lua_src: &str) -> ScriptEngine {
        let engine = ScriptEngine::new().unwrap();
        engine.lua().load(lua_src).exec().unwrap();
        engine
    }

    #[test]
    fn predefined_system_basic() {
        let engine = load(
            r#"
            define_predefined_system {
                id = "sol",
                name = "Sol",
                position = { 0.0, 0.0, 0.0 },
                star_type = "yellow_dwarf",
                planets = {
                    { name = "Earth", type = "terrestrial", habitability = 1.0, mineral_richness = 0.6 },
                    { name = "Mars", type = "barren", mineral_richness = 0.4 },
                },
                capital_for_faction = "humanity_empire",
            }
            "#,
        );
        let systems = parse_predefined_systems(engine.lua()).unwrap();
        assert_eq!(systems.len(), 1);
        let sol = &systems[0];
        assert_eq!(sol.id, "sol");
        assert_eq!(sol.name, "Sol");
        assert_eq!(sol.position, [0.0, 0.0, 0.0]);
        assert_eq!(sol.star_type_id, "yellow_dwarf");
        assert_eq!(sol.planets.len(), 2);
        assert_eq!(sol.planets[0].name, "Earth");
        assert_eq!(sol.planets[0].planet_type_id, "terrestrial");
        assert_eq!(sol.planets[0].attrs.habitability, Some(1.0));
        assert_eq!(sol.planets[0].attrs.mineral_richness, Some(0.6));
        assert_eq!(sol.capital_for_faction.as_deref(), Some("humanity_empire"));
    }

    #[test]
    fn predefined_system_accepts_named_position() {
        let engine = load(
            r#"
            define_predefined_system {
                id = "alpha",
                position = { x = 1.0, y = 2.0, z = 3.0 },
                star_type = "yellow_dwarf",
            }
            "#,
        );
        let systems = parse_predefined_systems(engine.lua()).unwrap();
        assert_eq!(systems[0].position, [1.0, 2.0, 3.0]);
        assert!(systems[0].planets.is_empty());
    }

    #[test]
    fn predefined_system_accepts_ref_table_for_star_type() {
        let engine = load(
            r#"
            local star = { _def_type = "star_type", id = "yellow_dwarf" }
            define_predefined_system {
                id = "sol",
                position = { 0, 0, 0 },
                star_type = star,
            }
            "#,
        );
        let systems = parse_predefined_systems(engine.lua()).unwrap();
        assert_eq!(systems[0].star_type_id, "yellow_dwarf");
    }

    #[test]
    fn map_type_basic() {
        let engine = load(
            r#"
            define_map_type {
                id = "spiral_galaxy",
                name = "Spiral",
                description = "classic",
                generator = function(ctx) end,
            }
            define_map_type {
                id = "empty",
                name = "Empty",
            }
            "#,
        );
        let types = parse_map_types(engine.lua()).unwrap();
        assert_eq!(types.len(), 2);
        let spiral = types.iter().find(|t| t.id == "spiral_galaxy").unwrap();
        assert_eq!(spiral.name, "Spiral");
        assert!(spiral.has_generator);
        let empty = types.iter().find(|t| t.id == "empty").unwrap();
        assert!(!empty.has_generator);
    }

    #[test]
    fn active_map_type_roundtrip() {
        let engine = load(r#"set_active_map_type("my_map")"#);
        assert_eq!(
            read_active_map_type(engine.lua()).as_deref(),
            Some("my_map")
        );
    }

    #[test]
    fn lookup_generator_returns_function() {
        let engine = load(
            r#"
            define_map_type {
                id = "clustered",
                generator = function(ctx) _was_called = true end,
            }
            "#,
        );
        let f = lookup_map_type_generator(engine.lua(), "clustered")
            .unwrap()
            .expect("function");
        f.call::<()>(mlua::Value::Nil).unwrap();
        let called: Option<bool> = engine.lua().globals().get("_was_called").unwrap();
        assert_eq!(called, Some(true));
    }

    #[test]
    fn lookup_generator_missing_returns_none() {
        let engine = load(
            r#"
            define_map_type { id = "no_gen", name = "x" }
            "#,
        );
        assert!(
            lookup_map_type_generator(engine.lua(), "no_gen")
                .unwrap()
                .is_none()
        );
        assert!(
            lookup_map_type_generator(engine.lua(), "does_not_exist")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn define_predefined_system_returns_reference() {
        let engine = load(
            r#"
            my_ref = define_predefined_system {
                id = "sol",
                position = { 0, 0, 0 },
                star_type = "yellow_dwarf",
            }
            "#,
        );
        let r: mlua::Table = engine.lua().globals().get("my_ref").unwrap();
        assert_eq!(r.get::<String>("_def_type").unwrap(), "predefined_system");
        assert_eq!(r.get::<String>("id").unwrap(), "sol");
    }

    #[test]
    fn define_map_type_returns_reference() {
        let engine = load(
            r#"
            my_ref = define_map_type {
                id = "spiral",
                generator = function(ctx) end,
            }
            "#,
        );
        let r: mlua::Table = engine.lua().globals().get("my_ref").unwrap();
        assert_eq!(r.get::<String>("_def_type").unwrap(), "map_type");
        assert_eq!(r.get::<String>("id").unwrap(), "spiral");
    }
}
