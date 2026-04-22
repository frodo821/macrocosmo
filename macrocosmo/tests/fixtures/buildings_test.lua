-- Test fixture for building_api tests.
-- Defines a minimal set of buildings with known values so Rust-side tests
-- are independent of production scripts/buildings/basic.lua.

define_building {
    id = "test_mine",
    name = "Test Mine",
    description = "A mine for testing",
    cost = { minerals = 100, energy = 25 },
    build_time = 5,
    maintenance = 0.5,
    modifiers = {
        { target = "colony.miner_slot", base_add = 3 },
    },
}

define_building {
    id = "test_farm",
    name = "Test Farm",
    description = "A farm for testing",
    cost = { minerals = 80, energy = 20 },
    build_time = 4,
    maintenance = 0.3,
    modifiers = {
        { target = "colony.farmer_slot", base_add = 4 },
    },
}

define_building {
    id = "test_shipyard",
    name = "Test Shipyard",
    description = "A shipyard for testing",
    cost = { minerals = 200, energy = 100 },
    build_time = 15,
    maintenance = 1,
    is_system_building = true,
    capabilities = { shipyard = {} },
}

define_building {
    id = "test_port",
    name = "Test Port",
    description = "A port for testing",
    cost = { minerals = 150, energy = 75 },
    build_time = 10,
    maintenance = 0.8,
    is_system_building = true,
    capabilities = { port = {} },
    ship_design_id = "station_port_v1",
}

define_building {
    id = "test_research_lab",
    name = "Test Research Lab",
    description = "A research lab for testing",
    cost = { minerals = 120, energy = 60 },
    build_time = 12,
    maintenance = 0.6,
    is_system_building = true,
    capabilities = { research = {} },
    ship_design_id = "station_research_lab_v1",
}

define_building {
    id = "test_advanced_mine",
    name = "Test Advanced Mine",
    description = "An advanced mine requiring tech",
    cost = { minerals = 250, energy = 80 },
    build_time = 10,
    maintenance = 0.8,
    prerequisites = has_tech("industrial_automated_mining"),
    modifiers = {
        { target = "colony.miner_slot", base_add = 6 },
    },
}
