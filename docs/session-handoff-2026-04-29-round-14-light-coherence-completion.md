# Session handoff — 2026-04-29 Round 14: Light-coherence completion (#491 epic)

## TL;DR

Round 13 直後の continuation。 commit 範囲 `424e8a4 → 5472335` で **7 commits**。 主成果:

- **Epic #491** light-coherence completion = epic #473 ShipProjection の player UX 全展開: outline tree (`#487` 既存) + Galaxy Map (`#477` 既存) に加え、 残 5 panel (`ship_panel` / `context_menu` / `situation_center::ship_ops_tab` / `system_panel` / `ui::mod` map tooltip + camera centering) を **`ShipView` projection 経由** に rewire 完了
- **`ShipSnapshotState::InTransit`** を `InTransitSubLight` / `InTransitFTL` に split (= FTL 中は外部干渉不可の game contract を player UI で区別)。 SAVE_VERSION 19 → 20、 fixture 再生成
- **Module split**: `knowledge::ship_view` (domain data shape) + `ui::ship_view` (egui-adjacent helpers) で hierarchy 反転を防止
- **Post-planning projection upgrade hook** = `poll_pending_routes` で route plan 完了時に projection の `intended_state` を SubLight → SubLight/FTL に upgrade、 `KnowledgeFact::ShipDeparted` 不要 (= dispatcher empire の信念の自己更新)
- **ShipView accessors**: `position()`、 `estimated_position(timing, clock, origin_pos, dest_pos)`、 `is_actionable()`、 `is_in_transit()`
- **ShipViewTiming constructors**: `from_projection` / `from_snapshot` / `from_realtime` で 4 panel の timing ladder reinvent を統一
- **0.3.1 polish bundle**: #493 (dispatcher validation order) + #494 (wire test rigor) + #496 (renderer saturation guard) + #497 (seed sentinel + dup query)
- **計 7 issues closed**: #491、 #495、 #493、 #494、 #496、 #497、 + helper followup (= direct push、 issue なし)
- **1 follow-up open**: `#499` observer mode contract migration (= HIGH、 別 PR で 4 panel 統一 migrate 予定)

`cargo test -p macrocosmo --tests` 最終 **isolated 全 green**。 SAVE_VERSION 20、 fixture (`tests/fixtures/minimal_game.bin`) 829 B。

前段: `docs/session-handoff-2026-04-28-round-13-shipprojection.md` (Round 13 ShipProjection epic + 2 wave adversarial debug)。

## Commit 順 (新しい順、 main 上の squash 形)

```
5472335 polish(0.3.1): #493 dispatcher validation order + #494 wire test rigor + #496 viz saturation + #497 cleanup bundle (#504)
cc96c78 fix(ui): situation_center ship_ops_tab uses ShipView projection (#491 PR-4) (#503)
67a9c61 fix(ui): map tooltip + camera centering use ShipView projection (#491 PR-6) (#502)
3a2f26f fix(ui): ship_panel.rs uses ShipView projection (#491 PR-2) (#501)
1f5c584 fix(ui): context_menu.rs uses ShipView projection (#491 PR-3) (#500)
435cce1 feat(knowledge): hoist ShipViewTiming constructors + ShipView::is_actionable for #491 sub-PR rebase
4b6de5d refactor(ui): factor ShipView helper into shared ui/ship_view module (#491 prep) (#498)
```

## Round 14 内訳

### A. PR #498 prep — Helper extraction + 14-fix update + BLOCKER fix wave

#### Phase 1 (`900c130` original prep、 squash 内)

`#487` で `outline.rs` 内に factor された `ShipOutlineView` / `ship_outline_view` / `realtime_state_to_snapshot` を新 module `ui/ship_view.rs` に移動 + rename (= `ShipView` / `ship_view`)。 加えて 5 helper (`ship_view_status_label` / `ship_view_eta` / `ship_view_progress` + `ShipViewTiming` / `ShipStatusInfo`) 追加。 outline.rs は alias re-export shim 化。

3 wave の adversarial review + BRP exploratory QA を実施:
- **Wave 1** (prep PR review): 14 件の bug + design finding。 module placement / writer drift / timing semantic / `_is_observer` cleanup / API surface 過剰等
- **Wave 2** (update PR review): BLOCKER 3 件: (1) `KnowledgeFact::ShipDeparted` 不在で intended_state upgrade path vapor、 (2) snapshot writer の omniscient leak (false alarm、 後で却下)、 (3) 5 helper が production caller 0
- **Wave 3** (BLOCKER fix review): 残 nice-to-fix 群 (`is_overdue` boundary、 docstring 訂正、 degenerate test fix)

#### Phase 2 (`15132ab` + `ca67da8`、 14-fix update)

review wave 1 + 2 の statement を fix:
- **D-C-1 module split**: `ShipView` data shape を `knowledge::ship_view` に hoist (= domain helper、 UI 型ゼロ)。 `ui::ship_view` は egui-adjacent (`ship_view_status_label` / `ShipStatusInfo`) のみ。 `outline.rs` は 2 module からの re-export shim
- **D-C-2 writer drift**: `knowledge/mod.rs` snapshot writer + `deep_space/mod.rs` ×2 + `ship/scout.rs` を `realtime_state_to_snapshot` 経由に migrate (= writer/reader 単一 source of truth)
- **D-H-3 timing rename**: `ShipViewTiming.{started_at, expected_at}` → `{origin_tick, expected_tick}` + per-source semantics docstring
- **D-H-4 ShipSnapshotState split**: `InTransit` → `InTransitSubLight` / `InTransitFTL` + `is_in_transit()` predicate。 SAVE 19 → 20 bump、 fixture 再生成 (829 B 維持)
- **D-H-5/6 accessors**: `ShipView::position()` (Loitering coords)、 `estimated_position(timing, clock, origin, dest)` (in-transit lerp)
- **D-H-7 system_name 統合**: `ui::params` に集約 (`pub fn system_name`)、 ship_panel / outline / situation_center が共有
- **D-H-8 ShipStatusInfo drop**: tuple return (`(String, Option<ShipViewProgress>)`) に変更
- **D-M-9 `_is_observer` 削除**: signature から除去、 outline.rs callsite 8 箇所 update (`#497` cleanup を本 PR に取り込み)
- **D-M-12 ShipViewProgress 構造体化**: `{ elapsed (raw), total, fraction (clamped), is_overdue }`
- **B-NTF**: `saturating_sub` 切替、 `Destroyed/Missing/Loitering/InSystem` で `progress=None` force、 `Surveying { system: None }` → `"target unknown"` suffix

#### Phase 3 BLOCKER fix wave (`9bf2e8a` + `8ef1a98` + `9d4fe3e` + `9b85d1c` + `205d0b7`)

BLOCKER #1 (intended_state upgrade vapor) の根本治療:

**`poll_pending_routes` 内に projection upgrade hook 追加** — `routing.rs` で route plan の async task が complete した瞬間、 first segment kind (`RouteSegment::FTL` / `SubLight`) を見て dispatcher empire の `KnowledgeStore.projections.{ship}.intended_state` を **正しい variant** に upgrade。

設計判断:
- `KnowledgeFact::ShipDeparted` 新規追加は **不要** (= dispatcher empire の belief は観察ではなく自己更新、 reconciler 拡張不要)
- BLOCKER #2 (= snapshot writer omniscient leak) は **false alarm** と判定 (= snapshot writer は元から「観察者が ship を観察した時の state」 を保存する path、 SubLight/FTL 区別は observation contract の自然な拡張)
- BLOCKER #3 (= dead helpers) は **doc 化** で解決 (= 「PR-491 prep、 PR #2..#6 で消費」 を module / item doc に明記、 削除はせず)

加えて:
- `is_overdue` boundary fix (= `now > expected_tick` strict)
- observer mode contract drift (= production が `viewing_knowledge=None` を渡してる) を `knowledge::ship_view` の doc-comment に明記、 `#499` で別 PR migrate 予定
- `outline_observer_mode_is_light_coherent_via_projection` の degenerate fix = foreign-ship-as-observer sibling test 追加 → `#495` close
- v20 wire format integration test 追加 (`tests/ship_snapshot_persistence.rs`)

#### PR #498 本体 commit (squash → main `4b6de5d`)

`refactor(ui): factor ShipView helper into shared ui/ship_view module (#491 prep)` で 8 commits を集約 land。 25 files、 +2207/-379、 helper module split + ShipSnapshotState 拡張 + writer migration + 14 fix + BLOCKER #1 root fix + observer mode doc + v20 round-trip test。

### B. Helper followup direct push (`435cce1`)

post-#498 review batch wave (= 5 sub-PR の adversarial review) で発見された D-1 (= 3 panel が `ShipViewTiming` ladder reinvent) を **Stage 1** で集中処理:

- **`ShipViewTiming::from_projection(&ShipProjection)`** = `origin_tick = dispatched_at`、 `expected_tick = expected_arrival_at`
- **`ShipViewTiming::from_snapshot(&ShipSnapshot)`** = `origin_tick = observed_at`、 `expected_tick = None` (= snapshot は point-in-time observation のみで ETA を含まない、 honest semantic として inline doc)
- **`ShipViewTiming::from_realtime(&ShipState)`** = 全 8 ShipState variant cover
- **`ship_view_with_timing(...)`** = ladder 1 fn 提供 (`Option<(ShipView, ShipViewTiming)>`)
- **`ShipView::is_actionable()`** = `Destroyed`/`Missing` で false、 4 panel が destructive action を gate

15 unit tests 追加。 main に **直接 push** (= `gh pr merge` ではなく `git push origin main`、 既存 main commit pattern 準拠)。 SAVE bump なし。

### C. 5 sub-PR (panel rewires) — `#500` / `#501` / `#502` / `#503` + `#5` audit

5 panel 並列 worktree-isolated dispatch:

| Sub-PR | Panel | Final commit | Result | New tests |
|---|---|---|---|---|
| #500 | `context_menu.rs` | `aa6b3a8` → squash `1f5c584` | merged | 10 (= 7+3 terminal-state guards) |
| #501 | `ship_panel.rs` | `37a7492` → squash `3a2f26f` | merged | 9 (= 5+1 Refitting +3 actionable) |
| #502 | `ui/mod.rs` (map tooltip + camera) | `3566a33` → squash `67a9c61` | merged | 6 (= 5+1 observer regression) |
| #503 | `situation_center::ship_ops_tab.rs` | `a2f533e` → squash `cc96c78` | merged | 6 (= 5+1 observer regression) |
| #5 | `system_panel/mod.rs` | (no PR) | **no-op verdict** | — |

#### `#5` no-op 判定根拠

`system_panel/mod.rs` の `station_ships_q: &Query<(Entity, &Ship, &ShipState, &SlotAssignment)>` は `colony::system_buildings::slots_view` 経由で **station-slot occupancy** 判定にのみ使用。 station は `ship.is_immobile()` (= ftl_speed = 0、 sublight_speed = 0) で物理的に移動不可、 ShipState は事実上 `InSystem` 固定。 = game-logic ground truth、 player-facing 知識 view ではない。 FTL leak surface ではない。 [issue #491 コメント](https://github.com/frodo821/macrocosmo/issues/491#issuecomment-4344886578) で audit 結論を pin。

#### 並列実装の Stage 2 review wave + fix

**Stage 2** = 4 sub-PR を Stage 1 helper 上に rebase + adversarial batch review (1 体に 4 PR まとめ) で発見した BLOCKER 3 件:

- **B-1 + B-2 observer mode contract が 4 PR で不揃い**: PR #500 + #502 (camera) は `Some(knowledge)` 無条件 pass、 PR #501 は `if observer_mode.enabled { None }` gate、 PR #503 は `resolve_player_empire` で完全無視。 = 4 panel が 3 通りの empire-view contract → **既存 #499 pattern (= `viewing_knowledge=None` realtime fallback)** で全 4 PR 統一
- **B-3 semantic merge conflict risk**: 3 PR が `ui/mod.rs::draw_main_panels_system` を独立 hunk で touch → **順次 merge + cargo test** で対処 (= main session で sequential)

加えて Stage 2 で:
- **D-1 timing hoist 消費**: PR #501 / #503 が `ship_view_with_timing` 経由に migrate、 panel 内 `ship_panel_view_timing` / `timing_from_*` 等 reinvent を撤去
- **B-NTF Refitting test**: ROE command_delay が remote refitting ship で light_delay accrue を pin
- **B-NTF doc 訂正**: `ui::ship_view` module doc の "no production callers" 解除 (= PR #502 で 1 PR にまとめ)
- **`tooltip_status_word`** を `crate::ui::ship_view::tooltip_status_word(&ShipSnapshotState) -> &'static str` に hoist (= production と test で同 mapping、 mutation 耐性向上)

#### Sequential merge (= B-3 mitigation)

main session で `gh pr merge --squash --delete-branch` を順次:
1. PR #500 → main `1f5c584`、 mergeability 再計算待ち
2. PR #501 → main `3a2f26f`
3. PR #502 → main `67a9c61`
4. PR #503 → main `cc96c78`

各 merge 後の `gh pr view <next> --json mergeable,mergeStateStatus` で `MERGEABLE/CLEAN` を確認 (= UNKNOWN は transient)。 全 fast-forward、 conflict なし (= main `435cce1` が strict descendant)。

merge 後の main で `cargo test --workspace --tests --no-fail-fast` 全 green (parallel 実行で `apply_pending_acks_system_drains_buffer_and_acks_queue` flake 1 件、 isolated で pass)。

### D. 0.3.1 polish bundle (`5472335`、 PR #504)

session 末尾の polish:

- **#493** dispatcher pre-validation projection write leak: `maybe_write_dispatcher_projection` を per-variant validation block 内に移動。 invalid command (target gone / immobile / no-op same-system) で projection が leak しなくなった
- **#494** persistence v19 strict-reject test rigor: `tests/common/wire_format.rs` 新設、 v19 byte fixture builder helper hoist。 真 v19 hand-craft は **defer** 判断 (= SAVE 19→20 が `ShipSnapshotState` enum tag shift のみで GameSave 外形 wire-identical、 internal `savebag` field layout coupling を要求するため maintenance burden 大、 deferred extension docstring で trace)
- **#496** renderer alpha/dash saturation guard: `intended_layer_alpha` / `intended_layer_dash_pattern` の冒頭で `i64::MAX / 2` 閾値 short-circuit、 release build slip-through 時に floor / steady-state を返す
- **#497** bundle:
  - seed `dispatched_at = SEED_DISPATCHED_AT_SENTINEL (i64::MIN)` で post-load 誤 seed reconciler gate filter 防止 (= sentinel は `fact_observed_at >= dispatched_at` で trivially true、 branch logic 変更不要、 post-reconcile で natural promotion)
  - `ProjectionWriteParams` の `star_positions` / `ruler_system_positions` 重複 query 統合
  - `_is_observer` は **既に Stage 2 で除去済** (= PR #498 update commit `ca67da8`)、 doc reference のみ残

各 issue dedicated regression test 付き。 PR #504 → squash merge `5472335`。 SAVE bump なし。

## Architecture / 設計メモ

### `ShipView` data shape (= `knowledge::ship_view`)

```rust
pub struct ShipView {
    pub state: ShipSnapshotState,  // 9 variants incl. InTransitSubLight/FTL split
    pub system: Option<Entity>,
}

impl ShipView {
    pub fn position(&self) -> Option<[f64; 3]>;            // Loitering only
    pub fn estimated_position(&self, timing, clock, origin_pos, dest_pos) -> Option<Position>;  // in-transit lerp
    pub fn is_in_transit(&self) -> bool;                   // SubLight | FTL
    pub fn is_actionable(&self) -> bool;                   // !Destroyed && !Missing
}
```

### `ShipViewTiming` 3 source ladder

```rust
impl ShipViewTiming {
    pub fn from_projection(&ShipProjection) -> Self;       // own-empire = dispatcher's belief
    pub fn from_snapshot(&ShipSnapshot) -> Self;           // foreign = last observation
    pub fn from_realtime(&ShipState) -> Self;              // no-store = ECS ground truth
}

pub fn ship_view_with_timing(...) -> Option<(ShipView, ShipViewTiming)>;
```

3 source の `origin_tick` semantic:
- own-empire = `projection.dispatched_at` (= dispatcher's command-send tick、 light-coherent with player UI)
- foreign = `snapshot.observed_at` (= viewing empire learned of state)
- no-store = `state.{departed_at, started_at, 0}` (= ECS ground truth、 observer-only)

`expected_tick`: own = `expected_arrival_at`、 foreign = `None` (= snapshot は point-in-time observation のみ)、 no-store = `state.{arrival_at, completes_at}`。

### `ShipSnapshotState` 9 variants

```rust
pub enum ShipSnapshotState {
    InSystem,
    InTransitSubLight,    // SubLight 経路 (外部干渉可、 player UI で区別)
    InTransitFTL,         // FTL 経路 (外部干渉不可、 player UI で区別)
    Surveying,
    Settling,
    Refitting,
    Loitering { position: [f64; 3] },
    Destroyed,            // is_actionable=false
    Missing,              // is_actionable=false
}
```

### Post-planning projection upgrade hook (`poll_pending_routes`)

dispatcher が `MoveTo` を dispatch した瞬間、 route plan は async task に投げただけで FTL/SubLight 未確定 → `intended_state = InTransitSubLight` 保守的 placeholder で write。 数 frame 後 `poll_pending_routes` で task complete 検出時、 `PlannedRoute.segments[0]` の kind を見て:
- `RouteSegment::FTL { .. }` → `intended_state = InTransitFTL`
- `RouteSegment::SubLight { .. }` → `intended_state = InTransitSubLight` (= no-op)

これは dispatcher empire の **信念の自己更新** で、 観察 fact (`KnowledgeFact::ShipDeparted` 等) 不要。 light-delay 経過後の reconciler は `KnowledgeFact::{ShipArrived, SurveyComplete, ShipDestroyed, ShipMissing}` の 4 fact で `projected_state` を update する path を維持。

### Observer mode contract (= 既存 technical debt、 `#499` で migrate 予定)

production caller (`ui/mod.rs:956-960` 等) が observer mode で `viewing_knowledge = None` を pass → `ship_view` が realtime fallback path に入る (= 観察対象 empire の knowledge ではなく ECS ground truth)。

intended contract: empire-view = 観察対象 empire の `KnowledgeStore` 経由で light-coherent。 `#499` で 4 panel 同時 migrate (= `viewing_knowledge = ObserverMode.viewing_empire's KnowledgeStore`)。

本 round では **5 callsite (4 sub-PR + outline tree) で同 pattern を統一** (= Stage 2 で B-1 + B-2 fix)、 `#499` migrate 時の影響範囲を限定。

### Module hierarchy

```
knowledge/ship_view.rs       ← data shape (ShipView, ShipViewTiming, ship_view, realtime_state_to_snapshot)
   ↑                            ↑
ui/ship_view.rs              ← egui-adjacent (ship_view_status_label, ship_view_progress, tooltip_status_word, ShipViewProgress)
   ↑
ui/outline.rs                ← re-export shim (ShipOutlineView = ShipView, ship_outline_view = ship_view)
   ↑
[ship_panel.rs, context_menu.rs, system_panel.rs, situation_center/, ui/mod.rs]
```

将来 `#466 ThreatState`、 `#490 omniscient`、 BRP exposure、 scripting からも `knowledge::ship_view` を消費可能 (= UI 反転を防止)。

## SAVE_VERSION 遷移

`19 → 20` (PR #498 squash 内、 commit `15132ab`)。

bump 理由: `ShipSnapshotState::InTransit` を `InTransitSubLight` / `InTransitFTL` に split (= postcard positional enum tag 1 = `InTransitSubLight` / 2 = `InTransitFTL`、 既存 v19 binary は decode reject)。

fixture (`tests/fixtures/minimal_game.bin`) regen 済 (829 B 維持、 minimal game に in-flight ship 不在のため payload に新 variant byte は不在、 ただし version byte は v20)。

`tests/region_persistence.rs::save_version_strictly_rejects_previous_version` を v19 reject に更新。 真 v19 wire hand-craft は `#494` の defer 判断 (= deferred extension docstring `tests/common/wire_format.rs::build_v19_positional_misparse_bytes`)。

## Test status (最終)

- isolated full: **3205 passed / 0 failed** (= `cargo test -p macrocosmo --tests --test-threads=1`)
- workspace 全体: 3549+ passed (incl. macrocosmo-ai crate tests)
- pre-existing flake (= 並列のみ):
  - `ui::situation_center::notifications_tab::tests::apply_pending_acks_system_drains_buffer_and_acks_queue` (= global `PENDING_ACK_BUFFER` race、 isolated で pass)
  - `tests/esc_notification_pipeline::ack_affects_esc_queue_only_not_banner` (= 同種 race、 isolated で pass)
- 新規 test:
  - 41 unit tests in `knowledge::ship_view::tests` + `ui::ship_view::tests` (= helper coverage)
  - 22 + 9 = 31 panel-level FTL leak guards (Stage 2 で追加 9)
  - 4 round-trip persistence tests (`tests/ship_snapshot_persistence.rs`)
  - 4 polish regression tests (`tests/ship_projection_dispatcher_path.rs` + `_intended_render.rs` + `_polish.rs` + `region_persistence.rs`)

## 起票・整理した issues

### 新規 (本 session 起票)

| # | severity | 内容 | 状態 |
|---|---|---|---|
| #498 | refactor | ShipView helper extraction prep | merged |
| #499 | bug | observer mode bypasses empire-view contract | OPEN (= 後続) |
| #500 | bug | context_menu.rs FTL leak fix | merged |
| #501 | bug | ship_panel.rs FTL leak fix | merged |
| #502 | bug | ui/mod.rs map tooltip + camera FTL leak fix | merged |
| #503 | bug | situation_center ship_ops_tab FTL leak fix | merged |
| #504 | polish | 0.3.1 polish bundle | merged |

### 整理 (本 session 中に close)

`#491`、 `#495`、 `#493`、 `#494`、 `#496`、 `#497` (= 6 件)

合計: **新規 7 PR / 1 follow-up issue 起票、 6 issue close、 7 PR merged**

## 残 follow-up issues

### High priority

- **`#499`** observer mode contract migration — 4 panel + outline tree (= 5 callsite) で `viewing_knowledge=None` 渡しを `ObserverMode.viewing_empire`'s `KnowledgeStore` 経由に migrate。 `ObserverMode` resource の semantic 確認 + integration test 必要。 epic 規模 中

### Design retrospective (= 別 issue 起票候補)

- **D-2 `ship_view_status_label` 採用率**: 4 PR 中 PR #501 のみ実消費 (PR #500 不要、 PR #502 inline match、 PR #503 inline match)。 helper API shape が panel ニーズに不適合 → API surface 縮小 (= delete) or 別 shape (= `StatusLabelFormat` enum) 検討
- **D-3 panel 間 status word format drift**: outline `"In Transit"`/`"FTL"`、 ship_panel `"Moving to X"`、 tooltip `"Sub-light"`/`"In FTL"`、 situation_center `"sublight transit"`/`"in FTL"` の **4 通り**。 player UX で 4 表記の混在
- **`ShipViewProgress.is_overdue` UI 未表示**: PR #501 が legacy tuple 変換時に捨てる、 dead field 化。 visualize 案 (= "overdue" suffix on ProgressBar text、 outline tree red highlight) or remove

### Epic-related (= 0.4.0 候補)

- **`#466`** ThreatState Phase 2 — projection 基盤 + `is_ship_overdue` helper + ShipDestroyed/Missing facts unblock 済、 ThreatStates Component / state transitions / ROE wiring 残。 design questions 残り (transition cadence、 storage shape)
- **`#467`** Mid-Mid arbiter (FCFS + rejection 通知) — `#449/#450` 後続
- **`#490`** omniscient mode (god-view) — `ObserverMode` 3-variant enum (`Disabled / EmpireView / Omniscient`) 拡張、 `Omniscient` 時 realtime 直読 path

### Tooling / BRP

- **BRP `mutate_components` 制約**: `HashMap<Entity, ShipProjection>` への path index 不可、 enum struct variant serialization 拒否。 dev velocity 影響あり、 別 issue 起票候補
- **BRP `eval_lua` global gs 不在**: event-callback scope のみ、 runtime 探査不可
- **`UiElementRegistry` カバレッジ**: top-bar buttons のみ、 ship right-click / context-menu / save 等 absent

### Cleanup

- worktree / branch cleanup は本 session で完遂 (= 12 worktree → 0、 26 branch → 2)、 disk 88% → 15%

## Tooling friction (本 session で観察)

- **5 並列 worktree cargo build で disk pressure 100%** → idle worktree の `target/` 削除で 79 GB 解放、 sub-agent が「I cannot proceed without disk」 で waiting → SendMessage で resume
- **GitHub squash merge の commit message が title-only** → PR body の `Closes #X` footer が auto-close を発火しない → main session で手動 `gh issue close` 必要
- **`gh pr merge --squash --delete-branch` が local worktree lock で fail** (= remote merge は成功、 local cleanup 失敗) → main session で worktree unlock + remove + branch -D の 3 step cleanup 必要
- **adversarial review wave の diminishing return** = 同 PR に 3+ wave かけると findings が design retrospective level に偏る、 BLOCKER は 1-2 wave で出尽くす傾向

## Code cleanup 残り

なし (本 session で意識的に完遂):
- `_is_observer` 引数除去
- `ShipViewTiming` 3 source ladder reinvent 解消
- `system_name` 重複 (3 → 1) 統合
- `ShipStatusInfo` struct surface 撤去 (tuple return)
- `ProjectionWriteParams` 重複 query 統合
- realtime_state_to_snapshot writer drift 統合

## 次セッション最優先

### Top: `#499` observer mode contract migration

5 callsite (`ui/mod.rs:956-960` outline tree + 4 panel) で `viewing_knowledge=None` 渡しを `ObserverMode.viewing_empire's KnowledgeStore` 経由に統一 migrate。 `ObserverMode` resource の field 確認 + 観察対象 empire の knowledge を resolve する helper (= `resolve_observer_knowledge(world)` 等) を追加、 5 callsite に展開。 規模 中、 1 PR で完結。 player UX で「observer mode で観察対象 empire の **light-coherent** view」 が完成。

### 次: Design retrospective 起票

D-2 / D-3 / `is_overdue` を 1-3 issue として起票、 polish backlog として trace。 着手は急がない (= player-blocking ではない)。

### 中位

- **`#466` ThreatState Phase 2** — projection 基盤完成で unblock 済、 design 確認後 epic-level work 開始候補。 Round 13 で `is_ship_overdue` helper land、 #472 ShipDestroyed/Missing facts land、 残 ThreatStates Component / state transitions / ROE wiring
- **`#490`** omniscient mode (god-view) — `ObserverMode` 3-variant enum 拡張
- **`#467`** Mid-Mid arbiter — `#449/#450` 後続、 design phase

### Skip 候補 (= 0.4.0)

- BRP tooling 改善 (= dev velocity 系、 game contract 影響なし)

## 次セッション再開プロンプト例

```
2026-04-29 ハンドオフ参照。
docs/session-handoff-2026-04-29-round-14-light-coherence-completion.md
を読んで全体像把握。 Round 14 = #491 epic close + 0.3.1 polish bundle。
SAVE_VERSION 20、 全 panel が ShipView projection 経由で light-coherent、
helper module split (knowledge::ship_view + ui::ship_view) で
hierarchy 反転防止、 post-planning projection upgrade hook で
intended_state を SubLight → SubLight/FTL に正しく upgrade。

優先度:
1. #499 observer mode contract migration (= 4 panel + outline tree
   で viewing_knowledge=None 渡しを ObserverMode.viewing_empire 経由に)
2. design retrospective 起票 (D-2 status_label 採用率、
   D-3 status word format drift、 is_overdue UI 未表示)
3. #466 ThreatState Phase 2 (projection 基盤完成で unblock)
4. #490 omniscient mode
```

## 重要な caveat

- **post-planning hook**: `poll_pending_routes` で projection upgrade、 `KnowledgeFact::ShipDeparted` 不要 (= dispatcher empire の信念の自己更新)
- **observer mode**: 現状 `viewing_knowledge=None` realtime fallback、 `#499` で empire-view contract に migrate 予定
- **`ShipViewTiming.expected_tick = None` for foreign**: snapshot は point-in-time observation のみで ETA 不在、 progress bar が foreign ship で出ない仕様 (= 後続 intel-channel design で延長検討)
- **`ShipSnapshotState` SAVE 19 → 20**: postcard positional enum tag shift、 v19 binary は decode reject (= 既存 fixture regen 済)
- **`SEED_DISPATCHED_AT_SENTINEL = i64::MIN`**: reconciler gate `fact_observed_at >= dispatched_at` で trivially true、 post-reconcile で natural promotion
- **dead helpers** = `ship_view_status_label` 等は PR-491 prep で land、 PR #2..#6 で実消費期待だったが採用率低 (= 1/4 PR)、 `#NEW` で retrospective 検討
