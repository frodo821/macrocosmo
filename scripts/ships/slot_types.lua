local weapon = define_slot_type { id = "weapon", name = "Weapon Slot" }
local utility = define_slot_type { id = "utility", name = "Utility Slot" }
local engine = define_slot_type { id = "engine", name = "Engine Slot" }
local special = define_slot_type { id = "special", name = "Special Slot" }

return {
    weapon = weapon,
    utility = utility,
    engine = engine,
    special = special,
}
