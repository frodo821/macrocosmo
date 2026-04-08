-- Industrial branch technologies

define_tech {
    id = 300,
    name = "Automated Mining",
    branch = "industrial",
    cost = 100,
    prerequisites = {},
    description = "Robotic systems for autonomous resource extraction",
    effects = {
        { type = "modify_resource_production", resource = "minerals", value = 0.15 },
    },
}

define_tech {
    id = 301,
    name = "Orbital Fabrication",
    branch = "industrial",
    cost = 200,
    prerequisites = { 300 },
    description = "Manufacturing facilities in orbit for zero-gravity construction",
    effects = {
        { type = "modify_construction_speed", value = 0.1 },
    },
}

define_tech {
    id = 302,
    name = "Fusion Power Plants",
    branch = "industrial",
    cost = 300,
    prerequisites = { 300 },
    description = "Harness fusion reactions for abundant clean energy",
    effects = {
        { type = "modify_resource_production", resource = "energy", value = 0.2 },
    },
}

define_tech {
    id = 303,
    name = "Nano-Assembly",
    branch = "industrial",
    cost = 500,
    prerequisites = { 301 },
    description = "Molecular-scale construction for unprecedented precision",
    effects = {
        { type = "modify_construction_speed", value = 0.2 },
    },
}
