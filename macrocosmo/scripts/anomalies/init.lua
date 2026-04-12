-- Anomaly definitions for survey discoveries
-- Each anomaly has a weighted probability of being found during surveys.

define_anomaly {
    id = "mineral_vein",
    name = "Deep Mineral Vein",
    description = "Rich mineral deposits detected underground",
    weight = 15,
    effects = { { type = "resource_bonus", resource = "minerals" } },
}

define_anomaly {
    id = "energy_source",
    name = "Geothermal Energy Source",
    description = "Powerful geothermal vents suitable for energy extraction",
    weight = 12,
    effects = { { type = "resource_bonus", resource = "energy" } },
}

define_anomaly {
    id = "ancient_ruins",
    name = "Ancient Ruins",
    description = "Remnants of a long-dead civilization",
    weight = 10,
    effects = { { type = "research_bonus", amount = 100 } },
}

define_anomaly {
    id = "hazardous_anomaly",
    name = "Hazardous Anomaly",
    description = "Dangerous radiation field detected",
    weight = 10,
    effects = { { type = "hazard", damage_percent = 30 } },
}

define_anomaly {
    id = "extra_building_sites",
    name = "Stable Geological Formations",
    description = "Ideal locations for additional construction",
    weight = 5,
    effects = { { type = "building_slots", extra = 2 } },
}

define_anomaly {
    id = "research_data",
    name = "Alien Data Cache",
    description = "A trove of encrypted alien scientific data",
    weight = 8,
    effects = { { type = "research_bonus", amount = 200 } },
}
