-- #182: Built-in map types.
--
-- The `default` map type does NOT install a generator. When a map type has no
-- `generator` function, `generate_galaxy` keeps its legacy behaviour: the
-- `on_galaxy_generate_empty` hook runs if any script registered one, otherwise
-- the Rust spiral-arm default.
--
-- Additional map types (spiral/clustered/ring/etc.) can be added in their own
-- files that `require("galaxy.map_types")` to append; this file intentionally
-- only defines `default` to keep the initial behaviour 100%-compatible.

define_map_type {
    id = "default",
    name = "Default",
    description = "Legacy spiral-arm generator. No-op wrapper — the engine falls through to the built-in Rust phase-A generator.",
    -- no `generator` field on purpose — see header comment.
}

-- Intentionally NOT calling `set_active_map_type` here. The engine treats an
-- unset active map type as "use built-in behaviour", so leaving it alone
-- preserves compatibility. Scenarios / scenarios-modules can opt in with:
--     set_active_map_type("spiral_galaxy")

return {}
