-- Physics branch technologies

define_tech {
    id = 200,
    name = "Advanced Sensor Arrays",
    branch = "physics",
    cost = 100,
    prerequisites = {},
    description = "Next-generation sensors for deep space observation",
    effects = {
        { type = "modify_sensor_range", value = 0.2 },
    },
}

define_tech {
    id = 201,
    name = "Improved Sublight Drives",
    branch = "physics",
    cost = 200,
    prerequisites = {},
    description = "Enhances sublight drive efficiency",
    effects = {
        { type = "modify_sublight_speed", value = 0.1 },
    },
}

define_tech {
    id = 202,
    name = "FTL Theory",
    branch = "physics",
    cost = 400,
    prerequisites = { 201 },
    description = "Theoretical foundations for faster-than-light travel",
    effects = {
        { type = "modify_ftl_range", value = 0.2 },
    },
}

define_tech {
    id = 203,
    name = "Warp Field Stabilisation",
    branch = "physics",
    cost = 600,
    prerequisites = { 202 },
    description = "Stabilise warp fields for safer FTL travel",
    effects = {
        { type = "modify_ftl_speed", value = 0.15 },
    },
}
