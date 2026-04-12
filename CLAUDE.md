# Macrocosmo — Development Guide

## Project Overview

Rust + Bevy 0.18 space 4X strategy game. Core mechanic: light-speed communication constraint.
See `docs/game-design.md` for full game design document.

## Tech Stack

- **Rust** (edition 2024), **Bevy 0.18.1** (ECS game engine)
- **bevy_egui 0.39.1** — UI panels (runs in `EguiPrimaryContextPass` schedule, NOT `Update`)
- **mlua 0.11** (luajit, vendored) — Lua scripting for game data definitions
- **rand 0.9** — Random number generation

## Time System

- Internal unit: **hexadies** (hexa-dies: 6 days in Latin). 1 month = 5 hexadies, 1 year = 60 hexadies.
- Constants: `HEXADIES_PER_YEAR`, `HEXADIES_PER_MONTH`
- `GameClock.elapsed: i64` — integer hexadies, no floating point

## Architecture

### Module Structure
```
src/
├── main.rs              # App setup (15+ plugins)
├── lib.rs               # pub mod re-exports for tests
├── amount.rs            # Amt (u64 fixed-point ×1000), SignedAmt
├── modifier.rs          # Modifier, ModifiedValue, ScopedModifiers, CachedValue
├── condition.rs         # Condition tree (All/Any/OneOf/Not), ConditionScope, ScopedFlags, EvalContext
├── components.rs        # Position
├── galaxy/              # StarSystem, Planet, SystemAttributes, Sovereignty, HostilePresence, generate_galaxy
├── ship/                # Ship, ShipState, movement, FTL, survey, settling, command queue, ROE, combat
├── ship_design.rs       # HullDefinition, ModuleDefinition, ShipDesignDefinition, registries
├── colony/              # Colony, Buildings, SystemBuildings, Production, BuildQueue, maintenance, colonization
├── deep_space/          # DeepSpaceStructure, StructureDefinition, StructureRegistry (capability-based)
├── knowledge/           # KnowledgeStore, light-speed info propagation (incl. resource snapshots)
├── communication/       # Messages, PendingCommand, CommandLog
├── technology/          # TechTree, GlobalParams, GameFlags, research (Lua-loaded)
├── scripting/           # LuaJIT ScriptEngine, require(), define_xxx() API, reference system
│   ├── mod.rs           # ScriptEngine, sandbox, load_all_scripts, setup_globals
│   ├── condition_parser.rs # Condition tree parsing from Lua tables (incl. scope)
│   ├── condition_ctx.rs    # ConditionCtx + ScopeHandle UserData for Lua function prerequisites
│   ├── ship_design_api.rs  # Hull/Module/Design parsing
│   ├── building_api.rs     # Building definition parsing
│   ├── structure_api.rs    # DeepSpaceStructure definition parsing
│   ├── galaxy_api.rs       # Star/Planet type parsing
│   ├── species_api.rs      # Species/Job parsing
│   ├── event_api.rs        # Event definition parsing
│   ├── modifier_api.rs     # Modifier parsing helpers
│   └── lifecycle.rs        # on_game_start, on_game_load hooks
├── events.rs            # GameEvent, EventLog, auto-pause (only important events)
├── event_system.rs      # EventSystem, EventDefinition, EventBus
├── player/              # Player, StationedAt, AboardShip, update_player_location
├── species.rs           # SpeciesDefinition, JobDefinition
├── physics/             # Distance, light delay, travel time calculations
├── time_system/         # GameClock (hexadies), GameSpeed
├── visualization/       # Galaxy map rendering (sprites, gizmos, camera, territory shader)
│   ├── mod.rs           # Star visuals, ship drawing, ghost markers, camera controls
│   └── territory.rs     # TerritoryMaterial (Material2d shader), authority field 1/r²
├── ui/                  # bevy_egui panels
│   ├── mod.rs           # UiState, 6 chained systems, map tooltips
│   ├── params.rs        # SystemParam bundles (MainPanelWorldQueries, etc.)
│   ├── top_bar.rs       # Time, speed, resources, ship designer button
│   ├── system_panel.rs  # System view (full-screen), planet window, colony detail
│   ├── ship_panel.rs    # Ship detail panel, action types, shared helpers
│   ├── context_menu.rs  # Right-click ship command menu
│   ├── outline.rs       # Left tree view (empire overview, tooltips)
│   ├── bottom_bar.rs    # Event log
│   └── overlays.rs      # Research panel, ship designer
├── setup/               # Initial fleet spawn
scripts/
├── init.lua             # Single entrypoint — require() loads everything in order
├── tech/                # Technology definitions (15 techs, 4 branches)
├── ships/               # Slot types, hulls, modules, designs
├── buildings/           # Building definitions (6 types)
├── structures/          # Deep space structure definitions (capability-based)
├── species/             # Species definitions
├── jobs/                # Job definitions
├── stars/               # Star type definitions
├── planets/             # Planet type definitions
├── events/              # Event definitions
└── lifecycle/           # Lifecycle hooks (on_game_start, etc.)
assets/
└── shaders/
    └── territory.wgsl   # Territory visualization fragment shader
tests/                   # 382 tests (275 unit + 107 integration, 11 test files)
```

### Key Design Patterns

**egui systems are chained in `EguiPrimaryContextPass`.** UI is split into 6 chained systems (`compute_ui_state` -> `draw_top_bar_system` -> `draw_outline_and_tooltips_system` -> `draw_main_panels_system` -> `draw_overlays_system` -> `draw_bottom_bar_system`). Each system gets its own `EguiContexts` parameter. Shared data (player info, resource totals) flows through `UiState` resource. Sub-modules still export plain functions that take `&egui::Context`, not Bevy systems. `SystemParam` bundles in `src/ui/params.rs` keep parameter counts under Bevy's 16-param limit.

**Bevy Query conflicts (B0001).** Never have two queries accessing the same component as both `&T` and `&mut T` in one system. Use a single mutable query and extract data into locals before mutation. `full_test_app()` in tests catches these at CI time.

**Ship selection persistence.** When a star system is clicked while a ship is selected, the ship stays selected — the system becomes the command target. Outline selections are independent — ship and system can both be selected simultaneously.

**ResourceStockpile on StarSystem.** Resources belong to star systems, not individual colonies. All colonies in a system share one stockpile.

**Planet vs System Buildings.** Mine/Farm/PowerPlant are on Colony (planet-level). Shipyard/Port/ResearchLab are on StarSystem via `SystemBuildings` component.

**Unified MoveTo command.** No separate FTL/SubLight commands. `QueuedCommand::MoveTo { system }` auto-routes via `plan_ftl_route` (FTL chain → hybrid FTL+sublight → sublight fallback). FTL requires surveyed destination.

**Capability-based definitions.** Deep space structures and future entities use `capabilities: HashMap<String, CapabilityParams>` instead of hardcoded enum variants. Specific behavior is Lua-defined.

**Scoped Conditions.** `ConditionAtom { kind: AtomKind, scope: ConditionScope }` — atoms carry scope (Any/Empire/System/Planet/Ship). `EvalContext` has named scope slots with `ScopeData` (flags, buildings). `ConditionScope::Any` searches ship→planet→system→empire. Lua supports both static tables (`has_tech("x")`) and function-based prerequisites (`function(ctx) return ctx.empire:has_tech("x") end`). `ConditionCtx` UserData is stateless — builds condition tables, doesn't evaluate.

**ScopedFlags.** `ScopedFlags` component on PlayerEmpire entity (future: StarSystem/Planet/Ship). Parallel to `GameFlags` (staged migration). `_pending_flags` drained from Lua into both in lifecycle hooks.

## Development Workflow

### GitHub Issue Management
- Issue の依存関係は `gh` カスタムエイリアスで管理:
  - `gh add-dep <issue> <blocked-by>` — #issue が #blocked-by にブロックされることを登録
  - `gh rm-dep <issue> <blocked-by>` — 依存関係を削除
  - `gh blocked-by <issue>` — その issue がブロックされている issue 一覧
  - `gh blocking <issue>` — その issue がブロックしている issue 一覧
- 優先度ラベル: `priority:icebox`, `priority:low`, `priority:medium`, `priority:high`, `priority:urgent`

### Parallel Agent Tasks
- Issues are created on GitHub with labels and milestones
- Independent issues are implemented in parallel using worktree-isolated agents
- Each agent works on one issue in its own git worktree
- After all agents complete, a merge agent combines changes into main
- **Beware:** worktree cargo builds share `~/.cargo` registry lock — many concurrent builds are slow but not deadlocked

### Merge Considerations
- `visualization/mod.rs`, `ui/side_panel.rs`, `ui/mod.rs` are frequent merge conflict sources
- Always check hexadies naming after merge (agents sometimes revert to old "sexadies")
- Run `cargo test` after every merge — query conflicts only show at runtime
- The `all_systems_no_query_conflict` integration test catches B0001 issues
- When cherry-picking from worktree branches based on older main, prefer merge agents for complex conflicts

### Testing
- `cargo test` runs all tests (263 unit + 107 integration across 11 files)
- `test_app()` — headless Bevy with game logic systems only
- `full_test_app()` — includes visualization systems for query conflict detection
- `advance_time(app, hexadies)` — helper to step game time in tests
- egui systems are excluded from tests (need EguiPlugin rendering context)
- `click_select_system` excluded from full_test_app (needs EguiContexts)
- **Always add regression tests with bug fixes**

### Lua Scripting

**Single entrypoint.** `scripts/init.lua` is the sole entrypoint for all Lua definitions. It uses `require()` to load subsystems in dependency order. Individual plugins no longer call `load_directory()` — they only parse accumulators after `load_all_scripts` runs.

**Startup ordering:**
```
init_scripting → load_all_scripts → [load_galaxy_types, load_building_registry,
                                      load_technologies, load_ship_designs,
                                      load_structure_definitions, load_species_and_jobs]
                                   → run_lifecycle_hooks
```

**`define_xxx` returns references.** Every `define_xxx { id = "..." }` call returns the table it received, tagged with `_def_type`. This enables return-value based cross-references instead of string IDs:
```lua
-- scripts/tech/industrial.lua
local automated_mining = define_tech { id = "industrial_automated_mining", ... }
local orbital_fabrication = define_tech {
    id = "industrial_orbital_fabrication",
    prerequisites = { automated_mining },  -- reference, not string
    ...
}
return { automated_mining = automated_mining, orbital_fabrication = orbital_fabrication }
```

**`require()` for dependencies.** Lua scripts use standard `require()` to import definitions from other modules:
```lua
-- scripts/ships/designs.lua
local hulls = require("ships.hulls")
local modules = require("ships.modules")
define_ship_design { hull = hulls.corvette, modules = { ... } }
```

**`forward_ref(id)` for not-yet-defined items.** Returns a placeholder table `{ _def_type = "forward_ref", id = id }` for items that will be defined later.

**Backward compatibility.** Rust-side parsers accept both string IDs and reference tables via `extract_ref_id()`. Condition helpers (`has_tech`, `has_building`, `has_modifier`) also accept both forms.

**Lua sandbox.** `ScriptEngine` uses `Lua::new_with()` to load only safe libraries (table, string, math, package, bit). `io`, `os`, `debug`, `ffi` are not loaded. `loadfile` and `dofile` are explicitly set to nil. Only `scripts/` directory files are loadable via `require()`.

**Script path resolution.** `resolve_scripts_dir()` searches: 1) `MACROCOSMO_SCRIPTS_DIR` env var (CI / test override), 2) next to executable, 3) executable ancestors (walks upward looking for `scripts/init.lua`), 4) CWD ancestors, 5) `CARGO_MANIFEST_DIR` (last-resort fallback — this path is baked in at compile time, so it only wins when every other lookup fails and a warning is logged). A directory is considered valid only if it contains `init.lua`. `try_resolve_scripts_dir()` surfaces a descriptive `ScriptsDirError` instead of falling back to a literal `"scripts"` path. Tests and CI can bypass resolution entirely via `ScriptEngine::new_with_scripts_dir(path)` / `ScriptEngine::new_with_rng_and_dir(rng, path)`. Absolute path used for `package.path`.

- BuildingRegistry resource loaded at startup; BuildingType enum still used for runtime logic (known tech debt — should migrate to capability-based)
- Fallback: `create_initial_tech_tree()` if scripts are missing (for tests)

## Common Pitfalls

1. **System ordering:** All game logic systems MUST use `.after(crate::time_system::advance_game_time)`. Without this, delta-based systems (tick_production, movement, etc.) may see delta=0 every frame if they run before the clock advances.
2. **egui schedule:** Use `EguiPrimaryContextPass`, not `Update`, for egui systems. Chain new egui systems with the existing chain in `UiPlugin`.
3. **Query conflicts:** `Query<&Ship>` + `Query<&mut Ship>` in same system = B0001 panic. Merge into one mutable query.
4. **hexadies naming:** All code uses "hexadies". Never "sexadies".
5. **Ship selection regression:** Don't set `selected_ship.0 = None` when changing SelectedSystem in `click_select_system`
6. **Disk space:** Worktree builds each compile Bevy from scratch. Clean `.claude/worktrees/*/target/` if disk fills up.
7. **FTL requires surveyed destination:** `plan_ftl_route` rejects unsurveyed systems. Ships use sublight to reach unsurveyed targets.
8. **Non-FTL ships must not enter FTL routing:** Gate FTL route planning on `ship.ftl_range > 0.0`, not just `effective_ftl_range > 0.0` (tech bonuses can make effective > 0 even for non-FTL ships).
9. **New game elements must be Lua-defined:** Rust provides the engine/framework, Lua defines specific content. No hardcoded enum variants for game content.
10. **Use ModifiedValue for game-affecting numbers.** When touching a numeric value that could be affected by tech, modules, events, or modifiers, make it a `ModifiedValue` (or `ScopedModifiers`). Don't refactor untouched code, but apply this when adding/changing features.
11. **Ship design fields are computed from hull + modules.** `ShipDesignDefinition` stats (hp, speed, ftl_range, build_cost, maintenance) must be calculated from hull + module definitions at registry time, not directly specified in Lua. `can_survey` = `survey_speed > 0`, `can_colonize` = `colonization_speed > 0` (no capability flags).

## Game Design Principles

- **Micromanagement should be deep and meaningful.** Direct management at player's location must be clearly better than AI delegation, giving the player a reason to physically be somewhere.
- **All definitions should be scriptable.** Technologies, events, buildings, ships, structures — Lua-defined for fast iteration and future mod support.
- **Resources are local.** Minerals/energy belong to star systems. Transfer requires physical courier transport. Research points aggregate at capital with light-speed delay.
- **Research points are flow, not stock.** They cannot accumulate — use them or lose them. Other resources (minerals, energy) are required upfront to start research.
- **Light-speed constrains information.** Empire resource totals use delayed KnowledgeStore data for remote systems. Survey results carried back by FTL ships (or light-speed if faster). Commands to remote ships have light-speed delay.
