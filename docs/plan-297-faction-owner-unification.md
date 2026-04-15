# Implementation Plan: Issue #297 — S-2 FactionOwner Unified Attachment

_Prepared 2026-04-15 by Plan agent. #295 (S-1 Sovereignty derived view) already merged; `Sovereignty.owner` is now derived from Core ship's `FactionOwner` via `system_owner()`. This plan extends `FactionOwner` attachment to Colony / SystemBuildings / DeepSpaceStructure / Ship so that all empire-owned entity classes share one diplomatic-identity component ahead of Phase 2 multi-faction._

## 1. Current State (grep-verified against `main`)

### `FactionOwner` component — `macrocosmo/src/faction/mod.rs:143-144`
```rust
#[derive(Component, Clone, Copy, Debug)]
pub struct FactionOwner(pub Entity);
```
Doc at L136-142 explicitly declares "combat/ROE checks consult `FactionRelations` keyed by this owner. Entities without `FactionOwner` have no diplomatic identity and are skipped by combat". That doc becomes a *lie* after S-2: colonies/structures will carry one too.

### Who currently attaches `FactionOwner`
- Hostile spawn: `macrocosmo/src/galaxy/generation.rs:676-682` (`(AtSystem, HostileHitpoints, HostileStats, Hostile, FactionOwner)` bundle)
- Backfill fallback: `macrocosmo/src/faction/mod.rs:1313+` (`attach_hostile_faction_owners` — #168 relic, still active per #293 follow-up)
- No other prod call sites. Ships/Colonies/SystemBuildings/DeepSpaceStructure never get it today.

### `Ship.owner: Owner` — `macrocosmo/src/ship/mod.rs:349-388`
```rust
pub enum Owner { Empire(Entity), Neutral }
```
Stored as a `Ship` struct field (NOT a separate component). Set by `spawn_ship` at `src/ship/mod.rs:549-617` via `owner` parameter. Readers:
- `src/ship/combat.rs:200-202` — `let Owner::Empire(faction_entity) = ship.owner else { return None; };`
- `src/ship/pursuit.rs:148-158` — `resolve_ship_faction(&ship.owner, faction_owner: Option<&FactionOwner>)` — already prefers `Owner::Empire` then falls back to `FactionOwner` component
- `src/ship/command.rs:313-314`, `src/ship/deliverable_ops.rs:205`, `src/visualization/territory.rs:250`, `src/scripting/gamestate_view.rs:232`, `src/setup/mod.rs:893` (test assertion)
- Savebag: `src/persistence/savebag.rs:471-517` (`SavedShip.owner: SavedOwner`)

### Colony `FactionOwner` — **none** today
### SystemBuildings `FactionOwner` — **none** today
### DeepSpaceStructure ownership — `DeepSpaceStructure.owner: Owner` struct field (`src/deep_space/mod.rs:40-44`), set via `spawn_deliverable_entity` param (`src/deep_space/mod.rs:490-532`). Savebag round-trips it as `SavedOwner` (`savebag.rs:2337-2357`).

### Savebag machinery (already prepared for S-2)
- `SavedComponentBag.faction_owner: Option<SavedFactionOwner>` — `savebag.rs:4309`
- Save path: `save.rs:355-357` — already iterates every entity and writes `FactionOwner` into the bag unconditionally (good news: **no save.rs change needed**)
- Load path: `load.rs:257-259` — already inserts `FactionOwner` back onto any entity whose bag has it (**no load.rs change needed**)
- `SAVE_VERSION = 1` — `save.rs:71`. No schema change: `FactionOwner` is already a per-entity optional field. Old saves without it on colonies simply deserialize with `faction_owner = None`.

### Spawn paths to touch (issue-required)
1. **`spawn_capital_colony`** — `src/colony/colonization.rs:58-129`. Spawns `Colony` bundle (no owner today). Runs on `Startup` after `generate_galaxy`. PlayerEmpire exists by then (spawned earlier in Startup via `spawn_player_empire`, `src/player/mod.rs:45`).
2. **`spawn_colony_on_planet`** — `src/setup/mod.rs:351-385`. Helper used by `apply_game_start_actions` (Lua `colonize_planet`). Called with `world: &mut World` at `src/setup/mod.rs:528`.
3. **`tick_colonization_queue`** — `src/colony/colonization.rs:134-268`. Spawns new Colony at L208-237 on build-queue completion. Has `ResourceStockpile` on system entity but no direct empire handle.
4. **Colony-ship landing** — `src/ship/settlement.rs:159-190` (`process_settling`). Spawns Colony on a settling colony ship's system. Has the settling `Ship` entity at L54; can pull `ship.owner` or look up `FactionOwner` on the ship.
5. **`spawn_capital_colony` scaffold's SystemBuildings insertion** — `src/colony/colonization.rs:114-127` (SystemBuildings added to capital StarSystem entity).
6. **`process_settling` SystemBuildings insertion** — `src/ship/settlement.rs:207-214` (conditional insert on first colony in system).
7. **`apply_game_start_actions` SystemBuildings paths** — `src/setup/mod.rs:541-544` (defensive re-insert if stockpile missing) and `src/setup/mod.rs:721-724` (test-only `spawn_test_capital` path).
8. **`spawn_deliverable_entity`** — `src/deep_space/mod.rs:490-532`. Already takes `owner: Owner` param. Must add `FactionOwner` component alongside, derived from `owner`.

### Test-only paths (informational; must not break)
- `tests/common/mod.rs:932` — test spawn of SystemBuildings
- `tests/colony.rs:1743, 1778, 1807, 2324, 2394` — direct `SystemBuildings { slots: ... }` literals
- `tests/ship.rs:313` — same
- `tests/common/mod.rs:1314 spawn_test_ship` — `Owner::Neutral` by default; does NOT insert `FactionOwner`

## 2. Design Decisions

### (A) Attach `FactionOwner` as a **sibling component** on all four entity classes

All four spawn families get `FactionOwner(faction_entity)` inserted alongside the existing bundle. We do **not** replace the existing `owner: Owner` field on `Ship` / `DeepSpaceStructure` — those remain (see §D).

**Rationale**: `FactionOwner` is already the canonical diplomatic-identity component (hostiles, and in S-1 the Core ship that derives Sovereignty). One component, one query, across every owned class. `Colony.owner` / `SystemBuildings.owner` struct fields are *not* introduced — the ECS sibling component is the single source of truth for these new classes.

### (B) Resolving the faction entity at each spawn site

- **`spawn_capital_colony`**: add `empire_q: Query<Entity, With<PlayerEmpire>>` param. `empire_q.single()` → faction entity. Warn + skip `FactionOwner` if missing (same defensive pattern as `building_queue.rs:309-312 ship_owner`). Phase 2-ready: `PlayerEmpire` is the "current player faction" marker; multi-faction refactor rewrites this lookup, not the spawn-site structure.
- **`spawn_colony_on_planet`**: takes `world: &mut World`. Resolve faction via `world.query_filtered::<Entity, (With<Empire>, With<Faction>)>` filtered by `faction_id` parameter (matches the idiom at `src/setup/mod.rs:620-625`). Fall through to `PlayerEmpire` then warn-and-skip. Pass `faction_entity: Option<Entity>` into the helper and insert conditionally.
- **`tick_colonization_queue`**: the colony being built inherits sovereignty from its *source colony* (L190 `order.source_colony`). Add `source_colony_owner: Query<&FactionOwner>` (or join) — `colonies_q.get(order.source_colony)` → look up `FactionOwner` by entity. This is correct and multi-faction-safe: a colony founded from an NPC colony should inherit the NPC owner, not `PlayerEmpire`. Fallback: same defensive warn-skip.
- **`process_settling`**: resolve from the settling `Ship`. Two options:
  - (preferred) Query `Option<&FactionOwner>` on the ship tuple at L31, plus `ship.owner` fallback. Use whichever is present, preferring `FactionOwner` (matches `resolve_ship_faction` precedence after §D).
  - Or consult just `ship.owner: Owner::Empire(e) → e`.
  During the transition both exist on ships; prefer component.
- **`spawn_deliverable_entity`**: already receives `owner: Owner`. When `owner = Owner::Empire(e)`, insert `FactionOwner(e)` into the bundle at L505-517. `Owner::Neutral` → no component (matches hostile-without-owner semantics).

### (C) SystemBuildings attachment — same entity as StarSystem

`SystemBuildings` is inserted *onto the StarSystem entity* (not a standalone entity), alongside `ResourceStockpile`. The StarSystem itself has `Sovereignty` (system-level). Decision: the **StarSystem entity** gets the `FactionOwner` component, not the `SystemBuildings` specifically. This is the semantically correct target: "this star system belongs to this faction". Inserting `FactionOwner` on SystemBuildings would be redundant, since SystemBuildings has no independent identity.

All three sites that insert `SystemBuildings` therefore also insert `FactionOwner` on the same `commands.entity(capital_entity).insert(...)` / `commands.entity(system_entity).insert(...)` batch.

Tension to note: #295 (S-1) defines `Sovereignty.owner` as derived *from Core ship presence*, not from static system assignment. The new `FactionOwner` on a StarSystem entity is a *separate* concept ("the faction that administratively owns this system's buildings"). The two can disagree — e.g. enemy Core ship sits in your system → `Sovereignty.owner = Some(enemy)` but `FactionOwner` (buildings/colony administrative owner) = you. This is deliberate and matches the S-1 design note "removing the Core ship removes sovereignty — colony presence alone does not confer ownership" (authority.rs:121-124). The two components model orthogonal axes; this issue is explicitly out-of-scope for cascade logic (S-10).

### (D) `Ship.owner: Owner` — **keep; do not migrate in this PR**

**Decision: retain the `Owner` enum and `Ship.owner` field; additionally insert `FactionOwner` on every ship spawned via `spawn_ship` with `Owner::Empire(e)`.**

Reasons:
1. `spawn_ship` is called from 6+ prod sites plus 10+ tests and 2 Lua scripts (`scripts/factions/init.lua`, `scripts/lib/capital.lua` via `ctx.system:spawn_ship`). Blast radius for signature change is high.
2. `Owner::Neutral` is a meaningful state (test-spawned ships, early-game scout). `FactionOwner` has no `Neutral` variant — dropping the component conveys "unaffiliated" correctly but forces every reader to add `Option<&FactionOwner>` to its query, plus defensive `None`-branch logic. That migration is mechanical but large.
3. `resolve_ship_faction` (pursuit.rs:153) already reads *both* (`Owner::Empire` first, `FactionOwner` second). This means a ship can carry both during transition with no behavioral drift.
4. `Ship.owner` round-trips via `SavedShip.owner: SavedOwner` independently of the savebag-level `faction_owner` field. Savebag already supports both side-by-side.

Scope of Ship change in this PR: **`spawn_ship` inserts `FactionOwner(e)` when `owner == Owner::Empire(e)`**. No API break. Non-empire ships (`Owner::Neutral`) get no component, same as today. A follow-up issue (S-11 or similar) will handle migration and deletion of the enum.

### (E) `entity_owner(world, entity) -> Option<Entity>` helper

Issue requires a "query helper: `entity_owner(world, entity) -> Option<Entity>`".

**Location**: `macrocosmo/src/faction/mod.rs`, immediately after `system_owner` (~L975). Keeps all owner-resolution helpers co-located.

**Signature (two variants for ergonomics)**:
```rust
/// #297 (S-2): Resolve the faction entity owning `entity`. Consults, in order:
/// 1. A `FactionOwner` component (canonical — applies to colony, ship,
///    SystemBuildings-bearing StarSystem, DeepSpaceStructure, Hostile).
/// 2. `Ship.owner = Owner::Empire(e)` if the entity is a Ship (transitional
///    until S-11 removes the `Owner` enum).
///
/// Returns `None` for wholly unaffiliated entities (e.g. `Owner::Neutral`
/// ships with no `FactionOwner`, or entities that never received one).
pub fn entity_owner(world: &World, entity: Entity) -> Option<Entity> {
    let e = world.get_entity(entity).ok()?;
    if let Some(fo) = e.get::<FactionOwner>() {
        return Some(fo.0);
    }
    if let Some(ship) = e.get::<crate::ship::Ship>() {
        if let crate::ship::Owner::Empire(f) = ship.owner {
            return Some(f);
        }
    }
    None
}
```

System-facing variant (for hot paths inside Bevy systems where a `&World` is unavailable):
```rust
pub fn entity_owner_from_query(
    entity: Entity,
    faction_owners: &Query<&FactionOwner>,
    ships: &Query<&crate::ship::Ship>,
) -> Option<Entity> { /* same precedence */ }
```

Ship-specific precedence tweak: **`FactionOwner` wins over `Ship.owner`** (opposite of current `resolve_ship_faction` which prefers `Owner::Empire` then falls back). Rationale: post-S-2, all empire ships carry both. When they agree, order doesn't matter. When they disagree (pathological, shouldn't happen), `FactionOwner` is the forward-compatible answer since `Owner` will be deleted later. Leave `resolve_ship_faction` unchanged inside `pursuit.rs` for now (its own semantics already work) — `entity_owner` is the new public API, `resolve_ship_faction` remains a pursuit-internal helper.

### (F) SAVE_VERSION — **do not bump**

No wire-format change: `SavedComponentBag.faction_owner` has existed since pre-S-2 and is an `Option`. Old saves have `None` on colonies/ships; new saves have `Some`. Load path at `load.rs:257` just inserts when present. Round-trip is automatic.

If any *behavior* depends on absence-vs-presence of `FactionOwner` on a colony after load (none identified in current code), add a "backfill on load" system analogous to `attach_hostile_faction_owners` — but grep shows no such dependency, so **no backfill system needed**.

## 3. Commit Series (4 commits, ~+240 / -15 lines)

### Commit 1: `faction::entity_owner` helper + tests
- `macrocosmo/src/faction/mod.rs`: +50 lines
  - `entity_owner(&World, Entity) -> Option<Entity>` immediately after `system_owner` (~L975)
  - `entity_owner_from_query(Entity, &Query<&FactionOwner>, &Query<&Ship>) -> Option<Entity>`
  - Unit tests (in `#[cfg(test)] mod tests` at L976): (a) bare entity → None, (b) entity with `FactionOwner` → Some, (c) Ship with `Owner::Empire(e)` only → Some(e), (d) Ship with both, agreeing → Some, (e) Ship with `Owner::Neutral` + no component → None
- lines: +50 / -0
- risk: low. Purely additive, zero callers yet.
- independent compile: yes

### Commit 2: Colony + SystemBuildings spawn paths get `FactionOwner`
- `macrocosmo/src/colony/colonization.rs`: `spawn_capital_colony` (+empire_q param, +2 `.insert(FactionOwner(e))` calls for Colony entity and StarSystem/SystemBuildings entity); `tick_colonization_queue` (+lookup source_colony's FactionOwner, +1 `.insert` on new Colony)
- `macrocosmo/src/colony/mod.rs:43`: `spawn_capital_colony` system signature — no change (new `Query` param is transparent to plugin wiring)
- `macrocosmo/src/setup/mod.rs:351-385 spawn_colony_on_planet`: +`faction_entity: Option<Entity>` param, +conditional `.insert(FactionOwner(e))` on new Colony entity. Update caller at L528 to resolve empire via same `empire_by_faction`/`PlayerEmpire` chain already used at L619-641.
- `macrocosmo/src/setup/mod.rs:541-544`: when defensive SystemBuildings insert fires, add `FactionOwner` on `capital_entity`.
- `macrocosmo/src/ship/settlement.rs:159-214 process_settling`: +`Option<&FactionOwner>` on ship tuple query (L31), insert on both new Colony and (when newly added) StarSystem entity.
- lines: +75 / -5
- risk: medium. Touches 4 spawn sites; care needed to preserve `SystemState` / `Commands` flow in `apply_game_start_actions` (the helper takes `&mut World` not `Commands`).
- independent compile: yes (Commit 1 not strictly required, but tests reference `entity_owner`)

### Commit 3: Ship + DeepSpaceStructure `FactionOwner` attachment
- `macrocosmo/src/ship/mod.rs:549-617 spawn_ship`: after `commands.entity(ship_entity).insert((Ship { ..., owner, ... }, ...))`, add `if let Owner::Empire(e) = owner { commands.entity(ship_entity).insert(FactionOwner(e)); }`. No signature change — all existing callers unaffected.
- `macrocosmo/src/deep_space/mod.rs:490-532 spawn_deliverable_entity`: identical pattern — when `owner = Owner::Empire(e)`, include `FactionOwner(e)` in the spawn bundle.
- `macrocosmo/tests/common/mod.rs:1314 spawn_test_ship`: no change (stays `Owner::Neutral`, no component). Callers that want a faction-owned test ship already manually insert `FactionOwner` today (see `tests/combat_scenarios.rs`, `tests/pursuit.rs`).
- lines: +20 / -0
- risk: low. `spawn_ship` is hot; insertion is conditional so `Owner::Neutral` tests are unaffected.
- independent compile: yes

### Commit 4: Regression tests
- `macrocosmo/tests/faction_owner_unification.rs` (new file): +100 lines
  1. `spawn_capital_colony` smoke: after Startup, every `Colony` carries `FactionOwner(player_empire)`; capital StarSystem carries it too
  2. `tick_colonization_queue`: new colony inherits `FactionOwner` from source colony (explicit test with mock empire A + mock empire B)
  3. `process_settling`: colony ship with `Owner::Empire(e)` → new Colony has `FactionOwner(e)`
  4. `spawn_ship`: `Owner::Empire(e)` → ship has `FactionOwner(e)`; `Owner::Neutral` → no component
  5. `spawn_deliverable_entity`: mirrors ship case
  6. `entity_owner` helper: integration test across all five entity classes
- `macrocosmo/tests/save_load.rs`: +50 lines — round-trip test: spawn a colony + SystemBuildings + DeepSpaceStructure with `FactionOwner`, save to bytes, load, assert all three still carry correct faction entity (after remap). Piggyback on existing `round_trip_*` test scaffolding.
- lines: +150 / 0
- risk: low
- independent compile: yes (depends on Commits 2 & 3)

### Alternative: single PR vs. split

Recommendation: **single PR, 4 commits**. Reasons:
- Total size ~+240/-15 lines is small for this codebase (compare plan-293 which was 7 commits, +500/-400).
- Commits 2+3 each introduce a *new invariant* (colonies have FactionOwner, ships have FactionOwner) that would leave the codebase in a half-migrated state between PRs — confusing for readers and for Phase 2 work.
- Commit 4 (tests) must ship with 2+3 anyway to prevent regression windows.
- Savebag needs no change, so no "wire format bump" concerns that would force a split.

## 4. Semantic merge-conflict watchlist

### Known parallel work
Run `git log --oneline -30 main` and `gh pr list --state open` before starting. Expected hot zones:

1. **`src/colony/colonization.rs`** — touched by #250 (base-production refactor, already merged) and recent job-system work. The `spawn_capital_colony` bundle literal at L81-112 and `tick_colonization_queue` Colony spawn at L208-237 are both *multi-field bundle literals*: any concurrent PR that adds another component will conflict trivially at the last `),` before the closing `));`.
2. **`src/ship/settlement.rs:159-190`** — same multi-field bundle pattern. `process_settling` has been recently churning (#52/#56/#293 all touched it).
3. **`src/setup/mod.rs:351-385 spawn_colony_on_planet`** and **:721-724 spawn_test_capital** — test-fixture code. Low concurrent-PR traffic but the signature change on `spawn_colony_on_planet` will conflict with any PR that touches that helper's call site.
4. **`src/ship/mod.rs:549-617 spawn_ship`** — any PR adding a component to the ship bundle (e.g. #287 γ-2 fleet work, #123 design-revision tweaks) conflicts at the bundle literal. Our change is a *follow-up `.insert()`* outside the bundle, minimizing conflict surface.
5. **`src/deep_space/mod.rs:490-532`** — #223 deliverable ops recently active. Our insertion into the spawn bundle touches L505-517.
6. **`src/persistence/save.rs` and `load.rs`** — NO changes required from this PR (see §2F). This removes what would otherwise be a high-conflict zone against #247 (save/load phase B/C).
7. **`src/faction/mod.rs`** — adding `entity_owner` helper at end of file. Very low conflict risk.

### Mitigation
- Rebase order if multiple merge candidates land: #293 follow-ups (if any) first, then this PR. S-1 (#295) is already merged so `system_owner` is stable.
- If #287 γ-2+ lands first (touches `spawn_ship` bundle), Commit 3 rebases cleanly since our insertion is a *separate statement* after the bundle.

## 5. Regression test matrix

| Test | Asserts | File |
|---|---|---|
| `spawn_capital_colony_attaches_faction_owner` | After Startup, unique Colony carries `FactionOwner(player_empire)` | new `tests/faction_owner_unification.rs` |
| `spawn_capital_colony_starsystem_faction_owner` | Capital StarSystem entity carries `FactionOwner` | same |
| `colonization_queue_inherits_source_owner` | Colony built via `tick_colonization_queue` from source owned by empire A carries `FactionOwner(A)` | same |
| `process_settling_attaches_faction_owner` | Colony-ship landing creates Colony with `FactionOwner` matching ship's `Owner::Empire` | same |
| `spawn_ship_empire_gets_faction_owner` | `spawn_ship(..., Owner::Empire(e), ...)` → ship has `FactionOwner(e)` | same |
| `spawn_ship_neutral_has_no_faction_owner` | `spawn_ship(..., Owner::Neutral, ...)` → no component | same |
| `spawn_deliverable_entity_empire_gets_faction_owner` | DeepSpaceStructure with `Owner::Empire(e)` has `FactionOwner(e)` | same |
| `entity_owner_resolves_all_classes` | Helper returns faction entity for ship / colony / structure / starsystem / hostile; `None` for neutral ship | same |
| `save_load_round_trips_colony_faction_owner` | Colony spawned with FactionOwner → save → load → still has FactionOwner pointing at remapped empire | `tests/save_load.rs` (extension) |
| `save_load_round_trips_system_buildings_faction_owner` | Same for StarSystem carrying both SystemBuildings and FactionOwner | same |
| `save_load_round_trips_deep_space_structure_faction_owner` | Same for DeepSpaceStructure | same |

Existing test suite should continue to pass. Most likely failure mode: a test asserting *absence* of `FactionOwner` on a colony entity (grep `(With<Colony>, Without<FactionOwner>)` returned zero hits — safe).

## 6. Out of scope

- **Ship.owner enum removal** (S-11 candidate). Kept dual-write with `FactionOwner`.
- **Sovereignty derive** (#295 S-1, merged). `Sovereignty.owner` still flows through `system_owner()` + Core ship.
- **Owner-change cascade** (S-10, explicit non-goal).
- **Multi-faction colonization UX** — `spawn_colony_on_planet` fallback-to-PlayerEmpire remains. Proper per-NPC flows come with NPC automation (#173 follow-ups).
- **`resolve_ship_faction` refactor** (`src/ship/pursuit.rs:153`). Unchanged.
- **Core ship spawn** (#296 S-3). `FactionOwner` on Core ships is handled by the generic `spawn_ship` change in Commit 3.
- **Lua exposure of `entity_owner`** — Rust-only for this PR.

## 7. Risk summary

| Risk | Likelihood | Mitigation |
|---|---|---|
| Startup ordering: `spawn_capital_colony` before `PlayerEmpire` exists | Low | Chain verified (`spawn_player_empire` → `spawn_hostile_factions` → `generate_galaxy` → `spawn_capital_colony`). Defensive warn-skip. |
| `tick_colonization_queue` source_colony missing `FactionOwner` | Medium during migration | Warn-skip. Backfill optional; not needed since all capital colonies get one in Commit 2. |
| Savebag forward/backward compat | Low | Bag field pre-existed; version stays 1. Existing `round_trip_save_load` tests cover absent-field case. |
| `B0001` query conflicts from new params | Low | New queries are read-only; no mutable overlap. `all_systems_no_query_conflict` catches regressions. |
| `process_settling` — `FactionOwner` vs `ship.owner` disagreement | Very low | Precedence: prefer component. In practice ships spawned via `spawn_ship` after Commit 3 have both, agreeing. |
| Lua scripts construct ships via `ctx.system:spawn_ship` | Low | The path flows through Rust `spawn_ship` (`scripting/game_start_ctx.rs:344-345` → `setup/mod.rs:648`). Lua side unchanged. |

## Critical Files for Implementation
- `macrocosmo/src/faction/mod.rs`
- `macrocosmo/src/colony/colonization.rs`
- `macrocosmo/src/ship/settlement.rs`
- `macrocosmo/src/ship/mod.rs`
- `macrocosmo/src/deep_space/mod.rs`
- `macrocosmo/src/setup/mod.rs`
