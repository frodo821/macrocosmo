//! #332: pure scoped-closure gamestate accessor (Option B).
//!
//! Replaces the snapshot-per-event `build_gamestate_table` path
//! (`gamestate_view.rs`) with a `Lua::scope` + `create_function` /
//! `create_function_mut` bundle that exposes `ctx.gamestate` as a method
//! surface sharing a `RefCell<&mut World>` across read and write
//! closures. Scope closures are automatically invalidated when the scope
//! exits, so mlua cleans up without a manual `gc_collect` (#320).
//!
//! # Invariants
//!
//! * **write helpers (`apply::*`) are Lua-unaware**: they take `&mut World`
//!   plus owned primitives, never `&Lua`, `mlua::Function`,
//!   `mlua::RegistryKey`, or any `mlua::Value`. See
//!   `memory/feedback_rust_no_lua_callback.md`.
//! * **read helpers (`views::build_*_view`)** may only call
//!   `lua.create_table()` / `table.set(...)` and read from `&mut World`
//!   (Bevy 0.18's query APIs require `&mut`); they must not invoke Lua
//!   code (`Function::call`, `lua.load().exec()`). The exclusive borrow
//!   is fine because Lua callbacks never run concurrently with
//!   themselves — scope closures serialise naturally.
//! * **reentrancy guard**: `try_borrow*` failures map to
//!   `mlua::Error::RuntimeError` via `map_reentrancy_err`; no panics ever.
//! * **`fire_event` stays queue-only**: there is no sync dispatch hook in
//!   this module; event fan-out still goes through `_pending_script_events`.
//!
//! # Modes
//!
//! `GamestateMode::ReadOnly` skips all write closures (`fire_condition`
//! contexts must be side-effect free). `GamestateMode::ReadWrite` exposes
//! setters (`push_empire_modifier` etc.) for event callback contexts.

use bevy::prelude::*;
use mlua::{Lua, Scope, Table};
use std::cell::RefCell;

/// Whether the built gamestate exposes write closures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GamestateMode {
    /// Read-only: used for `fire_condition` / any context where mutations
    /// would be unsafe (side-effect-free evaluation).
    ReadOnly,
    /// Read + write: used for event callbacks, lifecycle hooks, etc.
    ReadWrite,
}

/// Convert a `RefCell` borrow failure into a diagnostic `mlua::Error::RuntimeError`.
///
/// The message intentionally names the two usual culprits — read-from-write
/// and write-from-write — and reminds the modder that `fire_event` must go
/// through the queue.
pub(crate) fn map_reentrancy_err<E: std::fmt::Display>(e: E) -> mlua::Error {
    mlua::Error::RuntimeError(format!(
        "gamestate reentrancy guard: {e}; likely cause: a read helper \
         was invoked from inside a write callback, or a write helper ran \
         while another mutation was still borrowed. `fire_event` must \
         always go through the queue, never sync-dispatch."
    ))
}

/// Dispatch `callback` with `payload.gamestate` populated with live
/// read/write closures sharing `world` via a `RefCell`.
///
/// The closures are torn down when this function returns. Any Lua-side
/// reference captured post-scope will error cleanly on invocation (see
/// `tests/spike_mlua_scope.rs::spike_capture_resistance_closure_invalid_after_scope`).
///
/// * `mode` — `ReadOnly` omits write closures.
/// * `callback` — invoked once inside the scope, with the enriched payload.
pub fn dispatch_with_gamestate<F>(
    lua: &Lua,
    world: &mut World,
    payload: &Table,
    mode: GamestateMode,
    callback: F,
) -> mlua::Result<()>
where
    F: FnOnce(&Lua, &Table) -> mlua::Result<()>,
{
    let world_cell: RefCell<&mut World> = RefCell::new(world);
    lua.scope(|s| {
        let gs = build_scoped_gamestate(lua, s, &world_cell, mode)?;
        payload.set("gamestate", gs)?;
        callback(lua, payload)
    })
}

/// Assemble the `gamestate` table on the Lua scope. Exposes method-style
/// read closures; when `mode == ReadWrite`, also exposes setter closures.
fn build_scoped_gamestate<'scope, 'env>(
    lua: &Lua,
    s: &'scope Scope<'scope, 'env>,
    world_cell: &'env RefCell<&'env mut World>,
    mode: GamestateMode,
) -> mlua::Result<Table> {
    let gs = lua.create_table()?;

    // --- clock (plain-old-data snapshot — cheap, no RefCell borrow) ---
    {
        let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
        gs.set("clock", views::build_clock_table(lua, &mut **borrow)?)?;
    }

    // ------------------------------------------------------------------
    // READ closures
    // ------------------------------------------------------------------
    // Each read closure:
    // * calls `world_cell.try_borrow_mut()` (clean error on conflict)
    // * builds a plain Lua table via `views::build_*_view`
    // * releases the borrow before returning to Lua

    // :empire(id) -> Table
    let read_empire = s.create_function_mut(|lua, (_this, id): (Table, u64)| {
        let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
        let entity = Entity::from_bits(id);
        views::build_empire_view(lua, &mut **borrow, entity)
    })?;
    gs.set("empire", read_empire)?;

    // :player_empire() -> Table
    let read_player = s.create_function_mut(|lua, _this: Table| {
        let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
        match views::find_player_empire(&mut **borrow) {
            Some(e) => views::build_empire_view(lua, &mut **borrow, e),
            None => Ok(lua.create_table()?),
        }
    })?;
    gs.set("player_empire", read_player)?;

    // :system(id) -> Table
    let read_system = s.create_function_mut(|lua, (_this, id): (Table, u64)| {
        let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
        let entity = Entity::from_bits(id);
        views::build_system_view(lua, &mut **borrow, entity)
    })?;
    gs.set("system", read_system)?;

    // :planet(id) -> Table
    let read_planet = s.create_function_mut(|lua, (_this, id): (Table, u64)| {
        let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
        let entity = Entity::from_bits(id);
        views::build_planet_view(lua, &mut **borrow, entity)
    })?;
    gs.set("planet", read_planet)?;

    // :colony(id) -> Table
    let read_colony = s.create_function_mut(|lua, (_this, id): (Table, u64)| {
        let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
        let entity = Entity::from_bits(id);
        views::build_colony_view(lua, &mut **borrow, entity)
    })?;
    gs.set("colony", read_colony)?;

    // :ship(id) -> Table
    let read_ship = s.create_function_mut(|lua, (_this, id): (Table, u64)| {
        let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
        let entity = Entity::from_bits(id);
        views::build_ship_view(lua, &mut **borrow, entity)
    })?;
    gs.set("ship", read_ship)?;

    // :fleet(id) -> Table
    let read_fleet = s.create_function_mut(|lua, (_this, id): (Table, u64)| {
        let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
        let entity = Entity::from_bits(id);
        views::build_fleet_view(lua, &mut **borrow, entity)
    })?;
    gs.set("fleet", read_fleet)?;

    // :list_empires() -> {u64}
    let list_empires = s.create_function_mut(|lua, _this: Table| {
        let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
        views::list_empire_ids(lua, &mut **borrow)
    })?;
    gs.set("list_empires", list_empires)?;

    // :list_systems() -> {u64}
    let list_systems = s.create_function_mut(|lua, _this: Table| {
        let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
        views::list_system_ids(lua, &mut **borrow)
    })?;
    gs.set("list_systems", list_systems)?;

    // :list_planets([system_id]) -> {u64}
    let list_planets = s.create_function_mut(|lua, (_this, sid): (Table, Option<u64>)| {
        let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
        views::list_planet_ids(lua, &mut **borrow, sid.map(Entity::from_bits))
    })?;
    gs.set("list_planets", list_planets)?;

    // :list_colonies([system_id_or_empire_id]) -> {u64}
    let list_colonies = s.create_function_mut(|lua, (_this, filter): (Table, Option<u64>)| {
        let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
        views::list_colony_ids(lua, &mut **borrow, filter.map(Entity::from_bits))
    })?;
    gs.set("list_colonies", list_colonies)?;

    // :list_fleets([empire_id]) -> {u64}
    let list_fleets = s.create_function_mut(|lua, (_this, eid): (Table, Option<u64>)| {
        let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
        views::list_fleet_ids(lua, &mut **borrow, eid.map(Entity::from_bits))
    })?;
    gs.set("list_fleets", list_fleets)?;

    // :list_ships([fleet_id]) -> {u64}
    let list_ships = s.create_function_mut(|lua, (_this, fid): (Table, Option<u64>)| {
        let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
        views::list_ship_ids(lua, &mut **borrow, fid.map(Entity::from_bits))
    })?;
    gs.set("list_ships", list_ships)?;

    // ------------------------------------------------------------------
    // WRITE closures (ReadWrite mode only)
    // ------------------------------------------------------------------
    if matches!(mode, GamestateMode::ReadWrite) {
        // :push_empire_modifier(empire_id, target, opts) -> nil
        let push_empire = s.create_function_mut(
            move |_lua, (_this, id, target, opts): (Table, u64, String, Table)| {
                let parsed = apply::parse_modifier_opts(&opts)?;
                let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
                apply::push_empire_modifier(&mut **borrow, Entity::from_bits(id), &target, parsed)
            },
        )?;
        gs.set("push_empire_modifier", push_empire)?;

        // :push_system_modifier(system_id, target, opts) -> nil
        let push_system = s.create_function_mut(
            move |_lua, (_this, id, target, opts): (Table, u64, String, Table)| {
                let parsed = apply::parse_modifier_opts(&opts)?;
                let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
                apply::push_system_modifier(&mut **borrow, Entity::from_bits(id), &target, parsed)
            },
        )?;
        gs.set("push_system_modifier", push_system)?;

        // :push_colony_modifier(colony_id, target, opts) -> nil
        let push_colony = s.create_function_mut(
            move |_lua, (_this, id, target, opts): (Table, u64, String, Table)| {
                let parsed = apply::parse_modifier_opts(&opts)?;
                let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
                apply::push_colony_modifier(&mut **borrow, Entity::from_bits(id), &target, parsed)
            },
        )?;
        gs.set("push_colony_modifier", push_colony)?;

        // :push_ship_modifier(ship_id, target, opts) -> nil
        let push_ship = s.create_function_mut(
            move |_lua, (_this, id, target, opts): (Table, u64, String, Table)| {
                let parsed = apply::parse_modifier_opts(&opts)?;
                let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
                apply::push_ship_modifier(&mut **borrow, Entity::from_bits(id), &target, parsed)
            },
        )?;
        gs.set("push_ship_modifier", push_ship)?;

        // :push_fleet_modifier(fleet_id, target, opts) -> nil
        // Until #287 γ-2 lands (FleetState component), fleet modifiers
        // are applied to the flagship ship as a pragmatic proxy so the
        // API shape doesn't flip when γ-2 merges.
        let push_fleet = s.create_function_mut(
            move |_lua, (_this, id, target, opts): (Table, u64, String, Table)| {
                let parsed = apply::parse_modifier_opts(&opts)?;
                let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
                apply::push_fleet_modifier(&mut **borrow, Entity::from_bits(id), &target, parsed)
            },
        )?;
        gs.set("push_fleet_modifier", push_fleet)?;

        // :set_flag(scope_kind, scope_id, name, value?) -> nil
        let set_flag = s.create_function_mut(
            move |_lua,
                  (_this, scope_kind, id, name, value): (
                Table,
                String,
                u64,
                String,
                Option<bool>,
            )| {
                let val = value.unwrap_or(true);
                let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
                apply::set_flag(
                    &mut **borrow,
                    &scope_kind,
                    Entity::from_bits(id),
                    &name,
                    val,
                )
            },
        )?;
        gs.set("set_flag", set_flag)?;

        // :clear_flag(scope_kind, scope_id, name) -> nil
        let clear_flag = s.create_function_mut(
            move |_lua, (_this, scope_kind, id, name): (Table, String, u64, String)| {
                let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
                apply::clear_flag(&mut **borrow, &scope_kind, Entity::from_bits(id), &name)
            },
        )?;
        gs.set("clear_flag", clear_flag)?;
    }

    Ok(gs)
}

// ======================================================================
// views: Rust -> Lua Table builders (read path, never calls Lua code).
// ======================================================================
pub(crate) mod views {
    use super::*;
    use crate::colony::{Colony, ResourceStockpile};
    use crate::components::Position;
    use crate::condition::ScopedFlags;
    use crate::galaxy::{Planet, Sovereignty, StarSystem, SystemModifiers};
    use crate::player::{Empire, PlayerEmpire};
    use crate::ship::fleet::{Fleet, FleetMembers};
    use crate::ship::{Owner, Ship};
    use crate::technology::{GameFlags, TechTree};
    use crate::time_system::GameClock;
    use std::collections::HashSet;

    pub fn build_clock_table(lua: &Lua, world: &mut World) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        if let Some(clock) = world.get_resource::<GameClock>() {
            t.set("now", clock.elapsed)?;
            t.set("year", clock.year())?;
            t.set("month", clock.month())?;
            t.set("hexady_of_month", clock.hexadies())?;
        } else {
            t.set("now", 0i64)?;
            t.set("year", 0i64)?;
            t.set("month", 1i64)?;
            t.set("hexady_of_month", 1i64)?;
        }
        Ok(t)
    }

    pub fn find_player_empire(world: &mut World) -> Option<Entity> {
        let mut q = world.query_filtered::<Entity, (With<Empire>, With<PlayerEmpire>)>();
        q.iter(world).next()
    }

    pub fn list_empire_ids(lua: &Lua, world: &mut World) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        let mut q = world.query_filtered::<Entity, With<Empire>>();
        for e in q.iter(world) {
            t.push(e.to_bits())?;
        }
        Ok(t)
    }

    pub fn list_system_ids(lua: &Lua, world: &mut World) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        let mut q = world.query_filtered::<Entity, With<StarSystem>>();
        for e in q.iter(world) {
            t.push(e.to_bits())?;
        }
        Ok(t)
    }

    pub fn list_planet_ids(
        lua: &Lua,
        world: &mut World,
        filter_system: Option<Entity>,
    ) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        let mut q = world.query::<(Entity, &Planet)>();
        for (e, p) in q.iter(world) {
            if let Some(sys) = filter_system {
                if p.system != sys {
                    continue;
                }
            }
            t.push(e.to_bits())?;
        }
        Ok(t)
    }

    pub fn list_colony_ids(
        lua: &Lua,
        world: &mut World,
        filter: Option<Entity>,
    ) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        // Disambiguate filter: is it a system_id or empire_id?
        let (filter_is_system, filter_is_empire) = match filter {
            Some(e) => {
                let er = world.get_entity(e).ok();
                (
                    er.as_ref()
                        .map(|r| r.contains::<StarSystem>())
                        .unwrap_or(false),
                    er.as_ref().map(|r| r.contains::<Empire>()).unwrap_or(false),
                )
            }
            None => (false, false),
        };

        // Collect colony rows first to release the query borrow.
        let rows: Vec<(Entity, Entity)> = {
            let mut q = world.query::<(Entity, &Colony)>();
            q.iter(world).map(|(e, c)| (e, c.planet)).collect()
        };
        for (colony_entity, planet_entity) in rows {
            if let Some(f) = filter {
                let sys_entity = world.get::<Planet>(planet_entity).map(|p| p.system);
                if filter_is_system {
                    if sys_entity != Some(f) {
                        continue;
                    }
                } else if filter_is_empire {
                    let owner = sys_entity
                        .and_then(|s| world.get::<Sovereignty>(s).and_then(|sov| sov.owner));
                    match owner {
                        Some(Owner::Empire(e)) if e == f => {}
                        _ => continue,
                    }
                } else {
                    continue;
                }
            }
            t.push(colony_entity.to_bits())?;
        }
        Ok(t)
    }

    pub fn list_fleet_ids(
        lua: &Lua,
        world: &mut World,
        filter_empire: Option<Entity>,
    ) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        let rows: Vec<(Entity, Option<Entity>, Vec<Entity>)> = {
            let mut q = world.query::<(Entity, &Fleet, &FleetMembers)>();
            q.iter(world)
                .map(|(e, f, m)| (e, f.flagship, m.0.clone()))
                .collect()
        };
        for (entity, flagship, members) in rows {
            if let Some(emp) = filter_empire {
                let proxy = flagship.or_else(|| members.first().copied());
                let owner = proxy.and_then(|s| world.get::<Ship>(s)).map(|sh| sh.owner);
                match owner {
                    Some(Owner::Empire(e)) if e == emp => {}
                    _ => continue,
                }
            }
            t.push(entity.to_bits())?;
        }
        Ok(t)
    }

    pub fn list_ship_ids(
        lua: &Lua,
        world: &mut World,
        filter_fleet: Option<Entity>,
    ) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        let mut q = world.query::<(Entity, &Ship)>();
        for (e, ship) in q.iter(world) {
            if let Some(f) = filter_fleet {
                if ship.fleet != Some(f) {
                    continue;
                }
            }
            t.push(e.to_bits())?;
        }
        Ok(t)
    }

    pub fn build_empire_view(lua: &Lua, world: &mut World, entity: Entity) -> mlua::Result<Table> {
        let etbl = lua.create_table()?;
        // Pull the empire fields first so we can release the entity ref
        // before taking the resource_stockpile query borrow below.
        let (name, is_player) = {
            let Ok(eref) = world.get_entity(entity) else {
                return Ok(etbl);
            };
            let Some(empire) = eref.get::<Empire>() else {
                return Ok(etbl);
            };
            (empire.name.clone(), eref.contains::<PlayerEmpire>())
        };
        etbl.set("id", entity.to_bits())?;
        etbl.set("name", name.as_str())?;
        etbl.set("is_player", is_player)?;

        // resources: aggregate from all ResourceStockpile components.
        // Phase 1 single-empire simplification: player sees the sum,
        // others see 0.
        let rtbl = lua.create_table()?;
        if is_player {
            let (mut minerals, mut energy, mut research, mut food, mut authority) = (
                crate::amount::Amt::ZERO,
                crate::amount::Amt::ZERO,
                crate::amount::Amt::ZERO,
                crate::amount::Amt::ZERO,
                crate::amount::Amt::ZERO,
            );
            let mut q = world.query::<&ResourceStockpile>();
            for sp in q.iter(world) {
                minerals = minerals.add(sp.minerals);
                energy = energy.add(sp.energy);
                research = research.add(sp.research);
                food = food.add(sp.food);
                authority = authority.add(sp.authority);
            }
            rtbl.set("minerals", minerals.to_f64())?;
            rtbl.set("energy", energy.to_f64())?;
            rtbl.set("research", research.to_f64())?;
            rtbl.set("food", food.to_f64())?;
            rtbl.set("authority", authority.to_f64())?;
        } else {
            rtbl.set("minerals", 0.0_f64)?;
            rtbl.set("energy", 0.0_f64)?;
            rtbl.set("research", 0.0_f64)?;
            rtbl.set("food", 0.0_f64)?;
            rtbl.set("authority", 0.0_f64)?;
        }
        etbl.set("resources", rtbl)?;

        // techs / flags / capital / colony_ids need the empire entity ref
        // again — re-fetch after the query borrow was released.
        let techs_tbl = lua.create_table()?;
        let flags_tbl = lua.create_table()?;
        let mut flag_set: HashSet<String> = HashSet::new();
        {
            let Ok(eref) = world.get_entity(entity) else {
                return Ok(etbl);
            };
            if let Some(tree) = eref.get::<TechTree>() {
                for tid in tree.researched.iter() {
                    techs_tbl.set(tid.0.as_str(), true)?;
                }
            }
            if let Some(f) = eref.get::<GameFlags>() {
                flag_set.extend(f.flags.iter().cloned());
            }
            if let Some(f) = eref.get::<ScopedFlags>() {
                flag_set.extend(f.flags.iter().cloned());
            }
        }
        etbl.set("techs", techs_tbl.clone())?;
        etbl.set("tech", techs_tbl)?;
        for f in &flag_set {
            flags_tbl.set(f.as_str(), true)?;
        }
        etbl.set("flags", flags_tbl)?;

        // capital_system_id: first system with is_capital (Phase 1).
        let capital: Option<Entity> = {
            let mut q = world.query::<(Entity, &StarSystem)>();
            q.iter(world).find(|(_, s)| s.is_capital).map(|(e, _)| e)
        };
        if let Some(e) = capital {
            etbl.set("capital_system_id", e.to_bits())?;
        }

        // colony_ids: Phase 1 — player gets all colonies, others empty.
        let cids = lua.create_table()?;
        if is_player {
            let mut q = world.query_filtered::<Entity, With<Colony>>();
            for e in q.iter(world) {
                cids.push(e.to_bits())?;
            }
        }
        etbl.set("colony_ids", cids)?;

        Ok(etbl)
    }

    pub fn build_system_view(lua: &Lua, world: &mut World, entity: Entity) -> mlua::Result<Table> {
        let stbl = lua.create_table()?;
        let Ok(eref) = world.get_entity(entity) else {
            return Ok(stbl);
        };
        let Some(sys) = eref.get::<StarSystem>() else {
            return Ok(stbl);
        };
        stbl.set("id", entity.to_bits())?;
        stbl.set("entity", entity.to_bits())?;
        stbl.set("name", sys.name.as_str())?;
        stbl.set("surveyed", sys.surveyed)?;
        stbl.set("is_capital", sys.is_capital)?;
        stbl.set("star_type", sys.star_type.as_str())?;

        if let Some(pos) = eref.get::<Position>() {
            let ptbl = lua.create_table()?;
            ptbl.set("x", pos.x)?;
            ptbl.set("y", pos.y)?;
            ptbl.set("z", pos.z)?;
            stbl.set("position", ptbl)?;
        }
        if let Some(sp) = eref.get::<ResourceStockpile>() {
            let rtbl = lua.create_table()?;
            rtbl.set("minerals", sp.minerals.to_f64())?;
            rtbl.set("energy", sp.energy.to_f64())?;
            rtbl.set("research", sp.research.to_f64())?;
            rtbl.set("food", sp.food.to_f64())?;
            rtbl.set("authority", sp.authority.to_f64())?;
            stbl.set("resources", rtbl)?;
        }
        if let Some(sov) = eref.get::<Sovereignty>() {
            if let Some(Owner::Empire(e)) = sov.owner {
                stbl.set("owner_empire_id", e.to_bits())?;
            }
        }
        if let Some(mods) = eref.get::<SystemModifiers>() {
            let mtbl = lua.create_table()?;
            mtbl.set("ship_speed", mods.ship_speed.final_value().to_f64())?;
            mtbl.set("ship_attack", mods.ship_attack.final_value().to_f64())?;
            mtbl.set("ship_defense", mods.ship_defense.final_value().to_f64())?;
            stbl.set("modifiers", mtbl)?;
        }
        Ok(stbl)
    }

    pub fn build_planet_view(lua: &Lua, world: &mut World, entity: Entity) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        let Ok(eref) = world.get_entity(entity) else {
            return Ok(t);
        };
        let Some(planet) = eref.get::<Planet>() else {
            return Ok(t);
        };
        t.set("id", entity.to_bits())?;
        t.set("entity", entity.to_bits())?;
        t.set("name", planet.name.as_str())?;
        t.set("planet_type", planet.planet_type.as_str())?;
        t.set("biome", planet.planet_type.as_str())?;
        t.set("system_id", planet.system.to_bits())?;
        Ok(t)
    }

    pub fn build_colony_view(lua: &Lua, world: &mut World, entity: Entity) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        let Ok(eref) = world.get_entity(entity) else {
            return Ok(t);
        };
        let Some(colony) = eref.get::<Colony>() else {
            return Ok(t);
        };
        t.set("id", entity.to_bits())?;
        t.set("entity", entity.to_bits())?;
        t.set("population", colony.population)?;
        t.set("growth_rate", colony.growth_rate)?;
        t.set("planet_id", colony.planet.to_bits())?;

        let mut colony_system: Option<Entity> = None;
        if let Some(planet) = world.get::<Planet>(colony.planet) {
            colony_system = Some(planet.system);
            t.set("system_id", planet.system.to_bits())?;
            t.set("planet_name", planet.name.as_str())?;
        }
        if let Some(sys_entity) = colony_system {
            if let Some(sov) = world.get::<Sovereignty>(sys_entity) {
                if let Some(Owner::Empire(e)) = sov.owner {
                    t.set("owner_empire_id", e.to_bits())?;
                }
            }
        }
        if let Some(buildings) = eref.get::<crate::colony::Buildings>() {
            let slots_tbl = lua.create_table()?;
            let building_ids_tbl = lua.create_table()?;
            for (i, slot) in buildings.slots.iter().enumerate() {
                let idx = (i + 1) as i64;
                if let Some(bid) = slot {
                    let entry = lua.create_table()?;
                    entry.set("id", bid.0.as_str())?;
                    slots_tbl.set(idx, entry)?;
                    building_ids_tbl.set(idx, bid.0.as_str())?;
                } else {
                    slots_tbl.set(idx, mlua::Value::Nil)?;
                    building_ids_tbl.set(idx, mlua::Value::Nil)?;
                }
            }
            t.set("building_slots", slots_tbl)?;
            t.set("building_ids", building_ids_tbl)?;
        }
        if let Some(prod) = eref.get::<crate::colony::Production>() {
            let p = lua.create_table()?;
            p.set(
                "minerals_per_hexadies",
                prod.minerals_per_hexadies.final_value().to_f64(),
            )?;
            p.set(
                "energy_per_hexadies",
                prod.energy_per_hexadies.final_value().to_f64(),
            )?;
            p.set(
                "research_per_hexadies",
                prod.research_per_hexadies.final_value().to_f64(),
            )?;
            p.set(
                "food_per_hexadies",
                prod.food_per_hexadies.final_value().to_f64(),
            )?;
            t.set("production", p)?;
        }
        Ok(t)
    }

    pub fn build_ship_view(lua: &Lua, world: &mut World, entity: Entity) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        let Ok(eref) = world.get_entity(entity) else {
            return Ok(t);
        };
        let Some(ship) = eref.get::<Ship>() else {
            return Ok(t);
        };
        t.set("id", entity.to_bits())?;
        t.set("entity", entity.to_bits())?;
        t.set("name", ship.name.as_str())?;
        t.set("design_id", ship.design_id.as_str())?;
        t.set("hull_id", ship.hull_id.as_str())?;
        match ship.owner {
            Owner::Empire(e) => {
                t.set("owner_empire_id", e.to_bits())?;
                t.set("owner_kind", "empire")?;
            }
            Owner::Neutral => {
                t.set("owner_kind", "neutral")?;
            }
        }
        t.set("home_port", ship.home_port.to_bits())?;
        t.set("ftl_range", ship.ftl_range)?;
        t.set("sublight_speed", ship.sublight_speed)?;
        if let Some(fleet_entity) = ship.fleet {
            t.set("fleet_id", fleet_entity.to_bits())?;
        }
        if let Some(hp) = eref.get::<crate::ship::ShipHitpoints>() {
            let hp_tbl = lua.create_table()?;
            hp_tbl.set("hull", hp.hull)?;
            hp_tbl.set("hull_max", hp.hull_max)?;
            hp_tbl.set("armor", hp.armor)?;
            hp_tbl.set("armor_max", hp.armor_max)?;
            hp_tbl.set("shield", hp.shield)?;
            hp_tbl.set("shield_max", hp.shield_max)?;
            hp_tbl.set("shield_regen", hp.shield_regen)?;
            t.set("hp", hp_tbl)?;
        }
        let modules_tbl = lua.create_table()?;
        for (i, em) in ship.modules.iter().enumerate() {
            let entry = lua.create_table()?;
            entry.set("slot_type", em.slot_type.as_str())?;
            entry.set("module_id", em.module_id.as_str())?;
            modules_tbl.set((i + 1) as i64, entry)?;
        }
        t.set("modules", modules_tbl)?;
        if let Some(state) = eref.get::<crate::ship::ShipState>() {
            t.set("state", build_ship_state_table(lua, state)?)?;
        }
        Ok(t)
    }

    pub fn build_fleet_view(lua: &Lua, world: &mut World, entity: Entity) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        let Ok(eref) = world.get_entity(entity) else {
            return Ok(t);
        };
        let Some(fleet) = eref.get::<Fleet>() else {
            return Ok(t);
        };
        let members = eref.get::<FleetMembers>();
        t.set("id", entity.to_bits())?;
        t.set("entity", entity.to_bits())?;
        t.set("name", fleet.name.as_str())?;
        t.set("flagship", fleet.flagship.map(|e| e.to_bits()).unwrap_or(0))?;
        let members_tbl = lua.create_table()?;
        if let Some(m) = members {
            for mid in m.0.iter() {
                members_tbl.push(mid.to_bits())?;
            }
        }
        t.set("members", members_tbl)?;
        let ship_ids_tbl = lua.create_table()?;
        if let Some(m) = members {
            for mid in m.0.iter() {
                ship_ids_tbl.push(mid.to_bits())?;
            }
        }
        t.set("ship_ids", ship_ids_tbl)?;

        // Owner + state proxy via flagship (or first member) until #287 γ-2
        let proxy_ship: Option<Entity> = fleet
            .flagship
            .or_else(|| members.and_then(|m| m.0.first().copied()));
        if let Some(ps) = proxy_ship {
            if let Some(ship) = world.get::<Ship>(ps) {
                match ship.owner {
                    Owner::Empire(e) => {
                        t.set("owner_empire_id", e.to_bits())?;
                        t.set("owner_kind", "empire")?;
                    }
                    Owner::Neutral => {
                        t.set("owner_kind", "neutral")?;
                    }
                }
            }
            if let Some(ss) = world.get::<crate::ship::ShipState>(ps) {
                t.set("state", build_ship_state_table(lua, ss)?)?;
                use crate::ship::ShipState as S;
                match ss {
                    S::SubLight {
                        origin,
                        destination,
                        target_system,
                        ..
                    } => {
                        let o = lua.create_table()?;
                        o.set("x", origin[0])?;
                        o.set("y", origin[1])?;
                        o.set("z", origin[2])?;
                        t.set("origin", o)?;
                        let d = lua.create_table()?;
                        d.set("x", destination[0])?;
                        d.set("y", destination[1])?;
                        d.set("z", destination[2])?;
                        t.set("destination", d)?;
                        if let Some(ts) = target_system {
                            t.set("destination_system", ts.to_bits())?;
                        }
                    }
                    S::InFTL {
                        origin_system,
                        destination_system,
                        ..
                    } => {
                        t.set("origin_system", origin_system.to_bits())?;
                        t.set("destination_system", destination_system.to_bits())?;
                    }
                    S::Docked { system } => {
                        t.set("origin_system", system.to_bits())?;
                    }
                    _ => {}
                }
            }
        }
        Ok(t)
    }

    /// Flatten a ShipState variant into a `{kind=..., ...}` Lua table.
    pub fn build_ship_state_table(
        lua: &Lua,
        state: &crate::ship::ShipState,
    ) -> mlua::Result<Table> {
        use crate::ship::ShipState as S;
        let t = lua.create_table()?;
        match state {
            S::Docked { system } => {
                t.set("kind", "docked")?;
                t.set("system", system.to_bits())?;
            }
            S::SubLight {
                origin,
                destination,
                target_system,
                departed_at,
                arrival_at,
            } => {
                t.set("kind", "sublight")?;
                let o = lua.create_table()?;
                o.set("x", origin[0])?;
                o.set("y", origin[1])?;
                o.set("z", origin[2])?;
                t.set("origin", o)?;
                let d = lua.create_table()?;
                d.set("x", destination[0])?;
                d.set("y", destination[1])?;
                d.set("z", destination[2])?;
                t.set("destination", d)?;
                if let Some(ts) = target_system {
                    t.set("target_system", ts.to_bits())?;
                }
                t.set("departed_at", *departed_at)?;
                t.set("arrival_at", *arrival_at)?;
            }
            S::InFTL {
                origin_system,
                destination_system,
                departed_at,
                arrival_at,
            } => {
                t.set("kind", "in_ftl")?;
                t.set("origin_system", origin_system.to_bits())?;
                t.set("destination_system", destination_system.to_bits())?;
                t.set("departed_at", *departed_at)?;
                t.set("arrival_at", *arrival_at)?;
            }
            S::Surveying {
                target_system,
                started_at,
                completes_at,
            } => {
                t.set("kind", "surveying")?;
                t.set("target_system", target_system.to_bits())?;
                t.set("started_at", *started_at)?;
                t.set("completes_at", *completes_at)?;
            }
            S::Settling {
                system,
                planet,
                started_at,
                completes_at,
            } => {
                t.set("kind", "settling")?;
                t.set("system", system.to_bits())?;
                if let Some(p) = planet {
                    t.set("planet", p.to_bits())?;
                }
                t.set("started_at", *started_at)?;
                t.set("completes_at", *completes_at)?;
            }
            S::Refitting {
                system,
                started_at,
                completes_at,
                target_revision,
                ..
            } => {
                t.set("kind", "refitting")?;
                t.set("system", system.to_bits())?;
                t.set("started_at", *started_at)?;
                t.set("completes_at", *completes_at)?;
                t.set("target_revision", *target_revision)?;
            }
            S::Loitering { position } => {
                t.set("kind", "loitering")?;
                let p = lua.create_table()?;
                p.set("x", position[0])?;
                p.set("y", position[1])?;
                p.set("z", position[2])?;
                t.set("position", p)?;
            }
            S::Scouting {
                target_system,
                origin_system,
                started_at,
                completes_at,
                ..
            } => {
                t.set("kind", "scouting")?;
                t.set("target_system", target_system.to_bits())?;
                t.set("origin_system", origin_system.to_bits())?;
                t.set("started_at", *started_at)?;
                t.set("completes_at", *completes_at)?;
            }
            #[allow(unreachable_patterns)]
            other => {
                bevy::log::warn!(
                    "gamestate_scope: unknown ShipState variant, exposing as {{kind='unknown'}}: {:?}",
                    std::mem::discriminant(other)
                );
                t.set("kind", "unknown")?;
            }
        }
        Ok(t)
    }
}

// ======================================================================
// apply: &mut World mutators (write path, NEVER touches Lua).
// ======================================================================
pub(crate) mod apply {
    use super::*;
    use crate::amount::SignedAmt;
    use crate::condition::ScopedFlags;
    use crate::modifier::Modifier;
    use crate::technology::GameFlags;

    /// Parsed form of the Lua `opts` table for `push_*_modifier`.
    /// No `mlua::Value` is retained past parse.
    #[derive(Debug, Clone)]
    pub struct ModifierOpts {
        pub id: Option<String>,
        pub label: Option<String>,
        pub base_add: f64,
        pub multiplier: f64,
        pub add: f64,
        pub expires_at: Option<i64>,
    }

    /// Extract plain-data modifier options from a Lua table.
    ///
    /// Invariant: returns plain Rust values only — the input `opts`
    /// table is read but not persisted.
    pub fn parse_modifier_opts(opts: &Table) -> mlua::Result<ModifierOpts> {
        let id: Option<String> = opts.get("id").ok();
        let label: Option<String> = opts
            .get("description")
            .ok()
            .or_else(|| opts.get("label").ok());
        let base_add: f64 = opts.get("base_add").ok().unwrap_or(0.0);
        let multiplier: f64 = opts.get("multiplier").ok().unwrap_or(0.0);
        let add: f64 = opts
            .get("add")
            .ok()
            .or_else(|| opts.get("offset").ok())
            .unwrap_or(0.0);
        let expires_at: Option<i64> = opts.get("expires_at").ok();
        Ok(ModifierOpts {
            id,
            label,
            base_add,
            multiplier,
            add,
            expires_at,
        })
    }

    fn build_modifier(target: &str, opts: ModifierOpts) -> Modifier {
        Modifier {
            id: opts.id.unwrap_or_else(|| format!("lua_{}", target)),
            label: opts.label.unwrap_or_default(),
            base_add: SignedAmt::from_f64(opts.base_add),
            multiplier: SignedAmt::from_f64(opts.multiplier),
            add: SignedAmt::from_f64(opts.add),
            expires_at: opts.expires_at,
            on_expire_event: None,
        }
    }

    /// Empire-level modifier. Today only `empire.population_growth` has a
    /// typed slot; other targets land as a no-op with a RuntimeError so
    /// scripts know their target didn't apply.
    pub fn push_empire_modifier(
        world: &mut World,
        entity: Entity,
        target: &str,
        opts: ModifierOpts,
    ) -> mlua::Result<()> {
        use crate::technology::EmpireModifiers;
        let modifier = build_modifier(target, opts);
        let Ok(mut eref) = world.get_entity_mut(entity) else {
            return Err(mlua::Error::RuntimeError(format!(
                "push_empire_modifier: entity {} not found",
                entity.to_bits()
            )));
        };
        let Some(mut em) = eref.get_mut::<EmpireModifiers>() else {
            return Err(mlua::Error::RuntimeError(format!(
                "push_empire_modifier: entity {} has no EmpireModifiers",
                entity.to_bits()
            )));
        };
        match target {
            "empire.population_growth" | "population.growth" => {
                em.population_growth.push_modifier(modifier);
                Ok(())
            }
            _ => Err(mlua::Error::RuntimeError(format!(
                "push_empire_modifier: unknown target '{target}'"
            ))),
        }
    }

    /// System-level modifier — targets `ship.speed`, `ship.attack`,
    /// `ship.defense` on `SystemModifiers`.
    pub fn push_system_modifier(
        world: &mut World,
        entity: Entity,
        target: &str,
        opts: ModifierOpts,
    ) -> mlua::Result<()> {
        use crate::galaxy::SystemModifiers;
        let modifier = build_modifier(target, opts);
        let Ok(mut eref) = world.get_entity_mut(entity) else {
            return Err(mlua::Error::RuntimeError(format!(
                "push_system_modifier: entity {} not found",
                entity.to_bits()
            )));
        };
        let Some(mut sm) = eref.get_mut::<SystemModifiers>() else {
            return Err(mlua::Error::RuntimeError(format!(
                "push_system_modifier: entity {} has no SystemModifiers",
                entity.to_bits()
            )));
        };
        match target {
            "ship.speed" | "system.ship_speed" => sm.ship_speed.push_modifier(modifier),
            "ship.attack" | "system.ship_attack" => sm.ship_attack.push_modifier(modifier),
            "ship.defense" | "system.ship_defense" => sm.ship_defense.push_modifier(modifier),
            _ => {
                return Err(mlua::Error::RuntimeError(format!(
                    "push_system_modifier: unknown target '{target}'"
                )));
            }
        }
        Ok(())
    }

    /// Colony-level modifier — targets `production.{minerals,energy,research,food}`
    /// on `Production`.
    pub fn push_colony_modifier(
        world: &mut World,
        entity: Entity,
        target: &str,
        opts: ModifierOpts,
    ) -> mlua::Result<()> {
        use crate::colony::Production;
        let modifier = build_modifier(target, opts);
        let Ok(mut eref) = world.get_entity_mut(entity) else {
            return Err(mlua::Error::RuntimeError(format!(
                "push_colony_modifier: entity {} not found",
                entity.to_bits()
            )));
        };
        let Some(mut prod) = eref.get_mut::<Production>() else {
            return Err(mlua::Error::RuntimeError(format!(
                "push_colony_modifier: entity {} has no Production",
                entity.to_bits()
            )));
        };
        match target {
            "production.minerals" | "production.minerals_per_hexadies" => {
                prod.minerals_per_hexadies.push_modifier(modifier);
            }
            "production.energy" | "production.energy_per_hexadies" => {
                prod.energy_per_hexadies.push_modifier(modifier);
            }
            "production.research" | "production.research_per_hexadies" => {
                prod.research_per_hexadies.push_modifier(modifier);
            }
            "production.food" | "production.food_per_hexadies" => {
                prod.food_per_hexadies.push_modifier(modifier);
            }
            _ => {
                return Err(mlua::Error::RuntimeError(format!(
                    "push_colony_modifier: unknown target '{target}'"
                )));
            }
        }
        Ok(())
    }

    /// Ship-level modifier — targets `ship.speed` / `ship.ftl_range` / etc
    /// on `ShipModifiers`.
    pub fn push_ship_modifier(
        world: &mut World,
        entity: Entity,
        target: &str,
        opts: ModifierOpts,
    ) -> mlua::Result<()> {
        use crate::ship::ShipModifiers;
        let modifier = build_modifier(target, opts);
        let Ok(mut eref) = world.get_entity_mut(entity) else {
            return Err(mlua::Error::RuntimeError(format!(
                "push_ship_modifier: entity {} not found",
                entity.to_bits()
            )));
        };
        let Some(mut mods) = eref.get_mut::<ShipModifiers>() else {
            return Err(mlua::Error::RuntimeError(format!(
                "push_ship_modifier: entity {} has no ShipModifiers",
                entity.to_bits()
            )));
        };
        match target {
            "ship.speed" => mods.speed.push_modifier(modifier),
            "ship.ftl_range" => mods.ftl_range.push_modifier(modifier),
            "ship.survey_speed" => mods.survey_speed.push_modifier(modifier),
            "ship.colonize_speed" => mods.colonize_speed.push_modifier(modifier),
            "ship.evasion" => mods.evasion.push_modifier(modifier),
            "ship.cargo_capacity" => mods.cargo_capacity.push_modifier(modifier),
            "ship.attack" => mods.attack.push_modifier(modifier),
            "ship.defense" => mods.defense.push_modifier(modifier),
            "ship.armor_max" => mods.armor_max.push_modifier(modifier),
            "ship.shield_max" => mods.shield_max.push_modifier(modifier),
            "ship.shield_regen" => mods.shield_regen.push_modifier(modifier),
            _ => {
                return Err(mlua::Error::RuntimeError(format!(
                    "push_ship_modifier: unknown target '{target}'"
                )));
            }
        }
        Ok(())
    }

    /// Fleet-level modifier. Until #287 γ-2 (FleetState) lands, this
    /// delegates to the flagship ship's `ShipModifiers`.
    pub fn push_fleet_modifier(
        world: &mut World,
        entity: Entity,
        target: &str,
        opts: ModifierOpts,
    ) -> mlua::Result<()> {
        use crate::ship::fleet::Fleet;
        let proxy = world
            .get::<Fleet>(entity)
            .and_then(|f| f.flagship)
            .ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "push_fleet_modifier: fleet {} has no flagship (γ-2 FleetState not yet live)",
                    entity.to_bits()
                ))
            })?;
        push_ship_modifier(world, proxy, target, opts)
    }

    /// Set a flag on the given scoped entity. Only `"empire"` is supported
    /// until #293/#295 broaden scope carriers.
    pub fn set_flag(
        world: &mut World,
        scope_kind: &str,
        entity: Entity,
        name: &str,
        value: bool,
    ) -> mlua::Result<()> {
        match scope_kind {
            "empire" => {
                let Ok(mut eref) = world.get_entity_mut(entity) else {
                    return Err(mlua::Error::RuntimeError(format!(
                        "set_flag: empire entity {} not found",
                        entity.to_bits()
                    )));
                };
                if value {
                    if let Some(mut gf) = eref.get_mut::<GameFlags>() {
                        gf.set(name);
                    }
                    if let Some(mut sf) = eref.get_mut::<ScopedFlags>() {
                        sf.set(name);
                    }
                } else {
                    if let Some(mut gf) = eref.get_mut::<GameFlags>() {
                        gf.flags.remove(name);
                    }
                    if let Some(mut sf) = eref.get_mut::<ScopedFlags>() {
                        sf.flags.remove(name);
                    }
                }
                Ok(())
            }
            other => Err(mlua::Error::RuntimeError(format!(
                "set_flag: unsupported scope kind '{other}' (supported: empire)"
            ))),
        }
    }

    pub fn clear_flag(
        world: &mut World,
        scope_kind: &str,
        entity: Entity,
        name: &str,
    ) -> mlua::Result<()> {
        set_flag(world, scope_kind, entity, name, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::{Empire, PlayerEmpire};
    use crate::scripting::ScriptEngine;
    use crate::technology::GameFlags;

    fn make_world() -> World {
        let mut world = World::new();
        world.insert_resource(crate::time_system::GameClock::new(42));
        world.insert_resource(ScriptEngine::new().unwrap());
        let mut tree = crate::technology::TechTree::default();
        tree.researched
            .insert(crate::technology::TechId("tech_a".to_string()));
        let mut flags = GameFlags::default();
        flags.set("fa");
        world.spawn((
            Empire { name: "E".into() },
            PlayerEmpire,
            tree,
            flags,
            crate::condition::ScopedFlags::default(),
            crate::technology::EmpireModifiers::default(),
        ));
        world
    }

    #[test]
    fn test_dispatch_read_only_exposes_clock_and_empire() {
        let mut world = make_world();
        world.resource_scope::<ScriptEngine, _>(|world, engine| {
            let lua = engine.lua();
            let payload = lua.create_table().unwrap();
            dispatch_with_gamestate(lua, world, &payload, GamestateMode::ReadOnly, |lua, p| {
                lua.globals().set("_evt", p.clone())?;
                let now: i64 = lua.load("return _evt.gamestate.clock.now").eval().unwrap();
                assert_eq!(now, 42);
                let name: String = lua
                    .load("return _evt.gamestate:player_empire().name")
                    .eval()
                    .unwrap();
                assert_eq!(name, "E");
                Ok(())
            })
            .unwrap();
        });
    }

    #[test]
    fn test_dispatch_read_only_has_no_setters() {
        let mut world = make_world();
        world.resource_scope::<ScriptEngine, _>(|world, engine| {
            let lua = engine.lua();
            let payload = lua.create_table().unwrap();
            dispatch_with_gamestate(lua, world, &payload, GamestateMode::ReadOnly, |lua, p| {
                lua.globals().set("_evt", p.clone())?;
                // Calling a missing setter surfaces a Lua error (attempt to
                // call a nil value).
                let r: mlua::Result<()> = lua
                    .load(
                        r#"
                        _evt.gamestate:push_empire_modifier(0, "empire.population_growth", {})
                        "#,
                    )
                    .exec();
                assert!(r.is_err(), "ReadOnly mode must not expose setters");
                Ok(())
            })
            .unwrap();
        });
    }

    #[test]
    fn test_dispatch_readwrite_push_empire_modifier_live() {
        let mut world = make_world();
        // Grab player empire id.
        let pe_id = {
            let mut q = world.query_filtered::<Entity, With<PlayerEmpire>>();
            q.iter(&world).next().unwrap().to_bits()
        };
        world.resource_scope::<ScriptEngine, _>(|world, engine| {
            let lua = engine.lua();
            let payload = lua.create_table().unwrap();
            lua.globals().set("_pe_id", pe_id).unwrap();
            dispatch_with_gamestate(lua, world, &payload, GamestateMode::ReadWrite, |lua, p| {
                lua.globals().set("_evt", p.clone())?;
                lua.load(
                    r#"
                    _evt.gamestate:push_empire_modifier(
                        _pe_id,
                        "empire.population_growth",
                        { id = "test_mod", add = 1.0 }
                    )
                    "#,
                )
                .exec()
            })
            .unwrap();
        });
        // Verify live mutation persisted.
        let empire_entity = Entity::from_bits(pe_id);
        let em = world
            .get::<crate::technology::EmpireModifiers>(empire_entity)
            .unwrap();
        assert!(em.population_growth.final_value().to_f64() >= 1.0);
    }

    #[test]
    fn test_dispatch_readwrite_set_flag_live() {
        let mut world = make_world();
        let pe_id = {
            let mut q = world.query_filtered::<Entity, With<PlayerEmpire>>();
            q.iter(&world).next().unwrap().to_bits()
        };
        world.resource_scope::<ScriptEngine, _>(|world, engine| {
            let lua = engine.lua();
            let payload = lua.create_table().unwrap();
            lua.globals().set("_pe_id", pe_id).unwrap();
            dispatch_with_gamestate(lua, world, &payload, GamestateMode::ReadWrite, |lua, p| {
                lua.globals().set("_evt", p.clone())?;
                lua.load(
                    r#"_evt.gamestate:set_flag("empire", _pe_id, "trade_treaty_signed", true)"#,
                )
                .exec()
            })
            .unwrap();
        });
        let empire = Entity::from_bits(pe_id);
        let gf = world.get::<GameFlags>(empire).unwrap();
        assert!(gf.check("trade_treaty_signed"));
    }

    #[test]
    fn test_list_empires_returns_all() {
        let mut world = make_world();
        // Add a second empire.
        world.spawn((Empire {
            name: "Alien".into(),
        },));
        world.resource_scope::<ScriptEngine, _>(|world, engine| {
            let lua = engine.lua();
            let payload = lua.create_table().unwrap();
            dispatch_with_gamestate(lua, world, &payload, GamestateMode::ReadOnly, |lua, p| {
                lua.globals().set("_evt", p.clone())?;
                let count: i64 = lua
                    .load("return #_evt.gamestate:list_empires()")
                    .eval()
                    .unwrap();
                assert_eq!(count, 2);
                Ok(())
            })
            .unwrap();
        });
    }
}
