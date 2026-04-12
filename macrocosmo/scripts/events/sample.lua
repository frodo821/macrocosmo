local harvest_ended = define_event {
    id = "harvest_ended",
    name = "End of Harvest",
    description = "The bountiful harvest season has ended.",
    -- Manual trigger (no trigger field)
}

local minor_asteroid_impact = define_event {
    id = "minor_asteroid_impact",
    name = "Minor Asteroid Impact",
    description = "A small asteroid has struck near a colony.",
    trigger = mtth_trigger {
        years = 10,
    },
}

local monthly_report = define_event {
    id = "monthly_report",
    name = "Monthly Report",
    description = "A regular status update.",
    trigger = periodic_trigger {
        months = 1,
    },
}

return {
    harvest_ended = harvest_ended,
    minor_asteroid_impact = minor_asteroid_impact,
    monthly_report = monthly_report,
}
