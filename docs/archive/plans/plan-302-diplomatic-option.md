# Implementation Plan: Issue #302 — S-8 DiplomaticOption Framework

_No hard dependencies (foundation issue). Design spec: `docs/diplomacy-design.md` v2 §1/§2/§5/§6/§9._

---

## Overview

Replace the `DiplomaticAction` enum and `define_diplomatic_action` Lua API with a fully Lua-driven `DiplomaticOption` framework. Each option is a pure dispatcher: label + availability Condition + on_select event + POD responses for inbox items. Faction entities gain `allowed_diplomatic_options` to gate target-side reception. Diplomatic proposals propagate at light-speed as serializable `DiplomaticEvent` PODs.

## 1. DiplomaticOption Definition + Registry

**File:** `macrocosmo/src/faction/diplomatic_option.rs` (new module)

- `ResponseDef { id: String, label: String, event: String }` — POD, no closures, serializable
- `DiplomaticOptionDefinition { id: String, name: String, available: Condition, on_select_event: String, responses: Vec<ResponseDef> }`
  - `on_select_event`: the event id emitted when the actor clicks this option (captured from the Lua `on_select` function at define-time)
- `DiplomaticOptionRegistry` — `Resource`, `HashMap<String, DiplomaticOptionDefinition>`
- Re-export from `faction/mod.rs`

## 2. Faction.allowed_diplomatic_options

**File:** `macrocosmo/src/player/mod.rs` (or wherever `Faction` component lives)

- Add field: `pub allowed_diplomatic_options: HashSet<String>` to `Faction` component
- Default: empty set (factions that allow nothing reject all incoming options)
- Populated from `FactionTypeDefinition` preset at spawn time

**File:** `macrocosmo/src/scripting/faction_api.rs`

- Add `allowed_diplomatic_options` field to `FactionTypeDefinition` parse: optional array of string ids
- Copy into `Faction` component at galaxy generation / `on_game_start` spawn

## 3. DiplomaticEvent (replaces PendingDiplomaticAction payload)

**File:** `macrocosmo/src/faction/diplomatic_option.rs`

- `DiplomaticEvent { pub from: Entity, pub to: Entity, pub option_id: String, pub payload: HashMap<String, String>, pub arrives_at: i64 }` — Component, spawned as entity
  - `payload`: flat string map for serializable POD data (negotiation bundle details in future issues)
- New system `tick_diplomatic_events` in `Update` after `advance_game_time`:
  - Query all `DiplomaticEvent` entities where `arrives_at <= clock.elapsed`
  - On arrival: push into target's `DiplomaticInbox`, despawn the entity
- Coexistence: existing `PendingDiplomaticAction` + `tick_diplomatic_actions` remain until the migration commit removes them

## 4. DiplomaticInbox

**File:** `macrocosmo/src/faction/diplomatic_option.rs`

- `InboxItem { pub from: Entity, pub option_id: String, pub payload: HashMap<String, String>, pub arrived_at: i64 }`
- `DiplomaticInbox` — `Component` on faction entities, `Vec<InboxItem>`
  - `push(&mut self, item: InboxItem)`
  - `drain_by_option(&mut self, option_id: &str) -> Vec<InboxItem>` — for UI consumption
  - `items(&self) -> &[InboxItem]`
- Attach `DiplomaticInbox::default()` to faction entities at spawn

## 5. Lua API: define_diplomatic_option

**File:** `macrocosmo/src/scripting/diplomatic_option_api.rs` (new)

- Parse Lua table: `id`, `name`, `available` (Condition via condition_parser), `on_select` (Lua function — invoke at parse time against builder ctx to capture event id), `responses` (array of `{id, label, event}` — all strings, POD)
- Validate: `responses` must not contain closures (reject userdata/function values)
- Accumulator in ScriptEngine globals, drained into `DiplomaticOptionRegistry` by `load_diplomatic_option_definitions` startup system
- Wire into `scripting/mod.rs` `setup_globals`

**Lua script:** `scripts/factions/options.lua` (new)

- Rewrite existing `trade_agreement` and `cultural_exchange` from `actions.lua` as `define_diplomatic_option`
- Add `break_alliance` as unilateral option (responses: empty, on_select applies immediately)
- Add `require("factions.options")` to `scripts/init.lua`

## 6. FactionTypeDefinition Preset

**File:** `macrocosmo/src/scripting/faction_api.rs`

- Add `allowed_diplomatic_options: Vec<String>` to `FactionTypeDefinition`
- Parse from Lua `define_faction_type` table (optional field, default empty)

**File:** `scripts/factions/faction_types.lua`

- Add `allowed_diplomatic_options` to `empire` type: `{"generic_negotiation", "trade_agreement", "cultural_exchange", "break_alliance"}`
- `space_creature` and `ancient_defense` types: empty (no diplomacy)

## 7. Light-Speed Delay Dispatch

- When `on_select` event handler wants to send a proposal to a remote faction:
  - Compute one-way light-speed delay from actor position to target position (existing `physics::light_delay`)
  - Spawn `DiplomaticEvent` entity with `arrives_at = clock.elapsed + delay`
- Unilateral options (e.g. `break_alliance`): event handler applies immediately on actor side, spawns `DiplomaticEvent` for target notification
- `tick_diplomatic_events` delivers to inbox on arrival

## 8. Response Dispatch

- UI displays inbox items with response buttons from `DiplomaticOptionRegistry.get(item.option_id).responses`
- Button press emits the response's `event` id with payload `{ from: item.from, to: inbox_owner, original_payload: item.payload, response_id: resp.id }`
- Event handler (Lua-registered) processes the response (accept/reject/counter)
- Response event may itself spawn a return `DiplomaticEvent` for the original sender (e.g. acceptance notification with light-speed delay)

## 9. Migration: Remove Old API

**Final commit** — remove in same PR:

- Delete `DiplomaticAction` enum, `PendingDiplomaticAction` struct
- Delete `tick_diplomatic_actions`, `tick_custom_diplomatic_actions` systems
- Delete `DiplomaticActionRegistry` from `scripting/faction_api.rs`
- Delete `define_diplomatic_action` from Lua globals
- Delete `scripts/factions/actions.lua`
- Update all references in persistence (save/load), tests, UI

## 10. Persistence

**File:** `macrocosmo/src/persistence/savebag.rs`

- `SavedInboxItem { from: SavedEntityRef, option_id: String, payload: HashMap<String, String>, arrived_at: i64 }`
- `SavedDiplomaticEvent { from: SavedEntityRef, to: SavedEntityRef, option_id: String, payload: HashMap<String, String>, arrives_at: i64 }` — for in-flight events
- Add to save bag: inbox items per faction, in-flight DiplomaticEvent entities
- `allowed_diplomatic_options: HashSet<String>` saved as part of Faction component

**Files:** `persistence/save.rs`, `persistence/load.rs`

- Serialize/deserialize DiplomaticInbox, in-flight DiplomaticEvent entities, Faction.allowed_diplomatic_options
- Entity ref remapping

## Commit Sequence (6 commits)

1. **`[302] DiplomaticOptionDefinition + Registry + ResponseDef`** — new module, data types, registry resource
2. **`[302] Faction.allowed_diplomatic_options + FactionTypeDefinition preset`** — add field to Faction component, parse from faction_type Lua, copy at spawn
3. **`[302] define_diplomatic_option Lua API + parser`** — diplomatic_option_api.rs, wire into setup_globals, startup loader
4. **`[302] DiplomaticEvent + DiplomaticInbox + tick_diplomatic_events`** — event entity, inbox component, delivery system, light-speed dispatch helper
5. **`[302] Lua scripts: options.lua + faction_types update + init.lua`** — rewrite trade_agreement/cultural_exchange, add break_alliance, faction_type presets
6. **`[302] migration: remove DiplomaticAction enum + old API + persistence + tests`** — delete old types/systems/Lua API, add new persistence, all tests

## Test Plan (10 tests)

**File:** `macrocosmo/tests/diplomatic_option.rs` (new)

1. `test_define_diplomatic_option_lua_parse` — Lua define_diplomatic_option produces valid definition in registry with correct fields
2. `test_responses_are_pod` — responses contain only string fields, no closures
3. `test_faction_allowed_options_from_type` — Faction spawned from type preset has correct allowed_diplomatic_options set
4. `test_faction_allowed_options_empty_for_hostile` — space_creature faction has empty allowed set
5. `test_diplomatic_event_light_speed_delivery` — DiplomaticEvent with arrives_at in future is not delivered; after advancing time, it lands in target inbox
6. `test_diplomatic_event_immediate_delivery` — DiplomaticEvent with arrives_at <= now is delivered on next tick
7. `test_inbox_accumulates_items` — multiple events arriving → multiple InboxItem entries in order
8. `test_inbox_item_payload_preserved` — payload HashMap round-trips correctly through DiplomaticEvent → InboxItem
9. `test_diplomatic_event_save_load_roundtrip` — in-flight DiplomaticEvent + InboxItem survive save/load with correct entity refs
10. `test_old_diplomatic_action_removed` — after migration, no DiplomaticAction enum references compile (compile-time guarantee, but verify no runtime remnants in save format)

## Pitfall List

- **on_select capture:** The Lua `on_select` function must be invoked at parse-time against a builder context that captures the event id, not stored as a closure. Follow the same pattern as end_scenario.on_select in #305.
- **Condition atom gap:** `target_allows_option(id)` atom requires Condition atom expansion (separate issue). Initial `available` conditions can use existing atoms; the atom itself is registered in the expansion issue.
- **PendingDiplomaticAction coexistence:** During development, both old and new systems may coexist. The migration commit (6) removes the old system. Tests should verify no dual-delivery.
- **Faction component location:** `Faction` is in `player/mod.rs` as a simple component. Adding `allowed_diplomatic_options: HashSet<String>` increases its size — ensure save/load handles the new field (Option wrapper for backward compat, or bump SAVE_VERSION).
- **POD enforcement:** `responses` parsing must reject any Lua function/userdata values. Only `string` type allowed for `id`, `label`, `event` fields.
- **Entity lifetime:** `DiplomaticEvent.from`/`.to` reference faction entities. If a faction is despawned (Extinct) while an event is in-flight, `tick_diplomatic_events` must handle the missing entity gracefully (skip delivery, log warning).

## Files to Modify

- `macrocosmo/src/faction/diplomatic_option.rs` (new)
- `macrocosmo/src/faction/mod.rs` (re-export, plugin registration, eventually remove old types)
- `macrocosmo/src/player/mod.rs` (Faction component field)
- `macrocosmo/src/scripting/diplomatic_option_api.rs` (new)
- `macrocosmo/src/scripting/faction_api.rs` (FactionTypeDefinition field, remove old DiplomaticActionRegistry)
- `macrocosmo/src/scripting/mod.rs` (wire globals + startup)
- `macrocosmo/src/persistence/savebag.rs`
- `macrocosmo/src/persistence/save.rs`
- `macrocosmo/src/persistence/load.rs`
- `scripts/factions/options.lua` (new)
- `scripts/factions/faction_types.lua` (update)
- `scripts/factions/actions.lua` (delete in migration)
- `scripts/init.lua` (require update)
- `macrocosmo/tests/diplomatic_option.rs` (new)
