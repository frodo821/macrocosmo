# Implementation Record: Issue #293 — HostilePresence / HostileType 完全廃止

**Status**: 実装完了 (PR #308 + follow-up #311 / #312 merged 2026-04-15)
**Original plan date**: 2026-04-14

## 実装結果

計画通り 7 commit で HostilePresence struct / HostileType enum を完全削除、visibility layer を `AtSystem + FactionOwner + HostileHitpoints + Hostile` marker の組み合わせに置換。加えて `attach_hostile_faction_owners` backfill system を廃止し、**ordering flip** で `generate_galaxy.after(spawn_hostile_factions)` と直接 `FactionOwner` を付与する root-cause fix を採用 (Plan の "keep current ordering" 案より綺麗に収まった)。

### 変更内容

1. **FactionTypeDefinition 拡張** (Commit 1)
   - `strength` / `evasion` / `default_hp` / `default_max_hp` を optional f64 field として追加
   - `scripts/factions/faction_types.lua` に `space_creature` / `ancient_defense` の値を記述
   - 環境 `strength_mult` (距離スケーリング) は `generation.rs` 内で維持

2. **新 components** (Commit 2)
   - `AtSystem(Entity)` marker
   - `HostileHitpoints { hp, max_hp }` typed component
   - `Hostile` zero-sized marker (test alive-count query 用)

3. **generation.rs** (Commit 3)
   - `HostileType` match + HostilePresence spawn を `Res<FactionTypeRegistry>` + `Res<HostileFactions>` lookup に置換
   - 0.7/0.3 `space_creature`/`ancient_defense` split 維持

4. **Visibility layer migration** (Commit 4)
   - knowledge/mod.rs、visualization/stars.rs、ui/params.rs + mod.rs、ship/scout/settlement/survey/command/routing、deep_space/mod.rs を全て新 component 経由に書き換え
   - `Query<(&AtSystem, &FactionOwner, Option<&HostileHitpoints>), With<Hostile>>` + `FactionRelations.get_or_default(viewer, owner).can_attack_aggressive()` でフィルタ

5. **combat.rs migration** (Commit 5)
   - mutating query を `Query<(Entity, &AtSystem, &mut HostileHitpoints, &FactionOwner), With<Hostile>>` に
   - `tests/combat.rs` の `.hp` reads を `HostileHitpoints` reads に更新 (sed-able single-line + 15 manual multi-line)

6. **Test migration + attach_hostile_faction_owners 削除** (Commit 6)
   - `tests/common/mod.rs::setup_test_hostile_factions` 書き直し
   - 89 call-site 更新 (plan 見積 71 より多かった)
   - `src/faction/mod.rs:153-182` `attach_hostile_faction_owners` system + 2 unit tests 削除 + 登録解除

7. **Struct / enum 削除** (Commit 7)
   - `src/galaxy/mod.rs`: `HostilePresence` + `HostileType` 削除
   - `src/persistence/{savebag,save,mod}.rs`: `SavedHostilePresence` + `SavedHostileType` + bag field 削除 (persistence OR-tuple 15-filter 制限内)

### 計画との差分

- **Ordering flip** (計画では "keep current ordering + rename backfill"): 実装では `spawn_hostile_factions.after(spawn_player_empire)` + `generate_galaxy.after(spawn_hostile_factions)` に flip し、`attach_hostile_faction_owners` backfill を完全削除。plan 中では "risk 中" 扱いしていた ordering 変更だが、root-cause fix としてより綺麗
- **call-site 数**: plan 71 見積 → 実測 89 (tests/combat.rs の 69 + 他 20)

### Semantic merge conflict (2 件発覚、regression patch で対応)

1. **#309 (PR #311)**: combat test で **raw hostile spawn 20 箇所が `FactionOwner` 欠落** → combat gate すり抜けで 10 combat test fail。auto-merge OK だったが runtime で発覚。`cargo test --workspace` を merge 後に必ず走らせる教訓
2. **#310 (PR #312)**: `Res<HostileFactions>` 必須化で既存 19 tests (FactionRelationsPlugin なし) が panic → `generate_galaxy` を `Option<Res<HostileFactions>>` で graceful degrade

これらは `memory/feedback_semantic_merge_conflict.md` に規範化。

### Critical Files

- `macrocosmo/src/galaxy/mod.rs`
- `macrocosmo/src/galaxy/generation.rs`
- `macrocosmo/src/ship/combat.rs`
- `macrocosmo/src/scripting/faction_api.rs`
- `macrocosmo/tests/common/mod.rs`
- `macrocosmo/src/faction/mod.rs` (backfill 削除 + ordering)

## 関連 / Follow-up

- `docs/architecture-decisions.md` §13 (galaxy gen ordering) に contract 集約
- **#289** open: β Lua View types で faction / hostile 表現の型付きビュー構築
