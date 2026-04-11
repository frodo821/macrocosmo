-- Military branch technologies

local kinetic_weapons = define_tech {
    id = "military_kinetic_weapons",
    name = "Kinetic Weapons",
    branch = "military",
    cost = 100,
    prerequisites = {},
    description = "Mass-driver based weapon systems",
    on_researched = function(scope)
        scope:push_modifier("combat.weapon_damage", { multiplier = 0.10, description = "Kinetic Weapons: +10% weapon damage" })
        scope:set_flag("kinetic_weapons_unlocked", true, { description = "Enables kinetic weapon modules" })
    end,
}

local deflector_shields = define_tech {
    id = "military_deflector_shields",
    name = "Deflector Shields",
    branch = "military",
    cost = 200,
    prerequisites = {},
    description = "Energy barriers to deflect incoming projectiles",
    on_researched = function(scope)
        scope:push_modifier("combat.shield_strength", { multiplier = 0.15, description = "Deflector Shields: +15% shield strength" })
        scope:set_flag("deflector_shields_unlocked", true, { description = "Enables deflector shield modules" })
    end,
}

local composite_armor = define_tech {
    id = "military_composite_armor",
    name = "Composite Armor",
    branch = "military",
    cost = 250,
    prerequisites = { kinetic_weapons },
    description = "Multi-layered hull plating for enhanced protection",
    on_researched = function(scope)
        scope:push_modifier("combat.armor", { multiplier = 0.20, description = "Composite Armor: +20% armor strength" })
        scope:set_flag("composite_armor_unlocked", true, { description = "Enables composite armor modules" })
    end,
}

return {
    kinetic_weapons = kinetic_weapons,
    deflector_shields = deflector_shields,
    composite_armor = composite_armor,
}
