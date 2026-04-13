-- #241: Jobs declare per-pop production via `modifiers`.
-- Targets without a `job:` prefix get auto-prefixed to `job:<self_id>::...` at
-- load time, which routes the modifier into the right per-job rate bucket.
local miner = define_job {
    id = "miner",
    label = "Miner",
    modifiers = { { target = "colony.minerals_per_hexadies", base_add = 0.6 } },
}

local farmer = define_job {
    id = "farmer",
    label = "Farmer",
    -- Bumped from 0.6 (old base_output) to 1.0 so a 5-slot farm still yields
    -- 5.0 food/hexady matching the prior building balance.
    modifiers = { { target = "colony.food_per_hexadies", base_add = 1.0 } },
}

local researcher = define_job {
    id = "researcher",
    label = "Researcher",
    modifiers = { { target = "colony.research_per_hexadies", base_add = 0.5 } },
}

local power_worker = define_job {
    id = "power_worker",
    label = "Power Worker",
    -- Bumped from 0.6 so a 5-slot power_plant still yields 30 energy/hexady.
    modifiers = { { target = "colony.energy_per_hexadies", base_add = 6.0 } },
}

return {
    miner = miner,
    farmer = farmer,
    researcher = researcher,
    power_worker = power_worker,
}
