# Architecture Decisions (継続参照、将来判断に効く設計契約のみ)

旧 handoff 各本 (04-11 〜 04-14) のうち、既存 memory / CLAUDE.md / ソースコードで自明にならない **設計契約** を抽出して集約。完了 issue の再構成・操作ノウハウ・セッション経過は handoff には戻さず、対応する `memory/feedback_*.md` / `memory/project_*.md` / CLAUDE.md を参照。

最新の実装進捗・次セッション推奨は `docs/handoff-2026-04-15.md` を参照。

---

## 1. Workspace / 依存方向

- **`macrocosmo` → `macrocosmo-ai`** の 1 方向固定。`macrocosmo-ai` は `macrocosmo` / `bevy` の型を一切参照しない。
- CI workflow `ai-core-isolation` が `cargo tree -p macrocosmo-ai` で `bevy` / `macrocosmo` が出現しないことを検証。
- `macrocosmo-ai::mock` feature は **dev-dependency-only** (本番 feature 露出なし)。NPC 本番 decision tick は `NoOpPolicy` 登録、実 AI は `macrocosmo-ai::campaign/nash/feasibility` を後段で wire する (#189 配下)。

## 2. AI bus pattern (#195)

- `macrocosmo-ai` は **typed topic bus** として動作。3 topics:
  - **Metric** (数値観測)、**Command** (行動意思)、**Evidence** (証拠・出来事)
- `declare_*` で schema 宣言、`window/current/at_or_before` で query。
- **callback を ai_core に流さない** (一方向依存を厳守、`subscribe` は「bus がトピックを保持する宣言」であり callback 登録ではない)。
- 時間逆順 emit / 未宣言 emit / override は `log::warn + no-op(drop)` 方針。
- **Domain projector パターン**: AI は単一巨大アルゴリズムでなく、**ドメイン別 projector が bus に emit、feasibility formula で合成**:
  - 戦闘: Lanchester/Salvo + 閉形式 minimax (#190 保留、combat engine 成熟待ち)
  - 経済: trajectory projection + StrategicWindow 検出 (#191 実装済)
  - 外交: PerceivedStanding (#193 実装済)
  - 技術: 重み付き DAG priority (未実装)
  - 領土: influence / pressure map (未実装)
- 各 projector は pure fn、bus を読んで metric を吐く。
- memory: `project_ai_core_bus_architecture.md`

## 3. AI 情報制約

- AI も光速遅延を受ける (KnowledgeStore 経由のみ)。
- チートしない (他 faction の真値禁止)。
- 「古い情報で立てた計画が新情報到着で再評価される」ドラマが自然発生すべき。

## 4. 3 階層計画構造 (将来の AI 実装指針)

| 層 | 役割 | state persistence |
|---|---|---|
| **長期計画 (Grand Plan)** | Faction 単位の objective + adaptive assessment | stable objective, adaptive sub-state |
| **中期計画 (Campaign / Governor)** | 艦隊/星系/セクター単位 | intent 受信 + local_objectives + override_log |
| **短期戦術 (Tactics)** | 艦船単位、即応 | state ほぼなし |

委任パターン: 中期 AI = NPC 艦隊指揮官 = プレイヤー遠隔地 governor、**intent ベース通信**。

## 5. 光速制約のカバレッジ (core mechanic)

### 情報 (read side)

- **`KnowledgeStore`** が全遠隔系情報の single source of truth。UI/visualization は遠隔系を KnowledgeStore 経由で読む (直接 ECS 参照禁止)。
- **`PerceivedInfo<T> { value, last_updated, source }`** + `ObservationSource { Direct, Relay, Scout, Stale }`、`STALE_THRESHOLD_HEXADIES = 600`。
- **Scout > Relay 優先度**: `KnowledgeStore::update` 内で incoming/existing 両側で対称ルール。
- **Relay endpoint model**: `origin → nearest_relay (光速) → relay 網 FTL → nearest_relay_to_player (光速) → player`。`FTL_RELAY_MULTIPLIER = 10`、`floor(light_delay / multiplier)`。
- **ColonySnapshot atomicity** (#269): `build_system_snapshot` が 1 tick 内の全 colony 状態を atomic に書く。partial snapshot (buildings は T1、production は T2) を作らない。

### 命令 (write side)

- colony / system 建造命令は `PendingColonyDispatches` → `dispatch_pending_colony_commands` → `process_pending_commands` 経由で **全て光速遅延**。local (distance=0) も delay=0 で同じ pipeline を通す (1-frame UI latency は許容)。
- `RemoteCommand` schema (現行):
  ```rust
  enum RemoteCommand {
      BuildShip { design_id },
      SetProductionFocus { .. },
      Colony(ColonyCommand),        // building slot ops
      ShipBuild { host_colony, design_id, build_kind },
      DeliverableBuild { host_colony, def_id, display_name, cargo_size, minerals_cost, energy_cost, build_time },
  }
  struct ColonyCommand { scope: BuildingScope, kind: BuildingKind }
  enum BuildingScope { Planet(Entity), System }
  enum BuildingKind { Queue, Demolish, Upgrade }
  ```
- **到達時 compute 方針**: `ColonyCommand` + `ShipBuild` は ids + slot のみ運び、cost/time は arrival handler が registry + `ConstructionParams` で再解決。光速遅延中に empire modifier が変わったら現地条件が勝つ。
- **例外**: `DeliverableBuild` のみ payload 同梱 (deliverable defs は `StructureRegistry` 管理、arrival 側から引けない)。

### Fact pipeline (#249)

- **`EventId(u64)` + `NextEventId` resource** (counter) で 1 出来事 1 id。
- **`KnowledgeFact::*::event_id: Option<EventId>`** で fact と event を pair。
- **`NotifiedEventIds` tri-state**: missing = "既通知扱い" (safety net)、`Some(false)` = registered 未通知、`Some(true)` = 通知済。
- **`sweep_notified_event_ids`** system が毎 frame `true` entry を解放 (memory bound)。
- **`FactSysParam`** helper: `allocate_event_id` (allocate + register)、`record(fact, origin_pos, observed_at, vantage)` (内部で comms / relays を引く)。production callsite は 30 行 → 5 行に圧縮。
- **`auto_notify_from_events` whitelist**: `PlayerRespawn | ResourceAlert` のみ (世界出来事は fact delta 経由の notification、local は direct push の 2 系統)。

## 6. Definition schema 統一 (#226)

- 全 definition が `prerequisites: Option<Condition>` を持つ (Building / Hull / Module / ShipDesign)。
- `ShipDesignDefinition` は field を持たず、`ship_design_effective_prerequisites()` helper で hull + modules の AND 合成。
- `ModuleDefinition.prerequisite_tech: Option<String>` は **完全削除** (後方互換シムなし)。
- `build_tech_unlock_index` が全 definition を走査、Research パネル表示統一。

### define_structure vs define_deliverable (#223)

- **`define_structure`**: 世界にそのまま存在する構造 (debris, wrecks, anomalies、test fixtures)。
- **`define_deliverable`**: shipyard 建造 + Cargo 搬送 + Deploy pipeline 経由で置かれる構造。
- 内部表現は共通 `DeliverableDefinition` + `deliverable: Option<DeliverableMetadata>`。`deliverable.is_some()` が shipyard-buildable 判定。

### Ship design computed (feedback memory にも記載)

- hp/cost/speed/ftl_range/build_cost/maintenance は hull + modules から compute、Lua で authored された値は **warn-then-ignore**。
- `can_survey = survey_speed > 0`、`can_colonize = colonization_speed > 0` (capability flag 廃止、派生判定のみ)。
- Hull modifier も適用 (`courier_hull.ftl_range 1.2x`、`scout_hull.survey_speed 1.3x`)。

## 7. Job system (#241 + #245)

- **target string 規約 `job:<id>::<target>`**: Modifier struct 変更なし、target prefix で scope 表現。
- **Auto-prefix**: `define_job` 内の prefix なし target は load 時に `job:<self_id>::` 自動付与。
- **2 段階 ModifiedValue**:
  - Level 1: `rate(job, target)` per-pop
  - Level 2: `colony.<target>` aggregator
  - base_add/multiplier/add セマンティクス共通
- **Tech → empire → colonies broadcast**: `PendingColonyTechModifiers` + `sync_tech_colony_modifiers` (毎 tick、idempotent、id = `tech:<tech_id>:<target>`)。
- **Production base = 0 化** (4 spawn paths: `spawn_capital_colony` / `spawn_colony_on_planet` / `tick_colonization_queue` / colony ship 着陸)。resource_production_rate は `#[allow(dead_code)]` で一時保持。
- **`aggregate_job_contributions`** は **Stage 1 として独立 system、毎 Update で無条件に走る** (tick_production の `delta <= 0` 早期 return に巻き込まれない、PAUSE 中も UI coherent に保つ)。Startup chain にも流して初回 frame の UI 正確性を保証。
- **Multi-scope (`species:X,job:Y::`) は Faction #163 (multi-scope modifier) 側で検討**、v1 では扱わない。

## 8. Cargo / Deliverable

- `Cargo.items: Vec<CargoItem>` は既存 `cargo_capacity` を **共有プール** として使用。
- item は `GameBalance.mass_per_item_slot` (Lua 定数、default 1.0 Amt/slot) で Amt 質量に換算。
- 資源と item が同じ mass 空間で競合。

## 9. Save/Load contract (#247)

- **Wire format mirror approach**: live 型に serde derive を付けず、`SavedX` 構造を `src/persistence/savebag.rs` に集約。
- **postcard v1** シリアライザ採用 (bincode は 2026-01 頃 maintenance 終了、rkyv は ergonomic 重)。
- **`rand_xoshiro::Xoshiro256PlusPlus` 直使用** (SmallRng の内部非公開問題回避、bit-for-bit 継続保証)。
- **explicit `RemapEntities` trait** (serde custom serializer より debug しやすい)。
- **`SavedComponentBag` struct-of-Options** (multi-component entity 対応、issue の巨大 enum 案は不採用)。
- **Lua registries は save 対象外** (scripts 再ロードで復元)。
- **`tests/fixtures/*.bin`** (committed) が on-disk wire format を pin、`load_fixture(path)` helper で App を立ち上げる。`load_minimal_game_fixture_smoke` が SAVE_VERSION bump / savebag field 追加を検出。format bump 時は `regenerate_minimal_game_fixture` `#[ignore]` test で再生成。
- memory: `project_save_format_postcard.md`

## 10. Event / Lua gamestate (#263 → #332 pivot)

**現役設計: pure scoped closures (Option B)、#332 で実装済**。旧 snapshot + cache 路線 (#263 初期実装 / #320 leak fix / #328 cache) は本 pivot で obsolete。実装は `macrocosmo/src/scripting/gamestate_scope.rs`、`views` / `apply` submodule で read / write を分離。

### 最終決定 (2026-04-15)

- gamestate は **plain Lua table + `Lua::scope` で登録した `create_function` / `create_function_mut`** で構築、UserData 一切使わない
- `RefCell<&mut World>` を scope 全 closure で共有、read closure は `try_borrow`、write closure は `try_borrow_mut` で interior mutability
- **live read + live write**: read closure は呼出時に World から生データ → Lua table に組んで返す、write closure は &mut World 直 mutate
- **visibility contract**: event callback 内 mutation は **live within tick**。光速遅延 core mechanic は Rust side (`PendingCommand` / `PendingFactQueue`) で既に担保、Lua callback 内 mutation は内部一貫性の話で orthogonal
- **reentrancy 保護 2 階層**:
  - **規約レベル**: scope closure (read / write 両方) の body は **pure Rust**、Lua 変数・関数・メソッドを一切呼び出さない。Lua 側ロジックが必要なら Rust helper として切り出し、**Lua → Rust → World の 1 方向のみ** (`feedback_rust_no_lua_callback.md`)
  - **構造レベル**: `fire_event(...)` は sync dispatch 禁止、`_pending_script_events` queue に push 強制。`coroutine` lib は sandbox で無効化
  - これにより scope closure が別 scope closure を呼ぶ経路が構造的に存在しない、`try_borrow*` aliasing 衝突は現実的に発生しない (defense-in-depth として mlua error 変換は残す)
- **setter API**: `ctx.gamestate:push_empire_modifier(id, target, opts)` のような grouped method 形で提供、global 関数爆発を回避

### 歴史

#### 初期実装: snapshot-per-event (#263、PR #294)

Plan agent は live World view (`GameStateHandle<'w> { world: &'w World }` scoped UserData + child handle 返却) を設計したが、実装中に mlua 0.11 の制約発覚:

> (mlua `Lua::scope` docs) The lifetime of any function or userdata created through Scope lasts only until the completion of this method call, on completion all such created values are automatically dropped and Lua references to them are invalidated.

method 戻り値として scoped UserData を動的生成するのは、scope の「method 呼出終了で全 invalidate」保証を破るため不可能。child UserData 返却を諦めて **snapshot-per-event** (dispatch 時 world を 1 回走査して nested Lua table に固める) に pivot。

#### Leak fix (#320、PR #327)

snapshot build が 1 event あたり ~100 Lua ValueRef を生成、`payload.set("gamestate", snapshot)` で Lua 側に clone が入り GC 回らず累積。80 tick で `LUAI_MAXCSTACK` 枯渇 panic (release blocker)。**`evaluate_fire_conditions` + `dispatch_event_handlers` 両末尾に `lua.gc_collect()`** を挿入して minimum fix。

#### Cache 設計 (#328、close 済)

毎 event snapshot rebuild は性能劣化。`CachedGamestate { tick, RegistryKey }` で tick 境界 cache + "mutation は次 tick で反映" visibility contract を予定していたが、下記 Option B 発見で obsolete。

#### Option B 発見 (2026-04-15)

setter の戻り値が `()` なら scope 制約に抵触しないことに気づき、さらに **UserData を一切使わず `s.create_function` / `s.create_function_mut` の scope closure のみで gamestate を構築** できることを確認。Plan agent の #263 検討は UserData 路線に固執しており、この選択肢に到達していなかった。

Option B で一気に解決:

- **live read + live write 両方**実現、unsafe なし
- **snapshot build / cache / `gc_collect` すべて不要化**
- **Lua capture 耐性**: scope 終了で closure invalidate、capture 後参照は mlua runtime error
- **setter の namespace hygiene**: grouped method 形 (`ctx.gamestate:push_empire_modifier(...)`) で global 関数爆発を回避
- **reentrancy**: `_pending_script_events` queue 経由の event dispatch 規律で `RefCell` borrow 衝突を構造的に防ぐ

### 実装状態 (Phase A、#332 で land)

Phase A は event callback 経路 (`on(...)` bus handler / `on_trigger` / fire_condition) を live 化:

- `evaluate_fire_conditions` → `GamestateMode::ReadOnly` (setter は expose されない、spec 上 pure)
- `dispatch_event_handlers` → `GamestateMode::ReadWrite` (setter 経由で World を live mutate)
- 旧 `build_gamestate_table` / `attach_gamestate` 関連の `gc_collect()` (#320) は両 path から削除済、`stress_lua_scheduling` で 1000 tick 経過時の `final_memory` は ~95KB (ceiling 32MiB に対して 0.3%)

### Phase B 以降で live 化する hook (#332 残タスク)

- `on_game_start` / `on_game_load`: 初期データ seeding で World を直接いじる用途
- **effect declaration が目的の hook** (tech `on_researched` / faction action callbacks) は scope 外 context、引き続き `EffectScope` + `_pending_*` queue 経由

### 今後の拡張

- **光速遅延レンズ (`node:perspective(viewer)`)**: #215 PerceivedInfo を Lua に expose、ground truth との並走。Phase 2 で別 issue
- **unsafe raw pointer (app_data) 路線** は Option B が成立した時点で却下、Lua capture 耐性 + unsafe 審査コストのトレードオフで Option B が優位

### Visibility contract (invariant)

- **event callback 内 mutation は live within tick**: 同 callback 内で setter 呼出後に read 呼出すると即反映 (scope 閉路内で `&mut World` を同期 borrow)
- **mutation 路線の一元化**: 新規 event callback では `ctx.gamestate:push_*_modifier(...)` / `:set_flag(...)` を使う。旧 global `set_flag` / `modify_global` は **廃止予定** (Phase A では残置、Phase B で `EffectScope` 向け以外を削除)
- **fire_condition は読み取り専用**: 副作用を持つと評価順序依存で debugging 困難 → `GamestateMode::ReadOnly` で setter を expose しない
- **reentrancy 保護**: scope closure body は Lua 不接触の pure Rust。write helper (`apply::*`) は `&mut World` のみ受け、`mlua::Value` / `Function` / `RegistryKey` を persist しない (`memory/feedback_rust_no_lua_callback.md`)

memory: `project_lua_gamestate_api.md`、issue: **#332**

## 11. Scripting 設計原則

- **単一エントリポイント**: `scripts/init.lua` → require() で依存順に読み込み。
- **戻り値参照**: `define_xxx` が参照テーブルを返す、文字列 ID も後方互換。
- **Sandbox**: io/os/debug/ffi 無効、loadfile/dofile nil。
- **パス解決**: `MACROCOSMO_SCRIPTS_DIR` env → exe 隣 → ancestors → CWD ancestors → `CARGO_MANIFEST_DIR` (last-resort)。`init.lua` 存在で valid 判定。
- **Scoped Conditions** (既存): `ConditionAtom { kind, scope }`、`EvalContext` が named scope slot を持つ。`ConditionScope::Any` で ship→planet→system→empire 探索。Lua 側は static table と function (`function(ctx) return ctx.empire:has_tech("x") end`) 両対応。

## 12. 外交体系 v2 (2026-04-15 確定)

`docs/diplomacy-design.md` v2 が source of truth。`DiplomaticAction` enum / `define_diplomatic_action` Lua API は v2 で廃止予定:

- **DiplomaticOption** = label + Condition + event dispatch (pure dispatcher)
- **Faction.allowed_diplomatic_options** (1 set、actor/target 兼用) で option の可否を target 側が決める
- **NegotiationItemKind** を Lua-defined、merge/validate/apply を kind が所有
- **Casus belli** が war 全体を orchestrate (justification + base_demands + additional_demands + end_scenarios)
- **Single casus belli per war**
- **Faction type は preset** に格下げ、runtime は Faction instance が source of truth
- **Condition return による UI walk** で未充足理由を tooltip 表示
- **Inbox は ESC (#326) の 1 section** として分離

memory: `project_diplomacy_v2_spec.md`

## 13. Galaxy generation (3-phase)

```
empty → after_phase_a (connectivity, regions) → capitals → init
```

各段階で Lua フック (`define_map` / `on_after_phase_a` / `on_choose_capitals` / `on_initialize_system`)。

- **FTL connectivity bridge** (#199): capital → 全星系到達を保証、連結成分間の最短 FTL hop を bridge として追加。
- **FTL 不可領域** (#145): metaball `ForbiddenRegion` で plan_ftl_route がはじく。
- **`HostileFactions` resource** (`spawn_hostile_factions`) は `generate_galaxy` の前で seed (ordering flip #293)、galaxy は直接 `FactionOwner` を hostile entity に付ける。

## 14. UI / egui

- **6 chained systems in `EguiPrimaryContextPass`**: compute_ui_state → top_bar → outline/tooltips → main_panels → overlays → bottom_bar。各 system が独自 `EguiContexts`、共有データは `UiState` resource 経由。
- **`SystemParam` bundles** (`ui/params.rs`): Bevy の 16-param 制限回避。
- **`click_select_system`** は `full_test_app` から除外 (EguiContexts 必須)。

## 15. ROE 3 層

- **Aggressive**: 追撃あり (pursuit.rs detect system)
- **Defensive**: 反撃のみ
- **Retreat**: 敵対回避経路 (routing.rs の ROE weight)

ROE 変更は光速遅延で伝播。

## 16. 残る技術的負債

- **BuildingType enum**: capability-based への移行 (Lua 定義化)、既知 tech debt
- **`full_test_app`** 18 system 制約: 1 chain に詰められる system 数が上限に近い、今後追加時は chain 分割必要
- **Bevy 20-tuple chain 制限** (colony/mod.rs で経験済): 2 sub-tuple + outer `.chain()` で順序維持可
- **test parallelism flaky**: 一部 ship routing test が `--test-threads=1` でないと稀に flaky

---

## 関連ドキュメント

- **`CLAUDE.md`** — 開発規約 / pitfalls / 最新 module structure
- **`docs/game-design.md`** — ゲーム全体設計
- **`docs/diplomacy-design.md`** — 外交 v2 spec (source of truth)
- **`docs/ai-atom-reference.md`** — AI metric / command / evidence カタログ
- **`docs/handoff-2026-04-15.md`** — 最新の実装進捗・次セッション推奨
- **`todo.md`** — open issue 分類と着手順
