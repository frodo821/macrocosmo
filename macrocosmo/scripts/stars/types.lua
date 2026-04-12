define_star_type {
    id = "yellow_dwarf",
    name = "Yellow Dwarf",
    color = { r = 1.0, g = 0.9, b = 0.7 },
    planet_lambda = 2.5,
    max_planets = 8,
    habitability_bonus = 0.0,
    weight = 0.5,
}

define_star_type {
    id = "red_dwarf",
    name = "Red Dwarf",
    color = { r = 1.0, g = 0.5, b = 0.3 },
    planet_lambda = 1.5,
    max_planets = 5,
    habitability_bonus = -0.2,
    weight = 0.7,
}

define_star_type {
    id = "blue_giant",
    name = "Blue Giant",
    color = { r = 0.6, g = 0.7, b = 1.0 },
    planet_lambda = 4.0,
    max_planets = 12,
    habitability_bonus = -0.5,
    weight = 0.1,
}

define_star_type {
    id = "white_dwarf",
    name = "White Dwarf",
    color = { r = 1.0, g = 1.0, b = 1.0 },
    planet_lambda = 1.0,
    max_planets = 3,
    habitability_bonus = -0.3,
    weight = 0.2,
}

define_star_type {
    id = "orange_giant",
    name = "Orange Giant",
    color = { r = 1.0, g = 0.7, b = 0.3 },
    planet_lambda = 3.0,
    max_planets = 10,
    habitability_bonus = -0.1,
    weight = 0.3,
}

-- Exotic remnants and compact objects.
-- Unknown modifier targets are retained for future wiring — they are
-- preserved in StarTypeModifierSet even if no typed scope consumes them yet.

define_star_type {
    id = "neutron_star",
    name = "Neutron Star",
    description = "Stellar remnant of extreme density. Uninhabitable, but its intense radiation field makes it a rich source of energy and exotic research data.",
    color = { r = 0.8, g = 0.9, b = 1.0 },
    planet_lambda = 0.5,
    max_planets = 2,
    habitability_bonus = -1.0,
    weight = 0.05,
    modifiers = {
        { target = "system.energy_potential", multiplier = 0.5 },
        { target = "system.research_bonus", multiplier = 0.3 },
    },
}

define_star_type {
    id = "pulsar",
    name = "Pulsar",
    description = "A rapidly rotating neutron star whose lighthouse beams disrupt FTL and communications. Compensated by unusually high anomaly discovery rates.",
    color = { r = 0.7, g = 0.8, b = 1.0 },
    planet_lambda = 0.5,
    max_planets = 2,
    habitability_bonus = -1.0,
    weight = 0.04,
    modifiers = {
        { target = "system.ftl_range", multiplier = -0.3 },
        { target = "system.comm_delay", multiplier = 0.5 },
        { target = "system.anomaly_chance", multiplier = 1.0 },
    },
}

define_star_type {
    id = "magnetar",
    name = "Magnetar",
    description = "A neutron star with a magnetic field so extreme it disables most shielding technology. Abundant energy, but deadly to unprotected vessels.",
    color = { r = 0.9, g = 0.5, b = 1.0 },
    planet_lambda = 0.3,
    max_planets = 1,
    habitability_bonus = -1.0,
    weight = 0.03,
    modifiers = {
        { target = "ship.shield_max", multiplier = -1.0 },
        { target = "ship.shield_regen", multiplier = -1.0 },
        { target = "system.energy_potential", multiplier = 0.8 },
    },
}

define_star_type {
    id = "black_hole",
    name = "Black Hole",
    description = "An event horizon warps spacetime around it. No planets survive, but the local distortion of spacetime offers unique insights into FTL physics.",
    color = { r = 0.1, g = 0.05, b = 0.2 },
    planet_lambda = 0.0,
    max_planets = 0,
    habitability_bonus = -1.0,
    weight = 0.02,
    modifiers = {
        { target = "system.research_bonus", multiplier = 0.5 },
        { target = "ftl.research_bonus", multiplier = 1.0 },
        { target = "system.ftl_range", multiplier = -0.5 },
    },
}

define_star_type {
    id = "binary_star",
    name = "Binary Star",
    description = "Two stars locked in a gravitational dance. Unstable orbits reduce habitability, but tidal interactions seed rich mineral and energy deposits.",
    color = { r = 1.0, g = 0.85, b = 0.6 },
    planet_lambda = 2.5,
    max_planets = 6,
    habitability_bonus = -0.3,
    weight = 0.15,
    modifiers = {
        { target = "system.mineral_richness", multiplier = 0.4 },
        { target = "system.energy_potential", multiplier = 0.3 },
    },
}
