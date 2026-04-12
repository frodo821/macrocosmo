-- Dark Nebula: dense interstellar gas that fully disables FTL travel AND
-- FTL communication. The defining "wall" between two arms of the galaxy.

define_region_type {
    id = "dark_nebula",
    name = "Dark Nebula",
    capabilities = {
        blocks_ftl = { strength = 1.0 },
        blocks_ftl_comm = { strength = 1.0 },
    },
    visual = {
        color = { 0.35, 0.10, 0.55 },
        density = 0.75,
    },
}

return {}
