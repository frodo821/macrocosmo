# Implementation Record: Issue #173 — NPC 帝国 player mode spawn + pluggable mock AI

**Status**: 実装完了 (PR #319 merged 2026-04-15)
**Original plan date**: 2026-04-14

## 実装結果

### 変更内容

1. **player mode で NPC 帝国を spawn**
   - `src/setup/mod.rs` の `.run_if(in_observer_mode)` 削除 (+ ordering 追加 `.after(run_faction_on_game_start)`)
   - 既存 `existing_by_id` filter + passive skip が double-spawn を構造的に防ぐので、1 行削除で済んだ (設計通り Plan A 採用、B/C 案は却下)
   - `scripts/factions/init.lua` に NPC empire 2 つ追加 (`vesk_hegemony`, `aurelian_concord`)、`on_game_start` callback は持たせず capital 探索競合を回避

2. **Pluggable mock AI hook**
   - `src/ai/npc_decision.rs` 新規: `trait NpcPolicy { fn tick(...) -> Option<Command> }` + `NoOpPolicy` default
   - `npc_decision_tick` system を `AiTickSet::Reason` に登録、NPC を iterate (no-op emit)
   - 本番 feature gate なし、hand-written no-op を登録
   - `macrocosmo-ai::mock` feature は test binary 内 dev-dep からのみ使用

3. **Initial relations seed**
   - `seed_npc_relations` system を `src/faction/mod.rs` に追加 (`.after(run_all_factions_on_game_start)`)
   - NPC ↔ passive hostile = Neutral/-100、NPC ↔ NPC = Neutral/0

4. **Regression test**
   - `tests/npc_empires_in_player_mode.rs` 新規: ObserverMode=false で (1) NPC Empire 複数存在、(2) 100 tick idle panic なし、(3) diplomatic_action target 解決、(4) HostileFactions relation 健全

### Critical Files (実装に触れた箇所)

- `macrocosmo/src/setup/mod.rs`
- `macrocosmo/scripts/factions/init.lua`
- `macrocosmo/src/ai/npc_decision.rs` (新規)
- `macrocosmo/src/ai/plugin.rs`, `src/ai/mod.rs`
- `macrocosmo/src/faction/mod.rs` (seed_npc_relations)
- `macrocosmo/Cargo.toml` ([dev-dependencies] に `macrocosmo-ai = { features = ["mock", "playthrough"] }`)
- `macrocosmo/tests/npc_empires_in_player_mode.rs` (新規)

### 計画との差分

- ほぼ Plan 通り。Plan A (1 行削除) の読み通り、defensive filter で double-spawn 回避が効いた
- `seed_npc_relations` は Plan Commit B 内で着地、別 system に切り出しが綺麗に収まった

## 関連 / Follow-up

- **#163 Faction epic**: 本 PR で #173 が closable となり、#174 を #292 (Sovereignty Phase 2) へ移籍した上で #163 は実質 closable 状態
- **実 AI は #189 配下で別起票予定**: `macrocosmo-ai::campaign/nash/feasibility` を `NpcPolicy` 後段に wire、3 階層計画 / intent-based command / per-empire policy 切替等
- 現状 NPC は `NoOpPolicy` なので「存在するが何もしない」状態、#189 実装で順次アクティブ化
