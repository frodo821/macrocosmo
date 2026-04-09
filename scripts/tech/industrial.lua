-- Industrial branch technologies

define_tech {
    id = 300,
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
    id = 301,
    name = "Orbital Fabrication",
    branch = "industrial",
    cost = 200,
    prerequisites = { 300 },
    description = "Manufacturing facilities in orbit for zero-gravity construction",
    on_researched = function()
        -- TODO: push_empire_modifier("construction.speed", { multiplier = 0.1 })
    end,
}

define_tech {
    id = 302,
    name = "Fusion Power Plants",
    branch = "industrial",
    cost = 300,
    prerequisites = { 300 },
    description = "Harness fusion reactions for abundant clean energy",
    on_researched = function()
        -- TODO: push_empire_modifier("production.energy", { multiplier = 0.2 })
    end,
}

define_tech {
    id = 303,
    name = "Nano-Assembly",
    branch = "industrial",
    cost = 500,
    prerequisites = { 301 },
    description = "Molecular-scale construction for unprecedented precision",
    on_researched = function()
        -- TODO: push_empire_modifier("construction.speed", { multiplier = 0.2 })
    end,
}
