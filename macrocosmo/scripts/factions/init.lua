-- Faction definitions

-- Faction types must be defined first — `define_faction` calls below may
-- reference them by value (e.g. `faction_type = types.empire`).
local types = require("factions.faction_types")

-- Diplomatic actions (#172). Independent of faction definitions — the
-- registry resolves prerequisites against relations + types at call time.
require("factions.actions")

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

        ctx.system:spawn_ship("explorer_mk1", "Explorer-1")
        ctx.system:spawn_ship("explorer_mk1", "Explorer-2")
        ctx.system:spawn_ship("courier_mk1", "Courier-1")
        ctx.system:spawn_ship("colony_ship_mk1", "Colony Ship-1")
    end,
}

-- #173: NPC empires. Defined without `on_game_start` so they do not compete
-- with the player empire for the single capital star system. Homeworlds and
-- starting fleets for NPC empires are a follow-up under #189.
define_faction {
    id = "vesk_hegemony",
    name = "Vesk Hegemony",
    faction_type = types.empire,
}

define_faction {
    id = "aurelian_concord",
    name = "Aurelian Concord",
    faction_type = types.empire,
}

return {}
