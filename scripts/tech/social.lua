-- Social branch technologies

define_tech {
    id = "social_xenolinguistics",
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
    id = "social_colonial_admin",
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
    id = "social_interstellar_commerce",
    name = "Interstellar Commerce",
    branch = "social",
    cost = 250,
    prerequisites = { "social_colonial_admin" },
    description = "Trade frameworks spanning star systems",
    on_researched = function()
        -- TODO: push_empire_modifier("production.energy", { multiplier = 0.15 })
    end,
}

define_tech {
    id = "social_cultural_exchange",
    name = "Cultural Exchange Protocols",
    branch = "social",
    cost = 300,
    prerequisites = { "social_xenolinguistics" },
    description = "Formalised frameworks for cross-species cultural interaction",
    on_researched = function()
        -- TODO: push_empire_modifier("diplomacy.range", { add = 0.2 })
    end,
}
