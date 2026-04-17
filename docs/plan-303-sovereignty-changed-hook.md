# Implementation Plan: Issue #303 — S-10 Sovereignty Changed Hook

_Depends on #295 (S-1 Sovereignty derived view, merged), #297 (FactionOwner, merged). Relates to #305 (Diplomacy). Independent of #298/#299/#300._

---

## Overview

When `update_sovereignty` detects that a system's owner has changed, fire a `macrocosmo:sovereignty_changed` event through the EventBus. Additionally, cascade ownership updates to child entities (Colony, SystemBuildings, docked Ships, DeepSpaceStructure). Lua scripts can subscribe via `on("macrocosmo:sovereignty_changed", fn)`.

## 1. SovereigntyChangeReason Enum

**File:** `macrocosmo/src/colony/authority.rs`

```
pub enum SovereigntyChangeReason {
    Conquest,      // Enemy Core deployed / conquered existing Core
    Cession,       // Diplomatic transfer (future #305)
    Abandonment,   // Core withdrawn / destroyed (owner -> None)
    Secession,     // Rebel faction takes over (future)
    Initial,       // Game start / first Core deployment in unclaimed system
}
```

- Derives: `Debug, Clone, Copy, PartialEq, Eq`
- Implements `Display` for Lua string conversion

## 2. SovereigntyChangedContext

**File:** `macrocosmo/src/colony/authority.rs`

- Struct implementing `EventContext` trait (from `event_system.rs`):
  - `system: Entity`
  - `system_name: String`
  - `previous_owner: Option<Entity>` (faction entity)
  - `new_owner: Option<Entity>` (faction entity)
  - `reason: SovereigntyChangeReason`
- `event_id()` returns `"macrocosmo:sovereignty_changed"`
- `to_lua_table()` populates: `system_id`, `system_name`, `previous_owner_id`, `new_owner_id`, `reason` (string)

## 3. Modify `update_sovereignty`

**File:** `macrocosmo/src/colony/authority.rs` — `update_sovereignty` (L132-148)

Current system detects owner from CoreShip queries and writes `Sovereignty.owner`. Extend to:

- Track previous owner before update: `let prev = sov.owner`
- After update: compare `prev` vs new `sov.owner`
- If changed: determine reason:
  - `None -> Some(_)`: `Initial` (or `Conquest` if system previously had entities)
  - `Some(a) -> Some(b)` where `a != b`: `Conquest`
  - `Some(_) -> None`: `Abandonment`
- Write `SovereigntyChangedContext` to an event bus writer (add `EventBusWriter` or `MessageWriter<SovereigntyChangedEvent>` param)
- Need system name for context: add `Query<&StarSystem>` to system params

## 4. Cascade System: `cascade_sovereignty_changes`

**File:** `macrocosmo/src/colony/authority.rs` (or new `macrocosmo/src/colony/sovereignty_cascade.rs`)

- **Exclusive system** (takes `&mut World`) running `.after(update_sovereignty)`
- Reads pending sovereignty changes (from a `Resource<PendingSovereigntyChanges>` populated by `update_sovereignty`, or by consuming events)
- For each changed system:
  1. **Colony:** Query `(Entity, &Colony)` where colony's planet is in the system. Update `FactionOwner` on Colony entity to match new sovereign.
  2. **SystemBuildings:** The StarSystem entity itself carries `FactionOwner` (per #297). Update to new sovereign.
  3. **Docked Ships:** Query `(&ShipState, Entity)` where `ShipState::Docked { system }` matches. Update `FactionOwner` + `Ship.owner` to new sovereign. (Design note: only docked ships transfer — in-transit or loitering ships retain original owner.)
  4. **DeepSpaceStructure:** Query structures `With<AtSystem>` matching the system. Update `FactionOwner`.
- **Abandonment special case:** When new_owner is `None`, leave `FactionOwner` as-is on child entities. The entities remain owned by the previous faction — they just lose sovereignty protection. This is a deliberate design decision: abandoning a system does not magically transfer infrastructure to nobody.

## 5. Lua Event Dispatch

- `SovereigntyChangedContext` implements `EventContext`, so it flows through the existing `EventBus::fire` pipeline
- Lua scripts register: `on("macrocosmo:sovereignty_changed", function(evt) ... end)`
- `evt.gamestate` snapshot is available (per #332 gamestate scoped closures)
- Event payload fields: `evt.system_id`, `evt.system_name`, `evt.previous_owner_id`, `evt.new_owner_id`, `evt.reason`

## 6. Integration with EventBus

**File:** `macrocosmo/src/colony/authority.rs` or plugin wiring

- Option A: `update_sovereignty` directly calls `EventBus::fire` (requires `Res<ScriptEngine>` or `NonSendMut<Lua>`)
- Option B (preferred): `update_sovereignty` writes to a `MessageWriter<SovereigntyChangedEvent>`, a downstream system reads and fires via EventBus. This keeps `update_sovereignty` lightweight and avoids Lua in a system that queries many entities.
- The downstream "fire" system runs after `cascade_sovereignty_changes` so that Lua handlers see the post-cascade world state.

---

## Commit Sequence (4 commits)

1. **`[303] add SovereigntyChangeReason + SovereigntyChangedContext`** — enum, EventContext impl, to_lua_table
2. **`[303] detect owner change in update_sovereignty`** — track previous owner, emit change message/resource when ownership transitions
3. **`[303] cascade_sovereignty_changes exclusive system`** — update FactionOwner on Colony/SystemBuildings/Ships/DeepSpaceStructure, abandonment special case
4. **`[303] EventBus dispatch + Lua integration + tests`** — fire event through EventBus, register in plugin, all integration tests

## Test Plan (12 integration tests)

**File:** `macrocosmo/tests/sovereignty_changed.rs` (new)

1. `test_initial_sovereignty_fires_event` — Core deployed in unclaimed system, event fires with reason=Initial
2. `test_conquest_sovereignty_fires_event` — system changes from faction A to B, event fires with reason=Conquest
3. `test_abandonment_sovereignty_fires_event` — Core removed, event fires with reason=Abandonment
4. `test_no_event_when_owner_unchanged` — same owner across ticks, no event
5. `test_cascade_colony_faction_owner` — sovereignty change updates Colony's FactionOwner
6. `test_cascade_system_buildings_faction_owner` — StarSystem entity's FactionOwner updated
7. `test_cascade_docked_ships_faction_owner` — docked ships get new FactionOwner
8. `test_cascade_skips_in_transit_ships` — ships in SubLight/FTL state at the system retain original owner
9. `test_cascade_deep_space_structure` — DeepSpaceStructure FactionOwner updated
10. `test_abandonment_preserves_faction_owner` — when new_owner=None, child entities keep previous FactionOwner
11. `test_lua_handler_receives_sovereignty_event` — Lua `on("macrocosmo:sovereignty_changed", ...)` callback fires with correct payload fields
12. `test_sovereignty_changed_reason_display` — reason enum Display trait outputs expected strings

## Pitfall List

- **Exclusive system world access:** `cascade_sovereignty_changes` as exclusive system gets `&mut World`, which blocks all other systems that tick. Keep it lean — only iterate entities in changed systems.
- **Query conflict with update_sovereignty:** `cascade_sovereignty_changes` must not run in parallel with `update_sovereignty`. Chain ordering: `update_sovereignty -> cascade_sovereignty_changes -> fire_sovereignty_events`.
- **Docked ships filter:** Only `ShipState::Docked { system }` ships transfer. Ships in `Loitering`, `SubLight`, `FTL` at the same system coordinates do NOT transfer. This avoids capturing passing-through ships.
- **Ship.owner dual-write:** When cascading, update both `FactionOwner` component AND `Ship.owner` field (Owner::Empire(new_faction)). The dual-write exists until the Owner enum is removed (S-11).
- **Abandonment design:** Leaving FactionOwner intact when sovereignty is abandoned is deliberate. The alternative (clearing FactionOwner) would break combat/ROE for orphaned entities. Document this in code comments.
- **EventBus Lua access:** If using Option B (message-based), the fire system needs `NonSendMut<Lua>` or `Res<ScriptEngine>`. Ensure it's registered in the correct schedule and not conflicting with other Lua-accessing systems.
- **PendingSovereigntyChanges cleanup:** If using a resource to queue changes, drain it each tick to avoid stale events.

## Files to Modify

- `macrocosmo/src/colony/authority.rs` (SovereigntyChangeReason, SovereigntyChangedContext, update_sovereignty changes, cascade system)
- `macrocosmo/src/colony/mod.rs` (plugin registration for new systems)
- `macrocosmo/src/event_system.rs` (only if new event registration is needed beyond EventContext)
- `macrocosmo/src/ship/mod.rs` (if cascade needs Ship-specific re-exports)
- `macrocosmo/tests/sovereignty_changed.rs` (new)
