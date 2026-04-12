-- #145: Forbidden region definitions + default placement specs.
-- Blocks FTL travel (and optionally FTL comms) across parts of the galaxy.
-- Placement is constraint-driven: capital sanctuary, escape routes and
-- galaxy-wide connectivity are enforced by the Rust placement algorithm.

require("regions.dark_nebula")
require("regions.subspace_storm")

-- Default placement: a handful of each, well away from the capital so the
-- starter region remains playable. Tweak `count_range` / `sphere_radius_range`
-- per scenario.
galaxy_generation.add_region_spec {
    type = "dark_nebula",
    count_range = { 2, 4 },
    sphere_count_range = { 2, 5 },
    sphere_radius_range = { 3.0, 7.0 },
    min_distance_from_capital = 18.0,
}

galaxy_generation.add_region_spec {
    type = "subspace_storm",
    count_range = { 1, 3 },
    sphere_count_range = { 2, 4 },
    sphere_radius_range = { 2.5, 5.5 },
    min_distance_from_capital = 20.0,
}

return {}
