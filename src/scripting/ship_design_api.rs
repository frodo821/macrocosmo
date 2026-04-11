use crate::amount::Amt;
use crate::ship_design::{
    DesignSlotAssignment, HullDefinition, HullSlot, ModuleDefinition, ModuleModifier,
    ModuleUpgradePath, ShipDesignDefinition, SlotTypeDefinition, WeaponStats,
};

/// Parse slot type definitions from the Lua `_slot_type_definitions` global table.
pub fn parse_slot_types(lua: &mlua::Lua) -> Result<Vec<SlotTypeDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_slot_type_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;
        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        result.push(SlotTypeDefinition { id, name });
    }

    Ok(result)
}

/// Parse hull definitions from the Lua `_hull_definitions` global table.
pub fn parse_hulls(lua: &mlua::Lua) -> Result<Vec<HullDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_hull_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let description: String = table.get::<Option<String>>("description")?.unwrap_or_default();
        let base_hp: f64 = table.get::<Option<f64>>("base_hp")?.unwrap_or(100.0);
        let base_speed: f64 = table.get::<Option<f64>>("base_speed")?.unwrap_or(1.0);
        let base_evasion: f64 = table.get::<Option<f64>>("base_evasion")?.unwrap_or(0.0);
        let build_time: i64 = table.get::<Option<i64>>("build_time")?.unwrap_or(60);
        let maintenance_f64: f64 = table.get::<Option<f64>>("maintenance")?.unwrap_or(0.0);
        let maintenance = Amt::from_f64(maintenance_f64);

        // Parse slots array
        let slots = parse_hull_slots(&table)?;

        // Parse build_cost table
        let (build_cost_minerals, build_cost_energy) = parse_cost_table(&table, "build_cost")?;

        // Parse hull modifiers (optional, same format as module modifiers)
        let modifiers = parse_module_modifiers(&table)?;

        result.push(HullDefinition {
            id,
            name,
            description,
            base_hp,
            base_speed,
            base_evasion,
            slots,
            build_cost_minerals,
            build_cost_energy,
            build_time,
            maintenance,
            modifiers,
        });
    }

    Ok(result)
}

/// Parse module definitions from the Lua `_module_definitions` global table.
pub fn parse_modules(lua: &mlua::Lua) -> Result<Vec<ModuleDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_module_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let description: String = table.get::<Option<String>>("description")?.unwrap_or_default();
        let slot_type_value: mlua::Value = table.get("slot_type")?;
        let slot_type = crate::scripting::extract_ref_id(&slot_type_value)?;
        let prereq_value: mlua::Value = table.get("prerequisite_tech")?;
        let prerequisite_tech = match prereq_value {
            mlua::Value::Nil => None,
            v => Some(crate::scripting::extract_ref_id(&v)?),
        };

        // Parse modifiers array
        let modifiers = parse_module_modifiers(&table)?;

        // Parse weapon stats (optional)
        let weapon = parse_weapon_stats(&table)?;

        // Parse cost table
        let (cost_minerals, cost_energy) = parse_cost_table(&table, "cost")?;

        // Parse upgrade_to array (optional)
        let upgrade_to = parse_module_upgrade_to(&table)?;

        result.push(ModuleDefinition {
            id,
            name,
            description,
            slot_type,
            modifiers,
            weapon,
            cost_minerals,
            cost_energy,
            prerequisite_tech,
            upgrade_to,
        });
    }

    Ok(result)
}

/// Parse ship design definitions from the Lua `_ship_design_definitions` global table.
pub fn parse_ship_designs(lua: &mlua::Lua) -> Result<Vec<ShipDesignDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_ship_design_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let description: String = table.get::<Option<String>>("description")?.unwrap_or_default();
        let hull_value: mlua::Value = table.get("hull")?;
        let hull_id = crate::scripting::extract_ref_id(&hull_value)?;

        // Parse modules array
        let modules = parse_design_modules(&table)?;

        result.push(ShipDesignDefinition {
            id,
            name,
            description,
            hull_id,
            modules,
        });
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Helper parsers
// ---------------------------------------------------------------------------

/// Parse the `cost = { minerals = N, energy = N }` or `build_cost = { ... }` sub-table.
fn parse_cost_table(
    table: &mlua::Table,
    field_name: &str,
) -> Result<(Amt, Amt), mlua::Error> {
    let cost_value: mlua::Value = table.get(field_name)?;
    match cost_value {
        mlua::Value::Table(cost_table) => {
            let minerals: f64 = cost_table.get::<Option<f64>>("minerals")?.unwrap_or(0.0);
            let energy: f64 = cost_table.get::<Option<f64>>("energy")?.unwrap_or(0.0);
            Ok((Amt::from_f64(minerals), Amt::from_f64(energy)))
        }
        mlua::Value::Nil => Ok((Amt::ZERO, Amt::ZERO)),
        _ => Err(mlua::Error::RuntimeError(format!(
            "Expected table or nil for '{}' field",
            field_name
        ))),
    }
}

/// Parse the `slots = { { type = "weapon", count = 2 }, ... }` array.
/// The `type` field accepts both a string ID and a reference table from `define_slot_type`.
fn parse_hull_slots(table: &mlua::Table) -> Result<Vec<HullSlot>, mlua::Error> {
    let slots_value: mlua::Value = table.get("slots")?;
    match slots_value {
        mlua::Value::Table(slots_table) => {
            let mut slots = Vec::new();
            for pair in slots_table.pairs::<i64, mlua::Table>() {
                let (_, slot_table) = pair?;
                let type_value: mlua::Value = slot_table.get("type")?;
                let slot_type = crate::scripting::extract_ref_id(&type_value)?;
                let count: u32 = slot_table.get::<Option<u32>>("count")?.unwrap_or(1);
                slots.push(HullSlot { slot_type, count });
            }
            Ok(slots)
        }
        mlua::Value::Nil => Ok(Vec::new()),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'slots' field".to_string(),
        )),
    }
}

/// Parse the `modifiers = { { target = "...", base_add = N, ... }, ... }` array.
fn parse_module_modifiers(table: &mlua::Table) -> Result<Vec<ModuleModifier>, mlua::Error> {
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
                modifiers.push(ModuleModifier {
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
            "Expected table or nil for 'modifiers' field".to_string(),
        )),
    }
}

/// Parse the optional `weapon = { track, precision, cooldown, ... }` sub-table.
fn parse_weapon_stats(table: &mlua::Table) -> Result<Option<WeaponStats>, mlua::Error> {
    let weapon_value: mlua::Value = table.get("weapon")?;
    match weapon_value {
        mlua::Value::Table(w) => {
            let track: f64 = w.get::<Option<f64>>("track")?.unwrap_or(0.0);
            let precision: f64 = w.get::<Option<f64>>("precision")?.unwrap_or(0.0);
            let cooldown: i64 = w.get::<Option<i64>>("cooldown")?.unwrap_or(1);
            let range: f64 = w.get::<Option<f64>>("range")?.unwrap_or(0.0);
            let shield_damage: f64 = w.get::<Option<f64>>("shield_damage")?.unwrap_or(0.0);
            let shield_damage_div: f64 = w.get::<Option<f64>>("shield_damage_div")?.unwrap_or(0.0);
            let shield_piercing: f64 = w.get::<Option<f64>>("shield_piercing")?.unwrap_or(0.0);
            let armor_damage: f64 = w.get::<Option<f64>>("armor_damage")?.unwrap_or(0.0);
            let armor_damage_div: f64 = w.get::<Option<f64>>("armor_damage_div")?.unwrap_or(0.0);
            let armor_piercing: f64 = w.get::<Option<f64>>("armor_piercing")?.unwrap_or(0.0);
            let hull_damage: f64 = w.get::<Option<f64>>("hull_damage")?.unwrap_or(0.0);
            let hull_damage_div: f64 = w.get::<Option<f64>>("hull_damage_div")?.unwrap_or(0.0);

            Ok(Some(WeaponStats {
                track,
                precision,
                cooldown,
                range,
                shield_damage,
                shield_damage_div,
                shield_piercing,
                armor_damage,
                armor_damage_div,
                armor_piercing,
                hull_damage,
                hull_damage_div,
            }))
        }
        mlua::Value::Nil => Ok(None),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'weapon' field".to_string(),
        )),
    }
}

/// Parse the `upgrade_to = { { target = ref, cost = { minerals = N, energy = N } }, ... }` array on modules.
/// The `target` field accepts string IDs, reference tables, or forward_ref tables via `extract_ref_id()`.
fn parse_module_upgrade_to(table: &mlua::Table) -> Result<Vec<ModuleUpgradePath>, mlua::Error> {
    let value: mlua::Value = table.get("upgrade_to")?;
    match value {
        mlua::Value::Table(arr) => {
            let mut result = Vec::new();
            for pair in arr.pairs::<i64, mlua::Table>() {
                let (_, entry) = pair?;
                let target_value: mlua::Value = entry.get("target")?;
                let target_id = crate::scripting::extract_ref_id(&target_value)?;

                let (cost_minerals, cost_energy) = parse_cost_table(&entry, "cost")?;

                result.push(ModuleUpgradePath {
                    target_id,
                    cost_minerals,
                    cost_energy,
                });
            }
            Ok(result)
        }
        mlua::Value::Nil => Ok(Vec::new()),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for module 'upgrade_to' field".to_string(),
        )),
    }
}

/// Parse the `modules = { { slot_type = "...", module = "..." }, ... }` array.
/// Both `slot_type` and `module` accept string IDs or reference tables.
fn parse_design_modules(
    table: &mlua::Table,
) -> Result<Vec<DesignSlotAssignment>, mlua::Error> {
    let mods_value: mlua::Value = table.get("modules")?;
    match mods_value {
        mlua::Value::Table(mods_table) => {
            let mut assignments = Vec::new();
            for pair in mods_table.pairs::<i64, mlua::Table>() {
                let (_, mod_table) = pair?;
                let slot_type_value: mlua::Value = mod_table.get("slot_type")?;
                let slot_type = crate::scripting::extract_ref_id(&slot_type_value)?;
                let module_value: mlua::Value = mod_table.get("module")?;
                let module_id = crate::scripting::extract_ref_id(&module_value)?;
                assignments.push(DesignSlotAssignment {
                    slot_type,
                    module_id,
                });
            }
            Ok(assignments)
        }
        mlua::Value::Nil => Ok(Vec::new()),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'modules' field".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_parse_slot_types() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_slot_type { id = "weapon", name = "Weapon Slot" }
            define_slot_type { id = "utility", name = "Utility Slot" }
            define_slot_type { id = "engine", name = "Engine Slot" }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_slot_types(lua).unwrap();
        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0].id, "weapon");
        assert_eq!(defs[0].name, "Weapon Slot");
        assert_eq!(defs[1].id, "utility");
        assert_eq!(defs[2].id, "engine");
    }

    #[test]
    fn test_parse_hulls() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_hull {
                id = "corvette",
                name = "Corvette",
                base_hp = 50,
                base_speed = 0.75,
                base_evasion = 30.0,
                slots = {
                    { type = "weapon", count = 2 },
                    { type = "utility", count = 1 },
                    { type = "engine", count = 1 },
                },
                build_cost = { minerals = 200, energy = 100 },
                build_time = 60,
                maintenance = 0.5,
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_hulls(lua).unwrap();
        assert_eq!(defs.len(), 1);

        let corvette = &defs[0];
        assert_eq!(corvette.id, "corvette");
        assert_eq!(corvette.name, "Corvette");
        assert_eq!(corvette.base_hp, 50.0);
        assert_eq!(corvette.base_speed, 0.75);
        assert_eq!(corvette.base_evasion, 30.0);
        assert_eq!(corvette.slots.len(), 3);
        assert_eq!(corvette.slots[0].slot_type, "weapon");
        assert_eq!(corvette.slots[0].count, 2);
        assert_eq!(corvette.slots[1].slot_type, "utility");
        assert_eq!(corvette.slots[1].count, 1);
        assert_eq!(corvette.slots[2].slot_type, "engine");
        assert_eq!(corvette.slots[2].count, 1);
        assert_eq!(corvette.build_cost_minerals, Amt::units(200));
        assert_eq!(corvette.build_cost_energy, Amt::units(100));
        assert_eq!(corvette.build_time, 60);
        assert_eq!(corvette.maintenance, Amt::new(0, 500));
    }

    #[test]
    fn test_parse_modules() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_module {
                id = "ftl_drive",
                name = "FTL Drive",
                slot_type = "engine",
                modifiers = {
                    { target = "ship.ftl_range", base_add = 15.0 },
                },
                cost = { minerals = 100, energy = 50 },
            }
            define_module {
                id = "armor_plating",
                name = "Armor Plating",
                slot_type = "utility",
                modifiers = {
                    { target = "ship.armor_max", base_add = 30.0 },
                    { target = "ship.speed", multiplier = -0.05 },
                },
                cost = { minerals = 80 },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_modules(lua).unwrap();
        assert_eq!(defs.len(), 2);

        // FTL Drive
        let ftl = &defs[0];
        assert_eq!(ftl.id, "ftl_drive");
        assert_eq!(ftl.name, "FTL Drive");
        assert_eq!(ftl.slot_type, "engine");
        assert_eq!(ftl.modifiers.len(), 1);
        assert_eq!(ftl.modifiers[0].target, "ship.ftl_range");
        assert_eq!(ftl.modifiers[0].base_add, 15.0);
        assert_eq!(ftl.modifiers[0].multiplier, 0.0);
        assert!(ftl.weapon.is_none());
        assert_eq!(ftl.cost_minerals, Amt::units(100));
        assert_eq!(ftl.cost_energy, Amt::units(50));

        // Armor Plating
        let armor = &defs[1];
        assert_eq!(armor.id, "armor_plating");
        assert_eq!(armor.modifiers.len(), 2);
        assert_eq!(armor.modifiers[1].target, "ship.speed");
        assert_eq!(armor.modifiers[1].multiplier, -0.05);
        assert_eq!(armor.cost_minerals, Amt::units(80));
        assert_eq!(armor.cost_energy, Amt::ZERO);
    }

    #[test]
    fn test_parse_modules_with_weapon() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_module {
                id = "weapon_laser",
                name = "Laser Battery",
                slot_type = "weapon",
                weapon = {
                    track = 5.0, precision = 0.85, cooldown = 1, range = 10.0,
                    shield_damage = 4.0, shield_damage_div = 1.0, shield_piercing = 0.0,
                    armor_damage = 2.0, armor_damage_div = 0.5, armor_piercing = 0.0,
                    hull_damage = 3.0, hull_damage_div = 1.0,
                },
                cost = { minerals = 50, energy = 30 },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_modules(lua).unwrap();
        assert_eq!(defs.len(), 1);

        let laser = &defs[0];
        assert_eq!(laser.id, "weapon_laser");
        assert!(laser.weapon.is_some());

        let weapon = laser.weapon.as_ref().unwrap();
        assert_eq!(weapon.track, 5.0);
        assert_eq!(weapon.precision, 0.85);
        assert_eq!(weapon.cooldown, 1);
        assert_eq!(weapon.range, 10.0);
        assert_eq!(weapon.shield_damage, 4.0);
        assert_eq!(weapon.shield_damage_div, 1.0);
        assert_eq!(weapon.shield_piercing, 0.0);
        assert_eq!(weapon.armor_damage, 2.0);
        assert_eq!(weapon.armor_damage_div, 0.5);
        assert_eq!(weapon.armor_piercing, 0.0);
        assert_eq!(weapon.hull_damage, 3.0);
        assert_eq!(weapon.hull_damage_div, 1.0);
    }

    #[test]
    fn test_parse_ship_designs() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_ship_design {
                id = "explorer_mk1",
                name = "Explorer Mk.I",
                hull = "corvette",
                modules = {
                    { slot_type = "engine", module = "ftl_drive" },
                    { slot_type = "utility", module = "survey_equipment" },
                },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_ship_designs(lua).unwrap();
        assert_eq!(defs.len(), 1);

        let explorer = &defs[0];
        assert_eq!(explorer.id, "explorer_mk1");
        assert_eq!(explorer.name, "Explorer Mk.I");
        assert_eq!(explorer.hull_id, "corvette");
        assert_eq!(explorer.modules.len(), 2);
        assert_eq!(explorer.modules[0].slot_type, "engine");
        assert_eq!(explorer.modules[0].module_id, "ftl_drive");
        assert_eq!(explorer.modules[1].slot_type, "utility");
        assert_eq!(explorer.modules[1].module_id, "survey_equipment");
    }

    /// Integration test: verify the actual Lua scripts load and populate registries.
    #[test]
    fn test_ship_design_scripts_load() {
        let engine = ScriptEngine::new().unwrap();

        let init_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/init.lua");
        if !init_path.exists() {
            panic!(
                "scripts/init.lua not found at {:?}",
                init_path
            );
        }

        engine.load_file(&init_path).unwrap();

        // Slot types
        let slot_types = parse_slot_types(engine.lua()).unwrap();
        assert!(
            slot_types.len() >= 4,
            "Expected at least 4 slot types, got {}",
            slot_types.len()
        );

        // Hulls
        let hulls = parse_hulls(engine.lua()).unwrap();
        assert!(
            hulls.len() >= 3,
            "Expected at least 3 hulls, got {}",
            hulls.len()
        );

        // Modules
        let modules = parse_modules(engine.lua()).unwrap();
        assert!(
            modules.len() >= 10,
            "Expected at least 10 modules, got {}",
            modules.len()
        );

        // Ship designs
        let designs = parse_ship_designs(engine.lua()).unwrap();
        assert!(
            designs.len() >= 4,
            "Expected at least 4 ship designs, got {}",
            designs.len()
        );

        // Verify a specific hull was parsed
        let corvette = hulls.iter().find(|h| h.id == "corvette");
        assert!(corvette.is_some(), "Corvette hull should be defined");
        let corvette = corvette.unwrap();
        assert_eq!(corvette.base_hp, 50.0);

        // Verify a weapon module was parsed
        let laser = modules.iter().find(|m| m.id == "weapon_laser");
        assert!(laser.is_some(), "weapon_laser module should be defined");
        assert!(laser.unwrap().weapon.is_some());
    }

    #[test]
    fn test_parse_module_with_upgrade_to() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_slot_type { id = "weapon", name = "Weapon Slot" }
            local laser = define_module {
                id = "weapon_laser",
                name = "Laser Battery",
                slot_type = "weapon",
                cost = { minerals = 50, energy = 30 },
                upgrade_to = {
                    { target = forward_ref("weapon_adv_laser"), cost = { minerals = 80, energy = 50 } },
                },
            }
            define_module {
                id = "weapon_adv_laser",
                name = "Advanced Laser",
                slot_type = "weapon",
                cost = { minerals = 100, energy = 70 },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_modules(lua).unwrap();
        assert_eq!(defs.len(), 2);

        let laser = &defs[0];
        assert_eq!(laser.id, "weapon_laser");
        assert_eq!(laser.upgrade_to.len(), 1);
        assert_eq!(laser.upgrade_to[0].target_id, "weapon_adv_laser");
        assert_eq!(laser.upgrade_to[0].cost_minerals, Amt::units(80));
        assert_eq!(laser.upgrade_to[0].cost_energy, Amt::units(50));

        let adv = &defs[1];
        assert_eq!(adv.id, "weapon_adv_laser");
        assert!(adv.upgrade_to.is_empty());
    }
}
