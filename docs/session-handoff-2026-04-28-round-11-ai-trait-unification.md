# Session handoff — 2026-04-27/28 Round 11 AI trait unification + dispatch correctness

## TL;DR

最大級の単発セッション。 commit 範囲 `5d65a83 → 8e5a825` で **12 commits** landing。 主な成果:

- **Round 11 dispatch correctness** (Bug A/B/C): 前セッションハンドオフ末尾の最優先 3 bugs を全 fix。 `npc_decision_tick` が hostile-known system / outbox-resident in-flight commands も dedup 入力に union。 SAVE_VERSION 11 → 12 (PendingAssignment.stale_at field 削除)
- **#448 完了** (8 sub-PRs): AI 3 layer trait 統一の本丸。 `MidStanceAgent` に 8 rules 全移植 + AiPolicyMode flag default 反転 + SimpleNpcPolicy / NpcPolicy / NoOpPolicy / parity scaffolding 一括削除。 `npc_decision.rs` 1641 → 550 LoC、 全体 -2889 LoC 純減
- **新規 issue 3 件起票**: #466 ThreatState 機構、 #467 Mid-Mid 競合解決 (FCFS arbiter + rejection 通知 設計)、 #465 (#453 の dup として close)
- **既存 issue 整理**: #448 / #443 / #454 / #465 close、 #466 #467 を 0.3.1 milestone へ

`cargo test --workspace --tests` 全 green (#465/#453 既知 flaky のみ偶発)、 SAVE_VERSION 12、 fixture (`tests/fixtures/minimal_game.bin`) 不変。

前段: `docs/session-handoff-2026-04-27-ai-decomposition.md` (Round 10 + spec phase)。

## Commit 順 (新しい順)

```
8e5a825 refactor(ai): delete SimpleNpcPolicy + parity scaffolding (#448 PR3d)
cb21d10 feat(ai): flip AiPolicyMode default Legacy → Layered
82f0959 feat(ai): port Rule 5b (slot fill) to MidStanceAgent + parity coverage
ae9d19b feat(ai): port Rule 2 (survey) to MidStanceAgent + parity coverage
bbc2459 feat(ai): port Rules 3/6/7/8 to MidStanceAgent + parity coverage complete
d66107f feat(ai): port Rule 1 + Rule 5a to MidStanceAgent + parity test infra
913dcc2 feat(ai): MidGameAdapter trait + AiPolicyMode flag (Layered = noop)
1c22fcd feat(ai-core): introduce Proposal / ProposalOutcome / ConflictKind / Locality
a9c9b81 feat(ai-core): introduce LongTermState/MidTermState + extensible Stance
7d8f089 fix(ai): dedup NPC decision against outbox-resident in-flight commands (Bug A)
43bb9df refactor(ai): drop time-based PendingAssignment sweep, rely on knowledge cleanup (Bug C)
cbb2a17 fix(ai): skip known-hostile systems in survey/colonize candidates (Bug B)
```

## Round 11 内訳

### A. Dispatch correctness sweep (Bug B/C/A)

前セッションハンドオフで指定された「最優先 3 bug」 を順次 fix。

#### Bug B (cbb2a17) — hostile filter in survey/colonize
- 症状: NPC が `KnowledgeStore.has_hostile=true` の system に scout / colonizer を再派遣して全滅ループ
- Fix: `npc_decision.rs` で `hostile_systems_set: HashSet<Entity>` を構築、 `colonizable_systems` push と `candidates` filter の両方に `!has_hostile` 条件追加
- regression: `tests/ai_npc_avoid_hostile_systems.rs` 2 件

#### Bug C (43bb9df) — `sweep_stale_assignments` 削除
- 症状: `SURVEY_ASSIGNMENT_LIFETIME = 200` hex が短すぎ (実観察 ~1700 hex)、 marker mid-flight expire → Bug A の race を誘発
- Fix Option A 採用 (full cleanup): `sweep_stale_assignments` system + `SURVEY_ASSIGNMENT_LIFETIME` const + `PendingAssignment.stale_at` field + `survey_system` constructor の `lifetime` 引数 一括削除
- knowledge-driven cleanup (`sweep_resolved_survey_assignments`) + Bevy automatic despawn cleanup の 2 経路に依存
- SAVE_VERSION 11 → 12、 fixture regen (732 → 798 B)

#### Bug A (7d8f089) — outbox-aware dedup (approach b)
- 症状: AI bus emit 後 outbox 滞留中の in-flight command が `npc_decision_tick` の dedup 入力に含まれず、 mid_cadence=2 で 2 tick 毎に同 target へ重複 emit (Vesk Scout-1, Scout-2 が同 system に 163 hex 差で派遣)
- Fix: `Res<AiCommandOutbox>` を SystemParam に追加、 empire-loop 前に `outbox_survey_per_empire` / `outbox_colonize_per_empire` HashMap を pre-build (`FactionId → Entity` 1 段、 outbox.entries 1 段の合計 O(empires + outbox))、 各 empire の dedup set に union
- regression: `tests/ai_npc_outbox_dedup.rs` 2 件 (survey + colonize variants)
- SystemParam 数 13 → 14 (16 limit 内)
- 既存 issue **#454 を close** (description 完全一致)

### B. #448 「AI 3 layer trait 統一」 (8 sub-PRs)

ハンドオフの spec phase で確定済の architectural design を 4-stage Plan agent で更に sub-decompose、 計 8 sub-PRs で landing。

#### PR1 (a9c9b81) — ai-core State 構造体
- `LongTermState { pursued_metrics, current_campaign_phase, victory_progress }`
- `MidTermState { stance, active_operations, region_id }` (region_id は #449 placeholder)
- `Stance` enum: 4 core (`Expanding/Consolidating/Defending/Withdrawing`) + `Custom(StanceId)` extension hook (Lua-extensible 設計)
- `OrchestratorState.long_state` / `.mid_state` field 追加
- 新 placeholder type: `StanceId`, `RegionId`, `VictoryAxisId`, `CampaignPhase` (`arc_str_id!` パターン)
- macrocosmo-ai 単独、 SAVE bump 不要 (`OrchestratorRegistry` は `#[reflect(ignore)]`)

#### PR2a (1c22fcd) — Proposal types (#467 Phase 1)
- `Proposal { command, locality }`、 `ProposalOutcome { Accepted | Rejected { reason } }`、 `ConflictKind { AlreadyClaimed | OutOfRegion | ResourceExhausted | StaleAtArrival }`、 `Locality { FactionWide | System(SystemRef) | Region(RegionId) }`
- `MidId = FactionId` alias (#449 で newtype 化予定)
- ai-core 単独、 dependency なし

#### PR2b (913dcc2) — MidGameAdapter trait + AiPolicyMode flag
- `MidGameAdapter` trait (read-only interface for Mid logic)、 `BevyMidGameAdapter` 構造体 (`&NpcContext` 借用)
- `AiPolicyMode { Legacy, Layered }` Bevy Resource、 default `Legacy`
- `npc_decision_tick` に `match policy_mode { Legacy => SimpleNpcPolicy.decide, Layered => layered_decide_noop }` gate 挿入
- Layered 分岐は noop (空 Vec 返す)、 Legacy default で既存挙動完全保持

#### PR2c (d66107f) — Rule 1/5a + parity infra
- `MidStanceAgent::decide(adapter, stance, faction_id, now) -> Vec<Proposal>` を新設 (parallel to `IntentDrivenMidTerm`、 unification は PR4 候補)
- Rule 1 (attack hostile + move_ruler) と Rule 5a (build shipyard) を mirror byte-for-byte
- parity test infra: `tests/ai_layered_parity.rs` で `BTreeSet<CanonicalCommand>` 比較 (HashMap iter 非決定性回避)
- 観測点は `bus.pending_commands()` (outbox では zero-delay command が drained で見えない発見)

#### PR2d (bbc2459) — Rules 3/6/7/8
- Rule 3 (colonize)、 Rule 6 (build_ship 3-branch composition gap)、 Rule 7 (retreat、 strict `0.0 < ratio < 0.3` early-return)、 Rule 8 (fortify)
- Rule 7 early-return が Rule 8 を preempt する仕様を `rule_7_preempts_rule_8_via_early_return` で pin
- parity scenarios 計 6 件
- `tests/common/create_test_building_registry` の capabilities = empty を fix (`{"shipyard": {}}`/`{"port": {}}`) — production Lua 整合

#### PR3a (ae9d19b) — Rule 2 (survey)
- adapter に `unsurveyed_systems()` + `idle_surveyors()` 追加
- **dedup は upstream で完結** — `npc_decision_tick` が `pending_survey_targets` (PendingAssignment + outbox 由来、 Bug A の合算) を build してから `NpcContext.unsurveyed_systems` を作る、 adapter は as-is
- parity scenarios +2 (single + zip-with-fewer-surveyors)
- 既存 `ai_policy_mode_gate.rs::layered_mode_emits_no_commands` test (PR2b 由来 noop assertion) を `..._emits_commands_after_rule_ports` に flip

#### PR3b (82f0959) — Rule 5b (slot fill)
- adapter に `free_building_slots()` + `net_production_energy/food()` 追加 (per-faction metric topics)
- 3-branch `power_plant`/`farm`/`mine` を mirror、 strict `< 0.0` threshold、 `Proposal::faction_wide` (target_system なし、 handler が colony pick)
- parity scenarios +3 (energy-negative / food-negative / both-positive)
- 注意: `Amt` が unsigned のため fixture で metric 直接 override 必要 — `AiTickSet::MetricProduce` 後に system 注入で signed 値再 emit (`MetricStore::push` 同 tick 上書き OK)

#### PR3c (cb21d10) — default flip
- `#[default]` annotation を `AiPolicyMode::Legacy` → `AiPolicyMode::Layered` に
- `mid_adapter.rs::ai_policy_mode_defaults_to_legacy` → `..._defaults_to_layered`
- `tests/ai_policy_mode_gate::legacy_mode_emits_commands_today` を explicit `insert_resource(AiPolicyMode::Legacy)` に変更
- **Moment of truth**: 全 NPC integration test (`ai_npc_*`、 `ai_player_e2e`、 `ai_command_lightspeed`、 `ai_layered_parity` 11/11) green

#### PR3d (8e5a825) — Legacy 一括削除
- 削除: `SimpleNpcPolicy` + Rules 1-8 impl + `NpcPolicy` trait + `NoOpPolicy` + 14 inline unit tests + `AiPolicyMode` enum + `layered_decide` helper + `bus_from_resource` helper
- 削除 file: `tests/ai_policy_mode_gate.rs` (223 LoC) + `tests/ai_layered_parity.rs` (1518 LoC)
- `npc_decision.rs` 1641 → 550 LoC (= -1091 LoC 単独)
- npc_decision_tick の `match policy_mode` collapse → 単一 Layered call
- SystemParam 15 → 13 (14 with `ai-log` cfg)
- **SAVE_VERSION bump 不要** — `AiPolicyMode` は `register_type` で BRP に出てたが persist されてなかった、 `OrchestratorRegistry` は `#[reflect(ignore)]`、 fixture 不変

### C. PR4 skip 判断
- Plan agent が「飛ばしても良い (Q1=(c) なら本質ではない)」と明言
- 共通 `Agent` super-trait を ai-core に置く cosmetic refactor、 既存 trait は blanket impl で互換維持
- CLAUDE.md「不要な抽象化を入れない」 + 価値が小さい (~200 LoC) ため skip
- #448 は PR3d 完了で close

## 起票・整理した issues

### 新規 (本セッション起票)
| # | 内容 | 状態 | milestone |
|---|---|---|---|
| #465 | flaky `survey_command_outbox_holds_until_light_delay_elapses` | **closed (#453 の dup)** | — |
| #466 | ThreatState 機構 (Suspected/Confirmed) | open | 0.3.1 |
| #467 | Mid-Mid 競合解決 (FCFS arbiter + rejection 通知) | open、 blocked-by #449 #450 | 0.3.1 |

### 整理 (close)
| # | 理由 |
|---|---|
| #448 | 8 sub-PRs landed、 PR4 skip |
| #454 | Bug A (7d8f089) で fix、 description 完全一致 |
| #443 | Round 10 E-track + F-track で実装済 (e56cd20, 0302395, d8de79e, 8950ba8, ca5fcd6) |
| #465 | #453 の dup |

## 確定済 architectural design (本セッションで concrete 化)

### Mid-Mid 競合解決 (#467)

光速遅延下では Long が「全 Mid proposal を待ってから最適選択」 不可能 (infinite buffering NG)。 因果整合戦略は **First-Come-First-Served (FCFS)** のみ。 衝突は防げない前提で、 失敗を Mid に通知して re-plan させる構造:

```rust
pub struct Proposal { command, locality }  // priority/weight は FCFS では無意味
pub enum ProposalOutcome { Accepted, Rejected { reason: ConflictKind } }
pub enum ConflictKind {
    AlreadyClaimed { by_mid, claimed_at },  // FCFS で負け
    OutOfRegion,                            // 自 region 外への越権 emit
    ResourceExhausted,                      // budget cap
    StaleAtArrival,                         // 到着時には状況変化
}
```

`MidTermState` 拡張:
- `pending_proposals: BTreeMap<ProposalId, PendingProposal>` (送信済 outcome 待ち)
- `committed: BTreeMap<...>` (Accepted で active になったもの)
- `expected_outcome_by: i64` で 2*light delay 過ぎたら故障扱い (timeout retry は別 issue)

Failure modes:
- timeout (outcome 不着)、 oscillation (Reject → 即再 emit ループ)、 starvation (常に他 Mid に先着される) — backoff / stance 変化 / authority weight (将来) で緩和

PR2a で型定義 land 済。 arbiter 実装は #467 (#449/#450 依存)。

### Mid Stance 拡張可能設計 (PR1 で land)

```rust
pub enum Stance {
    Expanding,
    Consolidating,
    Defending,
    Withdrawing,
    Custom(StanceId),  // Lua/scenario 拡張用 hook
}

impl Default for Stance {
    fn default() -> Self { Self::Consolidating }  // 序盤 build-up が自然
}
```

`StanceRegistry` (Lua-defined) は将来追加。 PR3 までは 4 core variants のみ使用、 stance modulation は noop (= 全 stance で同挙動)。 PR3+ で stance-dependent priority weighting。

### Rules 2/5b の Mid 暫定 host (user 承認)

- Plan agent 元案: Rule 2 (survey) と Rule 5b (slot fill) は per-fleet/per-colony Short が natural owner (#449 Region 化後)
- 但し PR3 で Legacy 削除する必要があり、 user は Mid に **暫定移植** を承認
- 手戻り: #449 で Mid → Short へ再移植 (許容済)

## 次セッション最優先

### Top-3 候補

#### 1. #449 Region 概念導入 + Mid/Short 地理的 instance 化 ⭐️ **強く推奨**
- AI architecture 中核、 #450/#451/#452/#467 全ての前提
- 単一 Mid (per faction) → N Mid (per region) への分割
- Short も per-fleet / per-colony 化
- Rules 2/5b の Mid → Short 再移植もここで実施 (PR3 暫定の解消)
- 推定 規模 大 (~15 file)、 Plan agent → sub-PR 分解推奨
- handoff `docs/session-handoff-2026-04-27-ai-decomposition.md` line 211-242 に確定済 architectural design あり

#### 2. Per-empire 化 sweep (priority:high 系)
複数 issues で「player-only 経路 → per-empire 化」 のテーマ:
- #456 process_surveys owner empire 基準 (priority:high)
- #458 PendingResearch owner-scope (priority:high)
- #457 sensor buoy / relay knowledge per-empire (priority:high)
- #464 KnownFactions per-empire (priority:medium、 theme:ai)
- #463 GameEvent 意味契約 (priority:medium)
- 個々は中小規模、 並列化で高効率
- ThreatState (#466) の前提として `KnowledgeFact::ShipDestroyed` per-faction propagation 必要 → 関連性高い

#### 3. #461 request_command local/remote 分離 (priority:high、 theme:modding)
- Lua scripting 経由の命令が遅延 transport を bypass する bug
- 単独 fix 可能、 比較的小規模

### 中位

| # | 内容 | 規模 |
|---|---|---|
| #460 | Casus Belli auto-war / forced peace の遅延 bypass | 中 |
| #440 | observer mode read_only が ship panel 以外に効かない | 小 |
| #462 | context_menu の ShipState 直接書き込みを gate | 小 |
| #466 | ThreatState 機構 (依存: per-empire ShipDestroyed propagation = #457 等) | 大 |
| #347 | In-game keybinding manager + rebinding UI | 中 |
| #455 | clippy 警告整理 (段階的) | 小 |
| #411 | 戦闘 report 可視化 + アノマリー調査統合 | 中 |
| #445 | shipyard_capacity 値が活用されてない | 小 |

### Skip 候補
- #394 crate 分割 (icebox)
- #459 CommandLog (priority:low)
- #453 flaky test (priority:high だが reproducible 困難、 別問題で stuck)

## Known issues / 注意点

### 既知 flaky test
- `survey_command_outbox_holds_until_light_delay_elapses` (#453) — ~40% intermittent fail。 dispatch→process timing の over-gating が root cause、 Bug A 修正後も改善せず。 isolation single-test では 8/8 pass、 並列実行下のみ顕在化。 #465 を dup として close 済
- `cascade_ack_propagates_to_children` (`esc_notification_pipeline`) — 並列下のみ稀に flake、 isolation で 3/3 pass

### Code cleanup 残り
- `mid_adapter.rs` / `mid_stance.rs` / `npc_decision.rs` / `debug_log.rs` / `ai_player_e2e.rs` の doc comment に `Mirrors SimpleNpcPolicy::decide ...` 系の stale reference (~25 箇所) — 本セッション最後に cleanup pass 予定 (background agent)、 完了後 commit

### #448 PR4 (skipped、 deletable)
- 共通 `Agent` super-trait via blanket impl
- ai-core 単独 ~200 LoC、 cosmetic
- 必要になったら別 issue で再起票推奨

### `AiPolicyMode` 削除後の戻り道
- `AiPolicyMode` enum を削除済 → 代替 policy が必要になったら新 enum 作成 (1-variant の dead code を残さない原則)
- BRP 経由で AI policy を runtime 切替したいなら別 mechanism 検討

### `ai-log` feature
- 別 PR (前 round) で `BufWriter<File>: !Reflect` の build error 残存。 production path には影響しないが `cargo build --features ai-log` で error。 別 issue 化候補

## ファイル別主な変更点 (sessoin span)

### 新規作成
- `macrocosmo-ai/src/proposal.rs` (PR2a)
- `macrocosmo-ai/tests/proposal_types.rs` (PR2a)
- `macrocosmo/src/ai/mid_adapter.rs` (PR2b → PR3d で `AiPolicyMode` 削除)
- `macrocosmo/src/ai/mid_stance.rs` (PR2c)
- `macrocosmo/tests/ai_npc_avoid_hostile_systems.rs` (Bug B)
- `macrocosmo/tests/ai_npc_outbox_dedup.rs` (Bug A)

### 削除
- `macrocosmo/tests/ai_layered_parity.rs` (PR2c で created → PR3d で deleted)
- `macrocosmo/tests/ai_policy_mode_gate.rs` (PR2b で created → PR3d で deleted)

### 大規模変更
- `macrocosmo/src/ai/npc_decision.rs` 1641 → 550 LoC (PR3d で SimpleNpcPolicy + tests 削除)
- `macrocosmo-ai/src/agent.rs` (PR1 で State 構造体追加)
- `macrocosmo-ai/src/orchestrator.rs` (PR1 で OrchestratorState に thread)
- `macrocosmo-ai/src/ids.rs` (PR1 で 4 placeholder id 追加)
- `macrocosmo-ai/src/lib.rs` (re-export)
- `macrocosmo/src/ai/plugin.rs` (`AiPolicyMode` 登録 → 削除)
- `macrocosmo/src/persistence/save.rs` (Bug C で SAVE 11→12)
- `macrocosmo/src/persistence/savebag.rs` (Bug C で `SavedPendingAssignment.stale_at` 削除)
- `macrocosmo/tests/fixtures/minimal_game.bin` (Bug C で regen、 798 B、 PR3 では touch せず)
- `macrocosmo/tests/common/mod.rs` (PR2d で capabilities fix)

## 後半: Post-#448 fix sweep + per-empire 化 (同セッション継続)

#448 完了後、 user 指示で 0.3.1 milestone の単発 fix と per-empire 化系を一気に潰す。 計 9 commits + 9 issues close。

### Category 3: priority:high 単発 fix (4 commits)

| Commit | # | 内容 |
|---|---|---|
| `b62048a` | #461 | Lua `request_command` を local/remote 自動 routing 化。 issuer-target 異 system は `PendingScriptedCommand` で光速遅延、 `dispatch_pending_scripted_commands` system で arrival 時に typed message emit。 4 regression tests |
| `b66b60b` | #460 | CB auto-war / forced peace に sender-immediate / receiver-delayed semantics 適用。 `DIPLO_FORCED_PEACE` event kind 新設 (DIPLO_PROPOSE_PEACE は round-trip auto-accept のため再利用不可)。 5 regression tests |
| `97be643` | #462 | `context_menu` の direct `ShipState` write を `apply_local_ship_command` helper でラップ、 `debug_assert_eq!(expected_delay, 0)` で local-only invariant 強制。 `ShipState` に `Clone` derive 追加。 3 tests |
| `d5e248d` | #440 | Observer mode `read_only` を全 UI write path に展開。 4 chokepoint gate (`gate_diplomacy_action`/`gate_research_action`/`gate_ship_designer_action`/`gate_system_panel_writes`) で system_panel の 全 write を 3 sink (PendingColonyDispatches / colonization_actions / SystemPanelActions) で集約 truncation。 8 tests。 issue body 過大計上 (situation_center は既に display-only、 context_menu は既存 gate あり) |

### Category 2: per-empire 化 sweep (5 commits)

Round 9 PR #1/#2 で始まった per-faction 化の継続: 「player-only 経路 → per-empire 経路」 の sweep。

| Commit | # | 内容 | SAVE |
|---|---|---|---|
| `e8275d9` | #456 | `process_surveys` の light/FTL 判定を ship owner 基準に。 `Owner::Empire(e) → EmpireRuler → Ruler.StationedAt` で reference position 解決、 owner empire の `GlobalParams.ftl_speed_multiplier` 使用 (was: PlayerEmpire 固定)。 3 tests | 12 維持 |
| `0246c9e` | #457 | `sensor_buoy_detect_system` / `relay_knowledge_propagate_system` を per-empire viewer 化。 各 empire の `EmpireViewerSystem` から距離計算 → per-empire `observed_at`。 receiver-keyed `hostile_map` を `FactionRelations::get_or_default(receiver, hostile_owner)` で構築 (was: 最初の empire の view を全 empire に spray)。 3 tests | 12 維持 |
| `c266f98` | #458 | `PendingResearch` / `PendingKnowledgePropagation` に `owner: Entity` field 追加。 `emit_research` が colony `FactionOwner → HomeSystem` で delay anchor、 `receive_research` は owner 限定 ResearchPool に加算 (was: 全 empire pool に加算 = 経済漏洩 critical bug)。 3 tests | **12 → 13 + fixture regen** |
| `6b128dd` | #463 | `events.rs` module docstring で `GameEvent = omniscient simulation/audit only` を契約明文化。 `KnowledgeFact::CoreConquered` 新 variant 追加 (`record_for` で全 empire 光速遅延 propagation)、 non-FTL survey 完了時の `AnomalyDiscovered` emit 修正 (FTL path のみ wired だった bug)。 4 tests + 1 doc-presence guard | **13 → 14 + fixture regen** |
| `8b31b51` | #464 | `KnownFactions` を Resource → Component (per-empire) に migrate。 `detect_faction_discovery` を 全 Empire 対象に (Co-location + FactionRelations 経由)。 UI が viewer empire 参照に切替。 `KnownFactions::is_known(target)` + `find_known_factions(world, empire)` helper API。 7 tests | **14 → 15 + fixture regen** |

### Issues 整理 (本 session 全体)

```
Closed:
  #448 (8 sub-PRs landed)
  #443 (Round 10 deploy_core で実装済)
  #454 (Bug A 7d8f089 で fix)
  #465 (#453 の dup)
  #461 #460 #462 #440 (category 3 全件)
  #456 #457 #458 #463 #464 (category 2 全件)
合計 13 close。

Filed (new this session, not closed):
  #466 ThreatState 機構
  #467 Mid-Mid 競合解決 (FCFS arbiter + rejection 通知 設計)
両方 0.3.1 milestone。
```

### SAVE_VERSION の遷移

`11 → 12 (Bug C) → 13 (#458) → 14 (#463) → 15 (#464)`

各 bump は postcard positional encoding の wire-break または schema 拡張に対応。 fixture (`tests/fixtures/minimal_game.bin`) を SAVE bump ごとに regen、 最終 803 B (v15)。

### Test status (最終)

- `cargo test --workspace --tests`: 全 binary green (`survey_command_outbox_holds_until_light_delay_elapses` #453/#465 既知 flaky のみ偶発、 isolation で pass、 並列実行下のみ)
- 23 commits ahead of `origin/main`、 `5d65a83` (前 handoff commit) → `8b31b51`

## 次セッション最優先 (再確認)

### Top: #449 Region 概念導入

AI architecture 中核。 #450/#451/#452/#467 全ての前提。 単一 Mid → N Mid 化 + Short の per-fleet/per-colony 化。 PR3 で Mid 暫定 host にした Rules 2/5b の Short 再移植 もここ。 Plan agent → sub-PR 分解推奨 (`docs/session-handoff-2026-04-27-ai-decomposition.md` line 211-242 + 本 doc 「#467 Mid-Mid 競合解決」 section が確定済 spec)。

### 中位

| # | priority | 内容 |
|---|---|---|
| #466 | medium | ThreatState 機構 (依存: per-empire ShipDestroyed propagation の続編) |
| #347 | medium | In-game keybinding manager + UI |
| #455 | low | clippy 警告整理 |
| #411 | medium | 戦闘 report 可視化 |
| #445 | medium | shipyard_capacity 値活用 |
| #467 | medium | Mid-Mid 競合解決 (#449/#450 後) |
| #459 | low | CommandLog 意図確定 |
| #453 | high | flaky outbox test 修正 (root cause 困難) |

### Skip 候補
- #394 crate 分割 (icebox)

## さらに後半: #449 Region 概念導入 (同セッション継続、 Round 12 相当)

#449 を 6 sub-PRs で landing。 user 確定 architecture: **state-on-Component (Option c)** — orchestrator wrapping を game side から削除し、 agent state を MidAgent / ShortAgent Components に直接持たせる。

### sub-PR 6 件

| Commit | PR | 内容 |
|---|---|---|
| `980400a` | 2a | `Region` Component (`empire / member_systems / capital_system / mid_agent`) + `RegionMembership` 逆 index + `RegionRegistry` Resource + `EmpireLongTermState` Component。 PR1 の `OrchestratorState.long_state` を Empire entity に migrate (caller 不在で機械的) |
| `2754321` | 2b | `MidAgent` Component が `state: MidTermState` を `#[reflect(ignore)]` で直接保持。 `npc_decision_tick` を per-MidAgent loop に refactor (15 SystemParam)、 `BevyMidGameAdapter` に `member_systems` filter 追加 (4 site)。 既存 AI integration tests 用の `backfill_mid_agents_for_ai_controlled` system で test 経路を温存 |
| `1aeabce` | 2c | `ShortAgent { managed_by, scope: ShortScope::Fleet/ColonizedSystem, state: PlanState, auto_managed }` Component + `Added<Fleet>`/`Added<Colony>` 経由 spawn hook + `despawn_orphaned_short_agents` reaper。 game-side `OrchestratorRegistry` / `FactionOrchestrator` / `register_demo_orchestrator` / `run_orchestrators` を全削除 (= 295 LoC delete)、 `run_short_agents` system が `CampaignReactiveShort::tick` を per-ShortAgent で呼ぶ |
| `0bbe4da` | 2d | Rules 2 (survey) / 5b (slot fill) を Mid から Short に **一括 cutover**。 `ShortGameAdapter` trait + `BevyShortAgentAdapter` 構造体 + `ShortStanceAgent::decide` 新設。 `MidGameAdapter` から Rule 2/5b 用 method 削除。 同 tick 内 double-claim 防止 `claimed_survey_targets` set (Bug A の cross-tick dedup の同 tick 補完)。 sentinel test 2 件で Round 11 emit shape 不変を pin |
| `e03e186` | 2e | persistence: 6 savebag shims (`SavedRegion`/`SavedRegionMembership`/`SavedRegionRegistry`/`SavedEmpireLongTermState`/`SavedMidAgent`/`SavedShortAgent` + `SavedShortScope` enum)。 SAVE_VERSION **15 → 16**、 fixture regen (803 → 829 B)、 v15 strict reject (既存 policy 整合)。 `assign_save_ids` の `Or<>` bundle に `With<Region>`/`With<MidAgent>`/`With<ShortAgent>` 追加 |
| `4bfefbb` | 2f | per-Region NPC e2e smoke test (production code 不変、 +736 LoC test)。 2-region empire で Mid 独立 emit + cross-region leak なし、 save/load round-trip を統合検証 |

### 重要な設計判断

#### Orchestrator 削除の現実
当初 plan は `Orchestrator` 全削除だったが、 macrocosmo-ai の `OrchestratorState` は `intent_queue` / `pending_specs` / `campaigns` / `override_log` / `drop_log` 等の field を **依然保持**しており、 abstract scenario harness (`macrocosmo-ai/tests/scenario_*.rs` 10+ tests) で使われ続ける。 game-side wrapping (`OrchestratorRegistry` Resource、 `register_demo_orchestrator` system 等) は完全削除、 ai-core 側は engine-agnostic harness として温存、 という現実的着地。

#### state-on-Component の整理結果
- `LongTermState` → `EmpireLongTermState { inner: macrocosmo_ai::LongTermState }` Component on Empire entity
- `MidTermState` → `MidAgent.state` (`#[reflect(ignore)]` で wrap)
- `PlanState` → `ShortAgent.state` (`#[reflect(ignore)]` で wrap)
- 各 macrocosmo-ai 側 type は serde-derive 済 (PR1)、 savebag は serde 経由 passthrough、 ai-core-isolation 維持

#### Rules 2/5b cutover
- 一括 cutover (PR2d)、 sentinel test で Round 11 emit shape mirror を pin
- Mid 残存 rule: 1/3/5a/6/7/8 (= empire-level / region-level の 6 rules)
- Short 担当 rule: 2 (per-fleet survey)、 5b (per-colony slot fill)

#### 同 tick double-claim race fix (PR2d 副産物)
per-fleet ShortAgent split で同 tick 内に 2 fleet が同 target に survey emit する race が露見 → `run_short_agents` 内 `claimed_survey_targets: HashSet<(empire, target)>` で per-tick block。 Bug A (Round 11) の cross-tick outbox dedup の同 tick 補完。

### Multi-region は半分だけ完成
**重要 known limitation**: ShortAgent spawn hook が `RegionRegistry.by_empire[empire].first()` で MidAgent 解決、 multi-region empire でも全 ShortAgent が **primary region の MidAgent 配下** になる。 region isolation は今 Mid-decision layer (`Region.member_systems` filter) のみで実現中。 Per-system Fleet/Colony→MidAgent routing は **#471** (新規起票) で対応。 PR2f の e2e smoke が現 contract を explicitly pin、 #471 fix 時に test 更新で migration を gate。

### 関連 issue
- **#449 closed** (本セッション 6 sub-PRs)
- **#471 (新規起票、 0.3.1 milestone、 #451 blocked-by)**: per-system Fleet/Colony → MidAgent routing for multi-region empires
- **#451** (既存): Mid-Mid Short handoff、 #471 と統合検討候補
- **#450** (既存): inter-layer comm、 #471 の cross-region handover routing 経路

### Test status (#449 完了時点)
- `cargo test --workspace --tests`: 3300+ pass、 #465/#453 既知 flaky のみ偶発
- 11 new test files、 19 new integration tests across PR2a-2f
- ai-core-isolation CI 維持 (macrocosmo-ai に Bevy 依存なし)

### SAVE_VERSION 遷移 (本セッション通算)

`11 → 12 (Bug C) → 13 (#458) → 14 (#463) → 15 (#464) → 16 (#449 PR2e)`

最終 fixture: 829 B。

## 次セッション最優先 (改訂)

### Top: per-empire 化 sweep (priority:high 並列消化)
#449 が大規模 land 完了したので、 残り priority:high で並列化しやすい単発 fix:
- **#465/#453 flaky test** root cause investigation (long-standing、 dispatch→process timing over-gating)
- **#466 ThreatState** (依存: per-empire ShipDestroyed propagation の続編)

### 中位 (中規模)
- **#471 per-system ShortAgent routing** (#449 の真の完成、 #451 と統合検討)
- **#451 Mid-Mid Short handoff** + **#450 Inter-layer comm** + **#467 Mid-Mid arbiter** (geographic AI の残課題、 統合判断が必要)
- **#347 In-game keybinding manager + UI**

### 単発
- **#411** 戦闘 report 可視化
- **#445** shipyard_capacity 値活用
- **#455** clippy 整理
- **#459** CommandLog 意図確定

### Skip 候補
- #394 crate 分割 (icebox)

## 次セッション再開プロンプト例

```
2026-04-28/29 ハンドオフ参照。 docs/session-handoff-2026-04-28-round-11-ai-trait-unification.md
読んで全体像把握。 #448 / #449 完了で AI architecture の主軸 (3 layer + Region) は
landing 済。 残 0.3.1 milestone は per-empire/region sweep の細部 + flaky test
investigation。

優先度:
1. #471 per-system ShortAgent routing (#449 の補完、 中規模、 #451 と統合検討)
2. #451/#450/#467 inter-layer comm + Mid-Mid handoff arbiter (architectural、
   Plan agent → sub-PR 分解の流れ)
3. #466 ThreatState (依存: ShipDestroyed propagation)
4. flaky test #453/#465 root cause
```
