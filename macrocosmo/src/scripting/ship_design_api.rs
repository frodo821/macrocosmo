use crate::amount::Amt;
use crate::scripting::condition_parser::parse_prerequisites_field;
use crate::ship_design::{
    DesignSlotAssignment, HullDefinition, HullSlot, ModuleDefinition, ModuleModifier, ModuleSize,
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
        let description: String = table
            .get::<Option<String>>("description")?
            .unwrap_or_default();
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

        // Parse optional prerequisites (shared helper).
        let prerequisites = parse_prerequisites_field(&table)?;

        // #382: size is mandatory — mlua will error if missing.
        let size: u32 = table.get("size")?;
        let is_capital: bool = table.get::<Option<bool>>("is_capital")?.unwrap_or(false);

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
            prerequisites,
            size,
            is_capital,
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
        let description: String = table
            .get::<Option<String>>("description")?
            .unwrap_or_default();
        let slot_type_value: mlua::Value = table.get("slot_type")?;
        let slot_type = crate::scripting::extract_ref_id(&slot_type_value)?;

        // Parse modifiers array
        let modifiers = parse_module_modifiers(&table)?;

        // Parse weapon stats (optional)
        let weapon = parse_weapon_stats(&table)?;

        // Parse cost table
        let (cost_minerals, cost_energy) = parse_cost_table(&table, "cost")?;

        // Parse upgrade_to array (optional)
        let upgrade_to = parse_module_upgrade_to(&table)?;

        // #226: prerequisites (Condition tree). Hard migration — the legacy
        // `prerequisite_tech = "foo"` field is no longer read; any Lua that
        // still sets it will have its value silently dropped.
        let prerequisites = parse_prerequisites_field(&table)?;

        // #239: optional `build_time` field (hexadies). Defaults to 0 when
        // omitted so a module that's cheap in time authoring-wise stays
        // invisible to the final sum.
        let build_time: i64 = table.get::<Option<i64>>("build_time")?.unwrap_or(0);

        // #138: Power budget fields (default 0) and module size (default Small).
        let power_cost: i32 = table.get::<Option<i32>>("power_cost")?.unwrap_or(0);
        let power_output: i32 = table.get::<Option<i32>>("power_output")?.unwrap_or(0);
        let size = table
            .get::<Option<String>>("size")?
            .map(|s| ModuleSize::from_str_loose(&s))
            .unwrap_or(ModuleSize::Small);

        result.push(ModuleDefinition {
            id,
            name,
            description,
            slot_type,
            modifiers,
            weapon,
            cost_minerals,
            cost_energy,
            prerequisites,
            upgrade_to,
            build_time,
            power_cost,
            power_output,
            size,
        });
    }

    Ok(result)
}

/// Parse ship design definitions from the Lua `_ship_design_definitions` global table.
///
/// #236: Derived fields (`hp`, `sublight_speed`, `ftl_range`, `build_cost`,
/// `build_time`, `maintenance`, `can_survey`, `can_colonize`) are no longer
/// read from Lua. They are computed from the hull + modules by
/// `ship_design::apply_derived_to_definition` after parse. Authoring any of
/// those fields emits a `warn!` — the value is ignored but not an error, so
/// existing preset files continue to load.
pub fn parse_ship_designs(lua: &mlua::Lua) -> Result<Vec<ShipDesignDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_ship_design_definitions")?;
    let mut result = Vec::new();

    // Names of derived fields that Lua is no longer allowed to author.
    const DERIVED_FIELDS: &[&str] = &[
        "hp",
        "sublight_speed",
        "ftl_range",
        "build_cost",
        "build_time",
        "maintenance",
        "can_survey",
        "can_colonize",
    ];

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let description: String = table
            .get::<Option<String>>("description")?
            .unwrap_or_default();
        let hull_value: mlua::Value = table.get("hull")?;
        let hull_id = crate::scripting::extract_ref_id(&hull_value)?;

        // Parse modules array
        let modules = parse_design_modules(&table)?;

        // #236: Warn-then-ignore for any authored derived field. Parse as a
        // generic Value so we detect presence regardless of type.
        for field in DERIVED_FIELDS {
            let v: mlua::Value = table.get(*field)?;
            if !matches!(v, mlua::Value::Nil) {
                bevy::log::warn!(
                    "ship design '{}' authors derived field '{}'; value will be ignored (#236: derive from hull + modules)",
                    id,
                    field
                );
            }
        }

        // Zero-init derived fields; `apply_derived_to_definition` fills them
        // after validation at registry-load time.
        result.push(ShipDesignDefinition {
            id,
            name,
            description,
            hull_id,
            modules,
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::ZERO,
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 0,
            hp: 0.0,
            sublight_speed: 0.0,
            ftl_range: 0.0,
            revision: 0,
            is_direct_buildable: false,
        });
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Helper parsers
// ---------------------------------------------------------------------------

/// Parse the `cost = { minerals = N, energy = N }` or `build_cost = { ... }` sub-table.
fn parse_cost_table(table: &mlua::Table, field_name: &str) -> Result<(Amt, Amt), mlua::Error> {
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
                // #138: Optional `size` field — max module size this slot accepts.
                // Defaults to Large (accept anything) when omitted.
                let max_size = slot_table
                    .get::<Option<String>>("size")?
                    .map(|s| ModuleSize::from_str_loose(&s))
                    .unwrap_or(ModuleSize::Large);
                slots.push(HullSlot {
                    slot_type,
                    count,
                    max_size,
                });
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
fn parse_design_modules(table: &mlua::Table) -> Result<Vec<DesignSlotAssignment>, mlua::Error> {
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
            define_slot_type { id = "ftl", name = "FTL Drive Slot" }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_slot_types(lua).unwrap();
        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0].id, "weapon");
        assert_eq!(defs[0].name, "Weapon Slot");
        assert_eq!(defs[1].id, "utility");
        assert_eq!(defs[2].id, "ftl");
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
                size = 1,
                base_hp = 50,
                base_speed = 0.75,
                base_evasion = 30.0,
                slots = {
                    { type = "weapon", count = 2 },
                    { type = "utility", count = 1 },
                    { type = "ftl", count = 1 },
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
        assert_eq!(corvette.slots[2].slot_type, "ftl");
        assert_eq!(corvette.slots[2].count, 1);
        assert_eq!(corvette.build_cost_minerals, Amt::units(200));
        assert_eq!(corvette.build_cost_energy, Amt::units(100));
        assert_eq!(corvette.build_time, 60);
        assert_eq!(corvette.maintenance, Amt::new(0, 500));
        assert_eq!(corvette.size, 1);
        assert!(!corvette.is_capital);
    }

    #[test]
    fn test_hull_parses_prerequisites() {
        use crate::condition::{Condition, ConditionAtom};

        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_hull {
                id = "plain_hull",
                name = "Plain",
                size = 1,
                base_hp = 30,
            }
            define_hull {
                id = "cruiser",
                name = "Cruiser",
                size = 4,
                base_hp = 200,
                prerequisites = has_tech("hull_cruiser"),
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_hulls(lua).unwrap();
        assert_eq!(defs.len(), 2);

        let plain = defs.iter().find(|h| h.id == "plain_hull").unwrap();
        assert!(plain.prerequisites.is_none());

        let cruiser = defs.iter().find(|h| h.id == "cruiser").unwrap();
        assert_eq!(
            cruiser.prerequisites,
            Some(Condition::Atom(ConditionAtom::has_tech("hull_cruiser")))
        );
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
                slot_type = "ftl",
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
        assert_eq!(ftl.slot_type, "ftl");
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

        // #239: build_time defaults to 0 when the Lua table omits the field.
        assert_eq!(ftl.build_time, 0);
        assert_eq!(armor.build_time, 0);
    }

    /// #239: `build_time` on a module is parsed and round-trips through
    /// `parse_modules`.
    #[test]
    fn test_parse_modules_reads_build_time() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_module {
                id = "heavy_drive",
                name = "Heavy Drive",
                slot_type = "ftl",
                cost = { minerals = 100, energy = 50 },
                build_time = 15,
            }
            define_module {
                id = "cheap_cargo",
                name = "Cheap Cargo",
                slot_type = "utility",
                cost = { minerals = 30 },
                build_time = 5,
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_modules(lua).unwrap();
        assert_eq!(defs.len(), 2);
        let heavy = defs.iter().find(|d| d.id == "heavy_drive").unwrap();
        let cheap = defs.iter().find(|d| d.id == "cheap_cargo").unwrap();
        assert_eq!(heavy.build_time, 15);
        assert_eq!(cheap.build_time, 5);
    }

    /// #138: power_cost, power_output, and size are parsed from Lua module
    /// definitions. Missing fields default to 0/0/Small.
    #[test]
    fn test_parse_modules_reads_power_and_size() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_module {
                id = "reactor",
                name = "Reactor",
                slot_type = "reactor",
                power_output = 10,
                cost = { minerals = 80 },
            }
            define_module {
                id = "laser",
                name = "Laser",
                slot_type = "weapon",
                power_cost = 3,
                size = "medium",
                cost = { minerals = 50 },
            }
            define_module {
                id = "plain",
                name = "Plain",
                slot_type = "utility",
                cost = { minerals = 10 },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_modules(lua).unwrap();
        assert_eq!(defs.len(), 3);

        let reactor = defs.iter().find(|d| d.id == "reactor").unwrap();
        assert_eq!(reactor.power_output, 10);
        assert_eq!(reactor.power_cost, 0);
        assert_eq!(reactor.size, ModuleSize::Small);

        let laser = defs.iter().find(|d| d.id == "laser").unwrap();
        assert_eq!(laser.power_cost, 3);
        assert_eq!(laser.power_output, 0);
        assert_eq!(laser.size, ModuleSize::Medium);

        let plain = defs.iter().find(|d| d.id == "plain").unwrap();
        assert_eq!(plain.power_cost, 0);
        assert_eq!(plain.power_output, 0);
        assert_eq!(plain.size, ModuleSize::Small);
    }

    /// #138: hull slot `size` field is parsed as max_size.
    #[test]
    fn test_parse_hull_slot_size() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_hull {
                id = "sized_hull",
                name = "Sized Hull",
                size = 1,
                base_hp = 50,
                slots = {
                    { type = "weapon", size = "small", count = 2 },
                    { type = "utility", count = 1 },
                    { type = "defense", size = "large", count = 1 },
                },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_hulls(lua).unwrap();
        assert_eq!(defs.len(), 1);

        let hull = &defs[0];
        assert_eq!(hull.slots.len(), 3);
        // Weapon slot: size = "small"
        assert_eq!(hull.slots[0].slot_type, "weapon");
        assert_eq!(hull.slots[0].max_size, ModuleSize::Small);
        // Utility slot: no size → defaults to Large
        assert_eq!(hull.slots[1].slot_type, "utility");
        assert_eq!(hull.slots[1].max_size, ModuleSize::Large);
        // Defense slot: size = "large"
        assert_eq!(hull.slots[2].slot_type, "defense");
        assert_eq!(hull.slots[2].max_size, ModuleSize::Large);
    }

    #[test]
    fn test_module_parses_prerequisites() {
        use crate::condition::{Condition, ConditionAtom};

        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_module {
                id = "plain",
                name = "Plain",
                slot_type = "utility",
                cost = { minerals = 10 },
            }
            define_module {
                id = "advanced",
                name = "Advanced",
                slot_type = "utility",
                prerequisites = all(has_tech("laser_weapons"), has_flag("militarized")),
                cost = { minerals = 10 },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_modules(lua).unwrap();
        assert_eq!(defs.len(), 2);

        let plain = defs.iter().find(|m| m.id == "plain").unwrap();
        assert!(plain.prerequisites.is_none());

        let advanced = defs.iter().find(|m| m.id == "advanced").unwrap();
        assert_eq!(
            advanced.prerequisites,
            Some(Condition::All(vec![
                Condition::Atom(ConditionAtom::has_tech("laser_weapons")),
                Condition::Atom(ConditionAtom::has_flag("militarized")),
            ]))
        );
    }

    #[test]
    fn test_module_ignores_legacy_prerequisite_tech_field() {
        // #226 hard migration: `prerequisite_tech = "..."` is no longer read.
        // A module that only sets the legacy field ends up with no prerequisites.
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_module {
                id = "legacy",
                name = "Legacy",
                slot_type = "utility",
                prerequisite_tech = "old_tech_string",
                cost = { minerals = 10 },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_modules(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert!(
            defs[0].prerequisites.is_none(),
            "legacy prerequisite_tech must be dropped silently"
        );
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
                    { slot_type = "ftl", module = "ftl_drive" },
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
        assert_eq!(explorer.modules[0].slot_type, "ftl");
        assert_eq!(explorer.modules[0].module_id, "ftl_drive");
        assert_eq!(explorer.modules[1].slot_type, "utility");
        assert_eq!(explorer.modules[1].module_id, "survey_equipment");
    }

    /// Integration test: verify the actual Lua scripts load and populate registries.
    #[test]
    fn test_ship_design_scripts_load() {
        let engine = ScriptEngine::new().unwrap();

        let init_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/init.lua");
        if !init_path.exists() {
            panic!("scripts/init.lua not found at {:?}", init_path);
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

        // Verify a weapon module was parsed (#403: size variants renamed to _s/_m/_l)
        let laser = modules.iter().find(|m| m.id == "weapon_laser_s");
        assert!(laser.is_some(), "weapon_laser_s module should be defined");
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

    /// #236: Authored derived fields (hp/ftl_range/...) must be zero-init on
    /// the parsed definition. The registry loader overwrites them via
    /// `apply_derived_to_definition` — but at parse time the value from Lua
    /// must NOT be propagated.
    #[test]
    fn test_lua_authored_derived_fields_are_ignored() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_ship_design {
                id = "bogus",
                name = "Bogus",
                hull = "corvette",
                modules = {},
                -- Every one of these fields is a "derived" field that used
                -- to be authored in Lua. All must be ignored post-#236.
                hp = 999,
                sublight_speed = 99.0,
                ftl_range = 999.0,
                maintenance = 99.0,
                build_cost = { minerals = 9999, energy = 9999 },
                build_time = 9999,
                can_survey = true,
                can_colonize = true,
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_ship_designs(lua).unwrap();
        assert_eq!(defs.len(), 1);
        let d = &defs[0];
        // All derived fields must be zero/false post-parse. The registry
        // loader re-populates them via hull + modules.
        assert_eq!(d.hp, 0.0, "hp authored in Lua must be ignored");
        assert_eq!(d.sublight_speed, 0.0);
        assert_eq!(
            d.ftl_range, 0.0,
            "ftl_range authored in Lua must be ignored"
        );
        assert_eq!(d.maintenance, Amt::ZERO);
        assert_eq!(d.build_cost_minerals, Amt::ZERO);
        assert_eq!(d.build_cost_energy, Amt::ZERO);
        assert_eq!(d.build_time, 0);
        assert!(!d.can_survey);
        assert!(!d.can_colonize);
        assert!(
            !d.is_direct_buildable,
            "is_direct_buildable is derived, must be false post-parse"
        );
    }

    /// #382: `size` is mandatory — parsing a hull without it must error.
    #[test]
    fn test_hull_size_is_mandatory() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_hull {
                id = "no_size",
                name = "No Size",
                base_hp = 30,
            }
            "#,
        )
        .exec()
        .unwrap();

        let result = parse_hulls(lua);
        assert!(result.is_err(), "hull without `size` must fail to parse");
    }

    /// #382: `is_capital` defaults to false when omitted.
    #[test]
    fn test_hull_is_capital_defaults_false() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_hull {
                id = "basic",
                name = "Basic",
                size = 2,
                base_hp = 50,
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_hulls(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert!(!defs[0].is_capital);
    }

    /// #385: Station hull definitions parse from Lua files and appear in
    /// HullRegistry with the expected properties (immobile, size 10000).
    #[test]
    fn test_station_hulls_parse_from_lua() {
        let engine = ScriptEngine::new().unwrap();
        let init = engine.scripts_dir().join("init.lua");
        engine.load_file(&init).unwrap();

        let hulls = parse_hulls(engine.lua()).unwrap();
        let by_id: std::collections::HashMap<_, _> =
            hulls.iter().map(|h| (h.id.as_str(), h)).collect();

        for id in &[
            "station_shipyard_hull",
            "station_port_hull",
            "station_research_lab_hull",
        ] {
            let hull = by_id
                .get(id)
                .unwrap_or_else(|| panic!("Station hull '{}' should be in hull definitions", id));
            assert_eq!(hull.size, 10000, "{} should have size 10000", id);
            assert!(!hull.is_capital, "{} should not be capital", id);
            assert_eq!(hull.base_speed, 0.0, "{} should be immobile", id);
            assert_eq!(hull.base_evasion, 0.0, "{} should have zero evasion", id);
        }
    }

    /// #385: Station ship designs parse from Lua and appear in the design list.
    #[test]
    fn test_station_designs_parse_from_lua() {
        let engine = ScriptEngine::new().unwrap();
        let init = engine.scripts_dir().join("init.lua");
        engine.load_file(&init).unwrap();

        let designs = parse_ship_designs(engine.lua()).unwrap();
        let ids: Vec<&str> = designs.iter().map(|d| d.id.as_str()).collect();

        assert!(
            ids.contains(&"station_shipyard_v1"),
            "station_shipyard_v1 missing from designs"
        );
        assert!(
            ids.contains(&"station_port_v1"),
            "station_port_v1 missing from designs"
        );
        assert!(
            ids.contains(&"station_research_lab_v1"),
            "station_research_lab_v1 missing from designs"
        );

        // Verify hull references
        let shipyard = designs
            .iter()
            .find(|d| d.id == "station_shipyard_v1")
            .unwrap();
        assert_eq!(shipyard.hull_id, "station_shipyard_hull");
        assert_eq!(shipyard.modules.len(), 1);
        assert_eq!(shipyard.modules[0].module_id, "shipyard_bay");

        let port = designs.iter().find(|d| d.id == "station_port_v1").unwrap();
        assert_eq!(port.hull_id, "station_port_hull");

        let lab = designs
            .iter()
            .find(|d| d.id == "station_research_lab_v1")
            .unwrap();
        assert_eq!(lab.hull_id, "station_research_lab_hull");
    }
}
