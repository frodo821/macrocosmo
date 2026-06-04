# Refactor Plan: 2026-05-27 SRP / DRY / Crate Boundaries

## Scope

コード複雑化の抑制を目的に、単一責務原則、DRY、crate 分割境界の観点で作業候補を整理する。

Reviewed areas:

- `macrocosmo/src/ai/command_consumer.rs`
- `macrocosmo/src/ai/emitters.rs`
- `macrocosmo/src/ai/command_outbox.rs`
- `macrocosmo/src/persistence/savebag.rs`
- `macrocosmo/src/ui/mod.rs`
- `macrocosmo/src/knowledge/mod.rs`
- `macrocosmo/src/colony/system_buildings.rs`
- workspace / crate manifests

No code changes were made during the review.

## Current Shape

Workspace は現在 2 crate 構成。

- `macrocosmo`: Bevy app 本体。ECS components/systems、UI、Lua scripting、save/load、game rules、AI adapter を全て含む。
- `macrocosmo-ai`: engine-agnostic AI core。既存設計契約どおり、`macrocosmo` / `bevy` へ依存しない。

この依存方向は維持する。

```text
macrocosmo  ->  macrocosmo-ai
macrocosmo-ai  -X->  macrocosmo / bevy
```

当面の問題は `macrocosmo` 内に Bevy 非依存で扱える型・ルールと、Bevy ECS system / UI / Lua / persistence が混在していること。crate 分割を考えるなら、まずこの混在を module レベルで解消してから、安定した境界だけを crate 化するのが安全。

## Findings

### 1. AI command consumer is doing dispatch, parsing, and domain mutation

Priority: High  
Candidate files:

- `macrocosmo/src/ai/command_consumer.rs`
- `macrocosmo/src/ai/command_outbox.rs`
- `macrocosmo/src/ai/decomposition_rules.rs`

`drain_ai_commands` は AI bus から command を drain し、kind 文字列で分岐し、params を解釈し、build / research / retreat / deliverable などの実処理まで行っている。`CommandParams` の文字列キー (`target_system`, `design_id`, `building_id`, `ship_count`, `ship_0` など) も複数モジュールに散っている。

Refactor proposal:

1. `ai/command_params.rs` を追加し、typed accessor を集約する。
   - `target_system(params) -> Option<Entity>`
   - `required_str(params, key) -> Result<&str, CommandParamError>`
   - `ship_list(params) -> Vec<Entity>`
2. `ai/command_handlers/` を作る。
   - `build.rs`: `build_ship`, `fortify_system`, `build_deliverable`
   - `research.rs`: `research_focus`
   - `military.rs`: `retreat`
   - `structure.rs`: `build_structure`
3. `drain_ai_commands` は dispatch と lifecycle のみに縮小する。
4. stale ship-control kinds / macro command drop などの分類は `CommandRoute` enum へ移す。

Expected effect:

- `command_consumer.rs` の責務が「bus drain + route」に縮む。
- command param schema の重複が減る。
- handler 単位の focused test が書きやすくなる。

Do not split into a new crate yet. Bevy `Query`, `SystemParam`, `MessageWriter` への依存が強く、module 分割の方が低リスク。

### 2. Economic AI metrics mix aggregation with emission

Priority: High  
Candidate file:

- `macrocosmo/src/ai/emitters.rs`

`emit_economic_metrics` は colony production、population、food、territory、stockpile、infrastructure、technology を集計し、その場で `AiBusWriter` に emit している。SRP 的には「ECS から snapshot を作る」「snapshot を AI metric schema に変換する」が混ざっている。

Refactor proposal:

1. `ai/metrics/economy.rs` を追加する。
2. Pure data structs を作る。
   - `EmpireProductionSnapshot`
   - `EmpirePopulationSnapshot`
   - `EmpireStockpileSnapshot`
   - `EmpireInfrastructureSnapshot`
   - `EmpireEconomicSnapshot`
3. ECS query を読む system は snapshot 生成に集中する。
4. `emit_economic_snapshot(writer, faction, snapshot)` が metric 名へ変換する。

Expected effect:

- 集計値の unit test が bus なしで書ける。
- metric 名変更と経済集計変更を別々に扱える。
- 将来 `macrocosmo-rules` crate へ移せる候補が明確になる。

### 3. Savebag is a single high-conflict persistence module

Priority: High  
Candidate file:

- `macrocosmo/src/persistence/savebag.rs`

`savebag.rs` は 5500 行超で、全 domain の `Saved*` wire struct と `from_live` / `into_live` を保持している。既存 architecture decision の「wire format mirror approach」は妥当だが、1 ファイル集約は編集衝突と責務肥大が大きい。

Refactor proposal:

1. `persistence/savebag/` ディレクトリへ分割する。
   - `mod.rs`: top-level `SavedComponentBag` と re-export
   - `helpers.rs`: `remap_entity` など
   - `galaxy.rs`
   - `ship.rs`
   - `colony.rs`
   - `faction.rs`
   - `knowledge.rs`
   - `ai.rs`
   - `events.rs`
2. `SavedComponentBag` の field 名と postcard schema は維持する。
3. 分割 commit では wire format を変えない。
4. fixture test を必ず同時に走らせる。

Expected effect:

- domain 追加時の編集箇所が局所化する。
- save/load の設計契約は維持したまま、レビュー単位が小さくなる。

Crate split note:

- persistence は `bevy::Entity` remap と live ECS component に強く依存するため、当面 `macrocosmo` 内でよい。
- 将来、wire structs だけを `macrocosmo-save` に出す案はあるが、live component 変換を同 crate に入れると `macrocosmo-save -> macrocosmo` が必要になり循環しやすい。やるなら `macrocosmo-core` を先に切る必要がある。

### 4. UI mod.rs still owns too many UI-wide concerns

Priority: Medium  
Candidate file:

- `macrocosmo/src/ui/mod.rs`

`ui/mod.rs` は plugin setup、font setup、observer view 解決、knowledge view 解決、UI state 計算、通知、main panels、overlays、console、choice dialog を保持している。子 module は既にあるが、root module がまだ大きい。

Refactor proposal:

1. `ui/plugin.rs`: `UiPlugin` registration
2. `ui/font.rs`: CJK font setup + font tests
3. `ui/view_context.rs`: `resolve_ui_empire`, `resolve_viewing_knowledge`, omniscient handling
4. `ui/state.rs`: `UiState`, `compute_ui_state`, observer variant
5. `ui/notifications_panel.rs`: notification pill drawing

Expected effect:

- UI の表示ロジックと observer/knowledge contract が分離される。
- future UI changes が root module を汚しにくくなる。

Crate split note:

- UI は Bevy + egui + project-specific ECS に密結合しているため、crate 分割対象ではない。

### 5. System-building capability lookup is repeated and sometimes rebuilt per call

Priority: Medium  
Candidate files:

- `macrocosmo/src/colony/system_buildings.rs`
- `macrocosmo/src/ai/emitters.rs`
- `macrocosmo/src/colony/production.rs`
- `macrocosmo/src/deep_space/mod.rs`

`build_reverse_design_map` が複数箇所で使われ、station ship 走査も似た形が複数ある。現状は小さな重複だが、shipyard / port / core など system capability の意味が複数 domain に広がっている。

Refactor proposal:

1. `SystemBuildingIndex` resource を導入する。
   - `design_to_building: HashMap<String, BuildingId>`
   - capability lookup helper
2. registry load / script reload 後に index を再構築する。
3. station ship 走査 helper を iterator で提供する。
   - `station_buildings_at_system(system, ships, index, registry)`
   - `station_capabilities_by_owner(ships, index, registry)`

Expected effect:

- reverse map 再構築の重複が消える。
- shipyard/port 判定の基準が一箇所に寄る。

Crate split note:

- registry definition 側は Bevy 非依存化しやすいが、station ship 走査は ECS に依存する。まず index の data shape だけ pure に保つ。

### 6. Knowledge module has multiple subdomains in one mod.rs

Priority: Medium  
Candidate file:

- `macrocosmo/src/knowledge/mod.rs`

`knowledge/mod.rs` には vantage collection、ship projection、visibility tier、snapshot build、propagation、destroyed-ship update、delayed combat event drain、production knowledge が同居している。

Refactor proposal:

1. `knowledge/vantage.rs`
2. `knowledge/projection.rs`
3. `knowledge/visibility.rs`
4. `knowledge/snapshot.rs`
5. `knowledge/propagation.rs`
6. `knowledge/combat_events.rs`

Expected effect:

- light-speed read model の契約が読みやすくなる。
- UI / AI / scripting がどの knowledge API に依存しているか追いやすくなる。

Crate split note:

- `KnowledgeStore` の data model は将来 `macrocosmo-core` 候補。
- propagation system は Bevy ECS と game world query に依存するため `macrocosmo` に残す。

## Crate Boundary Options

### Proposed Boundary: core / simulation / interactions

User-proposed split:

```text
macrocosmo-core
  ↑
macrocosmo-simulation
  ↑
macrocosmo-interactions
  ↑
macrocosmo binary

macrocosmo-ai -> macrocosmo-core  (only if the shared contract is clean)
```

Assessment: this is a good target boundary, but it should be staged. It maps better to the current codebase than a generic `domain/save/ui` split, because it separates "what the game is" from "how the game advances" from "how a human observes/controls it."

The main caveat is that `simulation` will still depend on Bevy for the foreseeable future. Current components, resources, plugins, schedules, save/load, scripting, and knowledge propagation are all ECS-shaped. That is acceptable; the high-value boundary is not "no Bevy outside the binary", but "no rendering/input/UI dependencies inside simulation or core."

#### `macrocosmo-core`

Purpose:

- Shared game concepts and pure rules that both game simulation and AI can depend on.
- No Bevy, egui, mlua, postcard, or rendering/window dependencies.

Candidate contents:

- Stable IDs and value types that are not ECS-specific.
- Amount / scalar math, if reflect/component needs are kept in adapters.
- Definition data shapes:
  - buildings / capabilities
  - ship hulls, modules, ship designs
  - technologies / unlock data
  - deliverable definitions
  - conditions / effects, only after Lua/ECS adapters are separated
- Pure calculators:
  - ship design derived stats/costs
  - tech unlock index construction
  - building capability indexing
  - economic snapshot arithmetic
  - non-ECS portions of light-delay math

Potential AI usage:

- `macrocosmo-ai` may depend on `macrocosmo-core` if it removes duplicated domain concepts without importing engine/runtime details.
- Good candidates are stable IDs, definition metadata, and pure scoring inputs.
- Bad candidates are ECS entities, Bevy resources, `World`, `Query`, UI-facing view structs, and save wire structs.

Dependency rule:

```text
macrocosmo-core -> serde/log/small utility deps only
macrocosmo-core -X-> bevy / bevy_egui / mlua / postcard / macrocosmo-simulation
```

Benefit:

- Lets AI and simulation share vocabulary without coupling AI to Bevy.
- Gives pure unit tests a small compile target.
- Creates a stable place for rules that currently leak across UI, AI emitters, scripting, and simulation.

Risk:

- Moving too much too early creates adapter churn.
- Existing ECS components often combine domain data with Bevy derives; those need wrapper/adaptation before moving.

#### `macrocosmo-simulation`

Purpose:

- Owns the authoritative game world, ECS components/resources/systems, game loop, content loading, command processing, knowledge propagation, save/load adapters, and AI integration.
- May depend on Bevy and `macrocosmo-ai`.
- Must not depend on UI, egui, visualization, input, remote testing UI affordances, or window-specific logic.

Candidate contents from current `macrocosmo/src`:

- `ai`
- `casus_belli`
- `choice` data and pending-choice state; UI rendering moves to interactions
- `colony`
- `communication`
- `components`
- `condition` runtime evaluation, unless pure condition model moves to core
- `deep_space`
- `effect`
- `empire`
- `event_system`
- `events`
- `faction`
- `galaxy`
- `game_state`
- `knowledge`
- `modifier`
- `negotiation`
- `notifications` data/queue/auto-notify; drawing moves to interactions
- `persistence`
- `physics`
- `player`
- `region`
- `scripting`
- `setup`
- `ship`
- `ship_design` runtime registry/adapters, with pure calculation possibly in core
- `species`
- `technology`
- `time_system`
- `reflect_registration` only if still needed for simulation-side BRP type registration; otherwise interactions/remote owns it

Dependency rule:

```text
macrocosmo-simulation -> macrocosmo-core
macrocosmo-simulation -> macrocosmo-ai
macrocosmo-simulation -> bevy / mlua / postcard
macrocosmo-simulation -X-> bevy_egui / UI modules / visualization modules / input keybinding UI
```

Benefit:

- Headless simulation and AI tests can compile without egui/rendering/input.
- The game loop becomes testable as a product independent from the human interface.
- Clearer ownership for AI command consumers, knowledge propagation, save/load, and scripting.

Risk:

- Current `time_system` includes speed controls, which read user input semantics. It should split into clock advancement in simulation and speed-control input in interactions.
- `observer` is mixed: observer mode configuration affects setup/simulation, but keybindings and UI toggles are interactions. It likely needs a small simulation-side resource plus interaction-side controls.
- `notifications` is mixed: queue/event production belongs to simulation; pill rendering belongs to interactions.

#### `macrocosmo-interactions`

Purpose:

- Human and tool-facing interaction layer: rendering, UI, selection, camera, keybindings, remote control, observer controls.
- Depends on simulation public API and Bevy rendering/egui/input features.
- Does not own authoritative game rules.

Candidate contents from current `macrocosmo/src`:

- `ui`
- `visualization`
- `input`
- `remote`
- observer CLI / controls / UI-facing observer state, after simulation-facing config is split
- presentation portions of `notifications`
- presentation portions of `choice`

Dependency rule:

```text
macrocosmo-interactions -> macrocosmo-simulation
macrocosmo-interactions -> macrocosmo-core
macrocosmo-interactions -> bevy_egui / rendering / window / input deps
macrocosmo-interactions -X-> macrocosmo-ai directly, unless an AI debug view needs a stable simulation API
```

Benefit:

- UI and visualization churn stops increasing the simulation crate surface.
- A headless binary or simulation test harness becomes realistic.
- Remote/BRP automation can be treated as an interaction frontend instead of leaking into core simulation.

Risk:

- Current UI reads raw ECS queries heavily. The first split can allow that through `macrocosmo-simulation` public component types, but over time the better direction is simulation-owned view/query helpers.
- `ui/ai_debug` currently reads `AiBusResource`; this should go through simulation-owned debug snapshots rather than a direct `macrocosmo-ai` dependency if possible.

#### Binary crate

The final `macrocosmo` binary can become mostly composition:

```rust
app.add_plugins(DefaultPlugins);
app.add_plugins(macrocosmo_simulation::SimulationPlugin);
app.add_plugins(macrocosmo_interactions::InteractionsPlugin);
```

Feature flags can choose frontends:

- default desktop: simulation + interactions
- headless: simulation only
- remote: interactions + BRP remote
- test harness: simulation with minimal Bevy plugins

### Does This Split Have Real Value?

Yes, but only if `core` remains small and pure, and `interactions` is kept out of simulation.

Concrete benefits for this codebase:

- The biggest current files are not all UI files; many are simulation systems. A `simulation` crate gives those systems a cleaner home without forcing every cleanup to also touch UI/build/render dependencies.
- AI integration is currently spread between `macrocosmo-ai` and Bevy-facing adapters. A shared `core` can reduce duplicated IDs / definitions / rule math while preserving AI isolation.
- UI and visualization currently import many game modules directly. Moving them to `interactions` makes that dependency direction explicit and prevents simulation from reaching back into UI.
- Headless and regression testing should become cheaper: save/load, AI, knowledge, command dispatch, and economic ticks do not need egui or render plugins.

Costs:

- Initial crate split will expose many accidental dependencies, especially around observer mode, notifications, choice dialogs, and time speed controls.
- Public API pressure increases. Types that are currently `pub` only inside one crate may need deliberate exports.
- Compile times may improve for targeted tests, but total workspace compile may not improve immediately because `simulation` still uses Bevy.
- If `core` tries to own ECS components directly, the split loses most of its value.

Net: worth planning toward. Do not do it as the first refactor. First create the seams inside the current crate, then move modules into crates.

### Revised Target Crate Plan

Preferred eventual graph:

```text
macrocosmo-core
  ├─ pure ids / definitions / rule calculators
  └─ no engine/runtime dependencies

macrocosmo-ai
  └─ optionally depends on macrocosmo-core for shared pure contracts

macrocosmo-simulation
  ├─ depends on macrocosmo-core
  ├─ depends on macrocosmo-ai
  ├─ owns ECS world, simulation systems, scripting, persistence adapters
  └─ no UI / input / rendering dependencies

macrocosmo-interactions
  ├─ depends on macrocosmo-simulation
  ├─ depends on macrocosmo-core
  └─ owns UI, visualization, input, remote, observer controls

macrocosmo binary
  └─ composes plugins and feature flags
```

Important nuance: `macrocosmo-ai` should not be forced to depend on `macrocosmo-core` in Phase 1. Let duplication remain until a concrete shared type proves stable. Once `macrocosmo-core` has Bevy-free IDs/definitions that AI genuinely needs, add the dependency deliberately.

### Option A: Keep current crates, refactor modules first

Recommended first step.

```text
macrocosmo
  ├─ ai/
  ├─ colony/
  ├─ knowledge/
  ├─ persistence/
  ├─ scripting/
  ├─ ui/
  └─ ...

macrocosmo-ai
```

Pros:

- Lowest risk.
- No dependency graph churn.
- Faster iteration while responsibilities are still moving.

Cons:

- Compile boundary stays coarse.
- Bevy-free domain code can still accidentally depend on Bevy unless disciplined.

Use this until the module boundaries stop changing.

### Option B: Add `macrocosmo-core`

Best candidate once module refactors stabilize. This supersedes the earlier generic `macrocosmo-domain` name; `core` is a better fit if the crate is intentionally shared by simulation and AI.

Purpose:

- Bevy-free, UI-free, Lua-free, persistence-format-free domain types and pure rules.
- Shared by game runtime, save wire conversion, tests, and possibly tooling.

Potential contents:

- Amount types if decoupled from Bevy reflect needs.
- IDs and definition structs: buildings, ship designs, technologies, deliverables.
- Conditions / effects data model if `macrocosmo` and scripts can convert at boundary.
- Pure calculators:
  - ship design computed stats
  - building capability definitions
  - tech unlock indexes
  - economic snapshot math that does not require ECS queries

Dependency direction:

```text
macrocosmo-simulation -> macrocosmo-core
macrocosmo-ai         -> macrocosmo-core  (optional, after a concrete shared need)
macrocosmo-core       -X-> macrocosmo-simulation / macrocosmo-ai / bevy
```

Do not make `macrocosmo-ai` depend on `macrocosmo-core` initially. AI core already has engine-agnostic IDs, conditions, bus schema, and projections. Merging those domains prematurely could blur the clean AI isolation contract.

Migration guardrails:

- Core crate must not depend on Bevy, bevy_egui, mlua, postcard, simulation, interactions, or macrocosmo binary code.
- Prefer `serde` only where the data format is truly domain-owned.
- Avoid moving ECS components directly; create domain structs first, then wrap/adapt in `macrocosmo`.

### Option C: Add `macrocosmo-scripting`

Not recommended yet.

Lua integration is deeply tied to `World`, registries, lifecycle hooks, and effect application. A scripting crate would likely need to depend on `macrocosmo-core`, but most useful functions still require Bevy `World` access. Splitting now would add dependency complexity without reducing much local complexity.

Possible future shape:

```text
macrocosmo-scripting -> macrocosmo-core
macrocosmo           -> macrocosmo-scripting
```

Only revisit after core definitions have moved out.

### Option D: Add `macrocosmo-save`

Not recommended before `macrocosmo-core`.

Save wire structs mirror live components. If live components remain in `macrocosmo`, a save crate either duplicates too much or depends back on `macrocosmo`, which defeats the split.

Possible future shape:

```text
macrocosmo-save -> macrocosmo-core
macrocosmo      -> macrocosmo-save
macrocosmo      -> macrocosmo-core
```

`macrocosmo-save` should own only stable wire structs and versioned migration logic. Live ECS conversion should remain in `macrocosmo` or a thin adapter module to avoid cyclic dependency.

### Option E: Add `macrocosmo-interactions`

Recommended eventually, but only after `macrocosmo-simulation` exists.

Splitting UI alone is less useful than splitting the entire interaction layer. The target should include UI, visualization, input, remote, and observer controls together so the dependency boundary means "human/tool interface" rather than only "egui code."

## Recommended Work Plan

### Phase 1: Prioritize simulation / interactions separation in-place

Goal: stop rendering/input fixes from perturbing the authoritative game loop, and stop simulation/game-loop fixes from perturbing rendering. This phase keeps the crate graph unchanged and creates an internal boundary first.

1. Add internal top-level module groups:
   - `simulation::*`
   - `interactions::*`
2. Introduce `SimulationPlugin`.
   - Owns game state, setup, time clock advancement, galaxy, player/empire state, colony, ship, knowledge, scripting, technology, events, notifications data, AI integration, persistence, and command processing.
   - Does not install UI, egui, visualization, input keybindings, remote/BRP, camera, or window interaction systems.
3. Introduce `InteractionsPlugin`.
   - Owns UI, visualization, input, camera/selection, remote/BRP, observer controls, notification presentation, choice dialogs, and AI debug presentation.
   - Depends on simulation public resources/components but must not own authoritative rule mutation.
4. Make `main.rs` compose plugins in this order:

```text
DefaultPlugins
SimulationPlugin
InteractionsPlugin
```

5. Split the mixed modules before moving them:
   - `time_system`: keep `GameClock` and `advance_game_time` in simulation; move speed key handling to interactions.
   - `observer`: keep startup config and read-only/sim mode resources in simulation; move toggles/keybindings/UI-facing controls to interactions.
   - `notifications`: keep queue and event-to-notification production in simulation; move pill rendering to interactions.
   - `choice`: keep pending-choice state and resolution in simulation; move dialog rendering/input to interactions.
6. Add a headless smoke test that starts only `SimulationPlugin` with `MinimalPlugins` and advances at least one update without `UiPlugin`, egui, visualization, or input.
7. Add dependency/import checks for the internal boundary:

```text
rg "bevy_egui|egui::|crate::ui|crate::visualization|crate::input|KeyCode|ButtonInput|Window" macrocosmo/src/simulation
```

The check should return no production imports, except explicitly documented temporary shims.

Validation:

- `cargo test -p macrocosmo --lib`
- A new headless simulation smoke test.
- Existing targeted tests around save/load, AI command dispatch, knowledge propagation, and event/notification production.
- A desktop smoke/manual check that `SimulationPlugin + InteractionsPlugin` still renders.

Success criteria:

- `SimulationPlugin` can run without `InteractionsPlugin`.
- Interactions can be removed from the app without breaking game-loop setup.
- No simulation module imports egui/UI/input/rendering symbols.
- Rendering systems read simulation state or send simulation commands; they do not become owners of simulation lifecycle.

### Phase 2: Module-level SRP/DRY cleanup inside each side

Goal: reduce complexity after the simulation/interactions boundary exists.

1. On the simulation side:
   - Add `ai/command_params.rs` and move all command param accessors there.
   - Split `ai/command_consumer.rs` into command handler modules.
   - Split `ai/emitters.rs` economic aggregation into snapshot structs and emit functions.
   - Split `persistence/savebag.rs` by domain without changing postcard schema.
   - Split `knowledge/mod.rs` by subdomain.
2. On the interactions side:
   - Split `ui/mod.rs` into plugin/font/view_context/state modules.
   - Keep UI draw systems behind `InteractionsPlugin`.
   - Move observer UI controls and notification rendering out of simulation-facing modules.

Validation:

- `cargo test -p macrocosmo-ai`
- `cargo test -p macrocosmo --lib`
- Targeted integration tests around AI command dispatch, save/load fixtures, UI/observer knowledge, and knowledge propagation.

### Phase 3: Prepare pure core seams

Goal: identify code that can move to a future `macrocosmo-core` crate.

1. Add module-level dependency rules in comments or docs:
   - pure data/calculation modules must not import `bevy::prelude::*`
   - ECS systems live in `systems` or adapter modules
2. Move pure calculators behind non-ECS APIs.
3. Introduce conversion/adaptation boundaries where ECS components currently double as domain data.
4. Add `rg "bevy::|mlua|postcard"` checks for candidate pure modules.

Candidate pure modules after cleanup:

- ship design stat calculation
- building capability definition/index data
- technology unlock indexing
- condition/effect data model
- AI economic snapshot math

### Phase 4: Add `macrocosmo-core` only after seams are stable

Goal: crate split with minimal churn.

1. Create `macrocosmo-core` with only Bevy-free data and pure functions.
2. Move one low-risk domain first, likely ship design calculation or technology unlock indexing.
3. Make the current `macrocosmo` crate adapt ECS/resource wrappers around core types.
4. Keep `macrocosmo-ai` independent unless there is a concrete duplication problem that justifies a shared domain dependency.
5. Add a CI guard:

```text
cargo tree -p macrocosmo-core
```

and verify it does not include Bevy, mlua, postcard, or macrocosmo.

### Phase 5: Add `macrocosmo-simulation`

Goal: move authoritative game runtime to its own crate.

1. Create `macrocosmo-simulation`.
2. Move the `simulation` module tree into it.
3. Export a `SimulationPlugin` that installs game state, setup, ECS systems, scripting, persistence, AI integration, knowledge, and events.
4. Keep the old `macrocosmo` crate as the desktop binary/composition crate.
5. Add a headless test harness that starts `SimulationPlugin` without `UiPlugin`, egui, visualization, or input plugins.

Dependency guard:

```text
cargo tree -p macrocosmo-simulation
rg "bevy_egui|egui::|WindowPlugin|bevy_winit" macrocosmo-simulation/src
```

### Phase 6: Add `macrocosmo-interactions`

Goal: move frontends out of the binary crate.

1. Create `macrocosmo-interactions`.
2. Move `interactions` module tree into it.
3. Export `InteractionsPlugin`.
4. Make the desktop binary compose:

```text
SimulationPlugin
InteractionsPlugin
```

5. Move remote/BRP feature wiring into interactions where possible.
6. Replace direct `macrocosmo-ai` reads in UI debug surfaces with simulation-owned debug snapshots where practical.

## Anti-Goals

- Do not split crates just to reduce file size.
- Do not move Bevy ECS components into `macrocosmo-core` unless they are first decoupled from Bevy-specific derives/resources.
- Do not make `macrocosmo-ai` depend on `macrocosmo` or Bevy.
- Do not change save wire format during mechanical file splits.
- Do not refactor gameplay behavior and module boundaries in the same commit unless a test pins the behavior.

## Suggested First PR

Title:

```text
refactor(app): introduce simulation and interactions plugin boundary
```

Scope:

- Add `simulation` and `interactions` module entry points inside the existing `macrocosmo` crate.
- Move plugin composition out of `main.rs` into `SimulationPlugin` and `InteractionsPlugin`.
- Keep existing modules in place initially; use re-exports or thin plugin wrappers to avoid a huge move-only diff.
- Move only the safest mixed concern first: split `time_system` so clock advancement is simulation-owned and speed-control input is interaction-owned.
- Add a headless simulation smoke test that installs `SimulationPlugin` without UI/egui/visualization/input.
- No gameplay behavior changes.

Tests:

```text
cargo test -p macrocosmo --lib
cargo test -p macrocosmo --test pipeline_e2e -- --test-threads=1
```

This PR creates the highest-value boundary first: simulation can run without interactions, and future rendering/input changes have a clear dependency direction.
