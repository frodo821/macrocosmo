define_planet_type {
    id = "terrestrial",
    name = "Terrestrial",
    base_habitability = 0.7,
    base_slots = 4,
    resource_bias = { minerals = 1.0, energy = 0.8, research = 0.5 },
    weight = 0.4,
}

define_planet_type {
    id = "ocean",
    name = "Ocean World",
    base_habitability = 0.6,
    base_slots = 3,
    resource_bias = { minerals = 0.3, energy = 0.5, research = 1.2 },
    weight = 0.15,
}

define_planet_type {
    id = "arid",
    name = "Arid World",
    base_habitability = 0.4,
    base_slots = 5,
    resource_bias = { minerals = 1.5, energy = 1.0, research = 0.3 },
    weight = 0.2,
}

define_planet_type {
    id = "gas_giant",
    name = "Gas Giant",
    base_habitability = 0.0,
    base_slots = 0,
    resource_bias = { minerals = 0.0, energy = 1.5, research = 1.0 },
    weight = 0.15,
}

define_planet_type {
    id = "barren",
    name = "Barren World",
    base_habitability = 0.15,
    base_slots = 2,
    resource_bias = { minerals = 1.5, energy = 0.5, research = 0.2 },
    weight = 0.1,
}
