-- #335: planet_type definitions may reference a biome via `default_biome`.
-- When a planet is spawned for this type, that biome id is attached as the
-- Planet entity's Biome component. Unspecified / unknown biomes fall back to
-- `"default"` (see resolve_biome_id).
local biomes = require("biomes")

define_planet_type {
    id = "terrestrial",
    name = "Terrestrial",
    base_habitability = 0.7,
    base_slots = 4,
    resource_bias = { minerals = 1.0, energy = 0.8, research = 0.5 },
    weight = 0.4,
    default_biome = biomes.temperate,
}

define_planet_type {
    id = "ocean",
    name = "Ocean World",
    base_habitability = 0.6,
    base_slots = 3,
    resource_bias = { minerals = 0.3, energy = 0.5, research = 1.2 },
    weight = 0.15,
    default_biome = biomes.oceanic,
}

define_planet_type {
    id = "arid",
    name = "Arid World",
    base_habitability = 0.4,
    base_slots = 5,
    resource_bias = { minerals = 1.5, energy = 1.0, research = 0.3 },
    weight = 0.2,
    default_biome = biomes.arid,
}

define_planet_type {
    id = "gas_giant",
    name = "Gas Giant",
    base_habitability = 0.0,
    base_slots = 0,
    resource_bias = { minerals = 0.0, energy = 1.5, research = 1.0 },
    weight = 0.15,
    default_biome = biomes.gas,
}

define_planet_type {
    id = "barren",
    name = "Barren World",
    base_habitability = 0.15,
    base_slots = 2,
    resource_bias = { minerals = 1.5, energy = 0.5, research = 0.2 },
    weight = 0.1,
    default_biome = biomes.tundra,
}
