# Implementation Plan: Issue #299 — S-5 Core Auto-Spawn + Settle Gate

_Depends on #296 (S-3 CoreShip + spawn_core_ship_from_deliverable). #297 (FactionOwner) assumed merged._

---

## Overview

Two mechanics: (1) Lua `on_game_start` scripts can request a Core ship be auto-spawned in the faction's capital system; (2) colonization (settle) is gated on the target system having a Core owned by the settling faction.

## 1. GameStartActions: `spawn_core` Flag

**File:** `macrocosmo/src/scripting/game_start_ctx.rs`

- Add field to `GameStartActions`: `pub spawn_core: bool` (default: false)
- Add `ctx.system:spawn_core()` Lua method on `GameStartCtx` UserData impl that sets `spawn_core = true`
- No design_id parameter — uses the canonical `infrastructure_core_v1` design

## 2. apply_game_start_actions: Core Spawn

**File:** `macrocosmo/src/setup/mod.rs` — `apply_game_start_actions`

- After existing ship spawns (ships loop), check `actions.spawn_core`
- If true: call `spawn_core_ship_from_deliverable(commands, capital_system_entity, faction_entity, ...)` from `ship::core_deliverable`
- This reuses the existing spawn helper from #296 — no new spawn logic needed
- The Core spawns at `system_inner_orbit_position` like a deployed Core

## 3. Lua Script Updates

**File:** `scripts/factions/init.lua` (or per-faction files)

- Add `ctx.system:spawn_core()` call in the `on_game_start` callback for both player and NPC factions
- This replaces any manual Core deliverable deploy sequence at game start

**File:** `scripts/lib/capital.lua` (if shared capital setup helper exists)

- Include `spawn_core()` in the standard capital setup pattern

## 4. Settle Gate: handle_colonize_requested

**File:** `macrocosmo/src/ship/handlers/settlement_handler.rs` — `handle_colonize_requested`

- Add query param: `cores: Query<(&AtSystem, &FactionOwner), With<CoreShip>>`
- Before entering the settle state, check: does a CoreShip owned by the ship's faction exist in the target system?
- Resolution: `cores.iter().any(|(&at, &fo)| at.0 == req.target_system && fo.0 == ship_faction)`
- If no Core present: reject with `CommandResult::Rejected { reason: "no sovereignty core in target system" }`
- Ship's faction resolved via `FactionOwner` component on the ship (post-#297) or `ship.owner`

## 5. Safety Net: process_settling

**File:** `macrocosmo/src/ship/settlement.rs` — `process_settling`

- Add same Core-presence check as a safety net
- If Core disappears mid-settle (e.g. destroyed during settling period): abort settling, return ship to Docked state
- Log warning

## 6. UI Grey-Out

**File:** `macrocosmo/src/ui/context_menu.rs`

- In the "Colonize" action button logic: check Core presence in target system
- If no Core: grey out button, add tooltip "Requires sovereignty core in target system"
- Query `CoreShip + AtSystem + FactionOwner` available through existing UI params or added to context menu system

## Commit Sequence (4 commits)

1. **`[299] add spawn_core to GameStartActions + Lua method`** — GameStartActions field, GameStartCtx UserData method, apply_game_start_actions handler
2. **`[299] update faction Lua scripts to spawn core on game start`** — scripts/factions/init.lua, scripts/lib/capital.lua
3. **`[299] settle gate: require Core in target system`** — handle_colonize_requested check, process_settling safety net
4. **`[299] UI grey-out + tests`** — context_menu.rs colonize button, all integration tests

## Test Plan (6 integration tests)

**File:** `macrocosmo/tests/core_auto_spawn.rs` (new)

1. `test_game_start_spawns_core_in_capital` — after apply_game_start_actions with spawn_core=true, CoreShip entity exists at capital system
2. `test_game_start_no_core_when_flag_false` — spawn_core=false (default), no CoreShip spawned
3. `test_colonize_rejected_without_core` — ship attempts colonize in system without Core, gets Rejected
4. `test_colonize_accepted_with_core` — system has faction's Core, colonize proceeds normally
5. `test_settle_aborted_when_core_destroyed` — Core removed mid-settle, ship returns to Docked
6. `test_colonize_rejected_with_enemy_core` — system has Core but owned by different faction, colonize rejected

## Pitfall List

- **Startup ordering:** `spawn_core_ship_from_deliverable` in `apply_game_start_actions` runs with `&mut World`. Must ensure ship design registry is loaded before this runs (it already is — Lua scripts load first).
- **FactionOwner on ship:** The settle gate must resolve the ship's faction. Post-#297, ships have `FactionOwner` component. Use that, with `ship.owner` fallback.
- **NPC factions:** `run_all_factions_on_game_start` calls `apply_game_start_actions` per faction. Each NPC faction's `on_game_start` Lua script should also call `spawn_core()` — verify this in the Lua script update.
- **Query conflict:** New `cores` query in `handle_colonize_requested` is read-only `With<CoreShip>`. No overlap with mutable ship queries.
- **test_app compatibility:** Tests that call `apply_game_start_actions` must have `ShipDesignRegistry` populated (at minimum the `infrastructure_core_v1` design). May need test helper setup.

## Files to Modify

- `macrocosmo/src/scripting/game_start_ctx.rs` (GameStartActions + UserData)
- `macrocosmo/src/setup/mod.rs` (apply_game_start_actions)
- `macrocosmo/src/ship/handlers/settlement_handler.rs` (settle gate)
- `macrocosmo/src/ship/settlement.rs` (safety net)
- `macrocosmo/src/ui/context_menu.rs` (UI grey-out)
- `scripts/factions/init.lua`
- `scripts/lib/capital.lua`
- `macrocosmo/tests/core_auto_spawn.rs` (new)
