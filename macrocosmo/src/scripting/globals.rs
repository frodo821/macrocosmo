use mlua::prelude::*;
use std::path::Path;

use super::effect_scope;
use super::helpers::extract_id_from_lua_value;

/// Configure global tables and functions available to all Lua scripts.
pub fn setup_globals(lua: &Lua, scripts_dir: &Path) -> Result<(), mlua::Error> {
    let globals = lua.globals();

    // --- Sandbox: disable dangerous globals ---
    globals.set("loadfile", mlua::Value::Nil)?;
    globals.set("dofile", mlua::Value::Nil)?;

    // --- Set up require() search path using resolved absolute path ---
    let package: mlua::Table = globals.get("package")?;
    let dir = scripts_dir.display();
    package.set("path", format!("{dir}/?.lua;{dir}/?/init.lua"))?;
    package.set("cpath", "")?; // disable C module loading

    // Create the macrocosmo namespace table
    let mc = lua.create_table()?;
    globals.set("macrocosmo", mc)?;

    // forward_ref(id) -- creates a placeholder reference for not-yet-defined items
    let forward_ref = lua.create_function(|lua, id: String| {
        let t = lua.create_table()?;
        t.set("_def_type", "forward_ref")?;
        t.set("id", id)?;
        Ok(t)
    })?;
    globals.set("forward_ref", forward_ref)?;

    // --- Define accumulator tables and define_xxx functions ---
    // Each define_xxx appends to its accumulator AND returns the table
    // with a _def_type tag, enabling return-value based references.

    register_define_fn(lua, "tech_branch", "_tech_branch_definitions")?;
    register_define_fn(lua, "tech", "_tech_definitions")?;
    register_define_fn(lua, "building", "_building_definitions")?;
    register_define_fn(lua, "star_type", "_star_type_definitions")?;
    register_define_fn(lua, "planet_type", "_planet_type_definitions")?;
    // #335: Biome definitions (decoupled from planet_type so multiple
    // planet_types can share a biome and future features gate on biome id).
    register_define_fn(lua, "biome", "_biome_definitions")?;

    // --- #182: Predefined systems + map types ---
    register_define_fn(lua, "predefined_system", "_predefined_system_definitions")?;
    register_define_fn(lua, "map_type", "_map_type_definitions")?;

    // --- #145: Forbidden regions (nebulae, subspace storms) ---
    //
    // `define_region_type { id, name, capabilities, visual }` — declares a
    // placeable region type. Placement specs are added separately via
    // `galaxy_generation.add_region_spec { type=..., count_range=...}`
    // (registered further down alongside other generator helpers).
    register_define_fn(lua, "region_type", "_region_type_definitions")?;

    // Shared `_pending_region_specs` table, populated by
    // `galaxy_generation.add_region_spec`. Accumulates across script runs
    // and is drained at galaxy-generation time.
    globals.set("_pending_region_specs", lua.create_table()?)?;

    // `galaxy_generation` helper namespace. We create it if absent and attach
    // `add_region_spec` onto it — if other #145/#182 style helpers want to
    // share the namespace later, they can extend it without re-creating.
    let galaxy_generation: mlua::Table = match globals.get::<mlua::Value>("galaxy_generation")? {
        mlua::Value::Table(t) => t,
        _ => {
            let t = lua.create_table()?;
            globals.set("galaxy_generation", t.clone())?;
            t
        }
    };
    let add_region_spec = lua.create_function(|lua, table: mlua::Table| {
        let pending: mlua::Table = lua.globals().get("_pending_region_specs")?;
        let len = pending.len()?;
        pending.set(len + 1, table)?;
        Ok(())
    })?;
    galaxy_generation.set("add_region_spec", add_region_spec)?;

    // set_active_map_type(id_or_ref) — selects which map_type the engine uses
    // when generate_galaxy runs. Accepts a string id or a `define_map_type`
    // reference table. Writes to the global `_active_map_type`, consumed by
    // `MapTypeRegistry` at the Rust side.
    let set_active_map_type = lua.create_function(|lua, value: mlua::Value| {
        let id = extract_id_from_lua_value(&value)?;
        lua.globals().set("_active_map_type", id)?;
        Ok(())
    })?;
    globals.set("set_active_map_type", set_active_map_type)?;

    // --- Species and job definition Lua bindings ---

    register_define_fn(lua, "species", "_species_definitions")?;
    register_define_fn(lua, "job", "_job_definitions")?;

    // --- Event definition ---

    register_define_fn(lua, "event", "_event_definitions")?;

    // --- #350: Knowledge kind definition (Lua-extensible knowledge kinds) ---
    //
    // `define_knowledge { id, payload_schema }` appends to
    // `_knowledge_kind_definitions`, which is parsed by
    // `scripting::knowledge_api::parse_knowledge_definitions` at startup
    // (see `load_knowledge_kinds` system). The accumulator name must match
    // `knowledge_api::KNOWLEDGE_DEF_ACCUMULATOR`.
    register_define_fn(lua, "knowledge", "_knowledge_kind_definitions")?;

    // --- Ship design Lua bindings ---

    register_define_fn(lua, "slot_type", "_slot_type_definitions")?;
    register_define_fn(lua, "hull", "_hull_definitions")?;
    register_define_fn(lua, "module", "_module_definitions")?;
    register_define_fn(lua, "ship_design", "_ship_design_definitions")?;

    // --- Structure & Deliverable definition ---
    //
    // #223: `define_structure` is the world-side entry (not shipyard-buildable).
    // `define_deliverable` is the shipyard-buildable superset — adds `cost`,
    // `build_time`, `cargo_size`, `scrap_refund`, `upgrade_to`, `upgrade_from`.
    // Both feed `parse_structure_definitions`, which dispatches on which
    // accumulator they came from.
    register_define_fn(lua, "structure", "_structure_definitions")?;
    register_define_fn(lua, "deliverable", "_deliverable_definitions")?;

    // --- Anomaly definition ---

    register_define_fn(lua, "anomaly", "_anomaly_definitions")?;

    // --- Faction definition ---

    register_define_fn(lua, "faction", "_faction_definitions")?;
    register_define_fn(lua, "faction_type", "_faction_type_definitions")?;
    register_define_fn(lua, "diplomatic_action", "_diplomatic_action_definitions")?;
    register_define_fn(lua, "diplomatic_option", "_diplomatic_option_definitions")?;

    // --- #305 S-11: Casus Belli definition ---
    register_define_fn(lua, "casus_belli", "_casus_belli_definitions")?;

    // --- #321: Negotiation item kind definition ---
    register_define_fn(
        lua,
        "negotiation_item_kind",
        super::negotiation_api::ACCUMULATOR,
    )?;

    // --- #160: Balance constants Lua binding ---
    // `define_balance { ... }` is expected to be called AT MOST ONCE from
    // `scripts/config/balance.lua`. Subsequent calls overwrite the stored
    // table with a warning logged on the Rust side at parse time
    // (last-wins). Stores the raw Lua table under the global
    // `_balance_definition` for `load_game_balance` to pick up.
    globals.set("_balance_definition", mlua::Value::Nil)?;
    let define_balance = lua.create_function(|lua, table: mlua::Table| {
        let existing: mlua::Value = lua.globals().get("_balance_definition")?;
        if !matches!(existing, mlua::Value::Nil) {
            // Lua-side warning via print; Rust will log on parse.
            let _ = lua
                .load("print('[warn] define_balance called more than once; last-wins')")
                .exec();
        }
        table.set("_def_type", "balance")?;
        lua.globals().set("_balance_definition", table.clone())?;
        Ok(table)
    })?;
    globals.set("define_balance", define_balance)?;

    // --- #45: Global param / flag Lua bindings ---
    //
    // #332-B4: retired the `set_flag(name)` / `modify_global(param, v)`
    // global helpers and their backing `_pending_flags` /
    // `_pending_global_mods` queues. Event / lifecycle callbacks mutate
    // world state through the `gs:*` setter surface
    // (`gs:set_flag(scope_kind, id, name, value)` /
    // `gs:push_empire_modifier(...)`); tech and faction callbacks use
    // `EffectScope` descriptors. The `check_flag` global is retained
    // as a convenience read-path for ad-hoc scripts, but its backing
    // `_flag_store` table is kept as a stub for forward-compat; it is
    // populated exclusively by this local `check_flag` path now, not
    // by the removed `set_flag` helper.

    let flag_store = lua.create_table()?;
    globals.set("_flag_store", flag_store)?;

    // check_flag(name) -- returns true if the flag was stored in
    // `_flag_store` (e.g. via a test fixture priming it directly).
    let check_flag = lua.create_function(|lua, name: String| {
        let store: mlua::Table = lua.globals().get("_flag_store")?;
        let result: bool = store.get::<Option<bool>>(name)?.unwrap_or(false);
        Ok(result)
    })?;
    globals.set("check_flag", check_flag)?;

    // --- EventBus handler registration ---

    // Handler table for on() registrations
    let event_handlers = lua.create_table()?;
    globals.set("_event_handlers", event_handlers)?;

    // #350 K-1: Knowledge reserved events table (auto-registered lifecycle
    // event ids from define_knowledge). K-3 on() router checks this.
    globals.set("_knowledge_subscribers", lua.create_table()?)?;
    globals.set("_knowledge_reserved_events", lua.create_table()?)?;

    // #352 K-3: Knowledge subscription accumulator. on(event_id, fn) routes
    // knowledge-lifecycle event ids here; load_knowledge_subscriptions
    // drains into bucketed KnowledgeSubscriptionRegistry at startup.
    let knowledge_subscriptions = lua.create_table()?;
    globals.set(
        super::knowledge_registry::PENDING_KNOWLEDGE_SUBSCRIPTIONS,
        knowledge_subscriptions,
    )?;

    // on(event_id, [filter,] handler) -- registers an event handler with optional structural filter.
    //
    // #352 (K-3): event ids matching the knowledge pattern
    // (`<kind>@recorded` / `<kind>@observed` / `*@recorded` / `*@observed`)
    // are routed to the `_pending_knowledge_subscriptions` accumulator
    // instead of `_event_handlers`. The Rust-side
    // `load_knowledge_subscriptions` startup system drains that
    // accumulator into the bucketed `KnowledgeSubscriptionRegistry`
    // resource. Knowledge subscriptions do not accept structural filter
    // tables — filter is a bus-dispatch concept; knowledge payload
    // filtering happens inside the subscriber function itself.
    //
    // Event ids shaped like `foo@bar` where `bar` is not a recognised
    // knowledge lifecycle (i.e. anything other than `recorded` /
    // `observed`) are rejected at registration time with a clear error
    // (plan-349 §0.5 9.2 — load-time hygiene).
    let on_fn = lua.create_function(|lua, args: mlua::MultiValue| {
        let mut args_iter = args.into_iter();
        // First arg: event_id string
        let event_id: String = match args_iter.next() {
            Some(mlua::Value::String(s)) => s.to_str()?.to_string(),
            _ => {
                return Err(mlua::Error::RuntimeError(
                    "on() requires event_id string as first argument".into(),
                ));
            }
        };

        // Second arg: either a filter table or a handler function
        let second = args_iter.next().ok_or_else(|| {
            mlua::Error::RuntimeError(
                "on() requires handler function (or filter table + handler function)".into(),
            )
        })?;

        // Early classification so we can reject filters on knowledge ids
        // and surface unknown-lifecycle errors before we bother allocating
        // the entry table.
        if event_id.contains('@') {
            // Any id containing '@' is treated as knowledge-intent: either
            // valid knowledge lifecycle or explicit error. This prevents
            // a typo like `foo@observe` from silently entering
            // `_event_handlers`.
            if let Err(e) = super::knowledge_dispatch::parse_knowledge_event_id(&event_id) {
                return Err(mlua::Error::RuntimeError(format!(
                    "on(): {e}"
                )));
            }
            // Knowledge ids do not accept filter tables.
            if let mlua::Value::Table(_) = &second {
                return Err(mlua::Error::RuntimeError(format!(
                    "on(): knowledge event id '{event_id}' does not accept a filter table (filtering is a bus-dispatch feature; knowledge subscribers should filter in the callback body)"
                )));
            }
        }

        let is_knowledge = super::knowledge_dispatch::is_knowledge_event_id(&event_id);

        let entry = lua.create_table()?;
        entry.set("event_id", event_id.clone())?;

        match second {
            mlua::Value::Function(func) => {
                entry.set("func", func)?;
            }
            mlua::Value::Table(filter) => {
                entry.set("filter", filter)?;
                let func = match args_iter.next() {
                    Some(mlua::Value::Function(f)) => f,
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "on() with filter requires handler function as 3rd argument"
                                .into(),
                        ));
                    }
                };
                entry.set("func", func)?;
            }
            _ => {
                return Err(mlua::Error::RuntimeError(
                    "on() 2nd argument must be a filter table or handler function".into(),
                ));
            }
        }

        let target_table_name = if is_knowledge {
            super::knowledge_registry::PENDING_KNOWLEDGE_SUBSCRIPTIONS
        } else {
            "_event_handlers"
        };
        let target: mlua::Table = lua.globals().get(target_table_name)?;
        let len = target.len()?;
        target.set(len + 1, entry)?;
        Ok(())
    })?;
    globals.set("on", on_fn)?;

    // --- Condition helper functions ---
    // These return Lua tables that represent condition nodes, parsed by condition_parser.

    // has_tech / has_modifier / has_building accept either a string ID
    // or a reference table (returned by define_xxx) from which the id is extracted.
    let has_tech = lua.create_function(|lua, value: mlua::Value| {
        let t = lua.create_table()?;
        t.set("type", "has_tech")?;
        t.set("id", extract_id_from_lua_value(&value)?)?;
        Ok(t)
    })?;
    globals.set("has_tech", has_tech)?;

    let has_modifier = lua.create_function(|lua, value: mlua::Value| {
        let t = lua.create_table()?;
        t.set("type", "has_modifier")?;
        t.set("id", extract_id_from_lua_value(&value)?)?;
        Ok(t)
    })?;
    globals.set("has_modifier", has_modifier)?;

    let has_building = lua.create_function(|lua, value: mlua::Value| {
        let t = lua.create_table()?;
        t.set("type", "has_building")?;
        t.set("id", extract_id_from_lua_value(&value)?)?;
        Ok(t)
    })?;
    globals.set("has_building", has_building)?;

    let has_flag = lua.create_function(|lua, value: mlua::Value| {
        let t = lua.create_table()?;
        t.set("type", "has_flag")?;
        t.set("id", extract_id_from_lua_value(&value)?)?;
        Ok(t)
    })?;
    globals.set("has_flag", has_flag)?;

    let all_fn = lua.create_function(|lua, args: mlua::MultiValue| {
        let t = lua.create_table()?;
        t.set("type", "all")?;
        let children = lua.create_table()?;
        for (i, arg) in args.into_iter().enumerate() {
            children.set(i + 1, arg)?;
        }
        t.set("children", children)?;
        Ok(t)
    })?;
    globals.set("all", all_fn)?;

    let any_fn = lua.create_function(|lua, args: mlua::MultiValue| {
        let t = lua.create_table()?;
        t.set("type", "any")?;
        let children = lua.create_table()?;
        for (i, arg) in args.into_iter().enumerate() {
            children.set(i + 1, arg)?;
        }
        t.set("children", children)?;
        Ok(t)
    })?;
    globals.set("any", any_fn)?;

    let one_of_fn = lua.create_function(|lua, args: mlua::MultiValue| {
        let t = lua.create_table()?;
        t.set("type", "one_of")?;
        let children = lua.create_table()?;
        for (i, arg) in args.into_iter().enumerate() {
            children.set(i + 1, arg)?;
        }
        t.set("children", children)?;
        Ok(t)
    })?;
    globals.set("one_of", one_of_fn)?;

    // "not" is a Lua keyword, so we use "not_cond" as the function name.
    let not_cond_fn = lua.create_function(|lua, child: mlua::Table| {
        let t = lua.create_table()?;
        t.set("type", "not")?;
        t.set("child", child)?;
        Ok(t)
    })?;
    globals.set("not_cond", not_cond_fn)?;

    // mtth_trigger(params) -- constructor that tags a table as type "mtth"
    let mtth_trigger = lua.create_function(|_, table: mlua::Table| {
        table.set("_type", "mtth")?;
        Ok(table)
    })?;
    globals.set("mtth_trigger", mtth_trigger)?;

    // periodic_trigger(params) -- constructor that tags a table as type "periodic"
    let periodic_trigger = lua.create_function(|_, table: mlua::Table| {
        table.set("_type", "periodic")?;
        Ok(table)
    })?;
    globals.set("periodic_trigger", periodic_trigger)?;

    // Pending script-fired events table
    let pending_script_events = lua.create_table()?;
    globals.set("_pending_script_events", pending_script_events)?;

    // --- #151: Notification banner API ---

    // Pending notifications table — drained by `drain_pending_notifications`
    let pending_notifications = lua.create_table()?;
    globals.set("_pending_notifications", pending_notifications)?;

    // show_notification { title, description, icon?, priority?, target_system? }
    // priority defaults to "medium". target_system can be a star system entity
    // bits value (number). The notification is enqueued until the next frame's
    // drain system applies it.
    let show_notification_fn = lua.create_function(|lua, params: mlua::Table| {
        let pending: mlua::Table = lua.globals().get("_pending_notifications")?;
        let len = pending.len()?;

        let entry = lua.create_table()?;
        let title: String = params.get("title").unwrap_or_default();
        let description: String = params.get("description").unwrap_or_default();
        entry.set("title", title)?;
        entry.set("description", description)?;
        if let Ok(icon) = params.get::<String>("icon") {
            entry.set("icon", icon)?;
        }
        let priority: String = params
            .get::<String>("priority")
            .unwrap_or_else(|_| "medium".to_string());
        entry.set("priority", priority)?;
        // target_system can be a numeric Entity::to_bits value if scripts
        // ever expose them. Numeric only — reference tables to actual entities
        // are not yet stable from the script side.
        if let Ok(target) = params.get::<u64>("target_system") {
            entry.set("target_system", target)?;
        }

        pending.set(len + 1, entry)?;
        Ok(())
    })?;
    globals.set("show_notification", show_notification_fn)?;

    // --- #345 ESC-2: ESC (Empire Situation Center) notification push API ---
    //
    // `push_notification { title, message, severity, source, event_id,
    //                      timestamp, children? }` enqueues a *post-hoc*
    // ack-able notification into `EscNotificationQueue` (see
    // `crate::ui::situation_center::notifications_tab`).
    //
    // Distinct from `show_notification` — that API drives the top-banner
    // stack (#151), which is a live TTL-based popup. `push_notification`
    // targets the ESC Notifications tab: history + ack, not banners.
    //
    // Shape:
    //   - title / message: strings (at least one should be non-empty;
    //     the renderer uses `message` as the body, falling back to
    //     `title` if `message` is absent).
    //   - severity: "info" | "warn" | "critical" (default "info").
    //   - source: optional table `{ kind = ..., id = <u64 Entity bits> }`
    //     where `kind` is one of "none" | "empire" | "system" | "colony" |
    //     "ship" | "fleet" | "faction" | "build_order". Unknown kinds
    //     fall back to "none". Missing `source` is equivalent to `none`.
    //   - event_id: optional string OR numeric. When supplied it is
    //     routed through `#249 NotifiedEventIds::try_notify` by the Rust
    //     drain so duplicate pushes for the same id are silently
    //     suppressed (same mechanism the banner queue uses).
    //   - timestamp: optional i64 game-hexadies. When absent the drain
    //     reads the current `GameClock`.
    //   - children: optional array of tables shaped like the outer push
    //     (recursive; depth capped by Rust-side parser).
    //
    // The entry is appended to the global `_pending_esc_notifications`
    // Lua table; the Rust-side `drain_pending_esc_notifications` system
    // drains it every frame and applies the push to
    // `EscNotificationQueue`.
    globals.set("_pending_esc_notifications", lua.create_table()?)?;
    let push_notification_fn = lua.create_function(|lua, params: mlua::Table| {
        let pending: mlua::Table = lua.globals().get("_pending_esc_notifications")?;
        let len = pending.len()?;
        // We deliberately store the raw Lua table — the Rust side knows
        // the shape and can surface clear errors for each malformed
        // field instead of having Lua's `.get::<T>(...)` coerce silently.
        pending.set(len + 1, params)?;
        Ok(())
    })?;
    globals.set("push_notification", push_notification_fn)?;

    // --- #152: Player choice dialog API ---

    // Pending choice queue — drained by `drain_pending_choices`
    let pending_choices = lua.create_table()?;
    globals.set("_pending_choices", pending_choices)?;

    // show_choice { title, description, icon?, target_system?, options = { ... } }
    // Each option may carry { label, description, condition, cost, on_chosen }.
    // The call pushes the whole table (including `on_chosen` functions) onto
    // `_pending_choices`; the Rust side later drains it and stashes the table
    // under `_active_choices[id]` so `on_chosen` can be invoked at apply time.
    // Returns a reference table `{ _def_type = "choice", id = "..." }` to stay
    // consistent with other `define_xxx` style calls, where `id` is derived
    // from the title when not supplied (plus a monotonic counter for
    // uniqueness).
    let show_choice_fn = lua.create_function(|lua, params: mlua::Table| {
        let pending: mlua::Table = lua.globals().get("_pending_choices")?;
        let len = pending.len()?;

        // Derive a stable-ish id: prefer explicit `id`, else title slug, else
        // "choice". Always append a monotonically increasing counter to make
        // it unique even if the same title is shown repeatedly.
        let next_counter: u64 = lua.globals().get("_show_choice_counter").unwrap_or(0_u64);
        lua.globals()
            .set("_show_choice_counter", next_counter + 1)?;

        let id: String = if let Ok(explicit) = params.get::<String>("id") {
            explicit
        } else if let Ok(title) = params.get::<String>("title") {
            let slug: String = title
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '_'
                    }
                })
                .collect();
            format!("{slug}_{next_counter}")
        } else {
            format!("choice_{next_counter}")
        };

        pending.set(len + 1, params)?;

        // Return a reference-style table. This is *not* stashed in any
        // accumulator — choices are one-shot, not persistent definitions.
        let out = lua.create_table()?;
        out.set("_def_type", "choice")?;
        out.set("id", id)?;
        Ok(out)
    })?;
    globals.set("show_choice", show_choice_fn)?;

    // --- Lifecycle hook registration ---

    // Handler tables for lifecycle hooks
    globals.set("_on_game_start_handlers", lua.create_table()?)?;
    globals.set("_on_game_load_handlers", lua.create_table()?)?;
    globals.set("_on_scripts_loaded_handlers", lua.create_table()?)?;

    // on_game_start(fn) -- registers a callback to run when a new game starts
    let on_game_start = lua.create_function(|lua, func: mlua::Function| {
        let handlers: mlua::Table = lua.globals().get("_on_game_start_handlers")?;
        let len = handlers.len()?;
        handlers.set(len + 1, func)?;
        Ok(())
    })?;
    globals.set("on_game_start", on_game_start)?;

    // on_game_load(fn) -- registers a callback to run when a saved game is loaded
    let on_game_load = lua.create_function(|lua, func: mlua::Function| {
        let handlers: mlua::Table = lua.globals().get("_on_game_load_handlers")?;
        let len = handlers.len()?;
        handlers.set(len + 1, func)?;
        Ok(())
    })?;
    globals.set("on_game_load", on_game_load)?;

    // on_scripts_loaded(fn) -- registers a callback to run after all scripts have been loaded
    let on_scripts_loaded = lua.create_function(|lua, func: mlua::Function| {
        let handlers: mlua::Table = lua.globals().get("_on_scripts_loaded_handlers")?;
        let len = handlers.len()?;
        handlers.set(len + 1, func)?;
        Ok(())
    })?;
    globals.set("on_scripts_loaded", on_scripts_loaded)?;

    // --- #181: Galaxy generation hook registration ----------------------
    //
    // Each hook replaces the corresponding default phase in `generate_galaxy`.
    // If multiple callbacks are registered for the same hook, the LAST one
    // wins (replacement semantics, not composition). If no hook is registered,
    // the built-in Rust default runs.
    globals.set(
        super::galaxy_gen_ctx::GENERATE_EMPTY_HANDLERS,
        lua.create_table()?,
    )?;
    globals.set(
        super::galaxy_gen_ctx::CHOOSE_CAPITALS_HANDLERS,
        lua.create_table()?,
    )?;
    globals.set(
        super::galaxy_gen_ctx::INITIALIZE_SYSTEM_HANDLERS,
        lua.create_table()?,
    )?;
    globals.set(
        super::galaxy_gen_ctx::AFTER_PHASE_A_HANDLERS,
        lua.create_table()?,
    )?;

    let on_galaxy_generate_empty = lua.create_function(|lua, func: mlua::Function| {
        let handlers: mlua::Table = lua
            .globals()
            .get(super::galaxy_gen_ctx::GENERATE_EMPTY_HANDLERS)?;
        let len = handlers.len()?;
        handlers.set(len + 1, func)?;
        Ok(())
    })?;
    globals.set("on_galaxy_generate_empty", on_galaxy_generate_empty)?;

    let on_choose_capitals = lua.create_function(|lua, func: mlua::Function| {
        let handlers: mlua::Table = lua
            .globals()
            .get(super::galaxy_gen_ctx::CHOOSE_CAPITALS_HANDLERS)?;
        let len = handlers.len()?;
        handlers.set(len + 1, func)?;
        Ok(())
    })?;
    globals.set("on_choose_capitals", on_choose_capitals)?;

    let on_initialize_system = lua.create_function(|lua, func: mlua::Function| {
        let handlers: mlua::Table = lua
            .globals()
            .get(super::galaxy_gen_ctx::INITIALIZE_SYSTEM_HANDLERS)?;
        let len = handlers.len()?;
        handlers.set(len + 1, func)?;
        Ok(())
    })?;
    globals.set("on_initialize_system", on_initialize_system)?;

    // #199: on_after_phase_a(ctx) — runs after Phase A completes regardless
    // of which generator populated systems. Used for Lua-driven connectivity
    // enforcement (e.g. FTL-reachability bridge insertion).
    let on_after_phase_a = lua.create_function(|lua, func: mlua::Function| {
        let handlers: mlua::Table = lua
            .globals()
            .get(super::galaxy_gen_ctx::AFTER_PHASE_A_HANDLERS)?;
        let len = handlers.len()?;
        handlers.set(len + 1, func)?;
        Ok(())
    })?;
    globals.set("on_after_phase_a", on_after_phase_a)?;

    // fire_event(event_id, target?) -- queues an event to be fired from Lua
    let fire_event_fn = lua.create_function(|lua, args: (String, Option<u64>)| {
        let events: mlua::Table = lua.globals().get("_pending_script_events")?;
        let len = events.len()?;
        let entry = lua.create_table()?;
        entry.set("event_id", args.0)?;
        if let Some(target) = args.1 {
            entry.set("target", target)?;
        }
        events.set(len + 1, entry)?;
        Ok(())
    })?;
    globals.set("fire_event", fire_event_fn)?;

    // --- Effect descriptor helpers ---
    // effect_fire_event(event_id, payload?) -- returns a descriptor table (does NOT queue the event)
    let effect_fire_event_fn = effect_scope::create_fire_event_descriptor(lua)?;
    globals.set("effect_fire_event", effect_fire_event_fn)?;

    // hide(label, inner_descriptor) -- wraps a descriptor with a display label
    let hide_fn = effect_scope::create_hide_function(lua)?;
    globals.set("hide", hide_fn)?;

    Ok(())
}

/// Register a `define_xxx` global function that:
/// 1. Creates an accumulator table `_xxx_definitions`
/// 2. Registers `define_xxx(table)` which appends to the accumulator and
///    tags the table with `_def_type = def_type`, then returns it as a reference.
fn register_define_fn(
    lua: &Lua,
    def_type: &str,
    accumulator_name: &str,
) -> Result<(), mlua::Error> {
    let globals = lua.globals();

    let acc = lua.create_table()?;
    globals.set(accumulator_name, acc)?;

    let acc_name = accumulator_name.to_string();
    let dtype = def_type.to_string();
    let func = lua.create_function(move |lua, table: mlua::Table| {
        table.set("_def_type", dtype.as_str())?;
        let defs: mlua::Table = lua.globals().get(acc_name.as_str())?;
        let len = defs.len()?;
        defs.set(len + 1, table.clone())?;
        Ok(table)
    })?;
    globals.set(format!("define_{def_type}"), func)?;

    Ok(())
}
