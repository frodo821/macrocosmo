# Session handoff — 2026-05-24 Round 15: AI courier delay + observer-mode contract migration

## TL;DR

Round 14 (`docs/session-handoff-2026-04-29-round-14-light-coherence-completion.md`) 直後の continuation。 main 上の commit 範囲 `5e8ab40 → 3e4af27` で **5 PRs / 5 commits**、 全て fast-forward squash merge。 主成果:

- **#499 closed** — observer mode が empire-view contract を遵守するように 5 callsite を migrate (Round 14 で起票した follow-up)。
- **#468 closed (3 PR シリーズ)** — AI ship 系命令が **Ruler→target_system** で光速遅延を払っていたバグを **Ruler→ship** に構造修正。 `PendingAiShipCommand` per-ship holder と新 system `drain_ai_ship_commands` を導入し、 9 kind 全てを `AiCommandOutbox`/`drain_ai_commands` レガシー経路から移行。 `fortify_system` の spatial 誤分類も是正。
- **#469 closed** — `rank_survey_targets` を raw 3D distance から ship-relative ETA + greedy per-ship 割当に置換。 FTL/sublight 経路を考慮し、 deterministic tie-break (`Entity::index()`) で stable に。
- **`SAVE_VERSION = 20` 維持** (#468 で追加された `AssignmentTarget::Planet`、 `AssignmentKind::Colonize` は postcard additive tag、 旧 save round-trip OK)。
- **新規 issue 起票なし**、 follow-up open: `#470 queue_ship_at_shipyard BuildQueue 不在 bug` (Round 14 引継、 priority:high)、 `#466 ThreatState Phase 2`、 `#467 Mid-Mid arbiter`、 `#490 omniscient mode`。

最終 test status: `cargo test --workspace --tests -- --test-threads=1` → **3579 passed / 0 failed** (Round 14 baseline 3557 → +22)。

前段: `docs/session-handoff-2026-04-29-round-14-light-coherence-completion.md`。

## Commit 順 (新しい順、 main 上)

```
3e4af27 fix(ai): #468 PR-3 attack/move_ruler/deliverable/colonize_planet migration + 4 fold-in HIGHs (#509)
2d9335a fix(ai): #469 rank survey targets by ship-relative ETA + greedy per-ship (#508)
47f6b06 fix(ai): #468 PR-2 colonize/reposition/blockade courier delay = Ruler→ship (#507)
679f6a0 fix(ai): #468 PR-1 survey_system courier delay = Ruler→ship (proof of concept) (#506)
de6c10a fix(ui): #499 observer mode honours empire-view contract via viewing empire's KnowledgeStore (#505)
```

PR 一覧:

| PR | issue | scope | files | +/- |
|---|---|---|---|---|
| #505 | #499 | observer mode contract migration | 5 callsite | helper hoist |
| #506 | #468 PR-1 | `survey_system` per-ship pipeline | proof of concept | — |
| #507 | #468 PR-2 | `colonize_system` / `reposition` / `blockade` | generic refactor | — |
| #508 | #469 | survey target ETA ranking | per-ship greedy | — |
| #509 | #468 PR-3 | `attack_target` / `move_ruler` / `load_deliverable` / `unload_deliverable` / `colonize_planet` + `fortify_system` 是正 + 4 HIGHs | 14 files | +2241/-1190 |

## Round 15 内訳

### A. PR #505 — #499 observer mode contract migration

Round 14 の `viewing_knowledge=None` realtime fallback を、 observed empire の `KnowledgeStore` 経由で **light-coherent** な empire-view に統一 migrate。 5 callsite (outline tree / map tooltip / ship_panel / context_menu / camera centering / situation_center ship_ops_tab) を 2 helper で routing。

**新 helper**:

```rust
// ui/mod.rs
fn resolve_viewing_knowledge(empire: Entity, query: &Query<&KnowledgeStore>) -> Option<&KnowledgeStore>;
fn empire_view_knowledge(store: &KnowledgeStore) -> Option<&KnowledgeStore>;

// observer/mod.rs (hoisted)
pub fn resolve_viewing_empire(world: &World) -> Option<Entity>;
```

**doc 訂正**: `knowledge::ship_view` docstring を caller-invariant に rewrite (= "Callers MUST pass the observed empire's `KnowledgeStore`")。 `ObserverView.viewing` doc-comment を「Faction entity」 → 「Empire entity」 に訂正 (#417 以降 drift)。

**#490 (omniscient) future hook**: 2 helper 共に `Omniscient` mode で `None` を return する change point として doc 化。 3-variant enum 拡張時の影響範囲を 2 関数に限定。

**Adversarial review findings 折込**:
- HIGH: duplicated viewing-empire resolver → `observer::resolve_viewing_empire` に hoist
- HIGH: inline `Some(knowledge)` × 4 が #490 で fragile → 2 sibling helper に routing
- HIGH: 4 callsite が production-path regression test 不在 → `empire_view_tests` module 3 tests 追加
- MEDIUM: docstring が history narration → caller invariant に rewrite
- NIT: `ObserverView.viewing` doc 訂正

**Test**: 4 新規 regression test (`resolves_to_passed_empires_knowledge_store_in_observer_mode` 等)、 全 workspace **3557 → 3559** pass。

### B. PR #506 / #507 / #509 — #468 AI ship command 光速遅延の構造修正 (3 PR シリーズ)

#### 根本 bug

AI ship-control 命令が **Ruler→target_system** で光速遅延を払っていた。 例: Ruler が home A、 Scout が frontier B (5 ly)、 survey 対象が遠方 C (例 300 ly) の場合、 Scout は home に居なくても **~300 hexadies 待ち** してから survey を開始。 これが #468 「NPC が船を建造しない / 動かない」 の根本原因。

**正しい契約**: 命令は ship に届けばよい (= Ruler→ship)。 PR シリーズ完了で 9 kind 全てが per-ship 光速遅延 (`light_delay_ruler_to_ship`) を払う形に統一。

#### PR-1 (#506) — `survey_system` proof of concept

新規 component `PendingAiShipCommand { command, ship, issuer_empire, sent_at, arrives_at }` を `ai::command_consumer` に導入 (runtime-only、 `Reflect` 不要、 persist しない — `PendingScriptedCommand` をミラー)。

新 system `drain_ai_ship_commands` を `AiTickSet::CommandDrain` 内で `drain_ai_commands` の **前** に登録。 maturity (`clock.elapsed ≥ arrives_at`) で `SurveyRequested` を write、 holder を despawn。

`dispatch_ai_pending_commands` を `survey_system` で branch: per-ship `PendingAiShipCommand` を spawn (Ruler→ship 遅延)、 `PendingAssignment` を即時 insert (dedup 維持)、 per-ship `ShipProjection` を write。 `command_targets_system()` から `survey_system` を除去。

`handle_survey_system` を `drain_ai_commands` から削除、 body を `apply_survey_to_ship` に移植。

**dedup 拡張**: `npc_decision_tick` が `Query<&PendingAiShipCommand>` を walk するように。 3 query (`PendingAssignment` / `AiCommandOutbox` / `PendingAiShipCommand`) を新 `DedupParams` SystemParam に bundle (Bevy 16-param 制限回避)。

新 helper `physics::light_delay_ruler_to_ship` を `light_delay_hexadies` と並べて hoist。 player (`ui::context_menu`)、 Lua (`scripting::gamestate_scope::compute_request_light_delay`)、 AI dispatch が共有。

#### PR-2 (#507) — `colonize_system` / `reposition` / `blockade` + generic refactor

PR-1 の `dispatch_survey_per_ship` を `dispatch_ship_command_per_ship` に generalise:

```rust
fn dispatch_ship_command_per_ship(
    cmd: &AiCommand,
    ships: &[Entity],
    sent_at: i64,
    light_delay_fn: impl Fn(&Position, &Position) -> i64,
    assignment_factory: Option<impl Fn(Entity, Entity, i64) -> PendingAssignment>,
    ...
);
```

各 kind は assignment factory のみ供給:
- `survey_system` → `Some(PendingAssignment::survey_system)`
- `colonize_system` → `Some(PendingAssignment::colonize_system)`
- `reposition` / `blockade` → `None` (= pure movement、 marker 不要)

**新 `AssignmentKind::Colonize` variant**。 `sweep_resolved_survey_assignments` 拡張: 発令 empire の `KnowledgeStore.target.colonized = true` で Colonize marker も drop。

**Save additive**: `SavedPendingAssignment.kind: u8` に tag `1 = Colonize` 追加 (`0 = Survey`)。 SAVE_VERSION **不変** (postcard additive、 旧 save は tag `0` で Survey に decode)。

#### PR-3 (#509) — 残 5 kind + `fortify_system` mis-categorisation + 4 HIGHs

残 5 kind を migrate:

| kind | apply path | marker |
|------|-----------|--------|
| `attack_target` | `apply_move_to_ship("attack_target", ...)` | None |
| `move_ruler` | `apply_move_ruler_to_ship` (pushes `PendingRulerBoarding`) | None |
| `load_deliverable` | `apply_load_deliverable_to_ship` | None |
| `unload_deliverable` | `apply_unload_deliverable_to_ship` (uses ship's realtime `Position`) | None |
| `colonize_planet` | `apply_colonize_to_ship` with `target_planet = Some(p)` | `PendingAssignment::colonize_planet` (Planet target) |

**`fortify_system` 是正**: 元々 `command_targets_system` に spatial として list、 dispatcher が Ruler→target 光速遅延を払っていた。 だが実体は BUILD order (shipyard で combat ship を queue → capital にルーティング) → 空間命令ではない。 PR-3 で list から除去、 新契約 「全 kind は per-ship pipeline か、 Ruler→capital 遅延の二択。 Ruler→target route は残らない」 を確立。

**4 HIGH (PR-2 review 折込)**:
- **HIGH A**: `apply_reposition_to_ship` / `apply_blockade_to_ship` の 1-line wrapper を drain dispatch site で inline
- **HIGH B**: `assignment_factory: Option<fn(...)>` → `Option<F: Fn(...)>` に widen (= `colonize_planet` が planet entity を closure に capture できるように)
- **HIGH C**: `drain_ai_ship_commands` の 9-arm cascade を kind-keyed dispatch table に置換 (= PR-4+ で new kind 追加が「1 row + 1 apply fn」 になる)
- **HIGH D**: `count_outbox_for` を 3 test (`ai_npc_outbox_dedup` / `ai_per_region_npc_smoke` / `mid_agent_member_filter`) から `tests/common/ai_commitment.rs::count_ai_commitments` + `has_ai_commitment` に hoist

**Save additive**: `AssignmentTarget::Planet(Entity)` variant 追加。 postcard tag `1 = Planet` (旧 `0 = System` は round-trip)。 SAVE_VERSION **不変**。 decoder は unknown tag で `System` に fallback + warn。

#### #468 test 構成 (累積)

`tests/ai_command_lightspeed.rs` に **17 tests** (PR-1 で 3、 PR-2 で 5、 PR-3 で 5 + 1 fortify capital-delay pin + 1 colonize_planet reject-path)。 主要シナリオ:

- `survey_ai_scout_at_home_ruler_at_home_target_far_zero_delay` (= 元 #468 regression)
- `survey_ai_scout_remote_ruler_at_home_delays_by_ruler_to_ship`
- `colonize_ai_ship_at_home_ruler_at_home_target_far_zero_delay`
- `reposition_ai_ship_at_home_ruler_at_home_target_far_zero_delay`
- `blockade_ai_ship_at_home_ruler_at_home_target_far_zero_delay`
- `attack_target_*`、 `move_ruler_*`、 `load_deliverable_*`、 `unload_deliverable_*`、 `colonize_planet_*` 同 pattern
- `fortify_system_pays_ruler_to_capital_delay` (= 是正後 contract pin)
- reject-path: `rejected_colonize_at_drain_time_releases_pending_assignment` etc.

### C. PR #508 — #469 survey target ETA ranking

`rank_survey_targets` が raw 3D distance + Ruler-home tiebreak で候補を順位付けしていた。 pre-#468 は dispatcher が co-located で命令を発火する pattern が稀だったので近似は許容、 だが **#468 完了 で co-located ruler/ship の dispatch が即時化** → ranking 品質が load-bearing に。

問題点:
- FTL/sublight 移動モデルを無視 (= 等距離でも FTL hub 内なら ETA 大差)
- ship 位置を無視 (= frontier の scout でも Ruler home を基準に rank)
- tie-break が `Entity` 確保順依存で non-deterministic

**新 API**:

```rust
fn score_survey_target_eta(
    target: &Position,
    ship_pos: &Position,
    ftl_range: f64,
    sublight: f64,
    surveyed: &HashSet<Entity>,
) -> Option<i64>;  // hexadies ETA

fn rank_survey_targets_for_ship(
    ship: &ShipInfo,
    candidates: &[Entity],
    ...,
    ruler_to_ship_light_delay: i64,
) -> Vec<(Entity, i64)>;  // sorted by (score, Entity::index())
```

- `start_ftl_travel_full` + `start_sublight_travel_with_bonus` dispatch path をミラー。 FTL range 内の best surveyed waypoint を pick → sublight remainder を加算。 fallback で pure sublight。 immobile / unreachable は `None` で除外。
- Ruler→ship 光速遅延を score に加算 (= dispatch-to-arrival 全体時間を反映)
- tie-break `Entity::index()` で stable

**Greedy per-ship 割当**: `npc_decision_tick` が empire 単位で 1-pass greedy。 idle surveyor を `Entity::index()` で sort し、 各 ship が candidate を pick (= 二重割当回避)。 結果を `EmpireShortInputs.survey_assignments_by_fleet: HashMap<Entity, Vec<(Entity, Entity)>>` で publish。 Fleet-scope `ShortStanceAgent` が pre-paired tuple を消費。 legacy `idle_surveyors × unsurveyed_targets` zip は test stub 向けに fallback として残置。

`ShipInfo` に `position` + `sublight_speed` を追加、 `NpcContext` で plumb。

**Test**: 新 `tests/ai_rank_survey_targets_eta.rs` で 4 acceptance criteria + 4 scoring sanity:
- FTL hub preference vs pure-sublight equidistant target
- ship-relative ranking (ship が ruler から遠い場合)
- greedy per-ship no double-assignment
- `Entity::index()` stable tie-break (複数 call で safe)
- 等の 8 件追加。 `short_rules_cutover_sentinel::short_emits_survey_system_after_cutover` を 1-line setup fix (= ship が loiter system に居る場合 ETA=0 で自系統 survey が rank in、 test 意図に合わせ empire knowledge を pre-marked surveyed に)。

Workspace test: **3571 → 3579** pass。

## Architecture / 設計メモ

### `PendingAiShipCommand` 経路

```
ai::npc_decision::npc_decision_tick
    → emit AiCommand (Long/Mid/Short layer)
    → ai::command_consumer::dispatch_ai_pending_commands
        ├─ spatial kinds (per-ship) → spawn PendingAiShipCommand {arrives_at = sent_at + light_delay_ruler_to_ship}
        │     ├─ marker insert (PendingAssignment)
        │     └─ ShipProjection write
        └─ capital-routed kinds (build) → AiCommandOutbox (Ruler→capital delay)
    → ai::command_consumer::drain_ai_ship_commands (`AiTickSet::CommandDrain` 内、 drain_ai_commands の前)
        when (clock.elapsed >= arrives_at):
            ├─ apply_<kind>_to_ship (= ShipState 書き換え、 KnowledgeFact 発火等)
            ├─ marker strip on reject paths
            └─ holder despawn
```

**契約**: 9 kind 全てが Ruler→ship 光速遅延、 残るのは `fortify_system` の Ruler→capital のみ。 `command_targets_system()` は **「Ruler→target route が必要な kind」 = 0 件** (= 是正後)。

### `apply_*_to_ship` family (= drain dispatch table)

PR-3 HIGH C で 9-arm cascade を kind-keyed dispatch table に置換。 各 kind は:

```rust
fn apply_<kind>_to_ship(world, ship, target, issuer_empire, sent_at) -> Result<(), Reject>;
```

新 kind 追加 = 1 row in dispatch table + 1 apply fn。 PR-4+ で `repair_ship` 等の追加が trivial に。

### `AssignmentTarget` enum

```rust
pub enum AssignmentTarget {
    System(Entity),   // tag 0
    Planet(Entity),   // tag 1 (PR-3 で追加)
}
```

`colonize_planet` のみ Planet target を使用。 postcard additive、 旧 save tag 0 → System に decode。

### `AssignmentKind` enum

```rust
pub enum AssignmentKind {
    Survey,    // tag 0
    Colonize,  // tag 1 (PR-2 で追加)
}
```

`sweep_resolved_survey_assignments` で knowledge-based marker clear:
- Survey marker → `target.surveyed = true` で drop
- Colonize marker → `target.colonized = true` で drop

### Observer mode empire-view contract

```rust
// 5 callsite (outline / map tooltip / ship_panel / context_menu / camera / situation_center)
let knowledge = resolve_viewing_knowledge(observed_empire, &knowledge_query);
// → Some(&KnowledgeStore of observed empire) — observer mode で light-coherent
// → None — empire 不在 or omniscient mode (#490)

// ship_view 等は knowledge: Option<&KnowledgeStore> を受け取り、
// Some → empire-view (light-delayed projection / snapshot)
// None → realtime ECS ground truth (no-store / omniscient path)
```

Round 14 まで: `if observer_mode.enabled { None }` で realtime fallback (= 観察対象 empire ではなく ECS 直読)。 contract 違反。

Round 15 PR #505 で 5 callsite が `resolve_viewing_knowledge(viewing_empire, ...)` 経由に統一。 `viewing_empire` は `observer::resolve_viewing_empire(&World)` で `ObserverView.viewing` から resolve。

### `rank_survey_targets_for_ship` ETA model

```
score = ruler_to_ship_light_delay + ship_to_target_travel_time
ship_to_target_travel_time =
    if let Some((waypoint, ftl_dist)) = best_surveyed_waypoint_in_range(ship_pos, ftl_range, surveyed) {
        ftl_dist / ftl_speed + sublight_remainder(waypoint, target) / sublight_speed
    } else {
        direct_sublight_distance(ship_pos, target) / sublight_speed
    };
tie-break: Entity::index() ascending
```

`score_survey_target_eta` は immobile (`ftl_range == 0.0 && sublight == 0.0`) / unreachable で `None`。

## SAVE_VERSION 状況

**現状: `SAVE_VERSION = 20`** (Round 14 から不変)。

PR シリーズ #468 で追加された data:
- `AssignmentTarget::Planet(Entity)` (PR-3) → postcard tag 1、 旧 save tag 0 → System に decode
- `AssignmentKind::Colonize` (PR-2) → 同上、 旧 save tag 0 → Survey
- `PendingAiShipCommand` → runtime-only、 persist しない (`Reflect` 不要)

旧 v20 save は round-trip OK。 `tests/fixtures/minimal_game.bin` (829 B) 再生成不要。

`tests/fixtures_smoke.rs::load_minimal_game_fixture_smoke` green。 `tests/region_persistence.rs::save_version_strictly_rejects_previous_version` も v19 reject の状態維持。

## Test status (最終)

- `cargo test --workspace --tests -- --test-threads=1` → **3579 passed / 0 failed**
- baseline (Round 14 末) 3557 → +22 net (PR #505: +4、 #506: +5、 #507: +5、 #508: +8、 #509: +5 + 削除分 → 集計近似)
- isolated 全 green
- 新規 / 大幅更新 test file:
  - `tests/ai_command_lightspeed.rs` (12 → 17 tests、 全 9 kind の zero-delay 回帰 + fortify capital-delay + reject path 網羅)
  - `tests/ai_rank_survey_targets_eta.rs` (新規、 8 tests、 #469 acceptance)
  - `tests/common/ai_commitment.rs` (新規 helper、 3 file 集約)
  - `tests/common/mod.rs` (helper re-export)
  - `tests/ai_colonize_requires_core.rs` / `ai_npc_avoid_hostile_systems.rs` / `ai_npc_no_double_survey_assignment.rs` / `ai_npc_outbox_dedup.rs` / `ai_per_region_npc_smoke.rs` / `mid_agent_member_filter.rs` / `short_rules_cutover_sentinel.rs` を `AssignmentTarget::Planet` / 両 pipeline (`AiCommandOutbox` + `PendingAiShipCommand`) 対応に update
  - `ui::empire_view_tests` (#505、 4 regression tests)
  - `situation_center::ship_ops_tab::tests::observer_mode_routes_through_observed_empire_knowledge` (#505)

- pre-existing flake (parallel 実行のみ):
  - `ui::situation_center::notifications_tab::tests::apply_pending_acks_system_drains_buffer_and_acks_queue` (global static race、 isolated で pass)
  - `tests/esc_notification_pipeline::ack_affects_esc_queue_only_not_banner` (同種)

## 起票・整理した issues

### Closed (本 session)

| # | 内容 | 経緯 |
|---|---|---|
| #468 | AI ship 系命令 light-speed destination 不整合 | PR-1/2/3 シリーズで完遂、 PR-3 (#509) merge で auto-close |
| #469 | survey target ranking distance → ETA | PR #508 で close |
| #499 | observer mode empire-view contract | PR #505 で close (Round 14 起票分) |

### 新規 (本 session 起票)

なし。 PR 5 件は既存 issue の解消。

### Round 15 残 follow-up (= 次セッション候補)

| # | 内容 | priority | status / unblock |
|---|---|---|---|
| #470 | `queue_ship_at_shipyard` BuildQueue 不在 bug | **high** (`bug, simulation, theme:ai`) | unblock: #468 完。 「AI が船を建造しない」 残症状の root fix、 system 側 BuildQueue 不在の握りつぶしを正面から修正 |
| #466 | ThreatState 機構 (Suspected/Confirmed + ROE) | medium | Round 13 で `is_ship_overdue` helper land 済、 ShipDestroyed/Missing facts land 済。 ThreatStates Component / state transitions / ROE wiring が残 |
| #467 | Mid-Mid FCFS arbiter + rejection 通知 | medium | Round 11 #448/#449 後続、 design phase |
| #445 | Shipyard 数効果 (`shipyard_capacity` 未活用) | medium | balance |
| #444 | Short Agent `colonize_system` 分解 (deploy_core + colonize) | medium | AI behavior expansion |
| #441 | gamestate thread-local proxy 統一 (console + 全 callback) | high (`theme:modding`) | scripting infra |
| #452 / #451 / #450 | Long/Mid/Short layer command UI + inter-layer light-speed routing | medium | player UX、 AI 三層完成度 |
| #490 | omniscient (god-view) observer mode | medium | #499 contract migration で hook 化済、 2 helper で `Omniscient` branch を追加するだけ |
| #455 | clippy 警告整理 | low | 段階的 cleanup |
| #459 | `CommandLog` 意図確定 (player-only / per-empire) | low | refactor |
| #347 | keybinding manager 完成 (rebinding UI + persistence) | medium | KeybindingRegistry land 済、 rebinding UI が残 |

`#491` (light-coherence) は Round 14 で close。

### 静的防御 / 戦闘 (0.3.x 後期 〜 0.4.0)

`#211 戦闘 umbrella`、 `#213 静的防御`、 `#220 防衛 platform`、 `#218 wake signature`、 `#121 Interdictor`、 `#120 wake detection`、 `#184 地上戦`、 `#139 軌道爆撃`、 `#140 惑星破壊兵器` (icebox)。

### 1.0.0 後回し固定

`#174 外交 UI (旧)`、 `#143 UI icons`、 `#135 procedural textures`、 `#157 Lua UI panels` (但し priority:high)、 `#61 balance`。

## 次セッション最優先

### Top: `#470` AI 船建造 bug

「AI が船を建造しない」 の **残り** root cause。 Round 14 で起票時のコメント:

> `queue_ship_at_shipyard` が存在しない system 側 `BuildQueue` に書いて握りつぶされる

#468 (Ruler→ship 光速遅延) の構造修正で「AI が survey / colonize / 移動命令を発火」 系統は治療済 → **建造系の独立 bug** として顕在化。 root fix は system 側 `BuildQueue` 不在時の挙動 (= 適切な fallback / 起票 / capital へ routing) を確定 + 既存 dispatch path に wire。

### 次: AI 完成度系

- **#466 ThreatState Phase 2** — projection 基盤完成で unblock 済、 design 確認後 epic-level work。 Component / state transitions / ROE wiring が残
- **#444 colonize_system 分解** — Short Agent で `deploy_core` + `colonize` 2-phase に
- **#445 shipyard_capacity 活用** — balance bug、 値が ROE に反映されていない
- **#441 gamestate proxy 統一** — scripting infra、 console / event callback 全部に thread-local 拡張

### 中位

- **#467 Mid-Mid arbiter** — FCFS + rejection 通知、 design phase
- **#490 omniscient mode** — #499 migration で hook 化済、 trivial wire
- **#452 / #451 / #450 layer-aware UI + inter-layer routing** — player UX 系
- **#347 keybinding rebinding UI**

### Skip 候補 (= 0.4.0+)

- 戦闘 / 静的防御 epic 系 (= `#211/#213/#220/#218/#121/#120/#184/#139`)
- BRP tooling 改善
- clippy 警告整理 (= `#455`、 low)

## 重要な caveat

- **`SAVE_VERSION = 20` 維持** — Round 15 で追加された `AssignmentTarget::Planet`、 `AssignmentKind::Colonize` は postcard additive tag。 旧 save は tag 0 → 既存 variant に decode。 fixture 再生成不要。
- **`PendingAiShipCommand` は persist しない** — runtime-only holder、 `Reflect` 不要、 `SavedComponentBag` に bag されない。 save 中 in-flight 状態は holder 破棄、 marker (`PendingAssignment`) のみ残存。 これは pre-alpha における意図的なシンプリフィケーション (= AI 命令の途中状態 save/load は non-goal、 marker dedup で「重複発射しない」 のみ保証)。
- **`fortify_system` は Ruler→capital delay** — PR-3 で spatial 誤分類から是正。 `command_targets_system()` で list されないが `dispatch_ai_pending_commands` の build-routed branch で扱う。
- **observer mode 5 callsite の empire-view contract** — `resolve_viewing_knowledge` 経由で observed empire の `KnowledgeStore` を渡す。 omniscient mode (#490) は **未実装**、 現状の `ObserverMode` は 2-state (`enabled: bool`) で `enabled=true` 時 viewing empire の light-coherent view。
- **`rank_survey_targets_for_ship` ETA model** — FTL hub 探索は per-target で O(ships × candidates × surveyed)。 surveyed が増えると重くなる可能性、 future optim 候補 (= surveyed spatial index)。
- **dispatch table (HIGH C)** — `drain_ai_ship_commands` の kind-keyed table 化。 PR-4+ で `repair_ship` 等の新 kind は 1 行追加で済むはず。

## 次セッション再開プロンプト例

```
2026-05-24 ハンドオフ参照。
docs/handoff/2026-05-24-round-15-ai-courier-delay.md
を読んで全体像把握。 Round 15 = #468 PR-1/2/3 で AI ship 命令の
Ruler→ship 光速遅延構造修正 + #469 survey ETA ranking +
#499 observer mode contract migration、 計 5 PRs / 3 issue close。
SAVE_VERSION = 20 維持。 全 workspace test 3579 pass。

優先度:
1. #470 queue_ship_at_shipyard BuildQueue 不在 bug (= AI 船建造の root fix、 priority:high)
2. #466 ThreatState Phase 2 (= projection 基盤完成で unblock、 epic-level)
3. #444 colonize_system 分解 (deploy_core + colonize 2-phase)
4. #445 shipyard_capacity 活用 (balance bug)
5. #441 gamestate thread-local proxy 統一 (modding infra)
6. #467 Mid-Mid arbiter (design phase)
7. #490 omniscient mode (#499 で hook 化済、 trivial wire)
```

## Tooling friction (本 session で観察)

- **`gh pr view <num> --json mergeable,mergeStateStatus` の transient `UNKNOWN`** — 直前 merge 後の re-compute 待ち、 5-10 秒で `MERGEABLE/CLEAN` に
- **squash merge body の `Closes #X` footer auto-close** — PR-3 (#509) は `Closes #468` で正しく auto-close、 PR #505 も `Closes #499` で auto-close。 Round 14 の手動 close 問題は再発せず (= GitHub 側 fix?)
- **adversarial review wave**: Round 15 は PR-2 で 4 HIGH を fold-in、 PR-3 で残無し。 #468 シリーズで 1 round / 1 wave の rhythm が ROI 高
- **PR-3 規模** — 14 files、 +2241/-1190。 plan agent 起点で 4 sub-PR に分けずに 1 PR で land、 fold-in 折込で review 1 wave で済んだ (= migration が repetitive、 PR-1/2 で pattern 確立済)

## Cleanup 残り

なし。 stash list に 3 entry あり (`stash@{0}` `previous-session-491-attempt` 等) だが全て古い、 削除候補 (本 session では touch せず)。

## 参照 doc

- `docs/session-handoff-2026-04-29-round-14-light-coherence-completion.md` — Round 14、 #491 epic close + 0.3.1 polish
- `docs/session-handoff-2026-04-28-round-13-shipprojection.md` — Round 13、 ShipProjection epic
- `docs/session-handoff-2026-04-28-round-11-ai-trait-unification.md` — Round 11、 #448 trait unification
- `docs/ai-three-layer.md` — AI Long/Mid/Short 三層 architecture
- `docs/architecture-decisions.md` — ADR (= Lua gamestate API §10 等)
- `docs/game-design.md` — game design document
- `CLAUDE.md` — codebase / workflow guide
