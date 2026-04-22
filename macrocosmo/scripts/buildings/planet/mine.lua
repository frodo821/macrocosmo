local mine = define_building {
    id = "mine",
    name = "Mineshaft",
    description = "Extracts minerals from planetary deposits",
    cost = { minerals = 150, energy = 50 },
    build_time = 10,
    maintenance = 0.2,
    modifiers = {
        { target = "colony.miner_slot", base_add = 5 },
    },
    is_system_building = false,
    upgrade_to = {
        { target = forward_ref("advanced_mine"), cost = { minerals = 200, energy = 100 }, build_time = 8 },
    },
}

-- Upgrade-only buildings (not directly buildable)
local advanced_mine = define_building {
    id = "advanced_mine",
    name = "Advanced Mine",
    description = "Automated extraction with higher mineral yield",
    cost = nil,
    build_time = 10,
    maintenance = 0.4,
    -- 10 miners × 0.6 minerals/pop = 6.0, double the basic mine.
    modifiers = {
        { target = "colony.miner_slot", base_add = 10 },
    },
    is_system_building = false,
    prerequisites = has_tech(forward_ref("industrial_automated_mining")),
}

return {
    t1 = mine,
    t2 = advanced_mine,
}
