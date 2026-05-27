# Resume Prompt: 2026-05-27 Refactor Follow-up

You are working in `/Users/csakai/repos/macrocosmo`.

Use the plan documents in this directory as the source of truth:

- `docs/refactor/2026-05-27/README.md`
- `docs/refactor/2026-05-27/plan.md`
- `docs/refactor/2026-05-27/evidence.md`
- `docs/refactor/2026-05-27/decision-log.md`

The user asked to continue the refactor plan and explicitly allowed using
sub-agents for investigation/work.

## Current Status

The first recommended boundary step is already present in `HEAD`:

- `macrocosmo/src/simulation.rs`
- `macrocosmo/src/interactions.rs`
- `macrocosmo/src/interactions/time_controls.rs`
- `macrocosmo/src/interactions/player_controls.rs`
- `macrocosmo/src/interactions/observer_controls.rs`
- `macrocosmo/src/interactions/esc_notifications.rs`
- `macrocosmo/tests/simulation_plugin.rs`
- `macrocosmo/tests/simulation_boundary.rs`

`SimulationPlugin` runs the authoritative game loop without UI/render/input,
and `InteractionsPlugin` composes input, visualization, UI, observer controls,
ESC notification drain, and feature-gated remote support.

Two sub-agents verified:

- the simulation/interactions boundary is already implemented and the boundary
  smoke tests exist;
- the AI command consumer refactor was compile-safe but incomplete before the
  latest work.

## Latest Work Completed

The AI command consumer SRP cleanup from `plan.md` finding #1 was advanced.

Changed tracked files:

- `macrocosmo/src/ai/command_consumer.rs`
- `macrocosmo/src/ai/command_outbox.rs`
- `macrocosmo/src/ai/mod.rs`

New untracked code files created by this refactor:

- `macrocosmo/src/ai/command_params.rs`
- `macrocosmo/src/ai/command_route.rs`
- `macrocosmo/src/ai/command_handlers/mod.rs`
- `macrocosmo/src/ai/command_handlers/build.rs`
- `macrocosmo/src/ai/command_handlers/military.rs`
- `macrocosmo/src/ai/command_handlers/research.rs`

What changed:

- `drain_ai_commands` now matches on `command_route::classify(&cmd.kind)`.
- stale ship-control and macro-command classification lives in
  `ai/command_route.rs`.
- command param keys and typed accessors live in `ai/command_params.rs`.
- `command_outbox.rs` now reuses `target_system`, `optional_system`, and
  `ship_list` from `command_params`.
- `extract_ship_list` was removed from `command_consumer.rs`.
- `find_empire_entity` is centralized in `command_handlers/mod.rs`.
- `handle_retreat` moved to `command_handlers/military.rs`.
- `handle_research_focus` moved to `command_handlers/research.rs`.
- build-side handlers moved to `command_handlers/build.rs`:
  - `handle_build_ship`
  - `handle_fortify_system`
  - `handle_build_structure`
  - `handle_build_deliverable`
  - `pick_host_colony`
  - `queue_ship_at_shipyard`
  - `has_shipyard_check`
- `command_params.rs` now has direct unit tests for:
  - `required_str`
  - `target_system`
  - `ship_list`

## Validation Run

Successful:

```text
cargo check -p macrocosmo
cargo test -p macrocosmo command_route --no-fail-fast
cargo test -p macrocosmo command_params --no-fail-fast
cargo test -p macrocosmo --test ai_ship_build_queue --test ai_deliverable_registry_resolution --test simulation_boundary --test simulation_plugin --no-fail-fast
```

Also attempted:

```text
cargo test -p macrocosmo --no-fail-fast
```

Result: normal test binaries ran successfully, but the final doctest target
failed with a macOS runtime loader error:

```text
dyld: Library not loaded: @rpath/libstd-8676e64d1195d4db.dylib
```

Treat that as an environment/toolchain doctest issue unless it reproduces in a
way tied to this refactor.

Formatting note:

- touched Rust files were formatted with `rustfmt`;
- global `cargo fmt --check` still reports unrelated pre-existing formatting
  diffs in test files, so do not use it as a clean signal unless you intend to
  handle those unrelated files.

## Current Worktree Caution

There are unrelated/untracked docs in the worktree. Do not stage or modify
them unless explicitly requested.

Known unrelated/untracked examples include:

```text
docs/code-review-2026-05-25-ai-resource-gate.md
docs/handoff/
docs/plan-532-ship-marker-interpolation-followup.md
prompt.md
```

`docs/refactor/2026-05-27/` is intentionally untracked in this worktree.

## Recommended Next Step

Pick the next item from `plan.md` based on risk and scope.

Good next candidates:

1. `ai/emitters.rs` economic metrics SRP split:
   - introduce `ai/metrics/economy.rs`;
   - separate ECS snapshot collection from metric emission;
   - add focused tests for pure snapshot-to-metric logic.
2. `persistence/savebag.rs` split:
   - higher risk because it is large and save-format sensitive;
   - preserve wire field names and run fixture/save-load tests.
3. `ui/mod.rs` split:
   - medium priority; keep UI-only and avoid changing simulation behavior.

Avoid starting crate extraction yet. The current plan explicitly recommends
stabilizing module boundaries before adding `macrocosmo-core`,
`macrocosmo-simulation`, or `macrocosmo-interactions`.

## Useful Commands

```text
git status --short --untracked-files=all
git diff --stat
rg "bevy_egui|egui::|crate::interactions|crate::ui|crate::visualization|crate::input|KeyCode|ButtonInput|Window" macrocosmo/src/simulation.rs macrocosmo/src/time_system macrocosmo/src/player macrocosmo/src/scripting macrocosmo/src/setup macrocosmo/src/observer
cargo check -p macrocosmo
cargo test -p macrocosmo --test simulation_boundary --test simulation_plugin --no-fail-fast
```
