# Implementation Plan: Issue #384 — Harbour Dock Modifier System

_Prepared 2026-04-16. Adds harbour capacity as a modifier-driven stat, docked-ship modifier scoping (`docked_to:` prefix), and full harbour lifecycle systems (dock/undock, combat ROE, position sync)._

---

## Commit Sequence (7 commits)

### Commit 1: ShipStats + ShipModifiers harbour_capacity

**Scope.** Add `harbour_capacity: CachedValue` to `ShipStats` and corresponding `ScopedModifiers` entry in `ShipModifiers`. Wire `push_ship_modifier` to route `ship.harbour_capacity` target to the new field. A ship is harbourable when `harbour_capacity > 0` (no separate flag).

**Files.**
- `macrocosmo/src/ship/mod.rs` — `ShipStats`, `ShipModifiers` structs
- `macrocosmo/src/modifier.rs` — routing in `push_ship_modifier`

---

### Commit 2: ParsedModifier docked_scope + HarbourModifiers + sync extraction

**Scope.** Extend `ParsedModifier` with a `docked_scope()` parser that recognises the `docked_to:` prefix in modifier target strings. Introduce `HarbourModifiers` component to hold modifiers that a harbour propagates to its docked ships. Extract docked-target modifiers during `sync_ship_module_modifiers` and store them on the harbour entity.

**Modifier syntax.**
- `docked_to:self::ship.repair_rate` — harbour module modifier applied to ships docked in this harbour
- `docked_to:<hull_id>::ship.shield_regen` — tech modifier scoped to a specific harbour hull type
- `docked_to:*::ship.speed` — empire-wide modifier for all docked ships

**Files.**
- `macrocosmo/src/modifier.rs` — `ParsedModifier::docked_scope()`, `DockedScope` enum (`SelfHarbour`, `HullId(String)`, `Any`)
- `macrocosmo/src/ship/mod.rs` — `HarbourModifiers` component
- `macrocosmo/src/ship/harbour.rs` (new) — extraction in `sync_ship_module_modifiers`

---

### Commit 3: Harbour core — can_dock, dock, undock, capacity check

**Scope.** Core harbour operations in `ship/harbour.rs`. `can_dock(docker, harbour)` checks: harbour has capacity remaining, docker size fits, docker is in same system, docker is not already docked, and nesting soft-forbid (station `size=10000` >> any realistic capacity). `dock(docker, harbour)` inserts `DockedAt(harbour)` component on docker. `undock(docker)` removes it.

**Files.**
- `macrocosmo/src/ship/harbour.rs` — `can_dock`, `dock`, `undock`, capacity query helpers

---

### Commit 4: Harbour systems — position sync, force undock, auto undock on move

**Scope.** Three systems:
1. `sync_docked_position` — docked ship derives position from harbour entity position
2. `force_undock_on_harbour_destroy` — if harbour entity is despawned/destroyed, all docked ships are forcibly undocked
3. `auto_undock_on_move_command` — when a docked ship receives a `MoveTo` command, automatically undock first

**Files.**
- `macrocosmo/src/ship/harbour.rs` — three new systems
- `macrocosmo/src/ship/mod.rs` — system registration in `ShipPlugin`

---

### Commit 5: ROE combat — auto undock, UndockedForCombat, auto return, skip docked in resolve

**Scope.** Combat-ROE harbour interactions:
- `auto_undock_on_combat_roe` — when hostiles appear in system, ships with Aggressive/Defensive ROE auto-undock. Evasive/Passive ROE ships stay docked (harbour shields them).
- `UndockedForCombat` marker component — tracks which ships were undocked for combat and which harbour they came from.
- `auto_return_dock_after_combat` — after combat resolves and no hostiles remain, ships with `UndockedForCombat` marker attempt to re-dock at their original harbour.
- `resolve_combat` skip — ships with `DockedAt` component are excluded from combat resolution (they are shielded by the harbour).

**Files.**
- `macrocosmo/src/ship/harbour.rs` — `auto_undock_on_combat_roe`, `auto_return_dock_after_combat`
- `macrocosmo/src/ship/mod.rs` — `UndockedForCombat` component
- `macrocosmo/src/ship/combat.rs` — skip `DockedAt` in `resolve_combat`

---

### Commit 6: sync_docked_modifiers — apply harbour modifiers to docked ships

**Scope.** `sync_docked_modifiers` system reads `HarbourModifiers` from harbour entities and applies them to all ships with `DockedAt` pointing at that harbour. On undock (detected via `RemovedComponents<DockedAt>`), clean up the applied modifiers from the formerly-docked ship.

**Files.**
- `macrocosmo/src/ship/harbour.rs` — `sync_docked_modifiers` system
- `macrocosmo/src/ship/mod.rs` — system registration

---

### Commit 7: Integration tests + ROE label updates

**Scope.** Integration tests covering the full harbour lifecycle. Update ROE-related UI labels if needed to reflect docked behavior.

**Files.**
- `macrocosmo/tests/harbour.rs` (new) — integration test file
- `macrocosmo/src/ui/ship_panel.rs` — ROE tooltip updates (if applicable)

---

## System Ordering

```
sync_ship_module_modifiers
    → sync_docked_modifiers
    → auto_undock_on_move_command
    → movement systems
    → sync_docked_position
    → auto_undock_on_combat_roe
    → resolve_combat
    → force_undock_on_harbour_destroy
    → auto_return_dock_after_combat
```

All systems run `.after(advance_game_time)` per project convention.

---

## Design Decisions

### DD-1: Harbour detection — capacity-based, no flag
`harbour_capacity > 0` means the ship is a harbour. No separate `is_harbour` flag or capability check needed. This aligns with the computed-stats pattern (hull + modules determine capacity).

### DD-2: ROE-based combat undock
- **Aggressive / Defensive ROE** — auto-undock when hostiles appear, join combat.
- **Evasive / Passive ROE** — stay docked, harbour shields them (skip in `resolve_combat`).

### DD-3: Auto-return dock after combat
Ships undocked for combat (`UndockedForCombat` marker) automatically attempt to re-dock at their original harbour once hostiles are cleared from the system.

### DD-4: Nesting soft-forbid
No hard engine rule. Station hulls have `size=10000` which exceeds any realistic harbour capacity, making nesting effectively impossible without explicit Lua override.

### DD-5: Docked ship position
Docked ships derive their position from the harbour entity. No independent position tracking while docked.

---

## Test Plan

- **Unit: harbour_capacity CachedValue** — verify modifier routing populates harbour_capacity correctly
- **Unit: docked_scope parsing** — `docked_to:self::`, `docked_to:<hull>::`, `docked_to:*::` all parse correctly; invalid prefixes rejected
- **Unit: can_dock** — capacity check, same-system check, already-docked check, size check
- **Unit: dock/undock** — component insertion/removal
- **Integration: full dock lifecycle** — dock ship, verify position sync, undock, verify position independence
- **Integration: modifier propagation** — harbour with `docked_to:self::ship.repair_rate +5`, dock a ship, verify ship gets +5 repair_rate, undock, verify modifier removed
- **Integration: combat ROE** — dock ships with different ROE, spawn hostiles, verify Aggressive/Defensive undock while Evasive/Passive stay docked
- **Integration: auto-return** — undock for combat, clear hostiles, verify ships re-dock
- **Integration: harbour destruction** — destroy harbour, verify all docked ships forcibly undocked
- **Integration: move command undock** — issue MoveTo to docked ship, verify auto-undock before movement begins
- **Integration: resolve_combat skip** — docked ships take no damage during combat

---

## Pitfalls

1. **Query conflicts (B0001).** `sync_docked_modifiers` and `sync_ship_module_modifiers` both touch modifier components. Ensure they are chained, not parallel. Use `full_test_app()` to catch conflicts.
2. **RemovedComponents lifetime.** `RemovedComponents<DockedAt>` events are only available for one frame. `sync_docked_modifiers` must run in the same frame as undock to catch cleanup.
3. **Circular modifier application.** `sync_docked_modifiers` must not re-trigger `sync_ship_module_modifiers` in the same frame, or modifiers could double-apply. Use a separate `AppliedDockedModifiers` marker/component to track what was applied.
4. **Harbour destruction ordering.** `force_undock_on_harbour_destroy` must handle the case where the harbour entity is already despawned — use `RemovedComponents` or an event, not a direct query on the harbour.
5. **Save/load.** `DockedAt(Entity)`, `UndockedForCombat`, and `HarbourModifiers` components need persistence support. Coordinate with `persistence/savebag.rs` if not already handled by #372.
6. **System ordering with movement.** `auto_undock_on_move_command` must run before movement systems, otherwise the ship might attempt to move while still docked.
7. **Modifier cleanup on hot-reload.** If harbour modules change (e.g., module swap), previously applied docked modifiers must be cleaned up and reapplied. `sync_docked_modifiers` should diff against `HarbourModifiers` each frame.
