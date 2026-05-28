# Knowledge Redesign Implementation Plan

Status: implementation plan for `docs/knowledge-redesign.md`.

Goal: replace the current empire-only `KnowledgeStore` semantics with subject-local knowledge state while keeping the codebase shippable through small migration slices.

This plan intentionally starts with compatibility wrappers and dual-write paths. The codebase currently has many production call sites depending on `KnowledgeStore`, `ShipProjection`, `PendingAssignment`, `AiCommandOutbox`, and `PendingAiShipCommand`; replacing them in one pass would be too risky.

## Current Hotspots

### Knowledge Data And Systems

File: `macrocosmo/src/knowledge/mod.rs`

Important existing surfaces:

- `KnowledgeStore` at `knowledge/mod.rs:656` stores:
  - system observations: `entries`
  - ship observations: `ship_snapshots`
  - own-ship beliefs/intents: `projections`
- `ShipProjection` at `knowledge/mod.rs:600` mixes projected belief and intended commitment fields.
- `compute_ship_projection` at `knowledge/mod.rs:815` materializes own-ship projected state at dispatch time.
- `flush_ship_projection_writes`, `seed_own_ship_projections`, and `reconcile_ship_projections` at `knowledge/mod.rs:999`, `1053`, and `1157` maintain the projection store.
- `propagate_knowledge` at `knowledge/mod.rs:1812` is a producer from simulation truth into delayed snapshots.
- `update_destroyed_ship_knowledge` at `knowledge/mod.rs:2061` mutates perceived ship ghosts and emits missing facts.

The important distinction:

- `propagate_knowledge` is allowed to read ground truth because it is a knowledge producer.
- UI/AI/normal visualization should not use ground truth as fallback.

### AI Commitment/Dedup

Files:

- `macrocosmo/src/ai/assignments.rs`
- `macrocosmo/src/ai/npc_decision.rs`
- `macrocosmo/src/ai/command_outbox.rs`
- `macrocosmo/src/ai/command_consumer.rs`

Important existing surfaces:

- `PendingAssignment` in `ai/assignments.rs` is the durable-ish AI decision marker on a ship.
- `sweep_resolved_assignments` resolves those markers by reading the issuing empire's `KnowledgeStore`.
- `DedupParams` in `ai/npc_decision.rs:36` unions:
  - `PendingAssignment`
  - `AiCommandOutbox`
  - `PendingAiShipCommand`
- `npc_decision_tick` builds per-empire dedup sets around `ai/npc_decision.rs:760-950`.
- `dispatch_ship_command_per_ship` in `ai/command_outbox.rs:1059` stamps `PendingAssignment`, spawns `PendingAiShipCommand`, and writes `ShipProjection`.
- `PendingAiShipCommand` in `ai/command_consumer.rs:384` represents command-in-flight-to-ship. It is runtime-only.

This is the core semantic conflict: the same player/AI intention exists as outbox entries, pending ship command holders, marker components, and projection fields.

### Perception And Ground-Truth Fallback

Files:

- `macrocosmo/src/knowledge/ship_view.rs`
- `macrocosmo/src/ui/mod.rs`
- `macrocosmo/src/ui/outline.rs`
- `macrocosmo/src/ui/ship_panel.rs`
- `macrocosmo/src/ui/context_menu.rs`
- `macrocosmo/src/ui/situation_center/ship_ops_tab.rs`
- `macrocosmo/src/visualization/ships.rs`
- `macrocosmo/src/visualization/stars.rs`

Important existing surfaces:

- `ship_view` / `ship_view_with_timing` in `knowledge/ship_view.rs` fall back to realtime `ShipState` when `viewing_knowledge` is `None`.
- `ViewingEmpireResolver` in `visualization/stars.rs:480` returns `None` for omniscient mode.
- `draw_ships_omniscient` in `visualization/ships.rs:574` is an explicit allowed ground-truth path.
- `ui/mod.rs` helper variants collapse omniscient to `None`, which currently feeds fallback-heavy APIs.

This should become explicit `PerceptionMode::Subject` vs `PerceptionMode::Omniscient`, not `Option<&KnowledgeStore>`.

### Persistence

Files:

- `macrocosmo/src/persistence/savebag.rs`
- `macrocosmo/src/persistence/save.rs`

Important existing surfaces:

- `SavedPendingAssignment` at `savebag.rs:1637`.
- `SavedShipProjection` at `savebag.rs:3190`.
- `SavedKnowledgeStore` at `savebag.rs:3231`.
- `SavedPendingAiCommand` at `savebag.rs:5099`.
- `SAVE_VERSION` is currently `20` in `persistence/save.rs:155`.

Persistence migration should be delayed until the new runtime shapes are proven by tests. The first slices can avoid save format changes by dual-writing into runtime-only structures.

## Target Runtime Shape

Introduce new modules under `macrocosmo/src/knowledge/`:

```text
subject.rs       // KnowledgeSubject, KnowledgeNode, KnowledgeScope
observation.rs   // ObservationId, ObservationStamp, ObservationSet facade
commitment.rs    // CommitmentLedger, CommitmentKind, CommitmentTarget
belief.rs        // BeliefStamp, BeliefCache, materialized views
perception.rs    // PerceptionMode, Perception facade, no implicit realtime fallback
merge.rs         // MergeContext, MergeReport, merge rules
```

Keep `KnowledgeStore` initially as the storage component for compatibility, but add a new wrapper:

```rust
pub type KnowledgeState = KnowledgeStore; // temporary alias in slice 1
```

Then evolve `KnowledgeStore` fields toward:

```rust
pub struct KnowledgeStore {
    observations: ObservationSet,
    commitments: CommitmentLedger,
    beliefs: BeliefCache,
    inbox: KnowledgeInbox,

    // temporary compatibility fields until migrated:
    entries: HashMap<Entity, SystemKnowledge>,
    ship_snapshots: HashMap<Entity, ShipSnapshot>,
    projections: HashMap<Entity, ShipProjection>,
}
```

Do not rename the component in the first PR. Too many queries and saved fixtures assume `KnowledgeStore`.

## Slice 1: Subject Identity And Facade Types

Purpose: make "who owns knowledge" first-class without changing behavior.

Files to add:

- `macrocosmo/src/knowledge/subject.rs`
- `macrocosmo/src/knowledge/perception.rs`

Files to touch:

- `macrocosmo/src/knowledge/mod.rs`
- `macrocosmo/src/player/mod.rs`

Implementation:

1. Add:
   - `KnowledgeSubject`
   - `KnowledgeScope`
   - `KnowledgeNode`
   - `PerceptionMode`
   - `Perception`
2. Re-export them from `knowledge/mod.rs`.
3. When spawning an empire in `player/mod.rs`, add `KnowledgeNode { subject: KnowledgeSubject::Empire(empire), scope: KnowledgeScope::GlobalEmpire, parent: None }` next to `KnowledgeStore::default()`.
4. Keep all existing `Query<&KnowledgeStore, With<Empire>>` unchanged.

Tests:

- Add a small test in `macrocosmo/tests/knowledge_subject.rs` verifying spawned empires can carry both `KnowledgeStore` and `KnowledgeNode`.
- Run:
  - `cargo test -p macrocosmo --test player`
  - `cargo test -p macrocosmo --test knowledge`

Risk:

- Very low. This is additive.

## Slice 1.5: Subject Backfill And Load Invariant

Added 2026-05-29 after review (Finding 1).

Purpose: guarantee every `Empire + KnowledgeStore` carries a
`KnowledgeNode`, including loaded saves and test setups that bypass
the spawn helpers.

Files touched:

- `macrocosmo/src/knowledge/mod.rs` — `backfill_knowledge_node` system.
- `macrocosmo/src/reflect_registration.rs` — register `KnowledgeNode`,
  `KnowledgeSubject`, `KnowledgeScope` for BRP visibility.

Implementation:

1. Add `pub fn backfill_knowledge_node(...)` to `knowledge/mod.rs`,
   querying
   `(Entity, With<Empire>, With<KnowledgeStore>, Without<KnowledgeNode>)`
   and inserting `KnowledgeNode::empire(entity)`.
2. Register the system in `KnowledgeStorePlugin::build` under
   `Update.after(advance_game_time)` — ungated so loaded games gain
   the node on the first `Update` after `LoadingSave -> InGame`.
3. Register `KnowledgeNode` / `KnowledgeSubject` / `KnowledgeScope`
   in `reflect_registration.rs` alongside `KnowledgeStore`.

Tests:

- `backfill_inserts_knowledge_node_on_loaded_empire` — direct world
  spawn without `KnowledgeNode` gets one after the system runs.
- `backfill_is_idempotent_when_node_present` — re-running the system
  does not overwrite an existing node.

Risk:

- Low. Additive system with a `Without` filter.

## Slice 2: CommitmentLedger Skeleton And Dual Write

Purpose: create a single semantic home for commitments while leaving existing dedup markers intact.

Files to add:

- `macrocosmo/src/knowledge/commitment.rs`

Files to touch:

- `macrocosmo/src/knowledge/mod.rs`
- `macrocosmo/src/ai/command_outbox.rs`
- `macrocosmo/src/ai/assignments.rs`

Implementation:

1. Define:

   ```rust
   pub struct CommitmentLedger {
       entries: HashMap<CommitmentId, Commitment>,
       by_subject_kind_target: HashMap<(KnowledgeSubject, CommitmentKind, CommitmentTarget), Vec<CommitmentId>>,
   }
   ```

2. Start with these kinds:
   - `Survey`
   - `Colonize`
   - `Move`
   - `DeployDeliverable`

3. Add APIs:
   - `record_issued`
   - `mark_resolved`
   - `mark_failed`
   - `has_active(subject, kind, target)`
   - `active_for_actor(actor)`

4. Add `commitments: CommitmentLedger` to `KnowledgeStore`.
   - This will require updating `KnowledgeStore::default()` only if derive cannot infer it.
   - Avoid persistence in this slice by marking commitment persistence as TODO and accepting reset-on-load if necessary only if the field is skipped from saved wire format. Since `KnowledgeStore` has custom save mirrors, adding a live field does not automatically affect save format.

5. In `dispatch_ship_command_per_ship` (`ai/command_outbox.rs`), when it currently:
   - spawns `PendingAiShipCommand`,
   - inserts `PendingAssignment`,
   - writes `ShipProjection`,
   also record a `Commitment` in issuer empire's `KnowledgeStore.commitments`.

6. In `PendingAssignment` constructors, add conversion helpers only:
   - `kind.to_commitment_kind()`
   - `target.to_commitment_target(...)`

Do not remove any existing marker behavior yet.

Tests:

- Extend existing `ship_projection_dispatch` or add `knowledge_commitments.rs`:
  - AI dispatch of `survey_system` records active survey commitment.
  - `PendingAssignment` still exists.
  - `ShipProjection` still exists.
- Run:
  - `cargo test -p macrocosmo --test ship_projection_dispatch`
  - `cargo test -p macrocosmo --test ai_npc_no_double_survey_assignment`
  - `cargo test -p macrocosmo --test ai_npc_outbox_dedup`

Risk:

- Medium. Dispatch code is already dense and Bevy query borrowing can conflict. If `DispatchParams.knowledge_stores` is already mutably borrowed for projection writes, record commitment in the same borrow block.

## Slice 3: Commitment Resolution From Knowledge Arrival

Purpose: commitment lifetime should be resolved by observations/facts, not by scattered marker sweeps.

Files to touch:

- `macrocosmo/src/knowledge/mod.rs`
- `macrocosmo/src/ai/assignments.rs`
- `macrocosmo/tests/ship_projection_reconcile.rs`
- `macrocosmo/tests/ai_npc_no_double_survey_assignment.rs`

Implementation:

1. Add `resolve_commitments_from_observations` system after `reconcile_ship_projections`.
2. It should inspect arriving `KnowledgeFact`s from `PendingFactQueue` the same way `reconcile_ship_projections` does:
   - `SurveyComplete` resolves `CommitmentKind::Survey` for matching target/actor.
   - `ColonyEstablished` resolves `CommitmentKind::Colonize`.
   - `ShipDestroyed` / `ShipMissing` fails commitments for that actor.
3. Keep `sweep_resolved_assignments` in `ai/assignments.rs` for compatibility, but make it mirror `CommitmentLedger`:
   - If corresponding commitment is resolved/failed, remove marker.
   - Keep the old snapshot-based resolution as fallback for one slice.

Tests:

- Add/extend tests:
  - `SurveyComplete` arrival marks survey commitment resolved.
  - `ShipMissing` marks ship actor commitments failed.
  - resolved commitment removes `PendingAssignment`.
- Run:
  - `cargo test -p macrocosmo --test ship_projection_reconcile`
  - `cargo test -p macrocosmo --test ship_destruction_observation_contract`
  - `cargo test -p macrocosmo --test ai_npc_no_double_survey_assignment`

Risk:

- Medium-high. `PendingFactQueue` is global and not target-empire tagged today. Follow `reconcile_ship_projections`'s per-empire arrival recomputation until the queue is redesigned.

## Slice 4: AI Dedup Reads CommitmentLedger

Split into two sub-slices after review (2026-05-29). Slice 4a is the safe
parity-audit stage shipped alongside the initial knowledge core PR; Slice
4b removes the legacy dedup union once write coverage is complete.

### Slice 4a: Ledger Parity Audit (shipped with Slice 1-4 commit)

Purpose: read the ledger in parallel with the legacy dedup union, warn on
divergence, but keep the legacy union as the authoritative source so the
existing AI behavior is preserved bit-for-bit.

Files touched:

- `macrocosmo/src/ai/npc_decision.rs`

Implementation:

1. Add `has_active_commitment` on `KnowledgeStore`.
2. Add `ledger_dedup_targets(knowledge, empire)` returning
   `(survey_set, colonize_set)` from the ledger.
3. After legacy `pending_survey_targets` / `pending_colonize_targets`
   are constructed, compute the ledger-derived sets and emit
   `warn!("AI dedup divergence ...")` when they disagree.
4. Union the ledger sets into the legacy sets (additive — the legacy
   union is the floor; the ledger can only add, never subtract).

Tests:

- `ai_npc_no_double_survey_assignment`
- `ai_npc_outbox_dedup`
- `mid_agent_member_filter`
- `ai_short_agent_per_region_routing`

Risk:

- Low. Behavior is strictly a superset of the legacy union; smoke runs
  surface divergence without breaking gameplay.

### Slice 4b: Ledger Authoritative Dedup

Slice 4b is split into three sequential PR-sized steps so each one
keeps `npc_decision_tick` shippable and reviewable on its own. The
recurring shape is: extend the ledger to cover one more dedup
target → re-run the smoke suite → verify the parity-audit warning is
silent for that target → cut the legacy contribution for that target
only.

#### Slice 4b1: Ledger Coverage For `Colonize(Planet -> System)`

Files to touch:

- `macrocosmo/src/ai/command_outbox.rs` — `PlanetMarker` strategy
- `macrocosmo/src/ai/npc_decision.rs` — `ledger_dedup_targets`

Implementation:

1. In the `PlanetMarker` factory (`dispatch_ship_command_per_ship`'s
   colonize_planet arm), also resolve the planet's parent system at
   dispatch time and record a second commitment entry against
   `CommitmentTarget::System(parent_system)`. Keep the
   `CommitmentTarget::Planet` entry too — it preserves the
   planet-identity provenance for future UI.
2. Ensure `ledger_dedup_targets` can answer system-level colonize
   dedup from the ledger alone. The implemented route is a sibling
   system-keyed commitment written at dispatch time, so the audit path
   can ignore planet-keyed entries without walking `Planet -> System`
   during every decision tick.
3. Re-run `ai_npc_no_double_survey_assignment`,
   `ai_npc_outbox_dedup`, `mid_agent_member_filter`,
   `ai_short_agent_per_region_routing` and confirm no divergence
   warnings in the AI smoke log.

Risk:

- Low-medium. The dual-write at dispatch time and the planet-resolver
  read at audit time are well-bounded changes.

#### Slice 4b2: Ledger Coverage For `DeployDeliverable`

Files to touch:

- `macrocosmo/src/ai/command_outbox.rs` (macro emission +
  decomposition glue)
- `macrocosmo/src/ai/npc_decision.rs` — `ledger_dedup_targets`
- Possibly `macrocosmo/src/ai/command_consumer.rs` for the
  primitive-chain markers if they need to thread the parent
  commitment id

Implementation:

1. At the point `deploy_deliverable` is added to the outbox (before
   eager decomposition), record a `CommitmentKind::DeployDeliverable`
   commitment targeting the destination system on the issuer
   empire's ledger.
2. Decide whether the primitives (`build_deliverable`,
   `load_deliverable`, `reposition`, `unload_deliverable`) each get
   their own commitment or whether they inherit the parent
   `DeployDeliverable` id. The simplest start is one parent
   commitment + leave primitives marker-less; the per-primitive
   variant can land in Slice 4b3 if dedup gaps surface.
3. Extend `ledger_dedup_targets` to fold
   `DeployDeliverable(System)` into a new `deploy_targets` return so
   the audit covers Rule 3.5's deploy dedup.
4. Resolve the deploy commitment when the chain completes: today
   the `ColonyEstablished` arrival of the deployed Core is the
   terminal signal. Add it to
   `resolve_commitments_from_observations`'s match arms.

Risk:

- Medium. The macro decomposition logic is already dense; layering
  the commitment writes without disturbing the chain ordering needs
  care.

#### Slice 4b3: Legacy Dedup Cut-over

Prerequisites: smoke run with
`RUST_LOG=macrocosmo::knowledge::commitment=warn` shows zero
`AI dedup divergence` entries across an extended AI smoke (≥1000
ticks, multi-region).

Implementation:

1. Remove the `outbox_survey_per_empire` /
   `outbox_colonize_per_empire` / `outbox_deploy_per_empire`
   precompute pass.
2. Remove the `pending_assignments` / `pending_ai_ship_commands`
   scans that populate the per-empire dedup sets.
3. Replace with direct `CommitmentLedger` queries via
   `ledger_dedup_targets` (now the authoritative source).
4. Delete `log_dedup_divergence`; promote
   `ledger_dedup_targets` from helper to dedup primitive.
5. Add a focused regression test: a fresh test app, no legacy
   markers ever written, verifies dedup answers correctly for
   Survey / Colonize-System / Colonize-Planet / Deploy.

Risk:

- High. This is the actual cut-over; revert is one PR away but the
  AI smoke suite must be the gate.

## Slice 5: Perception Facade, No Implicit Realtime Fallback

Purpose: make ground-truth reads explicit.

Files to touch:

- `macrocosmo/src/knowledge/ship_view.rs`
- `macrocosmo/src/ui/mod.rs`
- `macrocosmo/src/ui/outline.rs`
- `macrocosmo/src/ui/ship_panel.rs`
- `macrocosmo/src/ui/context_menu.rs`
- `macrocosmo/src/ui/situation_center/ship_ops_tab.rs`
- `macrocosmo/src/visualization/ships.rs`
- `macrocosmo/src/visualization/stars.rs`

Implementation:

1. Add `PerceptionMode`:

   ```rust
   pub enum PerceptionMode<'a> {
       Subject {
           subject: KnowledgeSubject,
           knowledge: Option<&'a KnowledgeStore>,
       },
       Omniscient,
   }
   ```

2. Add:

   - `ship_view_perceived(...)` for subject mode.
   - `ship_view_omniscient(...)` for explicit realtime mode.

3. Change `ship_view` / `ship_view_with_timing` behavior:
   - Do not immediately delete the old functions.
   - Mark them compatibility wrappers.
   - Add tests showing subject mode with missing knowledge returns `None`, not realtime.

4. Update UI/visualization call sites gradually:
   - `visualization/ships.rs`: keep `draw_ships_omniscient` as the only realtime path.
   - `visualization/stars.rs`: `ViewingEmpireResolver::is_god_view` maps to `PerceptionMode::Omniscient`; otherwise subject.
   - `ui/mod.rs`: replace `Option<&KnowledgeStore>` helper names with `Perception`.
   - panels consume `Perception`, not raw `Option<&KnowledgeStore>`.

5. Delete or deprecate helper paths that say "Omniscient returns None so callers fall through to realtime".
   - `None` should mean unknown in subject mode.
   - Omniscient should be a separate branch.

Tests:

- Existing:
  - `observer_mode_omniscient`
  - `observer_mode`
  - `ship_panel_ftl_leak`
  - `context_menu_ftl_leak`
  - `outline_tree_ftl_leak`
  - `situation_center_ftl_leak`
  - `ship_projection_render`
- Add:
  - subject mode missing knowledge does not expose realtime state.
  - omniscient mode still exposes realtime state.

Risk:

- High but localized to UI/visualization. The main risk is accidentally breaking startup frames that currently rely on realtime fallback. Those should render `Unknown` or skip rows instead.

## Slice 6: Belief Materialization API For Ships

Purpose: stop treating `ShipProjection` as a raw map consumers understand.

Files to touch:

- `macrocosmo/src/knowledge/ship_view.rs`
- `macrocosmo/src/knowledge/mod.rs`
- `macrocosmo/src/visualization/ships.rs`
- `macrocosmo/src/ui/*`
- `macrocosmo/src/ai/threat_query.rs`

Implementation:

1. Add `ShipBelief`:

   ```rust
   pub struct ShipBelief {
       pub ship: Entity,
       pub state: ShipSnapshotState,
       pub system: Option<Entity>,
       pub timing: Option<ShipViewTiming>,
       pub stamp: BeliefStamp,
       pub source: ShipBeliefSource,
   }
   ```

2. Implement materialization from:
   - own-ship projection + commitments,
   - foreign ship snapshot,
   - omniscient realtime path.

3. Update `ShipView` to become a presentation wrapper around `ShipBelief`, or make `ShipView` a thin alias until UI migration finishes.

4. Move `is_ship_overdue` in `ai/threat_query.rs` from `KnowledgeStore::get_projection` toward `ShipBelief` / active commitment deadlines.

Tests:

- `ship_projection_reconcile`
- `ship_projection_render`
- `ship_projection_polish`
- `ai_ship_overdue`

Risk:

- Medium. This is mostly abstraction once Slice 5 has removed fallback ambiguity.

## Slice 7: Knowledge Merge Skeleton

Purpose: introduce merge semantics before adding ship/fleet/region stores.

Files to add:

- `macrocosmo/src/knowledge/merge.rs`

Implementation:

1. Add:
   - `MergeContext`
   - `MergeReport`
   - `KnowledgeChannel`
   - `merge_observations_only`

2. Start with system and ship observations only.
3. Do not merge commitments except with an explicit `CommitmentMergePolicy::None`.
4. Add tests with two stores:
   - older observation then newer observation,
   - same entity different source,
   - merge preserves both provenance entries where applicable.

Risk:

- Low-medium if not wired into gameplay yet.

## Slice 8: Persistence

Purpose: persist the new structures after runtime semantics stabilize.

Files to touch:

- `macrocosmo/src/persistence/savebag.rs`
- `macrocosmo/src/persistence/save.rs`
- `macrocosmo/tests/ship_projection_persistence.rs`
- `macrocosmo/tests/save_load.rs`

Implementation:

1. Add `SavedCommitment`, `SavedCommitmentLedger`, eventually `SavedKnowledgeSubject`.
2. Add fields to `SavedKnowledgeStore`.
3. Bump `SAVE_VERSION` in `persistence/save.rs`.
4. Regenerate fixture if project convention requires it.
5. Keep compatibility with current live data by reconstructing commitments from:
   - active projections with intended state,
   - pending assignment components,
   - pending outbox entries,
   only if needed for one transitional load version. Since current loader hard rejects version mismatch, simple bump may be enough.

Tests:

- `ship_projection_persistence`
- `ship_projection_save_load_inflight_fact`
- `save_load`
- `fixtures_smoke`

Risk:

- Medium. Save format churn is controlled but fixture updates are noisy.

## Slice 9: Module Split After Semantics Stabilize

Purpose: split `knowledge/mod.rs` only after the semantic migration gives modules real boundaries.

Move:

- subject types -> `subject.rs`
- observation data -> `observation.rs`
- commitment data/rules -> `commitment.rs`
- ship belief/projection materialization -> `belief.rs` or `ship_belief.rs`
- perception facade -> `perception.rs`
- merge -> `merge.rs`
- snapshot producers -> `snapshot.rs`
- propagation systems -> `propagation.rs`
- destroyed/combat event knowledge -> `combat_events.rs`
- visibility tier map -> `visibility.rs`

Keep re-exports stable from `knowledge/mod.rs` until downstream call sites are migrated.

Tests:

Run the focused suite:

```text
cargo test -p macrocosmo --test knowledge
cargo test -p macrocosmo --test knowledge_observed
cargo test -p macrocosmo --test knowledge_observation_contract
cargo test -p macrocosmo --test ship_projection_reconcile
cargo test -p macrocosmo --test ship_projection_render
cargo test -p macrocosmo --test ship_destruction_observation_contract
cargo test -p macrocosmo --test ai_npc_no_double_survey_assignment
cargo test -p macrocosmo --test observer_mode_omniscient
```

## Recommended PR Order

1. `feat(knowledge): introduce subject and perception types`
2. `feat(knowledge): add commitment ledger and dual-write AI ship commitments`
3. `feat(knowledge): resolve commitments from observed facts`
4. `refactor(ai): audit inflight target dedup against commitment ledger` (Slice 4a)
5. `refactor(ui): replace knowledge option fallback with perception mode`
6. `refactor(knowledge): materialize ship belief through facade`
7. `feat(knowledge): add provenance-preserving merge skeleton`
8. `feat(persistence): persist knowledge commitments`
9. `refactor(knowledge): split subject observation commitment belief modules`
10. `feat(ai): record colonize_planet parent-system commitment` (Slice 4b1)
11. `feat(ai): record deploy_deliverable commitment for the macro` (Slice 4b2)
12. `refactor(ai): make commitment ledger authoritative for dedup` (Slice 4b3, gated on smoke)

Slices 1-4a may bundle into a single "knowledge core" commit; Slice 1.5
slots into that same commit. Slices 4b1-4b3 are intentionally separate
so each cut-over step can wait until smoke runs show no
`AI dedup divergence` warnings for the target command family.

## Hard Rules During Migration

- Do not add `FleetKnowledgeStore`, `ShipKnowledgeStore`, or `RegionKnowledgeStore`.
- Do not add new consumer-side realtime fallback paths.
- Do not make commitment resolution time-based unless the timeout itself is a modeled belief/observation.
- Do not remove `PendingAssignment` until `CommitmentLedger` covers all AI dedup tests.
- Do not persist new live fields until runtime behavior has focused tests.
- Keep `Omniscient` as the explicit and named ground-truth path.
