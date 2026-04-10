-- Engine modules
define_module {
    id = "ftl_drive",
    name = "FTL Drive",
    slot_type = "engine",
    modifiers = {
        { target = "ship.ftl_range", base_add = 15.0 },
    },
    cost = { minerals = 100, energy = 50 },
}

define_module {
    id = "afterburner",
    name = "Afterburner",
    slot_type = "engine",
    modifiers = {
        { target = "ship.speed", multiplier = 0.2 },
    },
    cost = { minerals = 60, energy = 40 },
}

-- Weapon modules
define_module {
    id = "weapon_laser",
    name = "Laser Battery",
    slot_type = "weapon",
    weapon = {
        track = 5.0, precision = 0.85, cooldown = 1, range = 10.0,
        shield_damage = 4.0, shield_damage_div = 1.0, shield_piercing = 0.0,
        armor_damage = 2.0, armor_damage_div = 0.5, armor_piercing = 0.0,
        hull_damage = 3.0, hull_damage_div = 1.0,
    },
    cost = { minerals = 50, energy = 30 },
}

define_module {
    id = "weapon_railgun",
    name = "Railgun",
    slot_type = "weapon",
    weapon = {
        track = 2.0, precision = 0.90, cooldown = 3, range = 20.0,
        shield_damage = 1.0, shield_damage_div = 0.5, shield_piercing = 0.5,
        armor_damage = 8.0, armor_damage_div = 2.0, armor_piercing = 0.3,
        hull_damage = 10.0, hull_damage_div = 3.0,
    },
    cost = { minerals = 100, energy = 50 },
}

define_module {
    id = "weapon_missile",
    name = "Missile Launcher",
    slot_type = "weapon",
    weapon = {
        track = 8.0, precision = 0.70, cooldown = 2, range = 15.0,
        shield_damage = 1.0, shield_damage_div = 0.5, shield_piercing = 0.8,
        armor_damage = 6.0, armor_damage_div = 2.0, armor_piercing = 0.1,
        hull_damage = 8.0, hull_damage_div = 2.0,
    },
    cost = { minerals = 80, energy = 60 },
}

-- Utility modules
define_module {
    id = "armor_plating",
    name = "Armor Plating",
    slot_type = "utility",
    modifiers = {
        { target = "ship.armor_max", base_add = 30.0 },
        { target = "ship.speed", multiplier = -0.05 },
    },
    cost = { minerals = 80 },
}

define_module {
    id = "shield_generator",
    name = "Shield Generator",
    slot_type = "utility",
    modifiers = {
        { target = "ship.shield_max", base_add = 40.0 },
        { target = "ship.shield_regen", base_add = 2.0 },
    },
    cost = { minerals = 60, energy = 50 },
}

define_module {
    id = "survey_equipment",
    name = "Survey Equipment",
    slot_type = "utility",
    modifiers = {
        { target = "ship.survey_speed", base_add = 1.0 },
    },
    cost = { minerals = 60, energy = 40 },
}

define_module {
    id = "cargo_bay",
    name = "Cargo Bay",
    slot_type = "utility",
    modifiers = {
        { target = "ship.cargo_capacity", base_add = 500.0 },
    },
    cost = { minerals = 30 },
}

-- Special modules
define_module {
    id = "colony_module",
    name = "Colony Module",
    slot_type = "special",
    modifiers = {
        { target = "ship.colonize_speed", base_add = 1.0 },
    },
    cost = { minerals = 300, energy = 200 },
}

define_module {
    id = "command_array",
    name = "Fleet Command Array",
    slot_type = "special",
    modifiers = {
        { target = "fleet.attack", multiplier = 0.05 },
        { target = "fleet.defense", multiplier = 0.05 },
    },
    cost = { minerals = 200, energy = 100 },
}
