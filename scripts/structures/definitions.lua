-- Deep Space Structure definitions
-- Loaded by the StructureRegistry at startup.

define_structure {
    id = "sensor_buoy",
    name = "Sensor Buoy",
    max_hp = 20,
    build_cost_minerals = 50,
    build_cost_energy = 30,
    build_time = 15,
    capabilities = { "detect_sublight" },
    detection_range = 3.0,
    energy_drain = 100, -- millis (0.1 units per hexady)
}

define_structure {
    id = "ftl_comm_relay",
    name = "FTL Comm Relay",
    max_hp = 50,
    build_cost_minerals = 200,
    build_cost_energy = 150,
    build_time = 30,
    capabilities = { "ftl_comm" },
    energy_drain = 500, -- millis (0.5 units per hexady)
    prerequisite_tech = "ftl_communications",
}

define_structure {
    id = "interdictor",
    name = "Interdictor",
    max_hp = 80,
    build_cost_minerals = 300,
    build_cost_energy = 200,
    build_time = 45,
    capabilities = { "ftl_interdiction" },
    interdiction_range = 5.0,
    energy_drain = 1000, -- millis (1.0 units per hexady)
    prerequisite_tech = "ftl_interdiction_tech",
}
