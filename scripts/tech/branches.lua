-- Tech branch definitions
-- Branches group technologies for UI organisation. Tech entries reference a
-- branch by id (e.g. branch = "social"). All branches must be defined before
-- the tech files that use them — `tech/init.lua` requires this file first.

local social = define_tech_branch {
    id = "social",
    name = "Social",
    color = { 0.4, 0.6, 0.9 },
}

local physics = define_tech_branch {
    id = "physics",
    name = "Physics",
    color = { 0.5, 0.4, 0.9 },
}

local industrial = define_tech_branch {
    id = "industrial",
    name = "Industrial",
    color = { 0.7, 0.5, 0.3 },
}

local military = define_tech_branch {
    id = "military",
    name = "Military",
    color = { 0.9, 0.3, 0.3 },
}

return {
    social = social,
    physics = physics,
    industrial = industrial,
    military = military,
}
