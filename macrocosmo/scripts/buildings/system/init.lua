local orbital_research_lab = define_building {
    id = "orbital_research_lab",
    name = "Orbital Research Center",
    description = "A complex structure dedicated to scientific research. Provides bonuses to research output.",
    cost = { minerals = 150, energy = 200 },
    build_time = 15,
    maintenance = 0.5,
    -- 2 researchers × 0.5 research/pop = 1.0 research, matches prior balance.
    modifiers = {
        { target = "colony.researcher_slot", base_add = 2 },
        { target = "job:researcher::colony.research_per_hexadies", multiplier = 0.15 },
    },
    is_system_building = true,
    ship_design_id = "station_research_lab_v1",
}

local shipyard = define_building {
    id = "shipyard",
    name = "Shipyard",
    description = "Constructs and refits ships",
    cost = { minerals = 300, energy = 200 },
    build_time = 30,
    maintenance = 1.0,
    is_system_building = true,
    modifiers = {
        { target = "system.shipyard_capacity", base_add = 1 },
    },
    ship_design_id = "station_shipyard_v1",
}

local port = define_building {
    id = "port",
    name = "Spaceport",
    description = "Huge orbital structure for docking and trade. Provides bonuses to FTL travel and trade routes.",
    cost = { minerals = 400, energy = 300 },
    build_time = 40,
    maintenance = 0.5,
    is_system_building = true,
    modifiers = {
        { target = "system.port_ftl_range_bonus", base_add = 10.0 },
        { target = "system.port_travel_time_factor", base_add = -0.2 },
        { target = "system.port_repair", base_add = 1 },
    },
    ship_design_id = "station_port_v1",
}

return {
    orbital_research_lab = orbital_research_lab,
    shipyard = shipyard,
    port = port,
}
