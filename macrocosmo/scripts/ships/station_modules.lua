-- #385: Station-specific modules for system building ships.
--
-- These modules carry the modifiers that implement the runtime effects of
-- each station type. The modifier targets may not be fully wired in the
-- Rust modifier system yet -- they are defined here so the content is
-- ready when #384 connects the routing.

local slot_types = require("ships.slot_types")

-- Shipyard: harbour capacity for docked ships + build capability marker.
local shipyard_bay = define_module {
    id = "shipyard_bay",
    name = "Shipyard Construction Bay",
    slot_type = slot_types.utility,
    modifiers = {
        { target = "ship.harbour_capacity", base_add = 12 },
    },
    cost = { minerals = 0, energy = 0 },
    build_time = 0,
}

-- Port: harbour capacity + trade/logistics bonuses.
local port_dock = define_module {
    id = "port_dock",
    name = "Trade Port Dock",
    slot_type = slot_types.utility,
    modifiers = {
        { target = "ship.harbour_capacity", base_add = 8 },
    },
    cost = { minerals = 0, energy = 0 },
    build_time = 0,
}

-- ResearchLab: additional researcher slots at the system scope.
local research_array = define_module {
    id = "research_array",
    name = "Research Array",
    slot_type = slot_types.utility,
    modifiers = {
        { target = "system.researcher_slots", base_add = 2 },
    },
    cost = { minerals = 0, energy = 0 },
    build_time = 0,
}

return {
    shipyard_bay = shipyard_bay,
    port_dock = port_dock,
    research_array = research_array,
}
