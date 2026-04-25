# Session handoff — macrocosmo-ai 3 層 AI tuning (2026-04-25)

## TL;DR

`macrocosmo-ai` クレートに **抽象 3 層 AI (Long / Mid / Short)** を実装し、
6 ラウンドの tuning loop を抽象シナリオで実施。

- 全 **27 test binary green** (`cargo test -p macrocosmo-ai --features playthrough`)
- 仕様書: `docs/ai-three-layer.md` (Status: Approved 2026-04-25)
- メモリ: `~/.claude/projects/-Users-csakai-repos-macrocosmo/memory/project_ai_three_layer_design.md`

ゲーム要素を持ち込まず、scripted scenario で 3 層ループの整合性を tune した。

## 直近のコミット履歴 (新しい順)

```
a7394ce feat(ai): Mid に victory_status awareness + multi-faction parallel scenarios
33b0865 feat(ai): projection-driven per-leaf validity window
7bf0beb feat(ai): IntentDispatcher::estimate_delay + Long の動的 expiry 適応
542c62e feat(ai): priority-weighted command emission (Campaign.weight + Short fractional accumulator)
d4d40d3 feat(ai): Mid に prereq guardrail を追加 + tradeoff/competition シナリオ
4bd5c13 feat(ai): abstract scenario に command→metric feedback を追加
0660b7c test(ai): 抽象シナリオ 3 本 + Long の preemptive preserve 機能追加
dc740a0 feat(ai): macrocosmo-ai に 3 層 AI (Long/Mid/Short) アーキテクチャを追加
```

`dc740a0` が初期実装、`a7394ce` が現状最新。

## 現状の default agent capability

| 層 | 機構 | 関連 commit |
|---|---|---|
| **Long** (`ObjectiveDrivenLongTerm`) | terminal short-circuit, satisfied 短絡, prereq `safety_margin` (preemptive), retry/fallback (`max_retries`), projection-driven per-leaf window | dc, 0660b7c, 7bf0beb, 33b0865 |
| **Mid** (`IntentDrivenMidTerm`) | inbox 優先度 sort, `supersedes`, `stale_threshold`, expired drop, `prereq_guardrail`, weight stamping, terminal abandon | dc, d4d40d3, 542c62e, a7394ce |
| **Short** (`CampaignReactiveShort`) | priority-weighted fractional accumulator (`priority_weighted` 切替で legacy 1cmd/tick mode 残置) | 542c62e |

各 agent は **trait + デフォルト impl** の構造で、ゲーム側は trait 実装を差替可。

## Round-by-round 圧縮サマリ

| Round | テーマ | 追加 / 修正 | 露出 → 閉鎖した gap |
|---|---|---|---|
| 1 (`0660b7c`) | 初期 scenario 整備 | scenario_victory_unreachable / compound_win / preemptive_preserve、Long に `safety_margin` | reactive のみ → 事前対応 |
| 2 (`4bd5c13`) | command → metric feedback | `SyntheticDynamics.command_responses` + `MetricEffect` | AI が metric を動かせない |
| 3 (`d4d40d3`) | tradeoff / competition | Mid `prereq_guardrail`, `MidTermInput.victory` | 無策に prereq 枯渇 |
| 4 (`542c62e`) | priority-weighted emission | `Campaign.weight`, `CampaignOp::SetWeight`, Short fractional accumulator | 重要 pursuit と些末で同頻度 |
| 5 (`7bf0beb`) | dynamic expiry | `IntentDispatcher::estimate_delay`, `LongTermInput.recent_drops`, retry/fallback、`DropEntry.metric_hint` | expiry < estimate でも emit |
| 5b (`33b0865`) | projection-driven window | `LongTermDefaultConfig.use_projection_window` で per-leaf 動的 expiry | 全 leaf 同 window で粗い |
| 6 (`a7394ce`) | Mid 終端認識 + multi-faction | `MidTermInput.victory_status`, `abandon_on_terminal`、2 faction 並列 scenario 2 本 | terminal 後も Short が emit、faction 間混入 |

## ファイル / API surface

### 新規ファイル (`macrocosmo-ai/src/`)

```
intent.rs              IntentSpec / Intent / RationaleSnapshot / effective_priority
victory.rs             VictoryCondition / VictoryStatus / evaluate
dispatcher.rs          IntentDispatcher trait + DispatchResult + FixedDelayDispatcher
agent.rs               Long/Mid/Short trait + I/O + CampaignOp + OverrideEntry + ShortContext
orchestrator.rs        Orchestrator + 6-step tick + intent_queue/pending/drop logs
long_term_default.rs   ObjectiveDrivenLongTerm (config, retry, projection)
mid_term_default.rs    IntentDrivenMidTerm (guardrail, terminal abandon, weight stamp)
short_term_default.rs  CampaignReactiveShort (legacy + weighted modes)
playthrough/agent_scenario.rs  AgentScenario / FactionAgentSpec / FactionTrace / run_agent_scenario
```

### 既存への変更

- `campaign.rs` — `Campaign.source_intent`, `Campaign.weight` 追加
- `ids.rs` — `IntentKindId`, `IntentTargetRef`, `DeliveryHintId`, `ShortContext` 追加
- `playthrough/scenario.rs` — `SyntheticDynamics.command_responses` + `MetricEffect`
- `playthrough/mod.rs` — re-export

## テスト一覧 (`macrocosmo-ai/tests/`)

### Scenarios (各シナリオは 1〜3 件のテストを含む)

| ファイル | カバレッジ |
|---|---|
| `scenario_economic_growth.rs` | happy path: 成長 → Won |
| `scenario_survival_under_threat.rs` | 時間ベース victory + 脅威の波 |
| `scenario_victory_unreachable.rs` | prereq 違反 → Unreachable、Long emit 停止 |
| `scenario_compound_win.rs` | `win = All(A,B)` で per-leaf pursuit |
| `scenario_preemptive_preserve.rs` | safety_margin で事前対応 |
| `scenario_ai_driven_growth.rs` | command → metric feedback、AI 介入で勝つ vs 介入なしで勝てない |
| `scenario_pursuit_tradeoff.rs` | 無策で bleed → Unreachable / guardrail 有で cycle して Won |
| `scenario_intent_competition.rs` | 共有 budget で multi-pursuit、guardrail で両 leaf throttle |
| `scenario_priority_weighted.rs` | weight 比 0.81:0.27 → emission 比 ~3:1 |
| `scenario_expiry_adaptation.rs` | retry → Won / 全 leaf surrender → no-op |
| `scenario_projection_window.rs` | per-leaf 非対称 window (fast<slow) |
| `scenario_mid_terminal_awareness.rs` | Unreachable で Mid abandon、command 停止 |
| `scenario_multi_faction.rs` | 独立 / cooperative race |

### 整合性テスト

- `three_layer_consistency.rs` — vertical / temporal / informational の 5 件

## 確定した design 決定 (memory も参照)

`~/.claude/projects/-Users-csakai-repos-macrocosmo/memory/project_ai_three_layer_design.md` に詳細。

1. **IntentKind / CampaignSpec は open-kind** (`Arc<str>` + params bag)。crate 境界保持、enum 禁止
2. **IntentSpec / Intent split** — Long は未ルーティング `IntentSpec` を emit、Orchestrator が Dispatcher 経由で `Intent` 化
3. **IntentDispatcher は game-side 裁量** — courier / relay / 光速 signal の選択、build-then-dispatch、resource 消費は impl 自由
4. **Metric schema は macrocosmo-ai が固定名を持たない** — `AssessmentConfig` 注入でゲーム/シナリオが自由 MetricId
5. **Intent → Campaign mapping は Mid の責務** (1:1 / 1:N / override 全部表現可)
6. **Nash は trait 後付け** — 最初は単純 utility
7. **VictoryCondition** = `{ win, prerequisites, time_limit, score_hint }`、`prerequisites` は gate + pursuit target の dual role
8. **Mid → Short Intent 機構**: AI 単体 MVP では Campaign 直読 (省略可)、ゲーム統合段階で必須
9. **層の所在**: Long@Ruler / Mid@Governor(region) / Short@Context (FleetShort / ColonyShort)

## 次セッションの候補 (重要度の主観順)

### A. memory / spec doc に Round 1-6 の累積を反映
`docs/ai-three-layer.md` は初期 spec、現状の各 default agent の config/挙動が
記述されていない。memory も初期決定だけ。Round 1-6 で追加された機構
(safety_margin, prereq_guardrail, weight, projection window, terminal abandon, etc) を
反映 → 次の人 (将来の自分) が現状把握しやすくなる。**最も低コスト**。

### B. game integration 着手
`macrocosmo-ai` の primitive を `macrocosmo` クレートに繋ぐ。

具体的には:
- 既存 `SimpleNpcPolicy` を `ShortTermAgent` 実装に置き換え or 並置
- `GrandPlan`-相当を `LongTermAgent` 実装として作る
- macrocosmo 側 `CourierDispatcher` 実装 (光速 / courier / relay の選択)
- `AgentRegistry` で `IntentTargetRef` → 実 entity 解決
- まずは 1 faction で動く形 → multi-Mid 拡張は別 step

「Plan agent → レビュー → 実装」の対象 (10+ ファイル変更見込み)。

### C. 真の adversarial scenario
現 `command_responses` は per-issuer routing に対応していない。
`SyntheticDynamics.command_responses_per_faction: HashMap<FactionId, ...>` を
追加するか、metric を `metric.faction_X` 命名で per-faction scoped にする
ハーネス拡張が必要。これで `faction 0 が econ_0 を伸ばすと faction 1 の
資源が減る` のようなゼロサム scenario が書ける。

→ 抽象 scenario の表現力をさらに広げるが、game integration を急ぐなら
B が先。

## 再開の手順

```bash
# 現状確認
cd /Users/csakai/repos/macrocosmo
git log --oneline -8
cargo test -p macrocosmo-ai --features playthrough 2>&1 | grep -cE "^test result: ok"
# → 27 が出れば緑

# 仕様書とメモリ
cat docs/ai-three-layer.md | head -50
cat ~/.claude/projects/-Users-csakai-repos-macrocosmo/memory/project_ai_three_layer_design.md
```

次セッション開始のプロンプト例:

> macrocosmo-ai の 3 層 AI tuning を続ける。
> 現状は `docs/session-handoff-2026-04-25-ai-three-layer.md` 参照。
> 次は **A (memory/spec 反映)** からやる。

または:

> ハンドオフ doc 読んで現状把握してから、**B (game integration)** に着手する。
> `macrocosmo-ai` の primitive を `macrocosmo` 側にどう繋ぐか、Plan agent で
> 設計案を出して。

## 注意点 / 落とし穴

- `MetricScript` を持つ metric に `command_responses` で feedback すると、
  毎 tick script が値を上書きするので feedback が消える。**feedback 対象 metric
  は metric_scripts に入れない**(初期値は tick_fn で seed)
- `run_agent_scenario` では `tick_fn` が orchestrator の **前** に走る
  (`run_scenario` は後ろ)。tick 0 で初期値を seed する用途を意識した順序
- `detect_threshold` は metric が threshold を**越えた後**は future crossing を
  返さない → projection 退場 → static fallback。Long の per-leaf window
  feature 使用時に注意 (cross 後は短い window で問題ないので OK)
- `LongTermInput.recent_drops` の per-tick scope: `state.drops_seen_by_long_until`
  index で前回 long tick 以降の drop だけを slice 化。Long が同じ drop を二重カウントしない
- 既存の test は `MidTermInput { ... }` / `LongTermInput { ... }` を構造体リテラルで
  作ってる箇所が複数 — フィールド追加時は全て更新必要 (current count: Mid 5 sites,
  Long 6 sites in test modules + scenarios)

## 次の round で削るべき技術負債 (低優先)

- `MidTermInput` / `LongTermInput` のフィールドが増え続けている → builder pattern
  か `*Input::new(bus, faction, now)` + `.with_*()` chaining への移行検討
- `IntentSpec` も同様 (現状 11 fields)
- `OrchestratorState` にいろんな log が積もる (drop_log, override_log, intent_queue,
  pending_specs) → 長時間 scenario で線形成長、game integration 時に capped queue 化検討

---

# Round 7-8 追記 (2026-04-25 後半セッション)

## Round 7 — adversarial zero-sum + maintenance pursuit

`c68c990 feat(ai): adversarial zero-sum scenario + Won-maintenance pursuit option`

抽象 scenario で zero-sum dynamics (`+own, -opp` の cross-effect を `command_responses` で encode) を書いて挙動観察したところ、**asymmetric power scenario で弱い faction が勝つ逆転現象**が発生。原因を追ったところ:

- `Mid.abandon_on_terminal` が `Won → Succeeded` で active campaign を捨てる
- metric が adversary に侵食されて閾値割れ → Long が再 emit するまでに lag
- その間 adversary は monotonic に伸びる → 弱い側が勝つ

修正: `MidTermDefaultConfig.treat_won_as_terminal: bool` を追加 (default `true` で後方互換)。`false` のとき Won は abandon せず Active 維持 → Short が emit を続けて閾値を defend する **maintenance pursuit** モードになる。

設計上の結論: macrocosmo の core mechanic (光速遅延で他 faction の score 不可視 → AI は自 metric maximize 一択) と整合。score-race モデルでは threshold を「通過点」として扱い、達成後も Short の惰性 emit で score を伸ばし続ける。Long は閾値到達までガイド、達成後は Short が `treat_won_as_terminal=false` 経由で maintain。

新規 scenario:
- `symmetric_zero_sum_yields_stalemate` — 完全対称 → 互いに打ち消し → 誰も勝たない (PASS)
- `asymmetric_strength_decides_the_race` — f0=+2/-1, f1=+1/-1 → maintenance pursuit で f0 が ~tick 50 で Won 後も伸び続け、ore_0 が 250 まで到達 (PASS)

28 test binary green (元 27 + adversarial 1)。

### 露出した将来課題

「Long が閾値達成で停止 → 戦略の動的再評価が失われる」 + 「Long の戦略空間が pursue_metric 1 個のみ」。本格的な adversarial では **複数 strategy candidate を持つ Long** が必要 (例: concentrate vs distribute、offense vs defense)。これは `StrategyCandidate` trait + utility 比較の **Round 9 以降の architectural work** で対応。

## Round 8 — game integration 最小スケルトン

`54a5d9f feat(scripting): Lua-table parser for AI VictoryCondition`
`c36adfa feat(ai): FactionOrchestrator skeleton + registry resource`
`a94a37e feat(ai): wire FactionOrchestrator into AiPlugin (Step 2-3+6)`

macrocosmo-ai の 3 層 orchestrator を game crate に **additive** に統合 (既存 `SimpleNpcPolicy` と並列動作、 revert 容易)。

### 構成要素

- **Step 0** (`macrocosmo/src/scripting/victory_api.rs`): Lua-table → `VictoryCondition` parser。`define_victory` global 等の Lua-side wiring は後回し、parser surface だけ整備。「Lua が将来構築する table を Rust から直接渡せる」形。
- **Step 1** (`macrocosmo/src/ai/orchestrator_runtime.rs`):
  - `FactionOrchestrator` newtype (Orchestrator + FixedDelayDispatcher + VictoryCondition)
  - `OrchestratorRegistry` Resource (`HashMap<Entity, FactionOrchestrator>`)
  - `new_demo` constructor — Step 0 の parser 経由で demo VictoryCondition (`colony_count.faction_<n> > 1.0`) を構築
  - cadence: `long_cadence=5, mid_cadence=2`、dispatcher delay=2 hexadies
- **Step 2** (plugin): `register_demo_orchestrator` を `OnEnter(NewGame)` で 1 NPC empire に arm
- **Step 3** (plugin): `run_orchestrators` を `AiTickSet::Reason` `.after(npc_decision_tick)` で per-tick 駆動。produced commands は bus に emit (drain_ai_commands が unknown として silent ignore)
- **Step 6**: per-command observer log (`ai_orch_cmd` target)

### Demo

```bash
RUST_LOG=info,ai_orch=info,ai_orch_cmd=info \
  cargo run --bin macrocosmo -- --no-player --seed 1 --speed 4 --time-horizon 30
```

期待 log: `AI orchestrator armed for ...` → 数 tick で `ai_orch tick=N long=true mid=true short=true cmds=K status=Ongoing { progress: 0.0 }` → colony_count >= 1 で `status=Won` 維持 (Round 7 maintenance 動作)。

## 副次 fix: ObscuredByGas dead code 削除

`28de3cb fix(galaxy): remove ObscuredByGas dead prototype + bump SAVE_VERSION`

observer mode で挙動観察中に発見。 ObscuredByGas は #145 (CLOSED, milestone 0.2.0) で `ForbiddenRegion` (metaball field、 Lua-defined region types) に置き換えられたが、 削除されず残ってた visual-only prototype。

症状:
- 15% の system が click 不能 (`collect_candidates` が `obscured.is_some()` で skip)
- glow halo なし、 0.15-alpha sprite だけで「halo がない system がいくつか」状態
- NPC が ObscuredByGas を意識せず survey 命令を発行 → 別 race (下記) で loop 化が表面化

削除箇所: galaxy/{mod,generation}.rs、 visualization/{stars,mod}.rs、 persistence/{save,load,savebag}.rs。SAVE_VERSION 7 → 8、 minimal_game.bin fixture regen。

将来 nebula 風 FTL inhibition は `define_region_type { capabilities = { blocks_ftl = ... } }` で Lua 側に実装する (Rust 機構は `galaxy/region.rs` に既存)。

## 残課題: NPC survey loop (BRP debug 待ち)

ObscuredByGas 削除後も、特定 system に対して 30 hexadies 周期で survey 進捗が 0 にリセットされて永遠に終わらない症状が継続。NPC の再 emit ではなく、**1 つの survey command が dispatcher → handler → start_survey の loop を作ってる**疑い。

候補:
- `handle_survey_requested` の auto re-insert path (`docked_system != target` で `[MoveTo, Survey]` を queue head に挿入) が specific 条件で loop
- bridge insertion で生まれた system の position randomness で `start_survey` 内の range check に引っかかる、 など

debug 戦略:
- BRP `world.query` で問題 ship の `CommandQueue + ShipState + Position` 取得 → loop の起源特定
- ただし大半の component が `Reflect` 未実装で BRP から見えない → **Reflect 全 derive を Round 9 prep で先に landed させる**

## Round 9 候補 (優先順)

1. **Reflect 全 derive + register_type 一括対応** (worktree agent で並行実施中) — BRP 完全対応
2. **survey loop の root cause 特定 + fix** (Reflect 完了後 BRP で調査 → fix + regression test)
3. **NPC が `unsurveyed_systems` リストを正しく管理** (KnowledgeStore 連携不在) — survey loop と隣接する別 issue
4. **Long の戦略選択機構** (`StrategyCandidate` trait、 concentrate vs distribute scenario) — Round 7 で露出した本質的課題
