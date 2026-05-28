# Session 再開プロンプト — 2026-05-25 Round 16 直後

ハンドオフ参照: `docs/handoff/2026-05-25-round-16-ai-build-and-resource-gate.md`。
直前段の Round 15 は `docs/handoff/2026-05-24-round-15-ai-courier-delay.md`。

## 直前セッション (Round 16) サマリ

- 期間: `3e4af27 → cd08a7d` (main 上 7 PRs / 15 commits、 全 squash merge)
- 4 issue close + 1 epic 起票:
  - **#470** AI build → host colony BuildQueue routing (= 「AI が船を建造しない」 残 root fix) — PR #510
  - **#445** shipyard parallel slots + fractional speed accumulator (capability→modifier migration、 7 callsite) — PR #511
  - **#490** Omniscient (god-view) observer mode 3-variant enum + `NonOmniscientKind` newtype — PR #512
  - **#444** region deadlock 4-fix bundle (Mid Rule 3.5 deploy_deliverable + survey region-scope filter 撤去 + eager macro decomposition + `infra_core` typo fix) — PR #528
  - 2 hotfix series:
    - **PR #530** = `#490` fold-in regression: F9 default binding + dispatch extrapolation skip (= AI ship が map で動かないバグ)
    - **PR #531** = brp QA で発見した starvation cascade: Rule 6 fleet_composition fix (= surveying ship を census 包含) + soft resource gate (Rules 3.5 / 5a / 5b / 6) + planet-building dedup + **#529 A migration (pending-aware resource gate + region-scope filter + per_region inputs)**
- **新規 epic 起票**: **#529** epic(ai) — migrate all AI judgement paths to projection-based (= light-coherent AI)。 A (Resource gate) は PR #531 で land 済、 B/C/D が残
- **新規 followup 起票**: #513-#527 計 15 件 (大半は #445 / #470 / #490 / #528 / #531 由来の小型 backlog ticket)
- `SAVE_VERSION = 20` 維持 (全 changes runtime-only / postcard additive)
- 最終 test: `cargo test --workspace --tests -- --test-threads=1` → **3652 passed / 0 failed** (Round 15 末 3579 → +73)

## 次セッション優先順 (Top → 中位)

1. **`#529` epic 続行 (B/C/D migration)** — A (resource gate) は #531 で land 済。 残:
   - **B. Idle/busy 判定 migration** — `npc_decision_tick` の `ShipState::Loitering` 直接 query を `KnowledgeStore.projections[ship].intended_state` 経由に置換。 light-delay 中の自分の ship を「busy」 と認識 → 二重命令 + idle 誤認 (= #529 Symptom 1/2) を解決。 **現実的に次の最優先**
   - **C. Survey/colonize candidate 厳密化** — 全 AI metric を `KnowledgeStore` 経由化、 「不明」 状態 system 除外
   - **D. AI metric 全般 audit** — `mid_adapter` / `short_adapter` 全 method realtime ECS → projection-based 置換
2. **`#466`** — ThreatState 機構 Phase 2 (Suspected/Confirmed + ROE)。 projection 基盤完成で unblock、 epic 規模
3. **`#467`** — Mid-Mid 競合解決 (FCFS arbiter + rejection 通知、 design phase)
4. **`#441`** — gamestate thread-local proxy 統一 (console + 全 callback、 priority:high theme:modding)
5. **`#518`** — Rule 5a frontier shipyard strategy (= `total_shipyard_slots` metric 活用、 #445 follow-up)
6. **`#525`** — ObserverMode 3-way branch を `ResolveTarget` enum に collapse (= #490 refactor)
7. **`#523`** — player UI host_colony pick に FactionOwner check を backport (split-ownership latent bug)

## Skip 候補 (0.3.x 後期 〜 0.4.0)

- 戦闘 / 静的防御 epic 系 (`#211/#213/#220/#218/#121/#120/#184/#139`)
- BRP tooling 改善
- clippy 警告整理 (`#455`)
- 1.0.0 後回し固定 (`#174` / `#143` / `#135` / `#157` / `#61`)

## Round 16 で確立した契約 (覚え書き)

- **`pick_host_colony` (= #470)**: AI build dispatch は `(Entity, &Colony, &FactionOwner, &mut BuildQueue)` query で empire-matching colony を pick。 player UI より stricter (`FactionOwner` 直接 check、 split-ownership 対応)。
- **`shipyard_build_parallel_slots` / `shipyard_build_speed` (= #445)**: capability binary 評価から modifier path に migrate。 `tick_build_queue` は N orders parallel + per-slot funding gate + 分数 accumulator (`ShipyardSpeedAccumulators` Resource、 runtime-only)。 AI metric `systems_with_shipyard` (set-count) と `total_shipyard_slots` (sum) を distinct keep。
- **`ObserverModeKind` 3-variant (= #490)**: `Disabled` / `EmpireView` (= #499 light-coherent) / `Omniscient` (= god view、 全 KnowledgeStore 無視)。 `NonOmniscientKind` newtype が restore-loop を型レベル防止。 `is_empire_view()` / `is_omniscient()` / `is_any_observer()` で specific predicate 化、 旧 `enabled()` 削除。
- **Eager macro decomposition (= #528)**: `deploy_deliverable` を `dispatch_ai_pending_commands` で `build_deliverable → load_deliverable → reposition → unload_deliverable` に展開、 depth=4 cap + skip-list (`colonize_system` 除外)。
- **Map 外挿契約 (= #530)**: `intended_state.is_none()` で projection write skip (= spatial-less command が先行 extrapolation を clobber しない)。 `command_kind_to_intended_state` 11-kind 全 mapping audit pin。
- **Soft resource gate (= #531)**: `current_stockpile - sum(pending.cost) >= cost` で gate。 soft (deficit spending OK、 0 のみ block)、 Rule 6 priority pick → gate → 失敗で silent (= cheaper への fall-through なし)。
- **Pending-aware (= #529 A、 PR #531 commit 2)**: per-colony `BuildQueue` + per-system `SystemBuildingQueue` の `(cost - invested)` を `member_systems_set` で gate して subtract。 `Amt::sub` saturating で over-commit 時 0 clamp。 per-colony `BuildingQueue` (= mine/farm/power_plant 待ち列) は意図的に除外 (16-param ceiling)、 `handle_build_structure` の same-tick dedup が backstop。
- **`RegionShortInputs` (= #531 region-scope fold-in)**: 旧 `EmpireShortInputs` を per-region keyed に rename。 multi-MidAgent empire の overwrite 回避。 fleet census (Rule 6) は意図的に empire-wide 維持。

## 重要な caveat (再掲)

- `SAVE_VERSION` bump 必要なし — 全 Round 16 changes が runtime-only or postcard additive
- `ShipyardSpeedAccumulators` は save に乗らない — mid-tick の分数 remainder は load 後リセット (pre-alpha 許容)
- `ObserverMode.kind` も save に乗らない — F9 で Omniscient のまま save → load で Disabled。 #527 で persist 予定 (priority:low)
- F9 Omniscient toggle は `in_state(GameState::InGame)` gate (main-menu / loading で inert)、 `keybindings.toml` で上書き可
- pre-existing flake: `apply_pending_acks_system_drains_buffer_and_acks_queue` / `ack_affects_esc_queue_only_not_banner` (parallel 実行時のみ、 isolated で pass)
- worktree 4 件が locked のまま残存 (`agent-a1265f`、 `agent-a2a936`、 `agent-a6041e`、 `agent-ac4109`) — 全部 merged 済、 削除候補
- merged-but-not-deleted branch 多数あり、 `codex-review-pr-530` / `codex-review-pr-531` も transient

## 開始時に最初にやること

```
1. git -C /Users/csakai/repos/macrocosmo log --oneline -5 で最新コミット確認 (cd08a7d が HEAD のはず)
2. gh issue list --state open --limit 30 で残 issue 状況の差分把握、 特に #529 と #513-#527 backlog
3. ハンドオフ doc を Read (docs/handoff/2026-05-25-round-16-ai-build-and-resource-gate.md)
4. ユーザーの指示待ち、 or 最優先 #529 B (idle/busy 判定 migration) から着手
```
