# Implementation Plan: Issue #296 — S-3 Infrastructure Core Deliverable + spawn-as-immobile-Ship lifecycle

_Prepared 2026-04-15 by Plan agent. Depends on #297 (S-2 FactionOwner 統一付与, planning in parallel) for the Core-ship's `FactionOwner` attachment. #287 (Fleet γ-1) is merged. #281 (`on_built`/`on_upgraded` hooks) is merged and **not** a blocker here._

---

## 1. 既存 Deliverable 流路の実装箇所

### Core entry points
- **`src/ship/deliverable_ops.rs:168-238`** — `process_deliverable_commands` system. `QueuedCommand::DeployDeliverable { item_index }` が完了した tick で発火。Core deliverable の分岐を挿入する中核。
- **`src/ship/deliverable_ops.rs:201-207`** — 現行 `spawn_deliverable_entity` 呼び出し。Core の場合はここを **ship spawn 分岐に切り替える**。
- **`src/deep_space/mod.rs:490-532`** — `spawn_deliverable_entity(commands, metadata, position, owner, ...)` — non-Core deliverable は従来通り DeepSpaceStructure を生成。
- **`src/deep_space/mod.rs:166-172`** — `DeliverableMetadata` 定義。`spawns_as_ship: Option<String>` field を追加して Core 識別のマーカーにする。
- **`src/scripting/structure_api.rs:28-122`** — Lua `define_deliverable` のパーサ。`spawns_as_ship` field を受け取る拡張が必要。

### Cargo / queue
- `Ship.cargo` は `Vec<DeliverableItem>` (既存)、`item_index` 指定で消費。
- Deploy 中の self-destruct (validation fail) は cargo から item を remove するだけで十分。

### Shipyard での建造
- `Courier` + Shipyard / Port 経由で Core deliverable を建造 → Cargo に load → 目的地 system へ運搬。既存 Deliverable 流路と**同一**。新規コードなし。

---

## 2. Core ship spawn の具体的な場所

### 新設 system `resolve_core_deploys` in `src/ship/core_deliverable.rs`
- `.after(process_deliverable_commands)` で走る。
- `PendingCoreDeploys` resource から tickets を drain し、**同 system 同 tick の tie-break** 後に `spawn_core_ship_from_deliverable` を呼ぶ。
- spawn 後 `tickets.clear()`。

### `spawn_core_ship_from_deliverable(world, ticket) -> Option<Entity>`
1. `system_inner_orbit_position(ticket.target_system, &world)` で座標決定
2. `spawn_ship` を呼び出す (既存 `src/ship/mod.rs:549-617`) — design_id = `infrastructure_core_v1`, owner = ticket.owner, position = inner_orbit, state = `ShipState::Docked { system: target_system }`
3. 返ってきた `Entity` に以下を insert:
   - `CoreShip` (新設 marker、§9 参照)
   - `FactionOwner(ticket.faction_owner)` (**#297 依存 — PR merge 後に有効化**)
   - `AtSystem(target_system)` (sovereignty lookup のため)
4. `cargo` から item を consume、deployer ship の `QueuedCommand::DeployDeliverable` を解除

### Deploy 意図の検知箇所
`deliverable_ops.rs:168-238` 内の既存 deploy ブロックで:
- `metadata.spawns_as_ship.is_some()` なら Core branch へ。
- validation (§6) を実施、pass なら `PendingCoreDeploys.tickets.push(...)` + cargo consume。実際の spawn は後続 system で。
- Non-Core deliverable は既存 `spawn_deliverable_entity` 呼び出しのまま。

---

## 3. `system_inner_orbit_position` helper

### Signature
```rust
/// #296 (S-3): 指定 StarSystem の最内惑星よりさらに内側 (恒星寄り) の軌道位置を返す。
/// Core ship を配備する標準座標。Planet.orbital_radius が無いため
/// `INNER_ORBIT_OFFSET_LY = 0.05` の offset を system center から与える固定座標で近似する。
pub fn system_inner_orbit_position(system: Entity, world: &World) -> [f64; 3]
```

### 場所
- **`macrocosmo/src/galaxy/mod.rs`** に定数 + helper を追加。
  ```rust
  pub const INNER_ORBIT_OFFSET_LY: f64 = 0.05;
  pub const SYSTEM_RADIUS_LY: f64 = 0.1;  // 系の有効半径 (UI orbit 描画と整合)
  ```

### 実装方針
- `world.get::<Position>(system)` で system 中心座標を取得
- +X 方向に `INNER_ORBIT_OFFSET_LY` オフセット (deterministic、seed 不要)
- 将来 Planet に `orbital_radius` を導入した時点で「最内 planet.orbital_radius - ε」に置き換える TODO コメントを残す

### テスト
- `test_system_inner_orbit_position_deterministic` — 同 system で 2 回呼んで同一座標
- `test_system_inner_orbit_position_within_radius` — 結果が system center から `INNER_ORBIT_OFFSET_LY ± epsilon`

---

## 4. `is_immobile()` helper と routing/pursuit gate

### Helper 新設
**`Ship::is_immobile(&self) -> bool`** を `src/ship/mod.rs:362-388` (Ship struct impl block) に追加。
```rust
impl Ship {
    /// #296 (S-3): sublight/FTL ともに移動不能 (例: Infrastructure Core)。
    /// routing / UI MoveTo / pursuit detector で skip 対象にする。
    pub fn is_immobile(&self) -> bool {
        self.sublight_speed <= 0.0 && self.ftl_range <= 0.0
    }
}
```

### routing への gate
- **`src/ship/routing.rs:188-223 plan_route_full`** — 既に `max_speed <= 0.0` で `None` を返す実装があり、`is_immobile` ship は自動 skip 済。確認のみでコード変更不要。
- **`src/ship/routing.rs:576, 655, 673`** — caller 経路で `plan_route_full` が None を返す分岐処理を確認。

### `start_sublight_travel` の Result 化 (breaking change)
- **`src/ship/movement.rs:28-60`** — `pub fn start_sublight_travel[_with_bonus](...)` を `Result<(), &'static str>` 返却に変更。immobile ship は `Err("ship is immobile")` を返す。
- 全 caller を `let _ = ship.start_sublight_travel(...)` 等に書き換え:
  - `src/ship/command.rs:118, 273, 346`
  - `src/ship/routing.rs:576, 655, 673`
  - tests 配下

### UI MoveTo の gate
- `src/ui/ship_panel.rs:1273` — MoveTo push 前に `if ship.is_immobile() { return; }` で button を disable
- `src/ui/context_menu.rs:251, 259, 326` — 同じく MoveTo push を guard

### Pursuit detector の self-immobile early return
- `src/ship/pursuit.rs:206, 241` 付近の detector loop で `if self_ship.is_immobile() { continue; }` を追加。Core は Defensive ROE なので **検出側としては走らない** が念のため。
- **被検出側** (targets) として immobile ship が pursuit される可能性は許容 (Core は敵から pin されうる、仕様通り)。

### ROE 補足
- 現行 ROE variants: `Defensive | Retreat | Aggressive`
- Core ship はデフォルトの `Defensive` で OK。pursue/retreat は基本 `ShipState::SubLight | Loitering` でのみ発火し、Docked/Loitering の immobile ship は自然に skip される。

### まとめ
- helper: `Ship::is_immobile(&self) -> bool` は **新規**
- `start_sublight_travel[_with_bonus]` は **Result 化** (breaking change — 全 caller に `let _ = ...` または `.ok();`)
- UI MoveTo dispatch 3 箇所を guard
- pursuit detector loop で self-immobile early return

---

## 5. Core hull Lua 定義

### 配置判断: **新規 2 ファイル**

**理由**:
- Structures (`scripts/structures/definitions.lua`) は `define_deliverable` 主体で、Core は deliverable なので同系列。
- 同時に Core deliverable から spawn される ship の **design / hull** も定義する必要があり、これは `scripts/ships/` 配下のもの。
- 混在は読みにくいため、**2 ファイル新設**:
  1. `scripts/structures/cores.lua` — `define_deliverable { id = "infrastructure_core", ... }`
  2. `scripts/ships/core_hulls.lua` — `define_hull { id = "infrastructure_core_hull", ... }` + `define_ship_design { id = "infrastructure_core_v1", ... }`
- `scripts/structures/init.lua` と `scripts/ships/init.lua` で require を追加。

### Core deliverable Lua (新設 `scripts/structures/cores.lua`)

```lua
-- Infrastructure Core — S-3 (#296)
-- Shipyard で建造され、Courier で運搬、指定 system の内側軌道に deploy。
-- Deploy 瞬間に immobile Ship (sublight=0, ftl=0) として spawn し、
-- その system の sovereignty を宣言する (#295 system_owner の source)。

local infrastructure_core = define_deliverable {
    id = "infrastructure_core",
    name = "Infrastructure Core",
    description = "A self-governing sovereignty anchor. Deploys as an immobile "
               .. "command ship in the innermost orbit of its target star system.",
    max_hp = 200,
    cost = { minerals = 800, energy = 500 },
    build_time = 120,
    cargo_size = 5,
    scrap_refund = 0.2,
    capabilities = {},
    energy_drain = 0,
    -- deploy 時の spawn 分岐を有効化するマーカー (Rust 側で parse)
    spawns_as_ship = "infrastructure_core_v1",
}

return { infrastructure_core = infrastructure_core }
```

### Core hull + design (新設 `scripts/ships/core_hulls.lua`)

```lua
local slot_types = require("ships.slot_types")
local modules = require("ships.modules")

local infrastructure_core_hull = define_hull {
    id = "infrastructure_core_hull",
    name = "Infrastructure Core Hull",
    base_hp = 400,
    base_speed = 0.0,       -- immobile (sublight gate)
    base_evasion = 0.0,
    slots = {
        -- FTL なし、sublight なし → is_immobile() が true
        { type = slot_types.defense, count = 2 },
        { type = slot_types.utility, count = 3 },
        { type = slot_types.power, count = 2 },
        { type = slot_types.command, count = 1 },
    },
    build_cost = { minerals = 0, energy = 0 },  -- direct build 不可
    build_time = 0,
    maintenance = 1.0,
}

local infrastructure_core_v1 = define_ship_design {
    id = "infrastructure_core_v1",
    name = "Infrastructure Core Mk.I",
    hull = infrastructure_core_hull,
    modules = {
        { slot_type = "defense", module = modules.armor_plating },
        { slot_type = "utility", module = modules.cargo_bay },
    },
}

return {
    infrastructure_core_hull = infrastructure_core_hull,
    infrastructure_core_v1 = infrastructure_core_v1,
}
```

### init.lua 変更
- `scripts/structures/init.lua` — 現状 `return require("structures.definitions")` に `cores` を merge
- `scripts/ships/init.lua` — `core_hulls` を require

### Rust 側 metadata parse 拡張
- `DeliverableMetadata` (`src/deep_space/mod.rs:166-172`) に `pub spawns_as_ship: Option<String>` 追加
- `src/scripting/structure_api.rs` の `define_deliverable` 処理 (L28-122 周辺) で `spawns_as_ship` field を parse → `String` 保存

---

## 6. 配備 validation 3 パターン

### (a) 既存 Core 存在チェック
- **方法**: Query `existing_cores: Query<&AtSystem, With<CoreShip>>` を `process_deliverable_commands` に inject。deploy 時に `existing_cores.iter().any(|at| at.0 == target_system)` で判定。
- **自壊動作**: deliverable item を Cargo から remove、Ship spawn しない、warn log。

### (b) system 外判定
- **方法**: `deploy_pos` から最寄り StarSystem を探索 (`systems.iter()` で `distance_ly_arr(pos, sys_pos) < SYSTEM_RADIUS_LY` を満たすもの)。
- **SYSTEM_RADIUS_LY**: `src/galaxy/mod.rs` に `pub const SYSTEM_RADIUS_LY: f64 = 0.1;` を新設。
- **自壊動作**: 同上。

### (c) 同 tick 競合 tie-break (別 system 分離 方式を採用)

**設計** — `process_deliverable_commands` は Commands deferred のため同 tick 内で spawn 済 Core を再検出できない問題への対処:

1. `Resource<PendingCoreDeploys>` を新設:
   ```rust
   #[derive(Resource, Default)]
   pub struct PendingCoreDeploys {
       pub tickets: Vec<CoreDeployTicket>,
   }
   pub struct CoreDeployTicket {
       pub deployer: Entity,
       pub target_system: Entity,
       pub deploy_pos: [f64; 3],
       pub faction_owner: Entity,
       pub owner: Owner,
       pub design_id: String,
       pub cargo_item_index: usize,
       pub submitted_at: i64,
   }
   ```
2. `deliverable_ops.rs` は validation (a)(b) 通過したら `PendingCoreDeploys.tickets.push(ticket)` + cargo 消費。
3. 新 system `resolve_core_deploys` (`src/ship/core_deliverable.rs`、`.after(process_deliverable_commands)`) で:
   1. `tickets` を `target_system` でグループ化
   2. グループ内 2+ なら `GameRng` (既存 resource) で 1 つ選出、他は log のみ
   3. 既存 Core check を再実行 (前 tick までに deploy された Core との競合)
   4. 採用 ticket で `spawn_core_ship_from_deliverable` 実行
   5. tickets clear

**採用理由**: Local state 管理が不要、テスト容易、ordering clear。

---

## 7. #297 依存の明確化

### Core ship の FactionOwner 付与
- 現行 Ship は `owner: Owner::Empire(Entity)` field を持つが、`FactionOwner(Entity)` component は Ship に付与されていない。
- `system_owner(...)` (`src/faction/mod.rs:964-974`) は `Query<(&AtSystem, &FactionOwner)>` を使うため、**Ship に FactionOwner が付かなければ sovereignty が引けない**。
- この付与は **#297 (S-2) スコープ**: Ship の全 spawn 経路 (spawn_ship 経由) に `FactionOwner` を統一付与。

### impl 順序
**#297 merge 前提**。ただし本 issue の以下は **#297 と並行可**:
- Lua 定義 (cores.lua / core_hulls.lua)
- helper (is_immobile / system_inner_orbit_position)
- UI gate / pursuit self-immobile

### 単独では test 不成立?
- spawn 位置 test: FactionOwner 不要 → 先行 pass
- 自壊 validation test: FactionOwner 不要
- immobile gate test (MoveTo 拒否 / sublight 拒否): FactionOwner 不要
- **sovereignty derive test** (Core 配備で `system_owner` が faction を返す): FactionOwner 必須 → **#297 依存**
- savebag round-trip: FactionOwner は既存 savebag 対応済 (`savebag.rs:856-869`)、attach 通れば既存基盤で pass

**結論**: 完了条件のうち sovereignty derive test のみ #297 blocked。他は並行可。最終 PR merge 前に #297 merge 前提。

---

## 8. #281 との関係

### 事実確認
`gh issue view 281` → **state: CLOSED**。`on_built` / `on_upgraded` hook は実装済み (`src/scripting/structure_api.rs:108` で parse、`deep_space/mod.rs:207-212` で保持、`event_system.rs:348` に `BUILDING_BUILT_EVENT`、`colony/building_queue.rs:615, 740`、`colony/system_buildings.rs:304, 406`、`deep_space/mod.rs:606` で fire)。

### 本 issue との関連
- Core deliverable の Lua 定義は `define_deliverable { on_built = function(event) ... end }` を書ける (parser 既存対応)。
- **ただし** `deliverable_ops.rs:168-238` の DeployDeliverable 分岐では `BUILDING_BUILT_EVENT` を fire していない。
- Core は Structure ではなく Ship になる → 「building_built」の event 名と semantic mismatch。
- 既存 ship build event は? → **Ship 完成時の Lua event_bus fire は未実装**。Core 特有の話ではなく ship build 全般 → **本 issue では扱わない (別 issue で `on_ship_built` hook を整備)**。

**結論**: #281 は **blocker ではない**。closed 済・機能既存。Core spawn → Lua hook は本 issue 範囲外。

---

## 9. `CoreShip` marker component の新設判断

### 現状
`src/faction/mod.rs:959-963` の TODO:
```rust
/// TODO(#296): Filter by a dedicated `CoreShip` marker once S-3 lands. Until
/// then, this helper filters by `With<AtSystem>` — currently populated only on
/// hostile entities...
```

### 判断: **CoreShip marker を新設 (本 issue 必須)**

**理由**:
- #297 で Ship 全般に `AtSystem + FactionOwner` が付くと、`system_owner` の現行 filter はありとあらゆる ship を拾う → sovereignty が最初に到着した普通の patrol ship で宣言されてしまう。
- `CoreShip` marker は zero-sized struct で識別子として必要。
- 未導入のまま #297 が landing すると `system_owner` の semantics が壊れる。

### 実装
- **場所**: `src/ship/core_deliverable.rs` (新 module)
  ```rust
  #[derive(Component, Debug, Clone, Copy)]
  pub struct CoreShip;
  ```
- **export**: `ship/mod.rs` で `pub use core_deliverable::CoreShip;`
- **`system_owner` 更新**: `src/faction/mod.rs:964` の signature を:
  ```rust
  pub fn system_owner(
      system: Entity,
      cores: &Query<(&AtSystem, &FactionOwner), With<CoreShip>>,
  ) -> Option<Entity>
  ```
  全 caller (`src/colony/authority.rs:137`) を更新。
- **savebag**: `CoreShip` は marker なので `bag.core_ship: Option<()>` or `bool` で round-trip。savebag.rs に field 追加、save.rs で `e_ref.get::<CoreShip>().is_some()` を書き込み、load.rs で restore。

---

## 10. Regression test 計画

テスト配置: `macrocosmo/tests/infrastructure_core.rs` (新規、integration test)。

### (a) spawn 位置 (§3 helper)
- `test_system_inner_orbit_position_deterministic`
- `test_system_inner_orbit_position_within_epsilon`
- `test_core_spawned_at_inner_orbit` (end-to-end)

### (b) 自壊 3 パターン (§6)
- `test_core_deploy_outside_system_self_destructs`
- `test_core_deploy_existing_core_self_destructs`
- `test_core_deploy_same_tick_tie_break` (GameRng seed で deterministic)
- `test_core_deploy_different_systems_no_conflict`

### (c) immobile gate (§4)
- `test_core_ship_is_immobile`
- `test_start_sublight_travel_rejects_immobile`
- `test_move_to_queue_push_gated`
- `test_routing_plan_route_returns_none_for_immobile`
- `test_pursuit_skips_immobile_self`

### (d) savebag round-trip
- `test_core_ship_savebag_round_trip`
- `test_system_owner_survives_save_load`

### (e) sovereignty integration (#297 blocked)
- `test_core_deploy_sets_system_sovereignty`
- `test_core_despawn_clears_sovereignty`

### Lua 側
- `test_lua_define_deliverable_with_spawns_as_ship`
- `test_lua_define_hull_base_speed_zero`

### existing test への impact
- `start_sublight_travel_with_bonus` Result 化による既存 caller の修正確認

---

## 11. Commit 分割案 (7-9 commits、#297 merge 前提)

1. **`[296] add CoreShip marker + is_immobile helper`**
   - `src/ship/core_deliverable.rs` 新設 (CoreShip marker)
   - `Ship::is_immobile()` 追加 + re-export
   - unit test
2. **`[296] Result-ize start_sublight_travel for immobile ships`**
   - `src/ship/movement.rs` 戻り値変更、全 caller 更新
3. **`[296] add system_inner_orbit_position helper + SYSTEM_RADIUS_LY`**
   - `src/galaxy/mod.rs` に helper + 定数
4. **`[296] Lua core hull / deliverable definitions`**
   - 新設 `scripts/structures/cores.lua` + `scripts/ships/core_hulls.lua`
   - init.lua 更新
   - `DeliverableMetadata.spawns_as_ship` + parser
5. **`[296] spawn_core_ship_from_deliverable + PendingCoreDeploys resource`**
   - `src/ship/core_deliverable.rs`: helper + resource + `resolve_core_deploys` system
   - plugin wire
6. **`[296] deploy_deliverable_commands: branch Core to ticket queue`**
   - `deliverable_ops.rs:196-214` を Core / non-Core で分岐
7. **`[296] update system_owner to filter by CoreShip + UI move gate`**
   - `faction/mod.rs:964` Query に `With<CoreShip>` 付与、TODO 削除
   - `colony/authority.rs` caller 更新
   - UI MoveTo guard (ship_panel / context_menu)
   - pursuit self-immobile early return
8. **`[296] savebag support for CoreShip marker`**
9. **`[296] docs + handoff note`** (optional)

### 注意事項
- **commit 2 (Result 化)** は影響面積が広いので早い commit に置く。
- **#297 merge 前に着手可能**: 1, 2, 3, 4, (5 は FactionOwner attach 行を除けば可)
- **#297 待ち**: 5 の FactionOwner attach, 7 の sovereignty test, 8 の savebag test 一部

---

## 12. Out-of-scope / 将来 issue

- Core ship に対する attacker からの protection (hardening) — 別 issue
- Core destruction → sovereignty transfer cascade (S-10 相当) — #292 の別 S item
- Lua `on_ship_built` / `on_core_deployed` hook — 別 issue
- Planet `orbital_radius` data model 化 — 別 issue

---

## Critical Files for Implementation

- `macrocosmo/src/ship/deliverable_ops.rs`
- `macrocosmo/src/ship/movement.rs`
- `macrocosmo/src/ship/mod.rs`
- `macrocosmo/src/ship/core_deliverable.rs` (new)
- `macrocosmo/src/faction/mod.rs`
- `macrocosmo/src/galaxy/mod.rs`
- `macrocosmo/src/deep_space/mod.rs`
- `macrocosmo/src/scripting/structure_api.rs`
- `macrocosmo/scripts/structures/cores.lua` (new)
- `macrocosmo/scripts/ships/core_hulls.lua` (new)
- `macrocosmo/tests/infrastructure_core.rs` (new)
