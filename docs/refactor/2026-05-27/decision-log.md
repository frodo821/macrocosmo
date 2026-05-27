# Decision Log: 2026-05-27 Refactor Boundary

## D1. Preserve `macrocosmo-ai` isolation

Decision:

- Keep `macrocosmo-ai` engine-agnostic.
- Do not make it depend on `macrocosmo`, Bevy, or simulation runtime types.
- Consider `macrocosmo-ai -> macrocosmo-core` only after `core` contains stable Bevy-free contracts.

Reason:

- Existing architecture already enforces `macrocosmo -> macrocosmo-ai`.
- AI logic can benefit from shared pure definitions, but only if that does not pull in engine/runtime dependencies.

## D2. Prioritize simulation/interactions separation over AI/DRY cleanup

Decision:

- The first refactor track should separate rendering/input/UI from the authoritative game loop.
- AI command DRY cleanup remains valuable, but moves to Phase 2.

Reason:

- The acute failure mode is cross-breakage: rendering fixes breaking game loop behavior, and game loop fixes breaking rendering.
- A plugin-level boundary gives immediate protection and clearer ownership.

## D3. Do not start with crate extraction

Decision:

- First create `SimulationPlugin` and `InteractionsPlugin` inside the existing `macrocosmo` crate.
- Only extract `macrocosmo-simulation` and `macrocosmo-interactions` after the internal boundary is stable.

Reason:

- Current modules have many accidental imports.
- Crate extraction before module cleanup would force a large public API churn and produce noisy mechanical diffs.

## D4. Split mixed modules before moving them

Decision:

- Treat these as priority mixed modules:
  - `time_system`
  - `observer`
  - `notifications`
  - `choice`

Reason:

- They currently contain both simulation state/lifecycle and interaction/presentation concerns.
- Moving them wholesale would preserve the coupling under a new name.

## D5. First implementation PR should be a boundary PR

Decision:

First PR title:

```text
refactor(app): introduce simulation and interactions plugin boundary
```

Scope:

- Add internal `simulation` and `interactions` entry points.
- Add `SimulationPlugin` and `InteractionsPlugin`.
- Move plugin composition out of `main.rs`.
- Split `time_system` clock advancement from speed-control input.
- Add a headless simulation smoke test.

Reason:

- This is the smallest PR that directly addresses the stated coupling problem.
- It avoids behavior changes while creating a future crate extraction path.

