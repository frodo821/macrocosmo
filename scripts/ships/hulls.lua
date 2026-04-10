define_hull {
    id = "corvette",
    name = "Corvette",
    base_hp = 50,
    base_speed = 0.75,
    base_evasion = 30.0,
    slots = {
        { type = "weapon", count = 2 },
        { type = "utility", count = 1 },
        { type = "engine", count = 1 },
    },
    build_cost = { minerals = 200, energy = 100 },
    build_time = 60,
    maintenance = 0.5,
}

define_hull {
    id = "frigate",
    name = "Frigate",
    base_hp = 120,
    base_speed = 0.5,
    base_evasion = 15.0,
    slots = {
        { type = "weapon", count = 3 },
        { type = "utility", count = 2 },
        { type = "engine", count = 1 },
        { type = "special", count = 1 },
    },
    build_cost = { minerals = 400, energy = 200 },
    build_time = 120,
    maintenance = 1.0,
}

define_hull {
    id = "cruiser",
    name = "Cruiser",
    base_hp = 250,
    base_speed = 0.35,
    base_evasion = 5.0,
    slots = {
        { type = "weapon", count = 4 },
        { type = "utility", count = 3 },
        { type = "engine", count = 2 },
        { type = "special", count = 2 },
    },
    build_cost = { minerals = 800, energy = 400 },
    build_time = 240,
    maintenance = 2.0,
}
