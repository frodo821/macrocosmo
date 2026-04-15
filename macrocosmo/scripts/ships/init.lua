local slot_types = require("ships.slot_types")
local hulls = require("ships.hulls")
local modules = require("ships.modules")
local designs = require("ships.designs")
-- #296: Infrastructure Core hull + design (immobile).
local core_hulls = require("ships.core_hulls")

return {
    slot_types = slot_types,
    hulls = hulls,
    modules = modules,
    designs = designs,
    core_hulls = core_hulls,
}
