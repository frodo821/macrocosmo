local slot_types = require("ships.slot_types")
local hulls = require("ships.hulls")
local modules = require("ships.modules")
-- #385: Station hulls + modules (immobile, for system building ships).
local station_hulls = require("ships.station_hulls")
local station_modules = require("ships.station_modules")
local designs = require("ships.designs")
-- #296: Infrastructure Core hull + design (immobile).
local core_hulls = require("ships.core_hulls")

return {
    slot_types = slot_types,
    hulls = hulls,
    modules = modules,
    station_hulls = station_hulls,
    station_modules = station_modules,
    designs = designs,
    core_hulls = core_hulls,
}
