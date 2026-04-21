//! #145: Lua API for forbidden regions (nebulae, subspace storms).
//!
//! Exposes two entry points to Lua scripts:
//!
//! - `define_region_type { id, name, capabilities, visual }` — declares a
//!   region type, stored in `_region_type_definitions` (handled by the
//!   generic `define_xxx` accumulator in `globals.rs`).
//! - `galaxy_generation.add_region_spec { ... }` — queues a placement spec
//!   into `_pending_region_specs`. Drained on the Rust side before
//!   `generate_galaxy` places regions.
//!
//! Both tables are parsed here into [`RegionTypeDefinition`] and
//! [`RegionSpec`] respectively.

use std::collections::HashMap;

use crate::galaxy::region::{CapabilityParams, RegionSpec, RegionTypeDefinition};

/// Parse `_region_type_definitions` into a list of type defs.
pub fn parse_region_type_definitions(
    lua: &mlua::Lua,
) -> Result<Vec<RegionTypeDefinition>, mlua::Error> {
    let defs: mlua::Table = match lua
        .globals()
        .get::<mlua::Value>("_region_type_definitions")?
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
        let capabilities = parse_region_capabilities(&table)?;
        let (visual_color, visual_density) = parse_visual(&table)?;
        out.push(RegionTypeDefinition {
            id,
            name,
            capabilities,
            visual_color,
            visual_density,
        });
    }
    Ok(out)
}

fn parse_region_capabilities(
    table: &mlua::Table,
) -> Result<HashMap<String, CapabilityParams>, mlua::Error> {
    let caps_value: mlua::Value = table.get("capabilities")?;
    match caps_value {
        mlua::Value::Table(caps_table) => {
            let mut caps = HashMap::new();
            for pair in caps_table.pairs::<String, mlua::Table>() {
                let (key, params_table) = pair?;
                let strength: f64 = params_table.get::<Option<f64>>("strength")?.unwrap_or(1.0);
                caps.insert(key, CapabilityParams { strength });
            }
            Ok(caps)
        }
        mlua::Value::Nil => Ok(HashMap::new()),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'capabilities' field".into(),
        )),
    }
}

fn parse_visual(table: &mlua::Table) -> Result<([f32; 3], f32), mlua::Error> {
    let v: mlua::Value = table.get("visual")?;
    match v {
        mlua::Value::Table(t) => {
            let color = match t.get::<mlua::Value>("color")? {
                mlua::Value::Table(ct) => {
                    let r: f32 = ct.get::<Option<f32>>(1)?.unwrap_or(0.3);
                    let g: f32 = ct.get::<Option<f32>>(2)?.unwrap_or(0.1);
                    let b: f32 = ct.get::<Option<f32>>(3)?.unwrap_or(0.5);
                    [r, g, b]
                }
                _ => [0.3, 0.1, 0.5],
            };
            let density: f32 = t.get::<Option<f32>>("density")?.unwrap_or(0.6);
            Ok((color, density))
        }
        _ => Ok(([0.3, 0.1, 0.5], 0.6)),
    }
}

/// Parse `_pending_region_specs` (populated via
/// `galaxy_generation.add_region_spec { ... }`) into [`RegionSpec`].
pub fn parse_region_specs(lua: &mlua::Lua) -> Result<Vec<RegionSpec>, mlua::Error> {
    let defs: mlua::Table = match lua.globals().get::<mlua::Value>("_pending_region_specs")? {
        mlua::Value::Table(t) => t,
        _ => return Ok(Vec::new()),
    };

    let mut out = Vec::new();
    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;
        let type_raw: mlua::Value = table.get("type")?;
        let type_id = crate::scripting::helpers::extract_id_from_lua_value(&type_raw)?;
        let count_range = parse_u32_range(&table, "count_range", (2, 4))?;
        let sphere_count_range = parse_u32_range(&table, "sphere_count_range", (2, 5))?;
        let sphere_radius_range = parse_f64_range(&table, "sphere_radius_range", (3.0, 8.0))?;
        let min_distance_from_capital: f64 = table
            .get::<Option<f64>>("min_distance_from_capital")?
            .unwrap_or(15.0);
        let threshold: f64 = table.get::<Option<f64>>("threshold")?.unwrap_or(1.0);

        out.push(RegionSpec {
            type_id,
            count_range,
            sphere_count_range,
            sphere_radius_range,
            min_distance_from_capital,
            threshold,
        });
    }
    Ok(out)
}

fn parse_u32_range(
    table: &mlua::Table,
    key: &str,
    default: (u32, u32),
) -> Result<(u32, u32), mlua::Error> {
    match table.get::<mlua::Value>(key)? {
        mlua::Value::Table(t) => {
            let a: u32 = t.get::<Option<u32>>(1)?.unwrap_or(default.0);
            let b: u32 = t.get::<Option<u32>>(2)?.unwrap_or(default.1);
            Ok((a.min(b), a.max(b)))
        }
        _ => Ok(default),
    }
}

fn parse_f64_range(
    table: &mlua::Table,
    key: &str,
    default: (f64, f64),
) -> Result<(f64, f64), mlua::Error> {
    match table.get::<mlua::Value>(key)? {
        mlua::Value::Table(t) => {
            let a: f64 = t.get::<Option<f64>>(1)?.unwrap_or(default.0);
            let b: f64 = t.get::<Option<f64>>(2)?.unwrap_or(default.1);
            Ok((a.min(b), a.max(b)))
        }
        _ => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    fn load(src: &str) -> ScriptEngine {
        let engine = ScriptEngine::new().unwrap();
        engine.lua().load(src).exec().unwrap();
        engine
    }

    #[test]
    fn define_region_type_minimal() {
        let engine = load(
            r#"
            define_region_type {
                id = "dark_nebula",
                name = "Dark Nebula",
                capabilities = {
                    blocks_ftl = { strength = 1.0 },
                    blocks_ftl_comm = { strength = 0.8 },
                },
                visual = { color = {0.3, 0.1, 0.5}, density = 0.7 },
            }
            "#,
        );
        let defs = parse_region_type_definitions(engine.lua()).unwrap();
        assert_eq!(defs.len(), 1);
        let dn = &defs[0];
        assert_eq!(dn.id, "dark_nebula");
        assert_eq!(dn.name, "Dark Nebula");
        assert!(dn.capabilities.contains_key("blocks_ftl"));
        assert!(dn.capabilities.contains_key("blocks_ftl_comm"));
        assert!((dn.capabilities["blocks_ftl_comm"].strength - 0.8).abs() < 1e-9);
        assert_eq!(dn.visual_color, [0.3, 0.1, 0.5]);
        assert!((dn.visual_density - 0.7).abs() < 1e-6);
    }

    #[test]
    fn define_region_type_defaults() {
        let engine = load(
            r#"
            define_region_type {
                id = "subspace_storm",
                capabilities = { blocks_ftl = {} },
            }
            "#,
        );
        let defs = parse_region_type_definitions(engine.lua()).unwrap();
        assert_eq!(defs[0].name, "subspace_storm");
        assert!((defs[0].capabilities["blocks_ftl"].strength - 1.0).abs() < 1e-9);
        // Visual defaults.
        assert!((defs[0].visual_density - 0.6).abs() < 1e-6);
    }

    #[test]
    fn define_region_type_returns_reference() {
        let engine = load(
            r#"
            my_ref = define_region_type {
                id = "dark_nebula",
                capabilities = { blocks_ftl = {} },
            }
            "#,
        );
        let r: mlua::Table = engine.lua().globals().get("my_ref").unwrap();
        assert_eq!(r.get::<String>("_def_type").unwrap(), "region_type");
        assert_eq!(r.get::<String>("id").unwrap(), "dark_nebula");
    }

    #[test]
    fn add_region_spec_basic() {
        let engine = load(
            r#"
            galaxy_generation.add_region_spec {
                type = "dark_nebula",
                count_range = {2, 5},
                sphere_count_range = {2, 5},
                sphere_radius_range = {3.0, 8.0},
                min_distance_from_capital = 15.0,
            }
            "#,
        );
        let specs = parse_region_specs(engine.lua()).unwrap();
        assert_eq!(specs.len(), 1);
        let s = &specs[0];
        assert_eq!(s.type_id, "dark_nebula");
        assert_eq!(s.count_range, (2, 5));
        assert_eq!(s.sphere_count_range, (2, 5));
        assert_eq!(s.sphere_radius_range, (3.0, 8.0));
        assert!((s.min_distance_from_capital - 15.0).abs() < 1e-9);
        assert!((s.threshold - 1.0).abs() < 1e-9);
    }

    #[test]
    fn add_region_spec_accepts_reference() {
        let engine = load(
            r#"
            local dn = define_region_type {
                id = "dark_nebula",
                capabilities = { blocks_ftl = {} },
            }
            galaxy_generation.add_region_spec {
                type = dn,
                threshold = 1.5,
            }
            "#,
        );
        let specs = parse_region_specs(engine.lua()).unwrap();
        assert_eq!(specs[0].type_id, "dark_nebula");
        assert!((specs[0].threshold - 1.5).abs() < 1e-9);
    }
}
