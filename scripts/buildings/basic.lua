local mine = define_building {
    id = "mine",
    name = "Mine",
    cost = { minerals = 150, energy = 50 },
    build_time = 10,
    maintenance = 0.2,
    production_bonus = { minerals = 3.0 },
}

local power_plant = define_building {
    id = "power_plant",
    name = "PowerPlant",
    cost = { minerals = 50, energy = 150 },
    build_time = 10,
    maintenance = 0.0,
    production_bonus = { energy = 3.0 },
}

local research_lab = define_building {
    id = "research_lab",
    name = "ResearchLab",
    cost = { minerals = 100, energy = 100 },
    build_time = 15,
    maintenance = 0.5,
    production_bonus = { research = 2.0 },
}

local shipyard = define_building {
    id = "shipyard",
    name = "Shipyard",
    cost = { minerals = 300, energy = 200 },
    build_time = 30,
    maintenance = 1.0,
}

local port = define_building {
    id = "port",
    name = "Port",
    cost = { minerals = 400, energy = 300 },
    build_time = 40,
    maintenance = 0.5,
}

local farm = define_building {
    id = "farm",
    name = "Farm",
    cost = { minerals = 100, energy = 50 },
    build_time = 20,
    maintenance = 0.3,
    production_bonus = { food = 5.0 },
}

return {
    mine = mine,
    power_plant = power_plant,
    research_lab = research_lab,
    shipyard = shipyard,
    port = port,
    farm = farm,
}
