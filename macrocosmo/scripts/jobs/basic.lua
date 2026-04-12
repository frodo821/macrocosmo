local miner = define_job {
    id = "miner",
    label = "Miner",
    base_output = { minerals = 0.6 },
}

local farmer = define_job {
    id = "farmer",
    label = "Farmer",
    base_output = { food = 0.6 },
}

local researcher = define_job {
    id = "researcher",
    label = "Researcher",
    base_output = { research = 0.5 },
}

local power_worker = define_job {
    id = "power_worker",
    label = "Power Worker",
    base_output = { energy = 0.6 },
}

return {
    miner = miner,
    farmer = farmer,
    researcher = researcher,
    power_worker = power_worker,
}
