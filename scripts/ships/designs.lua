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
    can_survey = true,
    can_colonize = false,
    maintenance = 0.5,
    build_cost = { minerals = 200, energy = 100 },
    build_time = 60,
    hp = 50,
    sublight_speed = 0.75,
    ftl_range = 10.0,
}

local colony_ship_mk1 = define_ship_design {
    id = "colony_ship_mk1",
    name = "Colony Ship Mk.I",
    hull = hulls.frigate,
    modules = {
        { slot_type = "engine", module = modules.ftl_drive },
        { slot_type = "special", module = modules.colony_module },
    },
    can_survey = false,
    can_colonize = true,
    maintenance = 1.0,
    build_cost = { minerals = 500, energy = 300 },
    build_time = 120,
    hp = 100,
    sublight_speed = 0.5,
    ftl_range = 15.0,
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
    can_survey = false,
    can_colonize = false,
    maintenance = 0.3,
    build_cost = { minerals = 100, energy = 50 },
    build_time = 30,
    hp = 35,
    sublight_speed = 0.80,
    ftl_range = 0.0,
}

local scout_mk1 = define_ship_design {
    id = "scout_mk1",
    name = "Scout Mk.I",
    hull = hulls.scout_hull,
    modules = {
        { slot_type = "engine", module = modules.ftl_drive },
        { slot_type = "utility", module = modules.survey_equipment },
    },
    can_survey = true,
    can_colonize = false,
    maintenance = 0.4,
    build_cost = { minerals = 150, energy = 80 },
    build_time = 45,
    hp = 40,
    sublight_speed = 0.85,
    ftl_range = 10.0,
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
    can_survey = false,
    can_colonize = false,
    maintenance = 0.6,
    build_cost = { minerals = 380, energy = 200 },
    build_time = 60,
    hp = 50,
    sublight_speed = 0.75,
    ftl_range = 15.0,
}

return {
    explorer_mk1 = explorer_mk1,
    colony_ship_mk1 = colony_ship_mk1,
    courier_mk1 = courier_mk1,
    scout_mk1 = scout_mk1,
    patrol_corvette = patrol_corvette,
}
