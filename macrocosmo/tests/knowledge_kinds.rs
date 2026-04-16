//! #350 K-1 integration tests for `define_knowledge` + `KindRegistry` +
//! auto `<id>@recorded` / `<id>@observed` reservation.
//!
//! These tests exercise:
//!
//! * The `define_knowledge` Lua global registered by `setup_globals`.
//! * `parse_knowledge_definitions` walking `_knowledge_kind_definitions`.
//! * `register_auto_lifecycle_events` populating
//!   `_knowledge_reserved_events`.
//! * The `load_knowledge_kinds` Bevy system end-to-end against a real
//!   `ScriptEngine` + the `scripts/knowledge/sample.lua` fixture.
//!
//! Pure parser / registry tests live in the `scripting::knowledge_api` and
//! `knowledge::kind_registry` unit-test modules (~40 tests). This file
//! complements them with the "real script + startup system" path.
//!
//! K-3 (#352) / K-2 (#351) / later slices will extend these tests with
//! `on(...)` routing and dispatch coverage; K-1 only validates the
//! foundation.

use std::path::PathBuf;

use bevy::prelude::*;

use macrocosmo::knowledge::kind_registry::{KindOrigin, KindRegistry, KnowledgeKindDef};
use macrocosmo::scripting::knowledge_api::{
    KNOWLEDGE_DEF_ACCUMULATOR, KNOWLEDGE_RESERVED_EVENTS_TABLE, KNOWLEDGE_SUBSCRIBERS_TABLE,
    is_reserved_knowledge_event, parse_knowledge_definitions, register_auto_lifecycle_events,
};
use macrocosmo::scripting::{ScriptEngine, load_knowledge_kinds};

/// Locate the repo-shipped `scripts/` directory. Mirrors the lookup that
/// `ScriptEngine::new` does in production, but from a known anchor so the
/// test does not depend on cwd.
fn sample_scripts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts")
}

/// Build an `App` carrying just the `ScriptEngine` resource and run the
/// K-1 startup system manually. This sidesteps `ScriptingPlugin`'s full
/// plugin chain (BuildingRegistry / StructureRegistry etc. that this
/// slice does not touch) and isolates the system under test.
fn run_load_knowledge_kinds() -> App {
    let engine = ScriptEngine::new_with_scripts_dir(sample_scripts_dir())
        .expect("ScriptEngine construction");
    // Load `scripts/init.lua` exactly like `load_all_scripts` does in
    // production — the Lua `init.lua` pulls in `knowledge.sample`.
    let init_path = sample_scripts_dir().join("init.lua");
    engine
        .load_file(&init_path)
        .expect("scripts/init.lua must load cleanly for K-1 fixture");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(engine);
    // `load_knowledge_kinds` is a plain startup system — drive it through
    // a one-shot schedule so we don't need the rest of ScriptingPlugin.
    app.add_systems(Startup, load_knowledge_kinds);
    app.update();
    app
}

#[test]
fn load_knowledge_kinds_reads_sample_fixture() {
    // The startup system must pick up `scripts/knowledge/sample.lua`
    // (reached via `scripts/init.lua`) and hand the KindRegistry the four
    // kinds defined in that fixture.
    let mut app = run_load_knowledge_kinds();
    let registry = app
        .world_mut()
        .remove_resource::<KindRegistry>()
        .expect("KindRegistry inserted by load_knowledge_kinds");

    for id in [
        "sample:colony_famine",
        "sample:combat_report",
        "sample:anomaly_surveyed",
        "sample:diplomatic_signal",
    ] {
        assert!(registry.contains(id), "expected kind '{id}' in registry");
    }
    assert_eq!(registry.len(), 4);

    // `sample:anomaly_surveyed` exercises every v1 payload type.
    let def = registry.get("sample:anomaly_surveyed").expect("kind def");
    assert_eq!(def.origin, KindOrigin::Lua);
    assert_eq!(def.payload_schema.fields.len(), 5);
}

#[test]
fn sample_fixture_reserves_lifecycle_events() {
    // After startup the Lua `_knowledge_reserved_events` set must carry
    // an entry for every sample kind × {recorded, observed} — K-3 routes
    // `on(...)` based on this lookup.
    let app = run_load_knowledge_kinds();

    let engine = app.world().resource::<ScriptEngine>();
    let lua = engine.lua();

    for id in [
        "sample:colony_famine",
        "sample:combat_report",
        "sample:anomaly_surveyed",
        "sample:diplomatic_signal",
    ] {
        for lifecycle in ["recorded", "observed"] {
            let event_id = format!("{id}@{lifecycle}");
            assert!(
                is_reserved_knowledge_event(lua, &event_id).unwrap(),
                "expected {event_id} reserved"
            );
        }
    }

    // Wildcard matchers are always reserved regardless of kind registrations.
    assert!(is_reserved_knowledge_event(lua, "*@recorded").unwrap());
    assert!(is_reserved_knowledge_event(lua, "*@observed").unwrap());

    // Non-matching event ids stay unreserved — `on("harvest_ended", fn)`
    // still falls through to the legacy `_event_handlers` path.
    assert!(!is_reserved_knowledge_event(lua, "harvest_ended").unwrap());
    assert!(!is_reserved_knowledge_event(lua, "sample:colony_famine@expired").unwrap());

    // The knowledge-specific subscriber accumulator exists but is empty
    // (K-3 will add entries via the extended `on(...)`).
    let subs: mlua::Table = lua.globals().get(KNOWLEDGE_SUBSCRIBERS_TABLE).unwrap();
    assert_eq!(subs.len().unwrap(), 0);
}

#[test]
fn sample_fixture_exposes_accumulator_shape() {
    // Sanity check: the raw accumulator matches the registry size. Guards
    // against a future refactor that splits accumulators without updating
    // the parser.
    let app = run_load_knowledge_kinds();
    let engine = app.world().resource::<ScriptEngine>();
    let lua = engine.lua();
    let acc: mlua::Table = lua.globals().get(KNOWLEDGE_DEF_ACCUMULATOR).unwrap();
    assert_eq!(acc.len().unwrap(), 4);

    // Reserved set size is 2 × kinds (one per lifecycle). The set is
    // hash-style, so iterate via `pairs` (raw `#` returns 0).
    let reserved: mlua::Table = lua.globals().get(KNOWLEDGE_RESERVED_EVENTS_TABLE).unwrap();
    let mut count = 0;
    for pair in reserved.pairs::<String, bool>() {
        let (_, v) = pair.unwrap();
        if v {
            count += 1;
        }
    }
    assert_eq!(count, 8);
}

#[test]
fn define_knowledge_rejects_duplicate_kinds_via_parser() {
    // Uses the Lua state exposed by the real ScriptEngine so we also cover
    // the `define_knowledge` global function registered in `globals.rs`.
    let app = run_load_knowledge_kinds();
    let engine = app.world().resource::<ScriptEngine>();
    let lua = engine.lua();

    lua.load(
        r#"
        define_knowledge { id = "tst:dup" }
        define_knowledge { id = "tst:dup" }
        "#,
    )
    .exec()
    .unwrap();

    let err = parse_knowledge_definitions(lua).unwrap_err();
    let s = err.to_string();
    assert!(s.contains("duplicate kind id 'tst:dup'"), "unexpected: {s}");
}

#[test]
fn define_knowledge_rejects_core_namespace_at_runtime() {
    let app = run_load_knowledge_kinds();
    let engine = app.world().resource::<ScriptEngine>();
    let lua = engine.lua();
    // Clear the accumulator first so prior sample fixture entries don't
    // interfere with the error message assertion (parser walks the whole
    // accumulator).
    let acc: mlua::Table = lua.globals().get(KNOWLEDGE_DEF_ACCUMULATOR).unwrap();
    for i in (1..=acc.len().unwrap()).rev() {
        acc.raw_remove(i).unwrap();
    }

    lua.load(r#"define_knowledge { id = "core:foo" }"#)
        .exec()
        .unwrap();
    let err = parse_knowledge_definitions(lua).unwrap_err();
    let s = err.to_string();
    assert!(
        s.contains("core:") && s.contains("reserved"),
        "unexpected: {s}"
    );
}

#[test]
fn kind_registry_insert_enforces_invariants() {
    // Targets `KindRegistry::insert` directly (no Lua), exercising the
    // same invariants the startup system relies on. The startup path
    // already logs+swallows these errors; here we prove the underlying
    // contract surfaces cleanly for tooling / future Rust-side callers.
    use macrocosmo::knowledge::kind_registry::{KindRegistryError, KnowledgeKindId, PayloadSchema};

    let mut reg = KindRegistry::default();

    reg.insert(KnowledgeKindDef {
        id: KnowledgeKindId::parse("mod:a").unwrap(),
        payload_schema: PayloadSchema::default(),
        origin: KindOrigin::Lua,
    })
    .unwrap();
    assert_eq!(reg.len(), 1);

    // Second insert with same id -> DuplicateKind.
    let err = reg
        .insert(KnowledgeKindDef {
            id: KnowledgeKindId::parse("mod:a").unwrap(),
            payload_schema: PayloadSchema::default(),
            origin: KindOrigin::Lua,
        })
        .unwrap_err();
    assert!(matches!(err, KindRegistryError::DuplicateKind(_)));

    // Lua-origin inserting into the core: namespace -> CoreNamespaceReserved.
    let err = reg
        .insert(KnowledgeKindDef {
            id: KnowledgeKindId::parse("core:xx").unwrap(),
            payload_schema: PayloadSchema::default(),
            origin: KindOrigin::Lua,
        })
        .unwrap_err();
    assert!(matches!(err, KindRegistryError::CoreNamespaceReserved(_)));

    // Core-origin for the same id is allowed (K-5 preload path).
    reg.insert(KnowledgeKindDef {
        id: KnowledgeKindId::parse("core:xx").unwrap(),
        payload_schema: PayloadSchema::default(),
        origin: KindOrigin::Core,
    })
    .unwrap();
}

#[test]
fn register_auto_lifecycle_events_is_idempotent_end_to_end() {
    // Re-running the reservation step against the same registry must not
    // bloat the reserved-events table or error — the startup system may
    // be invoked twice in hot-reload scenarios, and idempotence is the
    // contract we want modders to be able to rely on.
    let app = run_load_knowledge_kinds();
    let engine = app.world().resource::<ScriptEngine>();
    let lua = engine.lua();

    let defs = parse_knowledge_definitions(lua).unwrap();
    register_auto_lifecycle_events(lua, &defs).unwrap();
    register_auto_lifecycle_events(lua, &defs).unwrap();

    // Still only 8 entries (4 kinds × 2 lifecycles).
    let reserved: mlua::Table = lua.globals().get(KNOWLEDGE_RESERVED_EVENTS_TABLE).unwrap();
    let mut count = 0;
    for pair in reserved.pairs::<String, bool>() {
        let (_, v) = pair.unwrap();
        if v {
            count += 1;
        }
    }
    assert_eq!(count, 8);
}
