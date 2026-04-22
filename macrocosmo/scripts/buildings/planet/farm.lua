local farm = define_building {
    id = "farm",
    name = "Hydroponic Cultivator",
    description = "Produces food to sustain population",
    cost = { minerals = 100, energy = 50 },
    build_time = 20,
    maintenance = 0.3,
    -- 5 farmers × 1.0 food/pop = 5.0 food, matches prior balance.
    modifiers = {
        { target = "colony.farmer_slot", base_add = 5 },
    },
    is_system_building = false,
    upgrade_to = {
        { target = forward_ref("farm_t2"), cost = { minerals = 150, energy = 200 }, build_time = 10 },
    },
}

local farm_t2 = define_building {
    id = "farm_t2",
    name = "Nutrition Processor",
    description = "More capable food production facility with enhanced output",
    cost = nil,
    build_time = 30,
    maintenance = 0.3,
    -- 10 farmers × 1.0 food/pop = 10.0 food, matches prior balance.
    modifiers = {
        { target = "colony.farmer_slot", base_add = 10 },
    },
    is_system_building = false,
}

return {
    t1 = farm,
    t2 = farm_t2,
}
