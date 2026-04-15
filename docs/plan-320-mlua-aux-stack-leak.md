# Implementation Record: Issue #320 — mlua aux stack leak fix (release blocker)

**Status**: 実装完了 (PR #327 merged 2026-04-15)
**Original plan date**: 2026-04-14

> **⚠ 2026-04-15 更新: 本 fix は `#332` で obsolete 予定**
>
> snapshot-per-event (#263) が leak 源だったが、同日に Option B (UserData 廃止の scope closure 路線) へ pivot 決定。#332 実装後は snapshot build 自体が消え、本 PR で挿入した `lua.gc_collect()` も撤回される。本 fix は release blocker 対応として妥当だったが、長期的な解は #332。
>
> 詳細は `docs/architecture-decisions.md` §10 と **#332** を参照。


## 症状と原因

- **症状**: 80 tick 以上で `evaluate_fire_conditions` が panic (`cannot create a Lua reference, out of auxiliary stack space`)
- **原因**: `build_gamestate_table` が ~101 Lua ValueRef / build を生成。`payload.set("gamestate", gs_table)` で Lua 側 table に clone が格納 → **Lua GC が回らない限り残存**。80 tick × 100 ref ≈ 8000 = `LUAI_MAXCSTACK` 限界
- **本 PR は release blocker の minimum fix**、per-tick cache は #328 で別途

## 実装結果

### Leak source 2 箇所を修正

Plan agent の実測で **issue 指摘の `evaluate_fire_conditions` に加え、`dispatch_event_handlers` も同等 leak 源** と判明。両方に対処。

1. **Commit A: GC after Lua callback batches**
   - `macrocosmo/src/scripting/lifecycle.rs`
   - `evaluate_fire_conditions` (L320 付近、resource_scope closure 末尾) に `let _ = lua.gc_collect();`
   - `dispatch_event_handlers` (L435 付近、`for fired` ループ抜け後、resource_scope closure 末尾) にも同様
   - `warn!` on Err (Lua `__gc` metamethod error を吸収、panic させない)
   - +8 行

2. **Commit B: Skip gamestate build when Periodic fire_condition is None**
   - `macrocosmo/src/scripting/lifecycle.rs:229-233`
   - Periodic 分岐も `if fire_condition.is_some() { out.push(...) }` で wrap (既存 MTTH L253-259 と対称)
   - 仕様確認: Periodic 本体発火は `event_system.rs` 側 tick、`evaluate_fire_conditions` は suppress 判定のみ、filter 安全
   - +3 行

3. **Commit C: Regression test**
   - `macrocosmo/tests/stress_lua_scheduling.rs` 新規
   - mini_world fixture + 1000 tick で `evaluate_fire_conditions` + `dispatch_event_handlers` を走らせ panic しないことを assert
   - Periodic event + pending MTTH を仕掛け
   - determinism: LuaJIT vendored ビルド固定、環境非依存
   - +80-120 行
   - **regression guard 確認**: `HEAD~2` に revert した状態で stress test が panic することを実測、#320 症状を正確に再現している

### 検討された代替案 (採用しなかった)

| 案 | 理由 |
|---|---|
| (B) per-tick cache | 性能最適化として有効だが invalidation 仕様議論が必要、release blocker のスコープ外 → **#328 で follow-up** |
| (C) `lua.scope` | API 侵襲大 (署名全書換)、#263 での実装時に既に却下判断 |
| (D) `RegistryKey::expire_registry_values()` | RegistryKey 専用で ValueRef aux stack は別経路、**本 bug には無効** |

### 計画との差分

- ほぼ Plan 通り 3 commits、`gc_collect()` は 1 回呼びで採用 (mlua comment の "2 回保険" は performance 計測で閾値超えたら `gc_step_kbytes(256)` 置換の future task)
- Plan で指摘していた「leak source 2 箇所 (issue 未言及)」が的中、両方 fix しなければ release blocker 解消しなかった

### Critical Files

- `macrocosmo/src/scripting/lifecycle.rs` (両 closure 末尾に GC 追加)
- `macrocosmo/src/scripting/gamestate_view.rs` (変更なし、実測確認用)
- `macrocosmo/src/scripting/engine.rs` (`lua()` accessor 使用)
- `macrocosmo/src/event_system.rs` (Commit B 仕様検証用)
- `macrocosmo/tests/stress_lua_scheduling.rs` (新規、Commit C)

## 関連 / Follow-up

- **#328** open: per-tick gamestate cache — 性能最適化 (build ≈5ms × 数回/tick 削減)、`Resource CachedGamestate { tick: i64, key: RegistryKey }`、visibility contract "mutation は次 tick で反映" (光速通信遅延 core mechanic と整合)
- **`gc_step` 切替 & benchmark**: Commit A の full GC が profile で閾値超えた際の置換案
- **Legacy `attach_gamestate` 署名整理**: `scope`-based への漸進移行 (本 PR では見送り)
- `docs/architecture-decisions.md` §10 に visibility contract + gc_collect の必要性を集約
