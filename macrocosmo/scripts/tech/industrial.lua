-- Industrial branch technologies

local automated_mining = define_tech {
    id = "industrial_automated_mining",
    name = "Automated Mining",
    branch = "industrial",
    cost = { research = 100, minerals = 50 },
    prerequisites = {},
    description = "Robotic systems for autonomous resource extraction",
    on_researched = function(scope)
        scope:push_modifier("colony.minerals_per_hexadies", { multiplier = 0.15, description = "Automated Mining: +15% mineral production" })
        scope:set_flag("automated_mining_unlocked", true, { description = "Enables automated mining facilities" })
    end,
}

local orbital_fabrication = define_tech {
    id = "industrial_orbital_fabrication",
    name = "Orbital Fabrication",
    branch = "industrial",
    cost = 200,
    prerequisites = { automated_mining },
    description = "Manufacturing facilities in orbit for zero-gravity construction",
    on_researched = function(scope)
        scope:push_modifier("construction.speed", { multiplier = 0.10, description = "Orbital Fabrication: +10% construction speed" })
        scope:set_flag("orbital_fabrication_unlocked", true, { description = "Enables orbital fabrication yards" })
    end,
}

local fusion_power = define_tech {
    id = "industrial_fusion_power",
    name = "Fusion Power Plants",
    branch = "industrial",
    cost = 300,
    prerequisites = { automated_mining },
    description = "Harness fusion reactions for abundant clean energy",
    on_researched = function(scope)
        scope:push_modifier("colony.energy_per_hexadies", { multiplier = 0.20, description = "Fusion Power: +20% energy production" })
        scope:set_flag("fusion_power_unlocked", true, { description = "Enables fusion power plants" })
    end,
}

local nano_assembly = define_tech {
    id = "industrial_nano_assembly",
    name = "Nano-Assembly",
    branch = "industrial",
    cost = 500,
    prerequisites = { orbital_fabrication },
    description = "Molecular-scale construction for unprecedented precision",
    on_researched = function(scope)
        scope:push_modifier("construction.speed", { multiplier = 0.20, description = "Nano-Assembly: +20% construction speed" })
        scope:push_modifier("colony.minerals_per_hexadies", { multiplier = 0.10, description = "Nano-Assembly: +10% mineral production" })
    end,
}

return {
    automated_mining = automated_mining,
    orbital_fabrication = orbital_fabrication,
    fusion_power = fusion_power,
    nano_assembly = nano_assembly,
}
