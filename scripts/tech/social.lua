-- Social branch technologies

define_tech {
    id = 100,
    name = "Xenolinguistics",
    branch = "social",
    cost = 100,
    prerequisites = {},
    description = "Foundational study of alien communication patterns",
    on_researched = function()
        -- TODO: push_empire_modifier("diplomacy.range", { add = 0.1 })
    end,
}

define_tech {
    id = 101,
    name = "Colonial Administration",
    branch = "social",
    cost = 150,
    prerequisites = {},
    description = "Improved governance structures for distant colonies",
    on_researched = function()
        -- TODO: push_empire_modifier("population.growth", { add = 0.1 })
    end,
}

define_tech {
    id = 102,
    name = "Interstellar Commerce",
    branch = "social",
    cost = 250,
    prerequisites = { 101 },
    description = "Trade frameworks spanning star systems",
    on_researched = function()
        -- TODO: push_empire_modifier("production.energy", { multiplier = 0.15 })
    end,
}

define_tech {
    id = 103,
    name = "Cultural Exchange Protocols",
    branch = "social",
    cost = 300,
    prerequisites = { 100 },
    description = "Formalised frameworks for cross-species cultural interaction",
    on_researched = function()
        -- TODO: push_empire_modifier("diplomacy.range", { add = 0.2 })
    end,
}
