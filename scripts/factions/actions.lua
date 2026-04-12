-- Diplomatic action definitions (#172)
--
-- Custom diplomatic actions that coexist with the built-in DiplomaticAction
-- variants (DeclareWar / ProposePeace / ProposeAlliance / BreakAlliance).
--
-- Each action may specify prerequisites (requires_diplomacy, requires_state,
-- min_standing) and an `on_accepted(scope)` callback whose returned effects
-- (via `scope:push_modifier`, `scope:set_flag`, etc.) are applied as normal
-- DescriptiveEffects when the receiver accepts the action.
--
-- Loaded from `scripts/factions/init.lua` after faction-type definitions so
-- string state names ("peace", "neutral", ...) are already understood.

local trade_agreement = define_diplomatic_action {
    id = "trade_agreement",
    name = "Trade Agreement",
    description = "Economic cooperation between peaceful factions. Boosts trade income.",
    requires_diplomacy = true,
    requires_state = "peace",
    min_standing = 20,
    on_accepted = function(scope)
        return {
            scope:push_modifier("empire.trade_income", {
                multiplier = 0.1,
                description = "Trade Agreement: trade income +10%",
            }),
        }
    end,
}

local cultural_exchange = define_diplomatic_action {
    id = "cultural_exchange",
    name = "Cultural Exchange",
    description = "Shared research benefits between friendly factions.",
    requires_diplomacy = true,
    requires_state = "peace",
    min_standing = 40,
    on_accepted = function(scope)
        return {
            scope:push_modifier("empire.research_output", {
                multiplier = 0.05,
                description = "Cultural Exchange: research +5%",
            }),
        }
    end,
}

return {
    trade_agreement = trade_agreement,
    cultural_exchange = cultural_exchange,
}
