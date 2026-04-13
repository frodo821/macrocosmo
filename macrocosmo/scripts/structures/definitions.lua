-- Deep Space Structure definitions
-- Loaded by the StructureRegistry at startup.
--
-- #223: Shipyard-buildable, Cargo-transportable structures are declared via
-- `define_deliverable`. `define_structure` is reserved for world-only entities
-- (e.g. debris wrecks) or upgrade-only outputs that cannot be built directly.

-- Note: ftl_communications and ftl_interdiction_tech are forward references
-- to techs not yet defined. Using forward_ref() to express this intent.

local sensor_buoy = define_deliverable {
    id = "sensor_buoy",
    name = "Sensor Buoy",
    description = "Detects sublight vessel movements.",
    max_hp = 20,
    cost = { minerals = 50, energy = 30 },
    build_time = 15,
    cargo_size = 1,
    scrap_refund = 0.5,
    capabilities = {
        detect_sublight = { range = 3.0 },
    },
    energy_drain = 100, -- millis (0.1 units per hexady)
}

local ftl_comm_relay = define_deliverable {
    id = "ftl_comm_relay",
    name = "FTL Comm Relay",
    description = "Enables faster-than-light communication across systems.",
    max_hp = 50,
    cost = { minerals = 200, energy = 150 },
    build_time = 30,
    cargo_size = 2,
    scrap_refund = 0.4,
    capabilities = {
        -- range_ly: source relay observes ships within this range; receiver
        -- relay requires the player be within its own range_ly. A value of 0
        -- is treated as "infinite" by the relay_knowledge_propagate_system.
        ftl_comm_relay = { range = 5.0 },
    },
    energy_drain = 500, -- millis (0.5 units per hexady)
    prerequisites = has_tech(forward_ref("ftl_communications")),
}

local interdictor = define_deliverable {
    id = "interdictor",
    name = "Interdictor",
    description = "Disrupts FTL travel within its interdiction range.",
    max_hp = 80,
    cost = { minerals = 300, energy = 200 },
    build_time = 45,
    cargo_size = 3,
    scrap_refund = 0.3,
    capabilities = {
        ftl_interdiction = { range = 5.0 },
    },
    energy_drain = 1000, -- millis (1.0 units per hexady)
    prerequisites = has_tech(forward_ref("ftl_interdiction_tech")),
}

return {
    sensor_buoy = sensor_buoy,
    ftl_comm_relay = ftl_comm_relay,
    interdictor = interdictor,
}
