use std::collections::HashMap;

use crate::amount::Amt;
use crate::deep_space::{
    CapabilityParams, DeliverableMetadata, ResourceCost, StructureDefinition, UpgradeEdge,
};
use crate::scripting::condition_parser::parse_prerequisites_field;
use crate::scripting::helpers::extract_id_from_lua_value;

/// Parse structure + deliverable definitions from the Lua globals.
///
/// Pulls from two accumulators:
///   - `_structure_definitions` — populated by `define_structure { ... }`;
///     produces `DeliverableDefinition` with `deliverable = None` unless the
///     script supplies `cost` (legacy backwards-compat path).
///   - `_deliverable_definitions` — populated by `define_deliverable { ... }`;
///     produces `DeliverableDefinition` with `deliverable = Some(_)`.
pub fn parse_structure_definitions(lua: &mlua::Lua) -> Result<Vec<StructureDefinition>, mlua::Error> {
    let mut result = Vec::new();

    // Pass 1: `define_structure` — world-side structures.
    let structures: mlua::Table = lua.globals().get("_structure_definitions")?;
    for pair in structures.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;
        result.push(parse_one(lua, &table, /* deliverable_api */ false)?);
    }

    // Pass 2: `define_deliverable` — shipyard-buildable deliverables.
    // The accumulator is optional (older scripts may not register it).
    if let Ok(deliverables) = lua.globals().get::<mlua::Table>("_deliverable_definitions") {
        for pair in deliverables.pairs::<i64, mlua::Table>() {
            let (_, table) = pair?;
            result.push(parse_one(lua, &table, /* deliverable_api */ true)?);
        }
    }

    Ok(result)
}

/// Parse a single definition table.
///
/// `deliverable_api = true` means the table was produced by `define_deliverable`
/// (meaning: shipyard-buildable). In that case `cost` is required (unless
/// omitted intentionally for an upgrade-only target, but then `upgrade_from`
/// must be present — this invariant is checked at the registry level).
///
/// `deliverable_api = false` means `define_structure` (world-side only). `cost`
/// is honoured for legacy scripts but no DeliverableMetadata is synthesised.
fn parse_one(lua: &mlua::Lua, table: &mlua::Table, deliverable_api: bool) -> Result<StructureDefinition, mlua::Error> {
    let id: String = table.get("id")?;
    let name: String = table.get::<Option<String>>("name")?.unwrap_or_else(|| id.clone());
    let description: String = table.get::<Option<String>>("description")?.unwrap_or_default();
    let max_hp: f64 = table.get::<Option<f64>>("max_hp")?.unwrap_or(100.0);

    let energy_drain_raw: f64 = table.get::<Option<f64>>("energy_drain")?.unwrap_or(0.0);
    let energy_drain = Amt::milli(energy_drain_raw as u64);

    let prerequisites = parse_prerequisites_field(table)?;
    let capabilities = parse_capabilities_map(table)?;

    // `cost` may be Nil (upgrade-only target), a table (minerals/energy), or missing.
    let cost_opt = parse_cost_opt(table)?;

    // Deliverable metadata (only populated when API-level or legacy cost present).
    let deliverable = if deliverable_api {
        // Per issue: `cost = nil` is valid for `define_deliverable` when the
        // deliverable is upgrade-only (validated at registry level). When
        // `cost` is provided, the deliverable is shipyard-buildable.
        match cost_opt {
            Some(cost) => {
                let build_time: i64 = table.get::<Option<i64>>("build_time")?.unwrap_or(10);
                let cargo_size: u32 = table.get::<Option<u32>>("cargo_size")?.unwrap_or(1);
                let scrap_refund: f32 =
                    table.get::<Option<f64>>("scrap_refund")?.unwrap_or(0.0) as f32;
                Some(DeliverableMetadata {
                    cost,
                    build_time,
                    cargo_size,
                    scrap_refund: scrap_refund.clamp(0.0, 1.0),
                })
            }
            None => None,
        }
    } else {
        // Legacy `define_structure`: synthesise metadata when script supplies
        // `cost` (pre-#223 scripts did this). This preserves existing
        // behaviour. New-style scripts should prefer `define_deliverable`.
        match cost_opt {
            Some(cost) => {
                let build_time: i64 = table.get::<Option<i64>>("build_time")?.unwrap_or(10);
                let cargo_size: u32 = table.get::<Option<u32>>("cargo_size")?.unwrap_or(1);
                let scrap_refund: f32 =
                    table.get::<Option<f64>>("scrap_refund")?.unwrap_or(0.0) as f32;
                Some(DeliverableMetadata {
                    cost,
                    build_time,
                    cargo_size,
                    scrap_refund: scrap_refund.clamp(0.0, 1.0),
                })
            }
            None => None,
        }
    };

    let upgrade_to = parse_upgrade_to(table)?;
    let upgrade_from = parse_upgrade_from(table)?;
    // #281: Definition-level lifecycle hooks.
    let on_built = crate::scripting::parse_lua_function_field(lua, table, "on_built")?;
    let on_upgraded = crate::scripting::parse_lua_function_field(lua, table, "on_upgraded")?;

    Ok(StructureDefinition {
        id,
        name,
        description,
        max_hp,
        energy_drain,
        prerequisites,
        capabilities,
        deliverable,
        upgrade_to,
        upgrade_from,
        on_built,
        on_upgraded,
    })
}

/// Parse the `cost = { minerals = N, energy = N }` sub-table if present.
/// Returns `None` when the key is missing OR explicitly set to `nil`.
fn parse_cost_opt(table: &mlua::Table) -> Result<Option<ResourceCost>, mlua::Error> {
    let cost_value: mlua::Value = table.get("cost")?;
    match cost_value {
        mlua::Value::Table(cost_table) => {
            let minerals_raw: f64 = cost_table.get::<Option<f64>>("minerals")?.unwrap_or(0.0);
            let energy_raw: f64 = cost_table.get::<Option<f64>>("energy")?.unwrap_or(0.0);
            Ok(Some(ResourceCost {
                minerals: Amt::from_f64(minerals_raw),
                energy: Amt::from_f64(energy_raw),
            }))
        }
        mlua::Value::Nil => Ok(None),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'cost' field".to_string(),
        )),
    }
}

/// Parse `capabilities = { cap_name = { range = N }, ... }` as a HashMap.
fn parse_capabilities_map(table: &mlua::Table) -> Result<HashMap<String, CapabilityParams>, mlua::Error> {
    let caps_value: mlua::Value = table.get("capabilities")?;
    match caps_value {
        mlua::Value::Table(caps_table) => {
            let mut caps = HashMap::new();
            for pair in caps_table.pairs::<String, mlua::Table>() {
                let (key, params_table) = pair?;
                let range: f64 = params_table.get::<Option<f64>>("range")?.unwrap_or(0.0);
                caps.insert(key, CapabilityParams { range });
            }
            Ok(caps)
        }
        mlua::Value::Nil => Ok(HashMap::new()),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'capabilities' field".to_string(),
        )),
    }
}

/// Parse `upgrade_to = { { target = ..., cost = { ... }, build_time = N }, ... }`.
/// Each entry's `target` may be a string id or a reference table.
fn parse_upgrade_to(table: &mlua::Table) -> Result<Vec<UpgradeEdge>, mlua::Error> {
    let value: mlua::Value = table.get("upgrade_to")?;
    match value {
        mlua::Value::Nil => Ok(Vec::new()),
        mlua::Value::Table(list) => {
            let mut edges = Vec::new();
            for pair in list.pairs::<i64, mlua::Table>() {
                let (_, entry) = pair?;
                let target_val: mlua::Value = entry.get("target")?;
                let target_id = extract_id_from_lua_value(&target_val)?;
                let cost = match parse_cost_opt(&entry)? {
                    Some(c) => c,
                    None => ResourceCost::default(),
                };
                let build_time: i64 = entry.get::<Option<i64>>("build_time")?.unwrap_or(10);
                edges.push(UpgradeEdge {
                    target_id,
                    cost,
                    build_time,
                });
            }
            Ok(edges)
        }
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'upgrade_to' field".to_string(),
        )),
    }
}

/// Parse `upgrade_from = { source = ..., cost = { ... }, build_time = N }`.
/// Returns `UpgradeEdge` with `target_id` holding the SOURCE id (i.e. the
/// upstream deliverable this one can be reached from).
fn parse_upgrade_from(table: &mlua::Table) -> Result<Option<UpgradeEdge>, mlua::Error> {
    let value: mlua::Value = table.get("upgrade_from")?;
    match value {
        mlua::Value::Nil => Ok(None),
        mlua::Value::Table(entry) => {
            let source_val: mlua::Value = entry.get("source")?;
            let source_id = extract_id_from_lua_value(&source_val)?;
            let cost = match parse_cost_opt(&entry)? {
                Some(c) => c,
                None => ResourceCost::default(),
            };
            let build_time: i64 = entry.get::<Option<i64>>("build_time")?.unwrap_or(10);
            Ok(Some(UpgradeEdge {
                target_id: source_id,
                cost,
                build_time,
            }))
        }
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'upgrade_from' field".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::condition::{AtomKind, Condition, ConditionAtom, ConditionScope};
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_parse_structure_definitions() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_structure {
                id = "sensor_buoy",
                name = "Sensor Buoy",
                description = "Detects sublight vessel movements.",
                max_hp = 20,
                cost = { minerals = 50, energy = 30 },
                build_time = 15,
                capabilities = {
                    detect_sublight = { range = 3.0 },
                },
                energy_drain = 100,
            }
            define_structure {
                id = "interdictor",
                name = "Interdictor",
                description = "Disrupts FTL travel.",
                max_hp = 80,
                cost = { minerals = 300, energy = 200 },
                build_time = 45,
                capabilities = {
                    ftl_interdiction = { range = 5.0 },
                },
                energy_drain = 1000,
                prerequisites = has_tech("ftl_interdiction_tech"),
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_structure_definitions(lua).unwrap();
        assert_eq!(defs.len(), 2);

        // Sensor Buoy
        assert_eq!(defs[0].id, "sensor_buoy");
        assert_eq!(defs[0].name, "Sensor Buoy");
        assert_eq!(defs[0].description, "Detects sublight vessel movements.");
        assert_eq!(defs[0].max_hp, 20.0);
        let meta = defs[0].deliverable.as_ref().expect("legacy cost should synthesise metadata");
        assert_eq!(meta.cost.minerals, Amt::units(50));
        assert_eq!(meta.cost.energy, Amt::units(30));
        assert_eq!(meta.build_time, 15);
        assert!(defs[0].capabilities.contains_key("detect_sublight"));
        assert_eq!(defs[0].capabilities["detect_sublight"].range, 3.0);
        assert_eq!(defs[0].energy_drain, Amt::milli(100));
        assert!(defs[0].prerequisites.is_none());

        // Interdictor
        assert_eq!(defs[1].id, "interdictor");
        assert_eq!(defs[1].name, "Interdictor");
        assert_eq!(defs[1].max_hp, 80.0);
        assert!(defs[1].capabilities.contains_key("ftl_interdiction"));
        assert_eq!(defs[1].capabilities["ftl_interdiction"].range, 5.0);
        assert_eq!(defs[1].energy_drain, Amt::units(1));
        assert_eq!(
            defs[1].prerequisites,
            Some(Condition::Atom(ConditionAtom::has_tech(
                "ftl_interdiction_tech"
            )))
        );
    }

    #[test]
    fn test_parse_structure_minimal() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_structure {
                id = "basic",
                name = "Basic Structure",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_structure_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "basic");
        assert_eq!(defs[0].name, "Basic Structure");
        assert_eq!(defs[0].description, "");
        assert_eq!(defs[0].max_hp, 100.0);
        // No cost → no deliverable metadata for minimal structures.
        assert!(defs[0].deliverable.is_none());
        assert!(defs[0].capabilities.is_empty());
        assert_eq!(defs[0].energy_drain, Amt::ZERO);
        assert!(defs[0].prerequisites.is_none());
    }

    #[test]
    fn test_parse_structure_with_complex_prerequisites() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_structure {
                id = "advanced",
                name = "Advanced Structure",
                prerequisites = all(
                    has_tech("tech_a"),
                    any(has_modifier("mod_b"), has_building("bldg_c"))
                ),
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_structure_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(
            defs[0].prerequisites,
            Some(Condition::All(vec![
                Condition::Atom(ConditionAtom::has_tech("tech_a")),
                Condition::Any(vec![
                    Condition::Atom(ConditionAtom::has_modifier("mod_b")),
                    Condition::Atom(ConditionAtom::has_building("bldg_c")),
                ]),
            ]))
        );
    }

    #[test]
    fn test_parse_structure_with_function_prerequisites() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_structure {
                id = "scoped_station",
                name = "Scoped Station",
                prerequisites = function(ctx)
                    return all(
                        ctx.empire:has_tech("advanced_sensors"),
                        ctx.system:has_building("shipyard")
                    )
                end,
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_structure_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(
            defs[0].prerequisites,
            Some(Condition::All(vec![
                Condition::Atom(ConditionAtom::scoped(
                    AtomKind::HasTech("advanced_sensors".into()),
                    ConditionScope::Empire,
                )),
                Condition::Atom(ConditionAtom::scoped(
                    AtomKind::HasBuilding("shipyard".into()),
                    ConditionScope::System,
                )),
            ]))
        );
    }

    #[test]
    fn test_parse_structure_from_lua_file() {
        let engine = ScriptEngine::new().unwrap();

        let structure_script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/structures/definitions.lua");
        if !structure_script.exists() {
            // Skip if file doesn't exist yet (it may not be created yet during development)
            return;
        }

        engine.load_file(&structure_script).unwrap();
        let defs = parse_structure_definitions(engine.lua()).unwrap();

        assert!(
            defs.len() >= 3,
            "Expected at least 3 structure definitions from definitions.lua, got {}",
            defs.len()
        );

        // Build a quick lookup
        let map: std::collections::HashMap<String, _> =
            defs.into_iter().map(|d| (d.id.clone(), d)).collect();

        let buoy = map.get("sensor_buoy").expect("sensor_buoy should exist");
        assert_eq!(buoy.name, "Sensor Buoy");
        assert!(buoy.capabilities.contains_key("detect_sublight"));

        let relay = map.get("ftl_comm_relay").expect("ftl_comm_relay should exist");
        assert!(relay.capabilities.contains_key("ftl_comm_relay"));

        let interdictor = map.get("interdictor").expect("interdictor should exist");
        assert!(interdictor.capabilities.contains_key("ftl_interdiction"));
    }

    // --- #223: define_deliverable tests ---

    #[test]
    fn test_parse_define_deliverable_with_cost() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_deliverable {
                id = "sensor_buoy",
                name = "Sensor Buoy",
                cost = { minerals = 50, energy = 30 },
                build_time = 15,
                cargo_size = 1,
                max_hp = 20,
                energy_drain = 100,
                scrap_refund = 0.5,
                capabilities = { detect_sublight = { range = 3.0 } },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_structure_definitions(lua).unwrap();
        let buoy = defs.iter().find(|d| d.id == "sensor_buoy").unwrap();
        let meta = buoy
            .deliverable
            .as_ref()
            .expect("define_deliverable with cost → metadata present");
        assert_eq!(meta.cost.minerals, Amt::units(50));
        assert_eq!(meta.cost.energy, Amt::units(30));
        assert_eq!(meta.build_time, 15);
        assert_eq!(meta.cargo_size, 1);
        assert!((meta.scrap_refund - 0.5).abs() < 1e-5);
    }

    #[test]
    fn test_parse_define_structure_no_cost() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_structure {
                id = "debris_wreck",
                name = "Debris Wreck",
                max_hp = 1,
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_structure_definitions(lua).unwrap();
        let wreck = defs.iter().find(|d| d.id == "debris_wreck").unwrap();
        assert!(
            wreck.deliverable.is_none(),
            "define_structure without cost → no deliverable metadata"
        );
    }

    #[test]
    fn test_parse_upgrade_to_with_forward_ref() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_deliverable {
                id = "defense_platform_kit",
                name = "Defense Platform Kit",
                cost = { minerals = 200, energy = 100 },
                build_time = 20,
                cargo_size = 3,
                max_hp = 80,
                energy_drain = 50,
                scrap_refund = 0.3,
                upgrade_to = {
                    { target = forward_ref("defense_platform"),
                      cost = { minerals = 1800, energy = 700 },
                      build_time = 60 },
                },
                capabilities = { construction_platform = {} },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_structure_definitions(lua).unwrap();
        let kit = defs.iter().find(|d| d.id == "defense_platform_kit").unwrap();
        assert_eq!(kit.upgrade_to.len(), 1);
        assert_eq!(kit.upgrade_to[0].target_id, "defense_platform");
        assert_eq!(kit.upgrade_to[0].cost.minerals, Amt::units(1800));
        assert_eq!(kit.upgrade_to[0].cost.energy, Amt::units(700));
        assert_eq!(kit.upgrade_to[0].build_time, 60);
        assert!(kit.is_construction_platform());
    }

    #[test]
    fn test_parse_upgrade_from_with_forward_ref() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_deliverable {
                id = "new_structure",
                name = "New Structure",
                upgrade_from = {
                    source = forward_ref("universal_platform"),
                    cost = { minerals = 500, energy = 300 },
                    build_time = 40,
                },
                capabilities = {},
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_structure_definitions(lua).unwrap();
        let ns = defs.iter().find(|d| d.id == "new_structure").unwrap();
        let uf = ns
            .upgrade_from
            .as_ref()
            .expect("upgrade_from should parse");
        assert_eq!(uf.target_id, "universal_platform");
        assert_eq!(uf.cost.minerals, Amt::units(500));
        assert_eq!(uf.cost.energy, Amt::units(300));
        assert_eq!(uf.build_time, 40);
        // No cost on the deliverable itself → not shipyard-buildable.
        assert!(ns.deliverable.is_none());
    }
}
