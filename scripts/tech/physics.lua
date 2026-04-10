-- Physics branch technologies

define_tech {
    id = "physics_sensor_arrays",
    name = "Advanced Sensor Arrays",
    branch = "physics",
    cost = 100,
    prerequisites = {},
    description = "Next-generation sensors for deep space observation",
    on_researched = function()
        -- TODO: push_empire_modifier("sensor.range", { add = 0.2 })
    end,
}

define_tech {
    id = "physics_sublight_drives",
    name = "Improved Sublight Drives",
    branch = "physics",
    cost = 200,
    prerequisites = {},
    description = "Enhances sublight drive efficiency",
    on_researched = function()
        -- TODO: push_empire_modifier("ship.sublight_speed", { add = 0.1 })
    end,
}

define_tech {
    id = "physics_ftl_theory",
    name = "FTL Theory",
    branch = "physics",
    cost = 400,
    prerequisites = { "physics_sublight_drives" },
    description = "Theoretical foundations for faster-than-light travel",
    on_researched = function()
        -- TODO: push_empire_modifier("ship.ftl_range", { add = 0.2 })
    end,
}

define_tech {
    id = "physics_warp_stabilisation",
    name = "Warp Field Stabilisation",
    branch = "physics",
    cost = 600,
    prerequisites = { "physics_ftl_theory" },
    description = "Stabilise warp fields for safer FTL travel",
    on_researched = function()
        -- TODO: push_empire_modifier("ship.ftl_speed", { multiplier = 0.15 })
    end,
}
