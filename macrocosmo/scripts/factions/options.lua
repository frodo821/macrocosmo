-- Diplomatic option definitions (#302)
--
-- Each option represents a type of diplomatic interaction that factions can
-- initiate. Options are either "bilateral" (requiring a receiver response)
-- or "unilateral" (fire and forget).
--
-- Responses carry an `event_id` string that fires through the event system
-- when chosen, allowing Lua `on()` handlers to react. Payloads are POD
-- key-value maps (no closures).
--
-- Loaded from `scripts/factions/init.lua` after faction-type definitions.

local generic_negotiation = define_diplomatic_option {
    id = "generic_negotiation",
    name = "Open Negotiation",
    description = "Initiate bilateral negotiations. The receiver may accept or reject.",
    kind = "bilateral",
    responses = {
        { id = "accept", label = "Accept", event_id = "negotiation_accepted" },
        { id = "reject", label = "Reject", event_id = "negotiation_rejected" },
    },
    payload_schema = { "terms" },
}

local break_alliance = define_diplomatic_option {
    id = "break_alliance",
    name = "Break Alliance",
    description = "Unilaterally terminate an alliance. No receiver response required.",
    kind = "unilateral",
}

return {
    generic_negotiation = generic_negotiation,
    break_alliance = break_alliance,
}
