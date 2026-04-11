-- Social branch technologies

local xenolinguistics = define_tech {
    id = "social_xenolinguistics",
    name = "Xenolinguistics",
    branch = "social",
    cost = 100,
    prerequisites = {},
    description = "Foundational study of alien communication patterns",
    on_researched = function(scope)
        scope:push_modifier("diplomacy.range", { add = 1.0, description = "Xenolinguistics: +1 diplomacy range" })
        scope:set_flag("xenolinguistics_unlocked", true, { description = "Enables alien communication" })
    end,
}

local colonial_admin = define_tech {
    id = "social_colonial_admin",
    name = "Colonial Administration",
    branch = "social",
    cost = 150,
    prerequisites = {},
    description = "Improved governance structures for distant colonies",
    on_researched = function(scope)
        scope:push_modifier("population.growth", { multiplier = 0.10, description = "Colonial Admin: +10% population growth" })
    end,
}

local interstellar_commerce = define_tech {
    id = "social_interstellar_commerce",
    name = "Interstellar Commerce",
    branch = "social",
    cost = 250,
    prerequisites = { colonial_admin },
    description = "Trade frameworks spanning star systems",
    on_researched = function(scope)
        scope:push_modifier("production.energy", { multiplier = 0.15, description = "Interstellar Commerce: +15% energy production" })
        scope:set_flag("interstellar_commerce_unlocked", true, { description = "Enables interstellar trade routes" })
    end,
}

local cultural_exchange = define_tech {
    id = "social_cultural_exchange",
    name = "Cultural Exchange Protocols",
    branch = "social",
    cost = 300,
    prerequisites = { xenolinguistics },
    description = "Formalised frameworks for cross-species cultural interaction",
    on_researched = function(scope)
        scope:push_modifier("diplomacy.range", { add = 2.0, description = "Cultural Exchange: +2 diplomacy range" })
        scope:push_modifier("population.growth", { multiplier = 0.05, description = "Cultural Exchange: +5% population growth" })
    end,
}

return {
    xenolinguistics = xenolinguistics,
    colonial_admin = colonial_admin,
    interstellar_commerce = interstellar_commerce,
    cultural_exchange = cultural_exchange,
}
