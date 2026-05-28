# Resume Prompt: 2026-05-27 Refactor

You are working in:

```text
/Users/csakai/repos/macrocosmo
```

Start by reading:

```text
docs/refactor/2026-05-27/README.md
docs/refactor/2026-05-27/plan.md
docs/refactor/2026-05-27/remaining-work.md
docs/refactor/2026-05-27/decision-log.md
docs/refactor/2026-05-27/evidence.md
```

Then inspect the current tree:

```text
git status --short --branch --untracked-files=all
git diff --stat
```

The branch is:

```text
refactor/ai-command-consumer-handlers
```

It tracks:

```text
origin/refactor/ai-command-consumer-handlers
```

Latest pushed commit on the tracking branch:

```text
3e1908a refactor ai command consumer handlers
```

Local commits ahead of `origin/refactor/ai-command-consumer-handlers`:

```text
826e312 docs update refactor remaining work
925e817 refactor metrics and system building lookup
```

## Current Uncommitted Work

The prompt file itself is intentionally rewritten by this resume update.
At the time this prompt was updated, it was the only expected uncommitted file:

Expected changed files:

```text
docs/refactor/2026-05-27/prompt.md
```

## Already Completed Earlier

### Boundary Refactor

The internal simulation/interactions boundary exists:

- `macrocosmo/src/simulation.rs`
- `macrocosmo/src/interactions.rs`
- `macrocosmo/src/interactions/time_controls.rs`
- `macrocosmo/src/interactions/player_controls.rs`
- `macrocosmo/src/interactions/observer_controls.rs`
- `macrocosmo/src/interactions/esc_notifications.rs`
- `macrocosmo/tests/simulation_plugin.rs`
- `macrocosmo/tests/simulation_boundary.rs`

The desktop app composes:

```text
DefaultPlugins + SimulationPlugin + InteractionsPlugin
```

Keep `SimulationPlugin` free of UI/render/input dependencies.

### AI Command Consumer SRP

The AI command consumer split is implemented and pushed in commit `3e1908a`.

Relevant files:

```text
macrocosmo/src/ai/command_consumer.rs
macrocosmo/src/ai/command_outbox.rs
macrocosmo/src/ai/command_params.rs
macrocosmo/src/ai/command_route.rs
macrocosmo/src/ai/command_handlers/mod.rs
macrocosmo/src/ai/command_handlers/build.rs
macrocosmo/src/ai/command_handlers/military.rs
macrocosmo/src/ai/command_handlers/research.rs
```

Previously passing validation:

```text
cargo test -p macrocosmo --lib command_
cargo test -p macrocosmo --test ai_ship_build_queue
cargo test -p macrocosmo --test ai_deliverable_registry_resolution
cargo test -p macrocosmo simulation_plugin --tests
```

## Current Follow-Up Work

### Economic Metrics Snapshot Split

Committed in `925e817`.

Shape:

- added `macrocosmo/src/ai/metrics/mod.rs`;
- added `macrocosmo/src/ai/metrics/economy.rs`;
- introduced pure snapshot structs:
  - `EmpireProductionSnapshot`
  - `EmpirePopulationSnapshot`
  - `EmpireStockpileSnapshot`
  - `EmpireInfrastructureSnapshot`
  - `EmpireEconomicSnapshot`
- added `emit_economic_snapshot(...)` for metric-name mapping and bus emission;
- kept ECS query reading in `emit_economic_metrics`;
- did not intentionally change metric names or schema.

Validated:

```text
cargo test -p macrocosmo --lib emit_economic
cargo test -p macrocosmo --lib ai::metrics::economy
cargo test -p macrocosmo --test ai_resource_gate_hotfix
```

### System Building Capability Index

Committed in `925e817`.

Shape:

- added `SystemBuildingIndex` in `macrocosmo/src/colony/system_buildings.rs`;
- added `rebuild_system_building_index`;
- initialized and rebuilt the index from `ColonyPlugin`;
- kept `build_reverse_design_map(...)` as a compatibility wrapper;
- moved direct reverse-map use in these files toward `SystemBuildingIndex`:
  - `macrocosmo/src/ai/emitters.rs`
  - `macrocosmo/src/colony/production.rs`
  - `macrocosmo/src/deep_space/mod.rs`
  - internal helpers in `macrocosmo/src/colony/system_buildings.rs`

Validated:

```text
cargo test -p macrocosmo --lib system_building_index
cargo test -p macrocosmo --test system_building_capabilities
cargo test -p macrocosmo --test system_building_ship_migration
cargo test -p macrocosmo --test ai_resource_gate_hotfix
cargo test -p macrocosmo --test ai_ship_build_queue
cargo test -p macrocosmo --test ai_deliverable_registry_resolution
```

Also run:

```text
git diff --check
```

It passed.

## Warning Noise

The repository has many pre-existing warnings. Do not treat warning noise as
part of this refactor unless a warning is clearly introduced by the current
changes.

Known warnings seen during validation include unused imports/variables and
deprecated egui APIs in unrelated modules.

## Suggested Next Actions

1. If publishing the branch, push the two local commits:

   ```text
   git push
   ```

2. If continuing implementation, the next planned slices in `remaining-work.md`
   are:

   ```text
   Savebag module split
   UI root module split
   Knowledge module split
   ```

3. Keep `prompt.md` out of normal code commits unless the user explicitly wants
   to commit the local resume artifact.

## Boundary Guard

For simulation/interactions work, keep this check in mind:

```text
rg "bevy_egui|egui::|crate::interactions|crate::ui|crate::visualization|crate::input|KeyCode|ButtonInput|Window" \
  macrocosmo/src/simulation.rs \
  macrocosmo/src/time_system \
  macrocosmo/src/player \
  macrocosmo/src/scripting \
  macrocosmo/src/setup \
  macrocosmo/src/observer
```

Simulation-side production code should not import UI/render/input/window
symbols.

## Do Not Do Yet

Do not start crate extraction yet. The plan intentionally stages module
boundaries before introducing `macrocosmo-core`, `macrocosmo-simulation`, or
`macrocosmo-interactions`.
