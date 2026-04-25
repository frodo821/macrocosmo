# Lua API リファレンス

Macrocosmo の Lua スクリプトから利用できる API を全リスト化したもの。実装の起点は `macrocosmo/src/scripting/globals.rs` の `setup_globals_with_print_buffer`。それに加えて、Rust 側がコールバックの引数として渡してくる **userdata** が複数ある(後述)。

> **このドキュメントの読み方**
> - `define_xxx { ... }` 系は全部「テーブルを 1 つ受け取り、`_def_type` を付与して内部 accumulator に追加し、同じテーブルを返す」。返値はそのまま `prerequisites = { other_def, ... }` のように **参照** として再利用できる。
> - 関数シグネチャは Lua 風の擬似記法で書く。`?` 付きは省略可。
> - 「**書き込み API**」と書かれたものは、ECS World への副作用が発生する。`fire_condition` のような read-only コンテキストでは呼べない。

---

## 1. サンドボックス

`mlua` の `Lua::new_with` でロードされる標準ライブラリ:
- `table` / `string` / `math` / `package` / `bit` のみ

ロードされない: `io`, `os`, `debug`, `ffi`。

明示的に無効化:
- `loadfile = nil`
- `dofile = nil`
- `package.cpath = ""` (C モジュール禁止)

`require()` は `package.path` に `<scripts_dir>/?.lua;<scripts_dir>/?/init.lua` のみが設定されており、`scripts/` ディレクトリ配下のファイルしかロードできない。

### `print(...)`

通常の Lua `print` を **置き換え**。すべての引数を tab 区切りで連結し:
1. stdout に `[lua] <line>` として書き出す
2. `LogBuffer` (Bevy resource) に push される (Alt+F2 の Lua コンソールに表示)

`tostring` を経由せず、特定の primitive 型 (`Nil` / `Boolean` / `Integer` / `Number` / `String`) は直接整形、それ以外は `format!("{:?}", ...)`。

---

## 2. グローバル名前空間

| 名前 | 種別 | 内容 |
|------|------|------|
| `macrocosmo` | table | エンジンが自由に拡張できる空テーブル(現状未使用) |
| `_def_type` | (規約) | 各 define が返すテーブルに付くタグ文字列 |
| `_..._definitions` | accumulator | 各 `define_xxx` が値を append するテーブル |
| `_pending_*` | queue | Rust 側が drain する一時キュー |

ユーザコードから直接これらを触る必要はないが、テスト fixture などでは `_pending_choices` などを差し替えることがある。

---

## 3. 定義 API (`define_xxx`)

すべて `register_define_fn(lua, "<type>", "_<type>_definitions")` で登録される。シグネチャは共通:

```lua
local ref = define_xxx { id = "...", ...fields... }
-- ref は {_def_type = "xxx", id = "...", ...fields...} と同一テーブル
```

使える種別と、その内容を parse する Rust 側ファイル:

| 関数 | _def_type | パーサ | 用途 |
|------|-----------|--------|------|
| `define_tech_branch` | `tech_branch` | `technology/parsing.rs` | 技術ブランチ(Industrial/Military 等) |
| `define_tech` | `tech` | 同上 | 技術ノード(prerequisites, cost, on_researched) |
| `define_building` | `building` | `scripting/building_api.rs` | 惑星/システム両用建造物 |
| `define_star_type` | `star_type` | `scripting/galaxy_api.rs` | 恒星種(色/光度/イベント傾向) |
| `define_planet_type` | `planet_type` | 同上 | 惑星種(基本属性) |
| `define_biome` | `biome` | 同上 (#335) | 惑星バイオーム(planet_type と分離) |
| `define_predefined_system` | `predefined_system` | `scripting/map_api.rs` (#182) | 名前付き既存システム雛形 |
| `define_map_type` | `map_type` | 同上 | マップ生成器(`generator` callback) |
| `define_region_type` | `region_type` | `scripting/region_api.rs` (#145) | 星雲・サブスペースストーム等 |
| `define_species` | `species` | `scripting/species_api.rs` | 種族 |
| `define_job` | `job` | 同上 | 職業 |
| `define_event` | `event` | `scripting/event_api.rs` | ゲームイベント(後述) |
| `define_knowledge` | `knowledge` | `scripting/knowledge_api.rs` (#350) | KnowledgeKind 拡張(後述) |
| `define_slot_type` | `slot_type` | `scripting/ship_design_api.rs` | 船モジュールのスロット種 |
| `define_hull` | `hull` | 同上 | 船体 |
| `define_module` | `module` | 同上 | 船モジュール |
| `define_ship_design` | `ship_design` | 同上 | hull + modules の組合せ |
| `define_structure` | `structure` | `scripting/structure_api.rs` (#223) | 宇宙空間構造物(非船) |
| `define_deliverable` | `deliverable` | 同上 | shipyard で建造する構造物(cost/build_time/cargo_size) |
| `define_anomaly` | `anomaly` | `scripting/anomaly_api.rs` | 探査時アノマリー |
| `define_faction` | `faction` | `scripting/faction_api.rs` | プレイ可能/NPC 派閥 |
| `define_faction_type` | `faction_type` | 同上 | 派閥種別 |
| `define_diplomatic_option` | `diplomatic_option` | 同上 | 外交アクション |
| `define_casus_belli` | `casus_belli` | `scripting/casus_belli_api.rs` (#305 S-11) | 戦争事由 |
| `define_negotiation_item_kind` | `negotiation_item_kind` | `scripting/negotiation_api.rs` (#321) | 和平交渉アイテム種 |

> **共通ルール**: 定義テーブルでは ID 文字列の代わりに `define_xxx` の戻り値テーブル(`_def_type` 付き)が使える箇所が多い。Rust 側の `extract_id_from_lua_value` がどちらも受け付ける。

### `define_balance { ... }` (#160)

唯一 accumulator ではなく `_balance_definition` グローバルを単独セットする(at-most-once、複数呼ぶと上書き + 警告)。`scripts/config/balance.lua` から 1 回だけ呼ぶ想定。

```lua
define_balance {
    base_research_per_hexadies = 5.0,
    -- ...
}
```

### `forward_ref(id)`

まだ定義されていない id に対する placeholder を返す。

```lua
local placeholder = forward_ref("structure_starbase_v2")
-- => { _def_type = "forward_ref", id = "structure_starbase_v2" }
```

`define_xxx` 同士で循環参照がある場合や、ロード順を強制したくない場合に使う。

---

## 4. 条件式 (Conditions)

### Atom 関数(static / scope なし)

すべて「`{ type = "...", id = "..." }` のような table を返す」純粋関数。

| 関数 | 戻り値 |
|------|--------|
| `has_tech(id_or_ref)` | `{ type = "has_tech", id = ... }` |
| `has_modifier(id_or_ref)` | `{ type = "has_modifier", id = ... }` |
| `has_building(id_or_ref)` | `{ type = "has_building", id = ... }` |
| `has_flag(id_or_ref)` | `{ type = "has_flag", id = ... }` |

### Atom 関数(scope 明示) — `ConditionCtx` (#332-B)

`prerequisites` などを **関数で書いた**場合は引数 `ctx` を受け取る:

```lua
prerequisites = function(ctx)
    return all(
        ctx.empire:has_tech("industrial_automated_mining"),
        ctx.system:has_building("research_lab")
    )
end
```

`ctx` は `ConditionCtx` userdata。フィールドは:

| field | 型 | 内容 |
|-------|-----|------|
| `ctx.empire` | `ScopeHandle` | empire scope |
| `ctx.system` | `ScopeHandle` | system scope |
| `ctx.planet` | `ScopeHandle` | planet scope |
| `ctx.ship` | `ScopeHandle` | ship scope |

`ScopeHandle` のメソッド(scope 値が atom テーブルに埋め込まれる):

- `:has_tech(id)` → `{ type = "has_tech", id, scope = "<scope>" }`
- `:has_modifier(id)` → 同上
- `:has_building(id)` → 同上
- `:has_flag(id)` → 同上

`ConditionCtx` は **状態を持たない** 。条件 table を組み立てるだけで、評価は Rust 側 (`condition.rs::EvalContext`) で行われる。

### 外交条件 atom (#322)

派閥-対派閥の標準条件:

| 関数 | 戻り値 |
|------|--------|
| `target_state_is(state)` | `{ type = "target_state_is", state }` |
| `target_state_in(state, ...)` | `{ type = "target_state_in", states = {...} }` |
| `target_standing_at_least(thresh)` | `{ type = "target_standing_at_least", threshold }` |
| `relative_power_at_least(ratio)` | `{ type = "relative_power_at_least", ratio }` |
| `target_allows_option(opt)` | `{ type = "target_allows_option", option_id }` |
| `actor_has_modifier(mod)` | `{ type = "actor_has_modifier", modifier_id }` |
| `actor_holds_capital_of_target()` | `{ type = "actor_holds_capital_of_target" }` |
| `target_system_count_at_most(n)` | `{ type = "target_system_count_at_most", count }` |
| `target_attacked_actor_core_within(hd)` | `{ type = "target_attacked_actor_core_within", hexadies }` |

### Combinator

| 関数 | 戻り値 |
|------|--------|
| `all(c1, c2, ...)` | AND |
| `any(c1, c2, ...)` | OR |
| `one_of(c1, c2, ...)` | 1 つだけ true (XOR-N) |
| `not_cond(c)` | NOT (`not` は予約語なのでこの名前) |

### `check_flag(name)` (legacy)

旧フラグ store の読み取り API。`set_flag(name)` / `modify_global` などの旧 helper は **#332-B4 で廃止**。`gs:set_flag` を使う(後述)。`_flag_store` は forward-compat 用に残っているが、テスト fixture が直接 prime する以外で書かれる経路はない。

---

## 5. イベント

### `define_event { ... }`

```lua
define_event {
    id = "my_event",
    name = "表示名",
    description = "説明",
    trigger = mtth_trigger {        -- もしくは periodic_trigger / "manual" / nil
        years = 1, months = 0, sd = 0,  -- mean time to happen
        fire_condition = function(ctx) return true end,
        max_times = 5,
    },
    -- on_trigger は #263 以降は別系統(on() で event_id="my_event" を購読)
}
```

### `mtth_trigger { ... }` / `periodic_trigger { ... }`

定義テーブルに `_type = "mtth"` / `_type = "periodic"` をタグ付けするコンストラクタ。`years`/`months`/`sd` はゲーム時間単位(hexadies に変換される)。`fire_condition`(任意の関数)、`max_times`(任意)。

### `on(event_id, [filter,] handler)`

event handler を登録。

```lua
on("planet_settled", function(evt)
    print("settled:", evt.planet_id)
    -- evt.gamestate を経由して読み書きできる
end)

-- 構造的フィルタ(2 引数目に table)
on("building_completed", { building_id = "research_lab" }, function(evt)
    -- ...
end)
```

ハンドラは `_event_handlers` に enqueue され、Rust 側 dispatcher が引く。

#### Knowledge 系 event id ルーティング (#352 K-3)

`<kind>@recorded` / `<kind>@observed` / `*@recorded` / `*@observed` の形の event id は、`_event_handlers` ではなく `_pending_knowledge_subscriptions` (knowledge 専用の bucket) に振り分けられる。`<kind>@unknown_phase` のような不明 lifecycle はロード時にエラー。Knowledge 購読には **filter table を渡せない**(渡すと runtime error)。

### `fire_event(event_id, target_entity_bits?)`

Lua 側からイベントをキューイング。`target` は `Entity::to_bits()` の数値(現状 Lua から実体を得るのは難しいので主に effect descriptor 経由で渡される)。

> **重要**: イベント発火は **必ずキュー経由**。Lua から同期 dispatch する経路は意図的に無い(reentrancy 防止)。

### イベント payload と `evt.gamestate`

ハンドラに渡される `evt` テーブルは以下を持つ:
- `kind` / `target` / その他 event-defined フィールド
- `gamestate` — `gs` テーブル(後述)が **このイベント呼出しのスコープに限り** 生きる

スコープを抜けると `gs` の closure は無効化される(mlua の `Lua::scope` セマンティクス)。スコープ外で `gs:foo()` を呼ぶと clean error になる。

---

## 6. ゲームステート API (`gs`)

`gs` はイベント/ライフサイクル callback の `evt.gamestate` として渡される table。`GamestateMode::ReadOnly` (例: `fire_condition`) では writer は付かない。

> 内部実装は `scripting/gamestate_scope.rs::dispatch_with_gamestate`。`Lua::scope` + `RefCell<&mut World>` で読み書き closure を共有する。Lua 側に `gs` への参照を保存しても、スコープを抜けたら無効。

### Reader (常に利用可)

| メソッド | 戻り値 | 備考 |
|----------|--------|------|
| `gs.clock` | `{ now, year, month, hexady_of_month }` | snapshot (read 時) |
| `gs:empire(id)` | EmpireView table | `id` は `Entity::to_bits` (u64) |
| `gs:player_empire()` | EmpireView | プレイヤー empire(無ければ空 table) |
| `gs:system(id)` | SystemView | |
| `gs:planet(id)` | PlanetView | |
| `gs:colony(id)` | ColonyView | |
| `gs:ship(id)` | ShipView | |
| `gs:fleet(id)` | FleetView | |
| `gs:list_empires()` | `{u64...}` | |
| `gs:list_systems()` | `{u64...}` | |
| `gs:list_planets(system_id?)` | `{u64...}` | |
| `gs:list_colonies(filter?)` | `{u64...}` | filter は system か empire の id を渡せる(自動判別) |
| `gs:list_fleets(empire_id?)` | `{u64...}` | |
| `gs:list_ships(fleet_id?)` | `{u64...}` | |

#### View の主要フィールド (`scripting/gamestate_scope.rs::views`)

**EmpireView**: `id`, `name`, `is_player`, `resources` (`{minerals, energy, research, food, authority}`), `techs` / `tech` (set-style), `flags` (set-style), `capital_system_id?`, `colony_ids[]`

**SystemView**: `id` / `entity`, `name`, `surveyed`, `is_capital`, `star_type`, `position?` (`{x,y,z}`), `resources?`, `owner_empire_id?`, `modifiers?` (`{ship_speed, ship_attack, ship_defense}`)

**PlanetView**: `id` / `entity`, `name`, `planet_type`, `biome`, `system_id`

**ColonyView**: `id` / `entity`, `population`, `growth_rate`, `planet_id`, `system_id?`, `planet_name?`, `owner_empire_id?`, `building_slots[]` (`{id=...}` か nil), `building_ids[]`, `production?` (`{minerals_per_hexadies, energy_per_hexadies, research_per_hexadies, food_per_hexadies}`)

**ShipView**: `id` / `entity`, `name`, `design_id`, `hull_id`, `owner_empire_id?` / `owner_kind`, `home_port`, `ftl_range`, `sublight_speed`, `fleet_id?`, `hp?` (`{hull, hull_max, armor, armor_max, shield, shield_max, shield_regen}`), `modules[]` (`{slot_type, module_id}`), `state?` (tag-union)

**FleetView**: `id` / `entity`, `name`, `flagship` (u64 or 0), `members[]`, `ship_ids[]`, `owner_empire_id?` / `owner_kind`, `state?` (flagship proxy), `origin?`/`destination?`/`destination_system?`/`origin_system?`(state によって出る)

**ShipState tag-union** (`{kind=..., ...}`):
- `{kind="in_system", system}`
- `{kind="sub_light", origin, destination, target_system?, ...}`
- `{kind="in_ftl", origin_system, destination_system}`
- 他 5 種(survey, settling, combat 等)

### Writer (`ReadWrite` モードのみ — イベント callback / lifecycle)

すべて副作用がある。`ReadOnly` モードでは存在しない (`gs.set_flag == nil`)。

#### Modifier push

```lua
gs:push_empire_modifier(empire_id, target, opts)
gs:push_system_modifier(system_id, target, opts)
gs:push_colony_modifier(colony_id, target, opts)
gs:push_ship_modifier(ship_id, target, opts)
gs:push_fleet_modifier(fleet_id, target, opts)  -- 現状は flagship に proxy
```

`opts` の形(全て optional):
```lua
{ base_add = 0.0, multiplier = 0.0, add = 0.0, description = "..." }
```

#### Flag

```lua
gs:set_flag(scope_kind, scope_id, name, value?)   -- value デフォルト true
gs:clear_flag(scope_kind, scope_id, name)
```

`scope_kind`: `"empire" | "system" | "colony" | "ship" | "fleet"`(将来拡張)。`scope_id` はそのスコープの entity の `to_bits()`。

#### `gs:request_command(kind, args)` (#334)

ship/colony/fleet 等への命令を発行する。戻り値は新規 `CommandId` (u64)。`CommandRequested` メッセージとして emit され、別 Bevy system が処理(reentrancy 安全)。

`kind` の例(parser 定義は `apply::parse_request`):
- `"move_to"` `{ ship = id, target_system = id }`
- `"build_ship"` / `"build_structure"` / `"research_focus"` 等

> 詳細は `scripting/gamestate_scope.rs::apply::parse_request` を参照(現状ドキュメント未整備、現行サポート kind は AI command consumer #423 と同じ集合)。

#### `gs:record_knowledge { kind, origin_system?, payload }` (#351 K-2)

Lua 起点の Knowledge 投入。フロー:
1. `payload` を schema 検証(`KindRegistry`)
2. `<kind>@recorded` を **同期 dispatch**(購読者は `gs:set_flag` 等を呼んで OK — その間 World 借用は解放)
3. dispatch 後の最終 payload を snapshot 化
4. `enqueue_scripted_fact` で KnowledgeStore へ流し込む

```lua
gs:record_knowledge {
    kind = "battle_summary",
    origin_system = sys_id,  -- 任意
    payload = { aggressor = ..., losses = ... },
}
```

---

## 7. ライフサイクルフック

### グローバル `on_game_start(fn)`

新規ゲーム開始時に **1 回だけ** 呼ばれるグローバルフック。callback は `payload` table 1 引数を受け取り、`payload.gamestate` 経由で `gs:*` (ReadWrite) が使える(派閥ごとの首都セットアップではなく、グローバル初期化用):

```lua
on_game_start(function(payload)
    local gs = payload.gamestate
    print("game start at", gs.clock.now)
    -- gs:set_flag(...) や gs:push_empire_modifier(...) など
end)
```

`_on_game_start_handlers` に登録され、`run_on_game_start_with_gamestate` から `ReadWrite` モードで dispatch される。

> **派閥固有の `on_game_start` は別経路**。`define_faction { on_game_start = function(ctx) ... end }` のフィールドとして書く(下記)。両者は混同しないこと。

### `define_faction { on_game_start = function(ctx) ... end }`

派閥ごとに走る首都セットアップフック。callback には `GameStartCtx` userdata が渡る:

```lua
define_faction {
    id = "humanity_empire",
    on_game_start = function(ctx)
        ctx.system:set_attributes({ name = "Sol", surveyed = true })
        ctx.system:clear_planets()
        local earth = ctx.system:spawn_planet("Earth", "terrestrial", { habitability = 1.0 })
        earth:colonize(ctx.faction)
        earth:add_building("colony_hub_t1")
        ctx.system:spawn_ship("explorer_mk1", "Pioneer")
    end,
}
```

#### `ctx` 構造 (`GameStartCtx`)

| field/method | 内容 |
|--------------|------|
| `ctx.faction` / `ctx.faction_id` | 派閥 id 文字列 |
| `ctx.system` | `SystemHandle`(その派閥に割り当てられた首都) |

#### `SystemHandle` メソッド

| メソッド | 内容 |
|----------|------|
| `:get_planet(idx)` | 1-based 既存惑星の `PlanetHandle` |
| `:add_building(id)` | システム建造物(Shipyard/Port/Lab)を追加 |
| `:spawn_ship(design, name)` | 船を首都にスポーン |
| `:set_capital(bool)` | 首都フラグ |
| `:set_surveyed(bool)` | 既調査フラグ |
| `:clear_planets()` | 既存惑星を全部消す(spawn_planet と併用) |
| `:spawn_planet(name, type, attrs?)` | 新規惑星を生成し `PlanetHandle` を返す |
| `:spawn_core()` | Core ship (`infrastructure_core_v1`) をスポーン |
| `:set_attributes({name?, star_type?, surveyed?})` | 複数回呼ぶと merge |

#### `PlanetHandle` メソッド

| メソッド | 内容 |
|----------|------|
| `:colonize(faction)` | コロニーを置く |
| `:add_building(id)` | 惑星建造物(Mine/Farm/PowerPlant)を追加 |
| `:set_attributes(attrs)` | habitability/mineral_richness/energy_potential/research_potential/max_building_slots を上書き |
| `:index()` | 1-based index |
| `:is_spawned()` | spawn_planet で作ったものか |

> **重要**: `define_faction.on_game_start` は **意図を記録するだけ**。実際の World 適用は callback リターン後に Rust 側が `GameStartActions` を読み、ECS に反映する。`gs:*` 系は呼ばない(`ctx` には gamestate が無い)。

### `on_game_load(fn)`

セーブロード時に呼ばれる。グローバル `on_game_start` と同じく `payload.gamestate` (ReadWrite) を受け取る。`_on_game_load_handlers` に登録され、`run_on_game_load_with_gamestate` から dispatch される。

### `on_scripts_loaded(fn)`

全 Lua スクリプトのロード完了直後に **gamestate 無し** (`()` 引数) で走る static-validation hook。`define_xxx` を遅延発行したり、registry 構築前のクロスチェックに使う。

### Galaxy 生成フック (#181, #199)

すべて **last-wins**(複数登録すると最後のだけ呼ばれる)。

| 関数 | callback 引数 | 用途 |
|------|--------------|------|
| `on_galaxy_generate_empty(fn)` | `GalaxyGenerateCtx` | Phase A: 空のシステム配置 |
| `on_choose_capitals(fn)` | `ChooseCapitalsCtx` | Phase B: 派閥-首都割当 |
| `on_initialize_system(fn)` | `InitializeSystemCtx` | Phase C: 各システムの惑星生成 |
| `on_after_phase_a(fn)` | `GalaxyGenerateCtx` | Phase A 完了後の繋ぎ込み(connectivity bridge 等) |

#### `GalaxyGenerateCtx`

field:
- `ctx.settings` — `{num_systems, num_arms, galaxy_radius, arm_twist, arm_spread, min_distance, max_neighbor_distance, initial_ftl_range}`
- `ctx.systems` — Phase A で既に追加されたシステム配列(read-only snapshot)

メソッド:
- `:spawn_empty_system(name, position, star_type)` — `position` は `{x,y,z}` または `{1,2,3}`
- `:spawn_predefined_system(id_or_ref, {position?, name?})` — `define_predefined_system` の展開
- `:insert_bridge_at(position, star_type?)` — auto `Bridge-NNN` 名。default star_type は `"yellow_dwarf"`
- `:pick_provisional_capital()` — 原点に最も近いシステム entry(connectivity loop で擬似首都に使う)
- `:build_ftl_graph(ftl_range)` — `FtlGraph` userdata を返す

#### `FtlGraph`

field: `ftl_range`, `size`

メソッド:
- `:unreachable_from(system)` — 別 component のシステム配列。`system` は index か `{index=N}`
- `:connected_components()` — `{ {sys_entry, ...}, {sys_entry, ...}, ...}`
- `:closest_cross_cluster_pair(from)` — `(sys_a, sys_b)` 2 値返し(無いと nil, nil)。bridge 挿入位置決定用

#### `ChooseCapitalsCtx`

field:
- `ctx.factions` — 派閥 id 文字列配列
- `ctx.systems` — Phase A 完了時のシステム配列。各 entry に `name/star_type/position/index/capital_for_faction?` 

メソッド:
- `:assign_predefined_capitals()` — `capital_for_faction` ヒント付きシステムを自動割当(戻り値は割当数)
- `:assign_capital(sys_index_or_table, faction)` — 手動割当

#### `InitializeSystemCtx`

field: `index`, `name`, `star_type`, `position`, `is_capital`

メソッド:
- `:spawn_planet(name, type, attrs?)` — 1 回でも呼ぶとデフォルト惑星生成は **暗黙 disable**
- `:override_default_planets(bool?)` — 明示 disable(惑星 spawn せず attribute だけ override したい場合)
- `:set_attributes({name?, surveyed?})`

---

## 8. 通知・選択肢 UI

### `show_notification { ... }` (#151)

トップバナーに live 表示される TTL ベース通知。

```lua
show_notification {
    title = "...",
    description = "...",
    icon = "...",          -- optional
    priority = "low" | "medium" | "high",  -- default "medium"
    target_system = entity_bits,           -- optional, クリックで遷移
}
```

### `push_notification { ... }` (#345 ESC-2)

ESC (Empire Situation Center) 履歴タブへの post-hoc 通知(ack 可、永続表示)。

```lua
push_notification {
    title = "...", message = "...",
    severity = "info" | "warn" | "critical",
    source = { kind = "ship" | "colony" | ..., id = entity_bits },
    event_id = "...",   -- 重複抑制 (#249 経由)
    timestamp = clock_now,  -- 省略時は現在
    children = { { ... }, ... },  -- ネスト可、深さは Rust 側で cap
}
```

### `show_choice { ... }` (#152)

プレイヤー選択肢ダイアログ。

```lua
local choice_ref = show_choice {
    title = "Title",
    description = "...",
    icon = "...",
    target_system = entity_bits,
    options = {
        { label = "Yes", description = "...", condition = ..., cost = ..., on_chosen = function() ... end },
        { label = "No",  on_chosen = function() ... end },
    },
}
-- choice_ref = { _def_type = "choice", id = "<title-slug>_<counter>" }
```

`on_chosen` 関数はキューに保持され、プレイヤー選択時に Rust 側が呼び出す。

---

## 9. Effect descriptor / `EffectScope`

`define_tech` の `on_researched` や `define_diplomatic_option` の `apply` などで、副作用を **デクラレーティブに記述** する仕組み。

### `EffectScope` (callback 引数 `scope`)

```lua
on_researched = function(scope)
    scope:push_modifier("colony.research", { multiplier = 0.10, description = "..." })
    scope:set_flag("flag_name", true, { description = "..." })
end
```

メソッド:
- `:push_modifier(target, { base_add?, multiplier?, add?, description? })`
- `:pop_modifier(target)`
- `:set_flag(name, value, { description? })`

各メソッドは accumulator に effect を積む **と同時に** descriptor table も返す(チェーンや `hide` でラップするため)。

### `effect_fire_event(event_id, payload?)`

descriptor を返すだけで queue には積まない(scope に積まれる)。

### `hide(label, inner_descriptor)`

descriptor を表示用ラベルでラップ:
```lua
hide("ヘッドラインだけ表示", scope:set_flag("internal_flag", true))
```

---

## 10. `set_active_map_type(id_or_ref)` (#182)

エンジンが `generate_galaxy` で使う map_type を切替。`scripts/init.lua` の最後で呼ぶのが普通。

```lua
local m = define_map_type { id = "spiral_v2", generator = function(ctx) ... end }
set_active_map_type(m)  -- もしくは "spiral_v2"
```

`_active_map_type` グローバルに id 文字列を保存。`MapTypeRegistry` が読む。

## 11. `galaxy_generation.add_region_spec { ... }` (#145)

リージョン(星雲・サブスペースストーム)の placement spec を追加:

```lua
galaxy_generation.add_region_spec {
    type = nebula_type_ref,
    count_range = {3, 6},
    radius_range = {2.0, 5.0},
    threshold = 1.0,
}
```

`_pending_region_specs` に積まれ、galaxy 生成時に drain。

---

## 12. Knowledge API (#349 / #350 / #351 / #352)

### `define_knowledge { id, payload_schema? }`

```lua
define_knowledge {
    id = "battle_summary",
    payload_schema = {
        aggressor = "number",
        losses = "number",
        notes = "string",  -- "boolean" / "bool" / "table" も可
    },
}
```

`payload_schema` は flat table (1 階層、ネスト不可)。値は型タグ文字列。これを定義すると、エンジンは:
- `<id>@recorded` イベント
- `<id>@observed` イベント

を **自動登録** する(`register_auto_lifecycle_events`、`scripting/mod.rs` で呼ばれる)。

### 購読

```lua
on("battle_summary@recorded", function(evt) ... end)
on("battle_summary@observed", function(evt) ... end)
on("*@recorded", function(evt) ... end)  -- ワイルドカード
```

`@recorded` event payload: `{ kind, origin_system?, recorded_at, payload }` (kind/origin_system/recorded_at は **sealed**: `evt.payload.x = 1` は OK だが `evt.kind = "..."` は metatable で拒否される)

`@observed` event payload: 上記 + `{ observed_at, observer_empire, lag_hexadies, ... }`

### 投入

`gs:record_knowledge { kind, origin_system?, payload }` (前述の §6 Writer 参照) で Lua 起点の Knowledge を作れる。

---

## 13. ScriptEngine (Rust API、参考)

Lua 側からは直接見えないが、Rust テストや BRP `macrocosmo/eval_lua` で使う。

`scripting/engine.rs::ScriptEngine`:
- `new()` / `new_with_rng(rng)` / `new_with_scripts_dir(path)` / `new_with_rng_and_dir(rng, path)`
- `setup_globals(lua, scripts_dir)` (static, 後方互換 wrapper)
- `load_file(path)` / `load_directory(dir)` (※ `init.lua` から `require()` で読むのが推奨、これらは古い API)
- `lua()` — `&mlua::Lua` を返す
- `print_buffer()` — `SharedPrintBuffer` クローンを返す(LogBuffer drain 用)
- `scripts_dir()` — 解決済み絶対パス

スクリプトディレクトリ解決順 (`resolve_scripts_dir`):
1. `MACROCOSMO_SCRIPTS_DIR` 環境変数
2. 実行ファイルと同じディレクトリ
3. 実行ファイルの祖先(walk up して `init.lua` を探す)
4. CWD の祖先
5. `CARGO_MANIFEST_DIR`(コンパイル時に焼き込まれる、最終 fallback)

---

## 14. その他の規約

### `_def_type` タグ

すべての `define_xxx` 戻り値に付く識別タグ。

| 値 | 出典 |
|----|------|
| `"tech_branch"` / `"tech"` / `"building"` / ... | 各 `define_xxx` |
| `"forward_ref"` | `forward_ref(id)` |
| `"choice"` | `show_choice` の戻り値 |
| `"balance"` | `define_balance` |

ID 抽出 helper (`extract_id_from_lua_value`) は string・table どちらも受ける。

### Lua → Rust callback がない理由

`memory/feedback_rust_no_lua_callback.md` の通り、Rust 側から Lua function を呼ぶ際は **必ず `_pending_*` キュー経由**。`fire_event` の同期 dispatch hook は意図的に存在しない(reentrancy 防止)。

### Frame ごとに drain される `_pending_*` キュー一覧

| グローバル | drain system | 用途 |
|-----------|-------------|------|
| `_pending_script_events` | `event_system` 側 | `fire_event` のキュー |
| `_pending_notifications` | `drain_pending_notifications` | `show_notification` |
| `_pending_esc_notifications` | `drain_pending_esc_notifications` | `push_notification` |
| `_pending_choices` | `drain_pending_choices` | `show_choice` |
| `_pending_region_specs` | galaxy 生成 | `galaxy_generation.add_region_spec` |
| `_pending_knowledge_subscriptions` | `load_knowledge_subscriptions` | knowledge 購読(startup のみ drain) |

---

## 15. クイックリファレンス(関数名一覧)

定義系: `define_tech_branch` `define_tech` `define_building` `define_star_type` `define_planet_type` `define_biome` `define_predefined_system` `define_map_type` `define_region_type` `define_species` `define_job` `define_event` `define_knowledge` `define_slot_type` `define_hull` `define_module` `define_ship_design` `define_structure` `define_deliverable` `define_anomaly` `define_faction` `define_faction_type` `define_diplomatic_option` `define_casus_belli` `define_negotiation_item_kind` `define_balance`

参照: `forward_ref`

条件 atom: `has_tech` `has_modifier` `has_building` `has_flag` `target_state_is` `target_state_in` `target_standing_at_least` `relative_power_at_least` `target_allows_option` `actor_has_modifier` `actor_holds_capital_of_target` `target_system_count_at_most` `target_attacked_actor_core_within`

条件 combinator: `all` `any` `one_of` `not_cond`

条件 read: `check_flag`

トリガ: `mtth_trigger` `periodic_trigger`

イベント: `on` `fire_event`

ライフサイクル: `on_game_start` `on_game_load` `on_scripts_loaded` `on_galaxy_generate_empty` `on_choose_capitals` `on_initialize_system` `on_after_phase_a`

UI: `show_notification` `push_notification` `show_choice`

Effect descriptor: `effect_fire_event` `hide`

Map: `set_active_map_type` `galaxy_generation.add_region_spec`

組込: `print` (redirected) — `loadfile` / `dofile` は無効化済

Userdata 経由のメソッド呼び出し: `ctx:*` (`GameStartCtx` / `GalaxyGenerateCtx` / `ChooseCapitalsCtx` / `InitializeSystemCtx`), `scope:*` (`EffectScope`), `gs:*` (`GamestateScope`), `ctx.empire:*` (`ConditionCtx` / `ScopeHandle`), `graph:*` (`FtlGraph`), `system_handle:*` / `planet_handle:*`
