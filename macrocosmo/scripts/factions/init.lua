-- Faction definitions

-- Faction types must be defined first — `define_faction` calls below may
-- reference them by value (e.g. `faction_type = types.empire`).
local types = require("factions.faction_types")

-- #305 (S-11): Casus Belli definitions.
require("factions.casus_belli")

-- #321: Negotiation item kinds. Loaded before diplomatic options so option
-- definitions can reference item kinds by value if needed.
require("factions.negotiation_items")

-- Diplomatic options (#302 / #325). Lua-defined option framework for bilateral /
-- unilateral interactions. All diplomatic actions use this framework.
require("factions.options")

define_faction {
    id = "humanity_empire",
    name = "Terran Federation",
    faction_type = types.empire,
    on_game_start = function(ctx)
        -- Take full control of the capital system. Rather than relying on the
        -- random galaxy generator (which can produce capitals with bad rolls),
        -- explicitly clear the procedurally generated planets and spawn the
        -- canonical Sol system layout. This guarantees a survivable starting
        -- position regardless of RNG.
        ctx.system:set_attributes({
            name = "Sol",
            star_type = "yellow_dwarf",
            surveyed = true,
        })
        ctx.system:clear_planets()

        local earth = ctx.system:spawn_planet("Earth", "terrestrial", {
            habitability       = 1.0,
            mineral_richness   = 0.7,
            energy_potential   = 0.5,
            research_potential = 0.7,
            max_building_slots = 6,
        })
        earth:colonize(ctx.faction)
        -- #280: planetary_capital_t3 must be placed FIRST so it occupies slot 0.
        earth:add_building("planetary_capital_t3")
        earth:add_building("mine")
        earth:add_building("power_plant")
        earth:add_building("farm")

        ctx.system:spawn_planet("Mars", "arid", {
            habitability       = 0.4,
            mineral_richness   = 0.6,
            energy_potential   = 0.3,
            research_potential = 0.3,
            max_building_slots = 3,
        })

        ctx.system:spawn_planet("Jupiter", "gas_giant", {
            habitability       = 0.0,
            mineral_richness   = 0.2,
            energy_potential   = 0.8,
            research_potential = 0.5,
            max_building_slots = 2,
        })

        ctx.system:add_building("shipyard")
        ctx.system:spawn_core()

        ctx.system:spawn_ship("explorer_mk1", "Explorer-1")
        ctx.system:spawn_ship("explorer_mk1", "Explorer-2")
        ctx.system:spawn_ship("courier_mk1", "Courier-1")
        ctx.system:spawn_ship("colony_ship_mk1", "Colony Ship-1")
    end,
}

-- #429: NPC empires with on_game_start callbacks that use the shared
-- initialize_default_capital helper. Each faction gets its own home system
-- allocated during galaxy generation (Phase B).
define_faction {
    id = "vesk_hegemony",
    name = "Vesk Hegemony",
    faction_type = types.empire,
    on_game_start = function(ctx)
        initialize_default_capital(ctx, {
            home_planet_name = "Vesk Prime",
            home_planet_type = "arid",
            home_planet_attrs = {
                habitability       = 0.85,
                mineral_richness   = 0.8,
                energy_potential   = 0.6,
                research_potential = 0.4,
                max_building_slots = 5,
            },
            starter_ships = {
                { "explorer_mk1", "Vesk Scout-1" },
                { "explorer_mk1", "Vesk Scout-2" },
                { "courier_mk1", "Vesk Courier-1" },
                { "colony_ship_mk1", "Vesk Colony Ship-1" },
            },
        })
    end,
}

define_faction {
    id = "aurelian_concord",
    name = "Aurelian Concord",
    faction_type = types.empire,
    on_game_start = function(ctx)
        initialize_default_capital(ctx, {
            home_planet_name = "Aurelia",
            home_planet_type = "ocean",
            home_planet_attrs = {
                habitability       = 0.90,
                mineral_richness   = 0.5,
                energy_potential   = 0.7,
                research_potential = 0.6,
                max_building_slots = 5,
            },
            starter_ships = {
                { "explorer_mk1", "Aurelian Explorer-1" },
                { "explorer_mk1", "Aurelian Explorer-2" },
                { "courier_mk1", "Aurelian Courier-1" },
                { "colony_ship_mk1", "Aurelian Colony Ship-1" },
            },
        })
    end,
}

return {}
