# Implementation Plan: Issue #305 — S-11 Casus Belli System

_Depends on #302 (S-8 DiplomaticOption framework). Design spec: `docs/diplomacy-design.md` v2 §4._

---

## Overview

Introduce `define_casus_belli` Lua API and the Rust runtime that evaluates casus belli conditions, triggers auto-war transitions, computes available demands, and exposes end-war scenarios. A single casus belli is locked per war.

## 1. CasusBelli Definition + Registry

**File:** `macrocosmo/src/faction/casus_belli.rs` (new module)

- `CasusBelliDefinition { id, name, evaluate: Condition, auto_war: bool, base_demands: Vec<DemandItem>, additional_demands: Vec<AdditionalDemand>, end_scenarios: Vec<EndScenario> }`
- `DemandItem { kind: String, value: Option<f64>, scope: Option<String> }`
- `AdditionalDemand { unlocked_when: Condition, items: Vec<DemandItem> }`
- `EndScenario { id: String, label: String, available: Condition, on_select_event: String }`
  - `on_select_event` stores the event id string (no closure) — the `on_select` Lua function is invoked at define-time to register the event dispatch, but at runtime the system emits the stored event id
- `CasusBelliRegistry` — `Resource`, `HashMap<String, CasusBelliDefinition>`
- Re-export from `faction/mod.rs`

## 2. ActiveWar State

**File:** `macrocosmo/src/faction/casus_belli.rs`

- `ActiveWar { pub actor: Entity, pub target: Entity, pub casus_belli_id: String, pub started_at: i64 }`
- `ActiveWars` — `Resource`, `Vec<ActiveWar>`
- Lookup helpers: `find_war(actor, target) -> Option<&ActiveWar>`, `is_at_war(a, b) -> bool`
- When war ends (via end scenario event handler), the entry is removed from `ActiveWars` and relations transition to Peace

## 3. Lua API: `define_casus_belli`

**File:** `macrocosmo/src/scripting/casus_belli_api.rs` (new)

- Parse Lua table fields: `id`, `name`, `evaluate` (Condition via `condition_parser`), `auto_war` (bool, default false), `base_demands` (array of `{kind, value?, scope?}`), `additional_demands` (array of `{unlocked_when, items}`), `end_scenarios` (array of `{id, label, available, on_select}`)
- `on_select` is a Lua function — at parse time, invoke it against a builder context to capture the event id to emit; store only the event id string in `EndScenario`
- Accumulator: `Vec<CasusBelliDefinition>` in `ScriptEngine` globals, drained into `CasusBelliRegistry` by `load_casus_belli_definitions` startup system
- Startup ordering: after `load_all_scripts`, before `run_lifecycle_hooks`
- Wire into `scripting/mod.rs` `setup_globals`

**Lua script:** `scripts/factions/casus_belli.lua` (new)

- Define at least one CB for testing: `broken_treaty` (as shown in design doc §4)
- Add `require("factions.casus_belli")` to `scripts/init.lua`

## 4. evaluate_casus_belli System

**File:** `macrocosmo/src/faction/casus_belli.rs`

- System in `Update`, `.after(advance_game_time)`
- Each tick: for every CB definition, for every ordered faction pair `(a, b)`, evaluate `cb.evaluate` Condition with `EvalContext` scoped to `{actor=a, target=b}`
- If evaluate returns true and `auto_war == true`:
  - Check `ActiveWars` — if `(a, b)` not already at war, create `ActiveWar { actor: a, target: b, casus_belli_id: cb.id, started_at: clock.elapsed }`, push to `ActiveWars`
  - Transition relations to War via existing `declare_war` (immediate, no delay for auto-war — the CB itself is the justification)
- If evaluate returns true and `auto_war == false`: CB is "available" — stored in a transient set for UI to query (or computed on demand by UI)
- Performance note: faction pair count is small in pre-alpha (< 10 factions), full O(CB * pairs) is acceptable

## 5. Demand Computation

**File:** `macrocosmo/src/faction/casus_belli.rs`

- `pub fn available_demands(war: &ActiveWar, registry: &CasusBelliRegistry, eval_ctx: &EvalContext) -> Vec<DemandItem>`
  1. Look up `war.casus_belli_id` in registry
  2. Start with `cb.base_demands.clone()`
  3. For each `additional_demand`, evaluate `unlocked_when` — if true, append its `items`
  4. Return merged list (dedup by kind via `kind` string equality; if duplicate kinds, keep all — merge is deferred to `NegotiationItemKind.merge` from a future issue)

## 6. End Scenario Mechanics

**File:** `macrocosmo/src/faction/casus_belli.rs`

- `pub fn available_end_scenarios(war: &ActiveWar, registry: &CasusBelliRegistry, eval_ctx: &EvalContext) -> Vec<&EndScenario>`
  - Filter `cb.end_scenarios` where `available` Condition evaluates true
- `pub fn end_war(active_wars: &mut ActiveWars, relations: &mut FactionRelations, actor: Entity, target: Entity)`
  - Remove the `ActiveWar` entry
  - Transition `relations` to Peace (both directions)
- End scenario `on_select` emits an event (e.g. `diplomacy:open_negotiation`); the event handler chain (#302 pipeline) handles the actual negotiation and eventually calls `end_war`

## 7. Single CB per War Constraint

- `auto_war = true`: the first CB that evaluates true for a pair creates the `ActiveWar` with that CB locked
- Manual declaration (future UI): player selects exactly one CB from the "available" set; `ActiveWar` is created with that CB
- `evaluate_casus_belli` system skips pairs already in `ActiveWars` for auto-war purposes
- No mechanism to change `casus_belli_id` on an existing `ActiveWar`

## 8. Persistence

**File:** `macrocosmo/src/persistence/savebag.rs`

- `SavedActiveWar { actor: SavedEntityRef, target: SavedEntityRef, casus_belli_id: String, started_at: i64 }`
- Add `active_wars: Vec<SavedActiveWar>` to save bag (or as a separate resource field)

**Files:** `persistence/save.rs`, `persistence/load.rs`

- Serialize/deserialize `ActiveWars` resource
- Entity ref remapping for actor/target

## Commit Sequence (6 commits)

1. **`[305] CasusBelliDefinition + CasusBelliRegistry + ActiveWar/ActiveWars`** — new module, data types, registry resource, ActiveWars resource, lookup helpers
2. **`[305] define_casus_belli Lua API + parser`** — casus_belli_api.rs, wire into setup_globals, load_casus_belli_definitions startup system
3. **`[305] Lua script: broken_treaty CB definition`** — scripts/factions/casus_belli.lua, init.lua require
4. **`[305] evaluate_casus_belli system + auto-war transition`** — Update system, Condition evaluation per faction pair, auto-war ActiveWar creation
5. **`[305] demand computation + end scenario helpers`** — available_demands, available_end_scenarios, end_war
6. **`[305] persistence + tests`** — SavedActiveWar, save/load, all integration tests

## Test Plan (8 tests)

**File:** `macrocosmo/tests/casus_belli.rs` (new)

1. `test_define_casus_belli_lua_parse` — Lua define_casus_belli produces a valid CasusBelliDefinition in the registry
2. `test_evaluate_cb_true_auto_war` — CB with auto_war=true evaluates true → ActiveWar created, relations transition to War
3. `test_evaluate_cb_true_no_auto_war` — CB with auto_war=false evaluates true → no ActiveWar created (available only)
4. `test_evaluate_cb_false` — CB condition not met → no war transition
5. `test_single_cb_per_war` — second CB evaluating true for same pair does not replace the locked CB
6. `test_available_demands_base_only` — war with no additional unlocked → returns base_demands only
7. `test_available_demands_with_unlocked` — additional_demand unlocked_when true → items appended to result
8. `test_end_war_removes_active_war` — end_war removes entry from ActiveWars and transitions to Peace

## Pitfall List

- **Condition atom availability:** `evaluate` conditions may reference diplomacy atoms (`actor_has_modifier`, etc.) that require a separate Condition atom expansion issue. Initial implementation can use existing atoms only; new atoms gated behind the condition expansion issue.
- **Demand kind merge:** Full merge (via `NegotiationItemKind.merge`) depends on a future issue. This implementation collects raw items without merging.
- **End scenario event chain:** `on_select` emits events into the #302 DiplomaticOption event pipeline. Ensure #302 lands first or stub the event bus.
- **EvalContext for diplomacy:** The `evaluate` Condition needs actor/target scope in EvalContext. May need to extend `ConditionScope` or `ScopeData` with faction pair context.

## Files to Modify

- `macrocosmo/src/faction/casus_belli.rs` (new)
- `macrocosmo/src/faction/mod.rs` (re-export, plugin registration)
- `macrocosmo/src/scripting/casus_belli_api.rs` (new)
- `macrocosmo/src/scripting/mod.rs` (wire globals + startup system)
- `macrocosmo/src/persistence/savebag.rs`
- `macrocosmo/src/persistence/save.rs`
- `macrocosmo/src/persistence/load.rs`
- `scripts/factions/casus_belli.lua` (new)
- `scripts/init.lua` (require)
- `macrocosmo/tests/casus_belli.rs` (new)
