# AI 3 層アーキテクチャ — macrocosmo-ai 抽象設計

Status: **Approved (2026-04-25)** — 実装着手可。

## 背景

- `macrocosmo-ai` は engine-agnostic な AI core crate (bevy / macrocosmo / mlua 非依存)。
- 既存 primitives: `AiBus` (metric/command/evidence topics), `Objective`, `Assessment`,
  `Campaign` state machine, `Condition`, `ValueExpr`, `Projection`, `Standing`,
  `Playthrough` + `Scenario` harness。
- 足りないもの: 3 層 (長期 / 中期 / 短期) を「走らせる」agent 抽象、Intent パケット、
  勝利条件、3 層を繋ぐ Orchestrator、抽象シナリオでの整合性検証。
- 関連 issue: #189 (umbrella, closed), #195 (foundation, closed), memory
  `project_ai_three_layer_design.md`。

## スコープ

この設計は **macrocosmo-ai crate 内に閉じる**。ゲーム側 (macrocosmo) は本層を
後から利用する。抽象シナリオ (metric を自由に drive するだけ) で 3 層ループの
整合性を tune してから、ゲーム統合に進む。

**非スコープ:**
- Nash 連合分析の完全実装 (trait 分離して後付け)
- 複雑な勝利条件 (Conquest / Tech / Composite) — 実ゲームで検証
- macrocosmo 側の Rust 実装 (SimpleNpcPolicy 等の差し替え)

## 3 層モデル

### 長期 (Long) — faction の戦略判断

- **入力**: `VictoryCondition`, `Assessment` (from bus), metrics
- **出力**: `Intent` (中期向け意図)
- **頻度**: 低。default 30 tick 毎。緊急イベント (勝利条件 progress の急変等) で
  immediate recompute
- **state**: `GrandPlanState { active_objectives, recent_intents, last_victory_status }`

### 中期 (Mid) — 計画の具体化

- **入力**: Intent inbox (光速遅延 = `arrives_at` を過ぎた Intent のみ), metrics, 自身の
  Campaign state
- **出力**: `CampaignOp` (Start / Transition / AttachIntent), `OverrideEntry` ログ
- **頻度**: 中。default 5 tick 毎。Intent 到着でも起動。
- **state**: `MidState { active_campaigns, inbox, override_log }`

### 短期 (Short) — 実行

- **入力**: active Campaign 群, metrics, **context** (どの fleet / colony のための
  instance か)
- **出力**: `Command` 群 (bus に emit)
- **頻度**: 高 (毎 tick)
- **state**: ほぼ持たない (reactive)

## 層の所在 (spatial distribution)

各層が faction 内で**どこにいるか**は game 統合で重要になる (Intent 配送遅延を
正しくモデル化するため)。

| 層 | 所在 | faction あたり数 | 備考 |
|---|---|---|---|
| Long | Ruler 位置 (= プレイヤーの命令中枢と同じ) | 1 | 戦略判断の集約点 |
| Mid (Governor) | 管轄 region の command center (= Core 持ち首府 / セクター拠点) | N (region 数) | `#189` 「プレイヤー遠隔地委任」と同じ実装を共有 |
| Short (FleetShort) | Fleet / 旗艦 | 多数 (fleet ごと) | 戦闘・移動・ROE |
| Short (ColonyShort) | Colony (colonized system) | 多数 (colony ごと) | 生産・建造キュー・資源配分 |

### abstract scenario での degenerate

- faction あたり Long 1 / Mid 1 / Short 1 (`context = "faction"`)
- `IntentSpec.target = "faction"` 固定
- `FixedDelayDispatcher` が target を無視して scalar delay 返す
- これで 3 層ループが閉じる — macrocosmo-ai 単体で完結

### game 統合時

- macrocosmo 側に `AgentRegistry` 的なものを置き、`IntentTargetRef` →
  実際の Mid / Short instance の解決を担う
- Dispatcher は target の物理位置を AgentRegistry 経由で引いて delay 計算
- Mid は region ごとに独立 tick、`Orchestrator` は multi-Mid を束ねる
  cluster へ拡張 (下記参照)

## 型定義

### `intent.rs`

Long-term agent は位置情報や配送資源を知らないため、**`IntentSpec` (未ルーティング)
を emit** する。Orchestrator が `IntentDispatcher` 経由で **`Intent` (materialize 済み)
に昇格** させる:

```rust
use std::sync::Arc;
use ahash::AHashMap;
use crate::ids::{IntentId, IntentKindId, ObjectiveId, MetricId};
use crate::value_expr::ValueExpr;
use crate::time::Tick;

/// Long が emit する未ルーティング Intent。`arrives_at` / `id` / `issued_at` は
/// この段階では決定不能 (位置・配送資源を知らないため)。
pub struct IntentSpec {
    pub kind: IntentKindId,                   // Arc<str>, open-kind
    pub params: IntentParams,                 // AHashMap<Arc<str>, ValueExpr>
    pub priority: f32,                        // 時間割引される急ぎ度 [0, 1]
    pub importance: f32,                      // 陳腐化しない重要度 [0, 1]
    pub half_life: Option<Tick>,
    /// `issued_at` からの相対オフセット。Dispatcher が absolute 化
    pub expires_at_offset: Option<Tick>,
    pub rationale: RationaleSnapshot,
    pub supersedes: Option<IntentId>,
    /// 宛先アドレス (open-kind)。"faction" / "sector:alpha" / "fleet:42" など
    pub target: IntentTargetRef,
    /// 配送希望 (open-kind)。dispatcher が参考にする。"urgent"/"routine" など。
    /// None なら dispatcher 裁量
    pub delivery_hint: Option<DeliveryHintId>,
}

/// Orchestrator が IntentDispatcher 経由で完成させた Intent。
/// Mid はこの形で inbox から受信する
pub struct Intent {
    pub id: IntentId,                         // Orchestrator が mint
    pub spec: IntentSpec,
    pub issued_at: Tick,                      // Orchestrator が stamp
    pub arrives_at: Tick,                     // dispatcher が計算
    pub expires_at: Option<Tick>,             // spec.expires_at_offset を absolute 化
}

pub struct IntentParams(pub AHashMap<Arc<str>, ValueExpr>);
pub struct IntentTargetRef(pub Arc<str>);
pub struct DeliveryHintId(pub Arc<str>);

pub struct RationaleSnapshot {
    pub metrics_seen: AHashMap<MetricId, f64>,
    pub objective_id: Option<ObjectiveId>,
    pub note: Arc<str>,
}

impl Intent {
    /// 発行からの経過時間で指数減衰させた実効優先度
    pub fn effective_priority(&self, now: Tick) -> f32 { /* ... */ }

    pub fn is_expired(&self, now: Tick) -> bool { /* ... */ }
    pub fn has_arrived(&self, now: Tick) -> bool { now >= self.arrives_at }
}
```

### `dispatcher.rs`

**配送 mechanism 選択は game-logic 判断**で、単純な `(from, to) → delay` ではない
(courier 建造・FTL/relay/光速 signal 選択・資源消費を動的に評価)。
`IntentDispatcher` trait 経由で game 側に委ねる:

```rust
pub trait IntentDispatcher {
    fn dispatch(
        &mut self,
        spec: IntentSpec,
        issued_at: Tick,
        from: FactionId,
    ) -> DispatchResult;
}

pub enum DispatchResult {
    /// 完全 materialize、Orchestrator が intent_queue に入れる
    Sent(Intent),
    /// 今 tick は送れない (通信封鎖・relay 全断等)。Orchestrator が pending に保持
    /// し次 tick 再試行。通常の処理では発生しない稀ケース
    Deferred,
    /// 送信不可能 (target 消滅等)。Orchestrator の drop_log に記録
    Dropped { reason: Arc<str> },
}

/// macrocosmo-ai 側のデフォルト: scalar 固定遅延、常に Sent。
/// 抽象 scenario はこれで閉じる
pub struct FixedDelayDispatcher {
    pub delay: Tick,
}
```

**dispatcher の賢さ境界は game-side impl の自由**:
- FTL courier / relay / 光速 signal を動的比較して最速選択
- courier 不在時に「建造 → 配送」のほうが速ければ build command を side-effect
  として emit (bus 経由) して Sent を返すことも可能
- 資源制約・通信路状態・relay 中継健在性を内部で評価
- dispatcher 内部で任意の planning を行って良い

trait は single-call で open-ended。将来 `available_options` / `commit` への
分離が必要になったら evolve 可能だが現時点では不要。

### `victory.rs`

```rust
use crate::condition::Condition;
use crate::eval::EvalContext;
use crate::bus::AiBus;
use crate::ids::FactionId;
use crate::value_expr::ValueExpr;
use crate::time::Tick;

pub struct VictoryCondition {
    /// **内部条件** (faction 自身の state)。これが True で勝利
    pub win: Condition,
    /// **外部条件** (環境 / 世界状態)。False で `Unreachable` (= 道が絶たれた)
    /// Long-term agent は win に加えて prerequisites も pursuit target として扱う
    /// (= 違反寸前なら steering Intent を emit)
    pub prerequisites: Condition,
    pub time_limit: Option<Tick>,        // 超過で `TimedOut`
    pub score_hint: Option<ValueExpr>,   // UI / tuning 用 [0, 1]
}

pub enum VictoryStatus {
    Won,
    Unreachable,
    TimedOut,
    Ongoing { progress: f32 },
}

impl VictoryCondition {
    pub fn evaluate(
        &self,
        bus: &AiBus,
        ctx: &EvalContext,
        now: Tick,
    ) -> VictoryStatus { /* ... */ }
}
```

**敗北判定 (`lose`) は意図的に含めない**。victory progress の下降と
`prerequisites` 違反で十分検知可能。ゲーム終了判定そのものはゲーム側の責務。

### `agent.rs` — 3 trait + 入出力

```rust
use crate::bus::AiBus;
use crate::campaign::{Campaign, CampaignState};
use crate::command::Command;
use crate::ids::{FactionId, IntentId, ObjectiveId};
use crate::intent::Intent;
use crate::victory::VictoryCondition;
use crate::ai_params::AiParams;
use crate::time::Tick;

// --- Long ---

pub trait LongTermAgent: Send + Sync {
    fn tick(&mut self, input: LongTermInput<'_>) -> LongTermOutput;
}

pub struct LongTermInput<'a> {
    pub bus: &'a AiBus,
    pub faction: FactionId,
    pub victory: &'a VictoryCondition,
    pub victory_status: VictoryStatus,
    pub active_campaigns: &'a [&'a Campaign],
    pub now: Tick,
    pub params: &'a AiParams,
}

pub struct LongTermOutput {
    pub intents: Vec<IntentSpec>,        // 未ルーティング (Orchestrator が dispatcher 経由で Intent 化)
}

// --- Mid ---

pub trait MidTermAgent: Send + Sync {
    fn tick(&mut self, input: MidTermInput<'_>) -> MidTermOutput;
}

pub struct MidTermInput<'a> {
    pub bus: &'a AiBus,
    pub faction: FactionId,
    pub inbox: &'a [Intent],             // arrived intents
    pub campaigns: &'a [Campaign],
    pub now: Tick,
    pub params: &'a AiParams,
}

pub struct MidTermOutput {
    pub campaign_ops: Vec<CampaignOp>,
    pub override_log: Vec<OverrideEntry>,
}

pub enum CampaignOp {
    Start {
        objective_id: ObjectiveId,
        source_intent: Option<IntentId>,
        at: Tick,
    },
    Transition {
        campaign_id: ObjectiveId,
        to: CampaignState,
        at: Tick,
    },
    AttachIntent {
        campaign_id: ObjectiveId,
        intent_id: IntentId,
    },
}

pub struct OverrideEntry {
    pub intent_id: IntentId,
    pub reason: OverrideReason,
    pub at: Tick,
}

pub enum OverrideReason {
    StaleIntent,
    ConflictsWithLocalObservation,
    Superseded,
}

// --- Short ---

pub trait ShortTermAgent: Send + Sync {
    fn tick(&mut self, input: ShortTermInput<'_>) -> ShortTermOutput;
}

pub struct ShortTermInput<'a> {
    pub bus: &'a AiBus,
    pub faction: FactionId,
    /// 実行コンテキスト (open-kind)。"fleet:42" / "colony:sol" / "faction" 等。
    /// Short は同じ faction 内に複数 instance (FleetShort / ColonyShort) が並走しうる
    pub context: ShortContext,
    pub active_campaigns: &'a [&'a Campaign],
    pub now: Tick,
}

pub struct ShortContext(pub Arc<str>);

pub struct ShortTermOutput {
    pub commands: Vec<Command>,
}
```

### `campaign.rs` 拡張

既存 `Campaign` 構造に `source_intent` を追加:

```rust
pub struct Campaign {
    pub id: ObjectiveId,
    pub state: CampaignState,
    pub started_at: Tick,
    pub last_transition: Tick,
    pub source_intent: Option<IntentId>,   // NEW: Intent 由来なら紐付け
}
```

### `orchestrator.rs`

```rust
pub struct Orchestrator<L, M, S> {
    long: L,
    mid: M,
    short: S,
    config: OrchestratorConfig,
    state: OrchestratorState,
    next_intent_id: u64,
}

pub struct OrchestratorConfig {
    pub long_cadence: Tick,           // default 30
    pub mid_cadence: Tick,            // default 5
}

pub struct OrchestratorState {
    pub last_long_tick: Tick,
    pub last_mid_tick: Tick,
    /// dispatcher が Sent を返した未到着 Intent (arrives_at で inbox へ)
    pub intent_queue: Vec<Intent>,
    /// arrives_at を過ぎた Intent
    pub inbox: Vec<Intent>,
    /// dispatcher が Deferred を返した未発送 spec (次 tick で再試行)
    pub pending_specs: Vec<PendingSpec>,
    pub campaigns: Vec<Campaign>,
    pub override_log: Vec<OverrideEntry>,
    pub drop_log: Vec<DropEntry>,
}

pub struct PendingSpec {
    pub spec: IntentSpec,
    pub deferred_since: Tick,
}

pub struct DropEntry {
    pub spec_kind: IntentKindId,
    pub target: IntentTargetRef,
    pub reason: Arc<str>,
    pub at: Tick,
}

impl<L: LongTermAgent, M: MidTermAgent, S: ShortTermAgent> Orchestrator<L, M, S> {
    /// dispatcher は毎 tick 外部から注入 (game 側は mutable world を持つ impl を
    /// 渡す、scenario は `FixedDelayDispatcher` を渡す)
    pub fn tick<D: IntentDispatcher>(
        &mut self,
        bus: &mut AiBus,
        dispatcher: &mut D,
        faction: FactionId,
        victory: &VictoryCondition,
        now: Tick,
    ) -> OrchestratorOutput;
}

pub struct OrchestratorOutput {
    pub long_fired: bool,
    pub mid_fired: bool,
    pub commands: Vec<Command>,
    pub victory_status: VictoryStatus,
}
```

**tick 処理フロー** (1 tick あたり):
1. `pending_specs` を dispatcher に再提出、Sent → `intent_queue`、Deferred → 維持、Dropped → `drop_log`
2. Victory status を evaluate (`Won`/`Unreachable`/`TimedOut` 検出)
3. Long cadence / 緊急条件で Long tick → `IntentSpec` 群 を dispatcher に渡す:
   - `Sent(Intent)` → `intent_queue`
   - `Deferred` → `pending_specs`
   - `Dropped` → `drop_log`
4. `intent_queue` から `arrives_at <= now` の Intent を `inbox` に移動
   (expired は drop)。**Long の後に置くことで zero-delay 配送を同 tick で
   mid に届ける**
5. Mid cadence / Intent 到着で Mid tick → `campaign_ops` を適用、`override_log` 追記
6. 毎 tick Short tick → `commands` を bus に emit

## デフォルト実装

### `ObjectiveDrivenLongTerm`

**win / prerequisites の両方を pursuit target として扱う**:

1. `victory.win` Condition を traverse → 不足メトリクスに対して**推進 Intent**
   (priority=high, importance=high)
2. `victory.prerequisites` Condition を traverse → 違反寸前 or 未達メトリクスに
   対して**保全/steering Intent** (priority=medium, importance=high — 崩れると
   Unreachable のため)
3. 各 Intent は `IntentSpec` 形式で emit (kind=`"pursue_metric"` or
   `"preserve_metric"` 等 open-kind、params に `target_metric` / `direction` / `threshold`)
4. 既存 Intent が同じ目的で pending なら `supersedes` で置換
5. effective_priority による Mid 側の競合解決に委ねる

例: 「危機イベントをエンディング A で完遂」ケース
- `prerequisites = Atom(MetricEquals { metric: "crisis_ending", value: "A" })`
- 危機中に Long が `crisis_ending` を観測 → steering Intent
  (`kind="steer_crisis"`, `params={target_ending: "A"}`) を emit

### `IntentDrivenMidTerm`

- inbox の Intent を `effective_priority` で降順ソート
- トップ N の Intent を現 Campaign 状態と照合:
  - 対応 Campaign が無い → `CampaignOp::Start` + `AttachIntent`
  - 既存 Campaign 継続 → 何もしない
  - 矛盾する Campaign あり (別 Intent 由来) → 優先度比較、劣位を `Suspended` / `Abandoned`
- Intent が stale (`effective_priority < threshold`) → override_log に
  `StaleIntent` で記録、無視
- 現場観測 (bus metric) と矛盾 → `ConflictsWithLocalObservation` で記録、
  Intent を override して local Campaign 継続

### `CampaignReactiveShort`

- 各 active Campaign の `source_intent` を見て `params` から目標値取得
- metric の現状値と目標値の差分で command を emit
- active Campaign 無し → `Command` 空リスト (= no-op)

## シナリオ harness 拡張

既存 `playthrough::scenario::Scenario` の上に層を作る:

```rust
pub struct AgentScenario {
    pub base: Scenario,                            // 既存 (metric scripts + evidence pulses)
    pub factions: Vec<FactionAgentSpec>,
}

pub struct FactionAgentSpec {
    pub faction: FactionId,
    pub victory: VictoryCondition,
    pub long: Box<dyn LongTermAgent>,
    pub mid: Box<dyn MidTermAgent>,
    pub short: Box<dyn ShortTermAgent>,
    pub dispatcher: Box<dyn IntentDispatcher>,   // scenario 内で shared 可、default は FixedDelayDispatcher
    pub orchestrator_config: OrchestratorConfig,
}

pub fn run_agent_scenario(s: AgentScenario) -> AgentPlaythrough;

pub struct AgentPlaythrough {
    pub base: Playthrough,                    // bus events
    pub per_faction: Vec<FactionTrace>,
}

pub struct FactionTrace {
    pub faction: FactionId,
    pub intent_history: Vec<Intent>,
    pub campaign_history: Vec<CampaignSnapshot>,
    pub command_history: Vec<(Tick, Command)>,
    pub victory_timeline: Vec<(Tick, VictoryStatus)>,
    pub override_log: Vec<OverrideEntry>,
    pub drop_log: Vec<DropEntry>,
}
```

## Mid → Short 間の通信

**MVP (abstract scenario)**: Short は Campaign を**直接読む** (遅延なし)。Mid と Short
が同 Orchestrator 内にあるため physical 距離は 0。整合性テストは Long→Mid の
遅延だけで動く。

**game 統合段階**: Mid と Short は物理的に離れうる (Mid は region 首府、Short は
移動中 fleet 等)。従って:
- Mid は Intent を emit (`target = "fleet:42"` 等)
- 同じ `IntentDispatcher` trait を**再利用**して Mid→Short 遅延も配送
- Short は inbox から Intent を読む (Campaign 直読は廃止 or フォールバック)
- dispatcher 実装は Long→Mid / Mid→Short 両方を同じ logic で扱う
  (courier/relay/光速 signal の選択、資源消費)

→ **AI 単体 MVP では省略、game 統合段階では必須**。整合性確認したいケースが
出たら MVP でも Mid→Short Intent 化を試せる (同じ trait 再利用で追加コスト小)。

## Multi-agent 拡張 (game 統合時)

MVP の Orchestrator は 1 faction = 1 Long / 1 Mid / 1 Short (degenerate 構成)。
Game 統合では以下に拡張:

- `OrchestratorCluster` 的な束ね方で Mid を region 数ぶん持つ
- 各 Mid は独立 tick、`IntentTargetRef` で route 分配
- Short も context ごと (FleetShort per fleet, ColonyShort per colony)
- 同じ `ShortTermAgent` trait で種類の異なる Short を同居

**macrocosmo-ai 側の責務**: trait / 型 / 単一 Orchestrator の提供。
**macrocosmo 側**: cluster 的な束ね + `AgentRegistry` + game-specific dispatcher。

## 初期抽象シナリオ

### `scenario_economic_growth` (single faction)

**目的**: 平穏な経済成長ルート。3 層が噛み合って安定に動くことを確認。

- faction 1 つ (`me`)
- `VictoryCondition`:
  - `win`: `economic_capacity > 100.0`
  - `prerequisites`: `stockpile_months > 0.0`
  - `time_limit`: `Some(500)`
- Dynamics:
  - `net_production_minerals`: Linear 10 → 120 over 500 ticks
  - `stockpile_months`: Constant 5.0
  - `economic_capacity`: Linear 0 → 150 over 500 ticks (scenario が直接 emit)
- 期待挙動:
  - Long (tick 0, 30, 60, ...): `pursue_objective(growth)` Intent 発行継続
  - Mid (tick 5, 10, ...): `expand_economy` Campaign Active
  - Short (毎 tick): `grow_economy` command を emit
  - victory_timeline: `Ongoing` → tick ~450 付近で `Won`
  - override_log: 空 (矛盾なし)

### `scenario_survival_under_threat` (faction + rival)

**目的**: 脅威の波に応じて Intent が切替わり、Mid が Campaign を swap、Short
の command が追従することを確認。`override_log` に Intent の切替履歴が残る。

- 2 factions (`me`, `rival`)
- `VictoryCondition` (me):
  - `win`: `now > 500` (= 500 tick 生存)
  - `prerequisites`: `my_strength > 0`
  - `time_limit`: `Some(600)`
- Dynamics:
  - `my_strength`: Constant 30.0
  - `foreign.my_strength.faction_rival`: Sinusoid mean=50 amplitude=40 period=100
    (tick 25, 75, 125... で脅威ピーク)
- 期待挙動:
  - Long: 脅威ピーク付近で `fortify` Intent、低時に `maintain` Intent
    (supersedes chain)
  - Mid: Intent 切替に応じて `defensive_posture` / `idle` Campaign を swap
  - Short: `defensive_posture` active 中は防御 command、idle 中は no-op
  - victory_timeline: tick 500 で `Won`
  - override_log: Intent 切替の理由が記録される

## 整合性テスト

### `three_layer_consistency::vertical_consistency`

- Active Campaign が無い状態 → Short の output commands が空
- Intent が無い状態 → Mid は新 Campaign を生まない (既存継続のみ)
- Campaign が Failed/Abandoned になったら Short は当該 Campaign 向け command を
  出さない

### `three_layer_consistency::temporal_consistency`

- Long が Intent A から Intent B に切替 (supersedes) → 数 tick 内に Mid が追従
- 同一条件下で Long が目標を flip-flop しない (hysteresis + half_life の確認)

### `three_layer_consistency::informational_consistency`

- `delivery_delay_ticks=20` で Intent 発行 → tick 0-19 は Mid inbox に入らない
- priority 時間割引が期待通り (half_life=10 で elapsed=10 時 priority が 0.5 倍)
- 2 Intent 競合時に `effective_priority(now)` 順で選択

## 実装スライス順

各ステップで `cargo test -p macrocosmo-ai` が green を保つ:

### Slice 1: primitives
1. **`intent.rs`** — `IntentSpec` + `Intent` + `IntentTargetRef` + `DeliveryHintId`
   + `RationaleSnapshot` + `effective_priority`。unit test
2. **`victory.rs`** — `VictoryCondition` + `VictoryStatus` + `evaluate`。unit test
3. **`dispatcher.rs`** — `IntentDispatcher` trait + `DispatchResult` +
   `FixedDelayDispatcher`。unit test

### Slice 2: agent + orchestrator
4. **`agent.rs`** — 3 trait + 入出力 + `CampaignOp` + `OverrideEntry` +
   `ShortContext`
5. **`campaign.rs` 拡張** — `source_intent` 追加 (既存 API 互換 = `None` default)
6. **`orchestrator.rs`** — `Orchestrator` + `intent_queue` / `pending_specs` /
   `drop_log` + tick 処理フロー。stub agent で integration test

### Slice 3: default impls + playthrough
7. **default impls** — `long_term_default.rs` (win + prerequisites traversal) /
   `mid_term_default.rs` (effective_priority sort + override logic) /
   `short_term_default.rs` (campaign-reactive)
8. **playthrough 拡張** — `AgentScenario` + `run_agent_scenario` +
   `FactionTrace` + `DropEntry` history (playthrough/agent_scenario.rs)

### Slice 4: scenarios + consistency
9. **tests/scenarios**:
   - `scenario_economic_growth.rs`
   - `scenario_survival_under_threat.rs`
   - `three_layer_consistency.rs` (vertical / temporal / informational)

期待コード量: 新規 2000-2500 LoC、既存変更は `campaign.rs` + `lib.rs` re-export のみ。

## Open items (次セッション以降)

- **Nash trait の形状** — LongTermAgent の差し替え先として `StrategyChooser`
  trait を分離するか
- **`override_log` の詳細 schema** — デバッグ UI / playthrough 可視化で必要な
  情報
- **`AiParams` 拡張** — 3 層 cadence / delivery_delay / half_life デフォルト
- **Playthrough 記録の拡張** — 3 層 state snapshot を `BusSnapshot` と並置
- **macrocosmo 統合** — 既存 `SimpleNpcPolicy` を `ShortTermAgent` 実装に寄せる、
  `GrandPlan` を `LongTermAgent` 実装に寄せる、など
