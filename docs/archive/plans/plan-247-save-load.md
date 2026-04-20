# Implementation Record: Issue #247 — save/load 基盤 (postcard)

**Status**: 実装完了 (PR #317 merged 2026-04-15)
**Original plan date**: 2026-04-14

## 実装結果

Plan 調査で判明した通り、`src/persistence/` は既に 5,914 LOC で 62 components + round-trip/faction/KnowledgeStore/rng tests を実装済だった。本 PR は **narrow gap fill** として acceptance の隙間を埋める refine に徹した (Option A 採用、Option B 完全再実装は却下)。

### 変更内容

1. **Docs 整合** (Commit A)
   - `src/persistence/{mod,save,load}.rs:冒頭` の古い "Phase B/C に deferred" 記述を実態に合わせて更新

2. **命名揃い + 1 本追加** (Commit B)
   - issue 命名 `test_save_load_preserves_pending_commands` を新規追加 (PendingCommand/PendingShipCommand/PendingDiplomaticAction 3 カテゴリ一括検証)
   - 既存 `preserves_pending_colony_command` / `preserves_pending_facts` は粒度違いで残置

3. **Deterministic continuation 強化** (Commit C)
   - 既存 `deterministic_continuation` test (手で `clock.elapsed += 100` するのみ) を Schedule 駆動で書き直し。16 tick Schedule 走らせ live app と loaded app の GameClock + GameRng draw sequence 一致を実測

4. **Fixture helper + committed binary** (Commit D)
   - `tests/common/fixture.rs` 新規 (`load_fixture(path) -> App`)
   - `tests/fixtures/minimal_game.bin` (732 B、postcard encoded) を commit
   - `tests/fixtures_smoke.rs` 新規: `load_minimal_game_fixture_smoke` (format stability guard) + `regenerate_minimal_game_fixture` `#[ignore]` test

5. **#295 interaction test** (Commit E)
   - `Sovereignty.owner` が #295 で Core ship `FactionOwner` 由来 derived cache になったため、save に含めた後 load 直後 `update_sovereignty` を走らせて Core ship presence と整合することを assert

6. **運用 docs** (Commit F)
   - `CLAUDE.md` の Save-file Fixtures section: regeneration 手順 (`cargo test -p macrocosmo --test fixtures_smoke regenerate_minimal_game_fixture -- --ignored`) を明記

### 合計

+490 行 rust / -70 / +40 行 docs / +732 B binary

### 計画との差分

- Plan 通り 6 commits、各 bisect-compilable
- bincode → postcard migration は **不要** (Cargo.toml を grep で確認、既に postcard v1 使用中)。前回 stuck agent が「bincode 採用」の issue 本文を真に受けて migration 前提で探索しようとしていたのが原因と推定

### Critical Files

- `macrocosmo/tests/save_load.rs`
- `macrocosmo/tests/common/fixture.rs` (新規)
- `macrocosmo/tests/fixtures/minimal_game.bin` (新規 binary)
- `macrocosmo/tests/fixtures_smoke.rs` (新規)
- `macrocosmo/src/persistence/{mod,save,load}.rs` (doc 更新のみ)
- `CLAUDE.md`

## 関連 / Follow-up

- `docs/architecture-decisions.md` §9 に contract 集約
- まだ未着手の follow-up (#247 本文には含まない):
  - Schedule 100-tick parity benchmark (live vs loaded で完全 schedule 差分ゼロ検証)
  - SaveId 独立採番 (live Entity bits 流用を counter 分離)
  - Save file migration framework (SAVE_VERSION bump 時の v1→v2 adapter、現状 hard error)
  - SCRIPTS_VERSION auto-hash (string 手管理を Lua bundle hash で auto-bump)
