-- Refined ship slot type taxonomy.
-- Slot types are Lua-defined string IDs; Rust validates that each module's
-- slot_type matches a slot declared on the hull it's installed into.

local ftl = define_slot_type { id = "ftl", name = "FTL Drive Slot" }
local sublight = define_slot_type { id = "sublight", name = "Sublight Engine Slot" }
local weapon = define_slot_type { id = "weapon", name = "Weapon Slot" }
local defense = define_slot_type { id = "defense", name = "Defense Slot" }
local utility = define_slot_type { id = "utility", name = "Utility Slot" }
local power = define_slot_type { id = "power", name = "Power Slot" }
local command = define_slot_type { id = "command", name = "Command Slot" }

return {
    ftl = ftl,
    sublight = sublight,
    weapon = weapon,
    defense = defense,
    utility = utility,
    power = power,
    command = command,
}
