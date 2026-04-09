define_event {
    id = "harvest_ended",
    name = "End of Harvest",
    description = "The bountiful harvest season has ended.",
    -- Manual trigger (no trigger field)
}

define_event {
    id = "minor_asteroid_impact",
    name = "Minor Asteroid Impact",
    description = "A small asteroid has struck near a colony.",
    trigger = mtth_trigger {
        years = 10,
    },
}

define_event {
    id = "monthly_report",
    name = "Monthly Report",
    description = "A regular status update.",
    trigger = periodic_trigger {
        months = 1,
    },
}
