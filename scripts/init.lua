-- Macrocosmo script entrypoint
-- All game data definitions are loaded through this file via require().
-- Order matters: definitions must be loaded before they are referenced.

-- Base definitions (no cross-references)
require("stars")
require("planets")
require("jobs")

-- Species (references jobs by string key — no require dependency)
require("species")

-- Buildings (independent)
require("buildings")

-- Technology (may be referenced by modules, structures)
require("tech")

-- Ships (modules may reference techs; designs reference hulls + modules)
require("ships")

-- Structures (reference techs via conditions)
require("structures")

-- Events
require("events")

-- Lifecycle hooks (must be last — registers callbacks for game start/load)
require("lifecycle")
