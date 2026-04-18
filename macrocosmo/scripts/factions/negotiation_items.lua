-- #321: Negotiation item kind definitions
--
-- Defines the built-in negotiation item kinds used in diplomatic agreements.
-- Each kind describes a type of thing that can be exchanged, how multiple
-- items of that kind merge in one agreement, and (optionally) validation
-- and application callbacks.

local resources = define_negotiation_item_kind {
    id = "resources",
    name = "Resource Transfer",
    merge = "sum",
    validate = function(ctx)
        -- Placeholder: check that the offering faction has sufficient stockpile
        return true
    end,
    apply = function(ctx)
        -- Placeholder: one-time resource transfer from giver to receiver
    end,
}

local technology = define_negotiation_item_kind {
    id = "technology",
    name = "Technology Access",
    merge = "list",
    validate = function(ctx)
        -- Placeholder: check that giver actually has the specified tech
        return true
    end,
    apply = function(ctx)
        -- Placeholder: grant receiver access to the specified tech
    end,
}

local territory = define_negotiation_item_kind {
    id = "territory",
    name = "Territory Cession",
    merge = "list",
    validate = function(ctx)
        -- Placeholder: check that giver owns the specified system
        return true
    end,
    apply = function(ctx)
        -- Placeholder: transfer sovereignty of the system to receiver
    end,
}

local peace = define_negotiation_item_kind {
    id = "peace",
    name = "Peace Treaty",
    merge = "replace",
    validate = function(ctx)
        -- Placeholder: check that the two factions are at war
        return true
    end,
    apply = function(ctx)
        -- Placeholder: transition War -> Peace
    end,
}

local alliance = define_negotiation_item_kind {
    id = "alliance",
    name = "Alliance Pact",
    merge = "replace",
    validate = function(ctx)
        -- Placeholder: check Peace state + standing >= threshold
        return true
    end,
    apply = function(ctx)
        -- Placeholder: transition Peace -> Alliance
    end,
}

local standing_modifier = define_negotiation_item_kind {
    id = "standing_modifier",
    name = "Standing Modifier",
    merge = "sum",
    validate = function(ctx)
        -- Standing modifiers are always valid to offer
        return true
    end,
    apply = function(ctx)
        -- Placeholder: apply standing adjustment
    end,
}

local return_cores = define_negotiation_item_kind {
    id = "return_cores",
    name = "Return Conquered Cores",
    merge = "list",
    validate = function(ctx)
        -- Placeholder: check that giver holds cores belonging to receiver
        return true
    end,
    apply = function(ctx)
        -- Placeholder: return core ownership
    end,
}

local trade_agreement = define_negotiation_item_kind {
    id = "trade_agreement",
    name = "Trade Agreement",
    merge = "replace",
    -- No validate/apply yet — placeholder for ongoing trade mechanics
}

return {
    resources = resources,
    technology = technology,
    territory = territory,
    peace = peace,
    alliance = alliance,
    standing_modifier = standing_modifier,
    return_cores = return_cores,
    trade_agreement = trade_agreement,
}
