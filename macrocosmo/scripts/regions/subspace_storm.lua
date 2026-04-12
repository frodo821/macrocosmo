-- Subspace Storm: blocks FTL travel but leaves FTL comm intact (relays can
-- still hop past the storm — just ships can't FTL through it).

define_region_type {
    id = "subspace_storm",
    name = "Subspace Storm",
    capabilities = {
        blocks_ftl = { strength = 1.0 },
    },
    visual = {
        color = { 0.55, 0.25, 0.10 },
        density = 0.60,
    },
}

return {}
