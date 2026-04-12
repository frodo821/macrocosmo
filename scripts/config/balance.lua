-- #160: Scriptable game balance constants.
-- These are the baseline values used across ship / colony / authority
-- systems. Technologies, events, and modules may push modifiers onto these
-- values via `push_modifier("balance.<field>", { ... })` in `on_researched`
-- callbacks. The baseline itself stays fixed unless the value is edited
-- here (that work lives in #61).

define_balance {
    -- Ship / travel
    initial_ftl_speed_c      = 10.0, -- multiple of light speed
    survey_duration          = 30,   -- hexadies
    settling_duration        = 60,   -- hexadies
    survey_range_ly          = 5.0,
    port_ftl_range_bonus     = 10.0, -- LY added to FTL range when departing from a Port
    port_travel_time_factor  = 0.8,  -- FTL travel time multiplier at a Port (0.8 = 20% faster)
    repair_rate_per_hexadies = 5.0,

    -- Colony / authority
    colonization_mineral_cost   = 300,
    colonization_energy_cost    = 200,
    colonization_build_time     = 90, -- hexadies
    base_authority_per_hexadies = 1.0,
    authority_cost_per_colony   = 0.5,
}
