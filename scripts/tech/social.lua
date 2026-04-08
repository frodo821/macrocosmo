-- Social branch technologies

define_tech {
    id = 100,
    name = "Xenolinguistics",
    branch = "social",
    cost = 100,
    prerequisites = {},
    description = "Foundational study of alien communication patterns",
    effects = {
        { type = "modify_diplomacy_range", value = 0.1 },
    },
}

define_tech {
    id = 101,
    name = "Colonial Administration",
    branch = "social",
    cost = 150,
    prerequisites = {},
    description = "Improved governance structures for distant colonies",
    effects = {
        { type = "modify_population_growth", value = 0.1 },
    },
}

define_tech {
    id = 102,
    name = "Interstellar Commerce",
    branch = "social",
    cost = 250,
    prerequisites = { 101 },
    description = "Trade frameworks spanning star systems",
    effects = {
        { type = "modify_resource_production", resource = "energy", value = 0.15 },
    },
}

define_tech {
    id = 103,
    name = "Cultural Exchange Protocols",
    branch = "social",
    cost = 300,
    prerequisites = { 100 },
    description = "Formalised frameworks for cross-species cultural interaction",
    effects = {
        { type = "modify_diplomacy_range", value = 0.2 },
    },
}
