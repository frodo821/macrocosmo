# Code Review: 2026-05-25 AI Build and Resource Gate

## Scope

AI の建設命令、deliverable 展開、resource gate 周辺を中心に、直近 `origin/main` の潜在バグをレビューした。

Reviewed HEAD:

```text
cd08a7d Merge pull request #531 from frodo821/fix/hotfix-3-resource-gate
```

Commands run:

- `git status --short --branch`
- `git log --oneline --decorate -20`
- `rg` scans for `infrastructure_core`, `build_deliverable`, `deploy_deliverable`, `can_afford_design`, `fortify_system`, `BuildingQueue`, `ResourceGateParams`, and pending resource-gate paths
- Targeted source reads around `ai/command_consumer.rs`, `ai/mid_stance.rs`, `ai/short_stance.rs`, `ai/npc_decision.rs`, `ai/short_agent_runtime.rs`, `deep_space/mod.rs`, structure/ship Lua definitions, and related tests
- `cargo test -p macrocosmo --test ai_region_deadlock_hotfix -- --test-threads=1`
- `cargo test -p macrocosmo --test ai_resource_gate_hotfix -- --test-threads=1`
- `cargo test -p macrocosmo --test infrastructure_core lua_loads_infrastructure_core_deliverable_and_design -- --test-threads=1`

No code changes were made during the review.

## Summary

The recent resource-gate work compiles and the targeted regression tests pass, but three behavioral gaps remain. The highest-risk issue is the `infrastructure_core` build path: AI emits a deliverable id, while the handler resolves it through the ship-design registry. That mismatch is hidden by tests that inject a fake ship design with the deliverable id.

The other two findings are resource-gate completeness issues: `fortify_system` can still enqueue unaffordable ships, and planet-building pending costs are not subtracted even though Short Rule 5b now relies on the shared stockpile gate.

## Findings

### 1. AI `infrastructure_core` deliverable build path resolves against the wrong registry

Severity: High  
Status: Likely production bug from source review

`handle_build_deliverable` accepts `definition_id`, documented as a deliverable definition id such as `"infrastructure_core"`, but resolves cost and build time from `ShipDesignRegistry`.

Relevant files:

- `macrocosmo/src/ai/command_consumer.rs`
- `macrocosmo/src/ai/mid_stance.rs`
- `macrocosmo/scripts/structures/cores.lua`
- `macrocosmo/scripts/ships/core_hulls.lua`
- `macrocosmo/tests/ai_region_deadlock_hotfix.rs`
- `macrocosmo/tests/ai_decomposition_e2e.rs`

The production Lua definitions separate the ids:

- `define_deliverable { id = "infrastructure_core", cost = { minerals = 600, energy = 400 }, build_time = 120, cargo_size = 5, spawns_as_ship = core_hulls.infrastructure_core_v1 }`
- `define_ship_design { id = "infrastructure_core_v1", ... }`

The AI path emits `deploy_deliverable(infrastructure_core)`, which decomposes into `build_deliverable(definition_id = "infrastructure_core")`. The consumer then does `design_registry.get("infrastructure_core")`; in production that id belongs to the deliverable registry, not the ship-design registry.

Existing tests can pass while production fails because they inject a minimal `ShipDesignDefinition` whose id is `"infrastructure_core"` solely to satisfy the current handler lookup.

Impact:

- Rule 3.5 can emit frontier core deployment proposals.
- The outbox can decompose them into primitive commands.
- The game-side build step can still reject them as unknown deliverable definitions.
- AI expansion via infrastructure-core deployment may silently stall.

Fix proposal:

Use the deliverable/structure registry for `build_deliverable` metadata. The queued stockpile item should keep `design_id` or definition id as `"infrastructure_core"` for cargo/deploy lookup, but cost, build time, display name, and cargo size should come from `DeliverableMetadata`.

Also update the resource gate for `adapter.can_afford_design("infrastructure_core")`. Today that method is ship-design keyed and returns permissively for unknown ids, so it does not truly gate infrastructure-core affordability in production.

Recommended regression:

Create an end-to-end test that loads production Lua definitions and dispatches AI `deploy_deliverable(infrastructure_core)` through decomposition and `handle_build_deliverable`, then asserts a `BuildKind::Deliverable` order is queued with:

- `design_id == "infrastructure_core"`
- cost `600 / 400`
- `build_time_total == 120`
- `cargo_size == 5`

### 2. Rule 8 `fortify_system` bypasses the resource gate

Severity: Medium/High  
Status: Likely bug from source review

The resource-gate hotfix added affordability checks to several build-producing rules:

- Rule 3.5 `deploy_deliverable`
- Rule 5a `build_structure(shipyard)`
- Rule 6 `build_ship`
- Short Rule 5b planet buildings

Rule 8 remains gated only by shipyard availability and low ship count:

```text
can_build >= 1.0 && total_ships < colony_count * 2.0
```

It emits `fortify_system` without `design_id`. The handler then auto-picks a direct-buildable combat design and queues it. The handler has dedup logic, but no stockpile affordability check.

Relevant files:

- `macrocosmo/src/ai/mid_stance.rs`
- `macrocosmo/src/ai/command_consumer.rs`
- `macrocosmo/tests/ai_ship_build_queue.rs`

Impact:

- A bankrupt empire with a shipyard and low ship count can still enqueue one unaffordable combat ship.
- That order can sit in the build queue and starve progress.
- This violates the apparent hotfix intent that zero stockpile blocks AI build rules before queue mutation.

Fix proposal:

Prefer making Rule 8 emit a concrete `build_ship(design_id)` proposal only after choosing an affordable combat design. If `fortify_system` must remain the abstraction, extend the adapter with a method such as `affordable_fortify_design()` or add handler-side affordability checks using the same stockpile semantics.

Recommended regression:

Add a test where:

- `can_build_ships >= 1`
- `total_ships < colony_count * 2`
- stockpile is below `patrol_corvette` cost

Assert that no `fortify_system` or build queue order is produced.

### 3. Pending-aware resource gate excludes planet `BuildingQueue`

Severity: Medium  
Status: Confirmed design gap with current rule usage

`npc_decision_tick` subtracts pending costs from:

- colony `BuildQueue` ship/deliverable orders
- system-level `SystemBuildingQueue` building orders

It explicitly does not walk per-colony `BuildingQueue` for planet buildings such as `mine`, `farm`, and `power_plant`.

Relevant files:

- `macrocosmo/src/ai/npc_decision.rs`
- `macrocosmo/src/ai/short_stance.rs`
- `macrocosmo/src/ai/short_agent_runtime.rs`
- `macrocosmo/src/ai/command_consumer.rs`
- `macrocosmo/tests/ai_resource_gate_hotfix.rs`

The comment says this is acceptable until adapter rules need it. Short Rule 5b already needs it: it uses `adapter.can_afford_building(building_id)` before emitting planet-building orders. `run_short_agents` feeds that adapter from `RegionShortInputs.current_minerals/current_energy`, which come from the pending-adjusted stockpile that excludes planet `BuildingQueue`.

The handler dedups only the same building id at the same colony. Different building ids are intentionally allowed. That means a pending `mine` can consume most resources, while a later `farm` or `power_plant` still passes the gate because the pending mine cost was not subtracted.

Impact:

- Short Rule 5b can overcommit cross-building planet orders.
- The resource gate gives a stronger guarantee for ships, deliverables, and system buildings than for planet buildings.
- Existing tests cover same-id dedup and different-id allowance, but not pending-cost subtraction for planet buildings.

Fix proposal:

Fold per-colony `BuildingQueue` construction orders into `ResourceGateParams`, scoped by the colony host system just like colony `BuildQueue`. If Bevy system-param limits are the blocker, introduce a `SystemParam` bundle for resource-gate inputs rather than leaving the rule with inconsistent accounting.

Recommended regression:

Add a test that queues a pending planet building, runs the NPC decision input preparation, and asserts that the region's `current_minerals/current_energy` are reduced before Short Rule 5b evaluates another planet-building proposal.

## Test Results

Targeted tests passed:

```text
cargo test -p macrocosmo --test ai_region_deadlock_hotfix -- --test-threads=1
4 passed

cargo test -p macrocosmo --test ai_resource_gate_hotfix -- --test-threads=1
6 passed

cargo test -p macrocosmo --test infrastructure_core lua_loads_infrastructure_core_deliverable_and_design -- --test-threads=1
1 passed
```

The workspace still emits a large warning volume during these test builds. The warning volume was not the focus of this review, but it continues to reduce signal when scanning for real regressions.

## Recommended Next Steps

1. Fix `build_deliverable` to use deliverable metadata instead of `ShipDesignRegistry`.
2. Add a production-definition regression for AI `deploy_deliverable(infrastructure_core)` through queue insertion.
3. Gate Rule 8 `fortify_system` on affordability or convert it to an explicit affordable `build_ship` decision.
4. Include per-colony planet `BuildingQueue` pending costs in the resource gate.
5. Add focused tests for Rule 8 bankrupt behavior and planet-building pending-cost subtraction.
