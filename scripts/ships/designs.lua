local hulls = require("ships.hulls")
local modules = require("ships.modules")

-- Default designs matching current ShipType functionality
local explorer_mk1 = define_ship_design {
    id = "explorer_mk1",
    name = "Explorer Mk.I",
    hull = hulls.corvette,
    modules = {
        { slot_type = "engine", module = modules.ftl_drive },
        { slot_type = "utility", module = modules.survey_equipment },
    },
}

local colony_ship_mk1 = define_ship_design {
    id = "colony_ship_mk1",
    name = "Colony Ship Mk.I",
    hull = hulls.frigate,
    modules = {
        { slot_type = "engine", module = modules.ftl_drive },
        { slot_type = "special", module = modules.colony_module },
    },
}

local courier_mk1 = define_ship_design {
    id = "courier_mk1",
    name = "Courier Mk.I",
    hull = hulls.courier_hull,
    modules = {
        { slot_type = "engine", module = modules.ftl_drive },
        { slot_type = "engine", module = modules.ftl_drive },
        { slot_type = "utility", module = modules.cargo_bay },
    },
}

local scout_mk1 = define_ship_design {
    id = "scout_mk1",
    name = "Scout Mk.I",
    hull = hulls.scout_hull,
    modules = {
        { slot_type = "engine", module = modules.ftl_drive },
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
        { slot_type = "engine", module = modules.ftl_drive },
        { slot_type = "utility", module = modules.armor_plating },
    },
}

return {
    explorer_mk1 = explorer_mk1,
    colony_ship_mk1 = colony_ship_mk1,
    courier_mk1 = courier_mk1,
    scout_mk1 = scout_mk1,
    patrol_corvette = patrol_corvette,
}
