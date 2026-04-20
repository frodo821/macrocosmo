# 実装計画書: Issue #289 — β Lua View types (SystemView / ColonyView / FleetView / ShipView / PlanetView / EmpireView)

_Prepared 2026-04-15 by Plan agent._

---

## 1. 既存 `event.gamestate` snapshot のスキーマ (実測)

`build_gamestate_table` (`macrocosmo/src/scripting/gamestate_view.rs:71-317`) が現在 expose している tree を実装から直接起こしたもの。**`seal_table` は shadow-table トリックで read-only 化**、`*_ids` / `fleet.members` / `empire.colony_ids` は `ipairs` のため unsealed (LuaJIT `__pairs` 非対応の回避策)。

### 1.1 ルート (gs)

| key | 型 | 出典 | 備考 |
|---|---|---|---|
| `clock` | sealed table | `gamestate_view.rs:75-90` | `now/year/month/hexady_of_month` |
| `empires` | sealed map `[entity_id_u64]=empire_tbl` | `:94-146` | |
| `empire_ids` | unsealed array | `:95, 137, 147` | `ipairs` で iterate 可 |
| `player_empire` | sealed table | `:96, 139, 148-150` | shortcut |
| `systems` | sealed map | `:152-194` | |
| `system_ids` | unsealed array | `:154, 189, 194` | |
| `ships` | sealed map | `:196-250` | **既に存在** |
| `ship_ids` | unsealed array | `:198, 245, 250` | |
| `fleets` | sealed map | `:252-284` | **既に存在** |
| `fleet_ids` | unsealed array | `:254, 279, 283` | |
| `colonies` | sealed map | `:286-313` | |
| `colony_ids` | unsealed array | `:288, 308, 313` | |

### 1.2 各 entity snapshot の field (現状)

- **`empires[id]`** (`build_empire_table`, `:331-423`): `id/name/is_player/resources{m,e,r,f,a}/techs (set-like) /flags (set-like) /capital_system_id (Phase 1 heuristic)/colony_ids (player のみ全 colony、他は空)`
- **`systems[id]`** (`:170-190`): `id/name/surveyed/is_capital/star_type/resources{m,e,r,f,a}` のみ。**不足: `position` / `planets` / `colonies` / `owner` / `modifiers`**
- **`ships[id]`** (`:225-246`): `id/name/design_id/hull_id/owner_kind/owner_empire_id?/home_port/ftl_range/sublight_speed`。**不足: `fleet` (Ship.fleet 未公開) / `hp` / `modules` / `state`**
- **`fleets[id]`** (`:264-280`): `id/name/flagship/members (array、unsealed)`。**不足: `owner`, `state` (FleetState 未実装 — γ-2 #287 以降)、`origin`, `destination`**
- **`colonies[id]`** (`:296-309`): `id/population/growth_rate/planet_id/system_id?/planet_name?`。**不足: `owner`, `buildings`, `production`**
- **Planet snapshot なし**: colony が参照する planet entity の `planet_name` だけ inline されており、**独立した `planets[id]` table は存在しない**

### 1.3 呼び出しサイト
- `lifecycle.rs:283` `evaluate_fire_conditions` (per-tick、決定論のため 1 回 build して pending decision 全部で共有)
- `lifecycle.rs:435` `dispatch_event_handlers` (per-fired-event、毎回 `attach_gamestate`)
- どちらも `lua.gc_collect()` で後片付け (#320 fix)

---

## 2. view 型の実装方針選定

### 2.1 結論: **snapshot 方式を継続、"view 型"は既存 table の shape 拡張 + docs 上の呼称**

#263 の Spike A / snapshot-per-event pivot (`memory/project_lua_gamestate_api.md` L9-10、`gamestate_view.rs:24-47`) を踏襲する。UserData に戻すのは却下。

### 2.2 代替案比較

| 案 | 実装コスト | Risk | 採否 |
|---|---|---|---|
| **A. 現状の sealed-table snapshot を拡張** (本計画) | 中 (各 entity に field 追加、新規 `planets` map 追加) | 低 (既存の seal/navigation pattern 再利用) | **採用** |
| **B. `mlua::scope` + 非 'static UserData を再挑戦** | 高 (#263 Spike A で child UserData を method から返せないことが判明済) | 高 (mlua 0.11 制約は変わらず、proxy-table plumbing で二重実装) | 却下 |
| **C. `RegistryKey` + newtype wrapper** | 中-高 (aux-stack 管理、#328 cache と衝突) | 中 (GC lifecycle 要再設計) | **#328 後に再評価**、本 issue scope 外 |
| **D. Rust 側の真の view trait で Lua expose** | 高 (trait object + dyn userdata の mlua 制約) | 中-高 | 却下 |

### 2.3 判断根拠

1. **mlua 0.11 制約**: scoped UserData の method は `'static` bound、内部から子 UserData を返せない (#263 Spike A 結果、`gamestate_view.rs:30-33`)。
2. **観測的等価性**: 単一 callback 中 world は mutate されない (mutation は pending queue 経由、dispatch 後に drain) ため snapshot = live view と観測上区別不可能 (`memory/project_lua_gamestate_api.md` L10)。
3. **既実装の流用**: seal_table / seal_set_like_table / `_ids` array pattern が #263 で確立済で、LuaJIT ipairs 制約への対処も完了している (`gamestate_view.rs:481-488` コメント)。
4. **#328 との整合**: per-tick cache を入れる時、cache 対象は「この table」で済む。UserData にしてしまうと cache semantics が複雑化。
5. **"view 型" の命名**: issue #289 要件 (階層 navigation `system.colonies()[1].buildings`) は **関数 `()` で返す shape** を要求するが、sealed Lua table 上の `:colonies()` メソッドは `__index` metamethod から Rust closure を返せば実現可能。

### 2.4 view 型の最終 shape (Lua からの見え方)

issue #289 要件は「各 view が method 的に `()` で navigation を返す」形だが、これは sealed table + `__index` (metamethod) / 追加 helper 関数で両対応する。**method-call syntax (`system:colonies()`) と属性 syntax (`system.colonies`) の両方をサポート**、中身は同じ array snapshot を返す:

```lua
-- どちらも等価:
for _, cv in ipairs(sys:colonies()) do ... end
for _, cv in ipairs(sys.colonies)   do ... end
```

実装: `.colonies` を unsealed array として直接格納 (lazy 化は #328)。issue 本文の `()` suffix は docs 上の呼称 (Rust 的 method 表記) とみなす。

### 2.5 write 拒否 (read-only 保証)

**既存の seal_table パターンで十分**。shadow table に値を移して visible table を空にし、`__newindex` で runtime error を投げる (`gamestate_view.rs:435-489`)。新規追加する view 型 (planets / production 等) はすべて `seal_table(lua, &tbl)?` を最後に呼ぶ。
- 配列 (unsealed) のみ mutation 保護なしだが、これは既存挙動と一致。
- `Ship.modules` / `Colony.buildings` はすべて **配列 snapshot** として返すため、script が push/remove しても world に波及しない。

---

## 3. 各 view 型の不足 field 一覧 (snapshot 構築コードの差分)

### 3.1 SystemView (`gamestate_view.rs:170-190` 拡張)

| field | 現状 | 必要処置 |
|---|---|---|
| `.entity` | ✅ `id` として expose 済 | alias `entity` も set |
| `.name` | ✅ | — |
| `.position` | ❌ | `world.get::<components::Position>(entity)` → `{x, y, z}` sealed table |
| `.planets` | ❌ | `world.query::<(Entity, &Planet)>()` で `p.system == entity` な planet 列を filter、id 配列として expose |
| `.colonies` | ❌ | `Colony` を `Planet.system` 経由で逆引き、id 配列 |
| `.owner` | ❌ | `world.get::<Sovereignty>(entity)` → `Owner::Empire(e)` を empire_id として expose。`None` なら nil |
| `.modifiers` | ❌ | `world.get::<SystemModifiers>(entity)` → `{ship_speed, ship_attack, ship_defense}` を `.final_value()` した sealed table |
| `.surveyed`, `.is_capital`, `.star_type`, `.resources` | ✅ | 維持 |

実装 note: `planets` / `colonies` は **id 配列** として expose (`planet_ids` / `colony_ids` sibling arrays)。`system.colonies` を「colony table 配列」として返す完全版は、子 table への再 lookup で実現可能。ハイブリッドで提供:
- `system.colony_ids` (unsealed int array、cheap)
- `system.colonies` (eager snapshot 配列 — lazy 化は #328 後)

### 3.2 PlanetView (新規 `planets` map 追加)

現状 `planets` top-level map は **存在しない**。新規に:

| field | 必要 |
|---|---|
| `.entity` / `id` | Planet entity.to_bits() |
| `.name` | `Planet.name` |
| `.planet_type` | `Planet.planet_type: String` |
| `.biome` | **Biome component は存在しない** — `planet_type` と同じ文字列で埋める placeholder。TODO コメント |
| `.system` | `Planet.system` を entity_id で expose (`system_id`)、`.system` helper は `gs.systems[planet.system_id]` resolve |

実装箇所: `gamestate_view.rs:286` (colony 構築手前) に新規 `planets` / `planet_ids` セクションを追加。

### 3.3 ColonyView (`gamestate_view.rs:296-309` 拡張)

| field | 現状 | 必要処置 |
|---|---|---|
| `.entity` / `id` | ✅ | — |
| `.owner` | ❌ | **Colony → Empire link が存在しない** → Sovereignty.owner 経由で planet→system→Sovereignty chain して設定。`None` 時 nil |
| `.population` / `.growth_rate` | ✅ | 維持 |
| `.buildings` | ❌ | `world.get::<Buildings>(entity)` → `slots` から `Vec<Option<BuildingId>>` を map、空 slot は nil、building は `{id="shipyard"}` table。unsealed array |
| `.production` | ❌ | `world.get::<Production>(entity)` → `{minerals_per_hexadies, energy_per_hexadies, research_per_hexadies, food_per_hexadies}` を `.final_value().to_f64()` で sealed table |
| `.planet` | ✅ `planet_id` 済 | helper `colony.planet` は `gs.planets[planet_id]` resolve |

### 3.4 FleetView (`gamestate_view.rs:264-280` 拡張)

| field | 現状 | 必要処置 |
|---|---|---|
| `.entity` / `id` | ✅ | — |
| `.owner` | ❌ | Fleet 自体は Owner を直接持たない → **flagship ship の `Ship.owner`** を引いて `{kind="empire", empire_id=...}` で expose |
| `.ships` | ✅ `members` 済 | alias `ships` と `ship_ids` を両方 set |
| `.state` | ❌ | **FleetState は γ-2 (#287) まで未実装**。flagship の `ShipState` を proxy で expose |
| `.origin` | ❌ | `ShipState::SubLight.origin` / `InFTL.origin_system`。flagship 由来 |
| `.destination` | ❌ | 同上 |

### 3.5 ShipView (`gamestate_view.rs:225-246` 拡張)

| field | 現状 | 必要処置 |
|---|---|---|
| `.entity` / `id` | ✅ | — |
| `.name` / `.design_id` | ✅ | — |
| `.fleet` | ❌ | `Ship.fleet: Option<Entity>` を `fleet_id` で expose、`.fleet` helper は `gs.fleets[fleet_id]` resolve |
| `.hp` | ❌ | `world.get::<ShipHitpoints>(entity)` → `{hull, hull_max, armor, armor_max, shield, shield_max, shield_regen}` |
| `.modules` | ❌ | `Ship.modules: Vec<EquippedModule>` → `[{slot_type, module_id}, ...]` unsealed array |
| `.state` | ❌ (新規) | `ShipState` enum を tag-union table へ (Docked/SubLight/InFTL/Surveying/Settling/Refitting/Loitering/Scouting) |
| その他 | ✅ | 維持 |

### 3.6 EmpireView (`build_empire_table`, `:331-423` — 大部分 実装済)

| field | 現状 | 必要処置 |
|---|---|---|
| `.id` / `.name` | ✅ | — |
| `.tech` | ✅ `techs` (set-like) | alias `tech` を追加 (docs 上の呼称) |
| `.flags` | ✅ | 維持 |
| 他 | ✅ | 維持 |

**→ EmpireView は既にほぼ完成**。alias のみ追加で OK。

---

## 4. 新規 entity 種別 (Planet) の snapshot 追加

```rust
// gamestate_view.rs:286 colony セクション手前に挿入
// --- planets ---
let planets_tbl = lua.create_table()?;
let planet_ids  = lua.create_table()?;
let planet_rows: Vec<(Entity, String, Entity, String)> = {
    let mut q = world.query::<(Entity, &Planet)>();
    q.iter(world).map(|(e, p)| (e, p.name.clone(), p.system, p.planet_type.clone())).collect()
};
for (entity, name, system, planet_type) in planet_rows {
    let ptbl = lua.create_table()?;
    ptbl.set("id", entity.to_bits())?;
    ptbl.set("entity", entity.to_bits())?;
    ptbl.set("name", name.as_str())?;
    ptbl.set("planet_type", planet_type.as_str())?;
    ptbl.set("biome", planet_type.as_str())?;        // TODO: Biome 未実装の placeholder
    ptbl.set("system_id", system.to_bits())?;
    seal_table(lua, &ptbl)?;
    planets_tbl.set(entity.to_bits(), ptbl)?;
    planet_ids.push(entity.to_bits())?;
}
seal_table(lua, &planets_tbl)?;
gs.set("planets", planets_tbl)?;
gs.set("planet_ids", planet_ids)?;
```

**Ref cost 予測**: 現状 ~101 refs/build。planet 1 個で +6、system +15、ship +15、fleet +10、colony +10。**~+50 refs/build、build cost 倍増しない**。`lua.gc_collect()` で回収、#320 境界に影響限定的。

---

## 5. `__index(id)` / `ipairs` / `pairs` policy

| shape | 用途 | メタメソッド | Lua iteration | 本計画での適用 |
|---|---|---|---|---|
| **sealed map** (`empires`, `systems`, `ships`, `fleets`, `colonies`, 新規 `planets`) | id → snapshot lookup | `__index` + `__newindex` + `__metatable="locked"` | `pairs` 不可、id で lookup | `planets` も同 pattern |
| **unsealed array** (`*_ids`, `fleet.members`, `empire.colony_ids`) | id 列挙 | なし | `ipairs` ✅ | 新規 `system.planet_ids`, `system.colony_ids`, `colony.building_slots`, `ship.module_list`, `fleet.ship_ids` も同 pattern |
| **sealed set-like** (`techs`, `flags`) | membership query | `__index(id) -> bool` | `pairs` 不可 | 追加なし |
| **sealed leaf table** (`clock`, `resources`, 新規 `hp`, `production`, `position`, `modifiers`) | 決まった key の束 | `__index` + `__newindex` | `pairs` 不可 | 新規追加全部 |

---

## 6. Integration test 計画

### 6.1 既存 `gamestate_view.rs` unit tests の拡張 (~200 lines)

- `test_systemview_position_planets_colonies_owner`
- `test_planetview_basic`
- `test_colonyview_buildings_and_production`
- `test_shipview_hp_modules_state`
- `test_fleetview_owner_state_origin_destination`
- `test_all_view_tables_sealed_on_write`
- `test_hierarchical_navigation_from_lua`

### 6.2 新規 integration test (`macrocosmo/tests/lua_view_types.rs`, ~250 lines)

- `test_gamestate_view_hierarchical_navigation` — issue 受入 test
- `test_fleet_origin_destination_via_flagship`
- `test_ship_state_variants_exposed`
- `test_view_mutation_blocked_all_nested`
- 既存 `test_existing_event_scripts_still_work` guard を拡張

### 6.3 既存 `stress_lua_scheduling.rs` への影響

~100 → ~150 refs/build 見込み。1000 tick stress で `LUA_MEMORY_CEILING_BYTES = 32 MiB` 以下に収まるか確認必須。

---

## 7. #328 (per-tick gamestate cache) との整合性

### 7.1 #328 の contract

> World mutations become visible in the **next tick's** gamestate snapshot.

### 7.2 本 issue での前提

- **cache を前提とせず実装**。cache あり / なしで挙動不変。
- `lifecycle.rs:283 evaluate_fire_conditions` / `:435 dispatch_event_handlers` の**呼び出しパスは変更しない**。

### 7.3 推奨 merge order

1. **#328 先行** → 本 issue は純粋に field 追加、risk 最小化
2. **本 issue 先行** でも理論上通るが、stress test marginal

---

## 8. Commit 分割案 (6 commits、~700 lines total)

### Commit 1: Planets top-level map + SystemView 拡張 (~150 lines)
- `planets / planet_ids` セクション追加
- `systems[id]` に `position`, `planet_ids`, `colony_ids`, `owner_empire_id`, `modifiers` 追加

### Commit 2: ColonyView owner / buildings / production (~100 lines)

### Commit 3: ShipView hp / modules / state (~200 lines)

### Commit 4: FleetView owner / state / origin / destination (~130 lines)

### Commit 5: EmpireView alias (tech) + integration test suite (~260 lines)

### Commit 6: docs / CLAUDE.md / memory update (~60 lines)

---

## 9. `memory/project_lua_gamestate_api.md` の update 箇所

末尾に追記:

```markdown
## 追加確定事項 (issue #289)

**"view 型" は独立の Rust 型や UserData ではなく、既存 snapshot table の field 拡張 + docs 上の呼称**として実装。

### 新規 expose
- top-level `planets` / `planet_ids` map
- SystemView: position, planet_ids, colony_ids, owner_empire_id, modifiers
- PlanetView: id, name, planet_type, biome (placeholder), system_id
- ColonyView: owner_empire_id (Sovereignty chain), building_slots, production
- ShipView: fleet_id, hp, modules, state (tag-union)
- FleetView: owner_* (flagship proxy), state/origin/destination (flagship.ShipState proxy)
- EmpireView: tech alias

### 既知の仕様上の妥協
- ColonyView.owner_empire_id は chain 間接。Colony.Owner 別 issue で単純化
- FleetView は flagship proxy。γ-2 (#287) 後に FleetState 直接参照に切替
- PlanetView.biome は placeholder
- ShipView.state は 8 variant tag-union
```

---

## 10. リスクと事前検証項目

| # | risk | 対策 |
|---|---|---|
| R1 | ShipState variant 実装漏れ | match wildcard + warn、全 variant test |
| R2 | Biome 未実装 | placeholder + docs、別 issue |
| R3 | FleetState γ-2 待ち | flagship proxy、γ-2 merge 後に切替 |
| R4 | Colony→Empire link 不在 | Sovereignty chain、None test |
| R5 | aux-stack ceiling 再発 | stress test 確認、NG なら #328 先行 |
| R6 | 既存 script 破壊 | 追加のみ、既存 guard 維持 |
| R7 | ShipHitpoints 欠落 ship | Option 扱い、unwrap 禁止 |
| R8 | Sovereignty 欠落 system | None → nil |
| R9 | Amt → f64 | `.final_value().to_f64()` |

---

## 11. Out of scope (follow-up issue 推奨)

1. Biome component / BiomeDefinition
2. FleetState (γ-2 #287)
3. Colony.Owner component
4. #328 per-tick cache
5. 書き込み API
6. lifecycle / tech / faction hook への view 注入

---

## Critical Files for Implementation

- `macrocosmo/src/scripting/gamestate_view.rs`
- `macrocosmo/src/scripting/lifecycle.rs`
- `macrocosmo/src/galaxy/mod.rs`
- `macrocosmo/src/ship/mod.rs`
- `macrocosmo/tests/lua_view_types.rs` (新規)

---

## 12. 実装 note (2026-04-15、PR 作成時)

6 commit に分割して実装完了:

1. **Commit 1**: Planets top-level map + SystemView 拡張 (`position` / `planet_ids` / `colony_ids` / `owner_empire_id` / `modifiers`)。`planets_by_system` / `colonies_by_system` を Rust HashMap で事前集計して重複 query を回避
2. **Commit 2**: ColonyView 拡張 — `owner_empire_id` は planet.system → Sovereignty chain。Buildings は `building_slots` (sealed `{id}` entry) と `building_ids` (flat string array) の 2 形態を両立
3. **Commit 3**: ShipView に `fleet_id` / `hp` / `modules` / `state` (8 variant tag-union)。`build_ship_state_table` ヘルパを新設、wildcard arm は warn + `{kind="unknown"}` fallback (R1)
4. **Commit 4**: FleetView は flagship proxy で owner / state / origin / destination を expose。`proxy_ship = flagship.or(members.first())` で flagship unset 時も動作
5. **Commit 5**: EmpireView `tech` alias + integration test (`tests/lua_view_types.rs`) 3 本 (navigation / mutation block / tech alias)
6. **Commit 6**: docs update (plan 本ファイルと `memory/project_lua_gamestate_api.md`)

### 実装時の判断

- **sealed set-like table の alias**: `techs` と `tech` は同一 sealed table への 2 参照。`seal_set_like_table` は visible table を空にするため `.clone()` で Lua ref を 2 本持たせる
- **`SystemModifiers::default()` は 0 返し**: `ModifiedValue::default()` は base=0 のため final_value も 0。unit test は数値を固定せず `.is_ok()` 程度にとどめ、production の StarTypeModifierSet seed は別系統に委ねる
- **ReportMode variant**: 計画では `Auto` としたが実装は `FtlComm` / `Return` のみ。scouting test は `FtlComm` を使用

### aux-stack / ref count 実測

- 既存 `stress_lua_scheduling.rs` (1000 tick) で LuaJIT heap ceiling (32 MiB) 以下に収束することを確認
- 新規 field 追加による refs/tick 増加の実測値は PR body 参照
