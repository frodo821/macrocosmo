local slot_types = require("ships.slot_types")
local tech = require("tech")

-- #239: `build_time` (hexadies) contributes to the design's total build time
-- (`hull.build_time + Σ module.build_time`). Range is 5-20 hd with a rough
-- mapping to module weight/complexity:
--   light utilities (5-8):  cargo_bay, ion_thruster, afterburner,
--                           survey_equipment, scout_module
--   mid (10-12):             fusion_reactor, weapon_laser, weapon_missile,
--                           armor_plating, shield_generator
--   heavy (15-20):           ftl_drive, weapon_railgun, command_array,
--                           colony_module
-- These are deliberately coarse — fine balance is tracked in #61.
--
-- #138: `power_cost` / `power_output` / `size` fields support the power
-- budget and module size constraint system. Only reactor modules produce
-- power; all others consume it (or 0 for passive modules).

-- FTL drive modules (ftl slot)
local ftl_drive = define_module {
    id = "ftl_drive",
    name = "FTL Drive",
    slot_type = slot_types.ftl,
    modifiers = {
        { target = "ship.ftl_range", base_add = 15.0 },
    },
    cost = { minerals = 100, energy = 50 },
    build_time = 15,
    power_cost = 0,
}

-- Sublight engine modules (sublight slot)
local afterburner = define_module {
    id = "afterburner",
    name = "Afterburner",
    slot_type = slot_types.sublight,
    modifiers = {
        { target = "ship.speed", multiplier = 0.2 },
    },
    cost = { minerals = 60, energy = 40 },
    build_time = 8,
    power_cost = 1,
}

local ion_thruster = define_module {
    id = "ion_thruster",
    name = "Ion Thruster",
    slot_type = slot_types.sublight,
    modifiers = {
        { target = "ship.speed", base_add = 0.1 },
    },
    cost = { minerals = 40, energy = 30 },
    build_time = 6,
    power_cost = 1,
}

-- Weapon modules (weapon slot)
local weapon_laser = define_module {
    id = "weapon_laser",
    name = "Laser Battery",
    slot_type = slot_types.weapon,
    size = "small",
    prerequisites = has_tech(tech.military.kinetic_weapons),
    weapon = {
        track = 5.0, precision = 0.85, cooldown = 1, range = 10.0,
        shield_damage = 4.0, shield_damage_div = 1.0, shield_piercing = 0.0,
        armor_damage = 2.0, armor_damage_div = 0.5, armor_piercing = 0.0,
        hull_damage = 3.0, hull_damage_div = 1.0,
    },
    cost = { minerals = 50, energy = 30 },
    build_time = 10,
    power_cost = 2,
}

local weapon_railgun = define_module {
    id = "weapon_railgun",
    name = "Railgun",
    slot_type = slot_types.weapon,
    size = "medium",
    prerequisites = has_tech(tech.military.kinetic_weapons),
    weapon = {
        track = 2.0, precision = 0.90, cooldown = 3, range = 20.0,
        shield_damage = 1.0, shield_damage_div = 0.5, shield_piercing = 0.5,
        armor_damage = 8.0, armor_damage_div = 2.0, armor_piercing = 0.3,
        hull_damage = 10.0, hull_damage_div = 3.0,
    },
    cost = { minerals = 100, energy = 50 },
    build_time = 15,
    power_cost = 5,
}

local weapon_missile = define_module {
    id = "weapon_missile",
    name = "Missile Launcher",
    slot_type = slot_types.weapon,
    size = "small",
    prerequisites = has_tech(tech.military.kinetic_weapons),
    weapon = {
        track = 8.0, precision = 0.70, cooldown = 2, range = 15.0,
        shield_damage = 1.0, shield_damage_div = 0.5, shield_piercing = 0.8,
        armor_damage = 6.0, armor_damage_div = 2.0, armor_piercing = 0.1,
        hull_damage = 8.0, hull_damage_div = 2.0,
    },
    cost = { minerals = 80, energy = 60 },
    build_time = 12,
    power_cost = 3,
}

-- Defense modules (defense slot)
local armor_plating = define_module {
    id = "armor_plating",
    name = "Armor Plating",
    slot_type = slot_types.defense,
    size = "small",
    modifiers = {
        { target = "ship.armor_max", base_add = 30.0 },
        { target = "ship.speed", multiplier = -0.05 },
    },
    cost = { minerals = 80 },
    build_time = 10,
    power_cost = 0,
}

local shield_generator = define_module {
    id = "shield_generator",
    name = "Shield Generator",
    slot_type = slot_types.defense,
    size = "small",
    prerequisites = has_tech(tech.military.deflector_shields),
    modifiers = {
        { target = "ship.shield_max", base_add = 40.0 },
        { target = "ship.shield_regen", base_add = 2.0 },
    },
    cost = { minerals = 60, energy = 50 },
    build_time = 12,
    power_cost = 3,
}

-- Utility modules (utility slot)
local survey_equipment = define_module {
    id = "survey_equipment",
    name = "Survey Equipment",
    slot_type = slot_types.utility,
    modifiers = {
        { target = "ship.survey_speed", base_add = 1.0 },
    },
    cost = { minerals = 60, energy = 40 },
    build_time = 8,
    power_cost = 0,
}

local cargo_bay = define_module {
    id = "cargo_bay",
    name = "Cargo Bay",
    slot_type = slot_types.utility,
    modifiers = {
        { target = "ship.cargo_capacity", base_add = 500.0 },
    },
    cost = { minerals = 30 },
    build_time = 5,
    power_cost = 0,
}

local colony_module = define_module {
    id = "colony_module",
    name = "Colony Module",
    slot_type = slot_types.utility,
    modifiers = {
        { target = "ship.colonize_speed", base_add = 1.0 },
    },
    cost = { minerals = 300, energy = 200 },
    build_time = 20,
    power_cost = 0,
}

-- #217: Scout module — enables the Scout command and extends passive sensor
-- range used by the observation snapshot. `sensor.range` feeds GlobalParams
-- so it also benefits survey range (intentional: scouts are spec'd as
-- sensor-heavy recon hulls).
local scout_module = define_module {
    id = "scout_module",
    name = "Scout Sensor Array",
    slot_type = slot_types.utility,
    modifiers = {
        { target = "sensor.range", base_add = 1.0 },
    },
    cost = { minerals = 80, energy = 50 },
    build_time = 8,
    power_cost = 0,
}

-- Reactor modules (reactor slot)
local fusion_reactor = define_module {
    id = "fusion_reactor",
    name = "Fusion Reactor",
    slot_type = slot_types.reactor,
    power_output = 10,
    modifiers = {
        { target = "ship.shield_regen", base_add = 0.5 },
    },
    cost = { minerals = 80, energy = 0 },
    build_time = 10,
}

-- Communications modules (comms slot)
local command_array = define_module {
    id = "command_array",
    name = "Fleet Command Array",
    slot_type = slot_types.comms,
    modifiers = {
        { target = "fleet.attack", multiplier = 0.05 },
        { target = "fleet.defense", multiplier = 0.05 },
    },
    cost = { minerals = 200, energy = 100 },
    build_time = 18,
    power_cost = 2,
}

return {
    ftl_drive = ftl_drive,
    afterburner = afterburner,
    ion_thruster = ion_thruster,
    weapon_laser = weapon_laser,
    weapon_railgun = weapon_railgun,
    weapon_missile = weapon_missile,
    armor_plating = armor_plating,
    shield_generator = shield_generator,
    survey_equipment = survey_equipment,
    cargo_bay = cargo_bay,
    colony_module = colony_module,
    scout_module = scout_module,
    fusion_reactor = fusion_reactor,
    command_array = command_array,
}
