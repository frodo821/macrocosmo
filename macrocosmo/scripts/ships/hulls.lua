local slot_types = require("ships.slot_types")

local corvette = define_hull {
    id = "corvette",
    name = "Corvette",
    size = 1,
    base_hp = 50,
    base_speed = 0.75,
    base_evasion = 30.0,
    slots = {
        { type = slot_types.ftl, count = 1 },
        { type = slot_types.sublight, count = 1 },
        { type = slot_types.weapon, size = "small", count = 2 },
        { type = slot_types.defense, size = "small", count = 1 },
        { type = slot_types.utility, count = 1 },
        { type = slot_types.reactor, count = 1 },
    },
    build_cost = { minerals = 200, energy = 100 },
    build_time = 60,
    maintenance = 0.5,
}

local frigate = define_hull {
    id = "frigate",
    name = "Frigate",
    size = 2,
    base_hp = 120,
    base_speed = 0.5,
    base_evasion = 15.0,
    slots = {
        { type = slot_types.ftl, count = 1 },
        { type = slot_types.sublight, count = 1 },
        { type = slot_types.weapon, size = "medium", count = 3 },
        { type = slot_types.defense, size = "medium", count = 2 },
        { type = slot_types.utility, count = 2 },
        { type = slot_types.reactor, count = 1 },
        { type = slot_types.comms, count = 1 },
    },
    build_cost = { minerals = 400, energy = 200 },
    build_time = 120,
    maintenance = 1.0,
}

local cruiser = define_hull {
    id = "cruiser",
    name = "Cruiser",
    size = 4,
    base_hp = 250,
    base_speed = 0.35,
    base_evasion = 5.0,
    slots = {
        { type = slot_types.ftl, count = 1 },
        { type = slot_types.sublight, count = 2 },
        { type = slot_types.weapon, size = "large", count = 4 },
        { type = slot_types.defense, size = "large", count = 3 },
        { type = slot_types.utility, count = 2 },
        { type = slot_types.reactor, count = 2 },
        { type = slot_types.comms, count = 1 },
    },
    build_cost = { minerals = 800, energy = 400 },
    build_time = 240,
    maintenance = 2.0,
}

local scout_hull = define_hull {
    id = "scout_hull",
    name = "Scout Hull",
    size = 1,
    base_hp = 40,
    base_speed = 0.85,
    base_evasion = 35.0,
    slots = {
        { type = slot_types.ftl, count = 1 },
        { type = slot_types.sublight, count = 1 },
        { type = slot_types.utility, count = 2 },
        { type = slot_types.weapon, size = "small", count = 1 },
        { type = slot_types.reactor, count = 1 },
    },
    build_cost = { minerals = 150, energy = 80 },
    build_time = 45,
    maintenance = 0.4,
    modifiers = {
        { target = "ship.survey_speed", base_add = 0.0, multiplier = 1.3, add = 0.0 },
        { target = "ship.speed", base_add = 0.0, multiplier = 1.15, add = 0.0 },
    },
}

local courier_hull = define_hull {
    id = "courier_hull",
    name = "Courier Hull",
    size = 1,
    base_hp = 35,
    base_speed = 0.80,
    base_evasion = 25.0,
    slots = {
        { type = slot_types.ftl, count = 1 },
        { type = slot_types.sublight, count = 1 },
        { type = slot_types.utility, count = 2 },
        { type = slot_types.reactor, count = 1 },
    },
    build_cost = { minerals = 100, energy = 50 },
    build_time = 30,
    maintenance = 0.3,
    modifiers = {
        { target = "ship.cargo_capacity", base_add = 0.0, multiplier = 1.5, add = 0.0 },
        { target = "ship.ftl_range", base_add = 0.0, multiplier = 1.2, add = 0.0 },
    },
}

return {
    corvette = corvette,
    frigate = frigate,
    cruiser = cruiser,
    scout_hull = scout_hull,
    courier_hull = courier_hull,
}
