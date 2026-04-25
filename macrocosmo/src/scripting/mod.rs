pub mod anomaly_api;
pub mod building_api;
pub mod casus_belli_api;
pub mod condition_ctx;
pub mod condition_parser;
pub mod effect_scope;
pub mod engine;
pub mod esc_notifications;
pub mod event_api;
pub mod faction_api;
pub mod galaxy_api;
pub mod galaxy_gen_ctx;
pub mod game_rng;
pub mod game_start_ctx;
pub mod gamestate_scope;
pub mod globals;
pub mod helpers;
pub mod knowledge_api;
pub mod knowledge_dispatch;
pub mod knowledge_registry;
pub mod lifecycle;
pub mod log_buffer;
pub mod map_api;
pub mod modifier_api;
pub mod negotiation_api;
pub mod region_api;
pub mod ship_design_api;
pub mod species_api;
pub mod structure_api;
pub mod victory_api;

// Re-exports for backward compatibility
pub use engine::{
    SCRIPTS_DIR_ENV_VAR, ScriptEngine, ScriptsDirError, ScriptsDirInputs, find_scripts_dir_upwards,
    resolve_scripts_dir, resolve_scripts_dir_from, try_resolve_scripts_dir,
};
pub use game_rng::{GameRng, register_game_rand};
pub use helpers::{extract_id_from_lua_value, extract_ref_id, parse_lua_function_field};

use bevy::prelude::*;

pub struct ScriptingPlugin;

impl Plugin for ScriptingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameRng>()
            .init_resource::<crate::casus_belli::ActiveWars>()
            .add_systems(Startup, init_scripting)
            .add_systems(Startup, init_log_buffer.after(init_scripting))
            .add_systems(
                Update,
                log_buffer::drain_print_buffer.after(crate::time_system::advance_game_time),
            )
            .add_systems(Startup, load_all_scripts.after(init_scripting))
            .add_systems(Startup, load_faction_type_registry.after(load_all_scripts))
            .add_systems(Startup, load_casus_belli_registry.after(load_all_scripts))
            .add_systems(
                Startup,
                load_faction_registry
                    .after(load_all_scripts)
                    .after(load_faction_type_registry),
            )
            .add_systems(
                Startup,
                load_diplomatic_option_registry.after(load_all_scripts),
            )
            .add_systems(
                Startup,
                load_negotiation_item_kind_registry.after(load_all_scripts),
            )
            .add_systems(
                Startup,
                anomaly_api::load_anomaly_registry.after(load_all_scripts),
            )
            .add_systems(
                Startup,
                load_predefined_system_registry.after(load_all_scripts),
            )
            .add_systems(Startup, load_map_type_registry.after(load_all_scripts))
            .add_systems(Startup, load_region_type_registry.after(load_all_scripts))
            .add_systems(Startup, load_region_spec_queue.after(load_all_scripts))
            .add_systems(Startup, load_event_definitions.after(load_all_scripts))
            // #350 K-1: build KindRegistry + reserve <id>@recorded /
            // <id>@observed entries.
            .add_systems(
                Startup,
                load_knowledge_kinds
                    .after(load_all_scripts)
                    .before(lifecycle::run_lifecycle_hooks),
            )
            // #352 (K-3): drain Lua-side knowledge subscription accumulator
            // into the bucketed KnowledgeSubscriptionRegistry.
            .add_systems(
                Startup,
                knowledge_registry::load_knowledge_subscriptions
                    .after(load_all_scripts)
                    .after(load_knowledge_kinds)
                    .before(lifecycle::run_lifecycle_hooks),
            )
            // #281: After the building/structure registries are populated,
            // walk their `on_built` / `on_upgraded` fields and register
            // filtered handlers on `_event_handlers` so the dispatcher
            // treats them like any other `on("macrocosmo:building_built", ...)`
            // registration. Runs before `run_lifecycle_hooks` so lifecycle
            // code that fires events at game start observes the hooks.
            .add_systems(
                Startup,
                register_building_built_hooks
                    .after(crate::colony::load_building_registry)
                    .after(crate::deep_space::load_structure_definitions)
                    .before(lifecycle::run_lifecycle_hooks),
            )
            .add_systems(
                Startup,
                lifecycle::run_lifecycle_hooks
                    .after(load_all_scripts)
                    .after(load_event_definitions)
                    .after(crate::colony::load_building_registry)
                    .after(crate::technology::load_technologies),
            )
            .add_systems(
                Update,
                lifecycle::drain_script_events.after(crate::time_system::advance_game_time),
            )
            .add_systems(
                Update,
                // Must run before tick_events so suppressed events never hit
                // the fired_log. #263.
                lifecycle::evaluate_fire_conditions
                    .before(crate::event_system::tick_events)
                    .after(crate::time_system::advance_game_time),
            )
            .add_systems(
                Update,
                lifecycle::dispatch_event_handlers
                    .after(crate::event_system::tick_events)
                    .after(crate::time_system::advance_game_time),
            )
            // #351 K-2: Rust-origin knowledge records queue + dispatch.
            .init_resource::<knowledge_dispatch::PendingKnowledgeRecords>()
            .add_systems(
                Update,
                knowledge_dispatch::dispatch_knowledge_recorded
                    .after(crate::time_system::advance_game_time),
            )
            // #353 K-4 / #354 K-5: drain ALL ready facts (core +
            // scripted) whose arrival time has elapsed, fire
            // `<kind>@observed` subscribers, and push banners for core
            // variants as a post-dispatch side-effect. This replaces
            // the legacy `notify_from_knowledge_facts` drainer (now
            // unregistered from `NotificationsPlugin`). Exclusive
            // (&mut World) because subscribers may re-enter gs:*
            // setters.
            .add_systems(
                Update,
                knowledge_dispatch::dispatch_knowledge_observed
                    .after(crate::time_system::advance_game_time)
                    .after(crate::notifications::auto_notify_from_events),
            )
            // #345 ESC-2: drain the `_pending_esc_notifications` Lua
            // accumulator populated by `push_notification { ... }`
            // calls (typically from `scripts/notifications/default_bridge.lua`
            // inside the `*@observed` subscriber chain). Ordered
            // `.after(dispatch_knowledge_observed)` so subscribers
            // that fire this tick land in `EscNotificationQueue`
            // within the same frame. Also `.before(sweep_notified_event_ids)`
            // so the dedup map still holds `try_notify` state when we
            // read it.
            .add_systems(
                Update,
                esc_notifications::drain_pending_esc_notifications
                    .after(crate::time_system::advance_game_time)
                    .after(knowledge_dispatch::dispatch_knowledge_observed)
                    .before(crate::knowledge::sweep_notified_event_ids),
            );
    }
}

/// Startup system that initialises the Lua scripting engine and inserts it as a
/// Bevy resource. Other startup systems can depend on this via `.after(init_scripting)`.
pub fn init_scripting(mut commands: Commands, rng: Res<GameRng>) {
    let engine = ScriptEngine::new_with_rng(rng.handle())
        .expect("Failed to initialize Lua scripting engine");
    commands.insert_resource(engine);
}

/// Startup system that creates the `LogBuffer` resource wired to the
/// `ScriptEngine`'s shared print buffer. Must run after `init_scripting`.
fn init_log_buffer(mut commands: Commands, engine: Res<ScriptEngine>) {
    let buffer = log_buffer::LogBuffer::with_shared(engine.print_buffer());
    commands.insert_resource(buffer);
}

/// Startup system that loads all Lua scripts via `scripts/init.lua` (if it exists),
/// falling back to loading individual directories for backward compatibility.
/// Other startup systems that parse definitions should use `.after(load_all_scripts)`.
pub fn load_all_scripts(engine: Res<ScriptEngine>) {
    let scripts_dir = engine.scripts_dir();
    let init_path = scripts_dir.join("init.lua");
    if init_path.exists() {
        match engine.load_file(&init_path) {
            Ok(()) => {
                info!("All scripts loaded via {}", init_path.display());
                return;
            }
            Err(e) => {
                warn!(
                    "Failed to load {}: {e}; falling back to directory loading",
                    init_path.display()
                );
            }
        }
    }

    // Fallback: load directories individually (legacy path, used when init.lua is absent)
    let subdirs = [
        "stars",
        "planets",
        "jobs",
        "species",
        "buildings",
        "tech",
        "ships",
        "structures",
        "events",
    ];
    for subdir in &subdirs {
        let path = scripts_dir.join(subdir);
        if path.is_dir() {
            if let Err(e) = engine.load_directory(&path) {
                warn!("Failed to load scripts from {}: {e}", path.display());
            }
        }
    }
}

/// Startup system that parses Lua faction-type definitions into
/// `FactionTypeRegistry`. Scheduled before `load_faction_registry` so the
/// resource exists by the time anything that needs to resolve a faction's
/// `faction_type` runs.
pub fn load_faction_type_registry(mut commands: Commands, engine: Res<ScriptEngine>) {
    match faction_api::parse_faction_type_definitions(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            let mut registry = faction_api::FactionTypeRegistry::default();
            for def in defs {
                registry.types.insert(def.id.clone(), def);
            }
            commands.insert_resource(registry);
            info!("Loaded {} faction type definitions from Lua", count);
        }
        Err(e) => {
            warn!("Failed to parse faction type definitions: {e}");
            commands.insert_resource(faction_api::FactionTypeRegistry::default());
        }
    }
}

/// Startup system that parses Lua faction definitions into FactionRegistry.
pub fn load_faction_registry(mut commands: Commands, engine: Res<ScriptEngine>) {
    match faction_api::parse_faction_definitions(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            let mut registry = faction_api::FactionRegistry::default();
            for def in defs {
                registry.factions.insert(def.id.clone(), def);
            }
            commands.insert_resource(registry);
            info!("Loaded {} faction definitions from Lua", count);
        }
        Err(e) => {
            warn!("Failed to parse faction definitions: {e}");
            commands.insert_resource(faction_api::FactionRegistry::default());
        }
    }
}

/// #305 (S-11): Startup system that parses Lua `define_casus_belli` blocks
/// into [`CasusBelliRegistry`]. Runs after `load_all_scripts`.
pub fn load_casus_belli_registry(mut commands: Commands, engine: Res<ScriptEngine>) {
    use crate::casus_belli::CasusBelliRegistry;
    match casus_belli_api::parse_casus_belli_definitions(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            let mut registry = CasusBelliRegistry::default();
            for def in defs {
                registry.definitions.insert(def.id.clone(), def);
            }
            commands.insert_resource(registry);
            info!("Loaded {} casus belli definitions from Lua", count);
        }
        Err(e) => {
            warn!("Failed to parse casus belli definitions: {e}");
            commands.insert_resource(CasusBelliRegistry::default());
        }
    }
}

/// #321: Startup system that parses Lua `define_negotiation_item_kind` blocks
/// into [`NegotiationItemKindRegistry`]. Runs after `load_all_scripts`.
pub fn load_negotiation_item_kind_registry(mut commands: Commands, engine: Res<ScriptEngine>) {
    use crate::negotiation::NegotiationItemKindRegistry;
    match negotiation_api::parse_negotiation_item_kind_definitions(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            let mut registry = NegotiationItemKindRegistry::default();
            for def in defs {
                registry.kinds.insert(def.id.clone(), def);
            }
            commands.insert_resource(registry);
            info!(
                "Loaded {} negotiation item kind definitions from Lua",
                count
            );
        }
        Err(e) => {
            warn!("Failed to parse negotiation item kind definitions: {e}");
            commands.insert_resource(NegotiationItemKindRegistry::default());
        }
    }
}

/// Startup system that parses Lua diplomatic-option definitions into
/// [`faction_api::DiplomaticOptionRegistry`] (#302). Runs after
/// `load_all_scripts`.
pub fn load_diplomatic_option_registry(mut commands: Commands, engine: Res<ScriptEngine>) {
    match faction_api::parse_diplomatic_option_definitions(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            let mut registry = faction_api::DiplomaticOptionRegistry::default();
            for def in defs {
                registry.options.insert(def.id.clone(), def);
            }
            commands.insert_resource(registry);
            info!("Loaded {} diplomatic option definitions from Lua", count);
        }
        Err(e) => {
            warn!("Failed to parse diplomatic option definitions: {e}");
            commands.insert_resource(faction_api::DiplomaticOptionRegistry::default());
        }
    }
}

/// #182: Startup system that parses Lua `define_predefined_system` blocks
/// into [`map_api::PredefinedSystemRegistry`]. Runs after `load_all_scripts`.
pub fn load_predefined_system_registry(mut commands: Commands, engine: Res<ScriptEngine>) {
    match map_api::parse_predefined_systems(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            let mut registry = map_api::PredefinedSystemRegistry::default();
            for def in defs {
                registry.systems.insert(def.id.clone(), def);
            }
            commands.insert_resource(registry);
            info!("Loaded {} predefined system definitions from Lua", count);
        }
        Err(e) => {
            warn!("Failed to parse predefined system definitions: {e}");
            commands.insert_resource(map_api::PredefinedSystemRegistry::default());
        }
    }
}

/// #182: Startup system that parses Lua `define_map_type` blocks into
/// [`map_api::MapTypeRegistry`] and reads the active map type id from the
/// `_active_map_type` global.
pub fn load_map_type_registry(mut commands: Commands, engine: Res<ScriptEngine>) {
    let mut registry = map_api::MapTypeRegistry::default();
    match map_api::parse_map_types(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            for def in defs {
                registry.types.insert(def.id.clone(), def);
            }
            registry.current = map_api::read_active_map_type(engine.lua());
            info!(
                "Loaded {} map type definitions from Lua (active: {:?})",
                count, registry.current
            );
        }
        Err(e) => {
            warn!("Failed to parse map type definitions: {e}");
        }
    }
    commands.insert_resource(registry);
}

/// #145: Startup system that parses Lua `define_region_type` blocks into
/// [`crate::galaxy::region::RegionTypeRegistry`].
pub fn load_region_type_registry(mut commands: Commands, engine: Res<ScriptEngine>) {
    use crate::galaxy::region::RegionTypeRegistry;
    let mut registry = RegionTypeRegistry::default();
    match region_api::parse_region_type_definitions(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            for def in defs {
                registry.types.insert(def.id.clone(), def);
            }
            info!("Loaded {} region type definitions from Lua", count);
        }
        Err(e) => {
            warn!("Failed to parse region type definitions: {e}");
        }
    }
    commands.insert_resource(registry);
}

/// #145: Startup system that drains `_pending_region_specs` into the
/// [`crate::galaxy::region::RegionSpecQueue`] resource consumed by
/// `place_forbidden_regions` at galaxy-generation time.
pub fn load_region_spec_queue(mut commands: Commands, engine: Res<ScriptEngine>) {
    use crate::galaxy::region::RegionSpecQueue;
    let mut queue = RegionSpecQueue::default();
    match region_api::parse_region_specs(engine.lua()) {
        Ok(specs) => {
            queue.specs = specs;
            info!(
                "Loaded {} region placement specs from Lua",
                queue.specs.len()
            );
        }
        Err(e) => {
            warn!("Failed to parse region placement specs: {e}");
        }
    }
    commands.insert_resource(queue);
}

/// #281: Walk the loaded BuildingRegistry / StructureRegistry and register
/// their `on_built` / `on_upgraded` hooks as filtered entries on Lua's
/// `_event_handlers` table. Each hook becomes equivalent to a user-written
/// `on("macrocosmo:building_built", { building_id = ..., cause = ... }, fn)`
/// call, so:
///
/// * The dispatcher already knows how to call these (no new code path).
/// * External `on()` subscriptions and definition hooks coexist — they are
///   all entries in the same `_event_handlers` table.
/// * Filtering by `building_id` + `cause` means a hook on one building never
///   fires for a different building or the wrong completion kind.
///
/// Hooks with no real function attached (historical
/// `LuaFunctionRef::placeholder(i64)` test scaffolding) are skipped so we
/// don't insert broken entries.
pub fn register_building_built_hooks(
    engine: Res<ScriptEngine>,
    building_registry: Res<crate::scripting::building_api::BuildingRegistry>,
    structure_registry: Res<crate::deep_space::StructureRegistry>,
) {
    let lua = engine.lua();
    let Ok(handlers) = lua.globals().get::<mlua::Table>("_event_handlers") else {
        warn!("register_building_built_hooks: _event_handlers table missing; skipping");
        return;
    };

    let mut total_registered = 0usize;

    let mut push_entry = |building_id: &str, cause: &str, func: mlua::Function| {
        let entry = match lua.create_table() {
            Ok(t) => t,
            Err(e) => {
                warn!("register_building_built_hooks: create_table failed: {e}");
                return;
            }
        };
        let filter = match lua.create_table() {
            Ok(t) => t,
            Err(e) => {
                warn!("register_building_built_hooks: create_table (filter) failed: {e}");
                return;
            }
        };
        let _ = filter.set("building_id", building_id);
        let _ = filter.set("cause", cause);
        let _ = entry.set("event_id", crate::event_system::BUILDING_BUILT_EVENT);
        let _ = entry.set("filter", filter);
        let _ = entry.set("func", func);
        let next_idx = handlers.len().unwrap_or(0) + 1;
        if let Err(e) = handlers.set(next_idx, entry) {
            warn!("register_building_built_hooks: append failed: {e}");
            return;
        }
        total_registered += 1;
    };

    for def in building_registry.buildings.values() {
        if let Some(hook_ref) = &def.on_built {
            if let Ok(Some(func)) = hook_ref.get(lua) {
                push_entry(&def.id, "construction", func);
            }
        }
        if let Some(hook_ref) = &def.on_upgraded {
            if let Ok(Some(func)) = hook_ref.get(lua) {
                push_entry(&def.id, "upgrade", func);
            }
        }
    }

    for def in structure_registry.definitions.values() {
        if let Some(hook_ref) = &def.on_built {
            if let Ok(Some(func)) = hook_ref.get(lua) {
                push_entry(&def.id, "construction", func);
            }
        }
        if let Some(hook_ref) = &def.on_upgraded {
            if let Ok(Some(func)) = hook_ref.get(lua) {
                push_entry(&def.id, "upgrade", func);
            }
        }
    }

    info!(
        "#281: registered {} definition-level building_built hook(s)",
        total_registered
    );
}

/// Startup system that parses Lua event definitions and registers them in EventSystem.
fn load_event_definitions(
    engine: Res<ScriptEngine>,
    mut event_system: ResMut<crate::event_system::EventSystem>,
) {
    match event_api::parse_event_definitions(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            for def in defs {
                event_system.register(def);
            }
            info!("Loaded {} event definitions from Lua", count);
        }
        Err(e) => {
            warn!("Failed to parse event definitions: {e}");
        }
    }
}

/// #350: Startup system that parses Lua `define_knowledge` entries into a
/// [`crate::knowledge::kind_registry::KindRegistry`] resource and reserves
/// the matching `<id>@recorded` / `<id>@observed` lifecycle event ids in
/// `_knowledge_reserved_events` (plan-349 §3.1 commit 4).
///
/// Ordering: runs `.after(load_all_scripts).before(lifecycle::run_lifecycle_hooks)`
/// so that `on_game_start` callbacks observing newly-reserved kinds fire
/// against a fully-populated registry.
///
/// Error handling: parse failures surface as `warn!` + an empty registry
/// (consistent with `load_event_definitions` / `load_anomaly_registry`).
/// The game still boots; downstream `record_knowledge` calls will fail at
/// the callsite instead of at startup. K-5 preloads `core:*` here once
/// the Rust-side core variants land.
pub fn load_knowledge_kinds(mut commands: Commands, engine: Res<ScriptEngine>) {
    use crate::knowledge::kind_registry::KindRegistry;

    let lua = engine.lua();
    // #354 K-5: preload `core:*` kinds before draining the Lua
    // accumulator so subsequent Lua definitions of the same id trip
    // either `CoreNamespaceReserved` (Lua origin) or `DuplicateKind`
    // (both caught by the warn-log below). plan §0.5 9.6 / §3.5.
    let mut registry = KindRegistry::preload_core();
    info!(
        "Preloaded {} core:* knowledge kind definition(s)",
        registry.len()
    );

    // #354 K-5: Reserve `<core:*>@recorded` / `<core:*>@observed` event
    // ids in the Lua-side `_knowledge_reserved_events` table so K-3's
    // subscription tooling (`is_reserved_knowledge_event` + future
    // modder diagnostics) sees core:* alongside Lua-defined kinds.
    let core_defs: Vec<_> = registry.kinds.values().cloned().collect();
    if let Err(e) = knowledge_api::register_auto_lifecycle_events(lua, &core_defs) {
        warn!("Failed to reserve core:* knowledge lifecycle events: {e}");
    }

    match knowledge_api::parse_knowledge_definitions(lua) {
        Ok(defs) => {
            let count = defs.len();
            // Reserve lifecycle events first so even failed inserts (e.g.
            // a core-namespace attempt later) don't leave reservations in
            // an inconsistent state — `register_auto_lifecycle_events`
            // already tolerates duplicates.
            if let Err(e) = knowledge_api::register_auto_lifecycle_events(lua, &defs) {
                warn!("Failed to reserve knowledge lifecycle events: {e}");
            }
            for def in defs {
                let id = def.id.as_str().to_string();
                if let Err(e) = registry.insert(def) {
                    warn!("knowledge kind register error: {e} (id='{id}')");
                }
            }
            info!(
                "Loaded {} knowledge kind definition(s) from Lua",
                registry.len().min(count)
            );
        }
        Err(e) => {
            warn!("Failed to parse knowledge definitions: {e}");
        }
    }
    commands.insert_resource(registry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_engine_creates_globals() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // macrocosmo table exists
        let mc: mlua::Table = lua.globals().get("macrocosmo").unwrap();
        assert!(mc.len().unwrap() == 0);

        // define_tech function exists
        let _func: mlua::Function = lua.globals().get("define_tech").unwrap();

        // _tech_definitions table exists and is empty
        let defs: mlua::Table = lua.globals().get("_tech_definitions").unwrap();
        assert_eq!(defs.len().unwrap(), 0);
    }

    #[test]
    fn test_define_tech_accumulates() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_tech { id = 1, name = "A" }
            define_tech { id = 2, name = "B" }
            "#,
        )
        .exec()
        .unwrap();

        let defs: mlua::Table = lua.globals().get("_tech_definitions").unwrap();
        assert_eq!(defs.len().unwrap(), 2);
    }

    #[test]
    fn test_load_directory_missing_dir() {
        let engine = ScriptEngine::new().unwrap();
        // Should not error when directory doesn't exist
        engine
            .load_directory(Path::new("/nonexistent/path"))
            .unwrap();
    }

    // --- #45 → #332-B4: Lua binding tests ---
    //
    // `test_modify_global_lua` and `test_set_flag_lua` were removed
    // along with the `modify_global(param, v)` / `set_flag(name)`
    // global helpers they exercised (plan §9 / B4). Flag writes are
    // now performed via `gs:set_flag(scope_kind, id, name, value)` on
    // the event / lifecycle gamestate surface, and tested in
    // `tests/lua_gamestate_mutations.rs` +
    // `tests/lifecycle_hook_mutations.rs`. Global param changes go
    // through `EffectScope` descriptors (see `effect_scope.rs` and
    // `technology::effects`).

    #[test]
    fn test_check_flag_lua() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // `check_flag` looks up the name in `_flag_store`; unseen keys
        // return false.
        let result: bool = lua
            .load(r#"return check_flag("nonexistent")"#)
            .eval()
            .unwrap();
        assert!(!result);

        // Prime `_flag_store` directly (the `set_flag(name)` helper
        // that used to do this is retired in Phase B4).
        let store: mlua::Table = lua.globals().get("_flag_store").unwrap();
        store.set("my_flag", true).unwrap();

        let result: bool = lua.load(r#"return check_flag("my_flag")"#).eval().unwrap();
        assert!(result);
    }

    #[test]
    fn test_on_function_registers_handler() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on("macrocosmo:test_event", function(evt)
                -- handler body
            end)
            "#,
        )
        .exec()
        .unwrap();

        let handlers: mlua::Table = lua.globals().get("_event_handlers").unwrap();
        assert_eq!(handlers.len().unwrap(), 1);

        let entry: mlua::Table = handlers.get(1).unwrap();
        let eid: String = entry.get("event_id").unwrap();
        assert_eq!(eid, "macrocosmo:test_event");

        // No filter should be set
        let filter: mlua::Value = entry.get("filter").unwrap();
        assert!(matches!(filter, mlua::Value::Nil));

        // Handler function should be present
        let _func: mlua::Function = entry.get("func").unwrap();
    }

    #[test]
    fn test_on_with_filter() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on("macrocosmo:building_lost", { cause = "combat" }, function(evt)
                -- handler body
            end)
            "#,
        )
        .exec()
        .unwrap();

        let handlers: mlua::Table = lua.globals().get("_event_handlers").unwrap();
        assert_eq!(handlers.len().unwrap(), 1);

        let entry: mlua::Table = handlers.get(1).unwrap();
        let eid: String = entry.get("event_id").unwrap();
        assert_eq!(eid, "macrocosmo:building_lost");

        // Filter should be present with the correct key/value
        let filter: mlua::Table = entry.get("filter").unwrap();
        let cause: String = filter.get("cause").unwrap();
        assert_eq!(cause, "combat");

        // Handler function should be present
        let _func: mlua::Function = entry.get("func").unwrap();
    }

    #[test]
    fn test_on_multiple_handlers() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on("macrocosmo:event_a", function(evt) end)
            on("macrocosmo:event_b", { key = "val" }, function(evt) end)
            on("macrocosmo:event_a", function(evt) end)
            "#,
        )
        .exec()
        .unwrap();

        let handlers: mlua::Table = lua.globals().get("_event_handlers").unwrap();
        assert_eq!(handlers.len().unwrap(), 3);
    }

    // --- #352 (K-3) `on()` knowledge-event-id routing tests ---

    #[test]
    fn on_routes_knowledge_id_to_subscribers() {
        // `on("foo:bar@recorded", fn)` must land in the knowledge
        // subscription accumulator, NOT the legacy `_event_handlers` table.
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(r#"on("vesk:famine_outbreak@recorded", function(e) end)"#)
            .exec()
            .unwrap();

        let knowledge: mlua::Table = lua
            .globals()
            .get(knowledge_registry::PENDING_KNOWLEDGE_SUBSCRIPTIONS)
            .unwrap();
        assert_eq!(knowledge.len().unwrap(), 1);
        let entry: mlua::Table = knowledge.get(1).unwrap();
        let eid: String = entry.get("event_id").unwrap();
        assert_eq!(eid, "vesk:famine_outbreak@recorded");
        let _func: mlua::Function = entry.get("func").unwrap();

        // Legacy handler table remains empty.
        let legacy: mlua::Table = lua.globals().get("_event_handlers").unwrap();
        assert_eq!(legacy.len().unwrap(), 0);
    }

    #[test]
    fn on_routes_wildcard_to_subscribers() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(r#"on("*@observed", function(e) end)"#)
            .exec()
            .unwrap();

        let knowledge: mlua::Table = lua
            .globals()
            .get(knowledge_registry::PENDING_KNOWLEDGE_SUBSCRIPTIONS)
            .unwrap();
        assert_eq!(knowledge.len().unwrap(), 1);
        let eid: String = knowledge
            .get::<mlua::Table>(1)
            .unwrap()
            .get("event_id")
            .unwrap();
        assert_eq!(eid, "*@observed");
    }

    #[test]
    fn on_routes_legacy_event_id_to_handlers() {
        // Non-knowledge ids must continue to use `_event_handlers`.
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(r#"on("harvest_ended", function(e) end)"#)
            .exec()
            .unwrap();

        let legacy: mlua::Table = lua.globals().get("_event_handlers").unwrap();
        assert_eq!(legacy.len().unwrap(), 1);
        let knowledge: mlua::Table = lua
            .globals()
            .get(knowledge_registry::PENDING_KNOWLEDGE_SUBSCRIPTIONS)
            .unwrap();
        assert_eq!(knowledge.len().unwrap(), 0);
    }

    #[test]
    fn on_unknown_lifecycle_errors() {
        // Any id containing '@' must parse as a knowledge id; an unknown
        // lifecycle suffix is a load-time error (plan-349 §0.5 9.2).
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let r: mlua::Result<()> = lua.load(r#"on("foo@expired", function(e) end)"#).exec();
        assert!(r.is_err(), "unknown lifecycle must error");
        let msg = format!("{}", r.unwrap_err());
        assert!(msg.contains("unknown lifecycle"), "got: {msg}");
    }

    #[test]
    fn on_empty_kind_errors() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let r: mlua::Result<()> = lua.load(r#"on("@recorded", function(e) end)"#).exec();
        assert!(r.is_err(), "empty kind must error");
        let msg = format!("{}", r.unwrap_err());
        assert!(msg.contains("empty kind"), "got: {msg}");
    }

    #[test]
    fn on_double_at_errors() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let r: mlua::Result<()> = lua.load(r#"on("a@b@recorded", function(e) end)"#).exec();
        assert!(r.is_err(), "double '@' must error");
        let msg = format!("{}", r.unwrap_err());
        assert!(msg.contains("may not contain '@'"), "got: {msg}");
    }

    #[test]
    fn on_knowledge_with_filter_errors() {
        // Knowledge subscriptions do not accept the bus-style filter table.
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();
        let r: mlua::Result<()> = lua
            .load(r#"on("foo@recorded", { kind = "x" }, function(e) end)"#)
            .exec();
        assert!(r.is_err(), "filter on knowledge id must error");
        let msg = format!("{}", r.unwrap_err());
        assert!(msg.contains("does not accept a filter"), "got: {msg}");
    }

    #[test]
    fn on_mixed_registration_order_preserved() {
        // Multiple on() calls accumulate in registration order in their
        // respective tables. Exact and wildcard knowledge subscriptions
        // share a single accumulator (drain-time bucketing preserves the
        // per-bucket order).
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on("kind_a@recorded", function(e) end)
            on("*@recorded", function(e) end)
            on("kind_a@recorded", function(e) end)
            on("kind_b@observed", function(e) end)
            "#,
        )
        .exec()
        .unwrap();

        let pending: mlua::Table = lua
            .globals()
            .get(knowledge_registry::PENDING_KNOWLEDGE_SUBSCRIPTIONS)
            .unwrap();
        assert_eq!(pending.len().unwrap(), 4);
        let ids: Vec<String> = (1..=4)
            .map(|i| {
                pending
                    .get::<mlua::Table>(i)
                    .unwrap()
                    .get::<String>("event_id")
                    .unwrap()
            })
            .collect();
        assert_eq!(
            ids,
            vec![
                "kind_a@recorded".to_string(),
                "*@recorded".to_string(),
                "kind_a@recorded".to_string(),
                "kind_b@observed".to_string(),
            ]
        );
    }

    #[test]
    fn test_define_tech_returns_reference() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let result: mlua::Table = lua
            .load(r#"return define_tech { id = "test_tech", name = "Test" }"#)
            .eval()
            .unwrap();

        let def_type: String = result.get("_def_type").unwrap();
        assert_eq!(def_type, "tech");
        let id: String = result.get("id").unwrap();
        assert_eq!(id, "test_tech");
    }

    #[test]
    fn test_define_xxx_reference_in_prerequisites() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            local base = define_tech { id = "base_tech", name = "Base", branch = "physics", cost = 100, prerequisites = {} }
            define_tech { id = "advanced_tech", name = "Adv", branch = "physics", cost = 200, prerequisites = { base } }
            "#,
        )
        .exec()
        .unwrap();

        let defs: mlua::Table = lua.globals().get("_tech_definitions").unwrap();
        assert_eq!(defs.len().unwrap(), 2);
        // The second tech's prerequisites should contain a reference table
        let second: mlua::Table = defs.get(2).unwrap();
        let prereqs: mlua::Table = second.get("prerequisites").unwrap();
        let first_prereq: mlua::Table = prereqs.get(1).unwrap();
        let prereq_id: String = first_prereq.get("id").unwrap();
        assert_eq!(prereq_id, "base_tech");
    }

    #[test]
    fn test_forward_ref() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let result: mlua::Table = lua
            .load(r#"return forward_ref("future_tech")"#)
            .eval()
            .unwrap();

        let def_type: String = result.get("_def_type").unwrap();
        assert_eq!(def_type, "forward_ref");
        let id: String = result.get("id").unwrap();
        assert_eq!(id, "future_tech");
    }

    #[test]
    fn test_has_tech_accepts_reference() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // String form (backward compatible)
        let cond_str: mlua::Table = lua.load(r#"return has_tech("my_tech")"#).eval().unwrap();
        assert_eq!(cond_str.get::<String>("id").unwrap(), "my_tech");

        // Reference form
        let cond_ref: mlua::Table = lua
            .load(
                r#"
                local t = define_tech { id = "ref_tech", name = "Ref" }
                return has_tech(t)
            "#,
            )
            .eval()
            .unwrap();
        assert_eq!(cond_ref.get::<String>("id").unwrap(), "ref_tech");
    }

    #[test]
    fn test_require_support() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // package.path should be set
        let package: mlua::Table = lua.globals().get("package").unwrap();
        let path: String = package.get("path").unwrap();
        assert!(path.contains("scripts/?.lua"));
        assert!(path.contains("scripts/?/init.lua"));

        // cpath should be empty
        let cpath: String = package.get("cpath").unwrap();
        assert!(cpath.is_empty());
    }

    /// #151: show_notification queues a Lua-side notification entry that the
    /// drain system can later pull into the NotificationQueue.
    #[test]
    fn test_show_notification_lua_queues_entry() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            show_notification {
                title = "Discovery",
                description = "Ancient ruins",
                priority = "high",
                icon = "anomaly",
            }
            show_notification {
                title = "Heads-up",
                description = "Just FYI",
            }
            "#,
        )
        .exec()
        .unwrap();

        let pending: mlua::Table = lua.globals().get("_pending_notifications").unwrap();
        assert_eq!(pending.len().unwrap(), 2);

        let first: mlua::Table = pending.get(1).unwrap();
        assert_eq!(first.get::<String>("title").unwrap(), "Discovery");
        assert_eq!(first.get::<String>("description").unwrap(), "Ancient ruins");
        assert_eq!(first.get::<String>("priority").unwrap(), "high");
        assert_eq!(first.get::<String>("icon").unwrap(), "anomaly");

        // Defaults: medium priority, no icon
        let second: mlua::Table = pending.get(2).unwrap();
        assert_eq!(second.get::<String>("priority").unwrap(), "medium");
        let icon: mlua::Value = second.get("icon").unwrap();
        assert!(matches!(icon, mlua::Value::Nil));
    }

    #[test]
    fn test_extract_ref_id() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // String value
        let s = mlua::Value::String(lua.create_string("hello").unwrap());
        assert_eq!(extract_ref_id(&s).unwrap(), "hello");

        // Table with id
        let t = lua.create_table().unwrap();
        t.set("id", "world").unwrap();
        let v = mlua::Value::Table(t);
        assert_eq!(extract_ref_id(&v).unwrap(), "world");

        // Number should fail
        let n = mlua::Value::Number(42.0);
        assert!(extract_ref_id(&n).is_err());
    }
}
