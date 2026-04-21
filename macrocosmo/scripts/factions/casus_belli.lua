-- #305 (S-11): Casus Belli definitions
--
-- Defines the built-in casus belli types. Each CB has:
--   id:         unique string identifier
--   name:       human-readable name
--   auto_war:   if true, war is declared automatically when evaluate() returns true
--   evaluate:   function(attacker_id, defender_id) -> bool
--   demands:    base demands imposed on the loser
--   end_scenarios: named ways the war can end

local core_attack = define_casus_belli {
    id = "core_attack",
    name = "Unprovoked Core Attack",
    auto_war = true,

    -- Evaluate whether this CB should trigger auto-war.
    -- Called each tick for every ordered pair of empire factions.
    -- Returns true if the attacker should declare war on the defender.
    --
    -- The core_attack CB fires when a faction's Infrastructure Core has been
    -- attacked during peacetime (the GameEventKind::CasusBelli event).
    -- The evaluate function checks whether the defender has the
    -- "core_attacked_by_<attacker>" modifier. For now, always returns false;
    -- actual modifier-checking integration is a follow-up.
    evaluate = function(attacker_id, defender_id)
        -- Placeholder: in the full integration, this would check whether
        -- attacker's core was attacked by defender. For now, Lua-side
        -- auto-war triggering is gated by external callers setting up
        -- the right conditions (e.g., the bridge_casus_belli_to_event_system
        -- fires macrocosmo:core_attacked, which Lua handlers can subscribe to).
        return false
    end,

    demands = {
        { kind = "return_cores" },
    },

    end_scenarios = {
        {
            id = "white_peace",
            label = "White Peace",
            -- No demand_adjustments = all demands dropped
            available = function(attacker_id, defender_id)
                -- White peace is always available
                return true
            end,
        },
        {
            id = "unconditional_surrender",
            label = "Unconditional Surrender",
            demand_adjustments = {
                { kind = "return_cores" },
                { kind = "reparations", amount = "500" },
            },
            available = function(attacker_id, defender_id)
                -- Unconditional surrender requires significant war exhaustion
                -- (placeholder: always available for now)
                return true
            end,
        },
    },
}

return {
    core_attack = core_attack,
}
