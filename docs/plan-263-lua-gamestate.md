# Implementation Record: Issue #263 — Lua event callback に live gamestate を渡す

**Status**: 実装完了 (PR #294 merged、**設計 pivot あり**)
**Original plan date**: 2026-04-14

> **⚠ 2026-04-15 更新: 本 issue の snapshot-per-event 実装は `#332` で pivot 予定**
>
> Option B (UserData を廃止して `Lua::scope` + `create_function` のみで gamestate を構築) が mlua 0.11 で完全 safe に実現可能と判明。本 issue の snapshot-per-event は #332 に置換され、関連する #320 (`gc_collect` leak fix) と #328 (per-tick cache、close 済) は obsolete に。
>
> 詳細は `docs/architecture-decisions.md` §10 と **#332** を参照。


## 実装結果

**重要**: 当初計画は mlua 0.11 `Scope` による live World view (`GameStateHandle<'w> { world: &'w World }` + scope UserData) を想定していたが、実装中に **mlua 0.11 の scoped UserData が method 内から child UserData を返せない制約** に突き当たり、**snapshot-per-event 方式に pivot**。single callback 内で live view と観測上等価 (callback 中 world mutate しない前提)。

### 変更内容

1. **`GameStateHandle` scaffold + per-event scope wiring**
   - `macrocosmo/src/scripting/gamestate_view.rs` 新規: snapshot を build する関数群
   - `dispatch_event_handlers` / `tick_events` を exclusive system (`&mut World`) に昇格、`world.resource_scope::<ScriptEngine, _>(|world, engine| ...)` で `lua()` と `&World` 両持ち
   - `EventBus::fire` signature 拡張 `(lua, id, payload, &World)`

2. **ClockView + gamestate.clock** + Empire/System/Planet/Colony/Fleet/Ship handle
   - 全 handle は Lua table snapshot を build して返す (per-event、callback 内で再利用)
   - primitive field (name/id/energy) は都度 `world.get::<Component>(entity)` → copy
   - list field (colonies / fleets / ships_present) は一括 table
   - set field (techs / flags) は `__index(id) -> bool` (LuaJIT `ipairs` / `pairs()` 制約で pairs 未対応)
   - **resources**: empire レベル stockpile 未実装のため、全 colony `ResourceStockpile` を走査して sum を返す (意味論を docs に明示)

3. **`on_trigger` callback wiring**
   - 現状 Lua 側で書けるが Rust 側から呼ばれていなかった経路を wire
   - `lifecycle.rs` 内で `_event_definitions` scan、per-event scope で `on_trigger` 実行

4. **mtth / periodic `fire_condition` wiring**
   - `tick_events` exclusive 化、`LuaFunctionRef(i64)` を `Arc<RegistryKey>` で置き換え
   - Periodic は MTTH と対称に fire_fn None filter (build skip、後に #320 で再確認)

5. **Read-only enforcement**
   - Lua 側の mutation 試行は runtime error で拒否

### Critical Files

- `macrocosmo/src/event_system.rs`
- `macrocosmo/src/scripting/lifecycle.rs`
- `macrocosmo/src/scripting/event_api.rs`
- `macrocosmo/src/scripting/mod.rs` (plugin wiring)
- `macrocosmo/src/scripting/gamestate_view.rs` (新規、実装の中心)

### 計画との差分

- **(大) live World view → snapshot-per-event への pivot**: mlua 0.11 制約で scope approach 不可、proxy table + entity-id でのフォールバックも複雑度に見合わず、snapshot build 方式に切替
- **副作用**: build_gamestate_table が ~101 Lua refs/call を生み、tick 累積で LuaJIT aux stack 枯渇する leak が後続発覚 → **#320 で修正** (`gc_collect` 毎 tick 末)

## 関連 / Follow-up

- `docs/architecture-decisions.md` §10 に contract 集約
- **#320** ✅: mlua aux stack leak fix (本 issue 実装が原因の release blocker、PR #327 で修正)
- **#328** open: per-tick gamestate cache (build 1 回/tick + "mutation は次 tick で反映" visibility contract、性能最適化 + #320 の根本解消)
- **#289** open: β Lua View types (SystemView / ColonyView / FleetView / ShipView)、本 issue 上に型付き View を再構築

## 本 issue スコープ外 (別 issue 起票済 or 将来)

- (a) `node:perspective(viewer)` lens (#215 PerceivedInfo、KnowledgeStore 遅延ビュー) — 本 issue は god-view のみ
- (b) lifecycle / tech effect / faction hook への同 `GameStateHandle` 注入
- (c) QueryState 実行 cache (→ #328 で対応)
- (d) pending queue 以外の mutation API
