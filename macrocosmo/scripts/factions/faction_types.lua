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
    -- #293: Hostile combat stats moved from hard-coded HostileType::SpaceCreature
    -- constants in Rust. Environmental strength_mult in generation.rs still scales
    -- these base values based on stellar distance.
    strength = 10,
    evasion = 20,
    default_hp = 80,
    default_max_hp = 80,
}

local ancient_defense = define_faction_type {
    id = "ancient_defense",
    can_diplomacy = false,
    default_standing = -100,
    default_state = "neutral",
    -- #293: Hostile combat stats moved from hard-coded HostileType::AncientDefense
    -- constants in Rust.
    strength = 10,
    evasion = 10,
    default_hp = 200,
    default_max_hp = 200,
}

return {
    empire = empire,
    space_creature = space_creature,
    ancient_defense = ancient_defense,
}
