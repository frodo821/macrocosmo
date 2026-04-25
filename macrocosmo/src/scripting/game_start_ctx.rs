use std::sync::{Arc, Mutex};

use super::extract_id_from_lua_value;

/// A reference to a planet — either an existing (galaxy-generated) one by
/// 1-based index, or a planet that this on_game_start callback spawned.
///
/// Spawned planets are referred to by 1-based index into `GameStartActions::spawned_planets`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanetRef {
    /// 1-based index into the existing planets of the capital system.
    Existing(usize),
    /// 1-based index into `GameStartActions::spawned_planets`.
    Spawned(usize),
}

/// Attribute overrides for a planet (or a freshly spawned planet).
/// Any field set to `Some` overrides the planet's default attribute.
#[derive(Default, Clone, Debug, PartialEq, bevy::reflect::Reflect)]
pub struct PlanetAttributesSpec {
    pub habitability: Option<f64>,
    pub mineral_richness: Option<f64>,
    pub energy_potential: Option<f64>,
    pub research_potential: Option<f64>,
    pub max_building_slots: Option<u8>,
}

/// A planet that should be spawned by the engine when applying actions.
#[derive(Clone, Debug)]
pub struct SpawnedPlanetSpec {
    pub name: String,
    pub planet_type: String,
    pub attributes: PlanetAttributesSpec,
}

/// Attribute overrides for the capital StarSystem itself.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct SystemAttributesSpec {
    pub name: Option<String>,
    pub star_type: Option<String>,
    pub surveyed: Option<bool>,
}

/// Accumulated actions recorded by Lua `on_game_start` callbacks.
/// The Lua side only RECORDS intent; Rust applies these to the ECS afterward.
#[derive(Default, Debug)]
pub struct GameStartActions {
    /// Planet-level buildings to add: (planet_ref, building_id)
    pub planet_buildings: Vec<(PlanetRef, String)>,
    /// System-level buildings to add to the capital system.
    pub system_buildings: Vec<String>,
    /// Ships to spawn at the capital: (design_id, name)
    pub ships: Vec<(String, String)>,
    /// If set, colonize this planet for the faction.
    pub colonize_planet: Option<PlanetRef>,
    /// Whether to mark the system as the capital.
    pub mark_capital: bool,
    /// Whether to mark the system as surveyed.
    pub mark_surveyed: bool,
    /// If true, despawn all existing planets of the capital before spawning new ones.
    pub clear_planets: bool,
    /// Planets to spawn (in addition to / replacement for existing ones).
    pub spawned_planets: Vec<SpawnedPlanetSpec>,
    /// Per-planet attribute overrides applied after planets exist.
    pub planet_attribute_overrides: Vec<(PlanetRef, PlanetAttributesSpec)>,
    /// System-level attribute overrides (name, star_type, surveyed).
    pub system_attributes: Option<SystemAttributesSpec>,
    /// Whether to spawn a Core ship (infrastructure_core_v1) at the capital system.
    pub spawn_core: bool,
}

/// Parse a Lua table into a `PlanetAttributesSpec`.
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

/// Parse a Lua table into a `SystemAttributesSpec`.
fn parse_system_attrs(table: &mlua::Table) -> Result<SystemAttributesSpec, mlua::Error> {
    let mut spec = SystemAttributesSpec::default();
    if let Ok(v) = table.get::<String>("name") {
        spec.name = Some(v);
    }
    if let Ok(v) = table.get::<mlua::Value>("star_type") {
        if !matches!(v, mlua::Value::Nil) {
            spec.star_type = Some(extract_id_from_lua_value(&v)?);
        }
    }
    if let Ok(v) = table.get::<bool>("surveyed") {
        spec.surveyed = Some(v);
    }
    Ok(spec)
}

/// A handle on a planet (existing or spawned). Records intent into actions.
#[derive(Clone)]
pub struct PlanetHandle {
    pub planet_ref: PlanetRef,
    pub actions: Arc<Mutex<GameStartActions>>,
}

impl mlua::UserData for PlanetHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        // planet:colonize(faction) — records intent to colonize this planet for `faction`.
        methods.add_method("colonize", |_, this, _faction: mlua::Value| {
            let mut actions = this.actions.lock().unwrap();
            actions.colonize_planet = Some(this.planet_ref);
            Ok(())
        });

        // planet:add_building(id) — records a planet-level building to add.
        methods.add_method("add_building", |_, this, value: mlua::Value| {
            let id = extract_id_from_lua_value(&value)?;
            let mut actions = this.actions.lock().unwrap();
            actions.planet_buildings.push((this.planet_ref, id));
            Ok(())
        });

        // planet:set_attributes({ habitability=..., mineral_richness=..., ... })
        methods.add_method("set_attributes", |_, this, table: mlua::Table| {
            let spec = parse_planet_attrs(&table)?;
            let mut actions = this.actions.lock().unwrap();
            actions
                .planet_attribute_overrides
                .push((this.planet_ref, spec));
            Ok(())
        });

        // planet:index() — returns the 1-based planet index (existing planets only;
        // for spawned planets returns the spawn slot index).
        methods.add_method("index", |_, this, ()| {
            let idx = match this.planet_ref {
                PlanetRef::Existing(i) => i,
                PlanetRef::Spawned(i) => i,
            };
            Ok(idx)
        });

        // planet:is_spawned() — returns true if this planet was created via spawn_planet.
        methods.add_method("is_spawned", |_, this, ()| {
            Ok(matches!(this.planet_ref, PlanetRef::Spawned(_)))
        });
    }
}

/// A handle on the capital star system. Records intent into actions.
#[derive(Clone)]
pub struct SystemHandle {
    pub actions: Arc<Mutex<GameStartActions>>,
}

impl mlua::UserData for SystemHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        // system:get_planet(idx) — returns a PlanetHandle for the (1-based) planet index
        // of an existing (galaxy-generated) planet.
        methods.add_method("get_planet", |_, this, idx: usize| {
            Ok(PlanetHandle {
                planet_ref: PlanetRef::Existing(idx),
                actions: this.actions.clone(),
            })
        });

        // system:add_building(id) — records a system-level building to add.
        methods.add_method("add_building", |_, this, value: mlua::Value| {
            let id = extract_id_from_lua_value(&value)?;
            let mut actions = this.actions.lock().unwrap();
            actions.system_buildings.push(id);
            Ok(())
        });

        // system:spawn_ship(design_id, name) — records a ship to spawn at this system.
        methods.add_method(
            "spawn_ship",
            |_, this, (design, name): (mlua::Value, String)| {
                let design_id = extract_id_from_lua_value(&design)?;
                let mut actions = this.actions.lock().unwrap();
                actions.ships.push((design_id, name));
                Ok(())
            },
        );

        // system:set_capital(bool) — mark the system as the capital.
        methods.add_method("set_capital", |_, this, value: bool| {
            let mut actions = this.actions.lock().unwrap();
            actions.mark_capital = value;
            Ok(())
        });

        // system:set_surveyed(bool) — mark the system as surveyed.
        methods.add_method("set_surveyed", |_, this, value: bool| {
            let mut actions = this.actions.lock().unwrap();
            actions.mark_surveyed = value;
            Ok(())
        });

        // system:clear_planets() — despawn all existing planets of the capital
        // before spawning new ones. Useful when the faction wants full control
        // over the planet layout (e.g., to guarantee a habitable capital planet).
        methods.add_method("clear_planets", |_, this, ()| {
            let mut actions = this.actions.lock().unwrap();
            actions.clear_planets = true;
            Ok(())
        });

        // system:spawn_planet(name, type, attrs) — record a new planet to spawn,
        // returning a PlanetHandle that subsequent calls can reference.
        methods.add_method(
            "spawn_planet",
            |_, this, (name, planet_type, attrs): (String, mlua::Value, Option<mlua::Table>)| {
                let type_id = extract_id_from_lua_value(&planet_type)?;
                let attributes = match attrs {
                    Some(t) => parse_planet_attrs(&t)?,
                    None => PlanetAttributesSpec::default(),
                };
                let spawn_idx = {
                    let mut actions = this.actions.lock().unwrap();
                    actions.spawned_planets.push(SpawnedPlanetSpec {
                        name,
                        planet_type: type_id,
                        attributes,
                    });
                    actions.spawned_planets.len()
                };
                Ok(PlanetHandle {
                    planet_ref: PlanetRef::Spawned(spawn_idx),
                    actions: this.actions.clone(),
                })
            },
        );

        // system:spawn_core() — records intent to spawn a Core ship (infrastructure_core_v1)
        // at the capital system on game start.
        methods.add_method("spawn_core", |_, this, ()| {
            let mut actions = this.actions.lock().unwrap();
            actions.spawn_core = true;
            Ok(())
        });

        // system:set_attributes({ name=..., star_type=..., surveyed=... })
        methods.add_method("set_attributes", |_, this, table: mlua::Table| {
            let spec = parse_system_attrs(&table)?;
            let mut actions = this.actions.lock().unwrap();
            // Merge with any prior set_attributes call rather than replacing.
            let merged = match actions.system_attributes.take() {
                Some(prev) => SystemAttributesSpec {
                    name: spec.name.or(prev.name),
                    star_type: spec.star_type.or(prev.star_type),
                    surveyed: spec.surveyed.or(prev.surveyed),
                },
                None => spec,
            };
            actions.system_attributes = Some(merged);
            Ok(())
        });
    }
}

/// The context passed to a faction's `on_game_start` callback.
/// Provides `ctx.system` (SystemHandle), `ctx.faction` (the faction id string),
/// and `ctx.faction_id` (alias).
#[derive(Clone)]
pub struct GameStartCtx {
    pub faction_id: String,
    pub actions: Arc<Mutex<GameStartActions>>,
}

impl GameStartCtx {
    pub fn new(faction_id: String) -> Self {
        Self {
            faction_id,
            actions: Arc::new(Mutex::new(GameStartActions::default())),
        }
    }

    /// Take the accumulated actions, leaving the ctx empty.
    pub fn take_actions(&self) -> GameStartActions {
        std::mem::take(&mut *self.actions.lock().unwrap())
    }
}

impl mlua::UserData for GameStartCtx {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("system", |_, this| {
            Ok(SystemHandle {
                actions: this.actions.clone(),
            })
        });
        fields.add_field_method_get("faction", |_, this| Ok(this.faction_id.clone()));
        fields.add_field_method_get("faction_id", |_, this| Ok(this.faction_id.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_ctx_records_planet_colonize_and_buildings() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GameStartCtx::new("humanity_empire".into());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            local p = ctx.system:get_planet(1)
            p:colonize(ctx.faction)
            p:add_building("mine")
            p:add_building("power_plant")
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert_eq!(actions.colonize_planet, Some(PlanetRef::Existing(1)));
        assert_eq!(actions.planet_buildings.len(), 2);
        assert_eq!(
            actions.planet_buildings[0],
            (PlanetRef::Existing(1), "mine".to_string())
        );
        assert_eq!(
            actions.planet_buildings[1],
            (PlanetRef::Existing(1), "power_plant".to_string())
        );
    }

    #[test]
    fn test_ctx_records_system_buildings_and_ships() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GameStartCtx::new("humanity_empire".into());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            ctx.system:add_building("shipyard")
            ctx.system:spawn_ship("explorer_mk1", "Explorer I")
            ctx.system:spawn_ship("colony_ship_mk1", "Colony Ship I")
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert_eq!(actions.system_buildings, vec!["shipyard".to_string()]);
        assert_eq!(actions.ships.len(), 2);
        assert_eq!(
            actions.ships[0],
            ("explorer_mk1".into(), "Explorer I".into())
        );
        assert_eq!(
            actions.ships[1],
            ("colony_ship_mk1".into(), "Colony Ship I".into())
        );
    }

    #[test]
    fn test_ctx_set_capital_and_surveyed() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GameStartCtx::new("humanity_empire".into());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            ctx.system:set_capital(true)
            ctx.system:set_surveyed(true)
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert!(actions.mark_capital);
        assert!(actions.mark_surveyed);
    }

    #[test]
    fn test_ctx_faction_field_exposed() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GameStartCtx::new("test_faction".into());
        lua.globals().set("ctx", ctx).unwrap();

        let id: String = lua.load(r#"return ctx.faction"#).eval().unwrap();
        assert_eq!(id, "test_faction");
        let id2: String = lua.load(r#"return ctx.faction_id"#).eval().unwrap();
        assert_eq!(id2, "test_faction");
    }

    #[test]
    fn test_planet_add_building_accepts_reference() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GameStartCtx::new("humanity_empire".into());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        // Use a reference table (as define_xxx returns)
        lua.load(
            r#"
            local ref = { _def_type = "building", id = "farm" }
            ctx.system:get_planet(2):add_building(ref)
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert_eq!(
            actions.planet_buildings,
            vec![(PlanetRef::Existing(2), "farm".to_string())]
        );
    }

    #[test]
    fn test_take_actions_clears_state() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GameStartCtx::new("humanity_empire".into());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(r#"ctx.system:add_building("shipyard")"#)
            .exec()
            .unwrap();
        let first = ctx.take_actions();
        assert_eq!(first.system_buildings.len(), 1);

        let second = ctx.take_actions();
        assert!(second.system_buildings.is_empty());
    }

    #[test]
    fn test_clear_planets() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GameStartCtx::new("humanity_empire".into());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(r#"ctx.system:clear_planets()"#).exec().unwrap();

        let actions = ctx.take_actions();
        assert!(actions.clear_planets);
        assert!(actions.spawned_planets.is_empty());
    }

    #[test]
    fn test_spawn_planet_returns_handle_with_methods() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GameStartCtx::new("humanity_empire".into());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            ctx.system:clear_planets()
            local earth = ctx.system:spawn_planet("Earth", "terrestrial", {
                habitability = 1.0,
                mineral_richness = 0.7,
                energy_potential = 0.5,
                research_potential = 0.5,
                max_building_slots = 6,
            })
            earth:colonize(ctx.faction)
            earth:add_building("mine")
            local mars = ctx.system:spawn_planet("Mars", "terrestrial", { habitability = 0.4 })
            mars:add_building("farm")
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert!(actions.clear_planets);
        assert_eq!(actions.spawned_planets.len(), 2);
        assert_eq!(actions.spawned_planets[0].name, "Earth");
        assert_eq!(actions.spawned_planets[0].planet_type, "terrestrial");
        assert_eq!(
            actions.spawned_planets[0].attributes.habitability,
            Some(1.0)
        );
        assert_eq!(
            actions.spawned_planets[0].attributes.mineral_richness,
            Some(0.7)
        );
        assert_eq!(
            actions.spawned_planets[0].attributes.max_building_slots,
            Some(6)
        );
        assert_eq!(actions.spawned_planets[1].name, "Mars");
        assert_eq!(
            actions.spawned_planets[1].attributes.habitability,
            Some(0.4)
        );
        assert_eq!(actions.colonize_planet, Some(PlanetRef::Spawned(1)));
        assert_eq!(actions.planet_buildings.len(), 2);
        assert_eq!(
            actions.planet_buildings[0],
            (PlanetRef::Spawned(1), "mine".to_string())
        );
        assert_eq!(
            actions.planet_buildings[1],
            (PlanetRef::Spawned(2), "farm".to_string())
        );
    }

    #[test]
    fn test_planet_set_attributes() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GameStartCtx::new("humanity_empire".into());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            local p = ctx.system:get_planet(1)
            p:set_attributes({
                habitability = 0.9,
                mineral_richness = 0.6,
                research_potential = 0.8,
                max_building_slots = 7,
            })
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        assert_eq!(actions.planet_attribute_overrides.len(), 1);
        let (pref, spec) = &actions.planet_attribute_overrides[0];
        assert_eq!(*pref, PlanetRef::Existing(1));
        assert_eq!(spec.habitability, Some(0.9));
        assert_eq!(spec.mineral_richness, Some(0.6));
        assert_eq!(spec.research_potential, Some(0.8));
        assert_eq!(spec.max_building_slots, Some(7));
        assert_eq!(spec.energy_potential, None);
    }

    #[test]
    fn test_system_set_attributes() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GameStartCtx::new("humanity_empire".into());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            ctx.system:set_attributes({
                name = "Sol",
                star_type = "yellow_dwarf",
                surveyed = true,
            })
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        let spec = actions.system_attributes.expect("system_attributes set");
        assert_eq!(spec.name, Some("Sol".to_string()));
        assert_eq!(spec.star_type, Some("yellow_dwarf".to_string()));
        assert_eq!(spec.surveyed, Some(true));
    }

    #[test]
    fn test_system_set_attributes_accepts_star_type_reference() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GameStartCtx::new("humanity_empire".into());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        lua.load(
            r#"
            local star = { _def_type = "star_type", id = "yellow_dwarf" }
            ctx.system:set_attributes({ star_type = star })
            "#,
        )
        .exec()
        .unwrap();

        let actions = ctx.take_actions();
        let spec = actions.system_attributes.unwrap();
        assert_eq!(spec.star_type, Some("yellow_dwarf".to_string()));
    }

    #[test]
    fn test_planet_handle_index_and_is_spawned() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GameStartCtx::new("humanity_empire".into());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        let result: mlua::Table = lua
            .load(
                r#"
                local existing = ctx.system:get_planet(2)
                local spawned = ctx.system:spawn_planet("Foo", "terrestrial", nil)
                return {
                    e_idx = existing:index(),
                    e_spawned = existing:is_spawned(),
                    s_idx = spawned:index(),
                    s_spawned = spawned:is_spawned(),
                }
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(result.get::<i64>("e_idx").unwrap(), 2);
        assert_eq!(result.get::<bool>("e_spawned").unwrap(), false);
        assert_eq!(result.get::<i64>("s_idx").unwrap(), 1);
        assert_eq!(result.get::<bool>("s_spawned").unwrap(), true);
    }

    #[test]
    fn test_spawn_core_records_flag() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let ctx = GameStartCtx::new("humanity_empire".into());
        lua.globals().set("ctx", ctx.clone()).unwrap();

        // Default is false
        let actions = ctx.take_actions();
        assert!(!actions.spawn_core);

        // After calling spawn_core(), flag is true
        lua.load(r#"ctx.system:spawn_core()"#).exec().unwrap();
        let actions = ctx.take_actions();
        assert!(actions.spawn_core);
    }
}
