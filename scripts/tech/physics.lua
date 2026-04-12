-- Physics branch technologies

local sensor_arrays = define_tech {
    id = "physics_sensor_arrays",
    name = "Advanced Sensor Arrays",
    branch = "physics",
    cost = 100,
    prerequisites = {},
    description = "Next-generation sensors for deep space observation",
    on_researched = function(scope)
        scope:push_modifier("sensor.range", { add = 2.0, description = "Advanced Sensors: +2 survey range" })
        -- #160: Demonstrates the scriptable balance pipeline — cuts survey
        -- duration by 20% via a multiplier on the GameBalance.survey_duration
        -- ModifiedValue (target "balance.survey_duration").
        scope:push_modifier("balance.survey_duration", { multiplier = -0.2, description = "Advanced Sensors: Survey time -20%" })
        scope:set_flag("advanced_sensors_unlocked", true, { description = "Enables advanced sensor arrays" })
    end,
}

local sublight_drives = define_tech {
    id = "physics_sublight_drives",
    name = "Improved Sublight Drives",
    branch = "physics",
    cost = 200,
    prerequisites = {},
    description = "Enhances sublight drive efficiency",
    on_researched = function(scope)
        scope:push_modifier("ship.sublight_speed", { add = 0.1, description = "Improved Drives: +0.1 sublight speed" })
    end,
}

local ftl_theory = define_tech {
    id = "physics_ftl_theory",
    name = "FTL Theory",
    branch = "physics",
    cost = 400,
    prerequisites = { sublight_drives },
    description = "Theoretical foundations for faster-than-light travel",
    on_researched = function(scope)
        scope:push_modifier("ship.ftl_range", { add = 5.0, description = "FTL Theory: +5 FTL range" })
        scope:set_flag("ftl_theory_unlocked", true, { description = "Enables FTL drive research" })
    end,
}

local warp_stabilisation = define_tech {
    id = "physics_warp_stabilisation",
    name = "Warp Field Stabilisation",
    branch = "physics",
    cost = 600,
    prerequisites = { ftl_theory },
    description = "Stabilise warp fields for safer FTL travel",
    on_researched = function(scope)
        scope:push_modifier("ship.ftl_speed", { multiplier = 0.15, description = "Warp Stabilisation: +15% FTL speed" })
    end,
}

return {
    sensor_arrays = sensor_arrays,
    sublight_drives = sublight_drives,
    ftl_theory = ftl_theory,
    warp_stabilisation = warp_stabilisation,
}
