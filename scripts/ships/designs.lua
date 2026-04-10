-- Default designs matching current ShipType functionality
define_ship_design {
    id = "explorer_mk1",
    name = "Explorer Mk.I",
    hull = "corvette",
    modules = {
        { slot_type = "engine", module = "ftl_drive" },
        { slot_type = "utility", module = "survey_equipment" },
    },
}

define_ship_design {
    id = "colony_ship_mk1",
    name = "Colony Ship Mk.I",
    hull = "frigate",
    modules = {
        { slot_type = "engine", module = "ftl_drive" },
        { slot_type = "special", module = "colony_module" },
    },
}

define_ship_design {
    id = "courier_mk1",
    name = "Courier Mk.I",
    hull = "courier_hull",
    modules = {
        { slot_type = "engine", module = "ftl_drive" },
        { slot_type = "engine", module = "ftl_drive" },
        { slot_type = "utility", module = "cargo_bay" },
    },
}

define_ship_design {
    id = "scout_mk1",
    name = "Scout Mk.I",
    hull = "scout_hull",
    modules = {
        { slot_type = "engine", module = "ftl_drive" },
        { slot_type = "utility", module = "survey_equipment" },
    },
}

define_ship_design {
    id = "patrol_corvette",
    name = "Patrol Corvette",
    hull = "corvette",
    modules = {
        { slot_type = "weapon", module = "weapon_laser" },
        { slot_type = "weapon", module = "weapon_laser" },
        { slot_type = "engine", module = "ftl_drive" },
        { slot_type = "utility", module = "armor_plating" },
    },
}
