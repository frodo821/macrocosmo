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

-- #199: FTL connectivity bridge pass.
--
-- After Phase A finishes (whether Rust-default or a Lua generator drove it),
-- enforce that every star system is reachable from the provisional capital
-- under `settings.initial_ftl_range`. Whenever a cluster is disconnected,
-- insert a bridge star at the midpoint of the closest cross-cluster pair
-- and recompute. The loop caps at `max_bridge_iter` iterations as a safety
-- valve against pathological inputs (e.g. systems placed beyond twice the
-- initial FTL range from every other system).
--
-- This runs on top of (not in place of) the legacy Rust bridge pass, which
-- addresses local isolation (nearest-neighbour > max_neighbor_distance). The
-- #199 loop adds the graph-connectivity guarantee the legacy pass lacked.
on_after_phase_a(function(ctx)
    -- Cap proportional to potential cluster count. The default spiral spawns
    -- ~150 systems; pathological inputs can produce 40+ clusters. The loop
    -- terminates early once every system is reachable.
    local max_bridge_iter = 200
    local ftl_range = ctx.settings.initial_ftl_range
    for _ = 1, max_bridge_iter do
        local capital = ctx:pick_provisional_capital()
        if not capital then break end
        local graph = ctx:build_ftl_graph(ftl_range)
        local unreach = graph:unreachable_from(capital)
        if #unreach == 0 then break end
        local a, b = graph:closest_cross_cluster_pair(capital)
        if not a or not b then break end -- safety
        -- If the cross-cluster gap is wider than 2*ftl_range, a single
        -- midpoint bridge would still leave the endpoints unreachable from
        -- the bridge. Place multiple bridges along the segment so each hop
        -- is within FTL range.
        local dx = b.position.x - a.position.x
        local dy = b.position.y - a.position.y
        local dz = b.position.z - a.position.z
        local dist = math.sqrt(dx * dx + dy * dy + dz * dz)
        -- We need hops: every gap (including from a to first bridge and from
        -- last bridge to b) must be <= ftl_range. Number of bridges = ceil(dist/ftl_range) - 1,
        -- but at minimum 1 (even if dist < ftl_range we still want to join clusters;
        -- in that case a single midpoint bridge is redundant but harmless — handled
        -- by caller choosing closest pair which will be <= ftl_range after this iter).
        local bridges_needed = math.max(1, math.ceil(dist / ftl_range) - 1)
        for k = 1, bridges_needed do
            local t = k / (bridges_needed + 1)
            ctx:insert_bridge_at({
                x = a.position.x + dx * t,
                y = a.position.y + dy * t,
                z = a.position.z + dz * t,
            })
        end
    end
end)

return {}
