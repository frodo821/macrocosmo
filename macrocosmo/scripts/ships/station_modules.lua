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

-- #219: Point defense turret for station self-defense. Light armament with
-- high tracking (good vs small targets) but modest damage output.
local point_defense_turret = define_module {
    id = "point_defense_turret",
    name = "Point Defense Turret",
    slot_type = slot_types.weapon,
    weapon = {
        track = 8.0, precision = 0.80, cooldown = 1, range = 5.0,
        shield_damage = 2.0, shield_damage_div = 0.5, shield_piercing = 0.0,
        armor_damage = 1.5, armor_damage_div = 0.5, armor_piercing = 0.0,
        hull_damage = 2.0, hull_damage_div = 0.5,
    },
    cost = { minerals = 0, energy = 0 },
    build_time = 0,
}

return {
    shipyard_bay = shipyard_bay,
    port_dock = port_dock,
    research_array = research_array,
    point_defense_turret = point_defense_turret,
}
