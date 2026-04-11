-- Industrial branch technologies

local automated_mining = define_tech {
    id = "industrial_automated_mining",
    name = "Automated Mining",
    branch = "industrial",
    cost = { research = 100, minerals = 50 },
    prerequisites = {},
    description = "Robotic systems for autonomous resource extraction",
    on_researched = function()
        -- TODO: push_empire_modifier("production.minerals", { multiplier = 0.15 })
    end,
}

local orbital_fabrication = define_tech {
    id = "industrial_orbital_fabrication",
    name = "Orbital Fabrication",
    branch = "industrial",
    cost = 200,
    prerequisites = { automated_mining },
    description = "Manufacturing facilities in orbit for zero-gravity construction",
    on_researched = function()
        -- TODO: push_empire_modifier("construction.speed", { multiplier = 0.1 })
    end,
}

local fusion_power = define_tech {
    id = "industrial_fusion_power",
    name = "Fusion Power Plants",
    branch = "industrial",
    cost = 300,
    prerequisites = { automated_mining },
    description = "Harness fusion reactions for abundant clean energy",
    on_researched = function()
        -- TODO: push_empire_modifier("production.energy", { multiplier = 0.2 })
    end,
}

local nano_assembly = define_tech {
    id = "industrial_nano_assembly",
    name = "Nano-Assembly",
    branch = "industrial",
    cost = 500,
    prerequisites = { orbital_fabrication },
    description = "Molecular-scale construction for unprecedented precision",
    on_researched = function()
        -- TODO: push_empire_modifier("construction.speed", { multiplier = 0.2 })
    end,
}

return {
    automated_mining = automated_mining,
    orbital_fabrication = orbital_fabrication,
    fusion_power = fusion_power,
    nano_assembly = nano_assembly,
}
