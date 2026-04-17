-- #241: Buildings declare `modifiers` with target strings.
-- - `colony.<job>_slot` grants slot capacity (labor-intensive buildings)
-- - `colony.<resource>_per_hexadies` directly contributes to production
--   (fully-automated buildings — no pop required)
-- The legacy `production_bonus = { ... }` field is warn-then-ignored.
local mine = define_building {
    id = "mine",
    name = "Mine",
    description = "Extracts minerals from planetary deposits (labour-intensive)",
    cost = { minerals = 150, energy = 50 },
    build_time = 10,
    maintenance = 0.2,
    modifiers = {
        { target = "colony.miner_slot", base_add = 5 },
    },
    is_system_building = false,
    upgrade_to = {
        { target = forward_ref("advanced_mine"), cost = { minerals = 200, energy = 100 }, build_time = 8 },
    },
}

local power_plant = define_building {
    id = "power_plant",
    name = "PowerPlant",
    description = "Generates energy from local resources (labour-intensive)",
    cost = { minerals = 50, energy = 150 },
    build_time = 10,
    maintenance = 0.0,
    -- 5 power workers × 6.0 energy/pop = 30 energy, matches #236 balance.
    modifiers = {
        { target = "colony.power_worker_slot", base_add = 5 },
    },
    is_system_building = false,
    upgrade_to = {
        { target = forward_ref("advanced_power_plant"), cost = { minerals = 150, energy = 200 }, build_time = 10 },
    },
}

local research_lab = define_building {
    id = "research_lab",
    name = "ResearchLab",
    description = "Conducts scientific research (labour-intensive)",
    cost = { minerals = 100, energy = 100 },
    build_time = 15,
    maintenance = 0.5,
    -- 4 researchers × 0.5 research/pop = 2.0 research, matches prior balance.
    modifiers = {
        { target = "colony.researcher_slot", base_add = 4 },
    },
    is_system_building = true,
}

local shipyard = define_building {
    id = "shipyard",
    name = "Shipyard",
    description = "Constructs and refits ships",
    cost = { minerals = 300, energy = 200 },
    build_time = 30,
    maintenance = 1.0,
    is_system_building = true,
    capabilities = { shipyard = { concurrent_builds = 1 } },
}

local port = define_building {
    id = "port",
    name = "Port",
    description = "Reduces FTL travel time from this system",
    cost = { minerals = 400, energy = 300 },
    build_time = 40,
    maintenance = 0.5,
    is_system_building = true,
    capabilities = { port = { ftl_range_bonus = 10.0, travel_time_factor = 0.8 } },
}

local farm = define_building {
    id = "farm",
    name = "Farm",
    description = "Produces food to sustain population (labour-intensive)",
    cost = { minerals = 100, energy = 50 },
    build_time = 20,
    maintenance = 0.3,
    -- 5 farmers × 1.0 food/pop = 5.0 food, matches prior balance.
    modifiers = {
        { target = "colony.farmer_slot", base_add = 5 },
    },
    is_system_building = false,
}

-- Upgrade-only buildings (not directly buildable)
local advanced_mine = define_building {
    id = "advanced_mine",
    name = "Advanced Mine",
    description = "Automated extraction with higher mineral yield",
    cost = nil,
    build_time = 10,
    maintenance = 0.4,
    -- 10 miners × 0.6 minerals/pop = 6.0, double the basic mine.
    modifiers = {
        { target = "colony.miner_slot", base_add = 10 },
    },
    is_system_building = false,
    prerequisites = has_tech(forward_ref("industrial_automated_mining")),
}

local advanced_power_plant = define_building {
    id = "advanced_power_plant",
    name = "Advanced PowerPlant",
    description = "Fusion-powered energy generation with higher output",
    cost = nil,
    build_time = 10,
    maintenance = 0.2,
    -- 10 power workers × 6.0 energy/pop = 60.0, double the basic plant.
    modifiers = {
        { target = "colony.power_worker_slot", base_add = 10 },
    },
    is_system_building = false,
    prerequisites = has_tech(forward_ref("industrial_fusion_power")),
}

-- #280: Colony Hub buildings — auto-placed on colonization, non-dismantlable.
-- Hub provides base building slot capacity via the `colony_hub` capability.
local colony_hub_t1 = define_building {
    id = "colony_hub_t1",
    name = "Colony Hub",
    description = "Basic colony administration hub. Provides initial building slots.",
    cost = nil,
    build_time = 10,
    maintenance = 0.0,
    dismantlable = false,
    is_system_building = false,
    capabilities = { colony_hub = { fixed_slots = 4 } },
    modifiers = {
        { target = "colony.farmer_slot", base_add = 1 },
        { target = "colony.power_worker_slot", base_add = 1 },
    },
    upgrade_to = {
        { target = forward_ref("colony_hub_t2"), cost = { minerals = 200, energy = 100 }, build_time = 15 },
    },
}

local colony_hub_t2 = define_building {
    id = "colony_hub_t2",
    name = "Colony Hub II",
    description = "Upgraded administration hub with expanded building capacity.",
    cost = nil,
    build_time = 15,
    maintenance = 0.0,
    dismantlable = false,
    is_system_building = false,
    capabilities = { colony_hub = { fixed_slots = 6 } },
    modifiers = {
        { target = "colony.farmer_slot", base_add = 2 },
        { target = "colony.power_worker_slot", base_add = 2 },
    },
    upgrade_to = {
        { target = forward_ref("colony_hub_t3"), cost = { minerals = 400, energy = 200 }, build_time = 20 },
    },
}

local colony_hub_t3 = define_building {
    id = "colony_hub_t3",
    name = "Colony Hub III",
    description = "Advanced administration hub. Can be upgraded to a Planetary Capital.",
    cost = nil,
    build_time = 20,
    maintenance = 0.0,
    dismantlable = false,
    is_system_building = false,
    capabilities = { colony_hub = { fixed_slots = 8 } },
    modifiers = {
        { target = "colony.farmer_slot", base_add = 3 },
        { target = "colony.power_worker_slot", base_add = 3 },
    },
    prerequisites = has_tech(forward_ref("industrial_automated_mining")),
    upgrade_to = {
        { target = forward_ref("planetary_capital_t1"), cost = { minerals = 600, energy = 400 }, build_time = 30 },
    },
}

-- #280: Planetary Capital buildings — enhanced hub for capital/major colonies.
local planetary_capital_t1 = define_building {
    id = "planetary_capital_t1",
    name = "Planetary Capital",
    description = "Seat of planetary government. Provides substantial building capacity and research bonus.",
    cost = nil,
    build_time = 30,
    maintenance = 0.0,
    dismantlable = false,
    is_system_building = false,
    capabilities = { colony_hub = { fixed_slots = 10 } },
    modifiers = {
        { target = "colony.farmer_slot", base_add = 3 },
        { target = "colony.power_worker_slot", base_add = 3 },
        { target = "colony.research_per_hexadies", base_add = 1.0 },
    },
    upgrade_to = {
        { target = forward_ref("planetary_capital_t2"), cost = { minerals = 800, energy = 600 }, build_time = 40 },
    },
}

local planetary_capital_t2 = define_building {
    id = "planetary_capital_t2",
    name = "Planetary Capital II",
    description = "Expanded capital complex with enhanced administration.",
    cost = nil,
    build_time = 40,
    maintenance = 0.0,
    dismantlable = false,
    is_system_building = false,
    capabilities = { colony_hub = { fixed_slots = 12 } },
    modifiers = {
        { target = "colony.farmer_slot", base_add = 3 },
        { target = "colony.power_worker_slot", base_add = 3 },
        { target = "colony.research_per_hexadies", base_add = 2.0 },
    },
    upgrade_to = {
        { target = forward_ref("planetary_capital_t3"), cost = { minerals = 1000, energy = 800 }, build_time = 50 },
    },
}

local planetary_capital_t3 = define_building {
    id = "planetary_capital_t3",
    name = "Planetary Capital III",
    description = "Apex of planetary governance. Maximum building capacity and research output.",
    cost = nil,
    build_time = 50,
    maintenance = 0.0,
    dismantlable = false,
    is_system_building = false,
    capabilities = { colony_hub = { fixed_slots = 14, slot_ratio = 0.15 } },
    modifiers = {
        { target = "colony.farmer_slot", base_add = 3 },
        { target = "colony.power_worker_slot", base_add = 3 },
        { target = "colony.research_per_hexadies", base_add = 3.0 },
    },
}

return {
    mine = mine,
    power_plant = power_plant,
    research_lab = research_lab,
    shipyard = shipyard,
    port = port,
    farm = farm,
    advanced_mine = advanced_mine,
    advanced_power_plant = advanced_power_plant,
    colony_hub_t1 = colony_hub_t1,
    colony_hub_t2 = colony_hub_t2,
    colony_hub_t3 = colony_hub_t3,
    planetary_capital_t1 = planetary_capital_t1,
    planetary_capital_t2 = planetary_capital_t2,
    planetary_capital_t3 = planetary_capital_t3,
}
