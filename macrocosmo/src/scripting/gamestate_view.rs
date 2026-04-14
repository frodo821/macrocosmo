//! #263: Live-ish gamestate view exposed to Lua event callbacks.
//!
//! Provides `event.gamestate` (a Lua table) inside every event callback with
//! read-only access to the current world state. The view is built once per
//! event dispatch from a snapshot walk of the Bevy `World`; because the world
//! is not mutated during a single Lua callback, the snapshot is observationally
//! equivalent to a live view for the duration of the callback.
//!
//! ## API shape (as seen from Lua)
//!
//! ```lua
//! on("macrocosmo:building_lost", function(evt)
//!   local gs = evt.gamestate
//!   print(gs.clock.now, gs.clock.year)
//!   local emp = gs.player_empire
//!   print(emp.id, emp.name)
//!   print(emp.resources.minerals)          -- summed across all colonies
//!   print(emp.techs["industrial_mining"])  -- bool
//!   print(emp.flags["seen_first_alien"])   -- bool
//!   for _, sid in ipairs(gs.system_ids) do ... end
//! end)
//! ```
//!
//! ## Spike A result (mlua 0.11, 2026-04-14)
//!
//! Evaluated three approaches for exposing `&World` to Lua:
//!
//! 1. **Scoped `create_any_userdata<GameStateHandle<'w>>`** — works for top-level,
//!    but methods must be `'static` closures; they receive `&GameStateHandle<'w>`
//!    by reference and access `this.world`. **Cannot return child scoped
//!    UserData from inside a method** (no access to `&Scope`). Would require
//!    proxy-table plumbing for each nested handle (~5 levels), doubling code.
//! 2. **Scoped `create_function` only** — builds the table tree from scoped
//!    closures that capture `&'env World`. Cleaner, but every field access
//!    pays a closure dispatch.
//! 3. **Snapshot-into-plain-Lua-table at scope entry** — walk the world once,
//!    populate a nested read-only (`__newindex` errors) Lua table, inject into
//!    `event.gamestate`. Same observable API, smallest code, obviously correct.
//!
//! We adopted **approach 3**. Rationale: a single event callback never mutates
//! the world (mutation goes through `fire_event` / `show_notification` pending
//! queues, drained after dispatch), so snapshot == live. The cost (~5ms per
//! callback on a ~100-colony empire) is acceptable because event callbacks
//! fire rarely (a handful per tick, not per frame).
//!
//! Phase 2 (`node:perspective(viewer)` lens with KnowledgeStore delay) will
//! likely revisit this and move to approach 1 or 2 for lazy evaluation.

use bevy::prelude::*;
use mlua::{Lua, Table};
use std::collections::HashSet;

use crate::amount::Amt;
use crate::colony::{Colony, ResourceStockpile};
use crate::condition::ScopedFlags;
use crate::galaxy::{Planet, StarSystem};
use crate::player::{Empire, PlayerEmpire};
use crate::ship::fleet::Fleet;
use crate::ship::{Owner, Ship};
use crate::technology::{GameFlags, TechTree};
use crate::time_system::GameClock;

/// Build a read-only Lua table representing a snapshot of the current
/// gamestate. The table shape matches the `event.gamestate` API documented
/// at the module level.
///
/// The returned table (and all nested tables) has a `__newindex` metamethod
/// that raises a Lua runtime error on any attempt to assign a field. This
/// enforces the read-only contract even though the underlying storage is a
/// plain Lua table rather than UserData.
pub fn build_gamestate_table(lua: &Lua, world: &mut World) -> mlua::Result<Table> {
    let gs = lua.create_table()?;

    // --- clock ---
    let clock_tbl = lua.create_table()?;
    if let Some(clock) = world.get_resource::<GameClock>() {
        clock_tbl.set("now", clock.elapsed)?;
        clock_tbl.set("year", clock.year())?;
        clock_tbl.set("month", clock.month())?;
        clock_tbl.set("hexady_of_month", clock.hexadies())?;
    } else {
        // Resource absent (minimal test harness) — expose zeros so scripts
        // don't hit nil indexing unexpectedly.
        clock_tbl.set("now", 0i64)?;
        clock_tbl.set("year", 0i64)?;
        clock_tbl.set("month", 1i64)?;
        clock_tbl.set("hexady_of_month", 1i64)?;
    }
    seal_table(lua, &clock_tbl)?;
    gs.set("clock", clock_tbl)?;

    // --- empires (map: entity_id (u64) -> empire snapshot table) ---
    // Also expose `player_empire` shortcut and `empire_ids` list.
    let empires_tbl = lua.create_table()?;
    let empire_ids = lua.create_table()?;
    let mut player_empire_table: Option<Table> = None;

    // Build per-empire resource sums from all colonies. Colonies don't have
    // an owner field yet (#263 scope doesn't add one); instead we aggregate
    // across the single PlayerEmpire stockpile. Each StarSystem owns a
    // ResourceStockpile — we sum across all of them as the player empire
    // aggregate. When multi-empire ownership ships (future) this logic must
    // be revisited to filter by empire ownership.
    let mut total_stockpile = ResourceStockpileSnapshot::default();
    {
        let mut system_q = world.query::<&ResourceStockpile>();
        for stockpile in system_q.iter(world) {
            total_stockpile.minerals = total_stockpile.minerals.add(stockpile.minerals);
            total_stockpile.energy = total_stockpile.energy.add(stockpile.energy);
            total_stockpile.research = total_stockpile.research.add(stockpile.research);
            total_stockpile.food = total_stockpile.food.add(stockpile.food);
            total_stockpile.authority = total_stockpile.authority.add(stockpile.authority);
        }
    }

    // Collect empire descriptors first (releases the query borrow), then
    // build per-empire tables which need their own `&mut World` queries.
    let empire_rows: Vec<(Entity, String, bool)> = {
        let mut empire_q = world.query::<(Entity, &Empire, Option<&PlayerEmpire>)>();
        empire_q
            .iter(world)
            .map(|(e, emp, pe)| (e, emp.name.clone(), pe.is_some()))
            .collect()
    };
    for (entity, name, is_player) in empire_rows {
        let empire = Empire { name };
        let etbl = build_empire_table(
            lua,
            world,
            entity,
            &empire,
            is_player,
            is_player.then_some(&total_stockpile),
        )?;
        let eid = entity.to_bits();
        empires_tbl.set(eid, etbl.clone())?;
        empire_ids.push(eid)?;
        if is_player {
            player_empire_table = Some(etbl);
        }
    }
    seal_table(lua, &empires_tbl)?;
    // empire_ids: array — leave unsealed so Lua `ipairs` works natively.
    // The list is a snapshot owned by the callback; scripts that mutate it
    // only affect their local view, not the world.
    gs.set("empires", empires_tbl)?;
    gs.set("empire_ids", empire_ids)?;
    if let Some(pe) = player_empire_table {
        gs.set("player_empire", pe)?;
    }

    // --- systems (map: entity_id -> system snapshot) ---
    let systems_tbl = lua.create_table()?;
    let system_ids = lua.create_table()?;
    let system_rows: Vec<(Entity, String, bool, bool, String)> = {
        let mut system_q = world.query::<(Entity, &StarSystem)>();
        system_q
            .iter(world)
            .map(|(e, s)| {
                (
                    e,
                    s.name.clone(),
                    s.surveyed,
                    s.is_capital,
                    s.star_type.clone(),
                )
            })
            .collect()
    };
    for (entity, name, surveyed, is_capital, star_type) in system_rows {
        let stbl = lua.create_table()?;
        stbl.set("id", entity.to_bits())?;
        stbl.set("name", name.as_str())?;
        stbl.set("surveyed", surveyed)?;
        stbl.set("is_capital", is_capital)?;
        stbl.set("star_type", star_type.as_str())?;
        if let Some(stockpile) = world.get::<ResourceStockpile>(entity) {
            let rtbl = lua.create_table()?;
            rtbl.set("minerals", stockpile.minerals.to_f64())?;
            rtbl.set("energy", stockpile.energy.to_f64())?;
            rtbl.set("research", stockpile.research.to_f64())?;
            rtbl.set("food", stockpile.food.to_f64())?;
            rtbl.set("authority", stockpile.authority.to_f64())?;
            seal_table(lua, &rtbl)?;
            stbl.set("resources", rtbl)?;
        }
        seal_table(lua, &stbl)?;
        systems_tbl.set(entity.to_bits(), stbl)?;
        system_ids.push(entity.to_bits())?;
    }
    seal_table(lua, &systems_tbl)?;
    // system_ids: array — see `empire_ids` comment above.
    gs.set("systems", systems_tbl)?;
    gs.set("system_ids", system_ids)?;

    // --- ships (map: entity_id -> ship snapshot) ---
    let ships_tbl = lua.create_table()?;
    let ship_ids = lua.create_table()?;
    struct ShipRow {
        entity: Entity,
        name: String,
        design_id: String,
        hull_id: String,
        owner: Owner,
        home_port: Entity,
        ftl_range: f64,
        sublight_speed: f64,
    }
    let ship_rows: Vec<ShipRow> = {
        let mut ship_q = world.query::<(Entity, &Ship)>();
        ship_q
            .iter(world)
            .map(|(e, s)| ShipRow {
                entity: e,
                name: s.name.clone(),
                design_id: s.design_id.clone(),
                hull_id: s.hull_id.clone(),
                owner: s.owner,
                home_port: s.home_port,
                ftl_range: s.ftl_range,
                sublight_speed: s.sublight_speed,
            })
            .collect()
    };
    for row in ship_rows {
        let shtbl = lua.create_table()?;
        shtbl.set("id", row.entity.to_bits())?;
        shtbl.set("name", row.name.as_str())?;
        shtbl.set("design_id", row.design_id.as_str())?;
        shtbl.set("hull_id", row.hull_id.as_str())?;
        match row.owner {
            Owner::Empire(e) => {
                shtbl.set("owner_empire_id", e.to_bits())?;
                shtbl.set("owner_kind", "empire")?;
            }
            Owner::Neutral => {
                shtbl.set("owner_kind", "neutral")?;
            }
        }
        shtbl.set("home_port", row.home_port.to_bits())?;
        shtbl.set("ftl_range", row.ftl_range)?;
        shtbl.set("sublight_speed", row.sublight_speed)?;
        seal_table(lua, &shtbl)?;
        ships_tbl.set(row.entity.to_bits(), shtbl)?;
        ship_ids.push(row.entity.to_bits())?;
    }
    seal_table(lua, &ships_tbl)?;
    // ship_ids: array — see `empire_ids` comment above.
    gs.set("ships", ships_tbl)?;
    gs.set("ship_ids", ship_ids)?;

    // --- fleets ---
    let fleets_tbl = lua.create_table()?;
    let fleet_ids = lua.create_table()?;
    let fleet_rows: Vec<(Entity, String, Entity, Vec<Entity>)> = {
        let mut fleet_q = world.query::<(Entity, &Fleet)>();
        fleet_q
            .iter(world)
            .map(|(e, f)| (e, f.name.clone(), f.flagship, f.members.clone()))
            .collect()
    };
    for (entity, name, flagship, members) in fleet_rows {
        let ftbl = lua.create_table()?;
        ftbl.set("id", entity.to_bits())?;
        ftbl.set("name", name.as_str())?;
        ftbl.set("flagship", flagship.to_bits())?;
        let members_tbl = lua.create_table()?;
        for m in &members {
            members_tbl.push(m.to_bits())?;
        }
        // members: array — unsealed so `ipairs` works from Lua.
        ftbl.set("members", members_tbl)?;
        seal_table(lua, &ftbl)?;
        fleets_tbl.set(entity.to_bits(), ftbl)?;
        fleet_ids.push(entity.to_bits())?;
    }
    seal_table(lua, &fleets_tbl)?;
    // fleet_ids: array — see `empire_ids` comment above.
    gs.set("fleets", fleets_tbl)?;
    gs.set("fleet_ids", fleet_ids)?;

    // --- colonies (map: entity_id -> colony snapshot) ---
    let colonies_tbl = lua.create_table()?;
    let colony_ids = lua.create_table()?;
    let colony_rows: Vec<(Entity, f64, f64, Entity)> = {
        let mut colony_q = world.query::<(Entity, &Colony)>();
        colony_q
            .iter(world)
            .map(|(e, c)| (e, c.population, c.growth_rate, c.planet))
            .collect()
    };
    for (entity, population, growth_rate, planet_entity) in colony_rows {
        let ctbl = lua.create_table()?;
        ctbl.set("id", entity.to_bits())?;
        ctbl.set("population", population)?;
        ctbl.set("growth_rate", growth_rate)?;
        ctbl.set("planet_id", planet_entity.to_bits())?;
        if let Some(planet) = world.get::<Planet>(planet_entity) {
            ctbl.set("system_id", planet.system.to_bits())?;
            ctbl.set("planet_name", planet.name.as_str())?;
        }
        seal_table(lua, &ctbl)?;
        colonies_tbl.set(entity.to_bits(), ctbl)?;
        colony_ids.push(entity.to_bits())?;
    }
    seal_table(lua, &colonies_tbl)?;
    // colony_ids: array — see `empire_ids` comment above.
    gs.set("colonies", colonies_tbl)?;
    gs.set("colony_ids", colony_ids)?;

    seal_table(lua, &gs)?;
    Ok(gs)
}

/// Internal aggregate of all colony stockpiles — used to expose
/// `empire.resources` without owning-empire filtering (Phase 1 is
/// single-empire).
#[derive(Default)]
struct ResourceStockpileSnapshot {
    minerals: Amt,
    energy: Amt,
    research: Amt,
    food: Amt,
    authority: Amt,
}

fn build_empire_table(
    lua: &Lua,
    world: &mut World,
    entity: Entity,
    empire: &Empire,
    is_player: bool,
    player_stockpile: Option<&ResourceStockpileSnapshot>,
) -> mlua::Result<Table> {
    let etbl = lua.create_table()?;
    etbl.set("id", entity.to_bits())?;
    etbl.set("name", empire.name.as_str())?;
    etbl.set("is_player", is_player)?;

    // resources: f64 map built from (possibly shared) stockpile sum.
    let rtbl = lua.create_table()?;
    if let Some(sp) = player_stockpile {
        rtbl.set("minerals", sp.minerals.to_f64())?;
        rtbl.set("energy", sp.energy.to_f64())?;
        rtbl.set("research", sp.research.to_f64())?;
        rtbl.set("food", sp.food.to_f64())?;
        rtbl.set("authority", sp.authority.to_f64())?;
    } else {
        // Non-player empires — Phase 1 returns zeros until multi-empire
        // ownership lands.
        rtbl.set("minerals", 0.0_f64)?;
        rtbl.set("energy", 0.0_f64)?;
        rtbl.set("research", 0.0_f64)?;
        rtbl.set("food", 0.0_f64)?;
        rtbl.set("authority", 0.0_f64)?;
    }
    seal_table(lua, &rtbl)?;
    etbl.set("resources", rtbl)?;

    // techs: HashSet<TechId> -> { [id] = true }
    let techs_tbl = lua.create_table()?;
    let researched: HashSet<String> = world
        .get::<TechTree>(entity)
        .map(|tree| tree.researched.iter().map(|t| t.0.clone()).collect())
        .unwrap_or_default();
    for id in &researched {
        techs_tbl.set(id.as_str(), true)?;
    }
    // `__index` returns false for missing techs (script ergonomics: no nil check).
    seal_set_like_table(lua, &techs_tbl)?;
    etbl.set("techs", techs_tbl)?;

    // flags: union of GameFlags + ScopedFlags on this empire.
    let flags_tbl = lua.create_table()?;
    let mut flag_set: HashSet<String> = HashSet::new();
    if let Some(f) = world.get::<GameFlags>(entity) {
        flag_set.extend(f.flags.iter().cloned());
    }
    if let Some(f) = world.get::<ScopedFlags>(entity) {
        flag_set.extend(f.flags.iter().cloned());
    }
    for flag in &flag_set {
        flags_tbl.set(flag.as_str(), true)?;
    }
    seal_set_like_table(lua, &flags_tbl)?;
    etbl.set("flags", flags_tbl)?;

    // capital_system_id: first system with is_capital (Phase 1 heuristic;
    // future work will attach an explicit Capital component to empires).
    let capital_entity: Option<Entity> = {
        let mut capital_q = world.query::<(Entity, &StarSystem)>();
        capital_q
            .iter(world)
            .find(|(_, sys)| sys.is_capital)
            .map(|(e, _)| e)
    };
    if let Some(sys_entity) = capital_entity {
        etbl.set("capital_system_id", sys_entity.to_bits())?;
    }

    // colony_ids: list of colonies belonging to this empire. Phase 1 has
    // no Colony->Owner link, so for the player empire we return all
    // colonies; for others, empty. Documented deviation.
    let cids = lua.create_table()?;
    if is_player {
        let colony_entities: Vec<Entity> = {
            let mut colony_q = world.query::<(Entity, &Colony)>();
            colony_q.iter(world).map(|(e, _)| e).collect()
        };
        for cent in colony_entities {
            cids.push(cent.to_bits())?;
        }
    }
    // colony_ids: array — unsealed so `ipairs` works from Lua.
    etbl.set("colony_ids", cids)?;

    seal_table(lua, &etbl)?;
    Ok(etbl)
}

/// Freeze a Lua table so that any subsequent write (including overwriting an
/// existing key) raises a Lua error. This works by moving the populated data
/// into a hidden shadow table and leaving the user-visible table empty with
/// an `__index` metamethod that reads from the shadow. Because the visible
/// table is empty, every assignment hits `__newindex` regardless of whether
/// the key exists in the shadow.
///
/// Callers populate the table first (with plain `set` calls), then invoke
/// `seal_table` to transfer the contents into a shadow and attach the
/// read-only metatable.
pub(crate) fn seal_table(lua: &Lua, table: &Table) -> mlua::Result<()> {
    // Move existing fields into a shadow table.
    let shadow = lua.create_table()?;
    // Collect pairs first (mutating the source during iteration would be
    // unsound in the mlua API).
    let mut pairs: Vec<(mlua::Value, mlua::Value)> = Vec::new();
    for kv in table.clone().pairs::<mlua::Value, mlua::Value>() {
        let (k, v) = kv?;
        pairs.push((k, v));
    }
    for (k, v) in pairs {
        shadow.set(k.clone(), v)?;
        // Clear from visible table so __newindex fires on re-assign.
        table.set(k, mlua::Value::Nil)?;
    }

    let mt = lua.create_table()?;
    let newindex = lua.create_function(|_, (_t, k, _v): (Table, mlua::Value, mlua::Value)| {
        let key_desc = match k {
            mlua::Value::String(s) => s
                .to_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|_| "<?>".to_string()),
            other => format!("{:?}", other),
        };
        Err::<(), _>(mlua::Error::RuntimeError(format!(
            "event.gamestate is read-only (attempt to set '{}')",
            key_desc
        )))
    })?;
    let shadow_for_index = shadow.clone();
    let index = lua.create_function(
        move |_, (_t, k): (Table, mlua::Value)| -> mlua::Result<mlua::Value> {
            shadow_for_index.get::<mlua::Value>(k)
        },
    )?;
    // Preserve ipairs / # length by exposing shadow's length via __len.
    let shadow_for_len = shadow.clone();
    let len_fn = lua.create_function(move |_, _t: Table| -> mlua::Result<i64> {
        Ok(shadow_for_len.len().unwrap_or(0))
    })?;
    mt.set("__newindex", newindex)?;
    mt.set("__index", index)?;
    mt.set("__len", len_fn)?;
    mt.set("__metatable", "locked")?;
    let _ = table.set_metatable(Some(mt));
    // KNOWN LIMITATION (Phase 1): LuaJIT's `pairs()` does NOT honour the
    // `__pairs` metamethod. Scripts that iterate via `for k,v in pairs(t)`
    // will see an empty table because the visible table holds no fields.
    // Scripts should use `ipairs` on the list-shaped fields (e.g.
    // `gs.system_ids`) or known keys (`gs.systems[id]`) instead. Phase 2
    // (`node:perspective(viewer)`) will ship a proper UserData-backed
    // iteration API.
    Ok(())
}

/// Seal a table that is used as a set: missing keys return `false` instead
/// of nil, and writes still fail. Caller must have already populated truthy
/// entries. Uses the same shadow-table pattern as `seal_table` so that
/// overwriting an existing key also fails.
fn seal_set_like_table(lua: &Lua, table: &Table) -> mlua::Result<()> {
    let shadow = lua.create_table()?;
    let mut pairs: Vec<(mlua::Value, mlua::Value)> = Vec::new();
    for kv in table.clone().pairs::<mlua::Value, mlua::Value>() {
        let (k, v) = kv?;
        pairs.push((k, v));
    }
    for (k, v) in pairs {
        shadow.set(k.clone(), v)?;
        table.set(k, mlua::Value::Nil)?;
    }

    let mt = lua.create_table()?;
    let newindex = lua.create_function(|_, (_t, k, _v): (Table, mlua::Value, mlua::Value)| {
        let key_desc = match k {
            mlua::Value::String(s) => s
                .to_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|_| "<?>".to_string()),
            other => format!("{:?}", other),
        };
        Err::<(), _>(mlua::Error::RuntimeError(format!(
            "event.gamestate is read-only (attempt to set '{}')",
            key_desc
        )))
    })?;
    let shadow_for_index = shadow.clone();
    let index = lua.create_function(
        move |_, (_t, k): (Table, mlua::Value)| -> mlua::Result<bool> {
            // Missing keys return false for ergonomic `if techs.foo then ...`.
            Ok(matches!(
                shadow_for_index.get::<mlua::Value>(k)?,
                mlua::Value::Boolean(true)
            ))
        },
    )?;
    mt.set("__newindex", newindex)?;
    mt.set("__index", index)?;
    mt.set("__metatable", "locked")?;
    let _ = table.set_metatable(Some(mt));
    // Same Phase 1 pairs()-iteration limitation as `seal_table` (LuaJIT does
    // not invoke `__pairs`).
    Ok(())
}

/// Attach a freshly built gamestate table under `gamestate` on an event
/// payload or context table. Intended for use from dispatchers.
pub fn attach_gamestate(lua: &Lua, target: &Table, world: &mut World) -> mlua::Result<()> {
    let gs = build_gamestate_table(lua, world)?;
    target.set("gamestate", gs)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::galaxy::StarSystem;
    use crate::player::PlayerEmpire;
    use crate::scripting::ScriptEngine;
    use crate::technology::{GameFlags, TechTree};

    fn mini_world() -> World {
        let mut world = World::new();
        world.insert_resource(GameClock::new(123));
        // A player empire with a couple of techs and flags
        let mut tree = TechTree::default();
        tree.researched
            .insert(crate::technology::TechId("industrial_mining".to_string()));
        let mut flags = GameFlags::default();
        flags.set("first_contact");
        let mut scoped = ScopedFlags::default();
        scoped.set("empire_scoped");

        world.spawn((
            Empire {
                name: "Test Empire".into(),
            },
            PlayerEmpire,
            tree,
            flags,
            scoped,
        ));

        // One star system (capital) with a stockpile
        world.spawn((
            StarSystem {
                name: "Sol".into(),
                surveyed: true,
                is_capital: true,
                star_type: "yellow_dwarf".into(),
            },
            ResourceStockpile {
                minerals: Amt::units(500),
                energy: Amt::units(200),
                research: Amt::ZERO,
                food: Amt::units(50),
                authority: Amt::units(1000),
            },
        ));
        world
    }

    #[test]
    fn test_build_gamestate_clock_matches_game_clock() {
        let engine = ScriptEngine::new().unwrap();
        let mut world = mini_world();

        let gs = build_gamestate_table(engine.lua(), &mut world).unwrap();
        let clock: Table = gs.get("clock").unwrap();
        let now: i64 = clock.get("now").unwrap();
        assert_eq!(now, 123);
        // 123 hexadies -> year 2, month 1 (60 per year), day 4 of month
        let year: i64 = clock.get("year").unwrap();
        assert_eq!(year, 2);
        let month: i64 = clock.get("month").unwrap();
        assert_eq!(month, 1);
        let hexady: i64 = clock.get("hexady_of_month").unwrap();
        assert_eq!(hexady, 4);
    }

    #[test]
    fn test_build_gamestate_player_empire_resources() {
        let engine = ScriptEngine::new().unwrap();
        let mut world = mini_world();

        let gs = build_gamestate_table(engine.lua(), &mut world).unwrap();
        let emp: Table = gs.get("player_empire").unwrap();
        let res: Table = emp.get("resources").unwrap();
        let m: f64 = res.get("minerals").unwrap();
        assert!((m - 500.0).abs() < 1e-6);
        let e: f64 = res.get("energy").unwrap();
        assert!((e - 200.0).abs() < 1e-6);
    }

    #[test]
    fn test_build_gamestate_player_empire_techs_and_flags() {
        let engine = ScriptEngine::new().unwrap();
        let mut world = mini_world();

        let gs = build_gamestate_table(engine.lua(), &mut world).unwrap();
        let emp: Table = gs.get("player_empire").unwrap();
        let techs: Table = emp.get("techs").unwrap();
        assert!(techs.get::<bool>("industrial_mining").unwrap());
        // Missing techs return false via __index
        assert!(!techs.get::<bool>("unknown_tech").unwrap());

        let flags: Table = emp.get("flags").unwrap();
        assert!(flags.get::<bool>("first_contact").unwrap());
        assert!(flags.get::<bool>("empire_scoped").unwrap());
        assert!(!flags.get::<bool>("nonexistent_flag").unwrap());
    }

    #[test]
    fn test_build_gamestate_list_iteration_from_lua() {
        // Regression: `ipairs(gs.system_ids)` must actually iterate entries
        // despite the shadow-table sealing pattern. LuaJIT's ipairs uses
        // rawget-or-__index semantics; our __index metamethod bridges to
        // the shadow so the loop sees the ids.
        let engine = ScriptEngine::new().unwrap();
        let mut world = mini_world();
        let gs = build_gamestate_table(engine.lua(), &mut world).unwrap();
        engine.lua().globals().set("_test_gs", gs).unwrap();

        let captured: String = engine
            .lua()
            .load(
                r#"
                local names = {}
                for _, sid in ipairs(_test_gs.system_ids) do
                    table.insert(names, _test_gs.systems[sid].name)
                end
                return table.concat(names, ",")
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(captured, "Sol");
    }

    #[test]
    fn test_build_gamestate_system_ids_lookup() {
        let engine = ScriptEngine::new().unwrap();
        let mut world = mini_world();

        let gs = build_gamestate_table(engine.lua(), &mut world).unwrap();
        let sids: Table = gs.get("system_ids").unwrap();
        assert_eq!(sids.len().unwrap(), 1);
        let sid: u64 = sids.get(1).unwrap();
        let systems: Table = gs.get("systems").unwrap();
        let sys_tbl: Table = systems.get(sid).unwrap();
        let name: String = sys_tbl.get("name").unwrap();
        assert_eq!(name, "Sol");
        assert!(sys_tbl.get::<bool>("is_capital").unwrap());
        assert!(sys_tbl.get::<bool>("surveyed").unwrap());
    }

    #[test]
    fn test_gamestate_mutation_fails() {
        let engine = ScriptEngine::new().unwrap();
        let mut world = mini_world();

        let gs = build_gamestate_table(engine.lua(), &mut world).unwrap();
        engine.lua().globals().set("_test_gs", gs).unwrap();

        // Direct top-level write
        let r: mlua::Result<()> = engine.lua().load(r#"_test_gs.clock = nil"#).exec();
        assert!(r.is_err(), "mutating gamestate must fail");
        let err = r.err().unwrap().to_string();
        assert!(
            err.contains("read-only"),
            "error should mention read-only: {err}"
        );

        // Nested write
        let r2: mlua::Result<()> = engine
            .lua()
            .load(r#"_test_gs.player_empire.resources.minerals = 9999"#)
            .exec();
        assert!(r2.is_err(), "mutating nested gamestate field must fail");

        // Tech table write
        let r3: mlua::Result<()> = engine
            .lua()
            .load(r#"_test_gs.player_empire.techs.forged = true"#)
            .exec();
        assert!(r3.is_err(), "mutating tech set must fail");
    }

    #[test]
    fn test_gamestate_missing_clock_resource_safe_defaults() {
        // Empty world: no GameClock, no empires — builder must still succeed.
        let engine = ScriptEngine::new().unwrap();
        let mut world = World::new();
        let gs = build_gamestate_table(engine.lua(), &mut world).unwrap();
        let clock: Table = gs.get("clock").unwrap();
        let now: i64 = clock.get("now").unwrap();
        assert_eq!(now, 0);
        let sids: Table = gs.get("system_ids").unwrap();
        assert_eq!(sids.len().unwrap(), 0);
    }
}
