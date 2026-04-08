-- Military branch technologies

define_tech {
    id = 400,
    name = "Kinetic Weapons",
    branch = "military",
    cost = 100,
    prerequisites = {},
    description = "Mass-driver based weapon systems",
    effects = {
        { type = "modify_weapon_damage", value = 0.1 },
    },
}

define_tech {
    id = 401,
    name = "Deflector Shields",
    branch = "military",
    cost = 200,
    prerequisites = {},
    description = "Energy barriers to deflect incoming projectiles",
    effects = {
        { type = "modify_shield_strength", value = 0.15 },
    },
}

define_tech {
    id = 402,
    name = "Composite Armor",
    branch = "military",
    cost = 250,
    prerequisites = { 400 },
    description = "Multi-layered hull plating for enhanced protection",
    effects = {
        { type = "modify_armor", value = 0.2 },
    },
}
