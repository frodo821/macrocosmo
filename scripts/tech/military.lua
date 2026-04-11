-- Military branch technologies

local kinetic_weapons = define_tech {
    id = "military_kinetic_weapons",
    name = "Kinetic Weapons",
    branch = "military",
    cost = 100,
    prerequisites = {},
    description = "Mass-driver based weapon systems",
    on_researched = function()
        -- TODO: push_empire_modifier("combat.weapon_damage", { multiplier = 0.1 })
    end,
}

local deflector_shields = define_tech {
    id = "military_deflector_shields",
    name = "Deflector Shields",
    branch = "military",
    cost = 200,
    prerequisites = {},
    description = "Energy barriers to deflect incoming projectiles",
    on_researched = function()
        -- TODO: push_empire_modifier("combat.shield_strength", { multiplier = 0.15 })
    end,
}

local composite_armor = define_tech {
    id = "military_composite_armor",
    name = "Composite Armor",
    branch = "military",
    cost = 250,
    prerequisites = { kinetic_weapons },
    description = "Multi-layered hull plating for enhanced protection",
    on_researched = function()
        -- TODO: push_empire_modifier("combat.armor", { multiplier = 0.2 })
    end,
}

return {
    kinetic_weapons = kinetic_weapons,
    deflector_shields = deflector_shields,
    composite_armor = composite_armor,
}
