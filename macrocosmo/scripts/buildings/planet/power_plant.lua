local power_plant = define_building {
    id = "power_plant",
    name = "Combustion Power Plant",
    description = "Generates energy from local resources",
    cost = { minerals = 50, energy = 150 },
    build_time = 10,
    maintenance = 0.0,
    -- 5 power workers × 6.0 energy/pop = 30 energy, matches #236 balance.
    modifiers = {
        { target = "colony.power_worker_slot", base_add = 5 },
    },
    is_system_building = false,
    upgrade_to = {
        { target = forward_ref("advanced_power_plant"), cost = { minerals = 150, energy = 200 }, build_time = 10 },
    },
}

local advanced_power_plant = define_building {
    id = "advanced_power_plant",
    name = "Advanced Power Plant",
    description = "Fusion-powered energy generation with higher output",
    cost = nil,
    build_time = 10,
    maintenance = 0.2,
    -- 10 power workers × 6.0 energy/pop = 60.0, double the basic plant.
    modifiers = {
        { target = "colony.power_worker_slot", base_add = 10 },
    },
    is_system_building = false,
    prerequisites = has_tech(forward_ref("industrial_fusion_power")),
}

return {
    t1 = power_plant,
    t2 = advanced_power_plant,
}
