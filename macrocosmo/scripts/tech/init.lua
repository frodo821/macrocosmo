-- Branches must load first: tech entries reference them by id.
local branches = require("tech.branches")

local industrial = require("tech.industrial")
local military = require("tech.military")
local physics = require("tech.physics")
local social = require("tech.social")

return {
    branches = branches,
    industrial = industrial,
    military = military,
    physics = physics,
    social = social,
}
