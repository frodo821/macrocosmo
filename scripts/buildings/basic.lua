local mine = define_building {
    id = "mine",
    name = "Mine",
    description = "Extracts minerals from planetary deposits",
    cost = { minerals = 150, energy = 50 },
    build_time = 10,
    maintenance = 0.2,
    production_bonus = { minerals = 3.0 },
    is_system_building = false,
    upgrade_to = {
        { target = forward_ref("advanced_mine"), cost = { minerals = 200, energy = 100 }, build_time = 8 },
    },
}

local power_plant = define_building {
    id = "power_plant",
    name = "PowerPlant",
    description = "Generates energy from local resources",
    cost = { minerals = 50, energy = 150 },
    build_time = 10,
    maintenance = 0.0,
    production_bonus = { energy = 3.0 },
    is_system_building = false,
    upgrade_to = {
        { target = forward_ref("advanced_power_plant"), cost = { minerals = 150, energy = 200 }, build_time = 10 },
    },
}

local research_lab = define_building {
    id = "research_lab",
    name = "ResearchLab",
    description = "Conducts scientific research",
    cost = { minerals = 100, energy = 100 },
    build_time = 15,
    maintenance = 0.5,
    production_bonus = { research = 2.0 },
    is_system_building = true,
}

local shipyard = define_building {
    id = "shipyard",
    name = "Shipyard",
    description = "Constructs and refits ships",
    cost = { minerals = 300, energy = 200 },
    build_time = 30,
    maintenance = 1.0,
    is_system_building = true,
    capabilities = { shipyard = true },
}

local port = define_building {
    id = "port",
    name = "Port",
    description = "Reduces FTL travel time from this system",
    cost = { minerals = 400, energy = 300 },
    build_time = 40,
    maintenance = 0.5,
    is_system_building = true,
    capabilities = { port = true },
}

local farm = define_building {
    id = "farm",
    name = "Farm",
    description = "Produces food to sustain population",
    cost = { minerals = 100, energy = 50 },
    build_time = 20,
    maintenance = 0.3,
    production_bonus = { food = 5.0 },
    is_system_building = false,
}

-- Upgrade-only buildings (not directly buildable)
local advanced_mine = define_building {
    id = "advanced_mine",
    name = "Advanced Mine",
    description = "Automated extraction with higher mineral yield",
    cost = nil,
    build_time = 10,
    maintenance = 0.4,
    production_bonus = { minerals = 6.0 },
    is_system_building = false,
    prerequisites = has_tech(forward_ref("industrial_automated_mining")),
}

local advanced_power_plant = define_building {
    id = "advanced_power_plant",
    name = "Advanced PowerPlant",
    description = "Fusion-powered energy generation with higher output",
    cost = nil,
    build_time = 10,
    maintenance = 0.2,
    production_bonus = { energy = 6.0 },
    is_system_building = false,
    prerequisites = has_tech(forward_ref("industrial_fusion_power")),
}

return {
    mine = mine,
    power_plant = power_plant,
    research_lab = research_lab,
    shipyard = shipyard,
    port = port,
    farm = farm,
    advanced_mine = advanced_mine,
    advanced_power_plant = advanced_power_plant,
}
