# 勢力 (Faction) システム + 外交設計 — v2

親 epic: **#163** (Faction システム) → **#292** (Sovereignty Phase 2) 上で v2 spec として確定。

v1 (HostilePresence + builtin `DiplomaticAction` enum 時代) は廃止。本 v2 は完全な breaking change で、pre-alpha のため migration layer は設けない。

旧版は `git log -- docs/diplomacy-design.md` から参照。

---

## 0. 旧版からの主要変更

| 旧 (v1) | 新 (v2) |
|---|---|
| `DiplomaticAction` enum (DeclareWar / ProposePeace / ...) | 廃止。全 action は `define_diplomatic_option` Lua 定義 |
| `define_diplomatic_action` (Lua、on_accepted closure) | `define_diplomatic_option` (label + Condition + on_select event dispatch) |
| `requires_diplomacy: bool` 等 faction-type flag | `allowed_diplomatic_options: [option_id]` リストで明示 |
| Builtin DeclareWar | Casus belli 成立で UI ボタン unlock or auto-war |
| Builtin BreakAlliance | unilateral 系の DiplomaticOption (event handler 側で即時適用) |
| HostilePresence component | 全廃 (#293)、faction 関係 + ROE で代替 |
| faction_type が runtime データ参照 | type は galaxy generation の preset (instance に copy) |
| Inbox UI hardcoded | option-driven (option 側 `responses = {{label, event}, ...}` を UI が描画) |
| `Condition` を返す API なし | DiplomaticOption.available / Casus belli.evaluate / end_scenario.available 全て Condition 返り値 |

---

## 1. 中核モデル

外交体系は 4 つの Lua 定義で成立:

| 定義 | 役割 |
|---|---|
| **`define_diplomatic_option`** | UI 上の 1 ボタン = label + 表示条件 (Condition) + click 時の event dispatch |
| **`define_negotiation_item_kind`** | Negotiation バンドルの 1 line item の種別 (resources / tech / system_cession 等)、merge / validate / apply を定義 |
| **`define_casus_belli`** | 開戦の justification + war scope (base/additional demands) + 終戦シナリオ (end_scenarios) を一括所有 |
| **`define_faction_type`** | Galaxy generation 時の preset。`allowed_diplomatic_options` 等の初期値を持つ |

Faction component (runtime) は preset から copy された field を持ち、以降 instance ごとに override 可能。type 参照は spawn 時のみ。

---

## 2. DiplomaticOption

### 役割

Pure dispatcher。runtime セマンティクスは持たず、UI 表示と event 発火だけを担当。
Unilateral / bilateral / multi-step 等の挙動分岐は **event handler** の責務。

### Lua API

```lua
define_diplomatic_option {
    id = "generic_negotiation",
    name = "Negotiate",

    -- 表示可否を Condition で返す。false 時、UI は walk して未充足 atom の理由を tooltip 表示
    available = function(ctx)
        return all {
            target_state_in("peace", "neutral"),
            target_allows_option("generic_negotiation"),
        }
    end,

    -- クリック時に発火する event (event id + payload を Lua から emit)
    on_select = function(ctx)
        emit_event("diplomacy:open_negotiation_modal", {
            actor = ctx.actor, target = ctx.target,
        })
    end,

    -- Inbox に積まれた proposal を target 側が開いた時に並ぶ応答ボタン群
    -- ※ Lua closure を持たず、label と event id のみ (POD で in-flight serialize 可能)
    responses = {
        { id = "accept",  label = "Accept",          event = "diplomacy:negotiation_accept" },
        { id = "reject",  label = "Reject",          event = "diplomacy:negotiation_reject" },
        { id = "counter", label = "Counter Offer…",  event = "diplomacy:negotiation_counter" },
    },
}
```

### 重要な性質

- **Actor 側無制限**: actor の faction が「offer 可能か」のチェックは `available` Condition 内で actor 状態を見て判断 (resource 不足等)。actor type に capability list は持たない
- **Target 側受信可否**: `target_allows_option(id)` atom が target の `allowed_diplomatic_options` を確認
- **Unilateral / bilateral の区別なし**: `on_select` event handler が「proposal を inbox に積む」か「即時適用する」かを決める
  - 例: `break_alliance` option の on_select は `diplomacy:apply_immediate` event を投げ、handler が状態を即変更
  - 例: `generic_negotiation` の on_select は modal を開き、提出時に proposal を inbox に積む

### `responses` の制約

In-flight proposal (Inbox に積まれる serialized object) は POD のみ。Lua closure を持たない。`responses` の `event` は文字列 id で、event handler 側で実際の処理を行う。これにより:
- save / load 時の closure 寿命問題なし
- 光速遅延中の proposal も serialize 可

### 「強制 accept」シナリオ

Surrender や Ultimatum で「target は accept しか選べない」状況は、`responses` に accept のみを含む別 option を define して表現:

```lua
define_diplomatic_option {
    id = "victory_dictation",
    -- ... casus belli の victory end_scenario が emit する modal で使われる
    responses = {
        { id = "accept", label = "Accept Terms", event = "diplomacy:victory_accept" },
    },
}
```

UI 側に `force_accept_only` flag は不要。

---

## 3. NegotiationItemKind

Negotiation modal で組み立てる bundle の 1 行 (line item) の種別。kind 自身が merge / validate / apply を持つ。

### Lua API

```lua
define_negotiation_item_kind {
    id = "resources_pct",
    name = "Resource Indemnity",

    -- 複数 demand source から同 kind が来た時の合算戦略
    -- 引数 items = [{value=0.2, source=cb_a}, {value=0.5, source=cb_b}]
    merge = function(items) return sum_pct(items) end,

    -- give 側の妥当性 (実行可能か)
    validate_give = function(ctx, value) return ctx.giver.resources >= value end,

    -- bundle commit 時の効果
    apply = function(ctx, value) ctx.giver:transfer_resources(ctx.receiver, value) end,
}

define_negotiation_item_kind {
    id = "standing_apology",
    merge = function(items) return items[1] end,  -- 1 回だけ
    validate_give = function(ctx) return true end,
    apply = function(ctx) ctx.receiver:bump_standing_to(ctx.giver, 10) end,
}

define_negotiation_item_kind {
    id = "system_cession",
    merge = function(items) return union_systems(items) end,  -- 個別 system union
    validate_give = function(ctx, systems) return ctx.giver:owns_all(systems) end,
    apply = function(ctx, systems) ctx.giver:cede_systems(ctx.receiver, systems) end,
}
```

### 想定 v1 kinds

| id | merge | give validate | apply |
|---|---|---|---|
| `resources_pct` | sum | giver の stockpile に保有 | one-time 移転 |
| `tech_count` | sum | giver が保有する tech 一覧から | receiver に access 付与 (永続) |
| `system_cession` | union | giver が Core owner | Core ownership 書き換え |
| `standing_apology` | first | (常時 OK) | receiver の giver への standing +10 |
| `peace` | first | War 状態 | War → Peace |
| `alliance` | first | Peace 状態 + standing >= 50 | Peace → Alliance |
| `vassalize` | first | giver の system count <= 2 | giver を vassal flag 付き Faction に |
| `all_remaining_resources` | first | (常時) | giver の全 stockpile を移転 |

---

## 4. Casus Belli

War 全体の orchestrator。1 つの casus belli が以下を一括所有:

1. **Justification** (`evaluate`): いつ casus belli 成立か (Condition 返却)
2. **Auto-war flag** (`auto_war`): 成立瞬間に War 状態へ auto 遷移するか、UI ボタン unlock のみか
3. **Base demands**: war scope 上限 (常時 demand 可能)
4. **Additional demands**: 戦況進展で unlock される demand (旧 `define_war_achievement` を internalize)
5. **End scenarios**: 終戦選択肢 (surrender / white peace / victory dictation 等) — それぞれ available Condition + on_select event

### Lua API

```lua
define_casus_belli {
    id = "broken_treaty",
    name = "Broken Treaty",

    -- 成立条件 (Condition return、UI で未充足理由表示)
    -- 時効は modifier の expires_at に責務委譲、ここでは modifier 存在チェックのみ
    evaluate = function(ctx)
        return all { actor_has_modifier("cb_broken_treaty_recent") }
    end,

    auto_war = false,

    base_demands = {
        { kind = "resources_pct", value = 0.2 },
        { kind = "standing_apology" },
    },

    additional_demands = {
        {
            unlocked_when = function(ctx) return all { actor_holds_capital_of_target() } end,
            items = { { kind = "system_cession", scope = "any_target_owned" } },
        },
        {
            unlocked_when = function(ctx) return all { target_system_count_at_most(1) } end,
            items = { { kind = "vassalize" }, { kind = "all_remaining_resources" } },
        },
    },

    end_scenarios = {
        {
            id = "demand_victory",
            label = "Dictate Victory Terms",
            available = function(ctx)
                return any {
                    actor_holds_capital_of_target(),
                    target_system_count_at_most(2),
                }
            end,
            on_select = function(ctx)
                emit_event("diplomacy:open_negotiation", {
                    from = ctx.actor, to = ctx.target,
                    available_items = ctx:demands(),  -- base ∪ unlocked additional
                    option_id = "victory_dictation",  -- responses が accept のみ
                })
            end,
        },
        {
            id = "white_peace",
            label = "Propose White Peace",
            available = function(ctx) return condition.always_true() end,
            on_select = function(ctx)
                emit_event("diplomacy:open_negotiation", {
                    from = ctx.actor, to = ctx.target,
                    available_items = { "peace" },
                    option_id = "white_peace_negotiation",  -- accept/reject
                })
            end,
        },
        {
            id = "offer_surrender",
            label = "Offer Surrender",
            available = function(ctx) return condition.always_true() end,
            on_select = function(ctx)
                -- 立場逆転: target が demand 組み立て、actor は accept のみ
                emit_event("diplomacy:open_negotiation", {
                    from = ctx.target, to = ctx.actor,
                    available_items = ctx:demands(),
                    option_id = "victory_dictation",
                })
            end,
        },
    },
}
```

### Demand 合算 rule

War 中の negotiation modal で available items を計算する時:
1. 採用 casus belli (1 つ — V 制約) の `base_demands` を取得
2. `additional_demands` の各エントリで `unlocked_when` が true なものの items を append
3. 同 kind が複数あれば `kind.merge(items)` で合算

### 単一 casus belli per war

1 つの war は **正確に 1 つの casus belli** で運用:
- `auto_war = true` の場合: 成立した casus belli が war の orchestrator に固定
- 手動 `auto_war = false` の場合: 複数の casus belli が同時成立していたら **player が宣戦時に明示的に 1 つを選択**
- 後から別の casus belli が成立しても war 中は採用 casus belli 変更不可 (戦争目的の途中変更は混乱の元)

これにより end_scenarios も demands 計算も曖昧さなし。

### 時効

`define_casus_belli` 自体は時効 field を持たない。modifier の `expires_at` を介して表現:

```lua
on_event("diplomacy:treaty_broken", function(ctx)
    ctx.actor:push_modifier("cb_broken_treaty_recent", {
        duration = 100,  -- 100 hexadies で expire
    })
end)
```

`evaluate = function(ctx) return actor_has_modifier("cb_broken_treaty_recent") end` で modifier 期限切れ = casus belli 失効。

既存 `Modifier { expires_at, on_expire_event }` 機構をそのまま利用 (`macrocosmo/src/modifier.rs:44-62`)。Lua 側も既に `duration` field を受ける (`scripting/modifier_api.rs:80`)。

---

## 5. Faction

### Faction component (runtime)

```rust
#[derive(Component)]
pub struct Faction {
    pub id: String,
    pub name: String,
    pub display_color: Color,
    pub allowed_diplomatic_options: HashSet<String>,
    pub flags: HashSet<String>,
    // ... 既存 field
}
```

`allowed_diplomatic_options` は "actor / target 兼用 1 set"。actor は誰でも何でも offer 試みられるが、target 側がこの set に持っていない option は `target_allows_option` Condition で grey-out される。

### faction_type は preset

```lua
define_faction_type {
    id = "trade_federation",
    display_color = ...,
    allowed_diplomatic_options = { "generic_negotiation", "embassy", "non_aggression_pact" },
}

define_faction_type {
    id = "void_horror",
    allowed_diplomatic_options = {},
}
```

galaxy generation 時に preset から Faction component に値を copy。runtime では Faction が source of truth、type 参照は持たない。instance ごとに `faction:set_allowed_options({...})` 等で override 可能 (v1 から)。

### Extinct flag (annihilation)

target faction の system count が 0 になった時、Faction entity は despawn せず以下を付与:

```rust
#[derive(Component)]
pub struct Extinct { pub since: i64 }
```

history / 銀河ログから「滅んだ faction」として参照可能。relations は frozen (新規 diplomatic_option は available false)。

---

## 6. State machine

### RelationState (variant 維持)

```rust
pub enum RelationState { Neutral, Peace, War, Alliance }
```

### 遷移表

| 遷移 | trigger |
|---|---|
| `Neutral → Peace` | generic_negotiation で `peace` item を含む bundle 成立 |
| `Peace → Alliance` | generic_negotiation で `alliance` item を含む bundle 成立 |
| `Alliance → Peace` | `break_alliance` DiplomaticOption (unilateral、event handler が即時遷移) |
| `*  → War` | (a) auto_war casus belli 成立で system 自動、(b) UI で casus belli 選択 + Declare War ボタン押下 |
| `War → Peace` | end_scenarios の `white_peace` / `demand_victory` / `offer_surrender` 経由で peace item 含む bundle commit |

`DeclareWar` / `BreakAlliance` は DiplomaticOption の specific instance として扱う (前者は casus belli 経由、後者は普通の unilateral option)。enum 化はしない。

### 光速遅延

既存 `PendingDiplomaticAction` 機構を流用、payload を generic な `DiplomaticEvent { from, to, option_id, payload, arrives_at }` に置き換え。
- generic_negotiation の bundle commit も同じ pipeline
- end_scenario の event 発火も同じ pipeline (but ローカル発火、modal 表示後の bundle 提案で初めて遅延発生)

---

## 7. UI 構成

### Diplomacy panel (per other faction)

- header: 相手 faction 名、type、relation state、standing (color bar)、freshness banner ("as of N hd ago")
- body: `define_diplomatic_option` 全件を walk、`available` Condition true なら enable、false なら grey + tooltip で未充足理由
  - `target_allows_option` 由来の disable は「This faction does not engage in: 〜」表示
- War 中: 「End War…」ボタンが追加表示、押下で active casus belli の `end_scenarios` 一覧 modal を開く

### Negotiation modal (generic_negotiation 用)

- 左 column: 自分の give 側 line items (kind ごとに add ボタン、validate 失敗で赤マーク)
- 右 column: 相手の request 側 line items
- 下: Send button (両 column 全 item validate OK で enable)
- atomic preview: bundle 全体の effect summary

### Inbox

- 受信した proposal 一覧、各 item に: `from` / `option_id` / `payload preview` / `arrived_at` / `responses` ボタン群
- responses は option 側 `responses` 配列を walk して描画、押下で対応 event 発火
- counter response の event handler は modal 再 open 等を行う

### End-of-War scenario picker

- War 状態の Diplomacy panel で「End War…」押下時の modal
- 採用 casus belli の `end_scenarios` を walk、`available` true で enable
- 各 scenario の label + (false 時) 未充足理由
- 押下で `on_select` event 発火 → handler が negotiation modal を open

### Casus Belli viewer

- War 宣戦前: 相手 faction に対して active な casus belli 一覧 (evaluate true なもの)
- 「Declare War」ボタン: 採用する 1 casus belli を select、auto_war=false のもののみ列挙
- War 中: 採用 casus belli + 進捗 (additional_demands の unlock 状況) を表示

---

## 8. Condition system 拡張

`condition.rs` の `Condition` tree をそのまま活用、新規 atom kinds を追加:

| atom | scope | 用途 |
|---|---|---|
| `target_state_is(state)` | diplomacy | RelationState チェック |
| `target_state_in(states...)` | diplomacy | 複数 state OR |
| `target_standing_at_least(n)` | diplomacy | standing 比較 |
| `relative_power_at_least(ratio)` | diplomacy | 軍事比較 |
| `target_allows_option(id)` | diplomacy | 受信可否 |
| `actor_has_modifier(id)` | diplomacy | 時効 modifier 確認 (casus belli 用) |
| `actor_holds_capital_of_target()` | diplomacy | war achievement |
| `target_system_count_at_most(n)` | diplomacy | annihilation 進捗 |
| `target_attacked_actor_core_within(hexadies)` | diplomacy | core_attacked casus belli |

これら atom は Rust hardcoded で十分 (item kind と異なり、modding 拡張余地が低い)。`define_condition_atom` Lua API は不要 (v2 でも)。

UI 側の Condition walker:
- `Condition::All` で false → 失敗した子のみ表示
- `Condition::Any` で false → 全ての子を「いずれか必要」として表示
- 末端 atom → atom の `display_message()` を呼ぶ (atom 側に reason 文字列を持たせる)

---

## 9. Migration plan (v1 → v2、breaking change)

pre-alpha のため migration layer は設けない。一気に置換。

### Rust 側

- `pub enum DiplomaticAction { DeclareWar, ProposePeace, ... CustomAction }` 廃止
- `define_diplomatic_action` Lua API 廃止 (関連 `DiplomaticActionRegistry` も)
- `tick_custom_diplomatic_actions` 廃止
- `PendingDiplomaticAction` の payload を `DiplomaticEvent { from, to, option_id, bundle, arrives_at }` に置換
- 新規:
  - `DiplomaticOptionRegistry` (`define_diplomatic_option` 受け皿)
  - `NegotiationItemKindRegistry`
  - `CasusBelliRegistry`
  - `Condition` atom kinds 拡張
  - `Faction.allowed_diplomatic_options: HashSet<String>`
  - `Extinct` component

### Lua 側

- `scripts/factions/actions.lua` (v1 の `trade_agreement` / `cultural_exchange`) を新 `define_diplomatic_option` 形式に rewrite、modifier push は event handler に移動
- builtin だった `DeclareWar` / `BreakAlliance` 等も全 Lua 定義に降ろす (`scripts/factions/options.lua` 新規)
- `define_faction_type` 既存定義に `allowed_diplomatic_options` field を追加

### UI 側

- `Diplomacy panel` を新モデル (option-driven) で書き直し
- `Inbox` を responses-driven に
- `Casus Belli viewer` 新規追加
- `Declare War` flow 改修 (casus belli 選択 → 確認)

### Test

- 既存 `tests/faction*.rs` の DiplomaticAction enum テスト群を全廃 → 新 DiplomaticOption / Casus belli テスト群に置換
- Save/load: 新 `DiplomaticEvent` payload roundtrip test

---

## 10. 関連 issue (改訂 + 新規)

### 改訂

- **#302** (S-8) → 「DiplomaticOption + faction.allowed_diplomatic_options + Inbox dispatch (label+event id)」
- **#304** (S-9) → 「UI: Diplomacy panel + Negotiation modal + Inbox + End-of-War scenario picker + Casus Belli viewer」
- **#305** (S-11) → 「`define_casus_belli` + base/additional_demands + end_scenarios + auto_war + single-cb-per-war 制約」

### 新規起票候補

- `define_negotiation_item_kind` Lua API + merge/validate/apply
- Condition atom 拡張 (diplomacy 関連 9 種)
- faction_type → instance preset 化 refactor
- Annihilation handling (Extinct component + history + relation freeze)
- 既存 `DiplomaticAction` enum + `define_diplomatic_action` 廃止 migration

---

## 11. v2 スコープ外 (将来検討)

- Counter offer の revision 管理 (v1 では「reject + 新規 propose」運用、無理に状態管理しない)
- Ongoing trade (毎 tick の resource flow)
- Vassal / suzerainty の詳細メカニクス (item kind `vassalize` の apply 内で実装する Lua handler 範囲を超える時)
- War weariness (modifier ベースで Lua 側で組む方針、基盤追加なし)
- `define_condition_atom` Lua-defined atom (現状不要)
- faction-instance 上書きの UI (Lua からは v1 から可能、UI 経由 override は v2 以降)

---

## 旧版アーカイブ

旧 v1 (HostilePresence 時代) の依存グラフ #167-#174 は全て v2 で再設計対象。closed 済 issue (#165 / #167 / #168 / #170 / #169 / #172) の実装は base infrastructure として残るが、API surface は v2 で全面置換。
