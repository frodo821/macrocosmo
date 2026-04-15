local definitions = require("structures.definitions")
-- #296: Infrastructure Core deliverable.
local cores = require("structures.cores")

local merged = {}
for k, v in pairs(definitions) do
    merged[k] = v
end
for k, v in pairs(cores) do
    merged[k] = v
end
return merged
