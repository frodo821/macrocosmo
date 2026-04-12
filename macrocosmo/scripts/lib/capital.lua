-- scripts/lib/capital.lua
--
-- Default capital initialization helper for `define_faction { on_game_start }`.
--
-- Provides a single function, `initialize_default_capital(ctx, opts)`, that
-- records the canonical "habitable home + a couple of secondary planets +
-- starter buildings + starter ships" layout into the GameStartCtx.
--
-- Factions that want a fully bespoke layout (e.g. `humanity_empire` with
-- Sol/Earth/Mars/Jupiter) should NOT use this function and instead drive the
-- ctx directly. This helper is intended as an easy default for new / generic
-- factions and as a basis for procedurally generated ones.

local M = {}

-- Planet types defined in scripts/planets/types.lua. Keep weights in sync
-- when adding new ones. (The issue spec uses theoretical names like "terran"
-- / "desert" / "ice"; we map onto the actually-defined types here.)
local DEFAULT_ADDITIONAL_PLANET_WEIGHTS = {
    { weight = 10, value = "terrestrial" },
    { weight = 35, value = "barren" },
    { weight = 25, value = "gas_giant" },
    { weight = 20, value = "arid" },     -- "ice"-equivalent slot in available types
    { weight = 10, value = "ocean" },    -- "desert"-equivalent slot in available types
}

-- Reasonable default starting layout
local DEFAULT_HOME_PLANET_TYPE     = "terrestrial"
local DEFAULT_STARTER_BUILDINGS    = { "mine", "power_plant", "farm" }
local DEFAULT_SYSTEM_BUILDINGS     = { "shipyard" }
local DEFAULT_STARTER_SHIPS        = { { "explorer_mk1", "Explorer I" } }

-- Internal helper: pick a random habitability roll appropriate for a given
-- planet type. Ranges chosen so additional planets feel varied but never
-- overshadow the capital home planet's ~0.85-1.0 habitability.
local function random_habitability_for_type(type_id)
    if type_id == "terrestrial" then
        return game_rand.range(0.4, 0.7)
    elseif type_id == "ocean" then
        return game_rand.range(0.3, 0.6)
    elseif type_id == "arid" then
        return game_rand.range(0.2, 0.5)
    elseif type_id == "barren" then
        return game_rand.range(0.0, 0.2)
    else
        -- gas_giant and any future type default to uninhabitable.
        return 0.0
    end
end

-- Build a randomised attribute table for an additional (non-capital) planet.
local function random_additional_attrs(type_id)
    return {
        habitability       = random_habitability_for_type(type_id),
        mineral_richness   = game_rand.range(0.1, 0.9),
        energy_potential   = game_rand.range(0.1, 0.9),
        research_potential = game_rand.range(0.0, 0.3),
    }
end

-- Build a randomised attribute table for the capital home planet. Values are
-- biased high so the player always gets a survivable starting world.
local function random_home_attrs()
    return {
        habitability       = game_rand.range(0.85, 1.0),
        mineral_richness   = game_rand.range(0.5, 0.8),
        energy_potential   = game_rand.range(0.5, 0.8),
        research_potential = game_rand.range(0.5, 0.8),
        max_building_slots = game_rand.range_int(5, 6),
    }
end

-- Determine the count and per-planet specs for additional planets.
-- `additional_planets` may be:
--   * nil               -> 2-4 random planets
--   * a number          -> that many random planets
--   * an array of specs -> use as-is (each spec: { name=, type=, attrs= })
local function resolve_additional_planets(additional_planets, faction_name)
    if additional_planets == nil then
        local count = game_rand.range_int(2, 4)
        local specs = {}
        for i = 1, count do
            local type_id = game_rand.weighted(DEFAULT_ADDITIONAL_PLANET_WEIGHTS)
            specs[i] = {
                name  = faction_name .. " " .. tostring(i + 1),
                type  = type_id,
                attrs = random_additional_attrs(type_id),
            }
        end
        return specs
    end

    if type(additional_planets) == "number" then
        local specs = {}
        for i = 1, additional_planets do
            local type_id = game_rand.weighted(DEFAULT_ADDITIONAL_PLANET_WEIGHTS)
            specs[i] = {
                name  = faction_name .. " " .. tostring(i + 1),
                type  = type_id,
                attrs = random_additional_attrs(type_id),
            }
        end
        return specs
    end

    if type(additional_planets) == "table" then
        -- Caller supplied explicit per-planet specs. We still fill in any
        -- missing attrs randomly so callers can override partially.
        local specs = {}
        for i, raw in ipairs(additional_planets) do
            local type_id = raw.type or game_rand.weighted(DEFAULT_ADDITIONAL_PLANET_WEIGHTS)
            specs[i] = {
                name  = raw.name  or (faction_name .. " " .. tostring(i + 1)),
                type  = type_id,
                attrs = raw.attrs or random_additional_attrs(type_id),
            }
        end
        return specs
    end

    error("initialize_default_capital: opts.additional_planets must be nil, number, or table")
end

-- The public entrypoint. See module header comment for the option schema.
function M.initialize_default_capital(ctx, opts)
    opts = opts or {}

    local faction_name = ctx.faction or "Empire"
    local home_name    = opts.home_planet_name or (faction_name .. " Prime")
    local home_type    = opts.home_planet_type or DEFAULT_HOME_PLANET_TYPE
    local home_attrs   = opts.home_planet_attrs or random_home_attrs()

    local starter_buildings        = opts.starter_buildings        or DEFAULT_STARTER_BUILDINGS
    local starter_system_buildings = opts.starter_system_buildings or DEFAULT_SYSTEM_BUILDINGS
    local starter_ships            = opts.starter_ships            or DEFAULT_STARTER_SHIPS

    -- 1) Mark the system as the capital and surveyed.
    ctx.system:set_capital(true)
    ctx.system:set_surveyed(true)

    -- 2) Wipe the procedurally-generated planets so we start from a known state.
    ctx.system:clear_planets()

    -- 3) Spawn the home planet + colonize + add starter buildings.
    local home = ctx.system:spawn_planet(home_name, home_type, home_attrs)
    home:colonize(ctx.faction)
    for _, building in ipairs(starter_buildings) do
        home:add_building(building)
    end

    -- 4) Spawn additional planets.
    local additional_specs = resolve_additional_planets(opts.additional_planets, faction_name)
    for _, spec in ipairs(additional_specs) do
        ctx.system:spawn_planet(spec.name, spec.type, spec.attrs)
    end

    -- 5) Add system-level starter buildings.
    for _, building in ipairs(starter_system_buildings) do
        ctx.system:add_building(building)
    end

    -- 6) Spawn starter ships.
    for _, entry in ipairs(starter_ships) do
        local design = entry[1]
        local name   = entry[2] or (faction_name .. " Ship")
        ctx.system:spawn_ship(design, name)
    end
end

-- Expose the function as a global so faction scripts can call it directly
-- without re-requiring the module.
_G.initialize_default_capital = M.initialize_default_capital

return M
