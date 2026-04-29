# Session handoff — 2026-04-28 Round 13: ShipProjection epic + adversarial debug

## TL;DR

巨大セッション。 commit 範囲 `c02edbf → ea9accc` で **17 commits** landing。 主な成果:

- **#471 / #453 / #472** — Round 12 残務の per-system ShortAgent routing + flaky test fix + ShipDestroyed/Missing 契約整理 (= #463 dual-write を ship destruction に拡張、 SAVE 16→17)
- **Epic #473 ShipProjection** — own-empire ship trajectory projection 機構を新規導入。 6 sub-issue (#474-#479) + 1 carry-over (#480 → #466) で landing。 Galaxy Map FTL knowledge leak の根本 fix、 AI Suspected 判定基盤、 dashed intended-trajectory layer。 SAVE 17→18
- **2 rounds of adversarial review + BRP exploratory** で 9 + 6 = **15 件の follow-up bug** を検出、 6 件の hotfix を即時 land。 SAVE 18→19 (#483)
- **計 16 issues closed**: #471, #453, #472, #474–#480, #481, #482, #483, #484, #485, #486, #487, #488, #489, #492
- **7 follow-up open** (0.3.1 milestone): #491, #493, #494, #495, #496, #497 + 2 epic-related (#466 ThreatState Phase 2 本体、 #467 Mid-Mid arbiter、 #490 omniscient mode)

`cargo test --workspace --tests` 最終 **3377 passed / 0 failed / 1 ignored** (regenerate-fixture)。 SAVE_VERSION 19、 fixture (`tests/fixtures/minimal_game.bin`) 829 B。

前段: `docs/session-handoff-2026-04-28-round-11-ai-trait-unification.md` (Round 11 dispatch correctness + #448 trait unification + Round 12 #449 Region 化)。

## Commit 順 (新しい順)

```
ea9accc fix(ship): tighten dispatcher ShipProjection guard (#492)
e41bbbd fix(ship): defensive ShipProjection write in dispatch_queued_commands (#488)
0aa9618 fix(knowledge): three polish fixes (#484, #485, #486)
379e8e1 feat(viz): widen intended-trajectory alpha curve + dash-pattern variation (#489)
2a1dd12 fix(ui): outline tree uses ShipProjection (#487)
6f6d6e2 fix(persistence): persist KnowledgeFact ship field (#483)
71184d1 fix(viz, ui): close ShipProjection coverage gaps from #473 (#481, #482)
1b93c1f test(knowledge): integration regression suite for ShipProjection (#479)
8ca1c39 feat(viz): intended-trajectory dashed layer (#478)
4b45b41 feat(viz): render own-ship galaxy map from ShipProjection (#477)
b112faf feat(ai): is_ship_overdue helper for ThreatState Suspected basis (#480)
bdc6f4c feat(knowledge): reconcile ShipProjection from KnowledgeFact stream (#476)
70a61c6 feat(knowledge): compute ShipProjection at all command-dispatch sites (#475)
1d10bc6 feat(knowledge): introduce ShipProjection data model + per-empire storage (#474)
05be837 fix(events): per-faction ShipDestroyed/Missing facts (#472)
6cdbcab test(ai): fix #453 flaky outbox test
bcbc882 fix(ai): per-system MidAgent routing for multi-region ShortAgents (#471)
```

## Round 13 内訳

### A. Round 12 残務 (3 commits)

#### #471 (`bcbc882`) — per-system ShortAgent routing

`#449 PR2c` の制限事項 = `region_registry.by_empire[empire].first()` で primary region's MidAgent を解決していた → multi-region empire で region B の fleet/colony が region A's MidAgent 配下に誤帰属。

Fix: `resolve_mid_agent_for_system{,_world}` 3-tier fallback (system membership → empire-region scan → empire primary)。 spawn hook 2 つ + 新規 `rehome_fleet_short_agents` system (per-tick fleet flagship region 跨ぎ追跡)。 #449 PR2f e2e smoke の assertion を flip (region A → mid_a、 region B → mid_b)。 +3 regression test (`ai_short_agent_per_region_routing.rs`)。

#### #453 (`6cdbcab`) — flaky outbox test fix

`survey_command_outbox_holds_until_light_delay_elapses` が ~40% intermittent fail。 root cause = **observation race** (over-gating ではなかった): `iter_current_update_messages()` は double-buffered Messages の current buffer のみ見るため、 305-tick advance loop 後の単発 check が rotation 済 buffer を読んでた。 Round 11 handoff の hypothesis (dispatch→process timing race) は誤り。

Fix: per-tick accumulator + outbox 直接 probe (`outbox_holds_survey_for`)。 10/10 isolation pass。

#### #472 (`05be837`) — per-faction ShipDestroyed/Missing facts (SAVE 16→17)

`#463` で codify した dual-write 契約 (immediate audit `GameEvent` + per-faction `KnowledgeFact` via `record_for`) を ship destruction にも適用。

- `GameEvent::ShipDestroyed` 発火を `update_destroyed_ship_knowledge` の light-arrival timing から **destruction site (`ship/combat.rs` の 3 箇所)** に移動、 即時単一発火に
- `KnowledgeFact::ShipDestroyed/Missing` 新設、 destruction site で `fact_sys.record_for` per-faction emit (mirror `CoreConquered`)
- `GameEvent::ShipMissing` 撤去 (per-observer epistemic state は audit に載らない、 fact のみで表現)
- `update_destroyed_ship_knowledge` の `Some(empire_entity) == player_empire` gate 撤去
- `kind_registry.rs` に `core:ship_destroyed` / `core:ship_missing` 登録
- 5 regression tests (`ship_destruction_observation_contract.rs`)

### B. Epic #473 ShipProjection (6 + 1 sub-issues)

#### 背景

光速遅延 game contract に対して 2 つの構造的 leak が存在:

1. **Galaxy Map FTL leak**: 自国 ship の `ShipState::InTransit / Surveying / Settling` が realtime ECS 直読で render → dispatch 直後 (command が物理的に ship に届いていない時点) でも player は ship の現実状態を見える
2. **AI Suspected 判断基盤不在**: ThreatState (`#466`) の Suspected seed = 「派遣した ship が予定 tick までに帰ってこない」 を realtime 判定すると同様の FTL leak

両方の根 = empire が自国 ship について 「**dispatch 時点で local info から計算した予測軌跡**」 を持っていないこと。

#### 解決設計: `ShipProjection`

per-empire の `KnowledgeStore.projections: HashMap<Entity, ShipProjection>`。 fields (#474 で固定):

```rust
pub struct ShipProjection {
    pub entity: Entity,
    pub dispatched_at: i64,
    pub expected_arrival_at: Option<i64>,
    pub expected_return_at: Option<i64>,
    pub projected_state: ShipSnapshotState,        // 知識ベース
    pub projected_system: Option<Entity>,
    pub intended_state: Option<ShipSnapshotState>, // 意図ベース
    pub intended_system: Option<Entity>,
    pub intended_takes_effect_at: Option<i64>,     // dispatch + light_delay_to_ship
}
```

UI 上で **projected (= 知識、 light-coherent solid) と intended (= 意図、 dashed/translucent) を 2 layer 描画**。 命令到達予定 tick で intended が projected に「合流」 (alpha + dash pattern 変化で表現)。

#### 6 sub-issues (順)

| Commit | # | 内容 | Size |
|---|---|---|---|
| `1d10bc6` | #474 | データモデル + per-empire storage + persistence shim (SAVE 17→18) | S |
| `70a61c6` | #475 | dispatch-time computation at AI / Lua / player paths | M |
| `bdc6f4c` | #476 | reconcile from `KnowledgeFact` stream (event-driven) | M |
| `4b45b41` | #477 | Galaxy Map own-ship render を projection ベースに切替 | M |
| `8ca1c39` | #478 | intended-trajectory dashed layer (alpha 0.4→0.8、 dash 4/2) | M |
| `1b93c1f` | #479 | integration regression suite (4 tests) | S |
| `b112faf` | #480 | `is_ship_overdue` AI helper (#466 carry-over、 helper-only) | S |

#### 重要な設計判断

- **Storage choice**: Component on Empire `KnowledgeStore` (parallel to `ship_snapshots`)、 user の Q1 提案を採用
- **`intended_*` clearing**: matching arrival で clear (= 命令完了で fade)
- **`ShipDestroyed` 反映**: projection を `Lost` state で **retain** (= situational memory)
- **Reconciler ordering**: `.after(propagate_knowledge, update_destroyed_ship_knowledge, flush_ship_projection_writes, advance_game_time)`
- **Per-empire isolation**: reconciler が `compute_fact_arrival(observed_at, origin_pos, vantage_pos)` で per-empire arrival recomputation
- **`KnowledgeFact::{ShipArrived, SurveyComplete, ShipDestroyed, ShipMissing}` に `ship: Entity` field 追加** — 当初 `SavedKnowledgeFact` 側で drop + `Entity::PLACEHOLDER` rehydrate (後の #483 で正規化、 SAVE 18→19)

### C. Round 1 adversarial + BRP debug → 9 follow-up issues + 6 hotfix

implementing agent + cargo test 全 green は 3346 passed / 0 fail。 ただし adversarial review + BRP exploratory が **runtime regression を発見**:

**Critical (即 hotfix)**:
- `#481` own-empire ship が freshly-spawned で Galaxy Map から消える (spawn-time projection seed 不在) — fresh game で player の initial fleet 全消失
- `#482` zero-delay context-menu commands が projection write skip — ruler 同所配備の最も「直接管理」 経路で projection 不在

**High**:
- `#487` outline tree が realtime `ShipState` 直読 (= FTL leak parity が Galaxy Map で Local だけ達成)
- `#488` dispatcher path 直接 CommandQueue push が projection write 通さない (= BRP / 将来 plugin / test の覆い)
- `#483` saved `KnowledgeFact` が `ship` field drop → in-flight fact が save/load 跨ぎで lost (`Entity::PLACEHOLDER` rehydrate)
- `#484` reconciler `intended_*` clear が system identity だけで gate (mission-id gate 不在)

**Polish**:
- `#485` `flush_ship_projection_writes` の `InGame` gate 抜け
- `#486` `compute_ship_projection` saturation handling
- `#489` intended-trajectory alpha 視認性 (curve `0.4 → 0.8` が dark map で人間目には等価)

#### Hotfix wave (6 commits)

| Commit | # | Files | LoC |
|---|---|---|---|
| `71184d1` | #481 + #482 | knowledge/mod.rs (seed system)、 ui/context_menu.rs (zero-delay write)、 ui/mod.rs | +695 / -3 |
| `6f6d6e2` | #483 | persistence/savebag.rs、 save.rs (SAVE 18→19)、 fixture regen | +431 / -42 |
| `2a1dd12` | #487 | ui/outline.rs (rewrite via `ship_outline_view` helper)、 ui/mod.rs | +938 / -90 |
| `379e8e1` | #489 | visualization/ships.rs (alpha 0.3→1.0、 dash 4/2→8/4 interpolation) | +282 / -43 |
| `0aa9618` | #484+#485+#486 | knowledge/mod.rs (3 polish fixes 1 file) | +704 / -7 |
| `e41bbbd` | #488 | ship/dispatcher.rs (defensive projection write) | +709 / -3 |

各 fix に dedicated regression test を追加。 計 **9 new test files + 27 new tests**。

#### #487 observer mode patch (途中軌道修正)

#487 implementing agent が当初「observer mode = god-view → realtime ECS fallback」 と解釈してた。 user が「empire-view と god-view は別 mode」 と clarify、 patch:
- observer mode を **empire-view (light-coherent)** に統一
- god-view (= omniscient) は **#490 で別 mode** として future work
- test 5 (`outline_observer_mode_is_light_coherent_via_projection`) を新 contract に flip

### D. Round 2 adversarial + BRP debug → 6 follow-up issues + 1 hotfix

hotfix 6 件が landed 後、 round 2 で再 review。 結果:

**HIGH**:
- `#492` `#488` dispatcher 防衛 write が **`#481` spawn seed の存在で dead-on-arrival** — `is_some()` guard が常に発火 → defensive write 一度も走らない。 BRP exploratory で runtime confirm (Galaxy Map に Survey command 注入後 51 hex 経過しても「Sol 停泊」 のまま、 dashed layer 描画なし)

**MEDIUM** (open follow-ups):
- `#493` dispatcher 防衛 write が **validation 前** に発火 — drop された command でも projection が leak
- `#494` `region_persistence` v18 strict-reject test が **current-shape の version field 上書き** してるだけで真の v18 wire 形式を test していない
- `#495` `outline_observer_mode_is_light_coherent_via_projection` が degenerate (`viewing_empire == ship.owner` → foreign-ship 観測 case が uncovered)

**Polish** (open follow-ups):
- `#496` renderer 側 alpha/dash の `i64::MAX` saturation slip-through guard 不在
- `#497` (bundle) seed `dispatched_at` semantic + `ProjectionWriteParams` 重複 query + `_is_observer` 残骸

#### #492 hotfix (`ea9accc`)

guard 改善: `(intended_state, intended_system)` を head queued command と比較して match 時のみ skip:

```rust
let existing_matches = store.get_projection(ship)
    .map(|p| p.intended_state == intended_state
          && p.intended_system == intended_system)
    .unwrap_or(false);
if existing_matches { return; }
```

これで:
- Seed (intended=None) + 新 command (intended=Some) → mismatch → overwrite ✓
- Caller wrote (intended=X) + queue head (intended=X) → match → skip ✓
- Caller wrote (intended=X) + queue head (intended=Y) → mismatch → overwrite (= caller's stale write 上書き) ✓
- Post-reconcile (intended=None) + 新 command → mismatch → overwrite ✓

既存 `dispatcher_does_not_overwrite_fresh_caller_projection` test は **buggy 挙動を pinning** していたため削除、 4 split test に置換。 test scaffolding に `seed_own_ship_projections` を `.before(dispatch_queued_commands)` で wire (= production-shape 再現)。

## 起票・整理した issues

### 新規 (本セッション起票)

| # | severity | 内容 | 状態 |
|---|---|---|---|
| #472 | bug | per-faction ShipDestroyed/Missing facts | closed |
| #473 | epic | ShipProjection epic | closed |
| #474–#479 | sub | epic #473 sub-issues | closed |
| #480 | sub | is_ship_overdue helper (#466 carry) | closed |
| #481 | critical | spawn-time projection seed 不在 | closed |
| #482 | critical | zero-delay context-menu projection skip | closed |
| #483 | high | KnowledgeFact ship field persistence | closed |
| #484 | high | reconciler intended_* clear mission-id gate | closed |
| #485 | polish | flush InGame gate | closed |
| #486 | polish | saturation handling | closed |
| #487 | high | outline tree FTL leak | closed |
| #488 | high | dispatcher path bypass | closed |
| #489 | polish | alpha visibility | closed |
| #490 | enhancement | omniscient mode | open |
| #491 | bug | other UI panel FTL leak audit | open |
| #492 | high | dispatcher guard dead-on-arrival | closed |
| #493 | medium | dispatcher pre-validation leak | open |
| #494 | medium | persistence test rigor | open |
| #495 | medium | observer mode test gap | open |
| #496 | polish | alpha saturation guard | open |
| #497 | polish (bundle) | seed dispatched_at + 重複 query + _is_observer | open |

合計: **20 起票、 16 closed、 4 + 3 open** (open は milestone 0.3.1)。

### 整理 (close — 本セッション中に landing で auto-close)

`#471, #453, #472, #474, #475, #476, #477, #478, #479, #480, #481, #482, #483, #484, #485, #486, #487, #488, #489, #492` (= 20 件)

## Architecture / 設計メモ

### `ShipProjection` の write path 5 producers

post-#492 fix 状態:

1. **Spawn seed** (`#481`、 `seed_own_ship_projections`) — `Added<Ship>` で own-empire ship に projection_state=`InSystem`、 intended_*=None
2. **AI command outbox** (`#475`、 `dispatch_ai_pending_commands`) — AI が dispatch する瞬間に accurate projection を write
3. **Lua `request_command`** (`#475`、 `apply::request_command`) — sender's tick に write (call-time semantic)
4. **Player UI `pending_ship_commands`** (`#475`、 `draw_main_panels_system`) — egui frame で `commands.queue` 経由 deferred write
5. **Player UI zero-delay context-menu** (`#482`、 `ContextMenuActions.zero_delay_dispatches`) — `commands.queue` 経由 deferred write
6. **Dispatcher 防衛 write** (`#488` + `#492`、 `dispatch_queued_commands` 内の `maybe_write_dispatcher_projection`) — head command が既存 projection と mismatch の時だけ write

guard predicate (`#492` after): `existing.intended_state == head_intended && existing.intended_system == head_target` で skip。

### Reconciler

`reconcile_ship_projections` (`knowledge/mod.rs`) が `KnowledgeFact::{ShipArrived, SurveyComplete, ShipDestroyed, ShipMissing}` 到達で per-empire 反映。 ordering: `.after(propagate_knowledge, update_destroyed_ship_knowledge, flush_ship_projection_writes, advance_game_time)`。

`#484` で `fact_observed_at >= projection.dispatched_at` mission-id gate を `ShipArrived/SurveyComplete` arms に追加。 `ShipDestroyed/ShipMissing` は unconditional clear (= ship 死亡で全 mission 終了)。

### UI render

#### Galaxy Map (`#477` + `#478`)

own-empire ship: `KnowledgeStore.projections` 経由 (= projection-driven render)
foreign ship: `KnowledgeStore.ship_snapshots` 経由 (= 既存 #175 ghost、 unchanged)

Loitering inline coords / Destroyed (filtered out of own-render) / Missing (amber pulse via legacy helper) / InTransit (fallback to `projected_system` の destination position — interpolation field なし、 follow-up)。

intended layer: `compute_intended_render_inputs` が divergence > 0 時のみ active、 alpha curve `0.3→1.0` (`#489`)、 dash pattern `4/2 → 8/4` interpolation。

#### Outline tree (`#487`)

`ship_outline_view` helper が own-empire / foreign の per-ship 経路選択。 全 outline section (In Transit / Stationed Elsewhere / docked / station) が helper 経由。 observer mode は light-coherent (= viewing empire の knowledge 経由)。

### Observer mode と omniscient mode

`#440` で導入された `ObserverMode { enabled, viewing_empire }` は **empire-view** = 「他 empire の視点で観る」。 `#487` patch で UI 全体が viewing empire の `KnowledgeStore` 経由で light-coherent。

god-view (= 全 empire realtime ECS) は **`#490` で別 mode** として future work。 design 案: `ObserverMode` を 3-variant enum (`Disabled / EmpireView / Omniscient`) に拡張、 `Omniscient` 時は realtime 直読 path を render code 全体に展開。

## 残 follow-up issues (0.3.1 milestone)

### High priority

- `#491` 他 UI panel FTL leak — `#487` audit が見つけた `ship_panel.rs`、 `context_menu.rs`、 `situation_center/ship_ops_tab.rs`、 `system_panel.rs`、 `ui/mod.rs` map tooltip 等。 `#487` と同 pattern で 5 panel に展開、 単独 PR で landing 想定 (規模 中-大)

### Medium priority

- `#493` dispatcher pre-validation projection write leak — validation 後に move、 もしくは pre-check
- `#494` persistence v18 strict-reject test rigor — 真の v18 wire byte array fixture を hand-craft
- `#495` observer mode foreign-ship test gap — `viewing_empire ≠ ship.owner` case を pin

### Polish

- `#496` renderer alpha/dash saturation slip-through guard
- `#497` (bundle) seed `dispatched_at` sentinel + 重複 query 統合 + `_is_observer` 残骸処理
- `#490` omniscient mode (god-view) — 別 mode として独立追加

### Epic-related

- `#466` ThreatState Phase 2 本体 — Phase 1 の prerequisite (`#472` ShipDestroyed facts) + projection 基盤 (`#480` `is_ship_overdue`) は landed、 ThreatStates Component / state transitions / ROE wiring がまだ。 推定 規模 大、 design questions 残り (transition cadence、 storage shape)
- `#467` Mid-Mid 競合解決 (FCFS arbiter + rejection 通知) — `#449/#450` 後続、 architectural

## SAVE_VERSION 遷移 (本 session 通算)

`16 → 17 (#472) → 18 (#474) → 19 (#483)`

各 bump で fixture (`tests/fixtures/minimal_game.bin`) regen。 最終 829 B。 全 bump で `load.rs` strict-reject 設定 (`#494` で test rigor 補強の余地あり)。

## Test status (最終)

- `cargo test --workspace --tests --no-fail-fast`: **3377 passed / 0 failed / 1 ignored** (132 binaries)
- 既知 flaky:
  - `survey_command_outbox_holds_until_light_delay_elapses` (#453 で fix 済、 安定)
  - `ack_affects_esc_queue_only_not_banner` (`esc_notification_pipeline`) — 並列下のみ稀に flake、 isolation で 5/5 pass
  - `apply_pending_acks_system_drains_buffer_and_acks_queue` (`ui::situation_center::notifications_tab`) — pre-existing global-`PENDING_ACK_BUFFER` race、 isolation で pass

## Tooling friction (BRP exploratory で観察)

- `bevy/query` は Bevy 0.18 で **存在しない** — 実 method は `world.query / get_components / mutate_components / list_resources / get_resources`
- `world.mutate_components` enum serialization: external-tag string for unit variants、 struct variants は reject
- `macrocosmo/eval_lua` は `gs` global を持たない (event-callback scope のみで build される)
- `UiElementRegistry` は top-bar buttons のみ — ship right-click / context-menu / save buttons は absent
- `world.mutate_resource` 不存在 — runtime での `ObserverMode` toggle 不可

= BRP infra 改善が dev velocity に効く (= 別 issue 候補)

## Code cleanup 残り

なし (本 session で意識的に対応済)。

## 次セッション最優先

### Top: `#491` 他 UI panel FTL leak

`#487` の outline tree fix は完了、 ただし audit で **5 panel に同種 leak が残ってる**。 player UX の light-speed coherence は UI 全体で達成すべき。 規模 中-大、 panel ごとに sub-PR 分解できる:
1. `ship_panel.rs` (最大、 single PR)
2. `context_menu.rs`
3. `situation_center/ship_ops_tab.rs`
4. `system_panel/mod.rs::station_ships_q`
5. `ui/mod.rs` map tooltip

`outline.rs` で factor された `ship_outline_view` / `ShipOutlineView` helper を共通化して各 panel 消費。

### 次: Medium follow-ups

`#493 / #494 / #495` を並列で潰すと効率的。 異 file 群、 各々 small-medium。

### 中位

- `#466` ThreatState Phase 2 — projection 基盤 + #472 ShipDestroyed facts が unblock 済、 design questions 確認後に着手
- `#490` omniscient mode — `ObserverMode` enum 拡張、 規模 中
- `#496 / #497` polish

### Skip 候補

- `#394` crate 分割 (icebox)
- `#467` Mid-Mid arbiter — `#449/#450` 後続、 design phase

## 次セッション再開プロンプト例

```
2026-04-28/29 ハンドオフ参照。
docs/session-handoff-2026-04-28-round-13-shipprojection.md
を読んで全体像把握。 epic #473 ShipProjection は完了、
2 rounds の adversarial + BRP debug を経て #492 まで land。
SAVE_VERSION 19。 残 0.3.1 milestone は UI light-coherence の
他 panel 展開 (#491) と medium/polish 群。

優先度:
1. #491 他 UI panel FTL leak (ship_panel / context_menu /
   situation_center / system_panel / map tooltip 5 件)
2. #493 + #494 + #495 medium follow-up を並列で
3. #466 ThreatState Phase 2 本体 (Region 基盤 + projection
   helper が unblock 済)
4. #490 omniscient (god-view) mode
```

## 重要な caveat

- `KnowledgeFact::{ShipArrived, SurveyComplete, ShipDestroyed, ShipMissing}` の `ship: Entity` field は #483 で persistence-correct (SAVE 19)
- `ShipProjection` の write path は **6 producers** 並走、 `#492` の guard 改善で「intended match」 セマンティクスに統一
- observer mode は **light-coherent** (empire-view)、 god-view は `#490` で別 mode
- `update_projection` は plain insert (no observed_at dominance)、 reconciler が「newer info wins」 を担当
- `seed_own_ship_projections` の `dispatched_at = clock.elapsed` は post-load で `#484` mission-id gate と微妙な相互作用 — `#497` polish bundle で sentinel に変更検討
- BRP `mutate_components` の enum serialization 制約は dev tooling 側の制限、 production 動作には影響なし
