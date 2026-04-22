local research_lab = define_building {
    id = "research_lab",
    name = "Laboratory",
    description = "Conducts scientific research",
    cost = { minerals = 100, energy = 200 },
    build_time = 15,
    maintenance = 0.5,
    -- 5 researchers × 0.2 research/pop = 1.0 research
    modifiers = {
        { target = "colony.researcher_slot", base_add = 5 },
    },
    is_system_building = false,
    upgrade_to = {
        { target = forward_ref("research_lab_t2"), cost = { minerals = 150, energy = 200 }, build_time = 15 },
    },
}

local research_lab_t2 = define_building {
    id = "research_lab_t2",
    name = "Advanced Laboratory",
    description = "More sophisticated research facility that can hire more researchers",
    cost = nil,
    build_time = 15,
    maintenance = 0.5,
    -- 10 researchers × 0.2 research/pop = 2.0 research
    modifiers = {
        { target = "colony.researcher_slot", base_add = 10 },
    },
    is_system_building = false,
    upgrade_to = {
        { target = forward_ref("research_lab_t3"), cost = { minerals = 200, energy = 300 }, build_time = 20 },
    },
}

local research_lab_t3 = define_building {
    id = "research_lab_t3",
    name = "Innovation Facility",
    description = "A facility where top scientists push the boundaries of knowledge. Provides a significant boost to research output.",
    cost = nil,
    build_time = 15,
    maintenance = 0.5,
    -- 5 researchers × 0.2 research/pop = 1.0 research
    modifiers = {
        { target = "colony.researcher_slot", base_add = 5 },
    },
    is_system_building = false,
}

return {
    t1 = research_lab,
    t2 = research_lab_t2,
    t3 = research_lab_t3,
}