# Implementation Plan: Issue #372 — SystemBuilding Ship Unification + Harbour Capability + DockedAt

_Prepared 2026-04-16. Unifies all SystemBuildings (Shipyard/Port/ResearchLab) as Ship entities, introduces capability-based `harbour` on hulls, and replaces `ShipState::Docked` with orthogonal `DockedAt(Entity)` component + `ShipState::InSystem`._

---

## Sub-issue Table

| ID | Title | Key Files | Deps |
|----|-------|-----------|------|
| A | `HullDefinition.size` + `capabilities` field | `ship_design.rs`, `scripting/ship_design_api.rs`, `scripts/ships/hulls.lua` | none |
| B | `ShipState::Docked` abolition: `InSystem` + `DockedAt` component | `ship/mod.rs`, `ship/movement.rs`, `ship/combat.rs`, `ship/command.rs`, `ship/survey.rs`, `ship/settlement.rs`, `ship/scout.rs`, `ship/pursuit.rs`, `ship/courier_route.rs`, `ship/conquered.rs`, `ship/hitpoints.rs`, `ship/fleet.rs`, `ship/dispatcher.rs`, `ship/handlers/*.rs`, `ship/core_deliverable.rs`, `colony/maintenance.rs`, `colony/authority.rs`, `colony/colonization.rs`, `colony/production.rs`, `knowledge/mod.rs`, `persistence/savebag.rs`, `persistence/save.rs`, `persistence/mod.rs`, `scripting/gamestate_scope.rs`, `player/mod.rs`, `setup/mod.rs`, `visualization/mod.rs`, `visualization/ships.rs`, `visualization/territory.rs`, `ui/ship_panel.rs`, `ui/outline.rs`, `ui/context_menu.rs`, `ui/mod.rs`, tests (~12 files) | none |
| C | Harbour capability: dock/undock systems + capacity enforcement | `ship/harbour.rs` (new), `ship/mod.rs`, `ship/combat.rs` | A, B |
| D | Station hull Lua definitions (Shipyard/Port/ResearchLab/Core hulls) | `scripts/ships/hulls.lua`, `scripts/ships/modules.lua`, `scripts/ships/designs.lua` | A |
| E | SystemBuilding-to-Ship migration runtime | `colony/system_buildings.rs`, `colony/building_queue.rs`, `colony/production.rs`, `setup/mod.rs` | A, B, C, D |
| F | Colonization auto-spawn Shipyard + Core | `colony/colonization.rs`, `ship/settlement.rs`, `setup/mod.rs` | D, E |
| G | Persistence migration (`SAVE_VERSION` bump) | `persistence/savebag.rs`, `persistence/save.rs`, `persistence/mod.rs`, `tests/fixtures_smoke.rs`, `tests/fixtures/minimal_game.bin` | B, E |
| H | UI + visualization updates | `ui/system_panel/mod.rs`, `ui/ship_panel.rs`, `ui/outline.rs`, `visualization/ships.rs`, `visualization/stars.rs`, `ui/situation_center/ship_ops_tab.rs` | B, C, D |

## Merge Order

```
Phase 1 (parallel):   A ──┐     B ──┐
                           │         │
Phase 2 (after A+B):  C ──┤←── A,B  D ──┐←── A
                           │              │
Phase 3 (after C+D):  E ──┤←── C,D  G ──┤←── B,E
                           │              │
Phase 4 (after E):    F ──┘←── D,E  H ──┘←── B,C,D
```

- Phase 1: A and B are independent foundations, merge in parallel.
- Phase 2: C needs both A (size field) and B (DockedAt). D only needs A (hull fields).
- Phase 3: E needs all of A-D. G needs B (new ShipState variants in savebag) and E (migrated entities).
- Phase 4: F needs D+E. H needs B+C+D for display logic.

---

## Design Decisions

### DD-1: Combat behavior of docked ships -- CONFIRMED
**ROE-based.** Aggressive/Defensive ROE ships auto-undock and join combat when hostiles appear in the harbour's system. Evasive/Passive ROE ships stay docked (harbour acts as shield). Forced undock only on harbour destruction.

### DD-2: Hull size values -- PENDING
Proposed: corvette=1, frigate=2, cruiser=4, scout=1, courier=1, core=10, shipyard=20, port=15, lab=10. Need confirmation before D.

### DD-3: Harbour capacity -- PENDING
Proposed: shipyard=12, port=8 (in hull size units). Need confirmation before C/D.

### DD-4: Nesting prohibition -- PENDING
Should harbourable ships be prohibited from docking inside other harbours? Recommendation: hard engine rule (`NOT docker.hull.has_capability("harbour")`) for now. Lua-configurable later if needed.

### DD-5: ResearchLab modifier source -- PENDING
Options: (a) hull modifiers on the station hull definition, (b) modules on station design, (c) system-presence detection system that scans for ships with `research_lab` capability. Recommendation: (c) system-presence is cleanest for the "non-harbourable station" use case, but (b) aligns with existing module modifier framework.

### DD-6: BuildingId to ShipDesign mapping -- PENDING
How does migration (E) know which ShipDesign to spawn for each `BuildingId` in existing `SystemBuildings` slots? Options: (a) hardcoded map, (b) `BuildingDefinition.ship_design_id` Lua field, (c) naming convention (`"shipyard"` -> `"station_shipyard"`). Recommendation: (b) is most explicit and Lua-native.

### DD-7: SystemBuildings backward compat timeline -- PENDING
How long does the `SystemBuildings` component remain as a facade? Options: (a) remove in this epic, (b) keep as read-only derived view for 1-2 releases. Recommendation: (b) keep as derived view for now; fewer downstream breakages.

### DD-8: Port capability source -- PENDING
Port FTL range bonus currently comes from `SystemBuildings.port_ftl_range_bonus()`. After Ship unification: (a) query hull registry for ships with `port` capability in system, (b) runtime component `PortCapability { ftl_range_bonus }` on the ship entity. Recommendation: (a) avoids new components, mirrors existing `has_capability` queries.

---

## Sub-issue Details

### A: HullDefinition.size + capabilities field

**Scope.** Add `size: u32` (required) and `capabilities: HashMap<String, CapabilityParams>` (optional, defaults empty) to `HullDefinition`. Parse both from Lua `define_hull` calls. `CapabilityParams` reuses the same `HashMap<String, f64>` pattern as `DeepSpaceStructure` capabilities. All existing hulls in `scripts/ships/hulls.lua` get a `size` value. No behavioral systems change -- this is pure data model expansion.

**Files.**
- `macrocosmo/src/ship_design.rs` -- add fields to `HullDefinition`
- `macrocosmo/src/scripting/ship_design_api.rs` -- parse `size` + `capabilities` from Lua table
- `scripts/ships/hulls.lua` -- add `size` to every hull definition

**Test plan.**
- Unit: `HullDefinition` with `size` and `capabilities` round-trips through registry
- Unit: missing `size` in Lua `define_hull` produces a parse error
- Unit: `capabilities` absent defaults to empty map
- Unit: `capabilities.harbour.capacity` parses correctly as `f64`

**Pitfalls.**
- `size` must be required (not optional) to prevent silent zero-size ships bypassing capacity checks.
- Existing tests that construct `HullDefinition` manually will need the new field added.

---

### B: ShipState::Docked abolition -- InSystem + DockedAt

**Scope.** Replace `ShipState::Docked { system }` with `ShipState::InSystem { system }`. Add `DockedAt(Entity)` as an orthogonal optional `Component`. Every match arm on `ShipState::Docked` across ~32 files must be rewritten to `InSystem` (and optionally check `DockedAt`). The `spawn_ship` function's default state becomes `InSystem`. Savebag serialization changes from `Docked { system }` to `InSystem { system }` + optional `DockedAt`. This is the highest-risk sub-issue due to sheer file count; a systematic grep-and-replace with careful per-site review is required.

**Files.** (see table above -- approximately 32 source files + 12 test files)

**Test plan.**
- Existing tests pass with `Docked` replaced by `InSystem` (regression)
- New unit: ship with `DockedAt` + `InSystem` -- position derived from harbour entity
- New unit: ship without `DockedAt` + `InSystem` -- position from system
- New unit: `DockedAt` removal sets state to `InSystem` (undock invariant)
- New unit: harbour entity despawned -- orphan `DockedAt` cleanup system removes component, ship becomes `InSystem`
- Integration: `full_test_app()` no query conflicts with `DockedAt` component

**Pitfalls.**
- `ShipState::Docked` appears in savebag deserialization -- old saves must map `Docked { system }` to `InSystem { system }` (no `DockedAt` -- pre-unification saves had no harbour entity).
- Combat system currently skips `Docked` ships. After rename to `InSystem`, combat must check `DockedAt` presence to decide participation (interacts with DD-1 ROE-based logic in sub-issue C).
- `AtSystem` component must be inserted for `InSystem` ships (same as old `Docked`). Verify no code path assumes `AtSystem` implies `Docked`.
- Lua gamestate view (`gamestate_scope.rs`) exposes `ShipState` as `{kind, ...}` tag union -- the `"docked"` kind string must become `"in_system"` (breaking Lua API change; document in migration notes).

---

### C: Harbour capability -- dock/undock systems + capacity enforcement

**Scope.** New module `ship/harbour.rs` implementing: `can_dock(docker, target)` validation (capacity check, nesting prohibition per DD-4), `dock_ship` command (insert `DockedAt`, update capacity tracking), `undock_ship` command (remove `DockedAt`, set `InSystem`), auto-undock on MoveTo command, combat ROE-based undock (DD-1 confirmed), forced undock on harbour destruction. A `HarbourOccupancy` derived query (sum of docked ship sizes) is used for capacity checks.

**Files.**
- `macrocosmo/src/ship/harbour.rs` (new)
- `macrocosmo/src/ship/mod.rs` -- register harbour systems, re-export
- `macrocosmo/src/ship/combat.rs` -- ROE-based auto-undock before combat resolution
- `macrocosmo/src/ship/command.rs` -- auto-undock on MoveTo queue

**Test plan.**
- Unit: dock succeeds when capacity available, ship gets `DockedAt`
- Unit: dock rejected when cumulative size exceeds capacity
- Unit: dock rejected when docker is harbourable (nesting prohibition)
- Unit: undock removes `DockedAt`, ship state = `InSystem { harbour's system }`
- Unit: undock frees capacity for subsequent dock
- Unit: harbour destroyed -- all docked ships get forced `InSystem`, no orphan `DockedAt`
- Unit: Aggressive ROE ship auto-undocks when combat starts in system
- Unit: Evasive ROE ship stays docked during combat
- Unit: MoveTo command triggers auto-undock before movement begins

**Pitfalls.**
- Capacity check must query all `DockedAt` targeting the harbour, not maintain a counter (counters drift on entity despawn).
- Harbour destruction cleanup must run before combat damage resolution to avoid hitting despawned entities.
- System ordering: dock/undock systems must run after `advance_game_time` and before movement systems.

---

### D: Station hull Lua definitions

**Scope.** Define four station hulls (`station_shipyard`, `station_port`, `station_research_lab`, `station_core`) and their associated modules/designs in Lua. Station hulls have `base_speed: 0.0` (immobile), appropriate `size` values (DD-2), and relevant `capabilities`. Shipyard hull gets `harbour` + `shipyard` capabilities, Port gets `harbour` + `port` capabilities, ResearchLab gets `research_lab` capability (no harbour), Core keeps existing definition adapted to new hull fields.

**Files.**
- `scripts/ships/hulls.lua` -- four new hull definitions
- `scripts/ships/modules.lua` -- station-specific modules (research boost, port defense, etc.)
- `scripts/ships/designs.lua` -- four default station designs

**Test plan.**
- Unit: all four station hulls parse and register successfully
- Unit: station designs compute correct stats from hull + modules
- Unit: shipyard/port hulls have `harbour` capability with expected capacity
- Unit: research_lab hull has no `harbour` capability
- Integration: `load_all_scripts` succeeds with new definitions

**Pitfalls.**
- Core hull already exists as `infrastructure_core_v1` -- must migrate or alias, not duplicate.
- Station hulls must have `base_speed: 0.0` and `ftl_range: 0.0` to prevent movement commands.
- Module slots on station hulls should be station-specific slot types to prevent mounting weapons on labs (unless intentional for Port).

---

### E: SystemBuilding-to-Ship migration runtime

**Scope.** At game load (or on first tick after migration), convert each `BuildingId` in `SystemBuildings.slots` into a Ship entity with the corresponding station design (mapping per DD-6). The `SystemBuildings` component becomes a derived view (DD-7): a system that scans for station-ships with `AtSystem` and rebuilds the slots vec each tick (or on change detection). Building queue (`building_queue.rs`) switches from inserting `BuildingId` into slots to spawning Ship entities. Production modifiers from SystemBuildings redirect to module-based modifiers on station ships.

**Files.**
- `macrocosmo/src/colony/system_buildings.rs` -- derived view rebuild system
- `macrocosmo/src/colony/building_queue.rs` -- spawn ship instead of insert BuildingId
- `macrocosmo/src/colony/production.rs` -- modifier source migration
- `macrocosmo/src/setup/mod.rs` -- initial capital setup spawns station ships

**Test plan.**
- Unit: building queue completion spawns station ship with correct design
- Unit: `SystemBuildings` derived view reflects spawned station ships
- Unit: `has_shipyard()` / `has_port()` still work via derived view
- Unit: production modifiers from station ship modules match old SystemBuildings modifiers
- Integration: full game startup with Lua definitions produces correct station ships at capital

**Pitfalls.**
- Derived `SystemBuildings` must remain backward-compatible for all existing consumers (42 files reference it).
- Building queue currently operates on `BuildingId` string -- needs to map to `ShipDesignDefinition` for `spawn_ship`.
- Race condition: derived view must rebuild after station ship spawn in the same frame, or downstream systems see stale data. Use change detection or explicit ordering.

---

### F: Colonization auto-spawn Shipyard + Core

**Scope.** When a colony ship completes settling (`process_settling`), auto-spawn a default Shipyard station ship and Infrastructure Core station ship in the new system. This replaces the current `SystemBuildings` initialization that inserts empty slots. The Shipyard provides the initial harbour for the colony. Interacts with Colony Hub (#280) design if that lands first.

**Files.**
- `macrocosmo/src/colony/colonization.rs` -- `spawn_capital_colony` adds station ships
- `macrocosmo/src/ship/settlement.rs` -- `process_settling` adds station ships
- `macrocosmo/src/setup/mod.rs` -- test helpers

**Test plan.**
- Unit: colonization completion spawns Shipyard + Core ships at system
- Unit: spawned Shipyard has `harbour` capability and correct capacity
- Unit: spawned Core has `CoreShip` marker
- Unit: existing ships can dock at auto-spawned Shipyard
- Integration: full colonization flow from colony ship to functional colony with dockable harbour

**Pitfalls.**
- Must not double-spawn if both `spawn_capital_colony` and `process_settling` run for the same system.
- Core auto-spawn logic in `core_auto_spawn` (#299) may conflict -- reconcile with this sub-issue.
- Auto-spawned stations need `FactionOwner` from the colonizing empire.

---

### G: Persistence migration

**Scope.** Bump `SAVE_VERSION`. Add `DockedAt` to `SavedComponentBag`. Map old `ShipState::Docked { system }` to `InSystem { system }` on load (no `DockedAt` for pre-migration saves). Station ships created by E must serialize/deserialize correctly. Regenerate `minimal_game.bin` fixture.

**Files.**
- `macrocosmo/src/persistence/savebag.rs` -- `SavedDockedAt`, `SavedShipState::InSystem`
- `macrocosmo/src/persistence/save.rs` -- serialize `DockedAt`
- `macrocosmo/src/persistence/mod.rs` -- `SAVE_VERSION` bump, migration logic
- `tests/fixtures_smoke.rs` -- update expected version
- `tests/fixtures/minimal_game.bin` -- regenerate

**Test plan.**
- Unit: save + load round-trip with `DockedAt` component
- Unit: old save (pre-migration) loads with `Docked` mapped to `InSystem`, no `DockedAt`
- Unit: station ships persist and reload with correct capabilities
- Integration: `load_minimal_game_fixture_smoke` passes after fixture regeneration
- Integration: save-load round-trip preserves harbour occupancy

**Pitfalls.**
- Must handle the case where old saves have `ShipState::Docked` but no `DockedAt` field in the bag -- deserialize as `InSystem` with no `DockedAt`.
- Fixture regeneration must run after all other sub-issues are merged (or at least B+E).

---

### H: UI + visualization updates

**Scope.** Update system panel to show station ships as "buildings" section (not in regular ship list). Ship panel shows dock status and harbour occupancy. Outline tree groups station ships under their system. Visualization renders station ships with capability-based icons (distinct from mobile ships). Ship ops ESC tab includes station status.

**Files.**
- `macrocosmo/src/ui/system_panel/mod.rs` -- station ship display in system view
- `macrocosmo/src/ui/ship_panel.rs` -- dock status, harbour occupancy display
- `macrocosmo/src/ui/outline.rs` -- station ship grouping
- `macrocosmo/src/visualization/ships.rs` -- capability-based rendering
- `macrocosmo/src/visualization/stars.rs` -- station markers
- `macrocosmo/src/ui/situation_center/ship_ops_tab.rs` -- station status events

**Test plan.**
- Manual: station ships appear in system panel building section
- Manual: dock/undock reflected in ship panel
- Manual: harbour capacity bar shows current/max occupancy
- Manual: station ships rendered distinctly from mobile ships on galaxy map
- Unit (where possible): outline tree includes station ships under correct system

**Pitfalls.**
- egui systems are not testable in headless mode -- rely on manual verification + screenshot testing.
- Station ships must not appear in "fleet" groupings or receive movement-related UI controls.
- `#369` (Core display) is absorbed here -- ensure Core gets distinct visual treatment.
