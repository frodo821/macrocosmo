-- Macrocosmo script entrypoint
-- All game data definitions are loaded through this file via require().
-- Order matters: definitions must be loaded before they are referenced.

-- Reusable Lua helper libraries (no game definitions; just functions exposed
-- as globals like `initialize_default_capital`). Loaded first so faction
-- on_game_start callbacks defined later can use them.
require("lib.capital")

-- #160: Balance constants. Loaded before anything else so that tech / event
-- definitions can reference `balance.*` modifier targets with confidence the
-- baseline values are populated by the time modifiers apply.
require("config.balance")

-- Base definitions (no cross-references)
require("stars")
require("planets")
require("jobs")

-- #182: Map types (includes `default`, registered without a generator).
require("galaxy.map_types")

-- #145: Forbidden region types + default placement specs.
-- Loads after map types so scenario-specific map types can opt out of the
-- default region specs if needed (by clearing `_pending_region_specs`).
require("regions")

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

-- Factions (must be before events/lifecycle that may reference them)
require("factions")

-- Anomalies (survey discovery definitions)
require("anomalies")

-- Events
require("events")

-- Lifecycle hooks (must be last — registers callbacks for game start/load)
require("lifecycle")
