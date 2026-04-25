use std::collections::HashMap;

use bevy::prelude::*;

use crate::amount::Amt;
use crate::condition::Condition;
use crate::event_system::LuaFunctionRef;
use crate::modifier::ParsedModifier;
use crate::scripting::condition_parser::parse_prerequisites_field;
use crate::scripting::modifier_api::parse_parsed_modifiers;

/// An upgrade path from one building to another.
#[derive(Clone, Debug)]
pub struct UpgradePath {
    /// Target building ID to upgrade to.
    pub target_id: String,
    /// Mineral cost of the upgrade.
    pub cost_minerals: Amt,
    /// Energy cost of the upgrade.
    pub cost_energy: Amt,
    /// Override build time for the upgrade (default: target's build_time / 2).
    pub build_time: Option<i64>,
}

/// A building definition parsed from Lua `define_building { ... }` calls.
/// This is the single source of truth for all building properties at runtime.
#[derive(Clone, Debug)]
pub struct BuildingDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub minerals_cost: Amt,
    pub energy_cost: Amt,
    pub build_time: i64,
    pub maintenance: Amt,
    pub production_bonus_minerals: Amt,
    pub production_bonus_energy: Amt,
    pub production_bonus_research: Amt,
    pub production_bonus_food: Amt,
    /// #241: Declarative modifiers (target string + base_add/multiplier/add).
    /// Replaces hardcoded `production_bonus_*`. Targets include
    /// `colony.<job>_slot` (job slot capacity), `colony.<resource>_per_hexadies`
    /// (colony aggregator), and `job:<id>::<target>` (per-job bucket).
    pub modifiers: Vec<ParsedModifier>,
    /// Whether this building is placed on a StarSystem (true) or Colony/Planet (false).
    pub is_system_building: bool,
    /// Named capabilities for special behavior (e.g. "shipyard", "port").
    pub capabilities: HashMap<String, CapabilityParams>,
    /// Available upgrade paths from this building.
    pub upgrade_to: Vec<UpgradePath>,
    /// Whether this building can be built directly (true) or only obtained via upgrade (false).
    /// Buildings with cost = nil in Lua are upgrade-only.
    pub is_direct_buildable: bool,
    /// Optional Condition tree gating construction / upgrade of this building.
    /// Populated from the Lua `prerequisites = has_tech(...)` / `all(...)` / ... field.
    pub prerequisites: Option<Condition>,
    /// #281: Optional Lua hook invoked when a building of this id finishes
    /// fresh construction (cause = "construction"). Auto-subscribed as a
    /// filtered handler on `macrocosmo:building_built` during registry load.
    pub on_built: Option<LuaFunctionRef>,
    /// #281: Optional Lua hook invoked when an upgrade to a building of this
    /// id completes (cause = "upgrade"). The event payload carries
    /// `previous_id` so the hook can distinguish upgrade sources.
    pub on_upgraded: Option<LuaFunctionRef>,
    /// #280: Whether this building can be demolished. Defaults to `true`.
    /// Hub and Capital buildings set this to `false` to prevent removal.
    pub dismantlable: bool,
    /// #385: Optional ship design id for buildings that should spawn as station
    /// ships (e.g. Shipyard → "station_shipyard_v1"). The runtime can look this
    /// up in the `ShipDesignRegistry` to spawn the corresponding station entity.
    pub ship_design_id: Option<String>,
    /// Number of colony building slots this building provides (colony hub tiers).
    /// Replaces the old `capabilities.colony_hub.fixed_slots` pattern.
    pub colony_slots: Option<usize>,
}

/// Parameters for a named building capability.
/// Supports arbitrary named parameters (e.g. `ftl_range_bonus`, `travel_time_factor`).
/// For simple capabilities, `params` may be empty or contain a single "value" entry.
#[derive(Clone, Debug, Default)]
pub struct CapabilityParams {
    pub params: HashMap<String, f64>,
}

impl CapabilityParams {
    /// Get a named parameter value, or None if not present.
    pub fn get(&self, key: &str) -> Option<f64> {
        self.params.get(key).copied()
    }

    /// Get a named parameter value, or a default if not present.
    pub fn get_or(&self, key: &str, default: f64) -> f64 {
        self.params.get(key).copied().unwrap_or(default)
    }
}

/// Strongly-typed building identifier. Wraps the string id from BuildingDefinition.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BuildingId(pub String);

impl BuildingId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BuildingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Registry of all building definitions loaded from Lua scripts.
/// Single source of truth for building properties at runtime.
#[derive(Resource, Default)]
pub struct BuildingRegistry {
    pub buildings: HashMap<String, BuildingDefinition>,
}

impl BuildingRegistry {
    /// Look up a building definition by its id.
    pub fn get(&self, id: &str) -> Option<&BuildingDefinition> {
        self.buildings.get(id)
    }

    /// Insert a building definition, replacing any existing definition with the same id.
    pub fn insert(&mut self, def: BuildingDefinition) {
        self.buildings.insert(def.id.clone(), def);
    }

    /// Return all planet-level building definitions that are directly buildable.
    pub fn planet_buildings(&self) -> Vec<&BuildingDefinition> {
        let mut result: Vec<_> = self
            .buildings
            .values()
            .filter(|b| !b.is_system_building && b.is_direct_buildable)
            .collect();
        result.sort_by(|a, b| a.id.cmp(&b.id));
        result
    }

    /// Return all system-level building definitions that are directly buildable.
    pub fn system_buildings(&self) -> Vec<&BuildingDefinition> {
        let mut result: Vec<_> = self
            .buildings
            .values()
            .filter(|b| b.is_system_building && b.is_direct_buildable)
            .collect();
        result.sort_by(|a, b| a.id.cmp(&b.id));
        result
    }

    /// Check if a building id represents a system building.
    pub fn is_system_building(&self, id: &str) -> bool {
        self.get(id).is_some_and(|b| b.is_system_building)
    }

    /// #437: Return planet-level buildings whose `prerequisites` evaluate to
    /// `true` in the given context (or have no prerequisites). Mirrors the
    /// existing `available_shipyard_deliverables` helper for deliverables.
    ///
    /// Used by the system panel build UI to filter which buildings appear as
    /// clickable buttons, and by the arrival-side validation in
    /// `colony::remote::apply_building_command` to reject `Queue` /
    /// `Upgrade` commands whose prerequisites are no longer met (e.g. a
    /// tech was cancelled between send and arrival — or the UI was
    /// bypassed entirely via a scripted/remote command).
    pub fn available_planet_buildings(
        &self,
        ctx: &crate::condition::EvalContext,
    ) -> Vec<&BuildingDefinition> {
        let mut result: Vec<_> = self
            .buildings
            .values()
            .filter(|b| !b.is_system_building && b.is_direct_buildable)
            .filter(|b| match &b.prerequisites {
                Some(cond) => cond.evaluate(ctx).is_satisfied(),
                None => true,
            })
            .collect();
        result.sort_by(|a, b| a.id.cmp(&b.id));
        result
    }

    /// #437: Return system-level buildings whose `prerequisites` evaluate to
    /// `true`. See [`Self::available_planet_buildings`].
    pub fn available_system_buildings(
        &self,
        ctx: &crate::condition::EvalContext,
    ) -> Vec<&BuildingDefinition> {
        let mut result: Vec<_> = self
            .buildings
            .values()
            .filter(|b| b.is_system_building && b.is_direct_buildable)
            .filter(|b| match &b.prerequisites {
                Some(cond) => cond.evaluate(ctx).is_satisfied(),
                None => true,
            })
            .collect();
        result.sort_by(|a, b| a.id.cmp(&b.id));
        result
    }

    /// #437: Evaluate a single building's `prerequisites` against `ctx`.
    /// Returns `true` when the building has no prerequisites or the tree
    /// evaluates as satisfied. Returns `false` if the id is unknown — an
    /// unknown building can never be built.
    pub fn prerequisites_satisfied(&self, id: &str, ctx: &crate::condition::EvalContext) -> bool {
        match self.get(id) {
            Some(def) => match &def.prerequisites {
                Some(cond) => cond.evaluate(ctx).is_satisfied(),
                None => true,
            },
            None => false,
        }
    }
}

impl BuildingDefinition {
    /// Production bonus tuple: (minerals, energy, research, food).
    pub fn production_bonus(&self) -> (Amt, Amt, Amt, Amt) {
        (
            self.production_bonus_minerals,
            self.production_bonus_energy,
            self.production_bonus_research,
            self.production_bonus_food,
        )
    }

    /// Build cost tuple: (minerals, energy).
    pub fn build_cost(&self) -> (Amt, Amt) {
        (self.minerals_cost, self.energy_cost)
    }

    /// Time to demolish (half of build time).
    pub fn demolition_time(&self) -> i64 {
        self.build_time / 2
    }

    /// Resource refund from demolition (50% of build cost).
    pub fn demolition_refund(&self) -> (Amt, Amt) {
        (
            Amt::milli(self.minerals_cost.raw() / 2),
            Amt::milli(self.energy_cost.raw() / 2),
        )
    }
}

/// Parse building definitions from the Lua `_building_definitions` global table.
/// Each entry should have at minimum `id` and `name` fields.
pub fn parse_building_definitions(lua: &mlua::Lua) -> Result<Vec<BuildingDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_building_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let description: String = table
            .get::<Option<String>>("description")?
            .unwrap_or_default();

        // Parse cost table (optional) — nil means upgrade-only
        let cost_value_check: mlua::Value = table.get("cost")?;
        let is_direct_buildable = !matches!(cost_value_check, mlua::Value::Nil);
        let (minerals_cost, energy_cost) = parse_cost_table(&table)?;

        let build_time: i64 = table.get::<Option<i64>>("build_time")?.unwrap_or(10);
        let maintenance_f64: f64 = table.get::<Option<f64>>("maintenance")?.unwrap_or(0.0);
        let maintenance = Amt::from_f64(maintenance_f64);

        // #241: legacy production_bonus is now warn-then-ignored. Emit a
        // warning if Lua still declares it (Rust zero-fills the fields, leaving
        // existing tests that compare these fields against ZERO unaffected).
        if matches!(
            table.get::<mlua::Value>("production_bonus")?,
            mlua::Value::Table(_)
        ) {
            warn!(
                "Building '{}' uses legacy `production_bonus` field; ignored. \
                 Migrate to modifiers with target = \"colony.<job>_slot\" or \
                 \"colony.<resource>_per_hexadies\" (#241).",
                id
            );
        }

        let is_system_building: bool = table
            .get::<Option<bool>>("is_system_building")?
            .unwrap_or(false);
        let capabilities = parse_capabilities_table(&table)?;
        let upgrade_to = parse_upgrade_to_table(&table)?;
        let prerequisites = parse_prerequisites_field(&table)?;
        let modifiers = parse_parsed_modifiers(&table, "modifiers", None)?;
        // #281: `on_built` fires after fresh construction completes;
        // `on_upgraded` fires after an upgrade path to this id completes.
        let on_built = crate::scripting::parse_lua_function_field(lua, &table, "on_built")?;
        let on_upgraded = crate::scripting::parse_lua_function_field(lua, &table, "on_upgraded")?;
        let dismantlable: bool = table.get::<Option<bool>>("dismantlable")?.unwrap_or(true);
        let ship_design_id: Option<String> = table.get::<Option<String>>("ship_design_id")?;

        // colony_slots: direct field, or fallback to capabilities.colony_hub.fixed_slots
        let colony_slots: Option<usize> = table
            .get::<Option<u32>>("colony_slots")?
            .map(|v| v as usize)
            .or_else(|| {
                capabilities
                    .get("colony_hub")
                    .and_then(|cap| cap.get("fixed_slots"))
                    .map(|v| v as usize)
            });

        result.push(BuildingDefinition {
            id,
            name,
            description,
            minerals_cost,
            energy_cost,
            build_time,
            maintenance,
            production_bonus_minerals: Amt::ZERO,
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers,
            is_system_building,
            capabilities,
            upgrade_to,
            is_direct_buildable,
            prerequisites,
            on_built,
            on_upgraded,
            dismantlable,
            ship_design_id,
            colony_slots,
        });
    }

    Ok(result)
}

/// Parse the `cost = { minerals = N, energy = N }` sub-table.
fn parse_cost_table(table: &mlua::Table) -> Result<(Amt, Amt), mlua::Error> {
    let cost_value: mlua::Value = table.get("cost")?;
    match cost_value {
        mlua::Value::Table(cost_table) => {
            let minerals: f64 = cost_table.get::<Option<f64>>("minerals")?.unwrap_or(0.0);
            let energy: f64 = cost_table.get::<Option<f64>>("energy")?.unwrap_or(0.0);
            Ok((Amt::from_f64(minerals), Amt::from_f64(energy)))
        }
        mlua::Value::Nil => Ok((Amt::ZERO, Amt::ZERO)),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'cost' field".to_string(),
        )),
    }
}

/// Parse the `capabilities = { name = { param = N, ... }, ... }` sub-table.
/// Supports:
/// - `capabilities = { name = true }` — empty params
/// - `capabilities = { name = { ftl_range_bonus = 10.0, travel_time_factor = 0.8 } }` — named params
/// - `capabilities = { name = { value = N } }` — legacy single-value form
fn parse_capabilities_table(
    table: &mlua::Table,
) -> Result<HashMap<String, CapabilityParams>, mlua::Error> {
    let cap_value: mlua::Value = table.get("capabilities")?;
    match cap_value {
        mlua::Value::Table(cap_table) => {
            let mut result = HashMap::new();
            for pair in cap_table.pairs::<String, mlua::Value>() {
                let (key, val) = pair?;
                let params = match val {
                    mlua::Value::Table(param_table) => {
                        let mut map = HashMap::new();
                        for kv in param_table.pairs::<String, f64>() {
                            let (k, v) = kv?;
                            map.insert(k, v);
                        }
                        CapabilityParams { params: map }
                    }
                    mlua::Value::Boolean(true) => CapabilityParams::default(),
                    _ => CapabilityParams::default(),
                };
                result.insert(key, params);
            }
            Ok(result)
        }
        mlua::Value::Nil => Ok(HashMap::new()),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'capabilities' field".to_string(),
        )),
    }
}

/// Parse the `upgrade_to = { { target = ref, cost = { minerals = N, energy = N }, build_time = N }, ... }` array.
/// The `target` field accepts string IDs, reference tables, or forward_ref tables via `extract_ref_id()`.
fn parse_upgrade_to_table(table: &mlua::Table) -> Result<Vec<UpgradePath>, mlua::Error> {
    let value: mlua::Value = table.get("upgrade_to")?;
    match value {
        mlua::Value::Table(arr) => {
            let mut result = Vec::new();
            for pair in arr.pairs::<i64, mlua::Table>() {
                let (_, entry) = pair?;
                let target_value: mlua::Value = entry.get("target")?;
                let target_id = crate::scripting::extract_ref_id(&target_value)?;

                let (cost_minerals, cost_energy) = {
                    let cost_val: mlua::Value = entry.get("cost")?;
                    match cost_val {
                        mlua::Value::Table(cost_table) => {
                            let m: f64 = cost_table.get::<Option<f64>>("minerals")?.unwrap_or(0.0);
                            let e: f64 = cost_table.get::<Option<f64>>("energy")?.unwrap_or(0.0);
                            (Amt::from_f64(m), Amt::from_f64(e))
                        }
                        mlua::Value::Nil => (Amt::ZERO, Amt::ZERO),
                        _ => {
                            return Err(mlua::Error::RuntimeError(
                                "Expected table or nil for upgrade 'cost' field".to_string(),
                            ));
                        }
                    }
                };

                let build_time: Option<i64> = entry.get::<Option<i64>>("build_time")?;

                result.push(UpgradePath {
                    target_id,
                    cost_minerals,
                    cost_energy,
                    build_time,
                });
            }
            Ok(result)
        }
        mlua::Value::Nil => Ok(Vec::new()),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'upgrade_to' field".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_parse_building_definitions() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_building {
                id = "mine",
                name = "Mine",
                cost = { minerals = 150, energy = 50 },
                build_time = 10,
                maintenance = 0.2,
                modifiers = {
                    { target = "colony.miner_slot", base_add = 5 },
                },
            }
            define_building {
                id = "farm",
                name = "Farm",
                cost = { minerals = 100, energy = 50 },
                build_time = 20,
                maintenance = 0.3,
                modifiers = {
                    { target = "colony.farmer_slot", base_add = 5 },
                },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_building_definitions(lua).unwrap();
        assert_eq!(defs.len(), 2);

        // Mine
        assert_eq!(defs[0].id, "mine");
        assert_eq!(defs[0].name, "Mine");
        assert_eq!(defs[0].minerals_cost, Amt::units(150));
        assert_eq!(defs[0].energy_cost, Amt::units(50));
        assert_eq!(defs[0].build_time, 10);
        assert_eq!(defs[0].maintenance, Amt::new(0, 200));
        // #241: production_bonus_* are zero under new modifier-based scheme.
        assert_eq!(defs[0].production_bonus_minerals, Amt::ZERO);
        assert_eq!(defs[0].modifiers.len(), 1);
        assert_eq!(defs[0].modifiers[0].target, "colony.miner_slot");
        assert!((defs[0].modifiers[0].base_add - 5.0).abs() < 1e-10);

        // Farm
        assert_eq!(defs[1].id, "farm");
        assert_eq!(defs[1].name, "Farm");
        assert_eq!(defs[1].modifiers.len(), 1);
        assert_eq!(defs[1].modifiers[0].target, "colony.farmer_slot");
    }

    #[test]
    fn test_parse_building_minimal() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_building {
                id = "basic",
                name = "Basic Building",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_building_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "basic");
        assert_eq!(defs[0].name, "Basic Building");
        assert_eq!(defs[0].minerals_cost, Amt::ZERO);
        assert_eq!(defs[0].energy_cost, Amt::ZERO);
        assert_eq!(defs[0].build_time, 10); // default
        assert_eq!(defs[0].maintenance, Amt::ZERO);
        assert_eq!(defs[0].production_bonus_minerals, Amt::ZERO);
        assert_eq!(defs[0].production_bonus_energy, Amt::ZERO);
        assert_eq!(defs[0].production_bonus_research, Amt::ZERO);
        assert_eq!(defs[0].production_bonus_food, Amt::ZERO);
    }

    #[test]
    fn test_building_registry_lookup() {
        let mut registry = BuildingRegistry::default();
        assert!(registry.get("mine").is_none());

        registry.insert(BuildingDefinition {
            id: "mine".to_string(),
            name: "Mine".to_string(),
            description: String::new(),
            minerals_cost: Amt::units(150),
            energy_cost: Amt::units(50),
            build_time: 10,
            maintenance: Amt::new(0, 200),
            production_bonus_minerals: Amt::units(3),
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers: Vec::new(),
            is_system_building: false,
            capabilities: HashMap::new(),
            upgrade_to: Vec::new(),
            is_direct_buildable: true,
            prerequisites: None,
            on_built: None,
            on_upgraded: None,
            dismantlable: true,
            ship_design_id: None,
            colony_slots: None,
        });

        let mine = registry.get("mine").unwrap();
        assert_eq!(mine.name, "Mine");
        assert_eq!(mine.minerals_cost, Amt::units(150));
        assert_eq!(mine.production_bonus_minerals, Amt::units(3));

        // Insert another
        registry.insert(BuildingDefinition {
            id: "farm".to_string(),
            name: "Farm".to_string(),
            description: String::new(),
            minerals_cost: Amt::units(100),
            energy_cost: Amt::units(50),
            build_time: 20,
            maintenance: Amt::new(0, 300),
            production_bonus_minerals: Amt::ZERO,
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::units(5),
            modifiers: Vec::new(),
            is_system_building: false,
            capabilities: HashMap::new(),
            upgrade_to: Vec::new(),
            is_direct_buildable: true,
            prerequisites: None,
            on_built: None,
            on_upgraded: None,
            dismantlable: true,
            ship_design_id: None,
            colony_slots: None,
        });

        assert_eq!(registry.buildings.len(), 2);
        assert!(registry.get("farm").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    /// Verify BuildingRegistry loaded from a Lua fixture.
    /// Uses a dedicated test fixture to decouple from production Lua content.
    #[test]
    fn test_building_registry_loaded_from_lua() {
        let engine = ScriptEngine::new().unwrap();

        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/buildings_test.lua");
        assert!(fixture.exists(), "fixture not found at {fixture:?}");
        engine.load_file(&fixture).unwrap();

        let defs = parse_building_definitions(engine.lua()).unwrap();
        assert_eq!(defs.len(), 6, "fixture defines exactly 6 buildings");

        let mut registry = BuildingRegistry::default();
        for def in &defs {
            registry.insert(def.clone());
        }

        // Mine: planet building, correct cost/modifiers.
        let mine = registry
            .get("test_mine")
            .expect("test_mine should be in registry");
        assert_eq!(mine.name, "Test Mine");
        assert_eq!(mine.minerals_cost, Amt::units(100));
        assert_eq!(mine.energy_cost, Amt::units(25));
        assert_eq!(mine.build_time, 5);
        assert!(!mine.is_system_building);
        assert!(
            mine.modifiers
                .iter()
                .any(|m| m.target == "colony.miner_slot" && (m.base_add - 3.0).abs() < 1e-10),
            "test_mine should declare colony.miner_slot +3"
        );

        // Farm: planet building with farmer_slot modifier.
        let farm = registry
            .get("test_farm")
            .expect("test_farm should be in registry");
        assert!(
            farm.modifiers
                .iter()
                .any(|m| m.target == "colony.farmer_slot"),
            "test_farm should declare colony.farmer_slot"
        );

        // Shipyard: system building with shipyard capability.
        let shipyard = registry
            .get("test_shipyard")
            .expect("test_shipyard should be in registry");
        assert!(shipyard.is_system_building);
        assert!(shipyard.capabilities.contains_key("shipyard"));

        // Port: system building with port capability.
        let port = registry
            .get("test_port")
            .expect("test_port should be in registry");
        assert!(port.is_system_building);
        assert!(port.capabilities.contains_key("port"));
    }

    #[test]
    fn test_building_registry_replace() {
        let mut registry = BuildingRegistry::default();

        registry.insert(BuildingDefinition {
            id: "mine".to_string(),
            name: "Mine".to_string(),
            description: String::new(),
            minerals_cost: Amt::units(150),
            energy_cost: Amt::units(50),
            build_time: 10,
            maintenance: Amt::new(0, 200),
            production_bonus_minerals: Amt::units(3),
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers: Vec::new(),
            is_system_building: false,
            capabilities: HashMap::new(),
            upgrade_to: Vec::new(),
            is_direct_buildable: true,
            prerequisites: None,
            on_built: None,
            on_upgraded: None,
            dismantlable: true,
            ship_design_id: None,
            colony_slots: None,
        });

        // Replace with updated values
        registry.insert(BuildingDefinition {
            id: "mine".to_string(),
            name: "Advanced Mine".to_string(),
            description: String::new(),
            minerals_cost: Amt::units(200),
            energy_cost: Amt::units(75),
            build_time: 15,
            maintenance: Amt::new(0, 300),
            production_bonus_minerals: Amt::units(5),
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers: Vec::new(),
            is_system_building: false,
            capabilities: HashMap::new(),
            upgrade_to: Vec::new(),
            is_direct_buildable: true,
            prerequisites: None,
            on_built: None,
            on_upgraded: None,
            dismantlable: true,
            ship_design_id: None,
            colony_slots: None,
        });

        assert_eq!(registry.buildings.len(), 1);
        let mine = registry.get("mine").unwrap();
        assert_eq!(mine.name, "Advanced Mine");
        assert_eq!(mine.production_bonus_minerals, Amt::units(5));
    }

    #[test]
    fn test_parse_building_with_upgrade_to() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            local mine = define_building {
                id = "mine",
                name = "Mine",
                cost = { minerals = 150, energy = 50 },
                build_time = 10,
                maintenance = 0.2,
                modifiers = { { target = "colony.miner_slot", base_add = 5 } },
                upgrade_to = {
                    { target = forward_ref("advanced_mine"), cost = { minerals = 200, energy = 100 }, build_time = 8 },
                },
            }
            define_building {
                id = "advanced_mine",
                name = "Advanced Mine",
                cost = nil,
                build_time = 10,
                maintenance = 0.4,
                modifiers = { { target = "colony.miner_slot", base_add = 10 } },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_building_definitions(lua).unwrap();
        assert_eq!(defs.len(), 2);

        // Mine should have an upgrade path
        let mine = &defs[0];
        assert_eq!(mine.id, "mine");
        assert!(mine.is_direct_buildable);
        assert_eq!(mine.upgrade_to.len(), 1);
        assert_eq!(mine.upgrade_to[0].target_id, "advanced_mine");
        assert_eq!(mine.upgrade_to[0].cost_minerals, Amt::units(200));
        assert_eq!(mine.upgrade_to[0].cost_energy, Amt::units(100));
        assert_eq!(mine.upgrade_to[0].build_time, Some(8));

        // Advanced Mine should be upgrade-only
        let adv_mine = &defs[1];
        assert_eq!(adv_mine.id, "advanced_mine");
        assert!(!adv_mine.is_direct_buildable);
        assert_eq!(adv_mine.minerals_cost, Amt::ZERO);
        assert_eq!(adv_mine.energy_cost, Amt::ZERO);
        assert_eq!(adv_mine.modifiers.len(), 1);
        assert!((adv_mine.modifiers[0].base_add - 10.0).abs() < 1e-10);
        assert_eq!(adv_mine.maintenance, Amt::new(0, 400));
    }

    #[test]
    fn test_registry_filters_non_direct_buildable() {
        let mut registry = BuildingRegistry::default();

        // Direct-buildable planet building
        registry.insert(BuildingDefinition {
            id: "mine".to_string(),
            name: "Mine".to_string(),
            description: String::new(),
            minerals_cost: Amt::units(150),
            energy_cost: Amt::units(50),
            build_time: 10,
            maintenance: Amt::new(0, 200),
            production_bonus_minerals: Amt::units(3),
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers: Vec::new(),
            is_system_building: false,
            capabilities: HashMap::new(),
            upgrade_to: Vec::new(),
            is_direct_buildable: true,
            prerequisites: None,
            on_built: None,
            on_upgraded: None,
            dismantlable: true,
            ship_design_id: None,
            colony_slots: None,
        });

        // Upgrade-only planet building
        registry.insert(BuildingDefinition {
            id: "advanced_mine".to_string(),
            name: "Advanced Mine".to_string(),
            description: String::new(),
            minerals_cost: Amt::ZERO,
            energy_cost: Amt::ZERO,
            build_time: 10,
            maintenance: Amt::new(0, 400),
            production_bonus_minerals: Amt::units(6),
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers: Vec::new(),
            is_system_building: false,
            capabilities: HashMap::new(),
            upgrade_to: Vec::new(),
            is_direct_buildable: false,
            prerequisites: None,
            on_built: None,
            on_upgraded: None,
            dismantlable: true,
            ship_design_id: None,
            colony_slots: None,
        });

        // planet_buildings() should only return direct-buildable ones
        let planet = registry.planet_buildings();
        assert_eq!(planet.len(), 1);
        assert_eq!(planet[0].id, "mine");

        // But the registry still has both
        assert!(registry.get("advanced_mine").is_some());
    }

    #[test]
    fn test_building_api_parses_prerequisites_field() {
        use crate::condition::{Condition, ConditionAtom};

        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_building {
                id = "plain",
                name = "Plain",
            }
            define_building {
                id = "tech_gated",
                name = "Tech Gated",
                prerequisites = has_tech("industrial_automated_mining"),
            }
            define_building {
                id = "complex_gated",
                name = "Complex Gated",
                prerequisites = all(
                    has_tech("tech_a"),
                    any(has_tech("tech_b"), has_flag("enabled"))
                ),
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_building_definitions(lua).unwrap();
        assert_eq!(defs.len(), 3);

        assert!(defs[0].prerequisites.is_none());

        assert_eq!(
            defs[1].prerequisites,
            Some(Condition::Atom(ConditionAtom::has_tech(
                "industrial_automated_mining"
            )))
        );

        assert!(matches!(&defs[2].prerequisites, Some(Condition::All(_))));
    }

    #[test]
    fn test_building_api_prerequisites_from_lua_file_wires_advanced_mine() {
        // Verify prerequisites are parsed from Lua fixture.
        use crate::condition::{AtomKind, Condition};

        let engine = ScriptEngine::new().unwrap();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/buildings_test.lua");
        assert!(fixture.exists(), "fixture not found at {fixture:?}");
        engine.load_file(&fixture).unwrap();

        let defs = parse_building_definitions(engine.lua()).unwrap();
        let by_id: std::collections::HashMap<_, _> =
            defs.iter().map(|d| (d.id.as_str(), d)).collect();

        let adv_mine = by_id
            .get("test_advanced_mine")
            .expect("test_advanced_mine should be in fixture");
        match &adv_mine.prerequisites {
            Some(Condition::Atom(atom)) => match &atom.kind {
                AtomKind::HasTech(id) => assert_eq!(id, "industrial_automated_mining"),
                other => panic!("expected HasTech atom, got {:?}", other),
            },
            other => panic!(
                "expected test_advanced_mine prerequisites to be a HasTech atom, got {:?}",
                other
            ),
        }

        // test_mine should have no prerequisites
        let mine = by_id.get("test_mine").unwrap();
        assert!(mine.prerequisites.is_none());
    }

    /// #385: `ship_design_id` parses from Lua and defaults to None.
    #[test]
    fn test_building_ship_design_id_parses() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_building {
                id = "plain",
                name = "Plain",
            }
            define_building {
                id = "shipyard",
                name = "Shipyard",
                ship_design_id = "station_shipyard_v1",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_building_definitions(lua).unwrap();
        assert_eq!(defs.len(), 2);

        assert!(
            defs[0].ship_design_id.is_none(),
            "plain building should have no ship_design_id"
        );
        assert_eq!(
            defs[1].ship_design_id.as_deref(),
            Some("station_shipyard_v1"),
            "shipyard should map to station_shipyard_v1"
        );
    }

    /// #385: Building definitions from Lua fixture wire ship_design_id for
    /// system buildings.
    #[test]
    fn test_building_ship_design_id_from_lua_file() {
        let engine = ScriptEngine::new().unwrap();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/buildings_test.lua");
        assert!(fixture.exists(), "fixture not found at {fixture:?}");
        engine.load_file(&fixture).unwrap();

        let defs = parse_building_definitions(engine.lua()).unwrap();
        let by_id: std::collections::HashMap<_, _> =
            defs.iter().map(|d| (d.id.as_str(), d)).collect();

        assert_eq!(
            by_id.get("test_port").unwrap().ship_design_id.as_deref(),
            Some("station_port_v1"),
        );
        assert_eq!(
            by_id
                .get("test_research_lab")
                .unwrap()
                .ship_design_id
                .as_deref(),
            Some("station_research_lab_v1"),
        );
        // Mine should have no ship_design_id
        assert!(by_id.get("test_mine").unwrap().ship_design_id.is_none());
    }
}
