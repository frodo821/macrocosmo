-- Faction definitions

define_faction {
    id = "humanity_empire",
    name = "Terran Federation",
    on_game_start = function(ctx)
        -- The capital system already has the standard Terran-controlled
        -- entities (StarSystem, Planets, ResourceStockpile, etc.) spawned
        -- by the engine. The on_game_start callback configures the initial
        -- buildings and ships for this faction.
        local planet = ctx.system:get_planet(1)
        planet:colonize(ctx.faction)
        planet:add_building("mine")
        planet:add_building("power_plant")
        planet:add_building("farm")

        ctx.system:add_building("shipyard")

        ctx.system:spawn_ship("explorer_mk1", "Explorer-1")
        ctx.system:spawn_ship("explorer_mk1", "Explorer-2")
        ctx.system:spawn_ship("courier_mk1", "Courier-1")
        ctx.system:spawn_ship("colony_ship_mk1", "Colony Ship-1")
    end,
}

return {}
