# Session handoff — 2026-04-26/27 AI decomposition + dispatch correctness

## TL;DR

3 ラウンド連続の大型セッション。 commit 範囲 `970db06 → ca5fcd6` で **24 commits** landing。 主な成果:

- **Bug fix sweep**: 序盤に user が報告した 3 bug + 関連を全部解決
  - 時間進まない (`RouteCalculationsPending` counter leak)
  - settlement の hostile gate が player 限定
  - visualization の ruler 位置が viewer-aware でなかった
  - 船建造しない (Lua capabilities 欠落)
  - Core 配備前 settle 失敗 loop (短期 filter で阻止)
  - Shipyard 量産 (capabilities fix で解消)
  - Refit が shipyard 不要 (gate 追加)
- **F-track 完了** (Closes #446 #447、 Partial #448): AI が `colonize_system` macro を `[deploy_deliverable, colonize_planet]` に分解し、 `deploy_deliverable` がさらに `[build_deliverable, load_deliverable, move, unload_deliverable]` に展開される full chain が動作。 Short layer に `PlanState` + `DecompositionRegistry` 導入
- **AI architecture spec phase**: Round 10+ の 3 layer geographic 分散 + light-speed inter-layer + player layered UI を設計、 5 issue (#448-452) 起票

73 test binary all green、 `cargo build --features remote` clean、 SAVE_VERSION 11。

前段: `docs/session-handoff-2026-04-26-round-7-9.md` (Round 7-9 まで)。 **本ファイルが Round 10 + spec phase の主ハンドオフ**。

## Recent commits (新しい順)

```
ca5fcd6 test(ai): e2e regression for colonize_system decomposition chain (H1)
56a89ae test(ai): end-to-end gameplay smoke for colonize_system decomposition
8950ba8 feat(ai): wire decomposition + Short emits primitives via PlanState (F4)
d8de79e feat(ai): game-side decomposition rules for colonize_system (F3)
c4e3458 feat(ai-core): DecompositionRegistry trait + Short consumer wiring (F2)
605237c feat(ai-core): PlanState type + Short input plumbing (F1)
0302395 feat(ai/consumer): wire deliverable handlers (E2)
e56cd20 feat(ai/schema): add deploy_deliverable family commands (E1)
adb4b6a fix(ai): filter colonizable_systems by Core presence
1a8fc76 fix(ship/refit): require shipyard for refit operations
003bce1 fix(scripts): add capabilities field to system buildings
8e64b30 feat(persistence): persist AiCommandOutbox + bump SAVE_VERSION 11
89d0cde feat(ai): wire AiCommandOutbox into Reason/CommandDrain pipeline
528a9ea feat(ai): AiCommandOutbox + PendingAiCommand types (no wiring)
942933b feat(ai-bus): push_command_already_dispatched re-entry method
caa64e7 test(ai/assignments): rewrite for knowledge-driven cleanup spec
21b947c fix(visualization/stars): viewer-aware update_star_colors
d803f1a fix(ship/settlement): per-faction hostile-presence gate
5e1352f style(workspace): rustfmt drift cleanup in ai/survey
1fa2c18 fix(time): replace RouteCalculationsPending counter with existence query
```

## Round 10 (本セッション内訳)

### A. Time fix — RouteCalculationsPending counter leak

`1fa2c18`。 `advance_game_time` の早期 return gate が `Resource { count: u32 }` 経路で leak 発見 (ship が `PendingRoute` 持ったまま despawn → counter 減らず → 時間永遠停止)。 counter 廃止、 `Query<(), With<PendingRoute>>::is_empty()` ベース存在 check に変更。 構造的に leak 不可能。

### B. Round 9 のキャリーオーバ

- `d803f1a`: `process_settling` の hostile gate を player→ship 自身の faction に
- `21b947c`: `update_star_colors` を ViewingEmpireResolver + ViewerRulerLookup ベースに

### C. AI command 光速遅延 (PR #3)

`942933b → 528a9ea → 89d0cde → 8e64b30`。 4 commits で `AiCommandOutbox` Resource + `PendingAiCommand` 型 + dispatch/process system + persistence。 既存 `compute_fact_arrival` 再利用で relay-aware delay。 spatial-less command (`research_focus` 等) は ruler→capital で delay 計算。 cycle-safety は `AiBus::push_command_already_dispatched` re-entry method で。 SAVE_VERSION 10 → 11、 fixture regen。

### D. Bug 報告 3 件の fix

序盤 user 報告:
1. AI が船を建造しない → `003bce1` (production Lua の `capabilities` 欠落 → `can_build_ships=0` 永続 → Rule 6 dormant)
2. Core 配備前 settle 失敗 loop → 短期 `adb4b6a` で `colonizable_systems` を 自 Core あり filter
3. Shipyard 量産 → `003bce1` で `can_build_ships=1.0` になり Rule 5a 静まる

副次: `1a8fc76` で refit に shipyard gate (新造船は要 shipyard なのに refit は素通りだった不整合)。

### E. F-track (E + F combined)

7 commits で landing。 #446 (deploy_deliverable schema) + #447 (Short PlanState + decomposition) + Partial #448 (G の trait 統一は別 round)。

#### E (#446) schema + handlers
- `e56cd20`: `build_deliverable` / `load_deliverable` / `unload_deliverable` (primitive) + `deploy_deliverable` (macro) + `colonize_planet` (primitive) を `ai/schema/ids.rs` に追加
- `0302395`: `command_consumer.rs` に handlers 配線。 既存 `LoadDeliverableRequested` / `DeployDeliverableRequested` event の AI bus tap (Lua emit path 共存)。 `CommandStamp` SystemParam で 16 param 上限回避

#### F (#447) Short decomposition
- `605237c` F1: `PlanState` 型 (`BTreeMap<(CommandKindId, ObjectiveId), Vec<Command>>`) + Short input に thread。 `OrchestratorState.plan_states: AHashMap<ShortContext, PlanState>` で per-Short 永続
- `c4e3458` F2: `DecompositionRegistry` trait + `StaticDecompositionRegistry` (BTreeMap-backed) + `ShortTermInput.decomp` 経由 thread
- `d8de79e` F3: game-side `build_default_registry()` で 2 rule 登録 — `colonize_system → [deploy_deliverable, colonize_planet]`、 `deploy_deliverable → 4 primitives`
- `8950ba8` F4: `CampaignReactiveShort::tick` を 2 段階 factor (`build_raw_commands` + `intercept_and_drain`)。 macro を eager flatten で recursive expand (depth-16 guard)、 `PlanState.pending` に push、 各 tick で head 1 件 drain。 macro re-emit dedup (`(macro_kind, objective)` slot 既 non-empty なら skip)。 **precondition gate** = `fn(&PlanState, Tick) -> bool` 形 (default `always_allow_gate`、 future precondition gating の hook を残す TODO)
- `56a89ae`: 5-tick `CampaignReactiveShort` smoke test
- `ca5fcd6` H1: full Bevy app e2e regression。 NPC empire spawn → `OrchestratorRegistry` 手動 insert → 5 tick 進行で `BuildQueue / LoadDeliverableRequested / MoveRequested / DeployDeliverableRequested / ColonizeRequested(planet=Some)` event sequence 全 assert

## 起票した Issue (Round 11+ で着手)

| # | 内容 | depends-on | priority |
|---|---|---|---|
| #445 | shipyard_capacity 値が活用されてない (multi-shipyard parallelism なし) | — | medium |
| #446 | AI に deploy_core 系 commands 追加 — **CLOSED by F-track** | — | done |
| #447 | Short Agent で colonize_system を分解 — **CLOSED by F-track** | #446 | done |
| #448 | 3 layer FSM trait 統一 + SimpleNpcPolicy 削除 — Partial | #447 | medium |
| #449 | Region 概念 + Mid/Short geographic instance 化 | #448 | medium |
| #450 | Inter-layer light-speed comm (Directive 型 + outbox 経由 routing) | #449 | medium |
| #451 | Mid-Mid handoff (auto-accept + move-with-handoff) | #449, #450 | medium |
| #452 | Player layer-aware UI (Region Manager + Command Center + auto toggle) | #449 | medium |

## **未 fix bug (次セッション最優先)** — playtest で観察された AI dispatch correctness

セッション後半で BRP 観察 (`cargo run --features remote` + `world.query`) により以下 3 bug 確定:

### Bug A: 重複 survey command race (= dedup 機構の盲点)

**症状**: Vesk Scout-1 と Scout-2 が **同 target system 4294966767** に dispatched (departed_at 2200 / 2363、 163 hex 差)。 dedup 機能してない。

**Root cause** (確定):
- `npc_decision_tick` の dedup は `Query<&PendingAssignment>` のみ参照
- `PendingAssignment` は **handler 走行時に挿入** (= AI emit から outbox 経由 light-speed delay 後)
- AI emit と handler insert の間 (= outbox 内 in-flight) は marker 不在
- mid_cadence=2 で 2 tick 毎 npc_decision 走るので、 outbox 内 in-flight な command は **後続 decision tick で見えない**
- 結果: 同 target に 2 emit、 両 handler insert で marker 並存、 両 ship 同 target に派遣

**Fix 案**:
- (a) emit 時 marker insert: AI bus emit と同時に world に PendingAssignment 書く → 一貫性高いが engine-agnostic 性破壊
- (b) **outbox を dedup 入力に追加**: `npc_decision` で `AiCommandOutbox.entries` も読み、 in-flight command の target を `pending_survey_targets` に含める → engine-agnostic 維持、 1 query 追加で軽量
- (c) hybrid: emit 時 light marker、 handler で full marker に upgrade

**推奨** = (b)。 **未確定**。

### Bug B: hostile-known system へ AI が凸る (= ROE 無視)

**症状**: Aurelian が System-072 を hostile (`has_hostile=true`) と認識済 (= 戦闘で scout 死亡)、 にも関わらず再 scout 派遣 → 全滅。 「行方不明になった場所に凸って全滅ループ」

**Root cause**:
- `npc_decision.rs:540-563` で `colonizable_systems` 構築時 `has_hostile` filter なし
- `:606` で `unsurveyed_systems` (= `rank_survey_targets`) も hostile filter なし
- AI の Rule 1 (attack) のみ hostile を targets として使い、 Rule 2 (survey) / Rule 3 (colonize) は素通り

**Fix scope**:
- 短期: `colonizable_systems` + `unsurveyed_systems` に `!has_hostile` filter 追加 (1-2 行 + regression test)
- 中期: **ThreatState 機構**:
  - `Suspected { since, reason }` (ship missing 段階、 確証なし、 後続 destruction log 待ち)
  - `Confirmed { observed_hostile }` (全滅 log 到達で has_hostile=true 確定)
  - ROE 別挙動: Retreat = Suspected + Confirmed avoid、 Defensive = Confirmed avoid、 Aggressive = engage
  - player UI で manual avoid flag 立てる仕組みも必要 (#452 K の一部)
- 長期: `KnowledgeFact::ShipDestroyed` の per-faction propagation (現在 player-only、 `knowledge/mod.rs:1326`)

### Bug C: SURVEY_ASSIGNMENT_LIFETIME = 200 hex が短すぎる

**症状**: 実 SubLight 移動が 798 hex (片道、 観察値) → survey 30 + return 798 + propagation = 計 ~1700 hex。 stale_at=200 は mid-flight で expire → marker 消えて Bug A の race を誘発。

**Fix 方針** (user 指示):
- `sweep_stale_assignments` システム自体を **削除**
- knowledge-driven cleanup (`sweep_resolved_survey_assignments`) のみに依存
- ship が despawn (combat 等で死亡) した場合 → Bevy 自動 component cleanup で marker も消える
- 「ship 生きてるが永遠に帰投しない」 corner case は `KnowledgeFact::ShipDestroyed` の per-faction propagation 後に再検討

これで Bug A も部分軽減 (stale で消える window が消滅 → outbox race window 内のみが問題)。

## 次セッション再開プロンプト例

```
2026-04-27 ハンドオフ参照。 まず docs/session-handoff-2026-04-27-ai-decomposition.md
読んで全体像把握。 今日は AI dispatch correctness 3 bug を順次 fix:

1. Fix B (hostile filter) を 即 agent 投入 — 1-2 行 + regression test、 確実な短期効果
2. Fix C (sweep_stale 削除) を続けて agent — assignments.rs から system 削除 +
   plugin.rs から登録削除 + 既存 test re-baseline
3. Fix A (outbox-aware dedup) は (b) 案 = npc_decision に AiCommandOutbox query
   追加で in-flight command を dedup 入力に。 設計判断要なら spec phase に戻る

並行: ThreatState 機構 (Suspected / Confirmed) は中期 design、 issue 起票して着手判断。
```

## 観察手順 (BRP via remote feature)

`cargo run --features remote` で起動、 port 15702。

### 重要な BRP query

```bash
# AI command outbox (in-flight commands を観察)
curl -s -X POST http://localhost:15702 -H "Content-Type: application/json" -d '{
  "jsonrpc":"2.0","id":1,"method":"world.get_resources",
  "params":{"resource":"macrocosmo::ai::command_outbox::AiCommandOutbox"}
}'

# 全 PendingAssignment marker
curl -s -X POST http://localhost:15702 -H "Content-Type: application/json" -d '{
  "jsonrpc":"2.0","id":1,"method":"world.query",
  "params":{"data":{"components":["macrocosmo::ai::assignments::PendingAssignment"]}}
}'

# ship state + queue
curl -s -X POST http://localhost:15702 -H "Content-Type: application/json" -d '{
  "jsonrpc":"2.0","id":1,"method":"world.query",
  "params":{"data":{"components":[
    "macrocosmo::ship::Ship",
    "macrocosmo::ship::ShipState",
    "macrocosmo::ship::CommandQueue"
  ]}}
}'

# empire 単位の KnowledgeStore (entries が surveyed/has_hostile/colonized 持つ、
# system_knowledge ではないので注意)
curl -s -X POST http://localhost:15702 -H "Content-Type: application/json" -d '{
  "jsonrpc":"2.0","id":1,"method":"world.get_components",
  "params":{"entity":<empire_entity>,"components":["macrocosmo::knowledge::KnowledgeStore"]}
}'
```

### 落とし穴

- `KnowledgeStore` の data field 名は `entries` (`system_knowledge` ではない)
- `BuildingRegistry.buildings` は HashMap で BRP serialize で空に見える (実際は populated)
- `--features remote` で再 build 後、 **走ってる game process を必ず再起動** しないと新 binary 反映されない
- Reflect 経路で見えない component / resource もある (`#[reflect(ignore)]` 付与箇所)

## 確定済 architectural design (Round 11+ 着手用)

### 3 layer geographic 分散 (Issue #449)

```
empire 内
├─ Long Agent (1)              [位置: Ruler.StationedAt]
│   └─ 全 Mid に directive 送信 (light-speed delay)
├─ Mid Agent (N)               [位置: 各 Region capital]
│   ├─ 自管轄 Short 集合を保持 (authority registry)
│   └─ 配下 Short に directive 送信 (light-speed delay)
└─ Short Agent (M)             [位置: 個別 unit / colony / fleet flagship]
    ├─ macro decompose して primitive emit
    └─ active_plans: Vec<PlanState>
```

### Region 概念

- 初期 empire = 1 region (= empire 全体)
- 後で player が手動分割 (Region Manager UI、 #452)
- 自動 prompt 条件: region capital からの **best-effort comm time** (= `compute_fact_arrival` の relay-aware 結果、 courier dispatch 除外) > 閾値
- 命名は仮 `region` (sector / constellation / 他 candidates あり、 後決め)

### Player 統合

- **Player input ≡ Long emit** (player 側に Long instance なし)
- Mid/Short には auto_managed: bool flag、 player UI で toggle
- player 命令 routing: target 指定 → target の owner Mid/Short に inject (auto なら override)
- player の物理位置で自然に layer 決定: capital = Long、 region capital = Mid、 unit local = Short
- 既存 `PendingCommand` (player command light-speed) は **Player の Short-level 命令** の特例として再解釈可能

### Inter-layer message 型 (Issue #450)

```rust
pub struct Directive {
    pub kind: DirectiveKind,  // strategic / tactical / operational
    pub payload: AiCommand,   // 既存 AiCommand を再利用
}
```

`AiCommandOutbox` 拡張 (or 新 `DirectiveOutbox` 並列) で routing。 origin/destination Position から `compute_fact_arrival` で arrives_at 計算。

### Failure modes

- **Ruler 失陥**: capital 残存で respawn 続行 (既存メカニクス踏襲)、 capital 喪失で敗北
- **Mid 失陥**: respawn 可なら別 location respawn、 不可なら最近接 Mid takeover
- **Short orphan**: Mid 失陥時 Long が再 assign (light-speed delay)

### Mid-Mid handoff (Issue #451)

- auto-accept (拒否なし、 v1 では reject mechanic 不要)
- `MoveWithHandoff { fleet, target_system, handoff_to_mid }` で fleet 移動 + handoff を 1 trip 化

### SimpleNpcPolicy 廃止

- Issue #448 G3 で削除
- 既存 logic を Mid (stance: `Expanding/Consolidating/Defending/Withdrawing` 4 値) と Short (PlanState 拡張) に分散

### ThreatState (上記 Bug B 関連、 0.4.0 級)

```rust
pub enum ThreatState {
    Clear,
    Suspected { since: i64, reason: SuspicionReason },
    Confirmed { observed_at: i64, hostile_faction: Option<Entity> },
}

pub enum SuspicionReason {
    ShipMissing { ship: Entity, sent_at: i64 },
    PlayerFlag,  // manual override (UI、 #452)
}
```

per-empire HashMap<system_entity, ThreatState>。 ROE 別挙動 + UI override。

## 注意点 / 落とし穴

- **Lua API 経由 emit (`gamestate_scope.rs`) と AI bus emit が同 event に書く**: `LoadDeliverableRequested` / `DeployDeliverableRequested` 等。 handler 接続変更時に Lua path を壊さないこと (event writer 追加で OK、 既存 reader 維持)
- **F-track の precondition gate は no-op default**: `always_allow_gate` 固定。 future の依存制御 (例: 「load_deliverable は build_deliverable 完了を待つ」) には PlanState 単独では情報不足、 AiBus or per-slot expected fact set が必要 — 別 issue
- **NPC `CommsParams` が compute_fact_arrival に渡されてる** (PR #3 で TODO #4 同時解決): inter-layer comm 設計時もこの per-empire CommsParams を使う前提
- **AiCommandOutbox の persistence**: `SerializedPendingAiCommand` shim 経由 + `Entity` references は `to_bits()` で raw u64 化 → 既存 `EntityMap` 機構が remap、 明示 remap pass 不要
- **macrocosmo-ai/src/spec.rs canonical kinds は存在しない**: command id の SoT は `macrocosmo/src/ai/schema/ids.rs::command`
- **`process_surveys` の SystemParam が肥大**: 既に 13 + bundle で限界近い、 次に追加するなら別 bundle 化必須 (前段 handoff 注意点踏襲)
- **flaky test 1 件**: `survey_command_outbox_holds_until_light_delay_elapses` (~30-50% intermittent fail)。 isolation で pass、 pre-existing nondeterminism、 上記 Bug A/C 修正後に再評価

## 次セッション最優先

1. **Bug B (hostile filter)** — 1 commit、 即 agent 投入で短期 fix
2. **Bug C (sweep_stale 削除)** — 1 commit、 system + plugin 登録 + test re-baseline
3. **Bug A (outbox-aware dedup)** — 設計判断後 1-2 commit、 (b) 案推奨だが user 確認要

並行: **ThreatState 機構** (Suspected / Confirmed) を Issue 起票 → Round 12 着手検討。

その後: **Issue #448 (Agent trait 統一)** から Round 11 着手。 Plan agent draft 済 (本ハンドオフの「確定済 architectural design」 section に集約)。
