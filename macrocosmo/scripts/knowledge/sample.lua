-- #350 K-1: sample knowledge kind definitions.
--
-- Exercises the `define_knowledge { id, payload_schema }` surface added by
-- the ScriptableKnowledge epic (#349). These are intentionally small and
-- domain-agnostic — they are here to:
--
-- * Prove the startup path (parse + KindRegistry insert + lifecycle-event
--   reservation) runs for a real Lua fixture.
-- * Give #351 / #352 / #353 / #354 a set of pre-defined kinds to exercise
--   record / observe / subscribe flows in integration tests.
-- * Document the expected shape of `payload_schema` for modders.
--
-- `core:` is reserved for Rust-side built-ins (K-5) — do not use it here.

-- Famine breakout in a colony. Payload carries severity + the affected
-- colony entity id so observers can disambiguate multiple simultaneous
-- events.
define_knowledge {
    id = "sample:colony_famine",
    payload_schema = {
        severity = "number",
        colony = "entity",
    },
}

-- Combat outcome report. Schema-less in this fixture; K-2 payloads will
-- carry attacker / defender ids.
define_knowledge {
    id = "sample:combat_report",
}

-- Anomaly surveyed. Exercises every top-level schema type tag so the K-1
-- parser has real-world coverage.
define_knowledge {
    id = "sample:anomaly_surveyed",
    payload_schema = {
        anomaly_id = "string",
        system = "entity",
        hazard = "boolean",
        extras = "table",
        yield_bonus = "number",
    },
}

-- Routine diplomatic signal. Multiple kinds in a single namespace is
-- explicitly supported — the registry key is the full `<ns>:<name>`.
define_knowledge {
    id = "sample:diplomatic_signal",
    payload_schema = {
        faction = "entity",
        tone = "string",
    },
}
