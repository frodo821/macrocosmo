local hulls = require("ships.hulls")
local modules = require("ships.modules")
local station_hulls = require("ships.station_hulls")
local station_modules = require("ships.station_modules")

-- #236: Ship design stats (hp/ftl_range/cost/maintenance/build_time/can_*)
-- are derived from hull + modules at registry load time. Presets no longer
-- author those fields — only id/name/hull/modules (and description) are read.

-- Default designs matching current ShipType functionality
local explorer_mk1 = define_ship_design {
    id = "explorer_mk1",
    name = "Explorer Mk.I",
    hull = hulls.corvette,
    modules = {
        { slot_type = "ftl", module = modules.ftl_drive },
        { slot_type = "utility", module = modules.survey_equipment },
    },
}

local colony_ship_mk1 = define_ship_design {
    id = "colony_ship_mk1",
    name = "Colony Ship Mk.I",
    hull = hulls.frigate,
    modules = {
        { slot_type = "ftl", module = modules.ftl_drive },
        { slot_type = "utility", module = modules.colony_module },
    },
}

local courier_mk1 = define_ship_design {
    id = "courier_mk1",
    name = "Courier Mk.I",
    hull = hulls.courier_hull,
    modules = {
        { slot_type = "ftl", module = modules.ftl_drive },
        { slot_type = "sublight", module = modules.afterburner },
        { slot_type = "utility", module = modules.cargo_bay },
    },
}

local scout_mk1 = define_ship_design {
    id = "scout_mk1",
    name = "Scout Mk.I",
    hull = hulls.scout_hull,
    modules = {
        { slot_type = "ftl", module = modules.ftl_drive },
        { slot_type = "utility", module = modules.survey_equipment },
    },
}

local patrol_corvette = define_ship_design {
    id = "patrol_corvette",
    name = "Patrol Corvette",
    hull = hulls.corvette,
    modules = {
        { slot_type = "weapon", module = modules.weapon_laser },
        { slot_type = "weapon", module = modules.weapon_laser },
        { slot_type = "ftl", module = modules.ftl_drive },
        { slot_type = "defense", module = modules.armor_plating },
    },
}

-- #385: Station designs for system buildings (immobile, spawned by building construction).
local station_shipyard_v1 = define_ship_design {
    id = "station_shipyard_v1",
    name = "Orbital Shipyard",
    hull = station_hulls.station_shipyard_hull,
    modules = {
        { slot_type = "utility", module = station_modules.shipyard_bay },
    },
}

local station_port_v1 = define_ship_design {
    id = "station_port_v1",
    name = "Trade Port",
    hull = station_hulls.station_port_hull,
    modules = {
        { slot_type = "utility", module = station_modules.port_dock },
    },
}

local station_research_lab_v1 = define_ship_design {
    id = "station_research_lab_v1",
    name = "Research Station",
    hull = station_hulls.station_research_lab_hull,
    modules = {
        { slot_type = "utility", module = station_modules.research_array },
    },
}

return {
    explorer_mk1 = explorer_mk1,
    colony_ship_mk1 = colony_ship_mk1,
    courier_mk1 = courier_mk1,
    scout_mk1 = scout_mk1,
    patrol_corvette = patrol_corvette,
    station_shipyard_v1 = station_shipyard_v1,
    station_port_v1 = station_port_v1,
    station_research_lab_v1 = station_research_lab_v1,
}
