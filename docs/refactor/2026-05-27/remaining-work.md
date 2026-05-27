# Remaining Work: 2026-05-27 Refactor

## Current State

This file tracks the remaining work after the first refactor pass.

Already represented in the current tree:

- `SimulationPlugin` / `InteractionsPlugin` internal boundary exists.
- `time_system` owns clock state and advancement only; speed-control input lives under `interactions/time_controls.rs`.
- Headless simulation smoke coverage exists in `tests/simulation_plugin.rs`.
- A static forbidden-import regression check exists in `tests/simulation_boundary.rs`.
- AI command consumer refactor is committed in `3e1908a`:
  - `ai/command_params.rs`
  - `ai/command_route.rs`
  - `ai/command_handlers/{build,military,research}.rs`
  - `command_consumer.rs` is reduced toward bus drain + route dispatch.
- Economic AI metrics split is committed in `925e817`:
  - pure snapshot structs live under `ai/metrics/economy.rs`;
  - ECS query reading stays in `emitters.rs`.
- System-building station design lookup is committed in `925e817`:
  - `SystemBuildingIndex` centralizes station design lookup;
  - the index is rebuilt from `BuildingRegistry` and used by the main station-capability call sites.

Verification run for the current pass:

```text
cargo test -p macrocosmo --lib command_
cargo test -p macrocosmo --test ai_ship_build_queue
cargo test -p macrocosmo --test ai_deliverable_registry_resolution
cargo test -p macrocosmo simulation_plugin --tests
```

All passed. Existing warning noise remains outside this refactor's scope.

## PR Slices

### Slice 1: Boundary PR

Status: mostly complete in the current tree.

Suggested PR title:

```text
refactor(app): introduce simulation and interactions plugin boundary
```

Keep this slice focused on:

- `SimulationPlugin` composition.
- `InteractionsPlugin` composition.
- `main.rs` composing `DefaultPlugins + SimulationPlugin + InteractionsPlugin`.
- `time_system` / `interactions/time_controls` split.
- `observer` state/control split already represented by `ObserverSimulationPlugin` and `ObserverControlsPlugin`.
- `tests/simulation_plugin.rs`.
- `tests/simulation_boundary.rs`.

Do not include the AI command handler split in this PR unless intentionally combining refactor tracks.

Before merging:

- Run `cargo test -p macrocosmo simulation_plugin --tests`.
- Run the boundary forbidden-import test if not covered by the same command.
- Confirm production simulation-side files do not import `bevy_egui`, `egui::`, `crate::ui`, `crate::visualization`, `crate::input`, `KeyCode`, `ButtonInput`, or `Window`.

### Slice 2: AI Command Consumer SRP

Status: committed in `3e1908a`.

Suggested PR title:

```text
refactor(ai): split command consumer routing and handlers
```

Scope:

- Keep `drain_ai_commands` focused on bus drain, route classification, and lifecycle stamping.
- Keep per-domain mutation in handlers:
  - `command_handlers/build.rs`
  - `command_handlers/research.rs`
  - `command_handlers/military.rs`
- Keep command param key parsing in `command_params.rs`.
- Keep stale ship-control and macro command classification in `command_route.rs`.
- Keep `command_outbox.rs` using shared accessors for ship list and target system extraction.

Follow-up cleanup inside this slice:

- Consider moving `build_structure` into a separate `command_handlers/structure.rs` if `build.rs` grows again.
- Add handler-level focused tests only where they catch behavior not already covered by `command_consumer` tests.
- Keep module visibility narrow: `pub(crate)` for module entry points, `pub(in crate::ai)` for shared `SystemParam` fields.

Validation:

```text
cargo test -p macrocosmo --lib command_
cargo test -p macrocosmo --test ai_ship_build_queue
cargo test -p macrocosmo --test ai_deliverable_registry_resolution
```

### Slice 3: Economic Metrics Snapshot Split

Status: committed in `925e817`.

Suggested PR title:

```text
refactor(ai): split economic metric snapshots from emission
```

Scope:

- Add `ai/metrics/economy.rs`.
- Introduce pure snapshot structs:
  - `EmpireProductionSnapshot`
  - `EmpirePopulationSnapshot`
  - `EmpireStockpileSnapshot`
  - `EmpireInfrastructureSnapshot`
  - `EmpireEconomicSnapshot`
- Keep ECS query reading in `emitters.rs` or a thin system wrapper.
- Move bus metric name mapping into `emit_economic_snapshot(writer, faction, snapshot)`.

Constraints:

- Do not change metric names or schema in the split PR.
- Do not move this into a new crate yet.
- Prefer pure unit tests for arithmetic and metric conversion.

Validation candidates:

```text
cargo test -p macrocosmo --lib emit_economic
cargo test -p macrocosmo --lib ai::metrics::economy
cargo test -p macrocosmo --test ai_resource_gate_hotfix
```

### Slice 4: Savebag Module Split

Status: not started.

Suggested PR title:

```text
refactor(save): split savebag wire structs by domain
```

Scope:

- Convert `persistence/savebag.rs` into `persistence/savebag/mod.rs`.
- Split domain sections into:
  - `helpers.rs`
  - `galaxy.rs`
  - `ship.rs`
  - `colony.rs`
  - `faction.rs`
  - `knowledge.rs`
  - `ai.rs`
  - `events.rs`
- Preserve `SavedComponentBag` field names, ordering intent, and postcard schema.
- Keep live ECS conversion in `macrocosmo`; do not create a save crate yet.

Constraints:

- This should be a mechanical split first.
- Avoid behavior changes and schema changes in the same PR.
- Re-export types from `mod.rs` so callers do not churn unnecessarily.

Validation:

```text
cargo test -p macrocosmo --test save_load
cargo test -p macrocosmo --test ship_snapshot_persistence
cargo test -p macrocosmo --lib savebag
```

### Slice 5: UI Root Module Split

Status: not started.

Suggested PR title:

```text
refactor(ui): split root ui module responsibilities
```

Scope:

- `ui/plugin.rs`: `UiPlugin` registration.
- `ui/font.rs`: CJK font setup and font tests.
- `ui/view_context.rs`: `resolve_ui_empire`, `resolve_viewing_knowledge`, omniscient handling.
- `ui/state.rs`: `UiState`, `compute_ui_state`, observer variant.
- `ui/notifications_panel.rs`: notification pill drawing.

Constraints:

- Keep Bevy/egui UI code in interactions-side modules.
- Do not change layout or behavior in this PR.
- Avoid moving large panel modules at the same time.

Validation:

```text
cargo test -p macrocosmo --test observer_mode
cargo test -p macrocosmo --test observer_mode_omniscient
cargo test -p macrocosmo --test ai_debug_smoke
```

### Slice 6: System Building Capability Index

Status: committed in `925e817`.

Suggested PR title:

```text
refactor(colony): centralize system building capability lookup
```

Scope:

- Add `SystemBuildingIndex`.
- Centralize `design_id -> BuildingId` reverse lookup.
- Rebuild index after registry load / script reload.
- Add helper(s) for station-building capability scans.

Constraints:

- Keep index data shape as pure as practical.
- Keep ECS station ship scanning in `macrocosmo`.
- Do not change shipyard / port / core behavior.

Validation candidates:

```text
cargo test -p macrocosmo --lib system_building_index
cargo test -p macrocosmo --test system_building_capabilities
cargo test -p macrocosmo --test system_building_ship_migration
cargo test -p macrocosmo --test ai_ship_build_queue
```

### Slice 7: Knowledge Module Split

Status: not started.

Suggested PR title:

```text
refactor(knowledge): split light-speed read model modules
```

Scope:

- `knowledge/vantage.rs`
- `knowledge/projection.rs`
- `knowledge/visibility.rs`
- `knowledge/snapshot.rs`
- `knowledge/propagation.rs`
- `knowledge/combat_events.rs`

Constraints:

- Keep public API stable where UI / AI / scripting call into knowledge.
- Keep propagation ECS systems in `macrocosmo`.
- Avoid changing light-delay semantics in the split PR.

Validation candidates:

```text
cargo test -p macrocosmo --test knowledge
cargo test -p macrocosmo --test knowledge_observed
cargo test -p macrocosmo --test ship_projection_reconcile
cargo test -p macrocosmo --test ship_destruction_observation_contract
```

## Later Crate Extraction

Do not start crate extraction until the internal module boundaries have survived at least the boundary PR and one domain refactor.

Future extraction order:

1. Identify Bevy-free data/rules already isolated by module splits.
2. Create `macrocosmo-core` only for stable pure contracts.
3. Consider `macrocosmo-ai -> macrocosmo-core` only if it avoids duplication without importing engine/runtime concepts.
4. Extract simulation/interactions crates only after `macrocosmo/src/simulation` has no UI/input/render imports and interactions are lifecycle consumers rather than owners.
