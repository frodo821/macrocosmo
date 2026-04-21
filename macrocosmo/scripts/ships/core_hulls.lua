-- #296 (S-3): Infrastructure Core hull and ship design.
--
-- The Core hull has no propulsion slots and base_speed = 0, so the derived
-- ShipDesignDefinition fields (sublight_speed, ftl_range) both resolve to 0.
-- This makes `Ship::is_immobile()` return true, which:
--   * Causes `start_sublight_travel` to reject the ship with
--     `Err("ship is immobile")`,
--   * Disables the "Move to" option in the UI context menu,
--   * Prevents the ship from being a pursuer in deep-space contact checks.
--
-- Core ships are spawned as immobile Ships by the deliverable pipeline
-- (see `scripts/structures/cores.lua` and `src/ship/core_deliverable.rs`).

local infrastructure_core_hull = define_hull {
    id = "infrastructure_core_hull",
    name = "Infrastructure Core Hull",
    description = "Anchors a star system's sovereignty. Immobile by design.",
    size = 10000,
    is_capital = false,
    base_hp = 400,
    base_speed = 0, -- immobile
    base_evasion = 0,
    slots = {
        -- No FTL, no sublight. Only a power slot to allow future upgrade
        -- modules (shields, comm boosters) without violating immobility.
    },
    build_cost = { minerals = 0, energy = 0 },
    build_time = 0,
    maintenance = 2.0,
}

local infrastructure_core_v1 = define_ship_design {
    id = "infrastructure_core_v1",
    name = "Infrastructure Core",
    hull = infrastructure_core_hull,
    modules = {
        -- No modules — the hull's zero slots already enforce immobility.
    },
}

return {
    infrastructure_core_hull = infrastructure_core_hull,
    infrastructure_core_v1 = infrastructure_core_v1,
}
