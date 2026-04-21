# 実装計画書: Epic #349 — ScriptableKnowledge (Lua-extensible knowledge kinds with `<id>@recorded` / `<id>@observed` lifecycle events)

_Prepared 2026-04-15 by Plan agent. Sub-issues #350 (K-1) / #351 (K-2) / #352 (K-3) / #353 (K-4) / #354 (K-5). Depends on #332 Phase A + B (landed PR #337 / #338) for `gamestate_scope.rs` + `GamestateMode::ReadWrite`. Depends on #334 Phase 4 (landed PR #348) for the `apply::request_command` template (write helper が `&Lua` 不使用、Bevy Messages queue 経由で downstream dispatch) が規範となる。K-6 は #345 に吸収済 (2026-04-15 close)。_

---

## 0. TL;DR

`KnowledgeFact` (Rust hardcoded enum、9 variant) を **Lua-extensible な knowledge kind registry** (`KindRegistry`) に一般化する。light-speed propagation pipeline (`PendingFactQueue` / `arrives_at` / `RelayNetwork` — `macrocosmo/src/knowledge/facts.rs:377-588`) はそのまま流用。kind 定義の副作用で **per-kind lifecycle event (`<id>@recorded` / `<id>@observed`) を自動 register**、Lua は `on(event_id, fn)` で subscribe する。

- **`define_knowledge { id, payload_schema }`** (K-1) で kind を宣言、KindRegistry に登録、lifecycle event 2 本を self-register
- **`gs:record_knowledge { kind, origin_system, payload }`** (K-2) で `<id>@recorded` 同期発火、payload は subscriber chain で mutable、最終形を `PendingFactQueue` へ enqueue (light-speed + Relay 短縮)
- **`on("<id>@<lifecycle>", fn)` / `on("*@<lifecycle>", fn)`** (K-3) で subscription、新 knowledge 専用 subscription registry、subscriber error は warn + chain 続行、suffix wildcard のみ v1 対応
- **`PendingFactQueue` drain 拡張** (K-4) で `<id>@observed` を per-observer empire per-payload-copy で発火、`lag_hexadies` / `observed_at` / `observer_empire` は sealed metadata
- **既存 `KnowledgeFact` variants に `core:*` namespace 付与** (K-5) で 9 variant を `core:hostile_detected` 等に mapping、KindRegistry に built-in 登録、Lua の `core:` 上書きは load time error

**v1 の non-goal**: subscription cancellation / priority / prefix wildcard / `@expired` / NPC observer expose / knowledge persistence (save/load 対応) / `define_event` 統合。

**既存 `notify_from_knowledge_facts` の扱い**: K-5 land 時に fact-driven banner 発火は *残す*。#345 bridge (K-6 吸収済) が Lua wildcard subscriber で一般化通知する別 path を足すのが次段階。従って K-5 の段階では *並存*、#345 land 時に legacy path の削除判断。

**Migration 方針: 増設 + 並存** — 既存 `KnowledgeFact` / `PendingFactQueue` API は書き換えず、新 variant `KnowledgeFact::Scripted { ... }` を追加する。旧 variant の書き方 (`FactSysParam::record`) もそのまま残す。K-5 で Rust 側 variant に kind id をマップする際も enum 表現自体は保持。

---

## 0.5 user 確定事項 (2026-04-16、§9 open questions の override)

§9 の plan agent 推奨に対する user 判断。impl はこの確定事項を先に参照、§9 は traceability のため残置。

### 9.1 `@recorded` 同期性 — **同 tick 発火に変更** (queue 化はする)
- plan agent は「Rust-origin = 次 tick latency」を推奨したが、Bevy の system ordering で writer→reader を直列化 (`reader.after(writer)`) すれば同 tick 内で chain 完結可能 (#334 dispatcher pattern と同形)。
- **実装**: Rust system が record する場合 `_pending_knowledge_records` 的な internal queue に積む → 同 tick 内の `dispatch_knowledge_recorded` system (`.after(emitter).before(push_to_pending_fact_queue)` で chain) が ScriptEngine exclusive access で drain → `@recorded` chain dispatch → 最終 payload を PendingFactQueue に push。next-tick latency 不要。
- Lua-origin (`gs:record_knowledge`) は scope closure 内で sync chain 可 (ScriptEngine 既 borrowed)。
- `feedback_rust_no_lua_callback` 違反は **同 tick でも回避可能** — emitter system は queue 化のみ、Lua dispatch は別 system (ScriptEngine exclusive) で行う。

### 9.2 event id parser hygiene — **error** (確定)
- `define_knowledge { id = "foo@bar" }` も `on("foo@bar@recorded", fn)` も load 時 error。

### 9.3 deep-copy depth_limit — **configurable**
- `pub const KNOWLEDGE_PAYLOAD_DEPTH_LIMIT: usize = 16` を default に、将来 setting / env で上書き可能な struct (`KnowledgePayloadConfig`) で wrap。v1 は constant 利用、`Resource` 化は v2 以降。
- 超過は error (`mlua::Error::RuntimeError`)。

### 9.4 subscription registry — **bucketing 化を v1 に前倒し**
- plan agent は v1 は full scan、v2 で bucketing と提案したが、subscriber が大量 (modder content 増加) になると O(N) per dispatch がボトルネックになるため最初から bucket 化。
- **実装**:
  - `HashMap<EventId, Vec<RegistryKey<Function>>>` (exact 一致用)
  - `HashMap<Lifecycle, Vec<RegistryKey<Function>>>` (`*@<lifecycle>` wildcard 用)
  - dispatch = exact bucket lookup + wildcard bucket lookup、O(1) lookup + O(K) iterate (K = bucket size)
  - 登録 (`on(...)`) 時に bucket に push、unregister は v1 範囲外 (`v1 で含めない`)

### 9.5 drain 集約 — **確定**
- K-5 で `notify_from_knowledge_facts` の drain を `dispatch_knowledge_observed` に統合。

### 9.6 namespace `<ns>:<name>` 強制 — **warn only** (確定)
- v1 は緩く、`define_knowledge { id = "foo" }` (namespace なし) は warn のみ。
- `core:` 上書き (`define_knowledge { id = "core:foo" }`) は **常に error** (Rust namespace 保護)。

### 9.7 `priority` field — **drop** (確定)
- `define_knowledge` の option table は `id` + `payload_schema` のみ。
- priority 概念は knowledge と意味的に合わない (modder が必要なら Lua subscriber 内で自前 priority logic を組む)。
- 関連 #345 でも `define_knowledge` 経由の priority 受領なし、別 channel (`push_notification` 引数) で渡す。

### §10 spike — 全 5 件採用 (確定)
- 10.1 (`seal_immutable_keys` metatable) — K-3 commit 1 の foundation で先行 land
- 10.2 (event id parser edge cases) — K-1 load-time validation の網羅
- 10.3 (deep-copy `Function` / `UserData` 拒否) — K-2 deep-copy invariant
- 10.4 (`create_function_mut` 内 `dispatch_knowledge` reentrancy) — K-2 critical pre-impl
- 10.5 (notification regression on K-5 drain shift) — K-5 commit 着手前に regression test 棚卸

---

## §1 現状棚卸し

### 1.1 `KnowledgeFact` variants (全列挙)

Source: `macrocosmo/src/knowledge/facts.rs:195-263`.

| # | variant | 主要 fields | 発火元 (grep) |
|---|---|---|---|
| 1 | `HostileDetected` | `event_id`, `target`, `detector`, `target_pos`, `description` | `ship/pursuit.rs` (contact detection) |
| 2 | `CombatOutcome` | `event_id`, `system`, `victor: CombatVictor`, `detail` | `ship/combat*.rs` |
| 3 | `SurveyComplete` | `event_id`, `system`, `system_name`, `detail` | `ship/survey.rs` |
| 4 | `AnomalyDiscovered` | `event_id`, `system`, `anomaly_id`, `detail` | anomaly discovery paths |
| 5 | `SurveyDiscovery` | `event_id`, `system`, `detail` | legacy discovery path |
| 6 | `StructureBuilt` | `event_id`, `system: Option<Entity>`, `kind`, `name`, `destroyed`, `detail` | `deep_space/*`, demolition |
| 7 | `ColonyEstablished` | `event_id`, `system`, `planet`, `name`, `detail` | `colony/settlement.rs` |
| 8 | `ColonyFailed` | `event_id`, `system`, `name`, `reason` | colony failure paths |
| 9 | `ShipArrived` | `event_id`, `system: Option<Entity>`, `name`, `detail` | `ship/mod.rs` |

`CombatVictor` (`facts.rs:174-181`) は `Player | Hostile` の 2 値 enum。

補助メソッド (`facts.rs:265-349`):
- `title() -> &'static str` / `description() -> String` / `priority() -> NotificationPriority` / `related_system() -> Option<Entity>` / `event_id() -> Option<EventId>`

### 1.2 Propagation flow (origin → observer arrival)

```
[origin emit site]
  record_fact_or_local(fact, origin_pos, observed_at, player_aboard, player_pos, …)
  facts.rs:605-651
  │
  ├─ if player_aboard || origin_pos == player_pos  → local banner 直接 push (instant)
  │    NotifiedEventIds::try_notify で dedupe
  │
  └─ else  → compute_fact_arrival(observed_at, origin, player, relays, comms)  (facts.rs:541-588)
        │                               ── Direct or Relay path 判定
        ▼
     PendingFactQueue::record(PerceivedFact { fact, observed_at, arrives_at, source, … })
     facts.rs:385-388

[per-tick drain]
  notify_from_knowledge_facts  (notifications.rs:290-317)
    clock.elapsed >= arrives_at なら drain_ready() で取り出し
    NotifiedEventIds::try_notify で dedupe
    NotificationQueue::push + GameSpeed::pause (High priority 時)
```

関連 step の file:line:

| step | 実装 | 場所 |
|---|---|---|
| origin 書き込み canonical entry | `FactSysParam::record` | `facts.rs:717-741` |
| local path (instant banner) | `record_fact_or_local` body の `is_local` branch | `facts.rs:621-638` |
| arrival 時刻計算 | `compute_fact_arrival` | `facts.rs:541-588` |
| relay 短縮距離計算 | `nearest_covering_relay` | `facts.rs:502-521` |
| relay lag | `relay_delay_hexadies` | `facts.rs:488-497` |
| relay 建築からの snapshot | `rebuild_relay_network` | `facts.rs:437-471` |
| queue 定義 | `PendingFactQueue` | `facts.rs:377-408` |
| drain | `PendingFactQueue::drain_ready` | `facts.rs:391-402` |
| drain → banner 発火 | `notify_from_knowledge_facts` | `notifications.rs:290-317` |
| 重複抑止 | `NotifiedEventIds` + `sweep_notified_event_ids` | `facts.rs:107-167` |

**重要**: 現状の propagation は *player empire 1 観測者のみ* を想定している (`vantage: &PlayerVantage`、`player_pos` / `player_aboard` だけを取る)。K-4 では **複数 empire observer** (現状は player + 将来 NPC) に拡張する必要がある。v1 は player 1 人で OK と issue で明記 (`#353 observer の決定`)。

### 1.3 `KnowledgeStore` (snapshot side、別系統)

`knowledge/mod.rs:206-281` に定義。`SystemKnowledge` / `SystemSnapshot` / `ColonySnapshot` / `ShipSnapshot` を保持。`propagate_knowledge` (`mod.rs:519-726`) が tick 毎に star system / ship をスキャンして light-speed 遅延込みで書き込む。

ScriptableKnowledge は **`KnowledgeStore` を触らない** (snapshot 側は既存のまま、fact 側だけを拡張する)。これは epic 設計サマリの通り。

### 1.4 `notify_from_knowledge_facts` の用途

`notifications.rs:290-317` — `PendingFactQueue::drain_ready` で到着 fact を取り出し、`NotifiedEventIds::try_notify` で dedupe し、`NotificationQueue::push` する。High priority 時に `GameSpeed::pause`。

対比で `auto_notify_from_events` (`notifications.rs:255-281`) は `GameEvent` を whitelisted 変種だけ banner 化する (`is_legacy_whitelisted` で `PlayerRespawn` と `ResourceAlert` のみ)。残りの world event は fact pipeline 経由。

**K-5 land 時の disable 範囲**: `notify_from_knowledge_facts` は *維持*。K-5 は「Rust 既存 fact に `core:*` namespace + lifecycle event 発火 wire」のみを追加、notification pipeline は二重発火回避のため `NotifiedEventIds` に依存する。Lua subscriber 経由の banner 発火 (`show_notification` 経由等) は **#345 で着手**、このタイミングで `notify_from_knowledge_facts` を deprecation 判断する。今の plan のスコープ外。

### 1.5 既存 `define_event` / `on(...)` の Lua surface

`register_define_fn(lua, "event", "_event_definitions")` (`scripting/globals.rs:101`) — 他の `define_xxx` と同じ accumulator pattern。

`on(event_id, [filter,] handler)` (`scripting/globals.rs:187-242`) — `_event_handlers` global table に `{ event_id, [filter,] func }` を push。`EventBus::fire` (`event_system.rs:392-458`) と `dispatch_bus_handlers` (`scripting/lifecycle.rs:496-544`) が event_id 完全一致で dispatch、filter で絞り込み。

`_event_handlers` の登録順 = dispatch 順 (`event_system.rs:416`: `for i in 1..=len`)。filter は `HashMap<String, String>` shape のみ (structural match)。

`fire_event(event_id, target?)` (`scripting/globals.rs:535-545`) — `_pending_script_events` global table に push、`drain_script_events` (`scripting/lifecycle.rs:124-160`) が tick 毎に `EventSystem::fire_event` へ転送。**queue-only invariant** で reentrancy を避ける。

Lua 側の使用例 (epic 設計サマリ記載):
```lua
define_event {
  id = "harvest_ended",
  name = "End of Harvest",
  trigger = periodic_trigger { years = 1 },
  on_trigger = function(event) end,
}
on("harvest_ended", function(e) end)
```

**knowledge は `define_event` と別 namespace**: epic 設計で決定 (memory:project_scriptable_knowledge.md `subscription registry` セクション)。理由:
- `define_event` は *trigger* (manual / mtth / periodic) を持つが、knowledge の lifecycle event は「knowledge record 時 / 観測到着時」に自動発火される、trigger なし
- `define_event` は 1 event に対し 1 definition、knowledge は 1 kind につき 2 lifecycle event (`@recorded` + `@observed`) が自動生成される
- `on()` は両者共通で使うが、registry は分離して knowledge dispatch を高速化・簡潔化する

### 1.6 `gamestate_scope` の setter 追加 pattern

#332 Phase A / #334 Phase 4 で確立。`macrocosmo/src/scripting/gamestate_scope.rs` の構造:

| 層 | 役割 | 場所 |
|---|---|---|
| `dispatch_with_gamestate` | `&mut World` を borrow して scope closure を起こす外側 wrapper | `gamestate_scope.rs:71-325` |
| `build_gs_table` | `gs` Lua table に read/write closure を attach | `gamestate_scope.rs:94-325` (scope 内) |
| `create_function_mut` + `RefCell<&mut World>::try_borrow_mut` | 全 closure 共通、`map_reentrancy_err` で reentrancy を RuntimeError 化 | `gamestate_scope.rs:101-321` |
| `apply::*` (pub(crate) mod) | `&mut World` と parsed args のみ受けて世界を書き換える pure Rust 関数。**`&Lua` 不使用** | `gamestate_scope.rs:997-1700` (request_command は L1500-) |

`gs:request_command(kind, args)` (`gamestate_scope.rs:316-321`) が特に本 epic の template. L311-314 の comment が全てを語る:

> Invariant (plan §9.2 / feedback_rust_no_lua_callback.md): the apply helper takes no `&Lua` and never invokes Lua code. Message emit is a pure Rust event-bus push; the downstream handler runs in a separate Bevy system on the next tick, breaking reentrancy.

つまり `gs:record_knowledge` (K-2) は **同じ pattern**:
1. Lua から `record_knowledge { kind, origin_system, payload }` を受ける
2. `parse_request` 相当の fn で Lua table を Rust struct に decode (Lua 参照は Rust 側に持ち込まない)
3. `apply::record_knowledge(world, parsed) -> Result<()>` が `&mut World` のみで `PendingFactQueue` に push
4. `@recorded` subscriber chain は **別経路** (sync ではなく次 tick の dispatch) — §2.4 で掘る

**Deviation note**: `@recorded` は epic 設計上「sync 同期発火、subscriber chain で payload mutate、最終形を queue へ」という要件がある。これは上記 #334 pattern (完全非同期) と矛盾する。§2.4 / §6 / §9 で決定 point を明示する。

### 1.7 `EffectScope` descriptor との関係

`scripting/effect_scope.rs` (514 LoC) は tech / faction callback が declarative effect descriptor (`effect_fire_event`, `hide`, etc.) を返す形で world mutation を delay する pattern。`EffectScope` は **Lua closure 外で preview / apply を symmetric にするため** の DSL (memory:project_lua_gamestate_api.md の Phase B 説明)。

**knowledge の `gs:record_knowledge` は EffectScope 経路を使わず、直接 setter に乗せる**。理由:
- event callback は `GamestateMode::ReadWrite` で live mutation を許されている (`gamestate_scope.rs:216` の `if matches!(mode, GamestateMode::ReadWrite)` 分岐)
- preview / apply symmetry が不要 — knowledge record は副作用、preview する意味がない
- EffectScope は tech effect の preview 表示に使われており、knowledge を混ぜると semantics が壊れる

したがって `record_knowledge` は `push_empire_modifier` / `set_flag` / `request_command` と並ぶ setter group のメンバー。

---

## §2 全体アーキテクチャ

### 2.1 `KindRegistry` resource (Rust 側)

```rust
// macrocosmo/src/knowledge/kind_registry.rs (新設)
#[derive(Resource, Default)]
pub struct KindRegistry {
    pub kinds: HashMap<String, KnowledgeKindDef>,
}

pub struct KnowledgeKindDef {
    pub id: String,                              // "vesk:famine_outbreak" or "core:hostile_detected"
    pub payload_schema: PayloadSchema,            // K-1 は field-level のみ
    pub origin: KindOrigin,                       // Core (Rust built-in) or Lua (define_knowledge)
}

pub enum KindOrigin {
    Core,  // `core:*` namespace、Lua 上書き禁止
    Lua,   // define_knowledge 経由
}

#[derive(Default)]
pub struct PayloadSchema {
    pub fields: HashMap<String, PayloadFieldType>,
}

pub enum PayloadFieldType {
    Number,
    String,
    Boolean,
    Table,
    Entity,   // i64 as Entity::to_bits
}
```

配置: `macrocosmo/src/knowledge/kind_registry.rs` (新モジュール、`knowledge/mod.rs:23` の `pub mod facts; pub mod perceived;` の隣に `pub mod kind_registry;` を追加)。

**構築 flow** (K-1 + K-5):

```
Startup:
  init_scripting              (scripting/mod.rs:40)
  load_all_scripts            (mod.rs:43)
  load_knowledge_kinds        ← NEW system, .after(load_all_scripts)
    │ Rust 側 KindRegistry::new() に core:* を preload
    │ Lua `_knowledge_kind_definitions` accumulator を parse、id 重複 / core: 衝突を error
    │ 各 kind の `<id>@recorded` / `<id>@observed` を subscription registry に self-register
    │   (K-3 land 前は placeholder、land 後は実際のエントリを作る)
  run_lifecycle_hooks         (mod.rs:98)
    │ on_game_start 内で on(...) / record_knowledge も呼べる
```

K-1 の `load_knowledge_kinds` は `.after(load_all_scripts).before(run_lifecycle_hooks)` でオーダリング。K-5 land 時は preload 部分を同関数内で済ませる (新規 system を足さない)。

### 2.2 Knowledge subscription registry (Rust + Lua 両側)

epic 設計サマリと #352 spec の通り、`define_event` の `_event_handlers` とは **別 registry**。理由は §1.5 に記した。

#### 2.2.1 Lua 側 accumulator

`_knowledge_subscribers` global table (新設):

```
_knowledge_subscribers = {
  {
    event_id = "vesk:famine_outbreak@recorded",  -- literal kind id + lifecycle
    func = function(e) ... end,
  },
  {
    event_id = "*@observed",                      -- wildcard
    func = function(e) ... end,
  },
  ...
}
```

登録 API は `on(event_id, fn)` (`globals.rs:187-242`) を拡張。knowledge event_id は pattern `^[^@]+@(recorded|observed)$` または `^\*@(recorded|observed)$` にマッチするものを `_knowledge_subscribers` に振り分ける。それ以外 (例 `"harvest_ended"`) は既存 `_event_handlers` へ (後方互換を保つ)。

#### 2.2.2 Rust 側 dispatcher

K-3 は **Lua table をそのまま walk** する実装を採用 (RegistryKey ベースの cached dispatcher は v2)。`dispatch_knowledge` (新 fn、`scripting/knowledge_dispatch.rs` 新モジュール):

```rust
pub fn dispatch_knowledge(
    lua: &Lua,
    kind_id: &str,
    lifecycle: &str,        // "recorded" or "observed"
    payload_table: &Table,  // mutable shared table (@recorded) or per-observer copy (@observed)
) -> mlua::Result<()>
```

1. `_knowledge_subscribers` から全 entry を順に scan
2. `entry.event_id` が `{kind_id}@{lifecycle}` or `*@{lifecycle}` にマッチしたら call
3. entry の error は warn + chain 続行 (#352 spec)
4. dispatch 順 = 登録順 (per-kind と wildcard を **統一登録順**、#352 spec に従う)

**Performance note**: 毎回 full scan だと O(N) per dispatch。現実的な N (< 200 subscriber) では無視できる。将来 K の性能問題が出たら per-lifecycle bucket + per-kind index cache を持つ (v2)。`_event_handlers` の現行実装も full scan なので precedent あり。

### 2.3 `core:` namespace 保護

K-1 で Lua の `define_knowledge` が呼ばれた時に `id.starts_with("core:")` なら error:

```rust
// scripting/knowledge_api.rs
if id.starts_with("core:") {
    return Err(mlua::Error::RuntimeError(
        format!("define_knowledge: 'core:' namespace is reserved for Rust-side variants (got '{id}')")
    ));
}
```

K-5 で Rust 側が KindRegistry に `core:*` を preload し、duplicate check は同じ `KindRegistry::insert(def)` 経路で行う (Lua 側 / Rust 側 で 2 度 insert しようとしたら error)。

### 2.4 Payload mutation chain の Rust 側実装

**Design tension**: epic は「`@recorded` は sync 発火、subscriber chain で payload を in-place mutate、最終形を PendingFactQueue に enqueue」を要求している (memory:project_scriptable_knowledge.md の Lifecycle table)。一方 `feedback_rust_no_lua_callback` は「write helper (apply_*) は Lua 不接触」を invariant とする。

両立する唯一の形:

**`gs:record_knowledge` 呼び出し flow**:

```
Lua callback: e:gamestate:record_knowledge { kind=..., origin_system=..., payload=... }
  │
  ▼
closure body (create_function_mut, has &Lua because scope closure):
  1. parse args (kind_id: String, origin_system: u64, payload: Table)  ← Lua 解析は OK
  2. KindRegistry で kind 検証 + payload_schema validation  ← &World を borrow (try_borrow_mut)
  3. payload を sealed metadata で wrap (`kind` / `origin_system` / `recorded_at` sealed sub-field)
  4. dispatch_knowledge(lua, kind_id, "recorded", &wrapped_payload)  ← Lua subscriber chain 実行 (world は unborrow 済)
  5. 最終 payload を snapshot (Lua table → serde-friendly map / PayloadSnapshot struct)
  6. apply::enqueue_scripted_fact(&mut World, parsed_fact)  ← &Lua 不使用の Rust 関数、PendingFactQueue に push
```

これで成立する。**key point**:
- step 4 の `dispatch_knowledge` は closure body に居るので `&Lua` を持てる (scope closure であるため) — **write helper ではない**
- step 6 の `apply::enqueue_scripted_fact` は Lua 不接触、`feedback_rust_no_lua_callback` invariant 遵守
- step 4 と step 6 の間で world の borrow を解放する必要がある (dispatch 中に Lua subscriber が `e.gamestate:set_flag(...)` 等を呼ぶと再借用するため)

**Reentrancy scenario**: subscriber 内で再び `gs:record_knowledge` を呼んだら?  → 新たな closure invocation、新 `try_borrow_mut`、完了するまでは outer の dispatch が一時停止。これは #334 Phase 4 の `on_command_completed` 内 `gs:request_command` と同じ (plan §9.2 / memory:project_lua_gamestate_api.md の「Reentrancy 検証」段落)。

**`@observed` は非同期**:

```
next-tick system: drain_knowledge_observed (新規 system、notify_from_knowledge_facts と同じ schedule 段階)
  │
  1. PendingFactQueue::drain_ready(clock.elapsed)
  │   Scripted variant は lifecycle event 化、core variant は従来の notification + lifecycle event 両方
  2. 各 observer empire 毎に:
  │    payload を deep-copy (Lua table clone helper, §2.5)
  │    observer / lag metadata を sealed sub-field として注入
  │    dispatch_knowledge(lua, kind_id, "observed", &per_observer_payload)
  3. (core variant のみ) 並行して notify_from_knowledge_facts の legacy path も走る
```

observer が player 1 人 (v1) なので最初は 1 iteration、将来 empire 多数に拡張する時に loop を Query で回す。

### 2.5 Payload deep-copy (per-observer isolation)

`@observed` subscriber chain が observer A の payload を mutate しても observer B に影響しない、という spec (#353) を満たすには **Lua table の deep copy** が必要。

候補実装:
1. `lua.load("function(t) ... end")` で Lua 側 deep-copy 関数を事前に load、Rust から call
2. Rust 側で `Table::pairs` を recurse して新しい `Table` を生成

**推奨**: 2 (Rust 側)。sealed metadata (K-2 で導入) との整合を取りやすい、`_def_type` / `_sealed` 等の prefix を見て除外する実装も Rust 側で閉じる。

実装場所: `scripting/knowledge_dispatch.rs::deep_copy_payload(lua, src, depth_limit)` — nested table を再帰コピー、depth_limit (例 8) を超えたら error。非 table 値は clone (Lua value は Cheap clone)。Function / UserData が混入していたら error (schema 違反)。

**Spike 推奨**: mlua 0.11 `Table::pairs` + `set_metatable` の挙動を最小 test で確認 (§9.3 に記載)。

### 2.6 Sealed metadata (Lua 側上書き禁止)

spec: `kind` / `origin_system` / `recorded_at` / `observed_at` / `observer_empire` / `lag_hexadies` は payload と一緒に Lua subscriber に渡すが、**書き換え禁止**。書き換え試行は `mlua::Error::RuntimeError`。

実装 option:

| option | 説明 | 利点 | 欠点 |
|---|---|---|---|
| A | metadata を payload table の sealed sub-field に隔離 (`e._meta.kind` 等)、`_meta` に `__newindex` で panic metatable | 実装シンプル | Lua 側 API が `e._meta.kind` でダサい、epic spec (`e.kind`) と衝突 |
| B | payload table 自体に `__newindex` metatable を付け、書き換え対象 field (`kind` etc.) だけ block | spec に忠実 (`e.kind` でアクセス可) | payload の他 field は書き換え可にする必要、metatable で分岐 (= slow-ish) |
| C | metadata を別 table で渡す (`e.meta.kind` + `e.payload.severity`)、`e.meta` を seal | spec 違反 (epic は `e.kind` と `e.payload.severity` を別 level で想定) | — |

**推奨: B**。epic spec (`e.kind`, `e.payload.severity`) を尊重。`__newindex` metatable で immutable keys set `{kind, origin_system, recorded_at, observed_at, observer_empire, lag_hexadies}` に書き込みを block、他は passthrough。`e.payload` 自体は plain table (mutable、epic spec 通り mutation 許可)。

**Seal helper** は既存 `gamestate_view.rs` の `seal_table` (`gamestate_scope.rs` への移行後も pattern として生存、plan-332 §1.1 参照) を参考に実装する:

```lua
-- internally set metatable
e = {                             -- plain table
  kind = "vesk:famine_outbreak",  -- immutable (metatable trap)
  origin_system = 42,
  recorded_at = 1234,
  payload = { severity = 0.7 },   -- plain mutable table
}
setmetatable(e, { __newindex = function(t, k, v) error("immutable key: "..k) end })
```

`payload` は nested、metatable を付けない (subscriber chain で自由に書き換え可)。payload の nested field は subscriber chain 内で自由に mutate できる。

### 2.7 Error handling (subscriber error)

spec: "subscriber error は warn log + 残り chain 続行" (#352 完了条件、memory:project_scriptable_knowledge.md `subscription registry`)。

`dispatch_knowledge` で各 `func.call::<()>(payload_table.clone())` を `Result` で受け、`Err(e)` は `warn!("knowledge subscriber error for {id}@{lc}: {e}")` して次へ。`payload_table.clone()` は Lua table の ref clone なので cheap。

**既存 `dispatch_bus_handlers` (lifecycle.rs:496-544)** の pattern に合わせる (L538-542 の warn + continue が precedent)。

### 2.8 Thread / scope safety

- `gamestate_scope` の `RefCell<&mut World>` と `dispatch_knowledge` で同じ world borrow を踏まないよう、`dispatch_knowledge` は **borrow 解放後** に呼ぶ (§2.4 の step 4 参照)。
- Lua は single-threaded (mlua 0.11 lua state)、再入だけ注意すれば thread safety は発生しない。
- `dispatch_knowledge` 呼び出し中に subscriber が `gs:record_knowledge` を再呼び出しする場合: outer の `record_knowledge` closure はすでに world borrow を解放済みなので、inner の `try_borrow_mut` は成功する。Lua 呼び出し stack は深くなるが、stack overflow は depth_limit (例 64) で検出 (future work)。
- v1 は depth_limit 実装せず、panic なしで recurse させる (test で 10 層程度を動作確認)。

### 2.9 `*@<lifecycle>` wildcard matcher

v1 は **suffix wildcard のみ**: `*@recorded` / `*@observed`。

実装 (dispatch 時):

```rust
fn event_id_matches(pattern: &str, kind_id: &str, lifecycle: &str) -> bool {
    if let Some((pat_kind, pat_lc)) = pattern.rsplit_once('@') {
        if pat_lc != lifecycle { return false; }
        if pat_kind == "*" { return true; }
        pat_kind == kind_id
    } else {
        false
    }
}
```

**Validation**: 登録時に `event_id` が `[^@]+@(recorded|observed)$` または `\*@(recorded|observed)$` にマッチしなければ、knowledge subscription registry には入れず **通常 `_event_handlers` へ fallback** (既存 `on()` の挙動を壊さない)。

K-3 は `on()` 本体 (`globals.rs:187-242`) に「event_id を検査して `_knowledge_subscribers` vs `_event_handlers` に振り分け」コードを追加する。

**Spike 推奨**: lifecycle suffix 検査の正規表現 (or `rsplit_once('@')`) の挙動で `vesk:famine_outbreak@recorded` / `vesk:famine_outbreak@observed` / `*@recorded` / `"harvest_ended"` を正しく分類できることを確認 (§9.2)。

---

## §3 sub-issue 別実装プラン

### §3.1 K-1 (#350): `define_knowledge` + KindRegistry + 自動 lifecycle event 登録

#### 新モジュール

- `macrocosmo/src/scripting/knowledge_api.rs` 新設 (推定 200-280 LoC)
  - `parse_knowledge_definitions(lua) -> Result<Vec<KnowledgeKindDef>>`
  - `parse_payload_schema(table) -> Result<PayloadSchema>`
  - `register_knowledge_kind_auto_events(lua, id) -> Result<()>` — `_knowledge_kind_definitions` accumulator を walk、各 kind に対し自動 lifecycle event を subscription registry に placeholder として登録
- `macrocosmo/src/knowledge/kind_registry.rs` 新設 (推定 120-180 LoC)
  - `KindRegistry`, `KnowledgeKindDef`, `KindOrigin`, `PayloadSchema`, `PayloadFieldType`
  - `KindRegistry::insert(def) -> Result<(), KindRegistryError>` (duplicate / core: namespace error)
  - `KindRegistry::validate_payload(&self, id, &Table) -> Result<(), mlua::Error>` — K-2 でも使うのでここに置く

#### `scripting/globals.rs` 変更

`register_define_fn(lua, "knowledge", "_knowledge_kind_definitions")` を追加 (1 line)。`define_knowledge { id, payload_schema }` の accumulator tag を配置。

更に自動 event 登録のため、K-1 単体で `_knowledge_subscribers` テーブルを用意する (K-3 が実装する前でも parser が参照先を持てるように):

```rust
// globals.rs に追加 (K-1)
let subs = lua.create_table()?;
globals.set("_knowledge_subscribers", subs)?;
```

K-3 の `on()` 拡張側でこの table を活用する (registration 先切り替え)。

#### `scripting/mod.rs` 変更

```rust
// 新 system: load_knowledge_kinds
.add_systems(
    Startup,
    load_knowledge_kinds
        .after(load_all_scripts)
        .before(lifecycle::run_lifecycle_hooks),
)
```

`load_knowledge_kinds` (新 fn):
```rust
pub fn load_knowledge_kinds(mut commands: Commands, engine: Res<ScriptEngine>) {
    let mut registry = knowledge::kind_registry::KindRegistry::default();
    // K-5 で core:* を preload (この plan の wave 4 で追加)
    match knowledge_api::parse_knowledge_definitions(engine.lua()) {
        Ok(defs) => {
            for def in defs {
                if let Err(e) = registry.insert(def) {
                    warn!("knowledge kind register error: {e}");
                }
            }
        }
        Err(e) => warn!("Failed to parse knowledge definitions: {e}"),
    }
    commands.insert_resource(registry);
}
```

#### `scripts/knowledge/sample.lua` (optional test fixture)

Issue 完了条件の `sample.lua` を `macrocosmo/scripts/knowledge/sample.lua` に置く:

```lua
define_knowledge {
  id = "sample:colony_famine",
  payload_schema = {
    severity = "number",
    colony = "entity",
  },
}
```

`scripts/init.lua` に `require("knowledge.sample")` を追加。

#### payload_schema validation

v1 緩い validation — `PayloadSchema::fields: HashMap<String, PayloadFieldType>` に対し、`record_knowledge` (K-2) 時にチェック:
- field が schema にある → 型チェック (Lua table field の type name で判定)
- field が schema にない → v1 は許容 (warn 出すだけ、strict は v2)
- nested schema は拒否 (schema parser で table 値を与えたら error)

K-1 単体では schema parser + unit test まで。実際の validate call site は K-2。

#### commit / LoC 推定

| # | commit | 主な変更 | LoC |
|---|---|---|---|
| 1 | `add KindRegistry + PayloadSchema resource` | `knowledge/kind_registry.rs` 新設 | ~150 |
| 2 | `add define_knowledge + parse_knowledge_definitions` | `scripting/knowledge_api.rs` 新設, `globals.rs` 3 行 | ~220 |
| 3 | `add load_knowledge_kinds startup system` | `scripting/mod.rs`, plugin wiring | ~50 |
| 4 | `add _knowledge_subscribers accumulator + self-register lifecycle events` | `globals.rs`, `knowledge_api.rs` の `register_knowledge_kind_auto_events` | ~80 |
| 5 | `scripts/knowledge/sample.lua + integration test` | Lua fixture + test | ~60 |

**Total: 5 commits, ~560 LoC (Rust 450 + Lua 110)**。

#### Test 計画 (K-1)

- `test_parse_knowledge_definitions_minimum` — id のみの kind が登録される
- `test_parse_knowledge_with_payload_schema` — field 型が正しく parse される
- `test_parse_knowledge_duplicate_id_errors` — 2 回定義で error
- `test_parse_knowledge_core_namespace_rejected` — `id = "core:xxx"` で error
- `test_parse_knowledge_nested_schema_rejected` — v1 は nested 禁止
- `test_auto_lifecycle_events_registered` — define 後に `_knowledge_subscribers` に placeholder (K-3 land 後は実動作) が入る or validation のみ通る
- `test_payload_schema_invalid_type_string_rejected` — `severity = "cucumber"` で error

### §3.2 K-3 (#352): `on(event_id, fn)` subscription + suffix wildcard

**Order**: K-1 と K-3 は並列 (independent、Wave 1)。

#### `scripting/globals.rs` の `on_fn` 拡張

現行 (L188-241) を修正:

```rust
let on_fn = lua.create_function(|lua, args: mlua::MultiValue| {
    // 既存: event_id と second (filter / handler) を parse

    // NEW: event_id が knowledge pattern に一致したら _knowledge_subscribers へ、それ以外は _event_handlers へ
    let target_table_name = if is_knowledge_event_id(&event_id_str) {
        "_knowledge_subscribers"
    } else {
        "_event_handlers"
    };
    let handlers: mlua::Table = lua.globals().get(target_table_name)?;
    // ... 既存 flow の続き
})?;

fn is_knowledge_event_id(s: &str) -> bool {
    // `*@recorded` / `*@observed` / `<anything>@recorded` / `<anything>@observed`
    match s.rsplit_once('@') {
        Some((_, lc)) => lc == "recorded" || lc == "observed",
        None => false,
    }
}
```

**Lifecycle validation**: `on("foo@expired", fn)` は `expired` が v1 未サポートなので **load time で error**:

```rust
if let Some((kind_part, lc)) = event_id_str.rsplit_once('@') {
    match lc {
        "recorded" | "observed" => { /* OK */ }
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "on(): knowledge lifecycle '{other}' not recognized (expected 'recorded' or 'observed')"
            )));
        }
    }
}
```

#### 新 dispatcher module

`macrocosmo/src/scripting/knowledge_dispatch.rs` 新設 (推定 250-350 LoC):

```rust
pub fn dispatch_knowledge(
    lua: &Lua,
    kind_id: &str,
    lifecycle: KnowledgeLifecycle,  // Recorded or Observed
    payload: &Table,
) -> mlua::Result<()> {
    let subs: Table = lua.globals().get("_knowledge_subscribers")?;
    let len = subs.len()?;
    for i in 1..=len {
        let entry: Table = match subs.get(i) { Ok(t) => t, Err(_) => continue };
        let pattern: String = match entry.get("event_id") { Ok(s) => s, Err(_) => continue };
        if !event_id_matches(&pattern, kind_id, lifecycle.as_str()) {
            continue;
        }
        let func: Function = match entry.get("func") { Ok(f) => f, Err(_) => continue };
        if let Err(e) = func.call::<()>(payload.clone()) {
            warn!("knowledge subscriber error for {kind_id}@{}: {e}", lifecycle.as_str());
        }
    }
    Ok(())
}

pub enum KnowledgeLifecycle {
    Recorded,
    Observed,
}

impl KnowledgeLifecycle {
    pub fn as_str(&self) -> &'static str { /* ... */ }
}

pub fn deep_copy_payload(lua: &Lua, src: &Table, depth_limit: u32) -> mlua::Result<Table> { /* §2.5 */ }

pub fn seal_immutable_keys(lua: &Lua, payload: &Table, keys: &[&str]) -> mlua::Result<()> { /* §2.6 */ }
```

K-3 の commit では `dispatch_knowledge` + seal + deep_copy の骨組みのみ land、実際の呼び出し site は K-2 / K-4。

#### commit / LoC 推定

| # | commit | 主な変更 | LoC |
|---|---|---|---|
| 1 | `route on() by event_id to knowledge vs event handlers` | `globals.rs` ~30 | ~80 |
| 2 | `add knowledge_dispatch module skeleton` | `knowledge_dispatch.rs` 新設 | ~280 |
| 3 | `add wildcard suffix matcher + unit tests` | 同 module | ~100 |
| 4 | `add seal_immutable_keys metatable helper + tests` | 同 module | ~120 |

**Total: 4 commits, ~580 LoC**。

#### Test 計画 (K-3)

- `test_on_routes_knowledge_id_to_subscribers` — `on("foo:bar@recorded", fn)` が `_knowledge_subscribers` に入る
- `test_on_routes_legacy_event_id_to_handlers` — `on("harvest_ended", fn)` が従来通り `_event_handlers` に入る
- `test_on_invalid_lifecycle_errors` — `on("foo@expired", fn)` で error
- `test_dispatch_per_kind_subscriber` — `dispatch_knowledge("foo", Recorded, ...)` で only `foo@recorded` 登録の func が呼ばれる
- `test_dispatch_wildcard_observed` — `*@observed` subscriber も dispatch 対象
- `test_dispatch_preserves_registration_order` — per-kind と wildcard を混ぜて登録順で呼ばれる
- `test_subscriber_error_continues_chain` — 1 個目の subscriber が error raise しても 2 個目以降が呼ばれる
- `test_seal_immutable_keys_blocks_write` — metatable 付け payload の `kind` 書き換えで error
- `test_seal_allows_payload_mutation` — `e.payload.severity = 0.5` は成功
- `test_deep_copy_payload_independent` — copy 後 mutate しても元 payload 不変
- `test_deep_copy_rejects_function_values` — schema 違反表現で error

### §3.3 K-2 (#351): `gs:record_knowledge` setter + `@recorded` 同期発火 + PendingFactQueue 連携

**Depends on**: K-1 (KindRegistry で kind 検証), K-3 (`dispatch_knowledge` + seal / deep_copy helpers)。

#### `knowledge/facts.rs` 拡張

`KnowledgeFact` enum に新 variant:

```rust
// facts.rs:263 の下に追加
/// Lua-defined knowledge kind (scripted content). The payload is
/// captured as a PayloadSnapshot (serde-compatible) so the fact survives
/// being queued without keeping Lua references alive.
Scripted {
    event_id: Option<EventId>,
    kind_id: String,                          // "vesk:famine_outbreak"
    origin_system: Option<Entity>,            // queue-level light-speed calc 用
    payload_snapshot: PayloadSnapshot,        // immutable at record time (@recorded chain 終了後)
    recorded_at: i64,
},
```

`PayloadSnapshot` (新 struct、`knowledge/kind_registry.rs` に併置 or `knowledge/payload.rs` 新設):

```rust
#[derive(Clone, Debug, Default)]
pub struct PayloadSnapshot {
    pub fields: HashMap<String, PayloadValue>,
}

#[derive(Clone, Debug)]
pub enum PayloadValue {
    Number(f64),
    Int(i64),
    String(String),
    Boolean(bool),
    Table(PayloadSnapshot),  // nested
    Entity(Entity),
}
```

postcard で serialize 可 (future #247 対応)。v1 は persistence しないので derive のみ。

`impl KnowledgeFact`:
- `title()` / `description()` / `priority()` / `related_system()` / `event_id()` を Scripted variant にも対応
- Scripted は default priority **Medium** (v1、後で per-kind config 化)
- `title()` は `kind_id` をそのまま、`description()` は payload から `detail` field があればそれを、なければ空文字

#### `scripting/gamestate_scope.rs` に setter 追加

`build_gs_table` の `if matches!(mode, GamestateMode::ReadWrite)` ブロック (L216-322) に:

```rust
let record_knowledge = s.create_function_mut(
    move |lua, (_this, args): (Table, Table)| {
        // parse
        let kind_id: String = args.get("kind")?;
        let origin_system: Option<u64> = args.get("origin_system").ok();
        let payload_lua: Table = args.get("payload")?;

        // step 1: validate kind + schema
        // need KindRegistry, borrow world
        {
            let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
            let registry = (*borrow).get_resource::<KindRegistry>()
                .ok_or_else(|| mlua::Error::RuntimeError("KindRegistry missing".into()))?;
            registry.validate_payload(&kind_id, &payload_lua)?;
        }

        // step 2: wrap payload in sealed event table
        let recorded_at = { /* world.borrow の中で clock.elapsed 取得 */ };
        let event = lua.create_table()?;
        event.set("kind", kind_id.as_str())?;
        if let Some(oid) = origin_system { event.set("origin_system", oid)?; }
        event.set("recorded_at", recorded_at)?;
        event.set("payload", payload_lua.clone())?;  // reference, mutable
        knowledge_dispatch::seal_immutable_keys(lua, &event,
            &["kind", "origin_system", "recorded_at"])?;

        // step 3: dispatch @recorded (world is NOT borrowed here → subscriber may re-call gs)
        knowledge_dispatch::dispatch_knowledge(
            lua, &kind_id, KnowledgeLifecycle::Recorded, &event
        )?;

        // step 4: snapshot final payload
        let final_payload_table: Table = event.get("payload")?;
        let snapshot = knowledge_payload::snapshot_from_lua(lua, &final_payload_table)?;

        // step 5: enqueue (Lua-free apply)
        let mut borrow = world_cell.try_borrow_mut().map_err(map_reentrancy_err)?;
        apply::record_knowledge(
            &mut **borrow,
            ParsedKnowledgeRecord {
                kind_id,
                origin_system: origin_system.map(Entity::from_bits),
                payload_snapshot: snapshot,
                recorded_at,
            },
        )?;
        Ok(())
    },
)?;
gs.set("record_knowledge", record_knowledge)?;
```

#### `scripting/gamestate_scope::apply::record_knowledge`

`gamestate_scope.rs:997-` の `pub(crate) mod apply` に:

```rust
pub struct ParsedKnowledgeRecord {
    pub kind_id: String,
    pub origin_system: Option<Entity>,
    pub payload_snapshot: PayloadSnapshot,
    pub recorded_at: i64,
}

/// Enqueue a scripted KnowledgeFact into PendingFactQueue. Never touches Lua.
///
/// Invariant (plan §9.2 / feedback_rust_no_lua_callback.md): this helper takes
/// no &Lua and never invokes Lua code. All subscriber dispatch happens upstream
/// in the scope closure.
pub fn record_knowledge(world: &mut World, req: ParsedKnowledgeRecord) -> mlua::Result<()> {
    use crate::knowledge::facts::*;

    // Allocate event id for dedup (reuse existing NextEventId)
    let event_id = {
        let mut nid = world.get_resource_mut::<NextEventId>()
            .ok_or_else(|| mlua::Error::RuntimeError("NextEventId missing".into()))?;
        nid.allocate()
    };
    world.resource_mut::<NotifiedEventIds>().register(event_id);

    let fact = KnowledgeFact::Scripted {
        event_id: Some(event_id),
        kind_id: req.kind_id,
        origin_system: req.origin_system,
        payload_snapshot: req.payload_snapshot,
        recorded_at: req.recorded_at,
    };

    // Compute arrival time
    let origin_pos = req.origin_system
        .and_then(|e| world.get::<Position>(e).map(|p| p.as_array()))
        .unwrap_or([0.0, 0.0, 0.0]);
    let player_vantage = collect_player_vantage(world);
    let comms = collect_empire_comms(world);
    let relays = world.get_resource::<RelayNetwork>().cloned().unwrap_or_default();

    let mut queue = world.resource_mut::<PendingFactQueue>();
    let mut notifications = world.resource_mut::<NotificationQueue>();
    let mut notified_ids = world.resource_mut::<NotifiedEventIds>();

    record_fact_or_local(
        fact,
        origin_pos,
        req.recorded_at,
        player_vantage.player_aboard,
        player_vantage.player_pos,
        &mut queue,
        &mut notifications,
        &mut notified_ids,
        &relays.relays,
        &comms,
    );
    Ok(())
}

fn collect_player_vantage(world: &mut World) -> PlayerVantage { /* ... */ }
fn collect_empire_comms(world: &mut World) -> CommsParams { /* ... */ }
```

**Note**: 複数 resource を同時 `resource_mut` すると conflict なので `let mut q = world.resource_mut::<A>();` / drop / next. `PendingFactQueue`, `NotificationQueue`, `NotifiedEventIds`, `RelayNetwork`, `NextEventId` は別 resource なので順番に取れば OK。`collect_*` で Query を使う部分は `world.query*` で処理。

#### `notify_from_knowledge_facts` の Scripted variant 対応

`notifications.rs:290-317` — drain した PerceivedFact.fact が Scripted variant の場合は **banner 発火をスキップ** (@observed で Lua subscriber が拾う想定、#345 bridge 後に banner 化される)。priority が High なら pause もスキップ。

```rust
for PerceivedFact { fact, .. } in ready {
    if matches!(fact, KnowledgeFact::Scripted { .. }) {
        // K-2: Scripted variants are dispatched via knowledge subscribers (K-4),
        // not banner-pushed here.
        continue;  // but still drain via drain_ready (already removed from queue)
    }
    // existing path for core variants
    ...
}
```

ただしこの edit を K-2 で入れると K-4 未実装時に Scripted facts が何も発火せず消える。したがって **K-2 時点ではまだ Scripted だけ queue に溜まるだけ** でも OK (@observed の処理は K-4)。K-2 の test は「queue に正しく enqueue された」を直接確認する形に。

#### commit / LoC 推定

| # | commit | 主な変更 | LoC |
|---|---|---|---|
| 1 | `add KnowledgeFact::Scripted variant + PayloadSnapshot` | `facts.rs`, `knowledge/payload.rs` 新設 | ~200 |
| 2 | `add snapshot_from_lua helper` | `scripting/knowledge_payload.rs` 新設 | ~150 |
| 3 | `add apply::record_knowledge + vantage/comms helpers` | `gamestate_scope.rs` apply module | ~180 |
| 4 | `add gs:record_knowledge setter with @recorded dispatch` | `gamestate_scope.rs` build_gs_table | ~120 |
| 5 | `integration test: record → @recorded chain → queued` | `tests/knowledge_record.rs` 新設 | ~200 |

**Total: 5 commits, ~850 LoC**。

#### Test 計画 (K-2)

- `test_record_knowledge_no_subscribers_enqueues_unchanged_payload` — subscriber 0 でも PendingFactQueue に Scripted が入る
- `test_record_knowledge_single_subscriber_mutates_payload` — `e.payload.severity = 0.5` が最終 enqueue 値に反映
- `test_record_knowledge_subscriber_chain_sequential_mutation` — 2 個の subscriber が順に payload を書き換え、最終値が 2 番目の結果
- `test_record_knowledge_immutable_kind_raises_error` — subscriber が `e.kind = "other"` で runtime error (chain は継続、他 subscriber は走る)
- `test_record_knowledge_light_speed_delay_applied` — origin_system が離れた位置なら arrives_at > recorded_at
- `test_record_knowledge_relay_shortcut` — Relay があれば source = Relay、arrives_at 短縮
- `test_record_knowledge_local_bypass` — player_aboard true で local banner — ただし Scripted は banner 化しないので test は "banner 出ない" を assert (K-4 land まで)
- `test_record_knowledge_unknown_kind_errors` — kind 未登録で Lua error
- `test_record_knowledge_schema_violation_errors` — payload field 型が schema 違反で Lua error
- `test_record_knowledge_reentrancy_permitted` — subscriber 内で `gs:record_knowledge` 再呼び出しが動く (world 借用は解放済)

### §3.4 K-4 (#353): `<id>@observed` 発火 + per-observer copy + observer/lag payload + wildcard dispatch

**Depends on**: K-1, K-2, K-3。

#### 新 system: `dispatch_knowledge_observed`

場所: `macrocosmo/src/scripting/knowledge_dispatch.rs`。schedule: `Update`, `.after(crate::time_system::advance_game_time)`, `.after(notify_from_knowledge_facts)`。

実装 sketch:

```rust
pub fn dispatch_knowledge_observed(world: &mut World) {
    // Fast path
    let has_scripted = world.get_resource::<PendingFactQueue>()
        .map(|q| q.facts.iter().any(|f| matches!(f.fact, KnowledgeFact::Scripted { .. })))
        .unwrap_or(false);
    if !has_scripted { return; }

    // 1) drain ready Scripted facts (NOT core variants — those stay in legacy path)
    let now = world.resource::<GameClock>().elapsed;
    let ready: Vec<PerceivedFact> = {
        let mut q = world.resource_mut::<PendingFactQueue>();
        // 新メソッド: drain_ready_scripted が Scripted だけ取り出す。core variant は残す。
        q.drain_ready_scripted(now)
    };

    if ready.is_empty() { return; }

    // 2) collect observer empires (v1 は player のみ)
    let observer_empires: Vec<Entity> = {
        let mut qq = world.query_filtered::<Entity, With<PlayerEmpire>>();
        qq.iter(world).collect()
    };

    // 3) per-observer dispatch
    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();
        for pf in &ready {
            let (kind_id, recorded_at, origin_system, payload_snapshot) = match &pf.fact {
                KnowledgeFact::Scripted { kind_id, recorded_at, origin_system, payload_snapshot, .. } =>
                    (kind_id.clone(), *recorded_at, *origin_system, payload_snapshot.clone()),
                _ => continue,
            };
            for &observer in &observer_empires {
                // deep-copy payload per observer
                let payload_tbl = knowledge_payload::snapshot_to_lua(lua, &payload_snapshot).unwrap();
                let event = build_observed_event_table(
                    lua,
                    &kind_id,
                    origin_system,
                    recorded_at,
                    pf.arrives_at,
                    observer,
                    &payload_tbl,
                ).unwrap();
                // seal immutable keys
                seal_immutable_keys(lua, &event,
                    &["kind", "origin_system", "recorded_at", "observed_at", "observer_empire", "lag_hexadies"])
                    .unwrap();
                if let Err(e) = dispatch_knowledge(lua, &kind_id, KnowledgeLifecycle::Observed, &event) {
                    warn!("knowledge dispatch error: {e}");
                }
            }
        }
    });
}
```

`build_observed_event_table` は event table を組み立て、`lag_hexadies = observed_at - recorded_at` を計算。

#### `PendingFactQueue::drain_ready_scripted`

`knowledge/facts.rs` 397-408 の `drain_ready` と同じく、ただし `Scripted` 判定で filter:

```rust
pub fn drain_ready_scripted(&mut self, now: i64) -> Vec<PerceivedFact> {
    let mut ready = Vec::new();
    let mut i = 0;
    while i < self.facts.len() {
        let is_scripted = matches!(self.facts[i].fact, KnowledgeFact::Scripted { .. });
        if is_scripted && self.facts[i].arrives_at <= now {
            ready.push(self.facts.remove(i));
        } else {
            i += 1;
        }
    }
    ready
}
```

core variant は引き続き `drain_ready` が取り出して `notify_from_knowledge_facts` が banner 化。2 system が同じ queue を順に drain する (`drain_ready_scripted` → `notify_from_knowledge_facts`)。

#### Plugin wiring

`scripting/mod.rs` に:

```rust
.add_systems(
    Update,
    knowledge_dispatch::dispatch_knowledge_observed
        .after(crate::time_system::advance_game_time)
        .after(crate::notifications::notify_from_knowledge_facts)
        // world-mut exclusive; run serially after notifications.
)
```

`notify_from_knowledge_facts` 側も Scripted を skip するコードを加える (§3.3 K-2 の末尾)。

#### commit / LoC 推定

| # | commit | 主な変更 | LoC |
|---|---|---|---|
| 1 | `PendingFactQueue::drain_ready_scripted + tests` | `facts.rs` | ~100 |
| 2 | `snapshot_to_lua helper + round-trip tests` | `scripting/knowledge_payload.rs` | ~150 |
| 3 | `dispatch_knowledge_observed exclusive system + wiring` | `knowledge_dispatch.rs`, `scripting/mod.rs` | ~220 |
| 4 | `observer/lag metadata injection + seal` | `knowledge_dispatch.rs` | ~100 |
| 5 | `integration tests (per-observer isolation, wildcard, lag calc)` | `tests/knowledge_observed.rs` | ~220 |

**Total: 5 commits, ~790 LoC**。

#### Test 計画 (K-4)

- `test_observed_fires_after_arrival_time` — recorded 後 light-speed delay 経過で @observed 到来
- `test_observed_does_not_fire_before_arrival` — delay 未満なら発火せず
- `test_observed_single_observer_receives_payload` — player empire が受信
- `test_observed_two_observers_independent_mutation` — (設計検証用; v1 では NPC observer ないので将来用、skip 可)
- `test_observed_wildcard_subscriber` — `on("*@observed", fn)` が Scripted 到着で呼ばれる
- `test_observed_lag_hexadies_matches_delay` — `e.lag_hexadies == observed_at - recorded_at`
- `test_observed_observer_empire_id_present` — `e.observer_empire` が正しい Entity bits
- `test_observed_immutable_metadata_raises_on_write` — `e.kind = "x"` で error、chain 継続
- `test_observed_payload_mutation_survives_chain` — 2 subscriber が順に mutate、最終値が 2 番目
- `test_observed_subscriber_error_continues_chain` — regression

### §3.5 K-5 (#354): 既存 Rust `KnowledgeFact` variants に `core:*` namespace 付与 + 両 lifecycle event 発火 wire

**Depends on**: K-1 (KindRegistry), K-2 (record_knowledge), K-4 (observed dispatch)。

#### Core kind id mapping

`knowledge/kind_registry.rs` に built-in preloader:

```rust
impl KindRegistry {
    pub fn preload_core() -> Self {
        let mut r = Self::default();
        // mapping table (Rust side)
        r.insert(KnowledgeKindDef {
            id: "core:hostile_detected".into(),
            origin: KindOrigin::Core,
            payload_schema: build_core_schema(&[
                ("target", PayloadFieldType::Entity),
                ("detector", PayloadFieldType::Entity),
                ("target_pos_x", PayloadFieldType::Number),
                ("target_pos_y", PayloadFieldType::Number),
                ("target_pos_z", PayloadFieldType::Number),
                ("description", PayloadFieldType::String),
            ]),
        }).unwrap();
        r.insert(KnowledgeKindDef {
            id: "core:combat_outcome".into(),
            // victor: String ("player" | "hostile"), system: Entity, detail: String
            ...
        }).unwrap();
        r.insert(KnowledgeKindDef { id: "core:survey_complete".into(), ... }).unwrap();
        r.insert(KnowledgeKindDef { id: "core:anomaly_discovered".into(), ... }).unwrap();
        r.insert(KnowledgeKindDef { id: "core:survey_discovery".into(), ... }).unwrap();
        r.insert(KnowledgeKindDef { id: "core:structure_built".into(), ... }).unwrap();
        r.insert(KnowledgeKindDef { id: "core:colony_established".into(), ... }).unwrap();
        r.insert(KnowledgeKindDef { id: "core:colony_failed".into(), ... }).unwrap();
        r.insert(KnowledgeKindDef { id: "core:ship_arrived".into(), ... }).unwrap();
        r
    }

    pub fn insert(&mut self, def: KnowledgeKindDef) -> Result<(), KindRegistryError> {
        if self.kinds.contains_key(&def.id) {
            return Err(KindRegistryError::Duplicate(def.id));
        }
        // Core preload 後 Lua が "core:xxx" を追加しようとすると duplicate error で弾かれる
        self.kinds.insert(def.id.clone(), def);
        Ok(())
    }
}
```

`load_knowledge_kinds` (K-1 で追加) を修正して:

```rust
pub fn load_knowledge_kinds(mut commands: Commands, engine: Res<ScriptEngine>) {
    // K-5: preload core:*
    let mut registry = knowledge::kind_registry::KindRegistry::preload_core();
    // ... then walk Lua definitions
}
```

これで Lua 側 `define_knowledge { id = "core:foo", ... }` は `KindRegistry::insert` が duplicate error を返して warn log (もしくは fatal)。

#### 発火 wire: `FactSysParam::record` に lifecycle event dispatch 追加

現行 (`facts.rs:717-741`):
```rust
pub fn record(&mut self, fact: KnowledgeFact, origin_pos, observed_at, vantage) -> ... {
    ...
    record_fact_or_local(fact, origin_pos, observed_at, ..., &relays, &comms)
}
```

K-5 ではこれに加えて「fact を Scripted ではなく core variant として enqueue する際、同時に lifecycle event 発火用の hook を叩く」必要がある。**ただし** `FactSysParam::record` は Bevy system 内で呼ばれるため `&Lua` を持っていない。ここで Lua subscriber を sync 発火すると `feedback_rust_no_lua_callback` 違反になる。

**解決**: queue-only pattern で二段化:
1. `FactSysParam::record` は現行通り `record_fact_or_local` を呼ぶ (従来 path を維持)
2. 追加で Rust event bus (`EventSystem::fire_event_with_payload`) に `core:<id>@recorded` event を submit。payload は event-context として変換 (`LuaDefinedEventContext` or 新 `KnowledgeFactContext` 型)
3. `dispatch_event_handlers` が次 tick で Lua subscriber chain を起こす

これは epic spec の「**sync 発火**」から少し外れる — core variant の `@recorded` は **次 tick で走る**。ただし以下のどちらかで妥協する:

- **Option A**: core variant `@recorded` は次 tick 発火を許容 (light-speed delay 前の 1 tick latency、人間が気付くレベルじゃない)
- **Option B**: core variant の record path を gamestate_scope 経由に変え、Lua callback 内でのみ発火するよう限定する (tech 的に困難、大量の Rust system を変える)

**推奨: Option A** (§9.4 で user 判断)。

`@observed` 側は Scripted と同じ path (`dispatch_knowledge_observed`) に core variant も流し、per-observer dispatch する:

```rust
// dispatch_knowledge_observed を変更
// Scripted だけでなく core variant も lifecycle event 対象に
let ready: Vec<PerceivedFact> = q.drain_ready_knowledge_lifecycle(now);
// → Scripted or core variants 全部 drain。core variant は notify_from_knowledge_facts が
//    別 drain を使うよう調整 (重複 drain 防止)
```

**重複 drain 防止**: K-5 land で `notify_from_knowledge_facts` と `dispatch_knowledge_observed` が同じ queue を drain しに行く。以下のいずれか:

- **分離**: `notify_from_knowledge_facts` は Notification 発火、`dispatch_knowledge_observed` は Lua subscriber 発火、両者 drain 権 を "観測した 2 回目は no-op" で実装
- **順序合わせ**: queue drain は `dispatch_knowledge_observed` が 1 回実行、その結果を「notification 対象 vector」+「lua subscriber 対象 vector」に分配、それぞれに push

**推奨: 後者** (clean)。`notify_from_knowledge_facts` は drain を手放し、drain は `dispatch_knowledge_observed` に集約、必要な notification push のみ責任分離。

ただしこの変更は `notify_from_knowledge_facts` の pipeline を大きく触る。PR size を抑えるために K-5 内で:

1. commit A: `@recorded` core event の Rust event bus push 追加 (並存、notify_from_knowledge_facts 無変更)
2. commit B: `dispatch_knowledge_observed` が core variant も対象に — **`notify_from_knowledge_facts` と drain を共有する経路に書き換え**、notification は別関数で post-drain に発火

#### backward compat

- 既存 Rust consumer (`auto_notify_from_events`, existing `notify_from_knowledge_facts` callers) は core:* namespace を **知る必要がない**。Rust 側は従来の `KnowledgeFact::HostileDetected` variant で参照する。kind id mapping は **emit 時の lifecycle event payload にのみ** 出現。
- `tests/` の既存 notification test (存在するなら) はそのまま pass する必要がある。K-5 commit で regression check。

#### commit / LoC 推定

| # | commit | 主な変更 | LoC |
|---|---|---|---|
| 1 | `preload core:* in KindRegistry + mapping table` | `kind_registry.rs` | ~200 |
| 2 | `variant → kind_id + payload snapshot converter` | `knowledge/facts.rs`, `knowledge/payload.rs` | ~180 |
| 3 | `fire core:*@recorded through EventSystem queue` | `scripting/knowledge_bridge.rs` 新設 + `FactSysParam` hook | ~220 |
| 4 | `unify PendingFactQueue drain into dispatch_knowledge_observed (core + scripted)` | `facts.rs`, `notifications.rs`, `knowledge_dispatch.rs` | ~260 |
| 5 | `integration: on("core:hostile_detected@observed", fn) receives payload` | `tests/knowledge_core_wire.rs` | ~200 |

**Total: 5 commits, ~1060 LoC**。

#### Test 計画 (K-5)

- `test_core_preload_has_all_variants` — 9 kind が preload される
- `test_lua_cannot_override_core_namespace` — `define_knowledge { id = "core:foo" }` で warn + skip
- `test_core_hostile_detected_fires_recorded_event` — Rust から HostileDetected write → 次 tick で Lua `on("core:hostile_detected@recorded", fn)` が呼ばれる
- `test_core_hostile_detected_fires_observed_event` — observer 到着 tick で `core:hostile_detected@observed` 発火
- `test_core_observed_payload_contains_mapped_fields` — `e.payload.description`, `e.payload.target` 等が Lua から読める
- `test_core_observed_wildcard_catch_all` — `on("*@observed", fn)` で core + scripted 両方受信
- `test_regression_legacy_notification_still_fires` — 従来 banner 発火 path が壊れない (既存 notification test 全部 pass)
- `test_regression_notified_event_ids_still_dedup` — EventId-based dedup が機能
- `test_core_recorded_core_variant_payload_mutable` — subscriber が `e.payload.description = "x"` で mutate → observed 到着時に反映 (per observer は別 copy だが @recorded での mutation は最終 enqueue 値に乗る)

---

## §4 実装順序 (parallelize 戦略)

### Wave 1 (parallel worktree)
- **K-1 (#350)**: `define_knowledge` + KindRegistry + 自動 lifecycle event 登録 placeholder
- **K-3 (#352)**: `on(event_id, fn)` subscription + suffix wildcard + dispatch skeleton

Wave 1 の 2 PR は独立 (KindRegistry resource と `_knowledge_subscribers` table は別)。同時に worktree で着手可能、merge 順は任意。

### Wave 2
- **K-2 (#351)**: `gs:record_knowledge` setter + `@recorded` dispatch + PendingFactQueue enqueue

Wave 1 両方 land 後。K-3 の `dispatch_knowledge` / `seal_immutable_keys` / `deep_copy_payload` を呼ぶので強依存。

### Wave 3
- **K-4 (#353)**: `@observed` exclusive system + per-observer isolation + wildcard

K-2 land 後。PendingFactQueue に Scripted fact が入るようになって初めて drain 対象となる。

### Wave 4
- **K-5 (#354)**: core:* namespace + 既存 variant lifecycle wire

K-4 land 後。`dispatch_knowledge_observed` を core variant 対象に拡張、`notify_from_knowledge_facts` の drain 責任を移動。

### 並列化の利点

- Wave 1 で 2 PR 並列 → 1 week 短縮可
- Wave 2-4 は直列 (依存関係あり) — total 4 merge cycle
- Worktree agent を使う場合は Wave 1 の 2 task を別 worktree で、Wave 2-4 は main に順序 merge

### Rollout 方針

- 各 Wave end で `cargo test` 全域 green 確認 (feedback:feedback_semantic_merge_conflict)
- K-5 land 後に #345 (ESC-2 Notifications) が動き出せる (Lua-bridge-from-start で実装)

---

## §5 conflict / risk zone

### 5.1 `scripting/lifecycle.rs` の dispatch 優先順 (K-3 / K-5)

現行 `dispatch_event_handlers` (L407-484) は `_event_handlers` のみ dispatch、`_knowledge_subscribers` は別経路 (`dispatch_knowledge`) で dispatch する。両者の **優先順**:

- K-5 で `core:*@recorded` が EventSystem 経由 (queue-only) で発火されると、`dispatch_event_handlers` が `_event_handlers` から pull → **ここで `_knowledge_subscribers` も同 event id で呼ぶ必要がある**
- もし `dispatch_event_handlers` が `_knowledge_subscribers` も dispatch するように書けば、knowledge 経路が 2 重に発火しうる (scripting scope 外で直接 `dispatch_knowledge` も走る場合)

**解決**: K-5 で core variant `@recorded` を EventSystem に流す際、**`dispatch_event_handlers` の内部で `_knowledge_subscribers` も scan** する形に unify。knowledge 専用直接 dispatch は `dispatch_knowledge_observed` のみで使う。

これは §3.5 の commit 3 / 4 で touching。K-3 段階ではこの unify は先取りせず、`dispatch_knowledge` はスタンドアロン機能として成立させる。

### 5.2 `scripting/gamestate_scope.rs` (K-2)

`build_gs_table` の `if matches!(mode, GamestateMode::ReadWrite)` block は既に 5 setter (L216-321) を持つ。`record_knowledge` を追加すると scope closure 数が増える → scope crate (mlua 0.11) の制限は恐らくないが、**ビルド時間増**が起きる (`create_function_mut` の generic instantiation)。

K-2 の closure は parse + 2 borrow + dispatch + 1 apply と中程度の複雑さ。既存 setter (L271-294 の set_flag など) と comparable。

**Worktree agent 注意**: 複数 PR が `build_gs_table` を同時修正すると merge conflict 必須。K-2 単独 worktree に保つ。

### 5.3 `knowledge/facts.rs` (K-2 variant 追加、K-4 drain 拡張、K-5 core 絡み)

- K-2: `KnowledgeFact::Scripted` variant 追加、`impl KnowledgeFact` 分岐 (title/description/priority/related_system/event_id) 拡張
- K-4: `PendingFactQueue::drain_ready_scripted` 追加
- K-5: drain 責任移動 (notify_from_knowledge_facts ↔ dispatch_knowledge_observed)

**Worktree merge 順**: K-2 → K-4 → K-5 を **直列 (同 branch 上)** で積む。K-4 merge 時に `drain_ready_scripted` が K-2 の Scripted variant に依存するため。

### 5.4 `notifications.rs` (K-5 の大きな変更)

`notify_from_knowledge_facts` (L290-317) は K-5 で **drain を放棄、post-drain callback で notification を発火**する形に書き換える。これは既存 notification test の多くに回帰懸念があるので regression test 必須 (`tests/notifications_*.rs` が存在するなら全部 green 確認)。

### 5.5 既存 `tests/` の互換

- `tests/fixtures_smoke.rs` — save format stable、K-1 で KindRegistry resource を insert してもは savebag に含まれない (v1 persistence なし) → fixture は影響なし
- 既存 knowledge / notification integration test — K-5 で legacy path 変更時に potentially 壊れる、regression 対策必須

---

## §6 共通 invariant (全 sub で守る)

1. **`apply::*` は `&mut World` のみ**、`&Lua` 不使用 (memory:feedback_rust_no_lua_callback.md)
   - K-2 `apply::record_knowledge` は Lua table を受け取らない → `PayloadSnapshot` (Rust struct) を受け取る
   - K-5 core variant 発火も apply path は Lua table 受けない

2. **subscriber dispatch は queue 化可能な限り queue 化**
   - `@recorded` (Lua-origin, scripted): scope closure 内の同期 dispatch は allowed (world borrow は解放済、`&Lua` は closure の lifetime 内)
   - `@recorded` (Rust-origin, core): `EventSystem::fire_event_with_payload` 経由で次 tick 発火 (queue-only invariant 厳格適用)
   - `@observed`: `dispatch_knowledge_observed` exclusive system で次 tick 発火 (queue-only)

3. **payload mutation の sealed metadata 境界**
   - `seal_immutable_keys(lua, payload, immutable_keys)` で `__newindex` metatable を設定
   - 書き換え試行は `mlua::Error::RuntimeError("immutable key: ...")`
   - mutation 可能な key は `payload` sub-table 配下のみ

4. **subscriber error は warn + 残り chain 継続**
   - `dispatch_knowledge` は `func.call` の `Err` を warn log してから next
   - 全 dispatcher で統一 (deterministic behaviour)

5. **登録順 = dispatch 順**
   - per-kind + wildcard を **統一登録順** で walk (#352 spec 推奨)
   - 数千 subscriber を超える場合は bucket 化を検討、v1 は full scan

---

## §7 v1 で含めない (再確認)

memory:project_scriptable_knowledge.md "v1 で含めない" セクションおよび #349 "v1 で含めない" セクションと一致:

- **subscription cancellation** (`@recorded` subscriber が record 自体を中止する API)
- **subscriber priority / ordering 制御** (登録順以外)
- **prefix wildcard** (`vesk:*@observed`) — regex 化は v2
- **`@expired` 等の追加 lifecycle** (TTL、auto-forget)
- **NPC empire の observer 化** (v1 は player 1 人のみ)
- **knowledge persistence** (#247 save/load 対応、savebag 追加なし)
- **`define_event` 側の subscription pattern 拡張** (統一 EventBus、将来別 epic)

---

## §8 Critical Files for Implementation

| file | 変更内容 | 関連 sub |
|---|---|---|
| `macrocosmo/src/knowledge/mod.rs` | `pub mod kind_registry; pub mod payload;` 追加 | K-1, K-2 |
| `macrocosmo/src/knowledge/kind_registry.rs` | 新設 — KindRegistry, KnowledgeKindDef, PayloadSchema | K-1, K-5 |
| `macrocosmo/src/knowledge/payload.rs` | 新設 — PayloadSnapshot, PayloadValue, snapshot_from_lua / snapshot_to_lua | K-2, K-4 |
| `macrocosmo/src/knowledge/facts.rs` | `KnowledgeFact::Scripted` variant + `drain_ready_scripted` | K-2, K-4 |
| `macrocosmo/src/scripting/knowledge_api.rs` | 新設 — parse_knowledge_definitions, parse_payload_schema | K-1 |
| `macrocosmo/src/scripting/knowledge_dispatch.rs` | 新設 — dispatch_knowledge, deep_copy_payload, seal_immutable_keys, dispatch_knowledge_observed | K-3, K-4 |
| `macrocosmo/src/scripting/knowledge_bridge.rs` | 新設 — core variant lifecycle event fire (K-5) | K-5 |
| `macrocosmo/src/scripting/knowledge_payload.rs` | 新設 or `knowledge/payload.rs` と統合 — Lua ↔ PayloadSnapshot round trip | K-2, K-4 |
| `macrocosmo/src/scripting/globals.rs` | `register_define_fn(lua, "knowledge", "_knowledge_kind_definitions")`; `on_fn` ルーティング拡張; `_knowledge_subscribers` 初期化 | K-1, K-3 |
| `macrocosmo/src/scripting/gamestate_scope.rs` | `record_knowledge` setter (build_gs_table L216-); `apply::record_knowledge` | K-2 |
| `macrocosmo/src/scripting/lifecycle.rs` | `dispatch_event_handlers` の `_knowledge_subscribers` 併せ dispatch (K-5 unify) | K-5 |
| `macrocosmo/src/scripting/mod.rs` | `load_knowledge_kinds` startup system; `dispatch_knowledge_observed` update system | K-1, K-4 |
| `macrocosmo/src/notifications.rs` | K-5 で drain 責任移動 | K-5 |
| `macrocosmo/scripts/knowledge/sample.lua` | fixture | K-1 |
| `macrocosmo/scripts/init.lua` | `require("knowledge.sample")` 等 | K-1 |
| `tests/knowledge_record.rs` | 新規 integration test | K-2 |
| `tests/knowledge_observed.rs` | 新規 integration test | K-4 |
| `tests/knowledge_core_wire.rs` | 新規 integration test | K-5 |

---

## §9 open questions (user 判断要)

### §9.1 `@recorded` dispatch の同期性 (Rust-origin 側)

**推奨: Option A (queue 化、次 tick 発火)**

issue #349 の spec は「`@recorded` は sync 発火」と書いているが、これは *Lua-origin* (`gs:record_knowledge`) のみ自然に実現可能。Rust-origin (`FactSysParam::record` など Rust system) で sync 発火すると `feedback_rust_no_lua_callback` 違反になる。

- **Option A (推奨)**: Rust-origin は `EventSystem::fire_event_with_payload` 経由 → 次 tick 発火 (1 tick latency ≈ 1/60 s)。payload mutation chain は機能するが、最終 payload は **PendingFactQueue に enqueue 済みのものを更新できない** → Rust-origin @recorded は事実上 "observer 通知 hook" となり、queue の中身には影響しない。
- **Option B**: Rust system が `&Lua` を borrow できるよう plumbing 拡張 (`world.resource_scope::<ScriptEngine, _>` を record 経路に持ち込む)。#332 Phase B の `run_lifecycle_hooks` と同じ exclusive system 化。極めて侵襲的 (全 fact-emitter を書き換え)。

**判断要因**: v1 で Rust-origin の `@recorded` subscriber が payload を mutate して queue に反映する use case が必須か? — issue/epic 設計上は「modder 定義 kind (Lua-origin) の enrichment 用途」が主目的なので A で機能的に十分。Rust-origin は observer notification のみで OK。

### §9.2 event id 解析の bullet-proof 性

`rsplit_once('@')` で `"foo:bar@recorded"` → `("foo:bar", "recorded")`、`"*@observed"` → `("*", "observed")` は OK。しかし `"foo@bar@recorded"` のような pathological input で `("foo@bar", "recorded")` と解釈される。ユーザー定義 id は namespace (`:`) で分離されているので `@` は lifecycle separator としてのみ使われる想定で OK だが、**load time validation** で `kind_id` 部分に `@` を含むものを明示的に拒否する:

- `define_knowledge { id = "foo@bar" }` → load error
- `on("foo@bar@recorded", fn)` → 後方互換観点で warn のみにするか error にするか要判断

**推奨: error** (syntax hygiene、早期検出)。

### §9.3 deep-copy の depth_limit

§2.5 で deep copy は depth_limit 8 を想定した。ただし実運用で payload に 8 段 nested table を入れる use case があるか不明。

**推奨: depth_limit 16、exceed で error (ensure forward-safe)**。v2 で制約を緩和するのは簡単、逆は難しい。

### §9.4 subscription registry の data structure (v1)

§2.2 で「full scan (O(N)) で v1 OK」とした。現状 `_event_handlers` も full scan なので precedent に合わせる。ただし将来 knowledge subscriber が大量 (> 1000) 登録される modder cases を懸念するなら、load 完了時点で per-kind / per-lifecycle bucket 化する dispatcher resource を作る方が良い。

**推奨: v1 は full scan、bucketing は v2 で着手**。subscribe 登録数が測れる状態 (modding scene が立ち上がり始めたタイミング) で判断。

### §9.5 `notify_from_knowledge_facts` との drain 競合

§5.4 / §3.5 で議論。K-5 で drain 責任を `dispatch_knowledge_observed` に集約する案を推奨したが、代案として **2 queue 化** (core 用と scripted 用に分離) もあり得る。

**推奨: drain 集約 (§3.5 の approach)**。queue 分離は Scripted/core の統一 API を壊す。ただし K-5 regression の影響範囲が大きいので、PR review で検討余地あり。

### §9.6 `*@<lifecycle>` wildcard 以外の Scripted kind id 形式

epic は `<namespace>:<name>` 形式を要求 (`vesk:famine_outbreak`)。`"famine_outbreak"` (namespace なし) を load error にするか?

**推奨: warn only、v1 は緩く**。後で strict mode を追加可能。ただし `define_knowledge { id = "core:foo" }` は常に error (namespace 衝突)。

### §9.7 per-kind default priority / notification behaviour (v1 範囲外?)

Scripted kind の banner 発火は #345 で扱う予定だが、`priority` 指定は `define_knowledge { priority = "high" }` のような API が欲しくなる。v1 で導入するか?

**推奨: v1 で導入しない**。`define_knowledge` の option table は `id` と `payload_schema` のみに絞る。#345 で `priority` field を後付けする方が安全。

---

## §10 Spike 推奨

### Spike 10.1: `seal_immutable_keys` metatable 挙動

mlua 0.11 で `Table::set_metatable` + `__newindex` を使い、immutable key set に含まれる key への write が `RuntimeError` になることを最小再現 test (`spike_seal_immutable_keys`) で確認。

```rust
#[test]
fn spike_seal_immutable_keys() {
    let lua = Lua::new();
    let t = lua.create_table().unwrap();
    t.set("kind", "foo").unwrap();
    t.set("payload", lua.create_table().unwrap()).unwrap();
    seal_immutable_keys(&lua, &t, &["kind"]).unwrap();
    // writing to "kind" must error
    let r: mlua::Result<()> = lua.load(r#" _t.kind = "bar" "#).exec();
    assert!(r.is_err());
    // writing to payload field must succeed
    lua.load(r#" _t.payload.x = 1 "#).exec().unwrap();
}
```

K-3 commit 4 の foundation として先行 land 推奨。

### Spike 10.2: `rsplit_once('@')` edge cases

`on("malformed", fn)` / `on("@recorded", fn)` / `on("foo@", fn)` の挙動を整理、load time validation を網羅する (§9.2)。

### Spike 10.3: deep-copy function vs UserData

Lua table に `Function` 値が混入した場合の `Table::pairs` の挙動を確認。schema validation で `Function` / `UserData` を拒否する invariant を先に定めておく (v1 scope)。

### Spike 10.4: `create_function_mut` 内で `dispatch_knowledge` 呼び出し → reentrancy

K-2 の `record_knowledge` setter で `world_cell.try_borrow_mut()` を解放して `dispatch_knowledge` を呼ぶ flow (§2.4 step 3-5) が期待通り動くことを spike (`spike_reentrancy_release_before_dispatch`) で検証。subscriber 内で `gs:set_flag(...)` などを呼んでも RuntimeError 起きない。

### Spike 10.5: notification regression on K-5 drain shift

K-5 commit 4 (drain 責任移動) 前に、既存 notification integration test を洗い出し、影響範囲マップを作成。回帰 test のスタック化を先に行う。

---

## §11 集計サマリ

| sub | commit | LoC | wave |
|---|---|---|---|
| K-1 (#350) | 5 | ~560 | Wave 1 (parallel) |
| K-3 (#352) | 4 | ~580 | Wave 1 (parallel) |
| K-2 (#351) | 5 | ~850 | Wave 2 |
| K-4 (#353) | 5 | ~790 | Wave 3 |
| K-5 (#354) | 5 | ~1060 | Wave 4 |
| **合計** | **24** | **~3840 (Rust 3600 + Lua 240)** | 4 wave, 2 parallel + 3 serial |

新規 module: 6 (`knowledge/kind_registry.rs`, `knowledge/payload.rs`, `scripting/knowledge_api.rs`, `scripting/knowledge_dispatch.rs`, `scripting/knowledge_bridge.rs`, `scripting/knowledge_payload.rs`) — `payload.rs` と `knowledge_payload.rs` は統合候補 (§10.3 判断次第)。

既存 module の touch: `knowledge/facts.rs` / `knowledge/mod.rs` / `scripting/globals.rs` / `scripting/gamestate_scope.rs` / `scripting/lifecycle.rs` / `scripting/mod.rs` / `notifications.rs`。

Test 追加 (概算): 40 unit test + 6 integration test = ~46 new test。

---

_End of plan._
