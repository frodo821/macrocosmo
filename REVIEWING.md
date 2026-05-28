# Re-review: Knowledge Redesign Slices 1-4

Date: 2026-05-29

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

No remaining blocking implementation gap was found beyond the already
planned Slice 4b3 cut-over:

- keep the legacy dedup union active for now
- run an extended smoke with `AI dedup divergence` logging enabled
- only then remove `outbox_*_per_empire`, `pending_assignments`, and
  `pending_ai_ship_commands` scans from the authoritative dedup path

## Previously Reported Issues

Resolved:

- Loaded empires now get `KnowledgeNode` via `backfill_knowledge_node`.
- `KnowledgeNode`, `KnowledgeSubject`, and `KnowledgeScope` are registered for reflection.
- Commitments now record `basis_observed_at`, `expected_effect_at`, and `expected_resolution_at`.
- `resolve_commitments_from_observations` now uses the current empire's `CommsParams`.
- Actor-loss resolution is now subject-scoped in `fail_commitments_for_actor`.
- `Colonize(Planet)` dispatch now writes both planet-keyed and system-keyed commitments.
- `DeployDeliverable(System)` commitments are recorded before macro decomposition and resolved by `ColonyEstablished`.

Still intentionally deferred:

- Slice 4b3 legacy dedup cut-over, gated on a 1000-tick smoke with zero divergence.
- `reconcile_ship_projections` keeps the pre-existing single-`CommsParams` shortcut; migrate it with the broader fact-arrival cleanup so projection and commitment resolution stay aligned.
- Commitment persistence is still deferred to the planned persistence slice.

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
