# 実装計画書: Issue #332 — gamestate architecture pivot (pure scoped closures / Option B)

_Prepared 2026-04-15 by Plan agent._

---

## 0. TL;DR

現行 `event.gamestate` は **snapshot-per-event** (`build_gamestate_table` が ~100 Lua ValueRef を event ごとに生成、#320 で `gc_collect()` で強制回収) で動いている。これを **`Lua::scope` + `create_function` / `create_function_mut` による live read/write closure** (Option B) に置き換える。

- **UserData は一切使わない**。scope closure のみで gamestate 構造を組む (method 戻り値が scope 制約に抵触しないよう、setter は `()` 返し、getter は plain Lua table 返し)
- **world は `RefCell<&mut World>` で closure 間共有**。read は `try_borrow`、write は `try_borrow_mut`
- **live within tick**: event callback 内 mutation は同 callback 内の後続 read に反映
- **reentrancy 保護**: `fire_event` は queue-only、`try_borrow*` 失敗は `mlua::Error::RuntimeError` (panic 禁止)
- **write helper は Lua 不接触** invariant (`memory/feedback_rust_no_lua_callback.md`): `apply_*` は `&mut World` のみ受け、Lua value / Function / RegistryKey を触らない

**破棄される従来資産**:
- `build_gamestate_table` / `attach_gamestate` の snapshot build
- `lua.gc_collect()` (lifecycle.rs:337, 461)
- #328 per-tick cache 案 (obsolete、close 済)
- `_pending_flags` / `_pending_global_mods` の event callback path (lifecycle / tech effect / faction hook path は維持)

**migration 方針: (b) 一気に置換**。旧 `gs.systems[id].planet_ids` 等の snapshot sealed-table shape は捨てて、`ctx.gamestate:system(id)` method 形へ全面置換。既存 event callback script / integration test もすべて書き換える (下位互換レイヤは用意しない)。

---

## §1 現状の棚卸し

### 1.1 `build_gamestate_table` / `attach_gamestate` の現行 flow

| entry point | 場所 | 呼出契機 |
|---|---|---|
| `build_gamestate_table` | `macrocosmo/src/scripting/gamestate_view.rs:72-602` | gamestate 全体 snapshot 構築 (~100 Lua ref/call) |
| `attach_gamestate` | `macrocosmo/src/scripting/gamestate_view.rs:956-960` | `target.set("gamestate", gs)` の thin wrapper |
| `seal_table` | `macrocosmo/src/scripting/gamestate_view.rs:849-903` | shadow-table トリックで read-only 化、pub(crate) |
| `seal_set_like_table` | `macrocosmo/src/scripting/gamestate_view.rs:909-952` | techs / flags の `__index(id)->bool` |
| `build_empire_table` | `macrocosmo/src/scripting/gamestate_view.rs:742-837` | empire 単体 |
| `build_ship_state_table` | `macrocosmo/src/scripting/gamestate_view.rs:622-740` | ShipState tag-union snapshot |
| `ResourceStockpileSnapshot` | `macrocosmo/src/scripting/gamestate_view.rs:607-614` | 全 colony stockpile sum の中間型 |

### 1.2 `evaluate_fire_conditions` / `dispatch_event_handlers` 呼出側

**`evaluate_fire_conditions`** (`macrocosmo/src/scripting/lifecycle.rs:161-375`):
- L230 `fire_condition.is_some()` filter (#320 Commit B、Periodic にも適用)
- L281-340 `world.resource_scope::<ScriptEngine, _>` closure
  - L285 `build_gamestate_table(lua, world)` — 1 tick 1 回
  - L297 `payload.set("gamestate", gs_table)`
  - L299-330 各 `PendingDecision` について fire_condition 呼出
  - **L337 `lua.gc_collect()`** (#320 fix、本 pivot で撤回予定)
- L346-374 suppress 適用フェーズ (snapshot 不要、定義ミューテート)

**`dispatch_event_handlers`** (`macrocosmo/src/scripting/lifecycle.rs:388-465`):
- L399-402 `fired_log.drain(..)`
- L408-464 `world.resource_scope::<ScriptEngine, _>` closure
  - 各 `FiredEvent` について:
    - L414-435 payload_table 生成 (`EventContext::to_lua_table` または `event_id` だけ)
    - L437 `attach_gamestate(lua, &payload_table, world)` — event ごとに毎回 build
    - L446 `dispatch_bus_handlers(...)` (L477-525)
    - L454 `dispatch_on_trigger(...)` (L531-556)
  - **L461 `lua.gc_collect()`** (#320 fix、本 pivot で撤回予定)

### 1.3 View 型 (#289 PR #329) が現在 expose している field

`macrocosmo/src/scripting/gamestate_view.rs` 現行:

| view | 場所 | 主要 field |
|---|---|---|
| root (gs) | `:73-601` | `clock, empires, empire_ids, player_empire, planets, planet_ids, systems, system_ids, ships, ship_ids, fleets, fleet_ids, colonies, colony_ids` |
| clock | `:75-91` | `now, year, month, hexady_of_month` |
| EmpireView | `:742-837` | `id, name, is_player, resources.{minerals,energy,research,food,authority}, techs, tech (alias), flags, capital_system_id, colony_ids` |
| SystemView | `:212-302` | `id, entity, name, surveyed, is_capital, star_type, position?.{x,y,z}, resources?, planet_ids, colony_ids, owner_empire_id?, modifiers?` |
| PlanetView | `:159-188` | `id, entity, name, planet_type, biome (placeholder = planet_type), system_id` |
| ColonyView | `:507-598` | `id, entity, population, growth_rate, planet_id, system_id?, planet_name?, owner_empire_id?, building_slots, building_ids, production.{minerals,energy,research,food}` |
| ShipView | `:305-401` | `id, entity, name, design_id, hull_id, owner_empire_id?, owner_kind, home_port, ftl_range, sublight_speed, fleet_id?, hp?, modules[], state?` |
| FleetView | `:404-505` | `id, entity, name, flagship, members, ship_ids, owner_empire_id?, owner_kind?, state?, origin?, destination?, origin_system?, destination_system?` (flagship proxy) |
| ShipState tag-union | `:622-740` | `kind ∈ {docked,sublight,in_ftl,surveying,settling,refitting,loitering,scouting,unknown}` |

### 1.4 既存 `_pending_*` queue の書き込みサイト

grep 実測 (2026-04-15):

**`_pending_flags`** (Lua global) — Lua `set_flag(name)` で積まれる (`globals.rs:178-187`):
- **書き込み (Lua 側)**: `scripts/tech/*.lua` 各所 (`scope:set_flag(...)` — ただしこれは **effect_scope** 経由、後述)
- **drain (Rust 側)**:
  - `lifecycle.rs:89` — `run_lifecycle_hooks` で on_game_start 後 drain
  - `technology/effects.rs:337` — `apply_tech_effects` で tech 適用後 drain
  - `technology/effects.rs:143` — `build_tech_effects_preview` で preview 後 drain (side-effect 防止)
  - `faction/mod.rs:825` — faction hook 適用後 drain
- **seed (Rust 側)**: `globals.rs:160` に空 table として登録

**`_pending_global_mods`** (Lua global) — Lua `modify_global(param, value)` で積まれる (`globals.rs:166-174`):
- **書き込み (Lua 側)**: `scripts/config/balance.lua` 内で `push_modifier("balance.<field>", ...)`、tech `on_researched` 内で `scope:push_modifier(...)` (effect_scope 経由)
- **drain (Rust 側)**:
  - `technology/effects.rs:331` — `apply_tech_effects`
  - `technology/effects.rs:142` — `build_tech_effects_preview`
  - `faction/mod.rs:835` — faction hook
- **seed**: `globals.rs:157`

**`_pending_script_events`** (Lua global) — Lua `fire_event(id, target?)` で積まれる (`globals.rs:550-562`):
- **drain (Rust 側)**:
  - `lifecycle.rs:108-142` `drain_script_events` per-tick system、`EventSystem::fire_event` に転送
- **seed**: `globals.rs:358`

**`_pending_notifications`** (Lua global) — Lua `show_notification {...}` (`globals.rs:370-` 付近):
- **drain**: 別 system (本 pivot 範囲外、touch しない)

### 1.5 EffectScope 経由の mutation declaration

`macrocosmo/src/scripting/effect_scope.rs`:
- `scope:push_modifier(target, opts)` (L34-) → descriptor table (`_effect_type = "push_modifier"`) を返す、**Lua レベルの effect 宣言**
- `scope:set_flag(name, value, opts?)` (L81-) → `_effect_type = "set_flag"` descriptor
- 呼出元: tech `on_researched`, faction action callbacks
- **note**: これは "effect 宣言" であり `_pending_*` への直接 push ではない。`collect_effects` (`technology/effects.rs` 付近) が scope から descriptor を吸い出して `DescriptiveEffect` へ変換し、`apply_effect` が本物の World mutation を行う

→ 本 pivot では **EffectScope pattern を維持** (scope 外 context での declarative effect 宣言)。event callback 内の mutation だけ live 化する。

### 1.6 既存 event callback test の一覧 (migration 必要)

新 API (`ctx.gamestate:empire(id)` method 形) に書き換え必要な test:

- `macrocosmo/src/scripting/lifecycle.rs:814-946` (mod tests)
  - `test_dispatch_attaches_gamestate_to_bus_handler` (L815)
  - `test_dispatch_invokes_on_trigger_with_gamestate` (L858)
  - `test_dispatch_gamestate_mutation_inside_handler_fails_gracefully` (L905) — **従来: mutation 禁止。pivot 後: mutation 許可 → test 意味論を反転**
  - `test_fire_condition_suppresses_periodic_event` (L959)
  - `test_fire_condition_allows_periodic_event` (L1010)
  - `test_fire_condition_suppresses_pending_mtth_event` (L1055)
  - `test_existing_event_scripts_still_work` (L1099) — payload 動作確認 (pivot 影響なし)
- `macrocosmo/src/scripting/gamestate_view.rs:1012-1653` (mod tests、~20 test)
  - すべて `build_gamestate_table(engine.lua(), &mut world)` 直叩き
  - 新 API では `dispatch_with_gamestate` 経由でないと gamestate が露出しないため、test helper を新設するか test 側を dispatch-based に書き換え
- `macrocosmo/tests/lua_view_types.rs:155-299` (integration tests 3 本)
  - `test_gamestate_view_hierarchical_navigation` (L158)
  - `test_ship_state_tag_union_docked` (L228 付近)
  - `test_fleet_proxy_through_flagship` (L280 付近)
- `macrocosmo/tests/stress_lua_scheduling.rs:136-173` (#320 regression)
  - 新 API で snapshot build がない → 1000 tick 実測値が大幅減るはず (§8.2)

### 1.7 `stress_lua_scheduling.rs` の aux stack 閾値

- `STRESS_TICKS: i64 = 1000` (L27)
- `LUA_MEMORY_CEILING_BYTES: usize = 32 * 1024 * 1024` (L35)
- baseline (healthy build): ~1 MiB
- 1 tick あたり: `evaluate_fire_conditions` (snapshot 1 回) + `dispatch_event_handlers` (event あたり snapshot 1 回 = 1 回/tick) = 2 回 build
- pivot 後の想定: closure ref は `lua.scope` 終了時に自動 invalidate、Lua registry に残存しない想定。final_memory は baseline に近く収まる想定

---

## §2 Option B アーキテクチャ詳細

### 2.1 Entry point signature

```rust
// macrocosmo/src/scripting/gamestate_scope.rs (新規)

/// Dispatch a Lua callback with a live `gamestate` accessor attached to
/// `payload`. The accessor is constructed from `Lua::scope` closures that
/// share `world` through a `RefCell`; the closures are invalidated when
/// this function returns.
///
/// INVARIANT: `fire_event` inside the callback must go through
/// `_pending_script_events`; sync dispatch is not supported here.
///
/// INVARIANT: write helpers never call back into Lua. See
/// `memory/feedback_rust_no_lua_callback.md`.
///
/// `mode` selects whether write closures are exposed:
///   - `ReadOnly`: fire_condition / pure read contexts. Setters are NOT
///     registered on the gamestate table (calling them is a Lua
///     `attempt to call a nil value` error, surfaced at the script site).
///   - `ReadWrite`: event callbacks / lifecycle hooks that may mutate.
pub enum GamestateMode {
    ReadOnly,
    ReadWrite,
}

pub fn dispatch_with_gamestate<'a>(
    lua: &Lua,
    world: &'a mut World,
    payload: &Table,
    mode: GamestateMode,
    callback: impl FnOnce(&Lua, &Table) -> mlua::Result<()> + 'a,
) -> mlua::Result<()> {
    let world_cell = std::cell::RefCell::new(world);
    lua.scope(|s| {
        let gs = build_scoped_gamestate(lua, s, &world_cell, mode)?;
        payload.set("gamestate", gs)?;
        callback(lua, payload)
    })
}
```

- Callback は `FnOnce(&Lua, &Table) -> mlua::Result<()>` 形にして、呼出し側が既存 `func.call::<()>(payload.clone())` を包む
- `callback` は `'a` で縛って world の lifetime と揃える (closure 内で Lua function を実行、結果は borrow 期間内で完結)

### 2.2 `build_scoped_gamestate` の構造

```rust
fn build_scoped_gamestate<'a, 'scope>(
    lua: &Lua,
    s: &Scope<'scope, 'a>,
    world_cell: &'a RefCell<&'a mut World>,
) -> mlua::Result<Table> {
    let gs = lua.create_table()?;

    // ---- read closures ----
    // ctx.gamestate:empire(id) -> Table (empire view)
    let read_empire = s.create_function(move |lua, (_this, id): (Table, u64)| {
        let borrow = world_cell.try_borrow().map_err(|e| {
            mlua::Error::RuntimeError(format!(
                "gamestate reentrancy: read during write ({e})"
            ))
        })?;
        let entity = Entity::from_bits(id);
        views::build_empire_view(lua, &*borrow, entity)
    })?;
    gs.set("empire", read_empire)?;

    // ctx.gamestate:system(id), :colony(id), :fleet(id), :ship(id),
    // :planet(id) — all symmetric
    // ...

    // ctx.gamestate:list_empires() -> Array<u64>
    let list_empires = s.create_function(move |lua, _: Table| {
        let borrow = world_cell.try_borrow().map_err(map_reentrancy_err)?;
        views::list_empire_ids(lua, &*borrow)
    })?;
    gs.set("list_empires", list_empires)?;

    // ctx.gamestate.clock -> clock table (snapshot ok here, plain-old-data)
    gs.set("clock", views::build_clock_table(lua, &*world_cell.borrow())?)?;

    // ---- write closures ----
    // ctx.gamestate:push_empire_modifier(id, target, opts) -> nil
    let push_empire_mod = s.create_function_mut(
        move |_lua, (_this, id, target, opts): (Table, u64, String, Table)| {
            let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
            let entity = Entity::from_bits(id);
            // opts is immediately extracted to a plain ModifierOpts struct
            // (no Lua value escapes into `apply_*`).
            let parsed = parse_modifier_opts(&opts)?;
            apply::push_empire_modifier(&mut *borrow, entity, &target, parsed)
        },
    )?;
    gs.set("push_empire_modifier", push_empire_mod)?;

    // ... push_system_modifier / push_colony_modifier / push_fleet_modifier
    //     / push_ship_modifier

    // ctx.gamestate:set_flag(scope_kind, scope_id, flag_name, value) -> nil
    let set_flag = s.create_function_mut(
        move |_lua, (_this, scope_kind, id, flag, value): (Table, String, u64, String, bool)| {
            let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
            let entity = Entity::from_bits(id);
            apply::set_flag(&mut *borrow, &scope_kind, entity, &flag, value)
        },
    )?;
    gs.set("set_flag", set_flag)?;

    Ok(gs)
}
```

`views::build_empire_view` は `&World` + `&Lua` → `Table` の pure builder (read helper)。`apply::push_empire_modifier` は `&mut World` のみ受ける (write helper、Lua 不接触)。

### 2.3 `RefCell<&mut World>` 共有 pattern

- `world_cell: RefCell<&mut World>` (lifetime `'a` は scope 呼出期間)
- read closures: `world_cell.try_borrow() -> Result<Ref<&mut World>, BorrowError>`
- write closures: `world_cell.try_borrow_mut() -> Result<RefMut<&mut World>, BorrowMutError>`
- `&*borrow` で `&World` を引き出し、`&mut *borrow` で `&mut World` を引き出す

**lifetime 衝突の懸念**: `Scope<'scope, 'a>` の `create_function` は `FnMut + 'scope` を要求。`move` closure で `&'a RefCell` を capture するため `'a: 'scope` 必要 → `dispatch_with_gamestate` が `world` を `&'a mut World` で受ければ自然に成立。

**`'static` 問題**: Closure 内で `Entity` / `String` / `bool` などは primitive / owned、`world_cell` のみ shared reference 経由。mlua 0.11 の `create_function` / `create_function_mut` は scope closure の場合 non-`'static` bound を許す。

### 2.4 `try_borrow*` 失敗の error 変換

```rust
fn map_reentrancy_err<E: std::fmt::Display>(e: E) -> mlua::Error {
    mlua::Error::RuntimeError(format!(
        "gamestate reentrancy guard: {e}; likely cause: a read helper \
         was invoked from inside a write callback, or a write helper \
         ran while another mutation was still borrowed. fire_event \
         must always go through queue, never sync-dispatch."
    ))
}
```

- panic 禁止 (invariant)
- error message は modder diagnostic として具体的に: "reentrancy" という単語を含め、原因候補 2 つ (read-from-write / write-from-write) を明示、fire_event queue-only 規律への link

---

## §3 Lua 側 API shape (migration 後)

### 3.1 Read methods

| 新 API | 旧 API (廃止) | 戻り値 |
|---|---|---|
| `ctx.gamestate:empire(id)` | `gs.empires[id]`, `gs.player_empire` | EmpireView table (live) |
| `ctx.gamestate:player_empire()` | `gs.player_empire` (shortcut) | EmpireView table、player empire 固定 |
| `ctx.gamestate:system(id)` | `gs.systems[id]` | SystemView table |
| `ctx.gamestate:planet(id)` | `gs.planets[id]` | PlanetView table |
| `ctx.gamestate:colony(id)` | `gs.colonies[id]` | ColonyView table |
| `ctx.gamestate:fleet(id)` | `gs.fleets[id]` | FleetView table |
| `ctx.gamestate:ship(id)` | `gs.ships[id]` | ShipView table |
| `ctx.gamestate:list_empires()` | `gs.empire_ids` | `{u64}` array |
| `ctx.gamestate:list_systems()` | `gs.system_ids` | `{u64}` array |
| `ctx.gamestate:list_planets([system_id])` | `gs.planet_ids` / `system.planet_ids` | filter 可の array |
| `ctx.gamestate:list_colonies([system_id or empire_id])` | `gs.colony_ids` / `system.colony_ids` / `empire.colony_ids` | filter 可の array |
| `ctx.gamestate:list_fleets([empire_id])` | `gs.fleet_ids` | filter 可の array |
| `ctx.gamestate:list_ships([fleet_id or empire_id])` | `gs.ship_ids` / `fleet.ship_ids` / `fleet.members` | filter 可の array |
| `ctx.gamestate.clock` | `gs.clock` | sealed table (snapshot ok、pure scalar) |

**View table の内部 shape** は #289 現行 view 型と同じ (EmpireView / SystemView / ... 参照 §1.3)。ただし以下の違い:

- **snapshot ではなく live**: `ctx.gamestate:system(id)` を呼ぶたびに build (毎回 world から引く)
- **nested navigation は再度 method 呼出**: 旧 `sys.planet_ids[1]` → 新 `for _, pid in ipairs(ctx.gamestate:list_planets(sid)) do ctx.gamestate:planet(pid) end`
- **seal_table は不要**: table の再利用がないため、`__newindex` read-only guard は optional。ただし legacy script の保護互換のため `seal_table` は keep (初期実装では付けておく)

**廃止される shape**:

- `gs.systems[id]` / `gs.empires[id]` / ... のような 2-level map — 代わりに method 呼出
- `sys.planet_ids` / `sys.colony_ids` のような nested array — 代わりに `list_*` method
- `empire.techs` / `empire.flags` の set-like sealed table — 代わりに `ctx.gamestate:list_techs(empire_id)` 等 (後述)

### 3.2 Write methods (setter)

| 新 API | 意味 | `&mut World` への影響 |
|---|---|---|
| `ctx.gamestate:push_empire_modifier(empire_id, target, opts)` | Empire 単位の modifier 追加 | `EmpireModifiers.<target>.push_modifier(...)` |
| `ctx.gamestate:push_system_modifier(system_id, target, opts)` | System 単位 | `SystemModifiers.<target>.push_modifier(...)` |
| `ctx.gamestate:push_colony_modifier(colony_id, target, opts)` | Colony 単位 (Production 等) | `Production.<target>.push_modifier(...)` |
| `ctx.gamestate:push_fleet_modifier(fleet_id, target, opts)` | Fleet 単位 (γ-2 #287 後) | `FleetModifiers.<target>.push_modifier(...)` |
| `ctx.gamestate:push_ship_modifier(ship_id, target, opts)` | Ship 単位 | `ShipModifiers.<target>.push_modifier(...)` (`ship/modifiers.rs:72-82`) |
| `ctx.gamestate:set_flag(scope_kind, scope_id, flag_name, value)` | Flag 設定 | `GameFlags.set(...)` / `ScopedFlags.set(...)` |
| `ctx.gamestate:clear_flag(scope_kind, scope_id, flag_name)` | Flag 解除 | 同上 |

`scope_kind` は string enum: `"empire" | "system" | "planet" | "ship" | "fleet"`。未対応 scope は `mlua::Error::RuntimeError` を返す。

`opts` Lua table の shape (既存 `push_modifier` と同じ):
```lua
{
    add = 1.0,          -- optional, flat base_add
    multiplier = 0.15,  -- optional, multiplicative
    offset = 2.0,       -- optional, flat offset (final)
    description = "...",-- optional, UI
    expires_at = nil,   -- optional, hexadies
}
```

### 3.3 fire_event queue-only 規律

- `fire_event(event_id, target?)` は `globals.rs:550` の関数のまま、**`_pending_script_events` push** 動作を維持
- sync dispatch は **存在しない** (Rust 側に sync dispatch 経路を作らないことで構造的に保証)
- modder が sync dispatch を試みる手段は物理的に無い (現行同様)

### 3.4 使用例

```lua
-- migration 後の事例
on("colony_lost", function(evt)
    local empire = evt.gamestate:player_empire()
    print(empire.name, empire.resources.minerals)

    -- 同 callback 内で mutation して、直後に効果を観測
    evt.gamestate:push_empire_modifier(
        empire.id,
        "production.energy",
        { multiplier = 1.2, description = "Wartime mobilization" }
    )

    -- 再度 read: 即反映 (live within tick)
    local e2 = evt.gamestate:empire(empire.id)
    -- e2 は別 table だが内容は最新

    -- 複雑系: ship state に応じた fleet 編成
    for _, fid in ipairs(evt.gamestate:list_fleets()) do
        local fleet = evt.gamestate:fleet(fid)
        if fleet.state and fleet.state.kind == "in_ftl" then
            evt.gamestate:push_fleet_modifier(fid, "ship.ftl_speed",
                { multiplier = 0.1 })
        end
    end

    -- 別 event を chain する場合は queue
    fire_event("wartime_declared")  -- _pending_script_events に push
end)
```

---

## §4 Rust 側 helper の非対称性 (invariant)

`memory/feedback_rust_no_lua_callback.md` に規定された invariant を本 pivot で具現化:

### 4.1 Read helper: `views::build_*_view`

- **Signature**: `fn build_empire_view(lua: &Lua, world: &World, entity: Entity) -> mlua::Result<Table>`
- **許可**: `lua.create_table()` / `table.set(key, value)` / `world.get::<C>(e)` / `world.query::<...>()` (read-only query)
- **禁止**:
  - `Function::call` / `lua.load().exec()` / Lua code 実行系全般
  - `lua.scope` 内に nested scope を作る (#263 の child UserData 制約と類似の罠)
  - Rust 側 event bus の sync fire (`EventSystem::fire_event` を直呼び)
- **推奨**: builder 関数は `views` submodule に分離、unit test 可能

### 4.2 Write helper: `apply::*`

- **Signature**: `fn push_empire_modifier(world: &mut World, entity: Entity, target: &str, opts: ModifierOpts) -> mlua::Result<()>`
- `opts` は **plain Rust struct** (`ModifierOpts { add: Option<SignedAmt>, multiplier: ..., ... }`); Lua table を persist しない
- **禁止**:
  - `&Lua` 引数を受けない (受けても `_lua` と prefix で unused)
  - `mlua::Function` / `mlua::RegistryKey` / `mlua::Value` の保持・呼出
  - World 内に保存した Lua callback の sync invoke (= Rust-side stored Lua RegistryKey を呼ぶ path。存在してはならない)
  - Rust-side event bus の sync fire — **queue 経由のみ**
- **推奨**: `apply` submodule に集約、`apply::push_*_modifier` は `#[cfg(test)]` で World fixture で unit test

### 4.3 設計 review の check list

PR review 時に以下を確認:

1. `create_function_mut` の closure body 内で `_lua` または `lua` 変数が使われていないか grep
2. `apply::*` の signature が `&mut World` のみ (plus primitive / owned data) か
3. `views::build_*_view` 内で `Function::call` / `lua.load` / `engine.fire_event` 等の呼出がないか
4. world 内の Lua callback 保存経路 (例: `EventSystem.LuaFunctionRef.RegistryKey`) が write helper から触られていないか
5. `fire_event` の唯一の実装が `_pending_script_events` push であり続けているか (sync 実装が cmd line で追加されていないか)

---

## §5 Hook 拡張 scope (2026-04-15 追記分対応)

### 5.1 現状のカバレッジ (snapshot 方式)

| hook | gamestate 注入 | notes |
|---|---|---|
| event callback (`on(...)`) | ✓ | `dispatch_event_handlers:437` |
| event `on_trigger` (`define_event { on_trigger = fn }`) | ✓ | 同 dispatch |
| fire_condition (MTTH / Periodic) | ✓ | `evaluate_fire_conditions:297` |
| lifecycle (`on_game_start` / `on_game_load` / `on_scripts_loaded`) | ✗ | `run_handlers` (lifecycle.rs:27-34) は引数 `()` で呼ぶだけ |
| tech `on_researched` | ✗ (EffectScope 経由) | `technology/effects.rs:286` `func.call(scope)` |
| faction action callbacks | ✗ (EffectScope 経由) | `faction/mod.rs` 付近 |

### 5.2 本 pivot で live 化する hook (Phase A)

**event callback 系のみ** を Phase A で live 化:
- `on(event_id, fn)` bus handler
- `on_trigger` on event definition
- fire_condition (MTTH / Periodic)

これは現状 snapshot 経由で既に `evt.gamestate` が注入されている hook 群。本 pivot の scope closure へ置き換える。

### 5.3 Phase B で live 化する hook

**live World mutation を目的とする hook** は Phase B で拡張:
- `on_game_start` / `on_game_load`: 初期データ seeding 系 (World を直接いじる用途)
- `on_scripts_loaded`: 静的検証のみ → **live 化しない** (scope 外 context、_pending_* path 維持)

**effect declaration が目的の hook** は scope 外 context として **EffectScope 維持**:
- tech `on_researched`: `EffectScope` + `scope:push_modifier(...)` descriptor 収集後に一括適用
- faction action callbacks: 同上

### 5.4 切替判断基準 (plan doc の判定ルール)

> hook が **World mutation 目的**なら live 化 (scope closure path)、**declarative effect 宣言目的**なら `_pending_*` / EffectScope 維持

例外:
- tech `on_researched` は effect 宣言系 → EffectScope 維持
- 但し、将来「特定 tech が即座に world を読む必要」が出た場合、同 hook に両方を注入する設計余地は残す (Phase C 以降、本 pivot の blocker ではない)

### 5.5 Phase B の実装方針

`run_on_game_start` / `run_on_game_load` (lifecycle.rs:13-25) の signature を変更:

```rust
pub fn run_on_game_start_with_gamestate(lua: &Lua, world: &mut World) -> Result<(), mlua::Error> {
    let handlers: mlua::Table = lua.globals().get("_on_game_start_handlers")?;
    for i in 1..=handlers.len()? {
        let func: mlua::Function = handlers.get(i)?;
        // Build ephemeral payload for lifecycle hooks (only gamestate field).
        let payload = lua.create_table()?;
        dispatch_with_gamestate(lua, world, &payload, |_lua, p| {
            func.call::<()>(p.clone())
        })?;
    }
    Ok(())
}
```

`run_lifecycle_hooks` Bevy system は exclusive (`&mut World`) に昇格必須 (現状 `Res<ScriptEngine>` + `Query`)。

---

## §6 FleetState 直接参照への切替 (#287 γ-2 との関係)

### 6.1 現状 (flagship proxy)

`build_fleet_view` (snapshot 版、gamestate_view.rs:404-505) は:
- `fleet.state` を flagship ShipState から proxy (L454-495)
- `fleet.owner_empire_id` を flagship Ship.owner から proxy (L442-450)

これは #287 γ-2 (FleetState 独立 component) が未 land のため。

### 6.2 γ-2 land 前の本 pivot

`views::build_fleet_view` は flagship proxy を **そのまま踏襲**。差し替えは γ-2 land 後の follow-up commit で:
- `FleetState` component が出現したら `world.get::<FleetState>(entity)` に切替
- flagship fallback path は削除 (fleet に状態が乗るため)

### 6.3 γ-2 land 後の本 pivot (もし順序が逆転した場合)

`views::build_fleet_view` を最初から FleetState 直接参照で書く。flagship proxy コードは不要。本 pivot PR の書き換えコストが減る方向。

### 6.4 判断

**本 pivot 実装時点の #287 γ-2 状態に合わせる**。両 path の切替は `views::build_fleet_view` 内部の分岐一箇所に閉じる (50 行程度の if-else) ため、どちらが先でも rebase 容易。

---

## §7 Commit 段取り (10 commit 想定)

user 指示の順序を踏襲。各 commit は `cargo test` green を維持。

### Commit 1: scope 基盤 + empire read closure 最小構成
- **新規**: `macrocosmo/src/scripting/gamestate_scope.rs`
  - `dispatch_with_gamestate<'a>(lua, world, payload, callback) -> mlua::Result<()>`
  - `map_reentrancy_err` helper
  - `views::build_clock_table` (pure、静的 scalar)
  - `views::build_empire_view` (read helper、snapshot 版 `build_empire_table` 流用)
  - scope 内で `empire` / `player_empire` / `list_empires` / `clock` だけ expose
- **変更**: `lifecycle.rs:dispatch_event_handlers` を新 API に置換
  - `attach_gamestate` 呼出を削除、代わりに `dispatch_with_gamestate` で包む
  - bus handler / on_trigger の `func.call` を callback 内に移す
- **test**: `test_dispatch_attaches_gamestate_to_bus_handler` / `test_dispatch_invokes_on_trigger_with_gamestate` を新 API で書き換え、**両 test green**
- **残存**: 他の read (system/colony/fleet/ship/planet) は旧 snapshot path が残る状態 (一時共存)
- **推定 LoC**: +400 / -50 (新規 helper + test 書き換え)

### Commit 2: 残 entity の read helper 完成
- **追加**: `views::build_system_view`, `build_planet_view`, `build_colony_view`, `build_fleet_view`, `build_ship_view`, `build_ship_state_table`
- **追加**: `list_systems` / `list_planets(system_id?)` / `list_colonies(scope_id?)` / `list_fleets(empire_id?)` / `list_ships(fleet_id?)` closures
- **削除**: `scripting/gamestate_view.rs` の旧 `build_gamestate_table` / `attach_gamestate`
- **変更**: `evaluate_fire_conditions` も新 API へ (fire_condition callback を `dispatch_with_gamestate` で包む)
- **test**: `test_fire_condition_suppresses_periodic_event` ほか fire_condition 系 3 本を書き換え
- **推定 LoC**: +600 / -800 (old snapshot build を大幅削除)

### Commit 3: setter (empire 層)
- **新規**: `macrocosmo/src/scripting/gamestate_apply.rs`
  - `apply::push_empire_modifier(&mut World, Entity, &str, ModifierOpts) -> mlua::Result<()>`
  - `apply::ModifierOpts` struct + `parse_modifier_opts(&Table) -> mlua::Result<ModifierOpts>`
- **追加**: scope 内 `push_empire_modifier` closure (create_function_mut)
- **test**: 新規 `tests/lua_gamestate_mutations.rs`
  - `test_push_empire_modifier_applies_live` — modifier 追加後の同 callback 内 read で反映確認
  - `test_push_empire_modifier_reentrancy_error` — write 内で read closure を呼ぶ → `RuntimeError`
- **推定 LoC**: +250 / -10

### Commit 4: setter (system / colony 層)
- **追加**: `apply::push_system_modifier` (`galaxy/generation.rs:100-102` 参照、SystemModifiers の push_modifier)
- **追加**: `apply::push_colony_modifier` (Production の push_modifier)
- **追加**: scope 内 closure
- **test**: 同 test file に 2 本追加
- **推定 LoC**: +200 / -5

### Commit 5: setter (fleet / ship 層)
- **追加**: `apply::push_ship_modifier` (`ship/modifiers.rs:72-82` の既存 ShipModifiers 関数を invoke)
- **追加**: `apply::push_fleet_modifier` (γ-2 未 land なら skip、or flagship ship に pass-through する暫定実装)
- **追加**: scope 内 closure
- **test**: 同 test file に 2 本追加
- **推定 LoC**: +200 / -5

### Commit 6: flag setter 実装
- **追加**: `apply::set_flag(&mut World, scope_kind: &str, Entity, flag: &str, value: bool)` / `apply::clear_flag`
- **追加**: scope 内 `set_flag` / `clear_flag` closure
- **新規 test**: `test_set_flag_empire_live`, `test_set_flag_unknown_scope_errors`
- **推定 LoC**: +150 / -0

### Commit 7: event callback scripts の migration
- **変更**: `macrocosmo/scripts/events/*.lua` で `evt.gamestate.*` を使用している箇所を新 API へ
  - 現状 `scripts/events/sample.lua` は gamestate を触っていない → migration 不要
  - ただし実行される user script / test script があるか再度 grep (`gamestate\.\|gamestate:`) で確認し、あれば書き換え
- **変更**: `macrocosmo/scripts/lifecycle/init.lua` — 現状 `fire_event("monthly_report")` のみ、gamestate 不使用 → migration 不要
- **確認**: `cargo test` で全 script がロード成功、既存 integration test pass
- **推定 LoC**: ~0 (migrate 対象が薄い想定、実測次第で +20 程度)

### Commit 8: test migration (`lua_view_types.rs` + 関連)
- **変更**: `macrocosmo/tests/lua_view_types.rs` 3 test を新 API で再実装
  - `build_gamestate_table(engine.lua(), &mut world)` 直叩き → `dispatch_with_gamestate` helper 経由
  - test helper: `fn with_gamestate<F>(world, assertion: F) where F: FnOnce(&Table)` 的なもの
- **変更**: `macrocosmo/src/scripting/gamestate_view.rs` の unit tests ~20 本 → `gamestate_scope.rs` に移す (旧ファイル削除が望ましい)
- **変更**: `lifecycle.rs` mod tests の gamestate 関連 test を新 API へ
  - `test_dispatch_gamestate_mutation_inside_handler_fails_gracefully` は **意味論反転**: 旧 "mutation は read-only error" → 新 "mutation は live に反映、reentrancy のみ error"
  - 代わりに `test_dispatch_gamestate_reentrancy_error` を新設
- **推定 LoC**: +500 / -600 (net -100、ファイル再編)

### Commit 9: global `set_flag` / `modify_global` 廃止 + Lua script migration
- **user 決定 (2026-04-15)**: global 関数を**廃止**、`ctx.gamestate:set_flag` / `:push_modifier` のみに一本化
- **変更**:
  - `globals.rs` の `set_flag(name)` / `modify_global(...)` global 関数を**削除** (export しない)
  - `_pending_flags` / `_pending_global_mods` の drain は `EffectScope` descriptor 経路用に維持 (lifecycle / tech / faction hook)
  - Lua script 全 scan: global 呼出箇所を `ctx.gamestate:set_flag(scope, id, name, value)` / `:push_modifier(...)` へ migrate (event callback 内であることを前提)
  - event callback の外で global を呼んでいる script があれば **error 起票**して script 側で `EffectScope` へ切替
- **確認**: `_pending_flags` の drain は lifecycle / tech / faction hook 経由のみで発生、event callback 内からは発生しないこと (既存の実態と一致、Appendix A-2 参照)
- **推定 LoC**: +40 / -30 (Lua script migration 中心、Rust 側は global 関数 2 本削除のみ)

### Commit 10: `gc_collect` 撤回 + #320 stress regression 検証
- **変更**: `lifecycle.rs:337` と `lifecycle.rs:461` の `lua.gc_collect()` 削除
- **user 決定 (2026-04-15)**: `stress_lua_scheduling.rs` の `LUA_MEMORY_CEILING_BYTES` は**現状維持 (32 MiB)**、実測で問題出たら follow-up
- **確認**: 1000 tick stress test が `gc_collect` 削除後も既存 ceiling で pass
- **推定 LoC**: +0 / -10

### Commit 11: Hook 拡張 (Phase B、lifecycle)
- **変更**: `run_lifecycle_hooks` (lifecycle.rs:69-104) を exclusive system (`&mut World`) に昇格
- **変更**: `run_on_game_start` / `run_on_game_load` の signature に `&mut World` 追加、`dispatch_with_gamestate` で包む
- **新規 test**: `test_on_game_start_can_mutate_via_gamestate`
- **推定 LoC**: +100 / -30

### Commit 12: docs 更新
- `docs/architecture-decisions.md` §10 を Option B 確定記述に書き直し
- `memory/project_lua_gamestate_api.md` — 既に update 済だが、実装 completion を追記
- `CLAUDE.md` — 「New game elements must be Lua-defined」後付近に gamestate write API の記載
- `docs/plan-263-lua-gamestate.md` に pivot completion note
- `docs/plan-320-mlua-aux-stack-leak.md` に `gc_collect` 撤回 note
- **close 候補**: #328 (既に close)、#332 (本 issue)
- **推定 LoC**: +200 / -50 (docs のみ)

### Commit 集計

- 総 commit 数: **12**
- 総推定 LoC: **+2,635 / -1,568 (net +1,067)**
- main 影響 ファイル数: 推定 15-20 (lifecycle.rs / gamestate_view.rs 削除 / gamestate_scope.rs 新規 / gamestate_apply.rs 新規 / lua_view_types.rs 再編 / stress_lua_scheduling.rs 更新 / globals.rs / events.rs / 各 tests / docs 5 本)

---

## §8 test migration 計画

### 8.1 既存 `lua_view_types.rs` hierarchical navigation の書き換え例

**旧**:
```rust
let gs = build_gamestate_table(engine.lua(), &mut world).unwrap();
lua.load(r#"
    local sys = gs.systems[system_id]
    for _, pid in ipairs(sys.planet_ids) do
        local p = gs.planets[pid]
        ...
    end
"#).exec().unwrap();
```

**新**:
```rust
let mut world = scenario_world();
let lua = {
    let engine = world.resource::<ScriptEngine>();
    engine.lua().clone()  // or use &engine.lua() directly
};

let payload = lua.create_table().unwrap();
dispatch_with_gamestate(&lua, &mut world, &payload, |lua, p| {
    lua.globals().set("_evt", p.clone())?;
    lua.load(r#"
        local sys = _evt.gamestate:system(system_id)
        for _, pid in ipairs(_evt.gamestate:list_planets(system_id)) do
            local p = _evt.gamestate:planet(pid)
            ...
        end
    "#).exec()
}).unwrap();
```

test helper `with_gamestate_scope(world, |lua, payload| { ... })` を tests/common に追加して boilerplate 削減。

### 8.2 `stress_lua_scheduling.rs` の aux stack peak 予測

- **旧 (snapshot)**: 1000 tick × 2 build/tick × ~100 ref/build ≈ 200K ref、`gc_collect` で毎 tick 回収 → final_memory ~4 MiB (gc_collect オーバーヘッド込み、ceiling 32 MiB)
- **新 (scope closure)**:
  - scope 終了時に mlua が closure 自動片付け
  - closure そのものは Lua registry を使わず stack 上だけで完結 (`Lua::scope` の本来動作)
  - 1 tick あたり: `dispatch_with_gamestate` × 2 回呼出、内部で `create_function` × N (closure 数、約 15-20)
  - closure 作成は Lua function object 生成 → scope 終了で destroy
  - **予測 final_memory**: baseline + 0-500KB 程度 (Lua parser cache や handler table 等の通常増分)
- **確認方法**: Commit 10 で `LUA_MEMORY_CEILING_BYTES` を 8 MiB に引き下げて CI pass 確認、手元で実測後 4 MiB 等に再調整

**リスク**: `create_function` 呼出が per-tick × 20 ≈ 20K 回、mlua が内部 Lua function object を毎回生成 → LuaJIT function hash table に pressure。ただし scope 終了で解放されるはずで蓄積はしない想定。**実測必須** (Commit 10 で要観測)。

### 8.3 書き込み系 test の新設

`tests/lua_gamestate_mutations.rs` (Commit 3 以降で漸次拡張):

1. `test_push_empire_modifier_applies_live` — 同 callback 内 read で反映
2. `test_push_empire_modifier_persists_across_ticks` — 次 tick の gamestate にも反映
3. `test_push_empire_modifier_reentrancy_error` — write 中に read closure 呼出 → error 捕捉 (Lua `pcall` で)
4. `test_set_flag_empire_live` — flag 設定後の GameFlags component 直接確認
5. `test_fire_event_queued_not_sync` — write callback 内で `fire_event("foo")`、その場では発火せず queue に積まれる
6. `test_multiple_writes_in_one_callback` — 複数 setter 連続呼出で最終結果反映
7. `test_write_unknown_target_errors` — `push_empire_modifier(id, "bogus.target", ...)` → `RuntimeError`
8. `test_write_unknown_scope_kind_errors` — `set_flag("not_a_scope", ...)` → `RuntimeError`

### 8.4 reentrancy test の設計難度

RefCell の `try_borrow_mut` は **同一 threadから** の再入を検知。Lua scope closure は 単一 thread で実行される (mlua の runtime は Send でない) ため、naive な reentrancy は確実に検知可能。

**捕捉パターン**:
```lua
-- 書き込み closure 内で setter が read closure を呼べないことを確認
ctx.gamestate:push_empire_modifier(id, "x", {
    description = ctx.gamestate:empire(id).name  -- これは Rust 側で RefCell 競合
})
```

mlua の呼出順序: `push_empire_modifier` (write closure 入る) → opts 評価 → `ctx.gamestate:empire(id)` (read closure 入る) → `try_borrow` 失敗 → `RuntimeError`。Lua `pcall` で捕捉し assertion。

**ただし**: `opts` が Lua side で事前評価されてから Rust 側 `push_empire_modifier` が呼ばれる場合、reentrancy が発生しない。
→ 実際には `table value = ctx.gamestate:empire(id)` を外側で評価してから setter に渡す形になる → reentrancy は起きない。**reentrancy が本当に起きるのは Rust write closure 内から Rust read closure を call する経路** のみ。

**結論**: write helper が Lua 不接触という invariant (§4) を守る限り、reentrancy error は起きない。**invariant 違反の検知用 test として test_write_helper_does_not_invoke_lua を `#[should_panic]` で用意**。

---

## §9 リスク表

| # | リスク | 影響 | 対策 |
|---|---|---|---|
| R1 | `RefCell` borrow 衝突時の Lua 側 error message の追いやすさ | modder DX 悪化 | `map_reentrancy_err` で具体的な診断メッセージ (§2.4)、原因候補 2 つを明示 |
| R2 | mlua 0.11 `create_function_mut` の `FnMut` bound が `&'a RefCell` capture で解けない | コンパイルエラー | spike prototype 必須 (Commit 1 の最初の step)、ダメなら `Rc<RefCell>` に格上げ検討 |
| R3 | closure 毎 tick × 20 回 create によるLuaJIT function heap pressure | 性能劣化 | §8.2 実測。問題なら closure を scope 外の `RegistryKey` で保持し、scope 内は binding だけ配る案に変更 |
| R4 | 既存 event callback script が想定外に多く、migration が爆発 | 工数超過 | §1.6 列挙済 (test 10 本 + script 0 本想定)。user script はまだ無いため OK |
| R5 | #328 close の timing | doc unsynced | Commit 12 で #328 を #332 superseded として close、cross-link |
| R6 | #320 stress test が pivot 後 pass しない (function heap leak) | release blocker | Commit 10 で `gc_collect` 削除前後両方で CI run、regression 検出時は `gc_collect` を optional に残す選択肢保持 |
| R7 | hook 拡張時の scope 内 / scope 外 区別の実装難度 | Phase B 遅延 | §5.4 判定基準、EffectScope 維持で最小影響 |
| R8 | reentrancy 保護が skip されるケース (Lua が Rust closure を保持) | silent data corruption | `Lua::scope` 終了で closure invalidate が mlua の invariant。回避されるケースは `table.remove` 等で closure reference を Lua 側から strip → それを scope 外で呼ぶ試み。**閉じられた closure の call は mlua 0.11 で "attempt to call an invalid Lua scope value" error** (要 spike 確認) |
| R9 | user が `ctx.gamestate:push_*_modifier` を setter 外 (例: fire_condition 内) で呼ぶ | fire_condition が副作用を持つ | fire_condition は spec 上 pure が期待されるが、本 pivot では technical には可能。docs で明示的に禁止、violation は stress test で検出 |
| R10 | `&mut World` を scope に持ち込むため、exclusive system 化が波及 | system ordering 再調整 | `dispatch_event_handlers` / `evaluate_fire_conditions` は既に exclusive、`run_lifecycle_hooks` は Phase B で昇格 |
| R11 | `#333 (#296 PR)` との rebase conflict | merge 衝突 | event_system.rs / lifecycle.rs 周辺のみ、手動解消可 |
| R12 | `attach_gamestate` の削除で外部 crate (macrocosmo-ai) が break | workspace 全体コンパイル失敗 | grep 済 (`scripting/gamestate_view.rs` / `lifecycle.rs` のみ参照、外部 crate からは参照なし)。macrocosmo-ai は mock feature で dev-dependency、本 API 未使用 |
| R13 | bevy の `World::resource_scope` 中の borrow 構造が変更 (Bevy 0.18 → 将来) | future-proof 問題 | 本 pivot scope 外 |

---

## §10 Out of scope / follow-up

本 pivot で **含めない** 項目:

- **#335 PlanetView.biome placeholder 解消** — Biome component + `define_biome` Lua API (#289 残分、本 pivot は view method 形に乗せ替えるだけ)
- **#336 Colony.Owner component 導入** — Sovereignty chain 間接解消 (#289 残分、同上)
- **光速遅延の Lua API expose (`node:perspective(viewer)`)** — #215 延長、KnowledgeStore delay view
- **unsafe raw pointer 路線 (Option 3)** — 却下、本 pivot で不要化
- **#334 command dispatch refactor** — 本 pivot と orthogonal、Phase 4 で統合検討
- **tech `on_researched` / faction callbacks の live 化** — scope 外 context、EffectScope 維持 (Phase C 以降で検討)
- **mtth/periodic の非 fire_condition callback (= on_trigger) の per-event gamestate cache** — Phase A で build 毎回なら性能問題になる可能性あるが、実測で問題なければ放置 (#328 は本 pivot で obsolete 化するが、もし再燃したら新 issue)
- **read closure の lazy table building** — 現状 `build_empire_view` 等は呼ばれた瞬間に全 field を populate。将来的には `__index` で lazy fetch も可能だが、本 pivot scope 外
- **`ctx.gamestate:iter_empires()` (iterator protocol)** — Lua 側で `list_empires` → `ipairs` で足りる
- **script reload 時の closure invalidation** — `ScriptEngine::new()` で Lua ごと再生成する現状動作で自動回収、特別対応不要
- **UI / save 系**: save/load fixture は gamestate scope を含まない (Lua state は save 対象外)

---

## §11 Critical Files for Implementation

### 既存 (変更対象)

| file | 変更内容 |
|---|---|
| `macrocosmo/src/scripting/mod.rs` | `pub mod gamestate_scope;` / `pub mod gamestate_apply;` 追加、`pub mod gamestate_view;` 削除 |
| `macrocosmo/src/scripting/gamestate_view.rs` | **削除** (内容は gamestate_scope.rs + views submodule に移植) |
| `macrocosmo/src/scripting/lifecycle.rs` | `evaluate_fire_conditions` / `dispatch_event_handlers` を scope API で書き直し、`gc_collect()` 削除、`run_on_game_start/load` を `&mut World` 版に (Phase B) |
| `macrocosmo/src/event_system.rs` | LuaFunctionRef 周辺は据え置き、`EventBus::fire` の内部 API が scope closure 経由で呼ばれる経路に合わせて小改修 |
| `macrocosmo/src/scripting/globals.rs` | `set_flag` / `modify_global` は維持 (lifecycle / tech path)、doc コメントで「event callback 内では `ctx.gamestate:*` 推奨」記載 |
| `macrocosmo/src/scripting/effect_scope.rs` | 変更なし (EffectScope 維持) |
| `macrocosmo/tests/lua_view_types.rs` | 3 test を新 API で書き直し |
| `macrocosmo/tests/stress_lua_scheduling.rs` | `LUA_MEMORY_CEILING_BYTES` 引き下げ、assertion tighten |
| `macrocosmo/scripts/events/sample.lua` | gamestate 不使用、変更なし想定 |
| `macrocosmo/scripts/lifecycle/init.lua` | Phase B 対象、必要なら `ctx.gamestate:*` に書き換え |

### 新規

| file | 内容 |
|---|---|
| `macrocosmo/src/scripting/gamestate_scope.rs` | `dispatch_with_gamestate`, `build_scoped_gamestate`, `map_reentrancy_err`、`views` submodule |
| `macrocosmo/src/scripting/gamestate_scope/views.rs` | `build_*_view` pure read helpers、`list_*` helpers、`build_ship_state_table`、`build_clock_table` |
| `macrocosmo/src/scripting/gamestate_apply.rs` | `ModifierOpts`, `parse_modifier_opts`, `apply::push_*_modifier`, `apply::set_flag/clear_flag` |
| `macrocosmo/tests/lua_gamestate_mutations.rs` | §8.3 列挙の 8 test |
| `macrocosmo/tests/common/gamestate.rs` (既存 common module 下、新規 sub) | `with_gamestate_scope` test helper |

### Docs

| file | 変更内容 |
|---|---|
| `docs/plan-332-gamestate-scoped-closures.md` | 本文書 |
| `docs/architecture-decisions.md` §10 | Option B 確定、history 整理 |
| `docs/plan-263-lua-gamestate.md` | pivot completion note |
| `docs/plan-289-lua-view-types.md` | view 型が scope closure 上に乗った旨 |
| `docs/plan-320-mlua-aux-stack-leak.md` | `gc_collect` 撤回 note |
| `CLAUDE.md` | gamestate API の記載 (必要なら) |

---

## Open questions — user 確定 (2026-04-15)

1. **global `set_flag` / `modify_global` の扱い** → **gamestate に一本化**
   - global `set_flag(name)` / `modify_global(...)` 関数は **廃止**、`ctx.gamestate:set_flag(scope, id, name, value)` / `:push_modifier(...)` のみ提供
   - migration: 既存 Lua script 中の global 呼出を `ctx.gamestate:set_flag(...)` 等へ書き換え
   - `EffectScope` descriptor 経由の `scope:set_flag` は scope 外 context 用として維持 (§5 参照)
   - → **Commit 9 を "global 関数廃止 + migration" に scope 拡大**

2. **fire_condition 内 setter 使用** → **(b) API レベル read-only mode**
   - `dispatch_with_gamestate(..., mode: GamestateMode::{ReadOnly, ReadWrite})` を引数に追加
   - `evaluate_fire_conditions` は `ReadOnly` で呼び、write closure を expose しない
   - `dispatch_event_handlers` / lifecycle hook は `ReadWrite`
   - 理由: fire_condition が副作用持つと評価順序依存で debugging が壊滅的
   - → §2 API skeleton に `mode` 引数追加、Commit 1 の `build_scoped_gamestate` で分岐

3. **#287 γ-2 FleetState land timing** → **pivot 先行**
   - #287 γ-2 は本 pivot 後 に land させる。現行 flagship proxy のまま Commit 2 で移植、γ-2 land 時に別 PR で FleetState 直接参照へ切替
   - → §6 を "flagship proxy 維持、γ-2 後に follow-up PR で切替" に確定

4. **visibility contract 明記先** → **`architecture-decisions.md` §10 のみ**
   - CLAUDE.md には記載しない (scripting section は既に肥大化、一次 source は architecture-decisions に集約)
   - → Commit 12 の docs 更新で architecture-decisions.md §10 のみ拡張

5. **stress_lua_scheduling 厳格化** → **現状維持 (32 MiB ceiling)**
   - 厳格化は放置、実測で問題が出たら follow-up
   - → Commit 10 は `gc_collect` 撤回 + 既存 ceiling で pass 確認のみ

## Spike 結果 (2026-04-15、impl 前の事前検証)

`macrocosmo/tests/spike_mlua_scope.rs` で Option B の前提を実証 (4/4 pass):

- `lua.scope` + `RefCell<&mut T>` 共有で read/write closure 同居 OK
- 外部変数は scope 終了後に mutation が persist する
- Lua globals 経由で post-scope 呼び出しすると runtime error (panic なし)
- 連続 borrow は衝突せず、意図的 double borrow_mut は clean な error で surface
- `send` feature + luajit + mlua 0.11 で問題なし

→ 本 plan のアーキテクチャ前提は実装前に検証済。spike test は Commit 1 の基盤と一緒に main 入りさせるか、削除するかは実装時に判断 (regression guard として残す価値はある)。

---

## Appendix A: Surprise / 調査で見つけた前提揺らし事実

本 plan 起票前の user 想定との差分:

1. **snapshot build が 1 event あたり 1 回、ではなく "payload ごと 1 回"**
   - `evaluate_fire_conditions:285` は 1 tick 1 回 (pending decisions まとめて共有)
   - `dispatch_event_handlers:437` は fired event ごと (複数 event → 複数 build)
   - → pivot 後の closure create 回数も fired event 依存。stress test は 1 event/tick なので比較容易だが、複数 event 時は要再評価

2. **`_pending_flags` の drain ポイントが 4 箇所**
   - user 想定: lifecycle + event callback 程度
   - 実態: lifecycle / apply_tech_effects / build_tech_effects_preview / faction hook の 4 箇所
   - → Commit 9 の scope 確定に影響。**preview は side-effect 防止の drain で本質的 (live 適用ではない)、event callback path は EffectScope 経由なので実はそもそも `_pending_flags` に直 push はない**
   - → 確認: `scripts/tech/*.lua` の `scope:set_flag` は **EffectScope descriptor** 経由 (effect_scope.rs:81-)、`_pending_flags` には積まない。`_pending_flags` に直 push するのは Lua global `set_flag(name)` のみ
   - → 事実上、本 pivot での `_pending_flags` event-callback-path 廃止は **no-op** (元々 event callback は EffectScope 路線でなく global set_flag も呼んでいない)。docs 上の invariant 再確認のみ

3. **`_pending_global_mods` も同様**
   - `scope:push_modifier` は EffectScope descriptor、`modify_global` は global 関数
   - event callback 内での `modify_global` 直叩きは現状 Lua 側の用例なし
   - → Commit 9 の実務差分は docs-only の可能性高

4. **`Lua::scope` が既存コードで一度も使われていない**
   - grep で 0 件。本 pivot が workspace 初の usage
   - → spike prototype で mlua 0.11 の動作確認 (特に `create_function_mut` + 非 `'static` capture) を先に実施推奨

5. **`seal_table` 内の `__newindex` は `{_t, k, _v}` の 3-tuple**
   - mlua の metamethod signature。pivot 後も read-only gate を残すなら利用可能
   - live view の場合 read-only でなくてもよい (再度 build されるため mutation は次回反映)

6. **`ScriptEngine` は `Lua::new_with` で sandbox 済**
   - io / os / debug / ffi なし、scope closure が Rust 側へ "脱走" する経路は構造的に存在しない (§4 invariant の背景)

7. **`dispatch_event_handlers` は `world.resource_scope::<ScriptEngine, _>` で既に exclusive**
   - scope API 導入で追加の system 昇格は不要。既存 signature のままで本 pivot に乗る
   - lifecycle hook (run_on_game_start) は非 exclusive、Phase B で昇格必要 (§5.5)

8. **既存 `build_gamestate_table` 内で `world.query::<X>()` を何度も呼ぶ**
   - query cache は毎回 new、per-entity の component fetch は `world.get::<C>(entity)`
   - pivot 後 read closure 内でも同じ pattern が使えるが、**closure ごと query 構築はコスト高**
   - → `views::build_empire_view` は `&World` を受けて query を内部で組む。closure 内で毎回呼ぶと per-invocation cost が snapshot より高い可能性
   - → 対策案: scope 開始時に一度 `WorldQueries` 構造体 (cached queries) を作って closure で shared。Phase A で実装せず実測後判断

9. **`Entity::to_bits()` / `Entity::from_bits()` の u64 round-trip が既に全 view で使われている**
   - pivot で `id: u64` → `Entity::from_bits(id)` の pattern をそのまま踏襲
   - Lua 側は u64 (= `i64` or `Integer`) で扱う; mlua 0.11 の `i64 ⇄ u64` は明示 cast が必要な場合あり → spike で要確認

10. **`FleetMembers` は Fleet 本体とは別 component** (`:409` `world.query::<(Entity, &Fleet, &FleetMembers)>()`)
    - `list_ships(fleet_id)` 実装時に Fleet だけでなく FleetMembers も fetch 必要

---

_End of plan._
