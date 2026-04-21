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
--
-- #403: Size variants (S/M/L) for weapons, defense, and reactor modules.
-- Scaling rules per size tier (S→M→L):
--   Stats:        ~2x per tier
--   Cost:         ~2.3x per tier (larger = slightly less cost-efficient)
--   Power:        ~2x per tier
--   Weapon stats: larger = slightly more range/cooldown, slightly less tracking

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

---------------------------------------------------------------------------
-- Weapon modules (weapon slot) — S/M/L size variants
---------------------------------------------------------------------------

-- Laser Battery — balanced energy weapon, strong vs shields
local weapon_laser_s = define_module {
    id = "weapon_laser_s",
    name = "Small Laser Battery",
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

local weapon_laser_m = define_module {
    id = "weapon_laser_m",
    name = "Medium Laser Battery",
    slot_type = slot_types.weapon,
    size = "medium",
    prerequisites = has_tech(tech.military.kinetic_weapons),
    weapon = {
        track = 4.0, precision = 0.87, cooldown = 1, range = 12.0,
        shield_damage = 8.0, shield_damage_div = 2.0, shield_piercing = 0.0,
        armor_damage = 4.0, armor_damage_div = 1.0, armor_piercing = 0.0,
        hull_damage = 6.0, hull_damage_div = 2.0,
    },
    cost = { minerals = 115, energy = 70 },
    build_time = 12,
    power_cost = 4,
}

local weapon_laser_l = define_module {
    id = "weapon_laser_l",
    name = "Large Laser Battery",
    slot_type = slot_types.weapon,
    size = "large",
    prerequisites = has_tech(tech.military.kinetic_weapons),
    weapon = {
        track = 3.0, precision = 0.90, cooldown = 2, range = 14.0,
        shield_damage = 16.0, shield_damage_div = 4.0, shield_piercing = 0.0,
        armor_damage = 8.0, armor_damage_div = 2.0, armor_piercing = 0.0,
        hull_damage = 12.0, hull_damage_div = 4.0,
    },
    cost = { minerals = 265, energy = 160 },
    build_time = 14,
    power_cost = 8,
}

-- Railgun — high armor penetration, medium+ only
local weapon_railgun_m = define_module {
    id = "weapon_railgun_m",
    name = "Medium Railgun",
    slot_type = slot_types.weapon,
    size = "medium",
    prerequisites = has_tech(tech.military.kinetic_weapons),
    weapon = {
        track = 2.0, precision = 0.90, cooldown = 3, range = 20.0,
        shield_damage = 2.0, shield_damage_div = 0.5, shield_piercing = 0.5,
        armor_damage = 8.0, armor_damage_div = 2.0, armor_piercing = 0.3,
        hull_damage = 10.0, hull_damage_div = 3.0,
    },
    cost = { minerals = 100, energy = 50 },
    build_time = 15,
    power_cost = 5,
}

local weapon_railgun_l = define_module {
    id = "weapon_railgun_l",
    name = "Large Railgun",
    slot_type = slot_types.weapon,
    size = "large",
    prerequisites = has_tech(tech.military.kinetic_weapons),
    weapon = {
        track = 1.5, precision = 0.92, cooldown = 4, range = 25.0,
        shield_damage = 4.0, shield_damage_div = 1.0, shield_piercing = 0.5,
        armor_damage = 16.0, armor_damage_div = 4.0, armor_piercing = 0.3,
        hull_damage = 20.0, hull_damage_div = 6.0,
    },
    cost = { minerals = 230, energy = 115 },
    build_time = 18,
    power_cost = 10,
}

-- Missile Launcher — high shield pierce, good hull damage
local weapon_missile_s = define_module {
    id = "weapon_missile_s",
    name = "Small Missile Launcher",
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

local weapon_missile_m = define_module {
    id = "weapon_missile_m",
    name = "Medium Missile Launcher",
    slot_type = slot_types.weapon,
    size = "medium",
    prerequisites = has_tech(tech.military.kinetic_weapons),
    weapon = {
        track = 6.0, precision = 0.75, cooldown = 3, range = 18.0,
        shield_damage = 2.0, shield_damage_div = 1.0, shield_piercing = 0.8,
        armor_damage = 12.0, armor_damage_div = 4.0, armor_piercing = 0.1,
        hull_damage = 16.0, hull_damage_div = 4.0,
    },
    cost = { minerals = 184, energy = 138 },
    build_time = 14,
    power_cost = 6,
}

---------------------------------------------------------------------------
-- Defense modules (defense slot) — S/M/L size variants
---------------------------------------------------------------------------

-- Armor Plating — passive defense, no power cost, slight speed penalty
local armor_plating_s = define_module {
    id = "armor_plating_s",
    name = "Small Armor Plating",
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

local armor_plating_m = define_module {
    id = "armor_plating_m",
    name = "Medium Armor Plating",
    slot_type = slot_types.defense,
    size = "medium",
    modifiers = {
        { target = "ship.armor_max", base_add = 60.0 },
        { target = "ship.speed", multiplier = -0.08 },
    },
    cost = { minerals = 184 },
    build_time = 12,
    power_cost = 0,
}

local armor_plating_l = define_module {
    id = "armor_plating_l",
    name = "Large Armor Plating",
    slot_type = slot_types.defense,
    size = "large",
    modifiers = {
        { target = "ship.armor_max", base_add = 120.0 },
        { target = "ship.speed", multiplier = -0.10 },
    },
    cost = { minerals = 423 },
    build_time = 14,
    power_cost = 0,
}

-- Shield Generator — active defense, requires power
local shield_generator_s = define_module {
    id = "shield_generator_s",
    name = "Small Shield Generator",
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

local shield_generator_m = define_module {
    id = "shield_generator_m",
    name = "Medium Shield Generator",
    slot_type = slot_types.defense,
    size = "medium",
    prerequisites = has_tech(tech.military.deflector_shields),
    modifiers = {
        { target = "ship.shield_max", base_add = 80.0 },
        { target = "ship.shield_regen", base_add = 4.0 },
    },
    cost = { minerals = 138, energy = 115 },
    build_time = 14,
    power_cost = 6,
}

local shield_generator_l = define_module {
    id = "shield_generator_l",
    name = "Large Shield Generator",
    slot_type = slot_types.defense,
    size = "large",
    prerequisites = has_tech(tech.military.deflector_shields),
    modifiers = {
        { target = "ship.shield_max", base_add = 160.0 },
        { target = "ship.shield_regen", base_add = 8.0 },
    },
    cost = { minerals = 317, energy = 265 },
    build_time = 16,
    power_cost = 12,
}

---------------------------------------------------------------------------
-- Reactor modules (reactor slot) — S/M/L size variants
---------------------------------------------------------------------------

local fusion_reactor_s = define_module {
    id = "fusion_reactor_s",
    name = "Small Fusion Reactor",
    slot_type = slot_types.reactor,
    size = "small",
    power_output = 10,
    modifiers = {
        { target = "ship.shield_regen", base_add = 0.5 },
    },
    cost = { minerals = 80 },
    build_time = 10,
}

local fusion_reactor_m = define_module {
    id = "fusion_reactor_m",
    name = "Medium Fusion Reactor",
    slot_type = slot_types.reactor,
    size = "medium",
    power_output = 20,
    modifiers = {
        { target = "ship.shield_regen", base_add = 1.0 },
    },
    cost = { minerals = 184 },
    build_time = 12,
}

local fusion_reactor_l = define_module {
    id = "fusion_reactor_l",
    name = "Large Fusion Reactor",
    slot_type = slot_types.reactor,
    size = "large",
    power_output = 40,
    modifiers = {
        { target = "ship.shield_regen", base_add = 2.0 },
    },
    cost = { minerals = 423 },
    build_time = 14,
}

---------------------------------------------------------------------------
-- Utility modules (utility slot) — no size variants
---------------------------------------------------------------------------

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

---------------------------------------------------------------------------
-- Communications modules (comms slot)
---------------------------------------------------------------------------

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

---------------------------------------------------------------------------
-- Backward-compatibility aliases (old names → small variants)
---------------------------------------------------------------------------

local weapon_laser = weapon_laser_s
local weapon_railgun = weapon_railgun_m   -- railgun has no small variant; alias to medium
local weapon_missile = weapon_missile_s
local armor_plating = armor_plating_s
local shield_generator = shield_generator_s
local fusion_reactor = fusion_reactor_s

return {
    -- FTL / sublight
    ftl_drive = ftl_drive,
    afterburner = afterburner,
    ion_thruster = ion_thruster,

    -- Weapons — size variants
    weapon_laser_s = weapon_laser_s,
    weapon_laser_m = weapon_laser_m,
    weapon_laser_l = weapon_laser_l,
    weapon_railgun_m = weapon_railgun_m,
    weapon_railgun_l = weapon_railgun_l,
    weapon_missile_s = weapon_missile_s,
    weapon_missile_m = weapon_missile_m,

    -- Defense — size variants
    armor_plating_s = armor_plating_s,
    armor_plating_m = armor_plating_m,
    armor_plating_l = armor_plating_l,
    shield_generator_s = shield_generator_s,
    shield_generator_m = shield_generator_m,
    shield_generator_l = shield_generator_l,

    -- Reactors — size variants
    fusion_reactor_s = fusion_reactor_s,
    fusion_reactor_m = fusion_reactor_m,
    fusion_reactor_l = fusion_reactor_l,

    -- Utility
    survey_equipment = survey_equipment,
    cargo_bay = cargo_bay,
    colony_module = colony_module,
    scout_module = scout_module,

    -- Comms
    command_array = command_array,

    -- Backward-compat aliases (old unsized names)
    weapon_laser = weapon_laser,
    weapon_railgun = weapon_railgun,
    weapon_missile = weapon_missile,
    armor_plating = armor_plating,
    shield_generator = shield_generator,
    fusion_reactor = fusion_reactor,
}
