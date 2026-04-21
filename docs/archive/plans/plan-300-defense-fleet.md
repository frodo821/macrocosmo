# Implementation Plan: Issue #300 — S-6 Defense Fleet

_Depends on #296 (S-3 CoreShip deployed), #287 (Fleet gamma-1 merged). #299 (S-5 Core auto-spawn) should land first so that Core ships exist at game start._

---

## Overview

When a Core ship is deployed (or spawned at game start), a Defense Fleet is automatically created in that system. The Core ship is reassigned from its auto-created single-ship Fleet to the Defense Fleet. Future ships can join the Defense Fleet (hook for #220). Defense Fleet ships cannot move (tied to the system).

## 1. New Component: `DefenseFleet`

**File:** `macrocosmo/src/ship/defense_fleet.rs` (new module)

- `DefenseFleet { pub system: Entity }` — marker on a Fleet entity, binding it to a specific star system
- Re-export from `ship/mod.rs`
- Defense Fleet entities also carry `Fleet` + `FleetMembers` (standard fleet components from `ship/fleet.rs`)

## 2. Core Deploy Creates Defense Fleet

**File:** `macrocosmo/src/ship/core_deliverable.rs` — after Core spawn in `handle_core_deploy_requested`

- After `spawn_core_ship_from_deliverable` returns the Core ship entity:
  1. The Core ship was auto-assigned a single-ship Fleet by `spawn_ship` (gamma-1 behavior)
  2. Create a new Defense Fleet entity: `commands.spawn((Fleet { name: format!("{} Defense Fleet", system_name), flagship: Some(core_entity) }, FleetMembers(vec![core_entity]), DefenseFleet { system: target_system }))`
  3. Remove Core from its auto-created fleet: `remove_ship_from_fleet(core_entity, old_fleet, &mut fleet_members)` from `fleet.rs`
  4. Assign Core to Defense Fleet: `core_ship.fleet = Some(defense_fleet_entity)`

- Same logic for `spawn_core_ship_from_deliverable` called from `apply_game_start_actions` (#299) — extract into a shared helper `create_defense_fleet_for_core`

## 3. Auto-Created Fleet Pruning

- The old single-ship fleet (now empty after Core removal) is automatically cleaned up by `prune_empty_fleets` system (existing, `ship/fleet.rs`)
- No new code needed for cleanup

## 4. Helper: `join_defense_fleet`

**File:** `macrocosmo/src/ship/defense_fleet.rs`

- `pub fn join_defense_fleet(commands: &mut Commands, ship: Entity, defense_fleet: Entity, fleet_members: &mut FleetMembers, ship_fleet: &mut Option<Entity>)`
- Removes ship from current fleet, adds to Defense Fleet's FleetMembers, updates ship's fleet reference
- Exported for future use by #220 (garrison assignment)
- Ships in a Defense Fleet inherit immobility: the `DefenseFleet.system` binding means move commands are conceptually invalid

## 5. Move Command Rejection

- Core ships are already immobile via `Ship::is_immobile()` (#296)
- Non-Core ships in Defense Fleets: move commands should be rejected
- Gate in `handle_move_requested` (or equivalent command dispatcher): if ship's fleet has `DefenseFleet` component, reject MoveTo
- Alternative: check at UI level only (context_menu), since Defense Fleet membership implies station-keeping. Decision: gate at handler level for safety, UI as convenience.

## 6. Persistence

**File:** `macrocosmo/src/persistence/savebag.rs`

- Add `SavedDefenseFleet { system: SavedEntityRef }` to `SavedComponentBag`
- Field: `defense_fleet: Option<SavedDefenseFleet>`

**File:** `macrocosmo/src/persistence/save.rs`

- Serialize `DefenseFleet` component on fleet entities

**File:** `macrocosmo/src/persistence/load.rs`

- Restore `DefenseFleet` component, remap `system` entity ref

Note: SAVE_VERSION bump may or may not be needed depending on whether #298 already bumped it. If #298 lands first (version 4), this adds `defense_fleet` as an optional field — no further bump required since it's `Option`. If landing independently, bump to 4.

## Commit Sequence (3 commits)

1. **`[300] add DefenseFleet component + create_defense_fleet_for_core helper`** — new module, component definition, helper that creates Defense Fleet and reassigns Core
2. **`[300] wire Defense Fleet creation into Core deploy + game start`** — integrate into handle_core_deploy_requested and apply_game_start_actions Core spawn path, move command rejection
3. **`[300] persistence + tests`** — SavedDefenseFleet, save/load, all integration tests

## Test Plan (8 integration tests)

**File:** `macrocosmo/tests/defense_fleet.rs` (new)

1. `test_core_deploy_creates_defense_fleet` — after Core deploy, a Fleet entity with DefenseFleet component exists at the system
2. `test_core_in_defense_fleet` — Core ship's `fleet` field points to the Defense Fleet entity
3. `test_old_single_ship_fleet_pruned` — the auto-created single-ship fleet is despawned by prune_empty_fleets
4. `test_defense_fleet_system_binding` — DefenseFleet.system matches the Core's AtSystem
5. `test_game_start_core_gets_defense_fleet` — after apply_game_start_actions with spawn_core, Defense Fleet exists
6. `test_join_defense_fleet_helper` — ship added via helper, correctly in FleetMembers, old fleet updated
7. `test_move_rejected_for_defense_fleet_member` — non-Core ship in Defense Fleet cannot execute MoveTo
8. `test_defense_fleet_savebag_round_trip` — save/load preserves DefenseFleet component and system binding

## Pitfall List

- **Fleet creation timing:** `spawn_ship` auto-creates a single-ship Fleet. The Defense Fleet creation must happen after spawn_ship returns, in the same system or a subsequent one. Since `handle_core_deploy_requested` runs after spawn, timing is correct. But `Commands` are deferred — ensure the old fleet removal and new fleet creation don't conflict within the same tick.
- **prune_empty_fleets ordering:** Must run after Defense Fleet creation to clean up the orphaned fleet. Already runs each tick — ordering should be fine since creation happens in `handle_core_deploy_requested` which runs earlier in the system chain.
- **Entity validity:** When removing Core from old fleet, the fleet entity must still exist. Since this happens in the same system (or same tick), the entity is valid.
- **Multiple Cores per system:** Currently prevented by #296 validation (one Core per system). Defense Fleet is 1:1 with Core. If future design allows multiple, this needs revisiting.
- **Save ordering:** DefenseFleet references a StarSystem entity. Entity remap in load.rs must handle this — same pattern as other entity refs (AtSystem, etc.).

## Files to Modify

- `macrocosmo/src/ship/defense_fleet.rs` (new)
- `macrocosmo/src/ship/mod.rs` (re-export, plugin wiring)
- `macrocosmo/src/ship/core_deliverable.rs` (wire Defense Fleet creation after Core spawn)
- `macrocosmo/src/setup/mod.rs` (wire Defense Fleet in game-start Core spawn path)
- `macrocosmo/src/ship/handlers/` (move rejection for Defense Fleet members)
- `macrocosmo/src/persistence/savebag.rs`
- `macrocosmo/src/persistence/save.rs`
- `macrocosmo/src/persistence/load.rs`
- `macrocosmo/tests/defense_fleet.rs` (new)
