# Re-review: Knowledge Redesign Slices 1-4

Date: 2026-06-04

Scope reviewed:

- `macrocosmo/src/knowledge/{subject,perception,commitment}.rs`
- `macrocosmo/src/knowledge/mod.rs`
- `macrocosmo/src/player/mod.rs`
- `macrocosmo/src/setup/mod.rs`
- `macrocosmo/src/ai/{assignments,command_outbox,npc_decision}.rs`
- `macrocosmo/src/reflect_registration.rs`
- `macrocosmo/tests/{knowledge_subject,knowledge_commitments}.rs`
- related save/load, projection, and AI command/decomposition tests

## Findings

No remaining blocking implementation gap was found in the completed
Slices 1-4 path after the Slice 4b3 cut-over.

## Previously Reported Issues

Resolved:

- Loaded empires now get `KnowledgeNode` via `backfill_knowledge_node`.
- `KnowledgeNode`, `KnowledgeSubject`, and `KnowledgeScope` are registered for reflection.
- Commitments now record `basis_observed_at`, `expected_effect_at`, and `expected_resolution_at`.
- `resolve_commitments_from_observations` now uses the current empire's `CommsParams`.
- Actor-loss resolution is now subject-scoped in `fail_commitments_for_actor`.
- `Colonize(Planet)` dispatch now writes both planet-keyed and system-keyed commitments.
- `DeployDeliverable(System)` commitments are recorded before macro decomposition and resolved by `ColonyEstablished`.

Completed after the original review:

- Slice 4b3 legacy dedup cut-over completed; the authoritative dedup path now uses the commitment ledger.
- Added a 1000-tick per-region NPC smoke that asserts no duplicate active commitments.
- Commitment ledger persistence landed in `SavedKnowledgeStore::commitments`; `SAVE_VERSION` is now 21 and the minimal fixture was regenerated.
- Fact arrival and `reconcile_ship_projections` now use the current faction / empire's own `CommsParams`.

Still intentionally deferred:

- Slice 5: tighten the perception facade and remove remaining implicit realtime fallback ambiguity.
- Slice 6+: ship belief materialization, merge skeleton, and later module split once semantic boundaries stabilize.

## Design Note

The implementation now preserves the intended separation between:

- **store owner**: the entity holding this `KnowledgeStore`
- **commitment subject**: the entity whose intention is recorded
- **actor**: the entity executing the intention

The subject filter in actor-loss resolution is the key guard for future
merged-store or multi-subject stores.

## Validation Run

Passed:

```text
cargo test -p macrocosmo --test knowledge_subject
cargo test -p macrocosmo --test knowledge_commitments
cargo test -p macrocosmo --test ai_npc_outbox_dedup
cargo test -p macrocosmo --test ai_npc_no_double_survey_assignment
cargo test -p macrocosmo --test save_load
cargo test -p macrocosmo --test ship_projection_reconcile
cargo test -p macrocosmo --test ai_decomposition_e2e
cargo test -p macrocosmo --test ai_command_lightspeed
```

Observed only pre-existing warnings:

- deprecated egui APIs
- private interface warning for `StarVisual`
- dropping Copy type in `scripting/gamestate_scope.rs`
