-- Deep Space Structure definitions
-- Loaded by the StructureRegistry at startup.

define_structure {
    id = "sensor_buoy",
    name = "Sensor Buoy",
    description = "Detects sublight vessel movements.",
    max_hp = 20,
    cost = { minerals = 50, energy = 30 },
    build_time = 15,
    capabilities = {
        detect_sublight = { range = 3.0 },
    },
    energy_drain = 100, -- millis (0.1 units per hexady)
}

define_structure {
    id = "ftl_comm_relay",
    name = "FTL Comm Relay",
    description = "Enables faster-than-light communication across systems.",
    max_hp = 50,
    cost = { minerals = 200, energy = 150 },
    build_time = 30,
    capabilities = {
        ftl_comm = { range = 0.0 },
    },
    energy_drain = 500, -- millis (0.5 units per hexady)
    prerequisites = has_tech("ftl_communications"),
}

define_structure {
    id = "interdictor",
    name = "Interdictor",
    description = "Disrupts FTL travel within its interdiction range.",
    max_hp = 80,
    cost = { minerals = 300, energy = 200 },
    build_time = 45,
    capabilities = {
        ftl_interdiction = { range = 5.0 },
    },
    energy_drain = 1000, -- millis (1.0 units per hexady)
    prerequisites = has_tech("ftl_interdiction_tech"),
}
