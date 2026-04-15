# Profiling Macrocosmo with Tracy (#284)

Macrocosmo integrates with [Tracy](https://github.com/wolfpld/tracy) via Bevy's
first-party `trace_tracy` feature. Enabling the `profile` cargo feature
pass-throughs `bevy/trace_tracy` (CPU spans + a `RenderQueue` GPU row) and
`bevy/debug` (human-readable system names in the timeline). A handful of
self-written hot-path systems also emit spans via the `prof_span!` macro —
these compile to **nothing** on default builds.

The default build therefore carries zero binary-size or startup overhead; you
only pay for profiling when you explicitly opt in.

## What gets captured

With `--features profile` on a `--release` build Tracy shows:

- **Bevy systems** — every scheduled system becomes a span, named via
  `bevy/debug` (human-readable names instead of numeric ids).
- **Self-written spans** — the macro `crate::prof_span!("...")` adds targeted
  spans on hot paths:
  - Lua callback dispatch: `EventBus::fire`, `run_lifecycle_hooks`,
    `drain_script_events`.
  - Knowledge propagation: `build_system_snapshot`, `propagate_knowledge`,
    `relay_knowledge_propagate_system`.
  - UI six-system chain: `compute_ui_state`, `draw_top_bar`,
    `draw_outline_and_tooltips`, `draw_main_panels`, `draw_overlays`,
    `draw_bottom_bar`.
  - Galaxy rendering: `update_star_colors`, `sync_territory_material`.
- **`RenderQueue` row** — Bevy's built-in GPU timing row (queue submission
  latencies per-frame). Use mean-time-per-call (MTPC) / distribution rather
  than frame-by-frame spikes, since the dynamic game clock introduces jitter.

Not captured (see non-goals below): memory allocations, FPS overlay, custom
render-pass GPU spans.

## Prerequisites

1. **Tracy client version must match the server (profiler UI)**. Check
   which tracy client version Bevy 0.18 pulls in:

   ```bash
   cargo tree -p macrocosmo --features profile | grep tracy
   ```

   Cross-reference against the
   [rust_tracy_client version support table](https://github.com/nagisa/rust_tracy_client?tab=readme-ov-file#version-support-table)
   to find the matching Tracy release. At the time of writing Bevy 0.18 uses
   `tracy-client 0.18.x`, which maps to Tracy **0.11.x**.

2. Install the matching Tracy binaries. On macOS you can build from source
   (`brew install tracy` sometimes lags behind):

   ```bash
   git clone --depth 1 --branch v0.11.1 https://github.com/wolfpld/tracy
   cd tracy/profiler/build/unix && make release
   cd ../../../capture/build/unix && make release
   ```

   You'll want two binaries: `tracy-profiler` (GUI) and `tracy-capture`
   (headless recorder).

## Always use `--release`

Bevy's profiling docs are emphatic about this:

> There is little point to profiling unoptimized code — numbers look nothing
> like a release build.

Run:

```bash
cargo run --release --features profile
```

Never profile a `dev` build; the numbers are meaningless.

## Workflow A — Live connection (quickest)

1. Start the profiler GUI **first** so it's waiting for a connection:

   ```bash
   tracy-profiler
   ```

   Click **Connect** in the GUI.

2. Launch the game:

   ```bash
   cargo run --release --features profile
   ```

3. Play through the scenario you want to measure. The GUI fills in
   real-time.

4. On exit (or at any time) click **File → Save Trace** in the GUI to keep
   the recording.

## Workflow B — Recording (recommended, lower overhead)

The live connection adds frame-by-frame network chatter. For longer sessions
or more reliable numbers, record first and inspect later.

1. Start the headless capture tool:

   ```bash
   tracy-capture -o trace.tracy
   ```

   It will wait for the app to start.

2. Run the game:

   ```bash
   cargo run --release --features profile
   ```

3. When the scenario is done, stop the game. `tracy-capture` writes
   `trace.tracy`.

4. Open the trace in the GUI:

   ```bash
   tracy-profiler trace.tracy
   ```

## Reading the results

- **Self-written spans** — filter the `Find zone` panel by e.g.
  `propagate_knowledge` to isolate your target. Use the `Statistics` tab for
  MTPC + total time.
- **Bevy systems** — with `bevy/debug` enabled each scheduled system appears
  by its function path, so `bevy_ecs::...::run_system` rows become readable.
- **GPU timing** — open the `RenderQueue` row near the bottom. Look at the
  distribution (mean, p95) rather than individual frames — the game clock is
  dynamic and frame duration is not a useful axis on its own.

## Adding new spans

Use the `prof_span!` macro (re-exported at crate root) at the top of any
function you want to measure:

```rust
pub fn my_hot_system(/* params */) {
    crate::prof_span!("my_hot_system");
    // ... work ...
}
```

Guidelines:

- Only spans on **hot paths** — per-tick / per-frame systems, Lua callback
  dispatch, Bevy systems that touch thousands of entities. Avoid spamming
  cold startup code.
- Keep names stable across builds; Tracy treats them as string identifiers.
- The macro is cfg-gated, so default builds emit nothing — do not add
  `#[cfg(feature = "profile")]` around the call site yourself.

## Non-goals (not implemented, see #284)

- `bevy/trace_tracy_memory` — allocation tracking. Non-trivial overhead and
  not needed for the current bottleneck hunt. File a follow-up if you need
  it.
- `bevy_dev_tools::FpsOverlayPlugin` — an in-game FPS counter independent of
  Tracy. Tracked separately.
- Chrome tracing (`bevy/trace_chrome`) / `perf` / flame graphs — Tracy
  supersedes these for the use cases we have today.
- Custom GPU spans via `RenderDiagnosticsPlugin` — we don't yet have custom
  render passes worth labelling.
- Automated benchmarks (`criterion`) / CI regression gates — out of scope.
- Compile-time profiling (`cargo --timings`, `cargo-bloat`) — revisit when
  build times become a concrete problem.
