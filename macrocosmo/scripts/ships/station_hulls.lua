-- #385: Station hull definitions for system buildings (Shipyard/Port/ResearchLab).
--
-- All station hulls are immobile (base_speed = 0, no FTL/sublight slots) and
-- use size = 10000 so they can never be loaded into another ship's harbour.
-- Modifier-bearing modules are installed via station designs to provide the
-- building's runtime effects (harbour capacity, research slots, etc.).

local slot_types = require("ships.slot_types")

local station_shipyard_hull = define_hull {
    id = "station_shipyard_hull",
    name = "Orbital Shipyard",
    description = "Immobile orbital platform for ship construction and refit.",
    size = 10000,
    is_capital = false,
    base_hp = 500,
    base_speed = 0.0,
    base_evasion = 0.0,
    slots = {
        { type = slot_types.utility, count = 2 },
    },
    build_cost = { minerals = 0, energy = 0 },
    build_time = 0,
    maintenance = 1.0,
}

local station_port_hull = define_hull {
    id = "station_port_hull",
    name = "Trade Port",
    description = "Immobile orbital trade hub providing harbour and logistics. Equipped with light point-defense armament.",
    size = 10000,
    is_capital = false,
    base_hp = 300,
    base_speed = 0.0,
    base_evasion = 0.0,
    slots = {
        { type = slot_types.utility, count = 2 },
        { type = slot_types.weapon, count = 1 },
    },
    build_cost = { minerals = 0, energy = 0 },
    build_time = 0,
    maintenance = 0.5,
}

local station_research_lab_hull = define_hull {
    id = "station_research_lab_hull",
    name = "Research Station",
    description = "Immobile orbital laboratory for scientific research.",
    size = 10000,
    is_capital = false,
    base_hp = 200,
    base_speed = 0.0,
    base_evasion = 0.0,
    slots = {
        { type = slot_types.utility, count = 2 },
    },
    build_cost = { minerals = 0, energy = 0 },
    build_time = 0,
    maintenance = 0.5,
}

return {
    station_shipyard_hull = station_shipyard_hull,
    station_port_hull = station_port_hull,
    station_research_lab_hull = station_research_lab_hull,
}
