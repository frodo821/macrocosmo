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
