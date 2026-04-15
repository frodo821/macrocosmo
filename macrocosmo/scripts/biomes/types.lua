-- #335: Base biome definitions.
--
-- Biome is decoupled from planet_type so multiple planet_types may share a
-- biome, and future biome-dependent gates (production bias, habitability,
-- terraforming) don't require per-planet-type changes.
--
-- Each `define_biome` returns a reference table that scripts can pass to
-- `default_biome = ...` on a `define_planet_type` call.

local default = define_biome {
    id = "default",
    display_name = "Default",
    description = "Fallback biome for planet_types that don't declare a specific biome.",
}

local temperate = define_biome {
    id = "temperate",
    display_name = "Temperate",
    description = "Mild climate with liquid water and moderate seasons.",
}

local arid = define_biome {
    id = "arid",
    display_name = "Arid",
    description = "Hot, dry surface with sparse surface water.",
}

local oceanic = define_biome {
    id = "oceanic",
    display_name = "Oceanic",
    description = "Surface dominated by deep liquid oceans.",
}

local tundra = define_biome {
    id = "tundra",
    display_name = "Tundra",
    description = "Cold, ice-locked surface with permafrost.",
}

local volcanic = define_biome {
    id = "volcanic",
    display_name = "Volcanic",
    description = "Molten and geologically active; lethal to unshielded life.",
}

local gas = define_biome {
    id = "gas",
    display_name = "Gaseous",
    description = "No solid surface; thick atmosphere of hydrogen and helium.",
}

return {
    default = default,
    temperate = temperate,
    arid = arid,
    oceanic = oceanic,
    tundra = tundra,
    volcanic = volcanic,
    gas = gas,
}
