# Implementation Record: Issue #295 — S-1 Sovereignty derived-view

**Status**: 実装完了 (PR #315 merged 2026-04-15)
**Original plan date**: 2026-04-14

## 実装結果

`Sovereignty.owner` の player_empire ハードコードを Core ship `FactionOwner` 引きに置換、`system_owner` helper を追加。Core ship (S-3 #296) は未実装のため、**現時点で `Sovereignty.owner` は常に `None`** (issue 仕様通り、S-3 land で有効化)。

### 変更内容

1. **`faction::system_owner` helper 追加** (Commit 1)
   - `macrocosmo/src/faction/mod.rs` 末尾に +35 行
   - Signature: `pub fn system_owner(system: Entity, at_system: &Query<(&AtSystem, &FactionOwner), With<CoreShip>>) -> Option<Entity>`
   - `CoreShip` marker は S-3 未着のため一時的に `With<AtSystem>` + TODO コメント `// TODO(#296): filter by CoreShip marker when S-3 lands`
   - unit test 1 本追加

2. **`update_sovereignty` を derived view に改修** (Commit 2)
   - `src/colony/authority.rs:121-145` 書き換え
   - colony 人口由来ロジック撤廃、`system_owner` 呼び出しで owner を決定
   - `player_empire` ハードコード 3 箇所 (L124, L127, L138) 削除
   - `control_score` は Core 存在時 1.0 / 不在時 0.0 (暫定、TODO: S-4 #298 で再設計)
   - `tests/ship.rs:469-470` assertion を `None` 期待に修正 + TODO コメント

3. **Regression test** (Commit 3)
   - `macrocosmo/tests/sovereignty.rs` 新設 +80 行:
     - (1) Core ship 不在時 `system_owner` returns None
     - (2) Core ship spawn (mock: `AtSystem + FactionOwner`) 時 faction entity 返す
     - (3) multi-faction 共存は S-2 (#297) 依存で TODO コメント skip
     - (4) `update_sovereignty` 回帰: colony あっても Core 無ければ `Sovereignty.owner == None`
   - `tests/common/mod.rs` に `spawn_mock_core_ship(world, system, faction)` helper 追加

### 計画との差分

- Plan 通り 3 commits、+150/-30 lines
- `Sovereignty.owner` field は **削除せず derived cache として残す** 方針を維持 (savebag + 10+ テスト初期化が依存、#247 との conflict を避ける段階的移行)

### Critical Files

- `macrocosmo/src/colony/authority.rs`
- `macrocosmo/src/faction/mod.rs`
- `macrocosmo/src/galaxy/mod.rs`
- `macrocosmo/tests/ship.rs`
- `macrocosmo/tests/common/mod.rs`
- `macrocosmo/tests/sovereignty.rs` (新規)

## 関連 / Follow-up (Sovereignty Phase 2 epic #292)

- **#297 (S-2)** open: FactionOwner 統一付与 (Colony/SystemBuildings/DeepSpaceStructure/Ship)
- **#296 (S-3)** open: Infrastructure Core Deliverable + spawn-as-immobile-Ship lifecycle (これが land すると `CoreShip` marker が揃い、本 issue 実装が実際に機能し始める)
- **#298 (S-4)** open: Conquered state mechanic — `control_score` の意味論再定義
- `Sovereignty.owner` field 削除: 段階的移行、本 issue では cache 維持

## 計画スコープ外 (別 issue)

- Core ship mechanic 本体 (#296 S-3)
- FactionOwner cascade (#297 S-2)
- `src/colony/building_queue.rs:311` の `Owner::Empire` 構築
- `update_sovereignty` tick 最適化 (`.run_if`)
- `Sovereignty.owner` field 削除
