use std::sync::{Arc, Mutex};

use super::extract_id_from_lua_value;

/// Accumulated actions recorded by Lua `on_game_start` callbacks.
/// The Lua side only RECORDS intent; Rust applies these to the ECS afterward.
#[derive(Default, Debug)]
pub struct GameStartActions {
    /// Planet-level buildings to add: (planet_idx_1based, building_id)
    pub planet_buildings: Vec<(usize, String)>,
    /// System-level buildings to add to the capital system.
    pub system_buildings: Vec<String>,
    /// Ships to spawn at the capital: (design_id, name)
    pub ships: Vec<(String, String)>,
    /// If set, colonize this planet (1-based index) for the faction.
    pub colonize_planet: Option<usize>,
    /// Whether to mark the system as the capital.
    pub mark_capital: bool,
    /// Whether to mark the system as surveyed.
    pub mark_surveyed: bool,
}

/// A handle on a planet (by 1-based index). Records intent into actions.
#[derive(Clone)]
pub struct PlanetHandle {
    pub planet_idx: usize,
    pub actions: Arc<Mutex<GameStartActions>>,
}

impl mlua::UserData for PlanetHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        // planet:colonize(faction) — records intent to colonize this planet for `faction`.
        // The faction argument may be a string id, a table reference, or a Faction handle.
        methods.add_method("colonize", |_, this, _faction: mlua::Value| {
            let mut actions = this.actions.lock().unwrap();
            actions.colonize_planet = Some(this.planet_idx);
            Ok(())
        });

        // planet:add_building(id) — records a planet-level building to add.
        methods.add_method("add_building", |_, this, value: mlua::Value| {
            let id = extract_id_from_lua_value(&value)?;
            let mut actions = this.actions.lock().unwrap();
            actions.planet_buildings.push((this.planet_idx, id));
            Ok(())
        });

        // planet:index() — returns the 1-based planet index (useful for debugging).
        methods.add_method("index", |_, this, ()| Ok(this.planet_idx));
    }
}

/// A handle on the capital star system. Records intent into actions.
#[derive(Clone)]
pub struct SystemHandle {
    pub actions: Arc<Mutex<GameStartActions>>,
}

impl mlua::UserData for SystemHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        // system:get_planet(idx) — returns a PlanetHandle for the (1-based) planet index.
        methods.add_method("get_planet", |_, this, idx: usize| {
            Ok(PlanetHandle {
                planet_idx: idx,
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
        assert_eq!(actions.colonize_planet, Some(1));
        assert_eq!(actions.planet_buildings.len(), 2);
        assert_eq!(actions.planet_buildings[0], (1, "mine".to_string()));
        assert_eq!(actions.planet_buildings[1], (1, "power_plant".to_string()));
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
        assert_eq!(actions.ships[0], ("explorer_mk1".into(), "Explorer I".into()));
        assert_eq!(actions.ships[1], ("colony_ship_mk1".into(), "Colony Ship I".into()));
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
        assert_eq!(actions.planet_buildings, vec![(2, "farm".to_string())]);
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
}
