-- Faction type definitions (#170)
--
-- Each faction belongs to one of these categories. Types supply defaults
-- for new diplomatic relationships and gate the diplomacy UI via
-- `can_diplomacy`. Factions reference a type via `faction_type = "id"`
-- (string) or `faction_type = empire` (reference returned by define_faction_type).
--
-- Loaded BEFORE `define_faction` calls so factions can reference these by
-- value. See `scripts/factions/init.lua` for the require order.

local empire = define_faction_type {
    id = "empire",
    can_diplomacy = true,
    default_standing = 0,
    default_state = "neutral",
}

local space_creature = define_faction_type {
    id = "space_creature",
    can_diplomacy = false,
    default_standing = -100,
    default_state = "neutral",
}

local ancient_defense = define_faction_type {
    id = "ancient_defense",
    can_diplomacy = false,
    default_standing = -100,
    default_state = "neutral",
}

return {
    empire = empire,
    space_creature = space_creature,
    ancient_defense = ancient_defense,
}
