# Plan: Fix #532 Galaxy Map Own-Ship Marker Interpolation Follow-up

## Context

Issue #532 was closed by PR #534, but the user-visible behavior can still remain broken.

PR #534 added interpolation helpers and wired them into the `InTransitSubLight | InTransitFTL` branch of `draw_ships`. The remaining gap is that the normal post-dispatch projection often still has:

```text
projected_state = InSystem
projected_system = origin
intended_state = InTransit*
intended_system = destination
now >= intended_takes_effect_at
```

In that state, `draw_ships` currently routes the ship through the docked-style `InSystem | Refitting` branch, where it is grouped into `docked_counts` at `projected_system`. That branch does not call `projection_screen_pos`, so the #534 interpolation code is bypassed.

## Evidence

- `macrocosmo/src/knowledge/mod.rs::compute_ship_projection`
  - Keeps `projected_state` from the last known snapshot, or falls back to `InSystem`.
  - A newly dispatched projected movement can therefore retain `projected_state = InSystem` while `intended_state = InTransit*`.
- `macrocosmo/src/visualization/ships.rs::draw_ships`
  - Branches on `item.projected_state`.
  - `InSystem | Refitting` only contributes to docked marker grouping.
  - `projection_screen_pos` is only used by the transit branch and selected command-queue overlay fallback.
- `macrocosmo/tests/ship_projection_interpolation_532.rs`
  - Tests `own_ship_marker_screen_pos` directly.
  - Does not cover the real `draw_ships` state routing that skips interpolation when `projected_state = InSystem`.

## Goal

Own-empire ship markers should visibly move from `projected_system` toward `intended_system` after the command reaches the ship, even if the light-coherent `projected_state` is still `InSystem` pending reconciliation.

The existing no-FTL-leak rule must remain intact:

- Before `intended_takes_effect_at`, the marker stays at `projected_system`.
- After `intended_takes_effect_at`, transit-style intended movement can interpolate.
- After reconciliation makes `projected_system == intended_system`, normal rendering resumes.

## Proposed Fix

1. Add a small helper in `visualization::ships`, for example:

   ```rust
   fn should_draw_projected_marker_with_interpolation(
       projection: &ShipProjection,
       now: i64,
   ) -> bool
   ```

   It should return true when:

   - `projection.intended_state.as_ref().is_some_and(ShipSnapshotState::is_in_transit)`
   - `projection.projected_system != projection.intended_system`
   - `projection.intended_takes_effect_at.is_some_and(|t| now >= t)`
   - `projection.expected_arrival_at.is_some()`

2. In `draw_ships`, before the `match &item.projected_state` docked handling, fetch the projection for the item when needed.

3. For `InSystem | Refitting`, if the helper returns true:

   - call `projection_screen_pos(projection, &stars, view.scale, clock.elapsed)`
   - draw the moving marker at that position
   - do not add the ship to `docked_counts` for that frame

4. Keep pre-effect behavior unchanged:

   - If `now < intended_takes_effect_at`, continue grouping the ship as docked at `projected_system`.
   - This preserves the #530 dispatch-window no-FTL-leak contract.

5. Keep the existing `InTransitSubLight | InTransitFTL` branch using `projection_screen_pos`.

## Test Plan

Add focused regression coverage that exercises the real render-routing decision, not only the pure lerp helper.

Recommended tests:

1. `projected_in_system_pre_effect_remains_docked`
   - Projection:
     - `projected_state = InSystem`
     - `projected_system = origin`
     - `intended_state = Some(InTransitFTL)` or `Some(InTransitSubLight)`
     - `intended_system = destination`
     - `now < intended_takes_effect_at`
   - Assert the render route remains docked/projected.

2. `projected_in_system_post_effect_uses_interpolated_marker`
   - Same projection shape, but `intended_takes_effect_at < now < expected_arrival_at`.
   - Assert the route uses `projection_screen_pos` and yields a midpoint marker rather than a docked marker.

3. `projected_in_system_missing_arrival_eta_remains_docked`
   - Same projection shape, but `expected_arrival_at = None`.
   - Assert no interpolation is attempted.

4. Keep existing tests:

   ```text
   cargo test -p macrocosmo --test ship_projection_render -- --test-threads=1
   cargo test -p macrocosmo --test ship_projection_interpolation_532 -- --test-threads=1
   cargo test -p macrocosmo --test ship_projection_intended_render -- --test-threads=1
   ```

If adding a pure render-routing helper is cleaner than testing `draw_ships` directly, test that helper. The key is to cover the `projected_state = InSystem` plus `intended_state = InTransit*` case.

## Risks And Checks

- Avoid moving markers before `intended_takes_effect_at`; that would reintroduce the FTL leak fixed by #530.
- Avoid removing docked grouping for ordinary `InSystem` ships with no transit intent.
- Avoid changing omniscient rendering; it already uses realtime ship state.
- Confirm selected command-queue overlays still anchor cleanly when the marker is interpolating.
- Confirm intended dashed overlay still starts at `projected_system` unless deliberately changed.

## Verification Commands

```text
cargo test -p macrocosmo --test ship_projection_render -- --test-threads=1
cargo test -p macrocosmo --test ship_projection_interpolation_532 -- --test-threads=1
cargo test -p macrocosmo --test ship_projection_intended_render -- --test-threads=1
```

Run broader workspace tests if the draw-path refactor touches shared helpers or public test fixtures.
