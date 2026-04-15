-- #296 (S-3): Infrastructure Core deliverable.
--
-- Built at a shipyard like any other deliverable, but on deploy the engine
-- spawns an immobile `Ship` entity (using the `spawns_as_ship` design id)
-- instead of a `DeepSpaceStructure`. The Core ship carries the `CoreShip`
-- marker and `AtSystem` component so `faction::system_owner` recognises the
-- system as sovereign to the deploying faction.
--
-- Validation (enforced in Rust):
--   * Deploy outside a star system → self-destruct.
--   * Deploy in a system that already has a Core ship → self-destruct.
--   * Same-tick tie → deterministic RNG picks one, rest self-destruct.

local core_hulls = require("ships.core_hulls")

local infrastructure_core = define_deliverable {
    id = "infrastructure_core",
    name = "Infrastructure Core",
    description = "A sovereignty anchor. Once deployed to a system, declares that system part of your empire.",
    max_hp = 400,
    cost = { minerals = 600, energy = 400 },
    build_time = 120,
    cargo_size = 5,
    scrap_refund = 0.25,
    capabilities = {},
    energy_drain = 200,
    -- Marks this deliverable as ship-spawning; the engine consumes this in
    -- `deliverable_ops::process_deliverable_commands` and dispatches to the
    -- Core deploy pipeline instead of `spawn_deliverable_entity`.
    spawns_as_ship = core_hulls.infrastructure_core_v1,
}

return {
    infrastructure_core = infrastructure_core,
}
