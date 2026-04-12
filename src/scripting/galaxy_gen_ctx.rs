//! Lua UserData context types exposed to the three galaxy-generation hooks
//! (#181):
//!
//! - `on_galaxy_generate_empty(ctx)` — Phase A. The callback populates a list
//!   of empty star systems (position + star type).
//! - `on_choose_capitals(ctx)` — Phase B. The callback picks which of the
//!   already-generated systems should become capitals, for which factions.
//! - `on_initialize_system(ctx, system)` — Phase C. Called once per system
//!   produced by Phase A; the callback can replace the default planet
//!   generation by spawning planets / overriding the system attributes.
//!
//! Each ctx is a thin, record-only UserData. The Lua side pushes intent into
//! an `Arc<Mutex<...>>` accumulator; the Rust side (galaxy::generation)
//! consumes those records after the callback returns.

use std::sync::{Arc, Mutex};

use super::helpers::extract_id_from_lua_value;

/// A `[f64; 3]` position recorded by `ctx:spawn_empty_system`.
pub type PositionF64 = [f64; 3];

/// A record produced by Lua `ctx:spawn_empty_system(name, position, star_type)`
/// (Phase A).
#[derive(Clone, Debug)]
pub struct SpawnedEmptySystemSpec {
    pub name: String,
    pub position: PositionF64,
    pub star_type: String,
}

/// Immutable snapshot of a galaxy-generation parameter set, exposed to Lua as
/// a read-only table via `ctx.settings`.
#[derive(Clone, Debug)]
pub struct GenerationSettings {
    pub num_systems: usize,
    pub num_arms: usize,
    pub galaxy_radius: f64,
    pub arm_twist: f64,
    pub arm_spread: f64,
    pub min_distance: f64,
    pub max_neighbor_distance: f64,
}

/// Actions recorded by a `on_galaxy_generate_empty` callback.
#[derive(Default, Debug, Clone)]
pub struct GalaxyGenerateActions {
    pub spawned_systems: Vec<SpawnedEmptySystemSpec>,
}

/// UserData handed to `on_galaxy_generate_empty(ctx)`.
///
/// Lua API:
/// - `ctx.settings` — table with numeric galaxy params (read-only snapshot).
/// - `ctx:spawn_empty_system(name, {x, y, z}, star_type)` — record a new
///   empty system. `star_type` accepts a string id or a `define_star_type`
///   reference.
#[derive(Clone)]
pub struct GalaxyGenerateCtx {
    pub settings: GenerationSettings,
    pub actions: Arc<Mutex<GalaxyGenerateActions>>,
}

impl GalaxyGenerateCtx {
    pub fn new(settings: GenerationSettings) -> Self {
        Self {
            settings,
            actions: Arc::new(Mutex::new(GalaxyGenerateActions::default())),
        }
    }

    pub fn take_actions(&self) -> GalaxyGenerateActions {
        std::mem::take(&mut *self.actions.lock().unwrap())
    }
}

fn parse_position(table: &mlua::Table) -> Result<PositionF64, mlua::Error> {
    // Accept either array form {x, y, z} or named { x=..., y=..., z=... }.
    if let Ok(x) = table.get::<f64>(1) {
        let y: f64 = table.get(2)?;
        let z: f64 = table.get::<Option<f64>>(3)?.unwrap_or(0.0);
        return Ok([x, y, z]);
    }
    let x: f64 = table.get("x")?;
    let y: f64 = table.get("y")?;
    let z: f64 = table.get::<Option<f64>>("z")?.unwrap_or(0.0);
    Ok([x, y, z])
}

impl mlua::UserData for GalaxyGenerateCtx {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("settings", |lua, this| {
            let t = lua.create_table()?;
            t.set("num_systems", this.settings.num_systems as i64)?;
            t.set("num_arms", this.settings.num_arms as i64)?;
            t.set("galaxy_radius", this.settings.galaxy_radius)?;
            t.set("arm_twist", this.settings.arm_twist)?;
            t.set("arm_spread", this.settings.arm_spread)?;
            t.set("min_distance", this.settings.min_distance)?;
            t.set("max_neighbor_distance", this.settings.max_neighbor_distance)?;
            Ok(t)
        });
    }

    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method(
            "spawn_empty_system",
            |_,
             this,
             (name, position, star_type): (String, mlua::Table, mlua::Value)| {
                let pos = parse_position(&position)?;
                let star_type_id = extract_id_from_lua_value(&star_type)?;
                let mut actions = this.actions.lock().unwrap();
                actions.spawned_systems.push(SpawnedEmptySystemSpec {
                    name,
                    position: pos,
                    star_type: star_type_id,
                });
                Ok(())
            },
        );
    }
}

// --- Phase B: choose capitals ------------------------------------------

/// A capital assignment record produced by Lua `ctx:assign_capital(sys_idx, faction)`.
#[derive(Clone, Debug)]
pub struct CapitalAssignmentSpec {
    /// 1-based index into the `systems` list provided to the callback.
    pub system_index: usize,
    pub faction_id: String,
}

/// Actions recorded by a `on_choose_capitals` callback.
#[derive(Default, Debug, Clone)]
pub struct ChooseCapitalsActions {
    pub assignments: Vec<CapitalAssignmentSpec>,
}

/// Read-only snapshot of a system that Phase B hooks can inspect.
#[derive(Clone, Debug)]
pub struct SystemSnapshot {
    pub name: String,
    pub position: PositionF64,
    pub star_type: String,
}

/// UserData handed to `on_choose_capitals(ctx)`.
///
/// Lua API:
/// - `ctx.factions` — sequence of faction id strings.
/// - `ctx.systems` — sequence of `{name=..., position={x,y,z}, star_type=...}`.
/// - `ctx:assign_capital(system_index, faction)` — record a capital.
#[derive(Clone)]
pub struct ChooseCapitalsCtx {
    pub systems: Vec<SystemSnapshot>,
    pub factions: Vec<String>,
    pub actions: Arc<Mutex<ChooseCapitalsActions>>,
}

impl ChooseCapitalsCtx {
    pub fn new(systems: Vec<SystemSnapshot>, factions: Vec<String>) -> Self {
        Self {
            systems,
            factions,
            actions: Arc::new(Mutex::new(ChooseCapitalsActions::default())),
        }
    }

    pub fn take_actions(&self) -> ChooseCapitalsActions {
        std::mem::take(&mut *self.actions.lock().unwrap())
    }
}

impl mlua::UserData for ChooseCapitalsCtx {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("factions", |lua, this| {
            let t = lua.create_table()?;
            for (i, id) in this.factions.iter().enumerate() {
                t.set(i + 1, id.as_str())?;
            }
            Ok(t)
        });

        fields.add_field_method_get("systems", |lua, this| {
            let arr = lua.create_table()?;
            for (i, sys) in this.systems.iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("name", sys.name.as_str())?;
                entry.set("star_type", sys.star_type.as_str())?;
                let pos = lua.create_table()?;
                pos.set(1, sys.position[0])?;
                pos.set(2, sys.position[1])?;
                pos.set(3, sys.position[2])?;
                pos.set("x", sys.position[0])?;
                pos.set("y", sys.position[1])?;
                pos.set("z", sys.position[2])?;
                entry.set("position", pos)?;
                entry.set("index", (i + 1) as i64)?;
                arr.set(i + 1, entry)?;
            }
            Ok(arr)
        });
    }

    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        // assign_capital(system_index_or_table, faction)
        // Accepts either:
        //   ctx:assign_capital(3, "humanity_empire")
        //   ctx:assign_capital(ctx.systems[3], "humanity_empire")
        // Also tolerates faction as a reference table (if factions ever get _def_type).
        methods.add_method(
            "assign_capital",
            |_,
             this,
             (sys_ref, faction): (mlua::Value, mlua::Value)| {
                let system_index = match sys_ref {
                    mlua::Value::Integer(i) => i as usize,
                    mlua::Value::Number(f) => f as usize,
                    mlua::Value::Table(t) => {
                        // Accept { index = N } — as returned by ctx.systems entries.
                        let idx: i64 = t.get("index")?;
                        idx as usize
                    }
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "assign_capital: first arg must be a system index or a system table"
                                .into(),
                        ));
                    }
                };
                let faction_id = extract_id_from_lua_value(&faction)?;
                let mut actions = this.actions.lock().unwrap();
                actions.assignments.push(CapitalAssignmentSpec {
                    system_index,
                    faction_id,
                });
                Ok(())
            },
        );
    }
}

// --- Phase C: initialize a single system -------------------------------

/// Attribute overrides for a spawned planet. Mirrors `game_start_ctx`.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct PlanetAttrsOverride {
    pub habitability: Option<f64>,
    pub mineral_richness: Option<f64>,
    pub energy_potential: Option<f64>,
    pub research_potential: Option<f64>,
    pub max_building_slots: Option<u8>,
}

fn parse_planet_attrs(table: &mlua::Table) -> Result<PlanetAttrsOverride, mlua::Error> {
    let mut spec = PlanetAttrsOverride::default();
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

/// A planet record produced by `system_ctx:spawn_planet`.
#[derive(Clone, Debug)]
pub struct InitializeSpawnedPlanet {
    pub name: String,
    pub planet_type: String,
    pub attrs: PlanetAttrsOverride,
}

/// Actions recorded by a single `on_initialize_system` callback call.
#[derive(Default, Debug, Clone)]
pub struct InitializeSystemActions {
    /// If true, the default planet-generation step is skipped entirely — only
    /// the planets spawned by the callback are created for this system.
    ///
    /// This is implicitly `true` whenever the callback spawns at least one
    /// planet. The field is exposed so that a callback that only wants to
    /// override system attributes (without planets) can opt out of the
    /// default planets explicitly.
    pub override_default_planets: bool,
    pub spawned_planets: Vec<InitializeSpawnedPlanet>,
    /// Optional override for the system's surveyed flag.
    pub surveyed: Option<bool>,
    /// Optional override for the system name.
    pub name: Option<String>,
}

/// UserData handed to `on_initialize_system(ctx, system)`.
///
/// Lua API (on `ctx`):
/// - `ctx.index` — 1-based index of the system within the generation list.
/// - `ctx.name`, `ctx.star_type`, `ctx.position` — read-only info for the system.
/// - `ctx.is_capital` — whether the system has been marked a capital in Phase B.
/// - `ctx:spawn_planet(name, type, attrs?)` — record a planet to spawn.
///   The first call implicitly disables the default planet generation.
/// - `ctx:set_attributes({ name=..., surveyed=... })` — override system-level
///   attributes.
#[derive(Clone)]
pub struct InitializeSystemCtx {
    pub index: usize,
    pub name: String,
    pub star_type: String,
    pub position: PositionF64,
    pub is_capital: bool,
    pub actions: Arc<Mutex<InitializeSystemActions>>,
}

impl InitializeSystemCtx {
    pub fn new(
        index: usize,
        name: String,
        star_type: String,
        position: PositionF64,
        is_capital: bool,
    ) -> Self {
        Self {
            index,
            name,
            star_type,
            position,
            is_capital,
            actions: Arc::new(Mutex::new(InitializeSystemActions::default())),
        }
    }

    pub fn take_actions(&self) -> InitializeSystemActions {
        std::mem::take(&mut *self.actions.lock().unwrap())
    }
}

impl mlua::UserData for InitializeSystemCtx {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("index", |_, this| Ok(this.index as i64));
        fields.add_field_method_get("name", |_, this| Ok(this.name.clone()));
        fields.add_field_method_get("star_type", |_, this| Ok(this.star_type.clone()));
        fields.add_field_method_get("is_capital", |_, this| Ok(this.is_capital));
        fields.add_field_method_get("position", |lua, this| {
            let t = lua.create_table()?;
            t.set(1, this.position[0])?;
            t.set(2, this.position[1])?;
            t.set(3, this.position[2])?;
            t.set("x", this.position[0])?;
            t.set("y", this.position[1])?;
            t.set("z", this.position[2])?;
            Ok(t)
        });
    }

    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method(
            "spawn_planet",
            |_,
             this,
             (name, planet_type, attrs): (String, mlua::Value, Option<mlua::Table>)| {
                let type_id = extract_id_from_lua_value(&planet_type)?;
                let attributes = match attrs {
                    Some(t) => parse_planet_attrs(&t)?,
                    None => PlanetAttrsOverride::default(),
                };
                let mut actions = this.actions.lock().unwrap();
                actions.override_default_planets = true;
                actions.spawned_planets.push(InitializeSpawnedPlanet {
                    name,
                    planet_type: type_id,
                    attrs: attributes,
                });
                Ok(())
            },
        );

        methods.add_method(
            "override_default_planets",
            |_, this, value: Option<bool>| {
                let mut actions = this.actions.lock().unwrap();
                actions.override_default_planets = value.unwrap_or(true);
                Ok(())
            },
        );

        methods.add_method("set_attributes", |_, this, table: mlua::Table| {
            let mut actions = this.actions.lock().unwrap();
            if let Ok(name) = table.get::<String>("name") {
                actions.name = Some(name);
            }
            if let Ok(surveyed) = table.get::<bool>("surveyed") {
                actions.surveyed = Some(surveyed);
            }
            Ok(())
        });
    }
}

// --- Hook-lookup helpers ------------------------------------------------

/// Names of the Lua global tables that store hook functions for each phase.
pub const GENERATE_EMPTY_HANDLERS: &str = "_on_galaxy_generate_empty_handlers";
pub const CHOOSE_CAPITALS_HANDLERS: &str = "_on_choose_capitals_handlers";
pub const INITIALIZE_SYSTEM_HANDLERS: &str = "_on_initialize_system_handlers";

/// Return the last registered hook function from the given handlers table, if any.
/// "Last wins" matches the semantics expected of a single-replacement hook.
pub fn last_registered_hook(
    lua: &mlua::Lua,
    table_name: &str,
) -> Result<Option<mlua::Function>, mlua::Error> {
    let Ok(handlers) = lua.globals().get::<mlua::Table>(table_name) else {
        return Ok(None);
    };
    let len = handlers.len()?;
    if len == 0 {
        return Ok(None);
    }
    let func: mlua::Function = handlers.get(len)?;
    Ok(Some(func))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    fn test_settings() -> GenerationSettings {
        GenerationSettings {
            num_systems: 100,
            num_arms: 3,
            galaxy_radius: 80.0,
            arm_twist: 2.5,
            arm_spread: 0.4,
            min_distance: 2.0,
            max_neighbor_distance: 8.0,
        }
    }

    #[test]
    fn test_generate_ctx_spawn_empty_system() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GalaxyGenerateCtx::new(test_settings());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            ctx:spawn_empty_system("Alpha", {1.0, 2.0, 3.0}, "yellow_dwarf")
            ctx:spawn_empty_system("Beta", { x = 4.0, y = 5.0, z = 6.0 }, "red_dwarf")
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert_eq!(actions.spawned_systems.len(), 2);
        assert_eq!(actions.spawned_systems[0].name, "Alpha");
        assert_eq!(actions.spawned_systems[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(actions.spawned_systems[0].star_type, "yellow_dwarf");
        assert_eq!(actions.spawned_systems[1].name, "Beta");
        assert_eq!(actions.spawned_systems[1].position, [4.0, 5.0, 6.0]);
    }

    #[test]
    fn test_generate_ctx_settings_exposed() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GalaxyGenerateCtx::new(test_settings());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        let radius: f64 = lua.load("return ctx.settings.galaxy_radius").eval().unwrap();
        assert!((radius - 80.0).abs() < 1e-10);
        let num: i64 = lua.load("return ctx.settings.num_systems").eval().unwrap();
        assert_eq!(num, 100);
    }

    #[test]
    fn test_generate_ctx_accepts_star_type_reference() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GalaxyGenerateCtx::new(test_settings());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            local star = { _def_type = "star_type", id = "yellow_dwarf" }
            ctx:spawn_empty_system("A", {0, 0, 0}, star)
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert_eq!(actions.spawned_systems[0].star_type, "yellow_dwarf");
    }

    #[test]
    fn test_choose_capitals_ctx_assignments() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let systems = vec![
            SystemSnapshot {
                name: "Sol".into(),
                position: [0.0, 0.0, 0.0],
                star_type: "yellow_dwarf".into(),
            },
            SystemSnapshot {
                name: "Beta".into(),
                position: [10.0, 0.0, 0.0],
                star_type: "red_dwarf".into(),
            },
        ];
        let ctx = ChooseCapitalsCtx::new(systems, vec!["humanity_empire".into()]);
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            ctx:assign_capital(1, ctx.factions[1])
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert_eq!(actions.assignments.len(), 1);
        assert_eq!(actions.assignments[0].system_index, 1);
        assert_eq!(actions.assignments[0].faction_id, "humanity_empire");
    }

    #[test]
    fn test_choose_capitals_ctx_assign_from_system_table() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let systems = vec![SystemSnapshot {
            name: "Sol".into(),
            position: [0.0, 0.0, 0.0],
            star_type: "yellow_dwarf".into(),
        }];
        let ctx = ChooseCapitalsCtx::new(systems, vec!["humanity_empire".into()]);
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            ctx:assign_capital(ctx.systems[1], "humanity_empire")
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert_eq!(actions.assignments[0].system_index, 1);
    }

    #[test]
    fn test_choose_capitals_ctx_exposes_fields() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let systems = vec![SystemSnapshot {
            name: "Sol".into(),
            position: [1.0, 2.0, 3.0],
            star_type: "yellow_dwarf".into(),
        }];
        let ctx = ChooseCapitalsCtx::new(systems, vec!["humanity".into(), "xeno".into()]);
        lua.globals().set("ctx", ctx.clone()).unwrap();

        let num_factions: i64 = lua.load("return #ctx.factions").eval().unwrap();
        assert_eq!(num_factions, 2);
        let num_systems: i64 = lua.load("return #ctx.systems").eval().unwrap();
        assert_eq!(num_systems, 1);
        let first_faction: String = lua.load("return ctx.factions[1]").eval().unwrap();
        assert_eq!(first_faction, "humanity");
        let first_star: String = lua.load("return ctx.systems[1].star_type").eval().unwrap();
        assert_eq!(first_star, "yellow_dwarf");
    }

    #[test]
    fn test_initialize_system_ctx_spawn_planet() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let ctx = InitializeSystemCtx::new(
            3,
            "Sol".into(),
            "yellow_dwarf".into(),
            [1.0, 2.0, 3.0],
            true,
        );
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            ctx:spawn_planet("Earth", "terrestrial", {
                habitability = 1.0,
                max_building_slots = 6,
            })
            ctx:spawn_planet("Mars", "terrestrial")
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert!(actions.override_default_planets);
        assert_eq!(actions.spawned_planets.len(), 2);
        assert_eq!(actions.spawned_planets[0].name, "Earth");
        assert_eq!(actions.spawned_planets[0].planet_type, "terrestrial");
        assert_eq!(actions.spawned_planets[0].attrs.habitability, Some(1.0));
        assert_eq!(
            actions.spawned_planets[0].attrs.max_building_slots,
            Some(6)
        );
        assert_eq!(actions.spawned_planets[1].name, "Mars");
    }

    #[test]
    fn test_initialize_system_ctx_no_planets_keeps_default() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let ctx = InitializeSystemCtx::new(
            1,
            "Test".into(),
            "yellow_dwarf".into(),
            [0.0, 0.0, 0.0],
            false,
        );
        lua.globals().set("ctx", ctx.clone()).unwrap();

        // No spawn_planet calls, so override should remain false.
        lua.load(r#"ctx:set_attributes({ name = "Renamed", surveyed = true })"#)
            .exec()
            .unwrap();

        let actions = ctx.take_actions();
        assert!(!actions.override_default_planets);
        assert!(actions.spawned_planets.is_empty());
        assert_eq!(actions.name, Some("Renamed".into()));
        assert_eq!(actions.surveyed, Some(true));
    }

    #[test]
    fn test_initialize_system_ctx_exposes_fields() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let ctx = InitializeSystemCtx::new(
            7,
            "Proxima".into(),
            "red_dwarf".into(),
            [1.5, -2.0, 0.1],
            false,
        );
        lua.globals().set("ctx", ctx.clone()).unwrap();

        let idx: i64 = lua.load("return ctx.index").eval().unwrap();
        assert_eq!(idx, 7);
        let name: String = lua.load("return ctx.name").eval().unwrap();
        assert_eq!(name, "Proxima");
        let star: String = lua.load("return ctx.star_type").eval().unwrap();
        assert_eq!(star, "red_dwarf");
        let cap: bool = lua.load("return ctx.is_capital").eval().unwrap();
        assert!(!cap);
        let x: f64 = lua.load("return ctx.position.x").eval().unwrap();
        assert!((x - 1.5).abs() < 1e-10);
    }

    #[test]
    fn test_initialize_system_ctx_explicit_override() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let ctx = InitializeSystemCtx::new(
            1,
            "Test".into(),
            "yellow_dwarf".into(),
            [0.0, 0.0, 0.0],
            false,
        );
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(r#"ctx:override_default_planets()"#).exec().unwrap();
        let actions = ctx.take_actions();
        assert!(actions.override_default_planets);
        assert!(actions.spawned_planets.is_empty());
    }

    #[test]
    fn test_last_registered_hook_returns_none_when_absent() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let result = last_registered_hook(lua, GENERATE_EMPTY_HANDLERS).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_last_registered_hook_returns_last() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on_galaxy_generate_empty(function(ctx) _first_called = true end)
            on_galaxy_generate_empty(function(ctx) _second_called = true end)
            "#,
        )
        .exec()
        .unwrap();

        let func = last_registered_hook(lua, GENERATE_EMPTY_HANDLERS)
            .unwrap()
            .expect("should find last hook");
        func.call::<()>(mlua::Value::Nil).unwrap();

        let first: Option<bool> = lua.globals().get("_first_called").unwrap();
        let second: Option<bool> = lua.globals().get("_second_called").unwrap();
        assert!(first.is_none(), "only the last registration should run");
        assert_eq!(second, Some(true));
    }
}
