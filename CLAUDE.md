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
├── main.rs              # App setup (12+ plugins)
├── lib.rs               # pub mod re-exports for tests
├── components.rs        # Position, MovementState
├── galaxy/              # StarSystem, SystemAttributes, Sovereignty, generate_galaxy
├── ship/                # Ship, ShipType, ShipState, movement, FTL, survey, settling, command queue
├── colony/              # Colony, Buildings, Production, BuildQueue, ProductionFocus, maintenance
├── knowledge/           # KnowledgeStore, light-speed info propagation
├── communication/       # Messages, PendingCommand, CommandLog
├── technology/          # TechTree, GlobalParams, GameFlags, research (Lua-loaded)
├── scripting/           # LuaJIT ScriptEngine, define_tech() API
├── events.rs            # GameEvent, EventLog, auto-pause
├── player/              # Player, StationedAt
├── physics/             # Distance, light delay, travel time calculations
├── time_system/         # GameClock (hexadies), GameSpeed
├── visualization/       # Galaxy map rendering (sprites, gizmos, camera, click selection)
├── ui/                  # bevy_egui panels
│   ├── mod.rs           # draw_all_ui (single system — egui requires this)
│   ├── top_bar.rs       # Time, speed, resources
│   ├── side_panel.rs    # System info + ship info (split panels)
│   ├── outline.rs       # Left tree view (empire overview)
│   ├── bottom_bar.rs    # Event log
│   └── overlays.rs      # Research panel
├── setup/               # Initial fleet spawn
scripts/tech/            # Lua technology definitions (15 techs, 4 branches)
tests/                   # 145+ tests (unit + integration)
```

### Key Design Patterns

**egui must run in a single system.** All UI panels are drawn from `draw_all_ui` in `src/ui/mod.rs`, registered in `EguiPrimaryContextPass` schedule. Sub-modules export plain functions that take `&egui::Context`, not Bevy systems. This avoids the "available_rect() before Context::run()" panic.

**Bevy Query conflicts (B0001).** Never have two queries accessing the same component as both `&T` and `&mut T` in one system. Use a single mutable query and extract data into locals before mutation. `full_test_app()` in tests catches these at CI time.

**Ship selection persistence.** When a star system is clicked while a ship is selected, the ship stays selected — the system becomes the command target. This was a recurring regression (fixed 3 times).

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
- `visualization/mod.rs` and `ui/side_panel.rs` are frequent merge conflict sources
- Always check hexadies naming after merge (agents sometimes revert to old "sexadies")
- Run `cargo test` after every merge — query conflicts only show at runtime
- The `all_systems_no_query_conflict` integration test catches B0001 issues

### Testing
- `cargo test` runs all tests (unit + integration)
- `test_app()` — headless Bevy with game logic systems only
- `full_test_app()` — includes visualization systems for query conflict detection
- `advance_time(app, hexadies)` — helper to step game time in tests
- egui systems are excluded from tests (need EguiPlugin rendering context)
- `click_select_system` excluded from full_test_app (needs EguiContexts)

### Lua Scripting
- Tech definitions in `scripts/tech/*.lua`
- `define_tech { id, name, branch, cost, prerequisites, effects, description }`
- Fallback: `create_initial_tech_tree()` if scripts/ directory is missing (for tests)
- Future: events, buildings, ships also Lua-defined

## Common Pitfalls

1. **System ordering:** All game logic systems MUST use `.after(crate::time_system::advance_game_time)`. Without this, delta-based systems (tick_production, movement, etc.) may see delta=0 every frame if they run before the clock advances.
2. **egui schedule:** Use `EguiPrimaryContextPass`, not `Update`, for egui systems
3. **Query conflicts:** `Query<&Ship>` + `Query<&mut Ship>` in same system = B0001 panic. Merge into one mutable query.
3. **hexadies naming:** All code uses "hexadies". Never "sexadies".
4. **Ship selection regression:** Don't set `selected_ship.0 = None` when changing SelectedSystem in `click_select_system`
5. **Disk space:** Worktree builds each compile Bevy from scratch. Clean `.claude/worktrees/*/target/` if disk fills up.
6. **ResourceStockpile query conflict:** top_bar reads stockpiles from the colonies query, not a separate `Query<&ResourceStockpile>`, to avoid conflict with the mutable colonies query.

## Game Design Principles

- **Micromanagement should be deep and meaningful.** Direct management at player's location must be clearly better than AI delegation, giving the player a reason to physically be somewhere.
- **All definitions should be scriptable.** Technologies, events, buildings, ships — Lua-defined for fast iteration and future mod support.
- **Resources are local.** Minerals/energy belong to individual colonies. Transfer requires physical courier transport. Research points aggregate at capital with light-speed delay.
- **Research points are flow, not stock.** They cannot accumulate — use them or lose them. Other resources (minerals, energy) are required upfront to start research.
