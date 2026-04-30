# Session handoff — Round 7-9 (2026-04-25 / 26)

## TL;DR

3 ラウンド連続で実装が進んだ大型セッション。 commit `c68c990 → 7afda7f` の **15 commits** で:

- **Round 7** — adversarial scenario + maintenance pursuit (`treat_won_as_terminal`)
- **Round 8** — game integration 最小スケルトン (`macrocosmo-ai` orchestrator が NPC empire に additive で wire される)
- **Round 9** — observer mode + NPC AI 周辺の player-only 前提を全面剥がし (knowledge per-faction、 PendingAssignment dedup + knowledge-driven cleanup、 NPC ship auto-return + deliver、 viewer-aware visualization)
- 副次: ObscuredByGas dead prototype 削除、 Reflect 全 derive で BRP `world.query` 完全対応、 `keybinding` Registry (#347、 UI 以外)

`70` test binary all green、 `cargo build --features remote` clean、 `cargo fmt` clean。 SAVE_VERSION `8 → 10` (PR #1 + #2 統合分)。

前段ハンドオフ: `docs/session-handoff-2026-04-25-ai-three-layer.md` (Round 1-6 + 後半 PR #1+2 まで追記済)。 **本ファイルが Round 7-9 の主ハンドオフ**。

## Recent commits (新しい順、 今日 + 前日 cleanup 含む)

```
7afda7f fix(visualization): viewer's ruler position via Empire→Ruler chain
515e7cf fix(ship/survey): per-empire deliver_survey_results
b0ea993 fix(ship/survey): per-ship-owner auto-return for NPC scouts
7ad059a fix(ai): PendingAssignment lifetime extends to KnowledgeStore arrival
3ceed45 chore(persistence): regen minimal_game.bin for SAVE_VERSION 10 + handoff doc
ce93186 feat(ai): PendingAssignment dedup against double survey dispatches  (= PR #2)
b3b36ac feat(knowledge): per-faction record_for at every callsite (Step 3)
47a83e7 feat(knowledge): per-empire PendingFactQueue Component (Step 2)
33ea009 feat(knowledge): FactionVantage + FactSysParam::record_for (Step 1, additive)
3075739 docs(ai): handoff doc に Round 7-8 + ObscuredByGas removal 追記
28de3cb fix(galaxy): remove ObscuredByGas dead prototype + bump SAVE_VERSION
ddf3a5d リモートデバッグのドキュメントを修正 (user)
4e37262 style(workspace): cargo fmt --all sweep
9eb52f7 docs(scripting): Lua API リファレンス
d3a181e style(workspace): rustfmt drift cleanup from prior agent work
90e8ee5 style(tests): rustfmt common test helper
69d3162 feat(input): KeybindingRegistry + migrate hardcoded keybinds (#347)
08145b3 feat(reflect): ReflectRegistrationPlugin — register every type for BRP
fd4861f feat(reflect): wire AI / scripting Resources into Reflect with engine-agnostic ignores
988ae36 feat(reflect): derive Reflect on game Component / Resource types
6e7b747 feat(reflect): derive Reflect on shared/foundation types
a94a37e feat(ai): wire FactionOrchestrator into AiPlugin (Step 2-3+6)
c36adfa feat(ai): FactionOrchestrator skeleton + registry resource
54a5d9f feat(scripting): Lua-table parser for AI VictoryCondition
c68c990 feat(ai): adversarial zero-sum scenario + Won-maintenance pursuit option
```

## Round 7 — adversarial scenario + maintenance pursuit

Commit `c68c990`。 抽象 zero-sum scenario (各 faction が own metric を pursue、 cross-effect を `command_responses` で encode) で挙動観察した結果、 **asymmetric 設定なのに弱い faction が勝つ逆転現象**を露出。

### Root cause

`MidTermDefaultConfig.abandon_on_terminal` が `Won → Succeeded` で active campaign を捨てる → metric が adversary に侵食されて閾値割れ → Long が再 emit するまで lag → adversary は monotonic 伸長 → 弱い側が勝つ。

### 修正

`MidTermDefaultConfig.treat_won_as_terminal: bool` を追加 (default `true` で後方互換)。 `false` のとき Won でも abandon せず Active 維持 → Short が emit を続けて閾値を defend する **maintenance pursuit** モード。

### 設計上の結論

macrocosmo の core mechanic (光速遅延で他 faction の score 不可視 → AI は self-metric maximize 一択) と整合。 score-race モデルでは threshold を「通過点」として扱い、 達成後も Short の惰性 emit で score を伸ばし続ける。

### 露出した将来課題 (Round 9 後でも未対応)

- 「Long が閾値達成で停止 → 戦略の動的再評価が失われる」 + 「Long の戦略空間が `pursue_metric` 1 個のみ」
- 本格的 adversarial では複数 strategy candidate を持つ Long が必要 (例: concentrate vs distribute、 offense vs defense)
- `StrategyCandidate` trait + utility 比較の **次回以降の architectural work**

## Round 8 — game integration 最小スケルトン

Commits `54a5d9f → c36adfa → a94a37e`。 macrocosmo-ai の 3 層 orchestrator を game crate に **additive** に統合 (既存 `SimpleNpcPolicy` と並列動作、 revert 容易)。

### 構成要素

- **Step 0** (`macrocosmo/src/scripting/victory_api.rs`): Lua-table → `VictoryCondition` parser。 `define_victory` global の Lua-side wiring は後回し、 parser surface だけ整備。 「Lua が将来構築する table を Rust から直接渡せる」形。
- **Step 1** (`macrocosmo/src/ai/orchestrator_runtime.rs`):
  - `FactionOrchestrator` newtype (Orchestrator + FixedDelayDispatcher + VictoryCondition)
  - `OrchestratorRegistry` Resource (`HashMap<Entity, FactionOrchestrator>`)
  - `new_demo` constructor — Step 0 parser 経由で demo VictoryCondition (`colony_count.faction_<n> > 1.0`) 構築
  - cadence: `long_cadence=5, mid_cadence=2`、 dispatcher delay=2 hexadies
- **Step 2** (plugin): `register_demo_orchestrator` を `OnEnter(NewGame)` で 1 NPC empire に arm
- **Step 3** (plugin): `run_orchestrators` を `AiTickSet::Reason` `.after(npc_decision_tick)` で per-tick 駆動。 produced commands は bus に emit (drain_ai_commands が unknown として silent ignore)
- **Step 6**: per-command observer log (`ai_orch_cmd` target)

### Demo

```bash
RUST_LOG=info,ai_orch=info,ai_orch_cmd=info \
  cargo run --bin macrocosmo -- --no-player --seed 1 --speed 4 --time-horizon 30
```

期待 log: `AI orchestrator armed for ...` → 数 tick で `ai_orch tick=N long=true mid=true short=true cmds=K status=Ongoing { progress: 0.0 }` → colony_count >= 1 で `status=Won` 維持 (Round 7 maintenance 動作)。

## ObscuredByGas dead prototype 削除

Commit `28de3cb`。 observer mode で挙動観察中に発見。 ObscuredByGas は #145 (CLOSED) で `ForbiddenRegion` (metaball field、 Lua-defined region types) に置き換えられたが、 削除されず残ってた visual-only prototype。

### 症状

- 15% の system が click 不能 (`collect_candidates` が `obscured.is_some()` で skip)
- glow halo なし、 0.15-alpha sprite だけで「halo がない system がいくつか」状態
- NPC が ObscuredByGas を意識せず survey 命令を発行 → 別 race (Round 9 で別途対応) で loop 化が表面化

削除箇所: `galaxy/{mod,generation}.rs`、 `visualization/{stars,mod}.rs`、 `persistence/{save,load,savebag}.rs`。 SAVE_VERSION 7 → 8、 minimal_game.bin fixture regen。

将来 nebula 風 FTL inhibition は `define_region_type { capabilities = { blocks_ftl = ... } }` で Lua 側に実装する (Rust 機構は `galaxy/region.rs` 既存)。

## Reflect 全 derive — BRP 完全対応

Commits `6e7b747 → 988ae36 → fd4861f → 08145b3`。 4 commits で全 Component / Resource に Reflect derive + ReflectRegistrationPlugin で **209 register_type calls**。

### 設計判断

`macrocosmo-ai` は **engine-agnostic 維持** (CI `ai-core-isolation.yml` 通る、 wrapper boundary で `#[reflect(ignore)]`)。 これにより BRP `world.query` が全 ECS state に対して動く。

### `#[reflect(ignore)]` した型

- `AiBusResource.0: AiBus` (macrocosmo-ai は bevy_reflect 依存追加できない)
- `OrchestratorRegistry.by_entity` (同上)
- `ScriptEngine.{lua, print_buffer}` (mlua + Mutex)
- `GameRng.inner` (Arc<Mutex<Xoshiro>>)
- `KnowledgeSubscriptionRegistry.{exact, wildcard}` (mlua::RegistryKey)
- `LuaFunctionRef.inner` (Arc<mlua::RegistryKey>)
- `FiredEvent.payload` (Arc<dyn EventContext>)
- `SituationTabRegistry.tabs` (Vec<Box<dyn SituationTab>>)
- `Condition::Not(Box<Condition>)` と `DescriptiveEffect::Hidden { inner: Box<...> }` (`bevy_reflect` 0.18 が `Box<T>` を Reflect しない、 outer 列挙 tag は visible)

### Drop-Reflect 完全 (no good Default)

- `PendingRoute` (bevy::tasks::Task)
- `AiDebugUi` (transitively non-Clone AiBus)

これで BRP query で **survey loop 等の挙動を実体観察** できるようになり、 Round 9 の root-cause 特定の前提条件が揃った。

## #347 KeybindingRegistry (UI 以外)

Commit `69d3162`。 並列 worktree agent で実装。

### 内容

- `KeybindingRegistry` Resource (action_id `string` → `KeyCombo` map)
- 14 actions registered (mirrors prior hardcoded behaviour exactly)
- `keybindings.toml` 永続化 (XDG / macOS Application Support / Windows AppData)
- `MACROCOSMO_KEYBINDINGS_PATH` env var bypass (tests / CI)
- 既存 hardcoded keybind 全 migration (time, camera, selection, UI panels, observer, debug)
- Conflict 検出 + warn (UI なし)
- 25 unit tests + `engine_defaults_only_known_intentional_collision` test

### 残り (rebinding UI)

issue #347 の rebinding UI 部分は別 PR で対応 (settings panel sub-section、 click-to-rebind capture)。

## Round 9 — observer mode + NPC AI per-faction generalize

3 段階 (PR #1 + #2 + follow-up commits) に分けて landed。

### 動機 (BRP で実体観察した結果)

`cargo run --features remote` で起動 → BRP `world.query` で 3 つの相互関連バグを発見:

1. **Bug 1 NPC duplicate survey emit race** — `idle_surveyors.zip(unsurveyed_systems)` で同じ target に複数 ship 割当
2. **Bug 2 AI 命令の光速遅延無視** — `bus.emit_command` → `drain_ai_commands` 同 tick 即消費、 `PendingCommand` 経由してない
3. **Bug 3 `fact_sys.record` が player-only** — observer mode で vantage=None になり全 KnowledgeStore に積まれない

これらは「player-only 前提」という同じ root から派生。

### Plan agent draft → 実装

Plan agent (`docs/session-handoff-2026-04-25-ai-three-layer.md` 末尾の Round 9 候補参照) で per-faction generalize architectural plan を draft → 3 PR 構成に分解 → 並列 worktree agent で PR #1 + PR #2 を実装 → 統合後 follow-up commits。

### PR #1 — knowledge layer per-faction generalize

Commits `33ea009 → 47a83e7 → b3b36ac` (Steps 1-3)。

- **Step 1 (additive)**: `FactionVantage { faction, ref_pos, ruler_aboard }` + `FactSysParam::record_for(fact, vantages: &[FactionVantage], origin_pos, at)`。 既存 `record(...)` は legacy 単一 PlayerVantage adapter として残す。
- **Step 2**: `PendingFactQueue` を Resource → per-empire Component 化。 NPC empire bundle に `CommsParams` 追加。 SAVE_VERSION 8 → 9 + fixture regen。
- **Step 3**: 全 8 fact-emitting callsite (`ship/{survey,combat,movement,settlement,pursuit,handlers/deliverable_handler}.rs`、 `colony/{colonization,building_queue}.rs`、 `deep_space/mod.rs`) を `record_for` 経由に migrate。 `FactionVantageQueries` SystemParam bundle で 4 query を 1 にまとめて 16 limit を回避。

#### 設計判断

- `NotifiedEventIds` は Resource 維持 (banner dedup は player-empire-only、 per-faction 化不要)
- Observer mode は `collect_faction_vantages` が空 `Vec` を返す経路で自動対応
- `compute_fact_arrival` は player empire の `CommsParams` を読む — NPC は own `CommsParams` を持つが小数の relay math は MVP の compromise (TODO documented)
- `Owner::Neutral` ships の facts は同 vantages slice で player に届く (special-case しない)

### PR #2 — PendingAssignment for NPC dedup

Commit `ce93186`。

- 新 Component `PendingAssignment { faction, kind, target, since, stale_at }` を ship entity に attach
- `AssignmentKind::Survey` + `AssignmentTarget::System` (将来 Colonize 等に拡張可能)
- `command_consumer.rs::handle_survey_system` で dispatch 時 insert
- `npc_decision_tick` で per-faction `pending_survey_targets` + `pending_assigned_ships` を pre-collect、 `unsurveyed_systems` から exclude
- `sweep_stale_assignments` system が `stale_at` 経過で sweep
- SAVE_VERSION 8 → 9 + fixture regen
- 3 regression tests in `tests/ai_npc_no_double_survey_assignment.rs`

PR #1 と並列で landed (file 干渉なし)。 SAVE_VERSION の重複 bump は **統合時に 9 → 10 に再 bump** で解決。

### Follow-up: PendingAssignment knowledge-driven cleanup

Commit `7ad059a`。 PR #2 の lifetime が短すぎる (handler `Ok` で remove → 完了直後に NPC が再 emit) のを修正。

#### 修正

- `handle_survey_requested` の `Ok` 分岐から `remove::<PendingAssignment>` を削除 (Rejected と ship-unavailable は残す)
- 新 system `sweep_resolved_survey_assignments`: 自 faction の `KnowledgeStore` を query → `assignment.target` の `surveyed=true` を検知したら remove
- `SURVEY_ASSIGNMENT_LIFETIME` 90 → **200 hexadies** (worst-case 光速 propagation + survey + slack)
- 失敗 path (ship lost): Bevy 自動 component cleanup + stale fallback (将来 `KnowledgeFact::ShipDestroyed` 検知に拡張可能)

#### 哲学

PendingAssignment は NPC の **意思決定 memory**。 KnowledgeStore (光速遅延込み観測) とは別 layer。 「自分が target X に scout を送り込んだ」を覚えてないと NPC は重複命令を発行する。 marker は **「成功 or 失敗を *knowledge として* 知った」** タイミングで dissolve するのが意味的に正しい。

#### 既存 regression test

`tests/ai_npc_no_double_survey_assignment.rs::ai_does_not_double_assign_two_ships_to_same_survey_target` は eager-remove 前提で書かれてたので **`#[ignore]`** で skip。 lifetime 延長後の挙動に test を rewrite するのが TODO。

### Follow-up: NPC ship auto-return per-empire

Commit `b0ea993`。 `process_surveys` の carry-back path の auto-return が **player の StationedAt 専用**だったため、 NPC scout は survey 完了後 home に戻れず target に居続けてた問題。

#### 修正

`Empire → EmpireRuler → Ruler.StationedAt` chain で per-ship-owner home system を解決。 legacy fallback (`player_system`) で既存 test (`Player+StationedAt` 直接 attach、 `Owner::Neutral` ship) も動作。

`process_surveys` に `empire_rulers` + `rulers_stationed` query 追加 (15 → 17 SystemParam だが余裕あり)。

### Follow-up: deliver_survey_results per-empire

Commit `515e7cf`。 NPC scout が own home に帰投できるようになった (b0ea993) が、 `deliver_survey_results` も player 専用 (`empire_q: Query<&mut KnowledgeStore, With<PlayerEmpire>>`) で NPC ship が docked しても **deliver されず、 `star.surveyed=true` 永遠に立たない** 問題。

#### 修正

- `empire_q` を `Query<&mut KnowledgeStore, With<Empire>>` に拡張
- ship.owner の home (Empire→Ruler chain) で deliver を gate
- 自軍は ship が物理的に持ち帰った data なので **own KnowledgeStore は即時 update** (光速遅延なし)
- 他 faction には `record_for` 経由で home 起点 light-speed delay

これで survey full chain が NPC でも完結:

1. **dispatch**: NPC AI emit `survey_system` → `handle_survey_system` で `PendingAssignment` insert + `SurveyRequested` 発火
2. **travel**: handler の auto re-insert で MoveTo + Survey → ship が target へ移動
3. **survey**: 30 hexadies で `process_surveys` 完了 → carry-back path で `SurveyData` insert
4. **auto-return**: own home へ MoveTo queue (`b0ea993`)
5. **deliver**: home docking で `deliver_survey_results` が `star.surveyed=true` + own KnowledgeStore 即時 update (`515e7cf`)
6. **memory dissolve**: `sweep_resolved_survey_assignments` が own KnowledgeStore.surveyed=true 検知 → `PendingAssignment` remove (`7ad059a`)
7. **next**: NPC AI が次 unsurveyed target を pick → 1 へ

### Follow-up: visualization viewer-aware ruler

Commit `7afda7f`。 observer mode で `draw_galaxy_overlay` の capital pulse ring が **galaxy 内最初に見つかる capital** に表示される (player_q 空 → `stars.iter().find(is_capital)` fallback) → viewer empire と無関係な capital が表示されるバグ。

#### 修正

`ViewerRulerLookup` SystemParam bundle (empire_rulers + rulers_stationed) を追加、 viewer empire の Ruler StationedAt から正しい system 解決。 `player_q` 削除 (#421 unified player + NPC under Ruler、 chain 経由で両方解決可能)。 SystemParam 16 limit 回避。

### Round 9 完了基準

- [x] Reflect 全 derive (BRP query 動く)
- [x] knowledge per-faction generalize (NPC empire の KnowledgeStore に fact 配信)
- [x] PendingAssignment dedup (同 target に複数 ship 割当 race 止め)
- [x] PendingAssignment knowledge-driven cleanup (NPC 意思決定 memory が success knowledge 到達まで持続)
- [x] NPC ship auto-return generalize (NPC scout が own home に帰還)
- [x] deliver_survey_results per-empire (NPC scout が home docking で fact deliver)
- [x] visualization viewer-aware ruler (pulse ring が viewer empire の capital に)
- [ ] **AI command 光速遅延 (Bug 2、 PR #3)** — 未着手

## 残課題 (次セッション以降)

### 高優先

1. **Round 9 PR #3: AI command 光速遅延統合** (Plan agent draft 済、 `docs/session-handoff-2026-04-25-ai-three-layer.md` の Round 9 候補参照)
   - `AiCommandOutbox` Resource + `dispatch_ai_pending_commands` system (interpose で `bus.emit_command` を遅延化)
   - `process_ai_pending_commands` system が `arrives_at` 到達で bus に re-push (cycle-safety: 「already_delayed」marker か別 channel)
   - 既存 `compute_fact_arrival` 再利用で relay-aware delay
   - `npc_decision_tick` と `run_orchestrators` 両方の emit を経由
   - SimpleNpcPolicy ↔ orchestrator は並列維持 (Round 10+ 判断)

2. **`tests/ai_npc_no_double_survey_assignment.rs` rewrite** — 現在 `#[ignore]`。 ships を Surveying → home docking → KnowledgeStore arrival まで drive してから assertion する形に。

3. **player-only 残り callsites の per-faction 化**:
   - `combat.rs`、 `colonization.rs`、 `scout.rs`、 `pursuit.rs` など (PR #1 で 8 callsite migrate 済だが他にも `Player` query や `PlayerEmpire` filter が残る可能性)
   - grep `with::<Player\|PlayerEmpire>` で systematic に洗う

### 中優先

4. **NPC `CommsParams` を `compute_fact_arrival` に渡す** (Round 9 PR #1 の TODO)
5. **`KnowledgeFact::ShipDestroyed` 配信 → `sweep_resolved_survey_assignments` が失敗 path も処理** (現状 Bevy 自動 cleanup + stale fallback のみ)

### 低優先 (architectural)

6. **Long の `StrategyCandidate` 機構** (Round 7 で露出した本質的課題、 concentrate vs distribute 等の 戦略空間拡張)
7. **NPC own knowledge propagation の light-speed accuracy** (現状 own は 0 delay、 game design 的にどこまで厳格にするか)

## 観察手順 (BRP via remote feature)

ゲーム起動: `cargo run --features remote`

### 基本 query

```bash
# 現在の game clock
curl -s http://localhost:15702 -X POST -H "Content-Type: application/json" -d '{
  "jsonrpc":"2.0","id":1,
  "method":"world.get_resources",
  "params":{"resource":"macrocosmo::time_system::GameClock"}
}'

# advance time
curl -s http://localhost:15702 -X POST -H "Content-Type: application/json" -d '{
  "jsonrpc":"2.0","id":1,
  "method":"macrocosmo/advance_time",
  "params":{"hexadies":60}
}'

# 全 ship + state + queue (loop 観察用)
curl -s http://localhost:15702 -X POST -H "Content-Type: application/json" -d '{
  "jsonrpc":"2.0","id":1,
  "method":"world.query",
  "params":{"data":{"components":[
    "macrocosmo::ship::Ship",
    "macrocosmo::ship::ShipState",
    "macrocosmo::ship::CommandQueue"
  ]}}
}'

# Ruler の現在位置
curl -s http://localhost:15702 -X POST -H "Content-Type: application/json" -d '{
  "jsonrpc":"2.0","id":1,
  "method":"world.query",
  "params":{"data":{"has":["macrocosmo::player::Ruler"]}}
}'
# 各 ruler entity ID を取って world.get_components で StationedAt 取得

# PendingAssignment markers
curl -s http://localhost:15702 -X POST -H "Content-Type: application/json" -d '{
  "jsonrpc":"2.0","id":1,
  "method":"world.query",
  "params":{"data":{"components":["macrocosmo::ai::assignments::PendingAssignment"],
    "filter":{"with":["macrocosmo::ai::assignments::PendingAssignment"]}}}
}'

# 全 system の surveyed 状況
curl -s http://localhost:15702 -X POST -H "Content-Type: application/json" -d '{
  "jsonrpc":"2.0","id":1,
  "method":"world.query",
  "params":{"data":{"components":["macrocosmo::galaxy::StarSystem"]}}
}'
```

### Round 9 動作確認ポイント

1. **NPC が own KnowledgeStore に survey 完了 fact を持つ** — empire entity の KnowledgeStore.system_knowledge が NPC ship survey 完了後に増加
2. **NPC scout が survey 完了後 own home に帰還** — ship state が `Surveying → InSystem(target) → SubLight to own_home → InSystem(own_home)` と推移
3. **`star.surveyed=true` が NPC ship でも立つ** — `world.query` の StarSystem で surveyed=true な数が NPC survey 完了後増加
4. **PendingAssignment の lifetime** — survey 命令から home docking + knowledge arrival までは持続、 その後 sweep
5. **同じ target に複数 ship 割当が起こらない** — SubLight 中の ship の target が全 distinct
6. **observer mode top-bar empire 切替で pulse ring が移動** — viewer empire の capital に表示

## 次セッションの再開プロンプト例

```
Round 9 続き。 まず docs/session-handoff-2026-04-26-round-7-9.md 読んで全体像把握。
今日は Round 9 PR #3 (AI command 光速遅延、 AiCommandOutbox + dispatch_ai_pending_commands)
を着手する。 Plan は前ハンドオフ docs/session-handoff-2026-04-25-ai-three-layer.md の
Round 9 候補参照。 既に PR #1 + PR #2 + follow-up が landed、 BRP で survey full chain
が NPC でも完結することを確認済。
```

または:

```
昨日 Round 9 で knowledge per-faction generalize と PendingAssignment dedup を
landed (docs/session-handoff-2026-04-26-round-7-9.md 参照)。 今日は残課題の中の
「player-only callsite の systematic 洗い出し」 を進める。 grep で
With<Player|PlayerEmpire> を全洗いして、 NPC perspective 漏れを修正。
```

## 注意点 / 落とし穴

- **agent worktree 失敗多発**: Round 9 で 3 並列 agent 全てが worktree 指定だったが、 実際は main checkout に書いてしまうケースあり (memory `feedback_agent_worktree_check` 該当)。 完了通知後に必ず `git status` + `git log` 確認、 cherry-pick 前に worktree branch base を `git merge-base` で確認する習慣をつける。
- **SAVE_VERSION の同時 bump 衝突**: 並列 PR が両方 `SAVE_VERSION = N` に bump すると cherry-pick で conflict。 統合時に `N+1` に再 bump + fixture regen する必要 (今回 8 → 10 の流れ)。
- **flaky test 1 件**: `apply_pending_acks_system_drains_buffer_and_acks_queue` (notifications_tab) が parallel test execution で稀に fail。 isolation で pass。 pre-existing、 今日の修正と無関係。
- **`process_surveys` の SystemParam が肥大** — 既に 13 + bundle で限界近い。 次に追加するなら別 bundle 化必須。
- **Ruler は Position component を持たない** — `StationedAt.system` から system entity の Position を引く (visualization、 fact_sys、 deliver で全部この pattern)。 BRP query で Ruler の Position 取れないのは仕様。
- **observer mode と --no-player の差**: `--observer` は player empire entity を残しつつ AI controlled、 `--no-player` は player empire 自体を spawn しない。 今日のセッションでユーザーは観察 mode を使ってたが ObserverMode.enabled=true + PlayerEmpire entities=0 だったので実質 `--no-player` 相当。
- **Reflect 全 derive で `Box<T>` の reflection 制限** — `Condition::Not(Box<Condition>)` 等は outer enum tag だけ visible、 内部は opaque。 BRP からネスト構造は完全には見えない。
