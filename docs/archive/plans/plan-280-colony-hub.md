# Implementation Plan: Issue #280 — Colony Hub Phase 1

_Independent of Sovereignty epic (S-series). Depends on #241 (building modifiers, merged) and #281 (on_built/on_upgraded hooks, merged). BuildingRegistry + BuildingDefinition infrastructure already exists._

---

## Overview

Introduce Colony Hub and Planetary Capital buildings as mandatory slot-0 infrastructure. Hub is auto-placed on colonization, provides base building slot capacity, and is non-dismantlable. Capital variant provides enhanced slots and bonuses. Upgrade path: Hub T1 -> T2 -> T3, Capital T1 -> T2 -> T3. Hub T3 upgrades to Capital T1.

## 1. Lua Building Definitions

**File:** `scripts/buildings/basic.lua` (extend existing, or new `scripts/buildings/hub.lua`)

6 new building definitions via `define_building`:

- **`colony_hub_t1`**: base building, `is_system_building = false`, slot 0 mandatory. Capabilities: `colony_hub = { fixed_slots = 4 }`. Cost = nil (not directly buildable — auto-placed). `dismantlable = false`.
- **`colony_hub_t2`**: upgrade from T1. `colony_hub = { fixed_slots = 6 }`. Cost: minerals + energy. `dismantlable = false`.
- **`colony_hub_t3`**: upgrade from T2. `colony_hub = { fixed_slots = 8, slot_ratio = 0.1 }` (10% bonus from population). `dismantlable = false`.
- **`planetary_capital_t1`**: upgrade from Hub T3 (or auto-placed at capital). `colony_hub = { fixed_slots = 10 }`. Additional modifiers: `colony.research_per_hexadies` bonus. `dismantlable = false`.
- **`planetary_capital_t2`**: upgrade from Capital T1. `colony_hub = { fixed_slots = 12 }`. `dismantlable = false`.
- **`planetary_capital_t3`**: upgrade from Capital T2. `colony_hub = { fixed_slots = 14, slot_ratio = 0.15 }`. `dismantlable = false`.

Upgrade chains defined via `upgrade_to` on each definition:
- `colony_hub_t1.upgrade_to = [{ target = "colony_hub_t2", ... }]`
- `colony_hub_t3.upgrade_to = [{ target = "planetary_capital_t1", ... }]`
- etc.

## 2. `dismantlable` Field on BuildingDefinition

**File:** `macrocosmo/src/scripting/building_api.rs`

- Add field to `BuildingDefinition`: `pub dismantlable: bool` (default: `true`)
- Parse from Lua: `dismantlable = table.get::<bool>("dismantlable").unwrap_or(true)`
- This field gates the Demolish action in UI and in `building_queue` processing

## 3. Hub Auto-Spawn on Colonization

Two colonization paths must auto-insert Hub T1 in slot 0:

### (a) `tick_colonization_queue`

**File:** `macrocosmo/src/colony/colonization.rs` — colony spawn block (~L208-237)

- After Colony entity is spawned with its initial `BuildingSlots`, insert `colony_hub_t1` into slot 0
- Requires `Res<BuildingRegistry>` to resolve the building ID
- Slot 0 is reserved — building_slots initialization must account for this (start with capacity from Hub's `fixed_slots` capability)

### (b) Ship settlement (`process_settling`)

**File:** `macrocosmo/src/ship/settlement.rs` — `process_settling` (~L159-190)

- Same pattern: after Colony spawn, insert `colony_hub_t1` into slot 0
- Share logic via a helper: `insert_initial_hub(commands, colony_entity, &building_registry)`

## 4. Slot Expansion via Capability

**File:** `macrocosmo/src/colony/building_queue.rs` (or `macrocosmo/src/colony/mod.rs`)

- When computing available building slots for a colony, check for `colony_hub` capability in slot 0 building
- `fixed_slots`: base number of slots provided by the hub
- `slot_ratio`: additional slots = `floor(population * slot_ratio)` (optional, 0.0 if absent)
- Total colony slots = `hub.fixed_slots + floor(pop * hub.slot_ratio)`
- This replaces or supplements the current `max_building_slots` from planet attributes
- Design: Hub capability is the primary source of slot count. Planet `max_building_slots` becomes a cap/modifier, not the base.

## 5. Capital Spawn at Game Start

**File:** `scripts/factions/init.lua` (or `scripts/lib/capital.lua`)

- In `on_game_start`, the capital colony gets `planetary_capital_t3` placed in slot 0 instead of `colony_hub_t1`
- Use existing `ctx.system:add_building(planet, "planetary_capital_t3")` targeting slot 0
- Or define a dedicated `ctx.colony:set_hub("planetary_capital_t3")` helper

## 6. UI: Hide Demolish for Non-Dismantlable Buildings

**File:** `macrocosmo/src/ui/system_panel.rs` (colony detail view)

- When rendering the Demolish/Remove button for a building slot, check `building_registry.get(building_id).dismantlable`
- If `false`: do not render the button (or render it greyed out with tooltip "This building cannot be demolished")

**File:** `macrocosmo/src/colony/building_queue.rs` — demolish request processing

- Add server-side validation: if `!def.dismantlable`, reject demolish command with warning log

## 7. Save Migration: Insert Hub for Existing Colonies

**File:** `macrocosmo/src/persistence/load.rs`

- After loading colonies from save, check each Colony's slot 0
- If slot 0 is empty (pre-#280 save): insert `colony_hub_t1` (or `planetary_capital_t3` for capital colony)
- Detection: check `SAVE_VERSION` — if loading a save from before this feature, run migration
- Capital detection: check if colony's system has `StarSystem.is_capital == true`
- This is a data migration, not a schema change. SAVE_VERSION bump signals migration needed.

**File:** `macrocosmo/src/persistence/save.rs`

- Bump `SAVE_VERSION` (to 4 or 5 depending on what lands first)

---

## Commit Sequence (6 commits)

1. **`[280] add dismantlable field to BuildingDefinition`** — Rust parser extension, default true, building_api.rs
2. **`[280] Lua hub + capital building definitions`** — 6 define_building calls, upgrade chains, dismantlable=false, capability params
3. **`[280] hub auto-spawn on colonization`** — insert_initial_hub helper, wire into tick_colonization_queue + process_settling
4. **`[280] slot expansion from colony_hub capability`** — compute slots from hub fixed_slots + slot_ratio, replace/supplement max_building_slots
5. **`[280] capital spawn at game start + UI demolish gate`** — scripts/factions/init.lua places planetary_capital_t3, UI hides demolish, server-side validation
6. **`[280] save migration + tests`** — SAVE_VERSION bump, load.rs migration for slot 0, all integration tests

## Test Plan (14+ integration tests)

**File:** `macrocosmo/tests/colony_hub.rs` (new)

1. `test_hub_t1_lua_definition_loads` — BuildingRegistry contains colony_hub_t1 with correct capabilities
2. `test_hub_dismantlable_false` — colony_hub_t1.dismantlable == false
3. `test_capital_t3_lua_definition_loads` — planetary_capital_t3 exists with correct fixed_slots
4. `test_upgrade_chain_hub_to_capital` — colony_hub_t1 -> t2 -> t3 -> planetary_capital_t1 -> t2 -> t3 upgrade_to chain is valid
5. `test_colonization_queue_inserts_hub` — new colony from build queue has colony_hub_t1 in slot 0
6. `test_settlement_inserts_hub` — colony from ship settling has colony_hub_t1 in slot 0
7. `test_hub_provides_fixed_slots` — colony with hub_t1 has 4 available building slots
8. `test_hub_t2_increases_slots` — after upgrading to t2, colony has 6 slots
9. `test_hub_slot_ratio_scales_with_pop` — hub_t3 with slot_ratio=0.1 and pop=50 gives 8 + 5 = 13 slots
10. `test_capital_spawn_at_game_start` — capital colony has planetary_capital_t3 in slot 0
11. `test_demolish_rejected_for_hub` — demolish command on slot 0 (hub) is rejected
12. `test_demolish_allowed_for_normal_building` — demolish on non-hub building succeeds (dismantlable=true)
13. `test_save_migration_inserts_hub` — load a pre-#280 save, verify slot 0 gets colony_hub_t1
14. `test_save_migration_capital_gets_capital_building` — capital colony in old save gets planetary_capital_t3

## Pitfall List

- **Slot 0 convention:** All code that iterates building slots must account for slot 0 being occupied by the hub. Demolish, build queue insertion, and display code must not attempt to overwrite slot 0.
- **BuildingSlots capacity:** Initial Colony spawn sets `BuildingSlots` with some capacity. That capacity must now come from the hub's `fixed_slots`, not solely from planet `max_building_slots`. Verify all Colony spawn paths set capacity correctly.
- **Upgrade-only buildings:** Hub buildings have `cost = nil` / `is_direct_buildable = false`. They should not appear in the "Build" menu — only in the "Upgrade" menu when the prerequisite building is present.
- **max_building_slots interaction:** The planet-level `max_building_slots` attribute and the hub capability must have a clear relationship. Proposal: hub `fixed_slots` is the base, planet attribute caps it. Document the interaction.
- **test_app setup:** Tests need BuildingRegistry populated with hub definitions. Either load Lua scripts or manually register test definitions.
- **Save migration robustness:** The migration must handle edge cases: colonies with all slots full (no room for hub — force-insert at slot 0, shift others?), colonies that already have a hub (idempotent), empty colonies.
- **NPC colonies:** NPC factions that colonize also get hubs. The auto-spawn logic in colonization paths is faction-agnostic — verify it works for NPC-owned colonies.
- **On_built hook:** Hub auto-insertion at colonization time should NOT trigger `on_built` Lua hooks (it's not a build-queue completion). Skip event emission for auto-placed buildings.

## Files to Modify

- `macrocosmo/src/scripting/building_api.rs` (dismantlable field)
- `scripts/buildings/basic.lua` or `scripts/buildings/hub.lua` (new definitions)
- `scripts/init.lua` (require new building file if separate)
- `macrocosmo/src/colony/colonization.rs` (auto-insert hub in tick_colonization_queue)
- `macrocosmo/src/ship/settlement.rs` (auto-insert hub in process_settling)
- `macrocosmo/src/colony/building_queue.rs` (slot expansion logic, demolish validation)
- `macrocosmo/src/colony/mod.rs` (shared helper)
- `macrocosmo/src/ui/system_panel.rs` (demolish button hide)
- `scripts/factions/init.lua` (capital gets planetary_capital_t3)
- `scripts/lib/capital.lua` (if shared capital setup)
- `macrocosmo/src/persistence/save.rs` (SAVE_VERSION bump)
- `macrocosmo/src/persistence/load.rs` (migration)
- `macrocosmo/tests/colony_hub.rs` (new)
