//! #352 (K-3) Spike 10.1 (plan-349 §10): `seal_immutable_keys` metatable
//! behaviour.
//!
//! Minimal reproduction to verify `mlua::Table::set_metatable` +
//! `__newindex` blocks writes to a fixed key set while allowing writes
//! to other keys (and nested tables) to pass through. This is the
//! foundation the K-3 `dispatch_knowledge` chain relies on to deliver
//! sealed `kind` / `origin_system` / `recorded_at` / `observed_at` /
//! `observer_empire` / `lag_hexadies` fields to Lua subscribers while
//! still letting them mutate `payload.*` entries.
//!
//! K-3 commit 1: land the helper + this spike before the routing /
//! dispatcher commits that consume it (Commit 3-4).

use macrocosmo::scripting::knowledge_dispatch::seal_immutable_keys;
use mlua::prelude::*;

#[test]
fn spike_seal_immutable_keys_blocks_sealed_key_write() {
    let lua = Lua::new();
    let t = lua.create_table().unwrap();
    t.set("kind", "vesk:famine_outbreak").unwrap();
    t.set("origin_system", 42_u64).unwrap();
    t.set("payload", lua.create_table().unwrap()).unwrap();

    seal_immutable_keys(&lua, &t, &["kind", "origin_system"]).unwrap();
    lua.globals().set("_t", t.clone()).unwrap();

    // Writing to a sealed key must error.
    let r: mlua::Result<()> = lua.load(r#"_t.kind = "other""#).exec();
    assert!(r.is_err(), "writing sealed key 'kind' must error");
    let msg = format!("{}", r.unwrap_err());
    assert!(
        msg.contains("immutable knowledge payload key 'kind'"),
        "error message should identify the sealed key, got: {msg}"
    );

    // origin_system write also blocked.
    let r: mlua::Result<()> = lua.load(r#"_t.origin_system = 99"#).exec();
    assert!(r.is_err(), "writing sealed key 'origin_system' must error");
}

#[test]
fn spike_seal_immutable_keys_allows_payload_mutation() {
    let lua = Lua::new();
    let t = lua.create_table().unwrap();
    t.set("kind", "foo").unwrap();
    let payload = lua.create_table().unwrap();
    payload.set("severity", 0.1).unwrap();
    t.set("payload", payload).unwrap();

    seal_immutable_keys(&lua, &t, &["kind"]).unwrap();
    lua.globals().set("_t", t.clone()).unwrap();

    // Mutating a nested mutable table must succeed.
    lua.load(r#"_t.payload.severity = 0.9"#).exec().unwrap();
    lua.load(r#"_t.payload.added = "yes""#).exec().unwrap();

    // Sealed key still reads back its original value.
    let kind: String = lua.load(r#"return _t.kind"#).eval().unwrap();
    assert_eq!(kind, "foo");
    let severity: f64 = lua.load(r#"return _t.payload.severity"#).eval().unwrap();
    assert!((severity - 0.9).abs() < 1e-9);
    let added: String = lua.load(r#"return _t.payload.added"#).eval().unwrap();
    assert_eq!(added, "yes");
}

#[test]
fn spike_seal_immutable_keys_allows_new_unsealed_key_write() {
    let lua = Lua::new();
    let t = lua.create_table().unwrap();
    t.set("kind", "foo").unwrap();
    seal_immutable_keys(&lua, &t, &["kind"]).unwrap();
    lua.globals().set("_t", t.clone()).unwrap();

    // Writing a brand-new non-sealed key must succeed.
    lua.load(r#"_t.arbitrary = 123"#).exec().unwrap();
    let v: i64 = lua.load(r#"return _t.arbitrary"#).eval().unwrap();
    assert_eq!(v, 123);
}

#[test]
fn spike_seal_immutable_keys_read_after_seal_returns_original() {
    let lua = Lua::new();
    let t = lua.create_table().unwrap();
    t.set("kind", "sealed_value").unwrap();
    t.set("origin_system", 7_u64).unwrap();

    seal_immutable_keys(&lua, &t, &["kind", "origin_system"]).unwrap();
    lua.globals().set("_t", t.clone()).unwrap();

    // Reading sealed keys must still return the original values via __index.
    let kind: String = lua.load(r#"return _t.kind"#).eval().unwrap();
    assert_eq!(kind, "sealed_value");
    let os: u64 = lua.load(r#"return _t.origin_system"#).eval().unwrap();
    assert_eq!(os, 7);
}
