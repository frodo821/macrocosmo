# Implementation Plan: Issue #298 — S-4 Conquered State Mechanic

_Depends on #296 (S-3 CoreShip marker, deployed). #297 (FactionOwner unification) assumed merged._

---

## Overview

When an Infrastructure Core's hull HP reaches 1.0, it enters a "Conquered" state instead of being destroyed. The conquering faction gains sovereignty while the original owner retains the shell. Recovery happens automatically when the attacker departs and the system is at peace.

## 1. New Component: `ConqueredCore`

**File:** `macrocosmo/src/ship/conquered.rs` (new module)

- `ConqueredCore { conquering_faction: Entity, conquered_at: i64 }`
- Marker indicating the Core is in conquered state
- Re-export from `ship/mod.rs`

## 2. Combat HP Clamp for CoreShip

**File:** `macrocosmo/src/ship/combat.rs` — `resolve_combat`

- Add `Option<&CoreShip>` to the ships query tuple (L104)
- In `apply_damage_to_ship` callsite: when `core_ship.is_some()`, clamp `hp.hull = hp.hull.max(1.0)` after damage application
- CoreShip hull never reaches 0.0 — it cannot be destroyed through combat

## 3. System: `check_conquered_transition`

**File:** `macrocosmo/src/ship/conquered.rs`

- Runs `.after(resolve_combat)`
- Query: `(Entity, &Ship, &ShipHitpoints, &FactionOwner), (With<CoreShip>, Without<ConqueredCore>)`
- When `hp.hull <= 1.0`: identify the attacking faction from hostile entities at the same system (query `AtSystem` + `FactionOwner` + `With<Hostile>` or enemy ships)
- `commands.entity(core).insert(ConqueredCore { conquering_faction, conquered_at: clock.elapsed })`
- Emit a game event for UI notification

## 4. System: `tick_conquered_recovery`

**File:** `macrocosmo/src/ship/conquered.rs`

- Runs `.after(resolve_combat)`
- Query: `(Entity, &mut ShipHitpoints, &ConqueredCore, &AtSystem), With<CoreShip>`
- Recovery conditions: (a) no hostile entities from `conquering_faction` present in same system, (b) no active combat this tick
- When conditions met: `hp.hull += balance.core_recovery_rate_per_hexadies() * delta`
- When `hp.hull >= threshold` (e.g. hull_max * 0.5 or a configurable value): remove `ConqueredCore` component

## 5. System: `enforce_conquered_hp_lock`

**File:** `macrocosmo/src/ship/conquered.rs`

- Safety clamp running after `tick_ship_repair`
- Query: `(&mut ShipHitpoints), (With<CoreShip>, With<ConqueredCore>)`
- Prevents other repair systems from healing a conquered Core above recovery threshold
- Also: add `Without<ConqueredCore>` filter to `tick_ship_repair` in `hitpoints.rs` (L56) so port repair skips conquered Cores

## 6. Casus Belli Hook

- Attacking a CoreShip during peacetime (no existing war/hostile status between factions) emits `macrocosmo:casus_belli` event via EventBus
- Event payload: `{ attacker_faction, defender_faction, system, reason = "core_attack" }`
- Hook point for future #305 (Diplomacy: Declare War). No consumer yet — event is fire-and-forget.

## 7. Lua Balance Parameter

**File:** `macrocosmo/src/technology/mod.rs` — `GameBalance`

- Add field: `core_recovery_rate_per_hexadies: f64` (default: 2.0)
- Add accessor: `pub fn core_recovery_rate_per_hexadies(&self) -> f64`
- Parse from Lua balance definition in `load_game_balance` system

**File:** `scripts/balance.lua` (or equivalent balance definition)

- Add: `core_recovery_rate_per_hexadies = 2.0`

## 8. Persistence

**File:** `macrocosmo/src/persistence/savebag.rs`

- Add `SavedConqueredCore { conquering_faction: SavedEntityRef, conquered_at: i64 }` to `SavedComponentBag`
- Field: `conquered_core: Option<SavedConqueredCore>`

**File:** `macrocosmo/src/persistence/save.rs`

- Serialize `ConqueredCore` from ECS
- Bump `SAVE_VERSION` from 3 to 4

**File:** `macrocosmo/src/persistence/load.rs`

- Restore `ConqueredCore` component, remap `conquering_faction` entity

## 9. Plugin Wiring

**File:** `macrocosmo/src/ship/mod.rs`

- Register systems in order: `check_conquered_transition.after(resolve_combat)`, `tick_conquered_recovery.after(check_conquered_transition)`, `enforce_conquered_hp_lock.after(tick_ship_repair)`

---

## Commit Sequence (5 commits)

1. **`[298] add ConqueredCore component + GameBalance field`** — new module, GameBalance extension, Lua balance param
2. **`[298] combat HP clamp for CoreShip`** — modify resolve_combat ships query, add hull clamp
3. **`[298] check_conquered_transition + tick_conquered_recovery systems`** — transition logic, recovery logic, plugin wiring
4. **`[298] enforce_conquered_hp_lock + repair exclusion`** — safety clamp system, `Without<ConqueredCore>` on tick_ship_repair
5. **`[298] persistence + casus belli event + tests`** — SavedConqueredCore, SAVE_VERSION bump, event emission, all integration tests

## Test Plan (9 integration tests)

**File:** `macrocosmo/tests/conquered_core.rs` (new)

1. `test_core_hull_clamped_at_one` — CoreShip in combat, hull never goes below 1.0
2. `test_conquered_transition_on_hull_one` — ConqueredCore attached when hull reaches 1.0
3. `test_conquered_recovery_when_attacker_absent` — hull recovers at configured rate, ConqueredCore removed at threshold
4. `test_conquered_no_recovery_while_attacker_present` — no recovery when conquering faction ships still in system
5. `test_conquered_core_skipped_by_port_repair` — tick_ship_repair does not heal conquered Core
6. `test_enforce_hp_lock_clamp` — safety clamp prevents over-healing
7. `test_casus_belli_event_on_peacetime_attack` — event fired when Core attacked without prior hostility
8. `test_conquered_core_savebag_round_trip` — save/load preserves ConqueredCore state
9. `test_conquered_sovereignty_transfer` — while conquered, system_owner returns conquering faction

## Pitfall List

- **Query conflict B0001:** Adding `Option<&CoreShip>` to resolve_combat's ships query must not overlap with any `&mut CoreShip` query in the same system. CoreShip is read-only here — safe.
- **tick_ship_repair filter:** `Without<ConqueredCore>` must be added to the existing query, not a new query, to avoid B0001.
- **Recovery race:** `tick_conquered_recovery` must run after `resolve_combat` to avoid recovering HP that gets immediately clamped back.
- **Entity remap in save/load:** `conquering_faction` is a faction Entity — must go through `entity_remap` in load.rs like all other entity references.
- **GameBalance parse ordering:** `core_recovery_rate_per_hexadies` must have a sane default so tests without Lua scripts still work.

## Files to Modify

- `macrocosmo/src/ship/conquered.rs` (new)
- `macrocosmo/src/ship/mod.rs` (re-export, plugin wiring)
- `macrocosmo/src/ship/combat.rs` (HP clamp)
- `macrocosmo/src/ship/hitpoints.rs` (Without filter)
- `macrocosmo/src/technology/mod.rs` (GameBalance field)
- `macrocosmo/src/persistence/savebag.rs`
- `macrocosmo/src/persistence/save.rs`
- `macrocosmo/src/persistence/load.rs`
- `scripts/balance.lua` or equivalent
- `macrocosmo/tests/conquered_core.rs` (new)
