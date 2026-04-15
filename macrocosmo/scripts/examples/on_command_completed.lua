-- #334 Phase 4 — example Lua hook for `on("macrocosmo:command_completed")`
--
-- NOT LOADED BY `init.lua`. This file documents the canonical shape of
-- the `request_command` setter and `on_command_completed` observer, and
-- is referenced from `docs/architecture-decisions.md` §10bis. Copy into
-- a real `scripts/events/` module (or a mod) to wire it into the game.
--
-- Invariant: `evt.gamestate:request_command(kind, args)` is ONLY
-- available in ReadWrite contexts (event callbacks, lifecycle hooks).
-- Fire-condition callbacks use `GamestateMode::ReadOnly` and will see
-- `nil` here — see `memory/feedback_rust_no_lua_callback.md`.

-- ---------------------------------------------------------------------
-- Supported kinds (Phase 4)
-- ---------------------------------------------------------------------
-- "move"                  -- { ship = u64, target = u64 }
-- "move_to_coordinates"   -- { ship = u64, target = {x, y, z} }
-- "scout"                 -- { ship = u64, target_system = u64,
--                             observation_duration? = i64,
--                             report_mode? = "return" | "ftl_comm" }
-- "load_deliverable"      -- { ship = u64, system = u64, stockpile_index = int }
-- "deploy_deliverable"    -- { ship = u64, position = {x, y, z}, item_index = int }
-- "transfer_to_structure" -- { ship = u64, structure = u64,
--                             minerals = number, energy = number }
-- "load_from_scrapyard"   -- { ship = u64, structure = u64 }
-- "colonize"              -- { ship = u64, target_system = u64, planet? = u64 }
-- "survey"                -- { ship = u64, target_system = u64 }

-- ---------------------------------------------------------------------
-- Example: auto-reissue a rejected Survey as a Scout
-- ---------------------------------------------------------------------

on("macrocosmo:command_completed", function(evt)
    -- All fields arrive as strings (consistent with LuaDefinedEventContext);
    -- coerce to numbers before comparing / re-feeding into gamestate.
    if evt.kind ~= "survey" or evt.result ~= "rejected" then
        return
    end

    local ship         = tonumber(evt.ship)
    local reason       = evt.reason or "unknown"
    -- The surveyed system is not carried by the completed event in
    -- Phase 4 (only ship / kind / result are generic). A real hook
    -- would persist the target somewhere (e.g. a per-ship
    -- `_pending_survey_targets` table keyed by command_id) at
    -- request_command time and look it up here. Sketch only:
    local target_system = _pending_survey_targets and _pending_survey_targets[evt.command_id]
    if not target_system then
        return
    end

    log("survey rejected (" .. reason .. "), retrying as scout for ship " .. ship)
    local new_id = evt.gamestate:request_command("scout", {
        ship                 = ship,
        target_system        = target_system,
        observation_duration = 5,
        report_mode          = "ftl_comm",
    })
    log("scout queued as command id " .. new_id)
end)

-- ---------------------------------------------------------------------
-- Example: track outstanding commands in a Lua table
-- ---------------------------------------------------------------------

_pending_cmds = _pending_cmds or {}

-- Helper wrapper that remembers the kind + command_id mapping locally.
function queue_command(gs, kind, args)
    local id = gs:request_command(kind, args)
    _pending_cmds[tostring(id)] = { kind = kind, args = args }
    return id
end

on("macrocosmo:command_completed", function(evt)
    local key = evt.command_id
    local tracked = _pending_cmds[key]
    if tracked then
        _pending_cmds[key] = nil
        log("finished tracked " .. tracked.kind .. " id " .. key .. " with " .. evt.result)
    end
end)
