local colony_hub_t1 = define_building {
    id = "colony_hub_t1",
    name = "Colony Hub",
    description = "Basic colony administration hub. Provides initial building slots.",
    cost = nil,
    build_time = 10,
    maintenance = 0.0,
    dismantlable = false,
    is_system_building = false,
    capabilities = { colony_hub = { fixed_slots = 4 } },
    modifiers = {
        { target = "colony.farmer_slot", base_add = 1 },
        { target = "colony.power_worker_slot", base_add = 1 },
    },
    upgrade_to = {
        { target = forward_ref("colony_hub_t2"), cost = { minerals = 200, energy = 100 }, build_time = 15 },
    },
}

local colony_hub_t2 = define_building {
    id = "colony_hub_t2",
    name = "Colony Hub II",
    description = "Upgraded administration hub with expanded building capacity.",
    cost = nil,
    build_time = 15,
    maintenance = 0.0,
    dismantlable = false,
    is_system_building = false,
    capabilities = { colony_hub = { fixed_slots = 6 } },
    modifiers = {
        { target = "colony.farmer_slot", base_add = 2 },
        { target = "colony.power_worker_slot", base_add = 2 },
    },
    upgrade_to = {
        { target = forward_ref("colony_hub_t3"), cost = { minerals = 400, energy = 200 }, build_time = 20 },
    },
}

local colony_hub_t3 = define_building {
    id = "colony_hub_t3",
    name = "Colony Hub III",
    description = "Advanced administration hub. Can be upgraded to a Planetary Capital.",
    cost = nil,
    build_time = 20,
    maintenance = 0.0,
    dismantlable = false,
    is_system_building = false,
    capabilities = { colony_hub = { fixed_slots = 8 } },
    modifiers = {
        { target = "colony.farmer_slot", base_add = 3 },
        { target = "colony.power_worker_slot", base_add = 3 },
    },
    prerequisites = has_tech(forward_ref("industrial_automated_mining")),
    upgrade_to = {
        { target = forward_ref("planetary_capital_t1"), cost = { minerals = 600, energy = 400 }, build_time = 30 },
    },
}

-- #280: Planetary Capital buildings — enhanced hub for capital/major colonies.
local planetary_capital_t1 = define_building {
    id = "planetary_capital_t1",
    name = "Planetary Capital",
    description = "Seat of planetary government. Provides substantial building capacity and research bonus.",
    cost = nil,
    build_time = 30,
    maintenance = 0.0,
    dismantlable = false,
    is_system_building = false,
    capabilities = { colony_hub = { fixed_slots = 10 } },
    modifiers = {
        { target = "colony.farmer_slot", base_add = 3 },
        { target = "colony.power_worker_slot", base_add = 3 },
        { target = "colony.research_per_hexadies", base_add = 1.0 },
    },
    upgrade_to = {
        { target = forward_ref("planetary_capital_t2"), cost = { minerals = 800, energy = 600 }, build_time = 40 },
    },
}

local planetary_capital_t2 = define_building {
    id = "planetary_capital_t2",
    name = "Planetary Capital II",
    description = "Expanded capital complex with enhanced administration.",
    cost = nil,
    build_time = 40,
    maintenance = 0.0,
    dismantlable = false,
    is_system_building = false,
    capabilities = { colony_hub = { fixed_slots = 12 } },
    modifiers = {
        { target = "colony.farmer_slot", base_add = 3 },
        { target = "colony.power_worker_slot", base_add = 3 },
        { target = "colony.research_per_hexadies", base_add = 2.0 },
    },
    upgrade_to = {
        { target = forward_ref("planetary_capital_t3"), cost = { minerals = 1000, energy = 800 }, build_time = 50 },
    },
}

local planetary_capital_t3 = define_building {
    id = "planetary_capital_t3",
    name = "Planetary Capital III",
    description = "Apex of planetary governance. Maximum building capacity and research output.",
    cost = nil,
    build_time = 50,
    maintenance = 0.0,
    dismantlable = false,
    is_system_building = false,
    capabilities = { colony_hub = { fixed_slots = 14, slot_ratio = 0.15 } },
    modifiers = {
        { target = "colony.farmer_slot", base_add = 3 },
        { target = "colony.power_worker_slot", base_add = 3 },
        { target = "colony.research_per_hexadies", base_add = 3.0 },
    },
}

return {
    t1 = colony_hub_t1,
    t2 = colony_hub_t2,
    t3 = colony_hub_t3,
    t4 = planetary_capital_t1,
    t5 = planetary_capital_t2,
    t6 = planetary_capital_t3,
}
