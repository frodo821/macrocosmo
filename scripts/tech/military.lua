-- Military branch technologies

define_tech {
    id = 400,
    name = "Kinetic Weapons",
    branch = "military",
    cost = 100,
    prerequisites = {},
    description = "Mass-driver based weapon systems",
    on_researched = function()
        -- TODO: push_empire_modifier("combat.weapon_damage", { multiplier = 0.1 })
    end,
}

define_tech {
    id = 401,
    name = "Deflector Shields",
    branch = "military",
    cost = 200,
    prerequisites = {},
    description = "Energy barriers to deflect incoming projectiles",
    on_researched = function()
        -- TODO: push_empire_modifier("combat.shield_strength", { multiplier = 0.15 })
    end,
}

define_tech {
    id = 402,
    name = "Composite Armor",
    branch = "military",
    cost = 250,
    prerequisites = { 400 },
    description = "Multi-layered hull plating for enhanced protection",
    on_researched = function()
        -- TODO: push_empire_modifier("combat.armor", { multiplier = 0.2 })
    end,
}
