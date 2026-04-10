-- Industrial branch technologies

define_tech {
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

define_tech {
    id = "industrial_orbital_fabrication",
    name = "Orbital Fabrication",
    branch = "industrial",
    cost = 200,
    prerequisites = { "industrial_automated_mining" },
    description = "Manufacturing facilities in orbit for zero-gravity construction",
    on_researched = function()
        -- TODO: push_empire_modifier("construction.speed", { multiplier = 0.1 })
    end,
}

define_tech {
    id = "industrial_fusion_power",
    name = "Fusion Power Plants",
    branch = "industrial",
    cost = 300,
    prerequisites = { "industrial_automated_mining" },
    description = "Harness fusion reactions for abundant clean energy",
    on_researched = function()
        -- TODO: push_empire_modifier("production.energy", { multiplier = 0.2 })
    end,
}

define_tech {
    id = "industrial_nano_assembly",
    name = "Nano-Assembly",
    branch = "industrial",
    cost = 500,
    prerequisites = { "industrial_orbital_fabrication" },
    description = "Molecular-scale construction for unprecedented precision",
    on_researched = function()
        -- TODO: push_empire_modifier("construction.speed", { multiplier = 0.2 })
    end,
}
