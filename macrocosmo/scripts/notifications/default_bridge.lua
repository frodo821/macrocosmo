-- #345 ESC-2: default Lua bridge from ScriptableKnowledge `*@observed`
-- events to the Empire Situation Center (ESC) Notifications tab.
--
-- This file registers a single wildcard subscriber via `on("*@observed",
-- fn)` and maps each built-in `core:*` kind (see
-- `crate::knowledge::kind_registry::CORE_KIND_IDS` — 9 kinds as of
-- K-5) to a `push_notification { ... }` call. The push lands in
-- `_pending_esc_notifications`; the Rust-side
-- `scripting::esc_notifications::drain_pending_esc_notifications`
-- system drains it the same frame.
--
-- The bridge is pure policy: severity / message formatting / source
-- mapping for each kind. Because it is Lua, downstream games /
-- modders can copy this file, tweak the policy, and override the
-- defaults without touching Rust.
--
-- ## Why a wildcard subscriber?
--
-- * A wildcard (`*@observed`) subscriber fires for every `core:*` +
--   `sample:*` + modder-defined kind in one registration. The
--   `.kind` switch below dispatches per-kind policy.
-- * The subscriber is queue-only: it calls `push_notification { ... }`
--   which enqueues a plain Lua table, never invoking Rust callbacks.
--   Rust owns the subsequent parse + queue mutation.
-- * Subscriber errors `warn` + chain continues (see
--   `knowledge_dispatch::dispatch_knowledge` semantics).
--
-- ## Out of scope
--
-- * `sample:*` kinds (from `scripts/knowledge/sample.lua`) are NOT
--   mapped by this default bridge. Those are fixtures for testing the
--   K-1..K-5 pipeline. If a game wants to turn them into ESC
--   notifications, it should add a separate `require(...)`-loaded
--   bridge file keyed on those ids.
-- * Top-banner (#151) behaviour is unchanged — banners continue to
--   flow through `auto_notify_from_events` + the inlined core:*
--   banner bridge in `dispatch_knowledge_observed`. The ESC queue is
--   a separate history path.

-- Helper: extract `e.payload.<field>` safely. Payload fields follow
-- the `core:*` schema in `crate::knowledge::kind_registry::core_kind_catalog`.
local function payload_field(e, key)
    if e.payload == nil then
        return nil
    end
    return e.payload[key]
end

-- Helper: format the "(lag N hd)" suffix when lag_hexadies is present
-- and non-trivial. Many core:* events are local / near-instant at
-- game start, and showing "(lag 0 hd)" everywhere is noise.
local function lag_suffix(e)
    if e.lag_hexadies == nil or e.lag_hexadies <= 0 then
        return ""
    end
    return string.format(" (lag %d hd)", e.lag_hexadies)
end

-- Helper: build a system-scoped source table when the payload carries
-- a `system` entity. Falls back to origin_system for variants where
-- the system is optional (core:structure_built, core:ship_arrived).
local function system_source(e)
    local system_id = payload_field(e, "system") or e.origin_system
    if system_id ~= nil then
        return { kind = "system", id = system_id }
    end
    return { kind = "none" }
end

-- Table of per-kind policy functions. Each entry takes the sealed
-- observed event table and returns a push_notification args table
-- (OR nil to skip). Keeping each entry as a function — instead of a
-- flat per-kind if/elseif ladder — isolates message formatting so
-- modders can swap entries without touching other kinds.
local KIND_POLICY = {
    ["core:hostile_detected"] = function(e)
        -- Payload: target (entity), detector (entity), target_pos_x/y/z,
        -- description. We dedupe by target entity so repeated spotting
        -- of the same hostile doesn't flood the tab.
        local target = payload_field(e, "target")
        local event_id = nil
        if target ~= nil then
            event_id = string.format("core:hostile:%d", target)
        end
        local description = payload_field(e, "description") or "Hostile contact"
        return {
            event_id = event_id,
            severity = "warn",
            source = system_source(e),
            message = description .. lag_suffix(e),
            timestamp = e.observed_at,
        }
    end,

    ["core:combat_outcome"] = function(e)
        -- Payload: system, victor ("player"|"hostile"), detail.
        local victor = payload_field(e, "victor") or "unknown"
        local detail = payload_field(e, "detail") or ""
        -- Victory = info, defeat = critical (mirrors banner priority).
        local severity = "info"
        local label = "Combat victory"
        if victor == "hostile" then
            severity = "critical"
            label = "Combat defeat"
        end
        local system = payload_field(e, "system")
        local event_id = nil
        if system ~= nil then
            event_id = string.format("core:combat:%d:%s", system, victor)
        end
        return {
            event_id = event_id,
            severity = severity,
            source = system_source(e),
            message = label .. ": " .. detail .. lag_suffix(e),
            timestamp = e.observed_at,
        }
    end,

    ["core:survey_complete"] = function(e)
        -- Payload: system, system_name, detail.
        local system_name = payload_field(e, "system_name") or "(unnamed)"
        local system = payload_field(e, "system")
        local event_id = nil
        if system ~= nil then
            event_id = string.format("core:survey_complete:%d", system)
        end
        return {
            event_id = event_id,
            severity = "info",
            source = system_source(e),
            message = "Survey complete: " .. system_name .. lag_suffix(e),
            timestamp = e.observed_at,
        }
    end,

    ["core:anomaly_discovered"] = function(e)
        -- Payload: system, anomaly_id, detail.
        local anomaly_id = payload_field(e, "anomaly_id") or "(unknown anomaly)"
        local detail = payload_field(e, "detail") or ""
        local system = payload_field(e, "system")
        local event_id = nil
        if system ~= nil then
            event_id = string.format("core:anomaly:%d:%s", system, tostring(anomaly_id))
        end
        local message
        if detail == "" then
            message = "Anomaly discovered: " .. anomaly_id
        else
            message = "Anomaly discovered (" .. anomaly_id .. "): " .. detail
        end
        return {
            event_id = event_id,
            severity = "warn",
            source = system_source(e),
            message = message .. lag_suffix(e),
            timestamp = e.observed_at,
        }
    end,

    ["core:survey_discovery"] = function(e)
        -- Payload: system, detail.
        local detail = payload_field(e, "detail") or "(unspecified)"
        local system = payload_field(e, "system")
        local event_id = nil
        if system ~= nil then
            -- Discoveries can stack per-system — include `detail` hash
            -- in id so different findings are tracked separately.
            event_id = string.format("core:discovery:%d:%s", system, tostring(detail))
        end
        return {
            event_id = event_id,
            severity = "info",
            source = system_source(e),
            message = "Discovery: " .. detail .. lag_suffix(e),
            timestamp = e.observed_at,
        }
    end,

    ["core:structure_built"] = function(e)
        -- Payload: system (optional), kind, name, destroyed (bool), detail.
        local name = payload_field(e, "name") or "(unnamed structure)"
        local destroyed = payload_field(e, "destroyed")
        local label
        local severity
        if destroyed == true then
            label = "Structure destroyed: "
            severity = "warn"
        else
            label = "Structure built: "
            severity = "info"
        end
        -- Dedup per structure name — repeated fact propagation (relay
        -- + direct channel) should only show once in the tab.
        local event_id = string.format("core:structure:%s:%s", tostring(name), tostring(destroyed))
        return {
            event_id = event_id,
            severity = severity,
            source = system_source(e),
            message = label .. name .. lag_suffix(e),
            timestamp = e.observed_at,
        }
    end,

    ["core:colony_established"] = function(e)
        -- Payload: system, planet, name, detail.
        local name = payload_field(e, "name") or "(unnamed colony)"
        local planet = payload_field(e, "planet")
        local event_id = nil
        if planet ~= nil then
            event_id = string.format("core:colony_est:%d", planet)
        end
        return {
            event_id = event_id,
            severity = "info",
            source = system_source(e),
            message = "Colony established: " .. name .. lag_suffix(e),
            timestamp = e.observed_at,
        }
    end,

    ["core:colony_failed"] = function(e)
        -- Payload: system, name, reason.
        local name = payload_field(e, "name") or "(unnamed colony)"
        local reason = payload_field(e, "reason") or "(unknown)"
        local system = payload_field(e, "system")
        local event_id = nil
        if system ~= nil then
            event_id = string.format("core:colony_failed:%d:%s", system, tostring(name))
        end
        return {
            event_id = event_id,
            severity = "critical",
            source = system_source(e),
            message = "Colony failed: " .. name .. " — " .. reason .. lag_suffix(e),
            timestamp = e.observed_at,
        }
    end,

    ["core:ship_arrived"] = function(e)
        -- Payload: system (optional), name, detail.
        local name = payload_field(e, "name") or "(unnamed ship)"
        local detail = payload_field(e, "detail") or ""
        -- Ship arrivals are high-frequency; dedup per ship name would
        -- mask fleet movements. Instead we namespace by observed_at so
        -- distinct arrival moments each get one entry. `observed_at`
        -- is tick-accurate so two arrivals at the exact same tick of
        -- the same ship (unusual) still collapse cleanly.
        local event_id = string.format(
            "core:ship_arr:%s:%d",
            tostring(name),
            e.observed_at or 0
        )
        local message
        if detail == "" then
            message = "Ship arrived: " .. name
        else
            message = "Ship arrived: " .. name .. " (" .. detail .. ")"
        end
        return {
            event_id = event_id,
            severity = "info",
            source = system_source(e),
            message = message .. lag_suffix(e),
            timestamp = e.observed_at,
        }
    end,
}

-- Wildcard `*@observed` subscriber. See `on()` semantics in
-- `scripting/globals.rs` — this registration routes through the
-- bucketed `KnowledgeSubscriptionRegistry` at startup.
on("*@observed", function(e)
    local policy = KIND_POLICY[e.kind]
    if policy == nil then
        -- Non-core kinds (sample:*, modder ids) are intentionally
        -- not handled by this bridge. Return silently so the
        -- dispatcher can continue to other subscribers.
        return
    end
    local args = policy(e)
    if args == nil then
        return
    end
    push_notification(args)
end)
