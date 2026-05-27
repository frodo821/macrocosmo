# Session handoff — 2026-05-25 Round 16: AI build routing + economy hotfix series + Omniscient mode

## TL;DR

Round 15 (`docs/handoff/2026-05-24-round-15-ai-courier-delay.md`) からの連続セッション。 main 上の commit 範囲 `3e4af27 → cd08a7d` で **7 PRs / 15 commits**、 全 squash + merge。 主成果:

- **#470 closed** — `queue_ship_at_shipyard` / `handle_build_deliverable` が StarSystem entity 上の `BuildQueue` を探していたが、 `BuildQueue` は Colony entity にしか attach されない → 全 AI 船建造命令が `debug!` で silent drop。 host colony pick + warn promotion + ship dedup で根本 fix。 PR #510。
- **#445 closed** — `shipyard_capacity` が 7 callsite で binary 評価 (`> Amt::ZERO`) されており 2 個目以降の shipyard が完全に inert。 modifier 鏡へ migrate (`shipyard_build_parallel_slots`、 `shipyard_build_speed`)、 `tick_build_queue` の parallel slot 処理 + 分数 accumulator + per-slot funding gate を導入。 PR #511。
- **#490 closed** — Omniscient (god-view) observer mode を `ObserverMode { kind: ObserverModeKind }` 3-variant enum (`Disabled` / `EmpireView` / `Omniscient`) に拡張。 F9 toggle + `NonOmniscientKind` newtype による型レベル restore-loop 防止 + #499 contract 維持。 PR #512。
- **#444 closed** — starter NPC empire (1 region = {capital}、 surveyed + colonized + cored) の **region deadlock** を Mid Rule 3.5 (frontier `deploy_deliverable`) + Rule 2 region-scope filter 撤去 + `dispatch_ai_pending_commands` eager macro decomposition + `infra_core` → `infrastructure_core` typo fix の 4 連結で破壊。 PR #528。
- **Hotfix #2 (PR #530)** — `#490` fold-in regression 2 件: F9 default binding が `register_engine_defaults` から消えていた + 「`deploy_deliverable` 展開 chain の trailing primitive (`unload_deliverable`) が先行する `reposition` extrapolation を clobber」 = map 上で AI ship が動かないバグ。 `intended_state.is_none()` で projection write skip、 9-kind audit pin test 追加。
- **#529 epic 起票 + Hotfix #3 (PR #531) merged** — brp QA で「starter empire が数百 hex で破産」 capture。 Rule 6 fleet_composition semantics fix (= surveying ship を census に含める)、 soft resource gate (Rules 3.5 / 5a / 5b / 6)、 planet-building dedup、 さらに #529 A migration として **pending-aware resource gate** (build queue 内の `(cost - invested)` を stockpile から差し引いて gate)。 region-scope codex review fold-in: colony build queue 走査を `member_systems_set` で gate (region B の pending が region A の `current_minerals` を下げない)、 `ShortAgentTickInputs` を per-region キーへ (`EmpireShortInputs` → `RegionShortInputs`)。
- **`SAVE_VERSION = 20` 維持** (全 changes runtime-only / postcard additive)。
- **最終 test status**: `cargo test --workspace --tests -- --test-threads=1` → **3652 passed / 0 failed** (Round 15 末 3579 → +73)。

新規 issue 起票: **#513-#527 + #529** 計 16 件 (大半は #445 / #470 / #490 / #528 / #531 follow-up を切り出した backlog ticket)。 #529 のみ大型 epic。

前段: `docs/handoff/2026-05-24-round-15-ai-courier-delay.md`、 `docs/session-handoff-2026-04-29-round-14-light-coherence-completion.md`。

## Commit 順 (chronological、 origin/main)

```
603e7b3 fix(ai): #470 queue AI ship build orders on host colony's BuildQueue (was StarSystem, silent fail)
b67db5d Merge pull request #510 from frodo821/fix/470-build-queue-colony-routing
69f07af fix(balance): #445 shipyard parallel slots + speed multiplier (capability→modifier migration)
a369ce1 feat(ui): #490 add omniscient (god-view) observer mode as 3-variant enum
6d326f7 fix(ui): #490 fold-in — delete enabled(), fix Omniscient bugs, add NonOmniscientKind newtype
c75f943 Merge pull request #512 from frodo821/feat/490-omniscient-observer-mode
3e81438 Merge pull request #511 from frodo821/fix/445-shipyard-parallel-slots
b2764ac fix(ai): #444 land deploy_core path + frontier scope to break region deadlock
fb7b0ac Merge pull request #528 from frodo821/fix/hotfix-ai-region-deadlock
fe798c8 fix(ui/ai): #490 fold-in F9 binding regression + dispatch extrapolation write fix (visualization hotfix)
3a6a172 fix(ai): resource gate + Rule 6 fleet_composition semantics (resource starvation hotfix)
9dd77d6 fix(ai): #529 A migration — pending-aware resource gate (PR #531 fold-in)
ce4d176 Merge pull request #530 from frodo821/fix/hotfix-2-visualization-extrapolation
89194e5 fix(ai): #529 A region-scope codex review fold-in (PR #531 finding 1+2)
cd08a7d Merge pull request #531 from frodo821/fix/hotfix-3-resource-gate
```

PR 一覧:

| PR | issue | scope | merge |
|---|---|---|---|
| #510 | #470 | AI build → host colony BuildQueue routing + dedup | 2026-05-24 |
| #511 | #445 | shipyard parallel slots + fractional speed accumulator | 2026-05-24 |
| #512 | #490 | Omniscient (god-view) observer mode 3-variant enum | 2026-05-24 |
| #528 | #444 | region deadlock 4-fix bundle (frontier scope + eager macro decomp + typo) | 2026-05-25 |
| #530 | #490 fold-in (hotfix #2) | F9 default + dispatch extrapolation skip | 2026-05-25 |
| #531 | #529 (新 epic) | resource gate + Rule 6 census + #529 A migration (pending-aware) + region-scope fold-in | 2026-05-25 |

## Round 16 内訳

### A. PR #510 — #470 AI build → host colony BuildQueue routing

#### 根本 bug

`queue_ship_at_shipyard` と `handle_build_deliverable` が `br.build_queues.get_mut(sys_entity)` を **StarSystem entity** に対して呼んでいた。 `BuildQueue` Component は **Colony entity** にしか attach されない (`setup/mod.rs:774`、 `colony/colonization.rs:203`、 `ship/settlement.rs:261` 参照)。 → 全 AI 船建造命令が `Err` を返し `debug!` で握りつぶされ、 NPC empire は starter fleet 消耗後ゼロ船建造。

#### Fix

- **`pick_host_colony` helper**: chosen shipyard system 内 colony を walk し、 `FactionOwner == issuing empire` の最初の colony を pick。 player UI の `host_colony` pattern に倣う + `FactionOwner` チェックで stricter (= 征服 system で `Sovereignty.owner != colony.FactionOwner` の場合に対応)。
- `BuildResearchParams.build_queues`: `Query<&mut BuildQueue>` → `Query<(Entity, &Colony, &FactionOwner, &mut BuildQueue)>` に widen。
- silent fail (`debug!`) → `warn!` promotion。 #470 が見えなかった元凶。
- `BuildKind::Ship + design_id` dedup を `queue_ship_at_shipyard` に追加 (`BuildKind::Deliverable` 既存 dedup を mirror)。 mid_stance Rule 6 の毎-Reason-tick re-emission による 30 倍 stack を防止。

#### Tests

- `macrocosmo/tests/ai_ship_build_queue.rs` 新規 5 tests:
  - happy path (build_ship が host colony に着地)
  - end-to-end (tick_build_queue が build_time + minerals/energy drain)
  - `fortify_system` auto-pick combat design routes to host colony
  - 征服 system reject (wrong-owner colony で no panic、 refuse)
  - dedup (same-design re-emission → 1 order)
- 4 既存 inline unit test + 1 e2e fixture (`ai_decomposition_e2e.rs`) を `(StarSystem, BuildQueue)` → `(Colony, FactionOwner, BuildQueue)` に migrate

3590 → 3591 pass。

### B. PR #511 — #445 shipyard parallel slots + capability→modifier migration

#### 根本 bug

`SystemModifiers.shipyard_capacity` の値が 7 callsite で `> Amt::ZERO` の binary 評価。 2 個目以降の shipyard が完全に inert。 build スループットが 1 系統 1 slot で頭打ち。

#### Fix (3 軸)

##### 1. Modifier rename + new field (`galaxy/mod.rs`)

- `shipyard_capacity` → `shipyard_build_parallel_slots` (semantic-explicit、 base = 0、 N shipyards = +N)
- 新 `shipyard_build_speed: ScopedModifiers` (base = 1.0、 multiplier identity)。 shipyard 本体は speed に寄与しない。 mass-production tech / module 用 reserved。

##### 2. `tick_build_queue` parallel processing (`colony/building_queue.rs`)

- 従来: head order のみ処理。 現在: 最大 N orders parallel。
- Speed multiplier の分数 accumulator: 新 `Resource ShipyardSpeedAccumulators(HashMap<Entity, Amt>)` (runtime-only、 mid-tick fractional state は save しない)。
  - `raw_progress = delta * speed_mult` (Amt fixed-point ×1000)
  - 整数 part → `effective_delta` で build_time decrement
  - 分数 remainder → 次 tick に carry over
  - 0.3× debuff at delta=1 → 3 tick 蓄積で 1 hex progress (= 旧 "floor 1" fallback で sub-1.0 が silently nullified だった bug 是正)
- Per-slot cost share = `cost / build_time_total`。 stockpile が 1 slot share を funding できなければ build_time decrement なし (= slot 2+3 が tick down → 負 build_time → 補給瞬時完了の greedy starvation を close)。
- `build_time_remaining` ≥ 0 で clamp。
- shipyard 全失で accumulator drop (= 無効中 free progress 蓄積防止)。

##### 3. Capability path → modifier path migration (7 callsite)

- `colony/system_buildings.rs`、 `colony/building_queue.rs`
- `deep_space/mod.rs::relay_knowledge_propagate_system`
- `ai/emitters.rs`、 `ai/command_consumer.rs`
- `ui/mod.rs`、 `ui/system_panel/mod.rs`、 `knowledge/mod.rs`

全部 `system_modifiers.shipyard_build_parallel_slots.final_value() > Amt::ZERO` で判定。 shipyard の Lua `capabilities = { shipyard = {} }` 定義は backward compat 用に **retain** (test fixture + modder 定義のため)。 削除は follow-up #513。

#### AI metric semantics 修正 (`ai/emitters.rs`)

- `systems_with_shipyard` REVERTED to **set-count** (= ≥1 shipyard を持つ system 数)。 一時 slot-sum に inadvertently 変更されており Rule 5/6 consumer の `>= 1.0` 比較を壊しかけた。
- 新 `total_shipyard_slots` metric が sum を carry。
- UI debug panel (`ui/ai_debug/governor.rs`) で `total_shipyard_slots` を surface。

#### Tests (`tests/shipyard_parallel_slots.rs` 8 tests)

- 2 parallel orders progress simultaneously
- 1 parallel order only head progresses
- 0 shipyard no progress
- build_speed default 1.0
- build_speed multiplier 2x speeds progress
- parallel cost consumption matches slot count (fold-in)
- build_speed half progress accumulates over 2 ticks (fold-in)
- parallel starvation does not advance build_time (fold-in)

3596 + 3 fold-in = **3599 passed / 0 failed**。

#### 起票した follow-up

- #513 shipyard capability の Lua 定義削除
- #514 Sovereignty owner attribution symmetry (`tick_build_queue` の host colony FactionOwner vs AI emitter の system Sovereignty)
- #515 `BuildKind::Deliverable` vs `BuildKind::Ship` を separate slot pool に分ける
- #516 `tick_build_queue` SystemParam bundle (16-param ceiling 接近)
- #517 `shipyard_build_parallel_slots` rename (deprecation window 後)
- #518 per-frontier shipyard strategy — Rule 5a で `total_shipyard_slots` metric 活用
- #519 `shipyard_build_speed` Lua 駆動 additive modifier (mass-production line / assembly automation tech)

### C. PR #512 — #490 Omniscient (god-view) observer mode

#### 元の `ObserverMode { enabled: bool }` 廃止

- 新 3-variant `ObserverModeKind`:
  - `Disabled` — PlayerEmpire view (default)
  - `EmpireView` — spawn-architecture observer mode (`--no-player` / `--observer`)、 viewing empire の `KnowledgeStore` 経由 (= #499 light-coherent contract)
  - `Omniscient` — god view、 全 `KnowledgeStore` 無視で realtime ECS 直読 (dev-only F9 toggle)
- `resolve_viewing_empire(&World)` は Omniscient で `None`。
- `resolve_viewing_knowledge` / `empire_view_knowledge` の `_omniscient` variant 追加 (Omniscient で `None` collapse、 panel が realtime ground-truth path へ落ちる)。
- `ui.toggle_omniscient` action (default F9)、 `Omniscient` ↔ `previous_kind` flip。

#### `NonOmniscientKind` newtype (= 型レベル restore-loop 防止)

`ObserverMode::previous_kind: Option<NonOmniscientKind>`。 `from_observer_kind` constructor が `Omniscient` を reject → 「previous = Omniscient」 を作る future code path が type-checker で nil。

#### #490 fold-in (PR-3 内 amend、 = 5 bug BLOCKER + 3 design BLOCKER + 7 nice-to-fix)

- **BUG 1**: `compute_ui_state` が Omniscient (normal session に F9 toggle) で `ObserverView.viewing` を `None` 化 → top-bar resources が zero。 Omniscient は PlayerEmpire path へ routing。
- **BUG 2**: `auto_pause_on_event` が Omniscient でも発火し続ける問題。 spawn-mode (`--no-player` / `--observer`) のみ drain-and-bail。
- **BUG 3**: `hostile_systems` が Omniscient で ground truth に落ちる。 EmpireView は observed empire の `KnowledgeStore` 経由 (= #499 contract)。
- **BUG 4**: god view が全 empire の ship を `draw_ships_omniscient` で render。
- **BUG 5**: `ViewingEmpireResolver::is_god_view` を Omniscient-only に。 #499 EmpireView contract 復旧。
- **DESIGN 1**: `previous_kind` を `NonOmniscientKind` newtype に。
- **DESIGN 2**: `ObserverMode::enabled()` 削除、 全 callsite を specific predicate (`is_empire_view()` / `is_omniscient()` / `is_any_observer()`) に書き換え。
- **DESIGN 3**: `is_god_view` docstring を Omniscient-only semantics に揃える。
- 5a: `toggle_omniscient_mode` を `in_state(GameState::InGame)` で gate (main-menu / loading で inert)。
- 5b: F9 default keybinding 削除 (← この変更が後で hotfix #2 で巻き戻し)
- 5c: dead `is_omniscient_mode` wrapper 削除
- 5d: top-bar empire selector を Omniscient で hide
- 5e: 8 new regression test (16 → 24 in `tests/observer_mode_omniscient.rs`)
- 5f: `omniscient_realtime_state_leaks_through_helper_one` を `omniscient_helper_returns_none_for_realtime_fallback` に rename (旧名は contract violation を含意していた、 test は correct behaviour pin)
- 5g: `in_observer_mode` / `not_in_observer_mode` を `is_empire_view()` 意味に audit (`is_any_observer()` ではない)。 F9-Omniscient on normal session で `esc_to_exit` / `check_time_horizon` が silently arm されないように

#### 起票した follow-up

- #525 ObserverMode 3-way branch を `ResolveTarget` enum に collapse
- #526 mode contract docstring を canonical block に集約
- #527 `ObserverMode.kind` を save/load 永続化

### D. PR #528 — #444 region deadlock 4-fix bundle

#### 根本 bug

starter NPC empires (region = {capital}、 capital surveyed + colonized + cored) で 19 game year passive idle in dock。 4 共役 bug:

1. **Rule 3 (colonize) starve**: own-Core gate を通る surveyed-but-uncolonised system が region に存在しない
2. **Rule 2 (survey) region-scope filter**: candidate set が {capital} (= 既に surveyed) に pin
3. **Mid 層 `deploy_deliverable` macro が decompose されない**: Short 層の `CampaignReactiveShort` は自分の campaign が emit した command のみ見る、 bus-emitted Mid 出力は never see
4. **`infra_core` typo**: 既存 rule (`decomposition_rules.rs`) が短 hand id を使い、 Lua の `define_deliverable { id = "infrastructure_core" }` に match せず → `build_deliverable` が silent reject

#### Fix (4 連結)

1. **Mid Rule 3.5** (`mid_stance.rs`): surveyed-but-not-owned system 毎に `deploy_deliverable(infrastructure_core)` を emit。 idle courier (= `can_colonize && idle` colony ship moonlighting until #446 dedicated transport class) と pair。 candidate frontier: surveyed ∧ ¬owned-core ∧ ¬pending-deploy ∧ ¬pending-colonize ∧ ¬hostile ∧ ¬region-member、 region-centroid 距離 sort。
2. **Survey region-scope filter 削除** (`npc_decision.rs`): Rule 2 を galaxy-wide に。 surveyed target は次 tick で Rule 3.5 frontier candidate に流れる。
3. **Eager macro decomposition** (`ai/command_outbox.rs::dispatch_ai_pending_commands`): `deploy_deliverable` (及び `dispatch_table` の primitive routing に乗らない future macro) は routing 前に `build_deliverable → load_deliverable → reposition → unload_deliverable` に expand。 depth=4 cap + skip-list (`colonize_system` は除外、 既存 `PendingAssignment` semantics 維持)。
4. **`infra_core` → `infrastructure_core`** (`decomposition_rules.rs`): typo fix。

`npc_decision_tick` で frontier 事前計算 + idle colony-capable ship を Rule 3 claimant と Rule 3.5 claimant に pre-partition (1 tick 内 double-book 防止)。

`MidGameAdapter` trait に default-empty method 追加 (`expansion_frontier_systems`、 `idle_couriers`) — `StubAdapter` 等の test fixture は無改変で済む。

#### Tests (`tests/ai_region_deadlock_hotfix.rs` 4 tests)

- `mid_emits_deploy_deliverable_for_unowned_surveyed_system`
- `survey_fires_galaxy_wide_not_region_bound`
- `no_double_deploy_during_courier_window` (chain count が deploy 中 tick で増えない、 outbox + per-ship pipeline + BuildQueue surface 全部 check)
- `rule_3_and_rule_3_5_do_not_double_book_ship`

3615 baseline + 4 new + 13 inherited = **3632 passed**。

#### 起票した follow-up

- 適切な Short Agent wiring: Mid-emitted macro が `CampaignReactiveShort` を通って流れるように (= dispatcher の eager fallback ではなく)
- Dedicated transport ship class (#446) — courier の colony ship moonlighting を解消

### E. PR #530 — `#490` fold-in regression hotfix #2

#### Root cause 1: F9 (Omniscient toggle) silent fail

- `UI_TOGGLE_OMNISCIENT` action_id は `input/mod.rs:356` で declared、 toggle handler `observer/mod.rs:329` は `kb.is_just_pressed(...)` で registry を consult。
- だが #490 fold-in で `register_default(UI_TOGGLE_OMNISCIENT, F9)` が `register_engine_defaults` から消えていた (NICE-TO-FIX 5b 過剰修正)。
- 通常 gameplay では `KeybindingPlugin` が install されているので registry path が常に hardcoded `F9` fallback に勝つ。 registry 不在で F9 dead。
- brp QA で Omniscient impl 本体は OK と確認 (BRP side で `world.insert_resources` した `ObserverMode` は flip する)。 binding のみ missing。
- **Fix**: F9 default を register 復活 (`input/mod.rs`)。 player は `keybindings.toml` で上書き可。

#### Root cause 2: Dispatch extrapolation lost for `deploy_deliverable`-expanded chains

- #528 の eager macro decomposition で `deploy_deliverable` が `build_deliverable → load_deliverable → reposition → unload_deliverable` 4-primitive に展開、 全部同じ drain に emit。
- `dispatch_ship_command_per_ship` は **無条件で** projection per primitive を write。 trailing `unload_deliverable` (intended_state = `None`、 intended_system = `ship.home_port` sentinel) が先行する `reposition` extrapolation を clobber → renderer の map projection layer に extrapolation line が残らない → ship marker が origin で凍結 (= AI ship が動いているのに map で動かない)。
- **Fix**: `intended_state.is_none()` で projection write skip (`ai/command_outbox.rs`)。 `PendingAiShipCommand` holder + (marker-dedup kind の) assignment marker は spawn 継続 — bogus projection write のみ skip。 player-side で `#493 dispatcher_skips_spatial_less_commands` が既に enforce している contract に align。

#### Map "外挿 / 観測 / 不明" contract (= user 確認)

- **Dispatch** が viewing empire の projection extrapolation (intended_state + intended_system) を write
- **Observation** (`KnowledgeFact::ShipArrived` / `SurveyComplete` 等) が ground truth と reconcile (mismatch → correct、 match → intended clear)
- **Negative observation** (`ShipMissing`) は `projected_state = Missing` set + intended clear (= renderer の「不明」 state)。 `apply_reconciliation` で既に正しく実装、 新 test で pin。

#### `command_kind_to_intended_state` 9-kind audit

| Kind | Intended state |
| --- | --- |
| `attack_target`, `reposition`, `blockade`, `fortify_system`, `move_ruler`, `move_to` | `InTransitSubLight` |
| `survey_system` | `Surveying` |
| `colonize_system`, `colonize_planet` | `Settling` |
| `load_deliverable`, `unload_deliverable` | `None` (spatial-less — dispatch が write skip) |

#### Tests (`tests/ai_visualization_extrapolation_hotfix.rs` 7 tests)

- `f9_default_binding_registered_for_omniscient_toggle`
- `dispatch_survey_writes_intended_to_projection`
- `dispatch_load_deliverable_does_not_overwrite_existing_projection`
- `dispatch_unload_deliverable_does_not_overwrite_existing_projection`
- `dispatch_reposition_writes_intended_to_projection`
- `command_kind_to_intended_state_full_audit` (11-kind 全 mapping pin)
- `ship_missing_fact_marks_projection_missing_and_clears_intended`

3632 baseline + 7 = **3639 passed**。

### F. PR #531 — resource gate + Rule 6 fleet_composition + #529 A migration

#### 起点: brp QA report の starvation cascade

ユーザーが brp で QA session を回したところ、 starter empire が数百 hexadies で破産。 2 結合 bug:

1. **Infinite explorer build loop**: Rule 6 第 1 branch (`comp.survey_count == 0 && has_unsurveyed_targets`) が `survey_count` を `NpcContext.ships` から引き、 これは `info.system.is_some()` でフィルタされた list → explorer が `ShipState::Surveying` に遷移した瞬間に `system == None` で list から evict → `survey_count == 0` → Rule 6 が 30 hex 毎に `build_ship explorer_mk1` 再 emit → 30 hex / 1 ship + maintenance 累積で stockpile zero までドレイン。
2. **165× `mine` stack**: brp QA 観測で 1 colony に `mine` が 165 件 stack。 Rule 5b は産出短期で毎 tick emit、 system-building branch は per-tick dedup あり、 **planet-building branch は dedup なし**。

#### 根本: 「AI resource gate」 不在 → 即「#529 epic」 起票

新 issue **#529** = "epic(ai): migrate all AI judgement paths to projection-based (= light-coherent AI)" を起票し、 hotfix #3 を migration の **A. Resource gate** に位置付け。

##### #529 contract (user 確認)

- 外挿 (= extrapolation) を知識として表示する。 観測と外挿がズレたときは観測で外挿を修正、 観測が negative result だけなら「不明」 で update。
- **表示だけでなく AI 判断にも効くべき**: AI も自分の knowledge / projection / commitments を踏まえて判断する。

##### Symptoms (= 現状の不具合)

1. 二重命令 (light-delay 中の自分の命令を忘れて再 emit)
2. idle 誤認 (light-delay 中の ship を「ECS では Loitering、 AI 判断では idle」)
3. Resource 0 まで build 続行 (自分の commitments を踏まえない)
4. AI が ground truth 依存

##### Migration scope

- **A. Resource gate** (hotfix #3 範囲) — `current_stockpile - sum(pending.cost) >= new_cost`
- **B. Idle/busy 判定** (後続 PR) — `KnowledgeStore.projections[ship].intended_state` を見て busy 判定
- **C. Survey/colonize candidate 厳密化** (後続 PR) — 全部 KnowledgeStore 経由
- **D. AI metric 全般** (後続 PR) — adapter 全 method を audit

#### Fix (PR #531 → 3 commit chain)

##### Commit 1 (`3a6a172`): 初期 hotfix

- **`fleet_composition` semantics**: `BevyMidGameAdapter` が事前計算した empire census (alive ship 全部 + 待ち列の `BuildKind::Ship` order 全部) を carry。 system-filtered `NpcContext.ships` を再走査しない。 surveying explorer + queued explorer 両方 count → 無限 loop close。
- **Soft resource gate on Rules 3.5 / 5a / 5b / 6**: 新 `MidGameAdapter::can_afford_design` / `can_afford_building` (and `ShortGameAdapter` equivalent for Rule 5b)。 `current_stockpile >= cost` を gate。 soft by design — deficit spending (revenue < expense、 stockpile > 0) OK、 `stockpile == 0` のみ block。 Rule 6 は priority 高い不満を pick → gate 適用、 gate 失敗時は silent (= cheaper design への fall-through なし)。
- **Planet-building dedup at colony level**: system-building dedup の mirror。 same `building_id` 既に queue 済 → skip。 cross-id (mine + farm) は independent dedup。
- **`ResourceGateParams` SystemParam**: `ResourceStockpile` + `BuildQueue` + `BuildingRegistry` + `ShipDesignRegistry` を bundle (16-param ceiling 回避のため `ShipDesignRegistry` を `npc_decision_tick` 直接 param から folding)。 per-empire stockpile sum を `EmpireShortInputs` に publish。

##### Commit 2 (`9dd77d6`): #529 A migration — pending-aware

`MidGameAdapter::can_afford_*` が「stockpile sum vs cost」 だった。 in-flight commitment が控除されないので「100 minerals + queued corvette (cost 80、 invested 0) で 2 機目 corvette を gate が通す → 産出 tick で両方 starve」。

Pending-aware に migrate:
- per-colony `BuildQueue.queue`: `(minerals_cost - minerals_invested)` + `(energy_cost - energy_invested)` を `BuildKind::Ship` / `BuildKind::Deliverable` 全部から subtract
- per-system `SystemBuildingQueue.queue`: `minerals_remaining` / `energy_remaining` を `member_systems` 上の order 全部から subtract (= shipyard / port / research_lab 待ち列)
- per-colony `BuildingQueue` (mine / farm / power_plant build 待ち列) は **意図的に除外** — query が `ResourceGateParams` 外、 入れると 16-param ceiling 超え。 `handle_build_structure` の same-tick dedup が backstop、 必要なら #529 A 後続で追加可能。

`Amt::sub` は saturating — pending > stockpile で `current_minerals` / `current_energy` が 0 にクランプ (wrap せず)。 over-committed empire への正しい signal (= 「新規 emit は全部 reject」)。

##### Commit 3 (`89194e5`): region-scope codex review fold-in (PR #531 finding 1+2)

Codex review 指摘 2 件:
- **Finding 1**: pending-subtraction 走査が **empire-wide** → region B の pending ship/deliverable が region A の `current_minerals` を下げる。 → `member_systems_set` で gate (colony の host-system が member に含まれる order のみ subtract)。 `ResourceGateParams.build_queues` に Colony component view + `planets` field 追加。 Rule 6 fleet census は意図的に empire-wide 維持。
- **Finding 2**: `ShortAgentTickInputs` が per-empire keyed (`EmpireShortInputs`) → multi-MidAgent empire (= 1 empire に複数 region) で互いの stockpile sum + survey assignment slice を overwrite。 → per-region keyed (`RegionShortInputs`、 `EmpireShortInputs` → rename、 `per_empire` → `per_region`)、 `run_short_agents` が agent の `managed_by → mid.region` chain で lookup。

#### Tests

- `mid_stance::tests` 7 new unit tests (Rule 6 census semantics + gate behaviour + no-fall-through + deficit-spending escape hatch + Rule 5a gate)
- `tests/ai_resource_gate_hotfix.rs` 3 integration tests (planet-building dedup via production `drain_ai_commands` pipeline; single-tick / multi-tick / cross-building-id)
- 同 file に 3 added (pending-aware): `resource_gate_subtracts_pending_orders_from_stockpile`、 `resource_gate_subtracts_only_remaining_cost_when_pending_invested`、 `system_building_queue_pending_also_subtracts_from_stockpile`
- 新 `ai_resource_gate_region_scope`: `colony_pending_outside_region_not_subtracted_from_other_region` + `multi_midagent_inputs_keyed_by_region_not_overwritten`
- `ai_resource_gate_hotfix.rs` 既存 pending-aware test を `find_empire_region` helper 経由で per_region API に migrate

3639 baseline + ~13 net = **3652 passed**。

## Architecture / 設計メモ

### Build queue routing 契約 (post-#470)

```
AI build emit (Mid Rule 5/6 → AiCommand)
    → dispatch_ai_pending_commands
        capital-routed branch (Ruler→capital delay via AiCommandOutbox)
    → drain_ai_commands (capital arrival)
        → handle_build_ship / handle_build_deliverable
            → pick_host_colony(system, empire) [walk colonies, FactionOwner match]
            → br.build_queues.get_mut(colony_entity).push(BuildOrder)
            → dedup check: BuildKind::Ship + design_id (mirrors Deliverable dedup)
```

Player UI (`ui/system_panel/mod.rs:1390-1413`) も `host_colony` pattern を使う。 player UI は upstream の `is_own_system` gate に依存しているが、 AI は征服 system で `Sovereignty.owner != colony.FactionOwner` の case を考慮し stricter な `FactionOwner` check を直接持つ。 follow-up #523 で player UI 側にも backport 予定 (split-ownership latent bug)。

### Shipyard parallel slot 契約

```
SystemModifiers.shipyard_build_parallel_slots: ScopedModifiers (base = 0)
    → shipyard 1 個 = +1 slot (modifier 増分)
    → tick_build_queue が最大 N orders を parallel に進める
SystemModifiers.shipyard_build_speed: ScopedModifiers (base = 1.0)
    → speed multiplier (shipyard 本体は寄与せず、 future tech / module 用)
    → ShipyardSpeedAccumulators が分数 remainder を保持 (next tick で carry)
    → per-slot cost share = cost / build_time_total
    → 1 slot 分 funding できなければ build_time decrement なし (starvation stall)
```

ai/emitters.rs metric:
- `systems_with_shipyard` = ≥1 shipyard を持つ system 数 (set-count、 mid_stance Rule 5/6 consumer の `>= 1.0` 比較を破壊しない)
- `total_shipyard_slots` = slot 数 sum (= 新 metric)
- `can_build_ships` = binary 0.0/1.0

### Observer mode 3-variant 契約

```rust
pub enum ObserverModeKind {
    Disabled,    // PlayerEmpire view
    EmpireView,  // observed empire の KnowledgeStore 経由 (= #499 light-coherent)
    Omniscient,  // god view、 全 KnowledgeStore 無視で realtime ECS 直読
}

pub struct ObserverMode {
    pub kind: ObserverModeKind,
    pub previous_kind: Option<NonOmniscientKind>,  // 型レベル restore-loop 防止
}
```

5 callsite (outline / map tooltip / ship_panel / context_menu / camera / situation_center):

```rust
let knowledge = resolve_viewing_knowledge_omniscient(observed_empire, &knowledge_query, &observer_mode);
// Disabled / EmpireView → Some(&KnowledgeStore of viewing empire)
// Omniscient → None (= realtime ECS path)
```

- `is_empire_view()` — spawn-architecture observer mode 判定 (旧 `enabled` 相当)
- `is_omniscient()` — god view 判定
- `is_any_observer()` — union (= EmpireView ∪ Omniscient)

`ObserverMode` は runtime-only、 save に乗らない。 #527 で persist (priority:low)。

### Map "外挿 / 観測 / 不明" contract (= post-#530 / #531 で全 path 統一)

```
Dispatch:
    intended_state = command_kind_to_intended_state(kind)
    if intended_state.is_some() {
        write projection extrapolation (intended_state + intended_system + intended_takes_effect_at)
    }
    // intended_state == None (= load/unload_deliverable) → projection skip

Observation:
    KnowledgeFact 受信時 reconcile_ship_projections が ground truth と比較
    mismatch → correct projected_state (= sublight が遅れた等)
    match → intended_state clear (= 外挿が観測で確定)
    negative (= ShipMissing) → projected_state = Missing + intended clear (= 「不明」)
```

#493 `dispatcher_skips_spatial_less_commands` と aligned。 #530 が AI 側を同 contract に揃えた。

### Resource gate 契約 (= #529 A migration)

```rust
// MidGameAdapter / ShortGameAdapter:
fn can_afford_design(&self, design_id: &str) -> bool;
fn can_afford_building(&self, building_id: &str) -> bool;

// Backing implementation:
let current = ResourceStockpile sum for empire
let pending = sum over BuildQueue + SystemBuildingQueue (member_systems_set でフィルタ):
                (minerals_cost - minerals_invested) + (energy_cost - energy_invested)
let available = current.saturating_sub(pending)
let can_afford = available >= cost  // soft gate、 0 のみ block、 deficit spending OK
```

Rule 6 priority 順:
1. survey (探索船不足)
2. colonize (居住船不足)
3. defense (戦闘船不足)
4. trade (交易船不足)

各 branch で priority 高い不満 pick → gate 適用 → 失敗で silent (= cheaper design への fall-through なし)。

Region-scope:
- pending subtraction は `member_systems_set` (= MidAgent.region) で gate
- fleet census (Rule 6) は意図的に empire-wide
- `RegionShortInputs` per-region keyed (= multi-MidAgent empire の overwrite 回避)

### #529 epic 残 migration (= 後続 PR)

- **B. Idle/busy 判定**: `npc_decision_tick` が `ShipState::Loitering` 等を直接 query → `KnowledgeStore.projections[ship].intended_state` 経由に
- **C. Survey/colonize candidate 厳密化**: 全 AI metric を `KnowledgeStore` 経由化、 「不明」 state system を candidate 除外
- **D. AI metric 全般 audit**: `mid_adapter` / `short_adapter` 全 method を realtime ECS query → projection-based 置換

## SAVE_VERSION 状況

**現状: `SAVE_VERSION = 20`** (Round 14 末から不変)。

Round 16 で追加された data:
- `ShipyardSpeedAccumulators` Resource → runtime-only (mid-tick fractional state は save しない)
- `ObserverMode { kind, previous_kind }` → runtime-only (#527 で persist 予定)
- `PendingAiShipCommand` → runtime-only (Round 15 から維持)
- `MidGameAdapter::expansion_frontier_systems` / `idle_couriers` → trait method、 persist しない
- `RegionShortInputs` (旧 `EmpireShortInputs`) → SystemParam tick input、 persist しない
- `fleet_composition` on `BevyMidGameAdapter` → adapter field、 persist しない
- `can_afford_*` adapter methods → 計算済み値、 persist しない

旧 v20 save は round-trip OK。 `tests/fixtures/minimal_game.bin` (829 B) 再生成不要。 `tests/fixtures_smoke.rs::load_minimal_game_fixture_smoke` green。

## Test status (最終)

- `cargo test --workspace --tests -- --test-threads=1` → **3652 passed / 0 failed**
- baseline (Round 15 末) 3579 → +73 net
  - PR #510: +5 (`ai_ship_build_queue.rs`)
  - PR #511: +8 (`shipyard_parallel_slots.rs`)
  - PR #512: +8 (`observer_mode_omniscient.rs` 16 → 24)
  - PR #528: +17 (`ai_region_deadlock_hotfix.rs` 4 + decomposition migration )
  - PR #530: +7 (`ai_visualization_extrapolation_hotfix.rs`)
  - PR #531: +13 net (resource gate 10 + region-scope 2 + per_region migration )
  - misc: -少数 (test fixture migration、 rename collapse)

- 新規 / 大幅更新 test file:
  - `tests/ai_ship_build_queue.rs` (新規 5 tests、 #470)
  - `tests/shipyard_parallel_slots.rs` (新規 8 tests、 #445)
  - `tests/observer_mode_omniscient.rs` (16 → 24 tests、 #490 fold-in)
  - `tests/ai_region_deadlock_hotfix.rs` (新規 4 tests、 #444)
  - `tests/ai_visualization_extrapolation_hotfix.rs` (新規 7 tests、 #490 hotfix #2)
  - `tests/ai_resource_gate_hotfix.rs` (新規 6 tests、 #531 + pending-aware fold-in)
  - `tests/ai_resource_gate_region_scope.rs` (新規 2 tests、 #531 region-scope fold-in)
  - `src/ai/mid_stance.rs::tests` (+7 unit tests、 #531)
- pre-existing flake (parallel 実行時のみ):
  - `ui::situation_center::notifications_tab::tests::apply_pending_acks_system_drains_buffer_and_acks_queue` (global static race、 isolated で pass)
  - `tests/esc_notification_pipeline::ack_affects_esc_queue_only_not_banner` (同種)

## 起票 / 整理した issues

### Closed (本 session)

| # | 内容 | PR |
|---|---|---|
| #470 | `queue_ship_at_shipyard` BuildQueue 不在 bug | #510 |
| #445 | shipyard 数効果 (shipyard_capacity 未活用) | #511 |
| #490 | omniscient (god-view) observer mode | #512 (+ hotfix #530) |
| #444 | colonize_system 分解 (deploy_core + colonize) — 実態は region deadlock 4-fix bundle | #528 |

### 新規起票 (本 session)

**Epic**:
- **#529** epic(ai): migrate all AI judgement paths to projection-based (= light-coherent AI) [priority:high, theme:ai] — A. Resource gate (= PR #531 で着手済 = pending-aware migration land)、 B. Idle/busy 判定 / C. Survey/colonize candidate 厳密化 / D. AI metric 全般 audit が残り

**Followup tickets (#445 / #470 / #490 / #528 由来)**:
- #513 refactor: shipyard capability の Lua 定義削除 (#445 follow-up) [low, theme:modding]
- #514 bug(ai): Sovereignty owner attribution symmetry (#445 follow-up) [medium, theme:ai]
- #515 feat(simulation): split `BuildKind::Deliverable` vs `BuildKind::Ship` into separate slot pools [low]
- #516 refactor: `tick_build_queue` SystemParam bundle (16-param ceiling) [low]
- #517 refactor: rename `shipyard_build_parallel_slots` (deprecation window 後) [icebox]
- #518 feat(ai): per-frontier shipyard strategy — Rule 5a で `total_shipyard_slots` 活用 [medium, theme:ai]
- #519 design: `shipyard_build_speed` mass-production tech / module 活用 [low]
- #520 refactor(ai): `command_consumer.rs` の残 `debug!` silent drop を `warn!` promotion audit [low, theme:ai]
- #521 refactor(tests): `run_until_drained` test helper (`for _ in 0..3 { update() }` 撤去) [low]
- #522 refactor(tests): `spawn_test_faction_full` helper (= `setup::run_faction_on_game_start` parity) [low]
- #523 bug(ui): backport `FactionOwner` check to player UI `host_colony` pick (split-ownership latent bug) [medium]
- #524 enhancement(ai): multi-colony per-system load balancing for build-order dedup [icebox, theme:ai]
- #525 refactor(ui): ObserverMode 3-way branch を `ResolveTarget` enum に collapse [medium]
- #526 docs(observer): mode contract docstring を canonical block に集約 [low, theme:polish]
- #527 feat(save): `ObserverMode.kind` を save/load 永続化 [low]

### Round 16 残 follow-up (= 次セッション候補)

| # | 内容 | priority | unblock |
|---|---|---|---|
| **#529** | epic projection-based AI judgement | **high** | A migration land 済、 B/C/D が残 |
| #466 | ThreatState 機構 (Suspected/Confirmed + ROE) | medium | projection 基盤完成で unblock、 Component / state transitions / ROE wiring 残 |
| #467 | Mid-Mid 競合解決 (FCFS arbiter + rejection 通知) | medium | design phase |
| #441 | gamestate thread-local proxy 統一 (console + 全 callback) | high (`theme:modding`) | scripting infra |
| #452/#451/#450 | Long/Mid/Short layer command UI + inter-layer light-speed routing | medium | player UX、 AI 三層完成度 |
| #347 | keybinding manager 完成 (rebinding UI + persistence) | medium | KeybindingRegistry land 済、 rebinding UI 残 |
| #518 | Rule 5a frontier shipyard strategy | medium | #445 metric 活用 |
| #525 | ObserverMode `ResolveTarget` enum collapse | medium | #490 refactor |

### 静的防御 / 戦闘 (0.3.x 後期 〜 0.4.0)

`#211 戦闘 umbrella`、 `#213 静的防御`、 `#220 防衛 platform`、 `#218 wake signature`、 `#121 Interdictor`、 `#120 wake detection`、 `#184 地上戦`、 `#139 軌道爆撃`、 `#140 惑星破壊兵器` (icebox)。

### 1.0.0 後回し固定

`#174 外交 UI (旧)`、 `#143 UI icons`、 `#135 procedural textures`、 `#157 Lua UI panels` (但し priority:high)、 `#61 balance`。

## 次セッション最優先

### Top: `#529` epic 続行 (B/C/D migration)

A (Resource gate) は PR #531 で land 済。 残:

- **B. Idle/busy 判定** — `npc_decision_tick` で `ShipState::Loitering` 等を直接 query している箇所を `KnowledgeStore.projections[ship].intended_state` 経由に migrate。 light-delay 中の自分の ship を「busy」 と認識し二重命令防止。 **これが現実的に次の最優先** — Symptom 1 (二重命令) + Symptom 2 (idle 誤認) を解決。
- **C. Survey/colonize candidate 厳密化** — `iter_known_systems()` 部分実装を全 AI metric に拡張、 「不明」 状態 system 除外。
- **D. AI metric 全般 audit** — adapter trait 全 method の realtime → projection-based 置換。

### 次: AI 完成度系

- **#466 ThreatState Phase 2** — projection 基盤完成で unblock 済、 design 確認後 epic-level work
- **#467 Mid-Mid arbiter** — FCFS + rejection 通知、 design phase
- **#518 Rule 5a frontier shipyard strategy** — `total_shipyard_slots` metric を活用した throughput-aware build placement
- **#525 ObserverMode `ResolveTarget` collapse** — #490 refactor
- **#523 player UI host_colony backport** — split-ownership latent bug の正式 fix

### 中位

- **#441 gamestate proxy 統一** — scripting infra、 console + event callback 全部に thread-local 拡張
- **#347 keybinding rebinding UI**
- **#452 / #451 / #450 layer-aware UI + inter-layer routing** — player UX 系

### Skip 候補 (= 0.4.0+)

- 戦闘 / 静的防御 epic 系 (`#211/#213/#220/#218/#121/#120/#184/#139`)
- BRP tooling 改善
- clippy 警告整理 (`#455`、 low)

## 重要な caveat

- **`SAVE_VERSION = 20` 維持** — Round 16 の全 changes が runtime-only か postcard additive。 旧 save round-trip OK、 fixture 再生成不要。
- **`ShipyardSpeedAccumulators` は runtime-only** — mid-tick の分数 remainder は save しない。 save → load の境界で「速度多倍率の累積部分」 はリセット、 pre-alpha 許容範囲 (= 1 tick 内で再蓄積)。
- **`ObserverMode.kind` も runtime-only** — F9 で Omniscient 化したまま save → load で Disabled に戻る。 #527 で persist する予定 (priority:low)。
- **`fortify_system` は Ruler→capital delay** — Round 15 の是正契約 (= spatial 誤分類から除外)。 `command_targets_system()` で list されないが `dispatch_ai_pending_commands` の build-routed branch で扱う。
- **Region-scope contract** — pending subtraction は `member_systems_set` で gate (= multi-region empire の隣 region への巻き込み防止)、 fleet census (Rule 6) は意図的に empire-wide。 後者を region-scope にすると「region B の在庫を見て region A が survey ship を建造しない」 等の strategic mistake を生む。
- **`MidGameAdapter` trait widening** — `expansion_frontier_systems` + `idle_couriers` (#528) と `can_afford_design` + `can_afford_building` (#531) が新 method。 デフォルト実装あり (空 / true)、 `StubAdapter` 系 test fixture は無改変。
- **F9 Omniscient toggle は in_state(GameState::InGame) gate** — main-menu / loading で inert (= #490 fold-in 5a)。 binding は `keybindings.toml` で上書き可。
- **#523 split-ownership latent bug 未 fix** — player UI の `host_colony` は `is_own_system` gate に依存しており、 split-ownership 例外で latent。 AI side は #470 で stricter な `FactionOwner` check を直接持つが、 player UI side の backport は #523 で 別 PR 予定。

## 次セッション再開プロンプト例

```
2026-05-25 ハンドオフ参照。
docs/handoff/2026-05-25-round-16-ai-build-and-resource-gate.md
を読んで全体像把握。 Round 16 = #470 (AI build colony routing) + #445 (shipyard
parallel slots) + #490 (Omniscient mode) + #444 (region deadlock 4-fix) + 2 hotfix
(visualization extrapolation + resource gate / pending-aware migration)、 計 6 PRs
/ 4 issue close + 1 大型 epic 起票 (#529)。 SAVE_VERSION = 20 維持。 全 workspace
test 3652 pass。

優先度:
1. #529 B. Idle/busy 判定 migration (= AI が自分の projection で busy 判定、 二重命令 + idle 誤認解決)
2. #529 C/D 続き (Survey/colonize candidate + AI metric 全般 audit)
3. #466 ThreatState Phase 2 (= projection 基盤完成で unblock、 epic-level)
4. #467 Mid-Mid arbiter (design phase)
5. #441 gamestate thread-local proxy 統一 (modding infra)
6. #518 Rule 5a frontier shipyard strategy (= #445 metric 活用)
7. #525 ObserverMode ResolveTarget collapse (= #490 refactor)
```

## Tooling friction (本 session で観察)

- **brp QA session の重要性** — ユーザーが brp で QA を回した結果、 hotfix #2 / #3 両方の問題が visible 化。 unit/integration test では捕まらない gameplay-level regression (= AI ship が map で動かない、 starter empire が破産) を実プレイで検出。
- **adversarial review wave** — #511 で BLOCKER + 2 HIGH + 2 MEDIUM、 #510 で HIGH + MEDIUM 5 件を fold-in。 #528 / #531 も類似 pattern。 大型 PR で 1 wave fold-in、 land 後別 PR で hotfix の rhythm が ROI 高。
- **codex review との連携** — PR #531 で codex review が 2 finding (region-scope + per_region migration) を出し、 commit 3 で fold-in。 in-PR fold-in 1-pass で済んだ。
- **eager macro decomposition の副作用** — #528 で導入した eager `deploy_deliverable` expansion が #530 visualization regression を発生。 dispatch path の `intended_state == None` skip で対症療法済、 真の解決は #529 C/D の AI metric projection-based 化。
- **F9 keybinding 削除の過剰修正** — #490 fold-in 5b で「Omniscient は dev-only」 と判断し F9 default 削除 → hotfix #2 で巻き戻し。 dev-only と「ユーザー操作不可」 を混同しないこと。 keybindings.toml で上書き可能性は dev-only validity と両立。

## Cleanup 残り

### Worktree (`git worktree list`)

以下 4 worktree が locked、 全部対応 PR は merged。 削除候補 (本 session では touch せず):

| worktree | branch | HEAD | status |
|---|---|---|---|
| `.claude/worktrees/agent-a1265f62e87347002` | `fix/hotfix-ai-region-deadlock` | b2764ac | PR #528 merged |
| `.claude/worktrees/agent-a2a9365e1b26ea3af` | `feat/490-omniscient-observer-mode` | 6d326f7 | PR #512 merged |
| `.claude/worktrees/agent-a6041ebb866e7e522` | `fix/hotfix-3-resource-gate` | 89194e5 | PR #531 merged |
| `.claude/worktrees/agent-ac410952116ab765f` | `fix/hotfix-2-visualization-extrapolation` | fe798c8 | PR #530 merged |

その他 worktree (`agent-a35da3..` 等) も古いと思われる、 個別確認推奨。

### Branch

merged branch (= remote 削除候補): `fix/470-build-queue-colony-routing`、 `fix/445-shipyard-parallel-slots`、 `feat/490-omniscient-observer-mode`、 `fix/hotfix-ai-region-deadlock`、 `fix/hotfix-2-visualization-extrapolation`、 `fix/hotfix-3-resource-gate`。

`codex-review-pr-530` / `codex-review-pr-531` は codex review 用 transient branch、 削除可。

### Stash

list に 3 entry あり (`previous-session-491-attempt` 等)、 全て古い。 削除候補だが本 session で touch せず。

## 参照 doc

- `docs/handoff/2026-05-24-round-15-ai-courier-delay.md` — Round 15 直前、 #468 AI courier delay 3-PR シリーズ + #469 + #499
- `docs/session-handoff-2026-04-29-round-14-light-coherence-completion.md` — Round 14、 #491 epic close + 0.3.1 polish
- `docs/session-handoff-2026-04-28-round-13-shipprojection.md` — Round 13、 ShipProjection epic
- `docs/session-handoff-2026-04-28-round-11-ai-trait-unification.md` — Round 11、 #448 trait unification
- `docs/ai-three-layer.md` — AI Long/Mid/Short 三層 architecture
- `docs/architecture-decisions.md` — ADR (= Lua gamestate API §10 等)
- `docs/game-design.md` — game design document
- `CLAUDE.md` — codebase / workflow guide
- `gh issue view 529` — projection-based AI judgement epic (= 次セッション最優先 source-of-truth)
