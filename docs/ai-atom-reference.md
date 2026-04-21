# AI Atom Reference (Bus Architecture Edition)

本ドキュメントは **AI Atom 語彙リファレンス** (#198) を、#195 で確定した
`macrocosmo-ai` **bus architecture** に沿って再定義したカタログである。

関連:
- #189 AI umbrella
- #192 Precondition + ValueExpr (atom 設計)
- #195 ai_core bus アーキテクチャ
- #203 AI integration layer (`macrocosmo::ai`)
- #204 FleetCombatCapability (初 capability — 本ドキュメントの軍事系 metric の emit 元)

## Overview

### Bus architecture — 用語の再定義

#198 の元の issue body は、`MyStrength` / `FactionStrength` / `NetProduction` / `Stockpile` などを **`ValueExpr` の enum variant** として列挙する設計だった。しかし #195 で bus architecture が採用された結果、これらは以下のように再解釈される:

- **Metric ID**: ゲームエンジン側が `AiBus::declare_metric` で宣言し、`AiBus::emit` で値を流し込む時系列 topic の **文字列 ID**。`my_strength` / `net_production_minerals` などの名前付きチャネル。
- **Atom**: `ai_core` 内の汎用評価器 (`ValueExpr::Metric(MetricRef)`, `ValueExpr::DelT`, `ConditionAtom::Compare`, `ConditionAtom::EvidenceCountExceeds` 等)。これらは **enum variant の追加を必要としない**。
- **Command kind ID**: AI が発行するコマンドの種類 (`attack_target` / `colonize_system` / `declare_war` …)。
- **Evidence kind ID**: 派閥間の観測イベントの種類 (`direct_attack` / `gift_given` / `major_military_buildup` …)。Perceived Standing (#193) が消費する。

つまり #198 の deliverable は:

1. **canonical な ID カタログ** (本ドキュメント)。
2. **schema declaration** (`macrocosmo/src/ai/schema.rs` で Tier 1 分を `declare_*` する)。
3. **ID helper 関数** (`macrocosmo/src/ai/schema/ids.rs` で `fn my_strength() -> MetricId` など)。

emit 実装は各 capability の担当 issue (#204 以降) に分割する。

### Tier 分類

| Tier | 内容 | #198 対象 |
|------|------|-----------|
| Tier 1 | 自派閥の real-time な state を aggregate した metric + 基本的な command / evidence | **yes** (本 issue で declare) |
| Tier 2 | 他派閥の light-delayed な state (`KnowledgeStore` 経由) | no — #193 / #130 で naming を詰める |
| Tier 3 | 時系列 / trajectory / composite assessment (`ThreatLevel`, `ConquerFeasibility`) | no — `ValueExpr::DelT` / `WindowAvg` / Lua 合成で表現 |

---

## Part 1: Metric Topic Catalog (Tier 1)

各行のフォーマット:
- **ID**: `AiBus::declare_metric` で使用する文字列 ID
- **Type**: `MetricType` (`Gauge` / `Counter` / `Ratio` / `Raw`)
- **Retention**: `Retention` (`Short` = 30sd, `Medium` = 120sd, `Long` = 500sd, `VeryLong` = 1200sd)
- **Emitted by**: 将来の producer system (Tier 1 では emit 未実装。`TBD #204` 等)
- **Meaning**: 一行説明

### 1.1 Military — Self (own faction aggregates)

Observer 自派閥の現在の軍事力。real-time — producer が event 駆動 or periodic に再 emit することを想定。

| ID | Type | Retention | Emitted by | Meaning |
|----|------|-----------|------------|---------|
| `my_total_ships` | Gauge | Medium | TBD #204 | 所有船舶の総数 (state 問わず) |
| `my_strength` | Gauge | Medium | TBD #204 | 戦闘力の aggregate (hp + firepower proxy) |
| `my_fleet_ready` | Ratio | Medium | TBD #204 | 稼働可能な艦の比率 (0..=1) |
| `my_armor` | Gauge | Medium | TBD #204 | 装甲 pool 合計 |
| `my_shields` | Gauge | Medium | TBD #204 | シールド pool 合計 |
| `my_shield_regen_rate` | Gauge | Medium | TBD #204 | シールド再生速度 / hexadies |
| `my_vulnerability_score` | Ratio | Medium | TBD #204 | ダメージ累積率 (0=無傷、1=瀕死) |
| `my_has_flagship` | Ratio | Medium | TBD #204 | 旗艦が健在なら 1.0、それ以外 0.0 |

**Source (実装時参照)**:
- `src/ship/hitpoints.rs` — `ShipHitpoints`
- `src/ship/fleet.rs` — fleet aggregate helper

### 1.2 Economy — Production (per-resource net flow)

Empire-wide scope。1 hexadies あたりの純生産量 (modifier 適用後)。

| ID | Type | Retention | Emitted by | Meaning |
|----|------|-----------|------------|---------|
| `net_production_minerals` | Gauge | Long | TBD economy capability | 鉱物純生産 / hexadies |
| `net_production_energy` | Gauge | Long | TBD economy capability | エネルギー純生産 / hexadies |
| `net_production_food` | Gauge | Long | TBD economy capability | 食糧純生産 / hexadies |
| `net_production_research` | Gauge | Long | TBD economy capability | 研究フロー / hexadies (flow, not stock) |
| `net_production_authority` | Gauge | Long | TBD economy capability | 権威発生 / hexadies |
| `food_consumption_rate` | Gauge | Long | TBD economy capability | 食糧消費率 / hexadies |
| `food_surplus` | Gauge | Long | TBD economy capability | `net_production_food - food_consumption_rate` |

**Notes**:
- 「research は flow であって stock ではない」という game design を踏襲し、`net_production_research` のみ。stockpile は存在しない。
- System-scoped production (例: `net_production_minerals_system_<id>`) は Tier 1 に含めない。system 単位での AI 判断が必要になった段階で追加する。

### 1.3 Economy — Stockpiles & Capacity

資源は star system 所属 (`ResourceStockpile` on `StarSystem`)。empire-wide metric は所有 system の合算。

| ID | Type | Retention | Emitted by | Meaning |
|----|------|-----------|------------|---------|
| `stockpile_minerals` | Gauge | Long | TBD economy capability | 鉱物 stockpile 合計 |
| `stockpile_energy` | Gauge | Long | TBD economy capability | エネルギー stockpile 合計 |
| `stockpile_food` | Gauge | Long | TBD economy capability | 食糧 stockpile 合計 |
| `stockpile_authority` | Gauge | Long | TBD economy capability | 権威 stockpile (signed の可能性) |
| `stockpile_ratio_minerals` | Ratio | Medium | TBD economy capability | `stockpile / capacity` (0..=1) |
| `stockpile_ratio_energy` | Ratio | Medium | TBD economy capability | 同上 |
| `stockpile_ratio_food` | Ratio | Medium | TBD economy capability | 同上 |
| `total_authority_debt` | Gauge | Medium | TBD economy capability | 権威不足額の合算 (>= 0) |

### 1.4 Population

| ID | Type | Retention | Emitted by | Meaning |
|----|------|-----------|------------|---------|
| `population_total` | Gauge | Long | TBD economy capability | 総人口 |
| `population_growth_rate` | Gauge | Long | TBD economy capability | 人口増加率 / hexadies |
| `population_carrying_capacity` | Gauge | Long | TBD economy capability | 最大可能人口 |
| `population_ratio` | Ratio | Medium | TBD economy capability | `total / capacity` (1 超で飽和) |

### 1.5 Territory & Expansion

| ID | Type | Retention | Emitted by | Meaning |
|----|------|-----------|------------|---------|
| `colony_count` | Gauge | Long | TBD territory capability | 所有 colony 数 |
| `colonized_system_count` | Gauge | Long | TBD territory capability | `Sovereignty.owner == observer` な system 数 |
| `border_system_count` | Gauge | Medium | TBD territory capability | 他派閥と隣接する自派閥 system 数 |
| `habitable_systems_known` | Gauge | Medium | TBD territory capability | KnowledgeStore 上の居住可能 system 数 |
| `colonizable_systems_remaining` | Gauge | Medium | TBD territory capability | 未支配の居住可能 system 数 |
| `systems_with_hostiles` | Gauge | Medium | TBD territory capability | 敵性存在を検出した system 数 |

### 1.6 Technology

| ID | Type | Retention | Emitted by | Meaning |
|----|------|-----------|------------|---------|
| `tech_total_researched` | Gauge | VeryLong | TBD tech capability | 研究済み tech 総数 (rollback で減る可能性あり → Counter ではなく Gauge) |
| `tech_completion_percent` | Ratio | VeryLong | TBD tech capability | 研究済み / 全 tech |
| `tech_unlocks_available` | Gauge | Long | TBD tech capability | prerequisite 充足済みだが未研究の tech 数 |
| `research_output_ratio` | Ratio | Medium | TBD tech capability | `net_production_research / cost_of_current_research` |

### 1.7 Infrastructure (Capabilities)

| ID | Type | Retention | Emitted by | Meaning |
|----|------|-----------|------------|---------|
| `systems_with_shipyard` | Gauge | Long | TBD infrastructure capability | shipyard 保有 system 数 |
| `systems_with_port` | Gauge | Long | TBD infrastructure capability | port 保有 system 数 |
| `max_building_slots` | Gauge | Long | TBD infrastructure capability | empire-wide 建築 slot 最大 |
| `used_building_slots` | Gauge | Long | TBD infrastructure capability | 使用中 slot 合計 |
| `free_building_slots` | Gauge | Long | TBD infrastructure capability | `max - used` |
| `can_build_ships` | Ratio | Medium | TBD infrastructure capability | shipyard >= 1 なら 1.0、else 0.0 |

### 1.8 Meta / Time / Diplomacy (count-only)

| ID | Type | Retention | Emitted by | Meaning |
|----|------|-----------|------------|---------|
| `game_elapsed_time` | Counter | VeryLong | TBD time capability | `GameClock.elapsed` (hexadies, 単調増加) |
| `number_of_allies` | Gauge | Long | TBD diplomacy capability | 同盟派閥数 |
| `number_of_enemies` | Gauge | Long | TBD diplomacy capability | 戦争中派閥数 |

---

## Part 2: Deferred Metric Categories

以下は **Tier 2+** として、本 issue では declare しない。

### 2.1 Foreign Faction Metrics (Tier 2 — 光速遅延)

元の issue §1.2 / §1.7 / §1.8 の `FactionStrength {faction}` / `StandingWith {faction}` 等。

**未定事項**:
- observer × target の per-pair metric をどう命名するか (`faction_strength.<target>` のような topic naming 規則)
- `KnowledgeStore` からの emit 経路 (snapshot 更新時に差分 emit するか、periodic に全 faction について emit するか)
- light-delay を `emit_at` の tick でどう表現するか (observed_at の tick で emit する想定だが、evaluator 側の扱い未確定)

→ #193 Perceived Standing / #130 Lua binding 確定後に再設計する。

### 2.2 Composite Assessments (Tier 3 — Lua 合成)

`ThreatLevel {faction}`, `ConquerFeasibility {target}`, `VulnerabilityScore`, `LogisticsSustainability`, `EconomicDominanceFeasibility` 等の元 issue §1.10。

これらは Tier 1 metric から `ValueExpr` tree で合成可能:

```rust
// 例: ThreatLevel = (faction_strength / my_strength) * (1 + diplomatic_tension)
ValueExpr::Mul(vec![
    ValueExpr::Div {
        num: Box::new(ValueExpr::Metric(MetricRef::new(faction_strength_id))),
        den: Box::new(ValueExpr::Metric(MetricRef::new(my_strength()))),
    },
    ValueExpr::Add(vec![
        ValueExpr::Literal(1.0),
        ValueExpr::Metric(MetricRef::new(diplomatic_tension_id)),
    ]),
])
```

専用 metric topic を切らない。Lua (#130) から同じ合成を書けるようにする。

### 2.3 Trajectory / Historical Atoms (Tier 3 — DelT で表現)

`FactionStrengthDeltaOverTime`, `PopulationTrendPerColony` 等。専用 topic は不要 — `ValueExpr::DelT { metric, window }` と `ValueExpr::WindowAvg { metric, window }` で time-series retention 内の履歴を参照できる (retention window 内に限る)。

```rust
// 例: 100 hexadies 内の my_strength の変動
ValueExpr::DelT {
    metric: MetricRef::new(my_strength()),
    window: 100,
}
```

長期保存が必要な metric (例: `tech_total_researched`) は `Retention::VeryLong` (1200 hexadies) を指定すれば十分。

---

## Part 3: Command Kind Catalog

AI が emit するコマンド種別。command payload の詳細は各 intent 実装 issue で定義する。

### 3.1 Military

| ID | Description |
|----|-------------|
| `attack_target` | 敵 system または fleet を攻撃 |
| `reposition` | fleet を戦術的位置へ移動 |
| `retreat` | fleet を戦闘から離脱 |
| `blockade` | target system を封鎖 |
| `fortify_system` | 自 system に防御施設を建設 |

### 3.2 Expansion / Infrastructure

| ID | Description |
|----|-------------|
| `colonize_system` | colony 船を派遣して新 colony を設立 |
| `build_ship` | ship design を shipyard で建造 queue |
| `build_structure` | building / structure を建築 queue |
| `survey_system` | surveyor を未探査 system に派遣 |

### 3.3 Research

| ID | Description |
|----|-------------|
| `research_focus` | empire の研究フォーカスを branch / tech に切替 |

### 3.4 Diplomacy

| ID | Description |
|----|-------------|
| `declare_war` | 宣戦布告 |
| `seek_peace` | 講和交渉 |
| `propose_alliance` | 同盟提案 |
| `establish_relation` | 汎用的な外交関係変更 |

---

## Part 4: Evidence Kind Catalog

Perceived Standing (#193) が消費する。`base_weight` / `ambiguous` / `interpretation_key` は `StandingConfig::EvidenceKindConfig` で設定する (本 issue では declare のみ、default 値での例を注記)。

### 4.1 Hostile (positive base_weight — 信頼低下)

| ID | 推奨 base_weight | Retention | Trigger |
|----|------------------|-----------|---------|
| `direct_attack` | +0.5 | VeryLong | 敵が自勢力資産を直接攻撃 (被弾 / 敗北) |
| `system_seized` | +0.7 | VeryLong | かつて自領だった system を奪取された |
| `border_incursion` | +0.4 | Long | 自勢力境界近傍で敵艦を観測 |
| `hostile_buildup_near` | +0.6 | Long | 近隣敵勢力の `faction_strength` が急上昇 |
| `blockade_imposed` | +0.6 | Long | 自領 system が 20+ hexadies 封鎖された |
| `hostile_engagement` | +0.5 | VeryLong | 敵勢力と戦闘 |
| `fleet_loss` | +0.5 | VeryLong | 敵に艦を喪失 |

### 4.2 Friendly (negative base_weight — 信頼上昇)

| ID | 推奨 base_weight | Retention | Trigger |
|----|------------------|-----------|---------|
| `gift_given` | -0.4 | Long | 資源 / tech の譲渡を受けた |
| `trade_agreement_established` | -0.4 | Long | 貿易協定締結 |
| `alliance_with_observer` | -0.6 | VeryLong | 同盟が成立中 |
| `support_against_enemy` | -0.5 | Long | 共通の敵に対する援助行動 |
| `military_withdrawal` | -0.5 | Long | 自勢力境界から敵艦が後退 |

### 4.3 Ambiguous (ambiguous=true, interpretation_key で modulate)

| ID | Retention | Trigger | Interpretation |
|----|-----------|---------|---------------|
| `major_military_buildup` | Long | `faction_strength DelT` > +30% / 100sd | 境界近隣なら hostile、遠方なら 0 |
| `border_colonization` | Long | 自領 < 10ly 内に敵 colony 設立 | 連続するなら assertive、単発なら 0 |

---

## Part 5: Atom Usage Examples

`ai_core` の汎用 atom と本 catalogue の ID を組み合わせる例。

### 5.1 MetricAbove: 単純閾値

```rust
use macrocosmo::ai::schema::ids;
use macrocosmo_ai::{Condition, ConditionAtom};

// 「艦隊 readiness > 0.7」
let fleet_ready = Condition::Atom(ConditionAtom::MetricAbove {
    metric: ids::metric::my_fleet_ready(),
    threshold: 0.7,
});
```

### 5.2 Compare + DelT: トレンド判定

```rust
use macrocosmo::ai::schema::ids;
use macrocosmo_ai::{Condition, MetricRef, ValueExpr};

// 「直近 30 hexadies で鉱物生産が減少している」
let minerals_falling = Condition::lt(
    ValueExpr::DelT {
        metric: MetricRef::new(ids::metric::net_production_minerals()),
        window: 30,
    },
    ValueExpr::Literal(0.0),
);
```

### 5.3 Div + Compare: 比率判定

```rust
use macrocosmo::ai::schema::ids;
use macrocosmo_ai::{Condition, MetricRef, ValueExpr};

// 「research / minerals 比率が 0.2 以上」
let research_intensity_ok = Condition::ge(
    ValueExpr::Div {
        num: Box::new(ValueExpr::Metric(MetricRef::new(
            ids::metric::net_production_research(),
        ))),
        den: Box::new(ValueExpr::Metric(MetricRef::new(
            ids::metric::net_production_minerals(),
        ))),
    },
    ValueExpr::Literal(0.2),
);
```

### 5.4 All + 複数 atom: 複合 precondition

```rust
use macrocosmo::ai::schema::ids;
use macrocosmo_ai::{Condition, ConditionAtom, MetricRef, ValueExpr};

// Intent: AttackTarget の precondition 例
let can_attack = Condition::All(vec![
    // 艦隊健康度
    Condition::Atom(ConditionAtom::MetricAbove {
        metric: ids::metric::my_fleet_ready(),
        threshold: 0.6,
    }),
    // ダメージ許容範囲
    Condition::Atom(ConditionAtom::MetricBelow {
        metric: ids::metric::my_vulnerability_score(),
        threshold: 0.4,
    }),
    // 戦争数制限
    Condition::lt(
        ValueExpr::Metric(MetricRef::new(ids::metric::number_of_enemies())),
        ValueExpr::Literal(3.0),
    ),
]);
```

### 5.5 EvidenceCountExceeds: standing 変化の発火

```rust
use macrocosmo::ai::schema::ids;
use macrocosmo_ai::{Condition, ConditionAtom};

// 「直近 200 hexadies で 3 回以上 direct_attack された」
let sustained_hostility = Condition::Atom(ConditionAtom::EvidenceCountExceeds {
    kind: ids::evidence::direct_attack(),
    window: 200,
    threshold: 3,
});
```

### 5.6 IfThenElse: 条件付き値

```rust
use macrocosmo::ai::schema::ids;
use macrocosmo_ai::{Condition, ConditionAtom, MetricRef, ValueExpr};

// 「flagship があれば my_strength、なければ 50% 減」
let effective_strength = ValueExpr::IfThenElse {
    cond: Box::new(Condition::Atom(ConditionAtom::MetricAbove {
        metric: ids::metric::my_has_flagship(),
        threshold: 0.5,
    })),
    then_: Box::new(ValueExpr::Metric(MetricRef::new(ids::metric::my_strength()))),
    else_: Box::new(ValueExpr::Mul(vec![
        ValueExpr::Metric(MetricRef::new(ids::metric::my_strength())),
        ValueExpr::Literal(0.5),
    ])),
};
```

---

## Part 6: Implementation Roadmap

### 6.1 This Issue (#198) — delivered

- [x] Metric / Command / Evidence canonical ID helpers (`macrocosmo::ai::schema::ids`)
- [x] Tier 1 `declare_*` calls in `schema::declare_all` (topic は declare されるが emit はまだ 0)
- [x] Doc catalogue (this file)
- [x] Smoke test (`ai_integration::ai_plugin_declares_tier1_schema_on_startup`)

### 6.2 Downstream Capability Issues (emit 実装)

| Category | Issue | Producer notes |
|----------|-------|----------------|
| Military (Self) | #204 FleetCombatCapability | ship query → aggregate → emit per-tick or on `ShipBuilt` / `ShipLost` event |
| Economy (Production) | (TBD) | production tick の出力を aggregate |
| Economy (Stockpile) | (TBD) | stockpile tick 後に empire-wide sum を emit |
| Population | (TBD) | colony population tick 後 |
| Territory | (TBD) | Sovereignty 変化 event 駆動 |
| Technology | (TBD) | `TechTree` 変化時 |
| Infrastructure | (TBD) | `BuildingCompleted` / `BuildingDemolished` 駆動 |
| Meta/Time | (TBD) | `GameClock` 進行時の簡易 emit |

### 6.3 Follow-up Issues (本 issue の範囲外)

- **#193 Perceived Standing evidence emit** — evidence producer の実装 + `StandingConfig` の Lua 化
- **#130 Lua binding** — Lua 側から本 catalogue の ID を参照する API
- **Tier 2 Foreign faction metrics** — per-observer × per-target naming convention 決定後
- **Tier 3 Composite assessment** — Lua/Rust 双方で合成可能な ValueExpr helper の整備
- **#190 Combat Projection** — `my_strength` の具体式 (hp × firepower の正確な重み付け)
- **#191 Economic Projection** — feasibility score の Lua 定義
- **#194 Assessment Metrics** — trajectory-based 予測 atom

---

## Appendix: Topic 命名規則

本 catalogue で確定した規則:

- **snake_case** のみ。
- **scope prefix**: empire-wide は無印 (`my_*` は observer 自派閥を指す既成慣例)。system-scope は未定 (Tier 2 で確定)。
- **per-resource metric** は `<action>_<resource>` の形 (例: `net_production_minerals`, `stockpile_energy`)。
- **ratio vs absolute**: 比率は `*_ratio` suffix (例: `stockpile_ratio_minerals`) か `*_percent` (tech に限る)。
- **boolean as gauge**: ratio 0.0/1.0 で表現。enum-like `MetricType::Bool` は設けない。

Tier 2 以降で ID 空間が広がったら、必要に応じて `.` 区切りの階層命名 (例: `faction_strength.observed.1234`) を導入することも検討する。現段階ではフラット命名のまま。
