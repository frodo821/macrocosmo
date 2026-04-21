//! #332 spike: verify mlua 0.11 `Lua::scope` + `create_function_mut` +
//! non-'static capture works under our `send` feature flag, so we can
//! safely share a `RefCell<&mut World>` across read/write closures in a
//! single event dispatch.
//!
//! This is a spike test: minimal, isolated, meant to prove the pattern
//! before we land the full Option B refactor. If the spike panics or
//! fails, the plan-332 architecture needs revising before impl.

use mlua::{Lua, Result as LuaResult};
use std::cell::RefCell;

/// Core pattern: shared `RefCell<&mut T>` borrowed by read (immutable)
/// and write (mutable) closures across a single scope. The exterior
/// value sees mutation after the scope closes.
#[test]
fn spike_read_mutate_read_via_scope_closures() -> LuaResult<()> {
    let lua = Lua::new();
    let mut counter: i64 = 0;

    {
        let counter_cell = RefCell::new(&mut counter);

        lua.scope(|s| {
            let gs = lua.create_table()?;

            let read = s.create_function(|_, ()| -> LuaResult<i64> {
                let v = counter_cell
                    .try_borrow()
                    .map_err(|e| mlua::Error::RuntimeError(format!("try_borrow failed: {e}")))?;
                Ok(**v)
            })?;
            let inc = s.create_function_mut(|_, by: i64| -> LuaResult<()> {
                let mut v = counter_cell.try_borrow_mut().map_err(|e| {
                    mlua::Error::RuntimeError(format!("try_borrow_mut failed: {e}"))
                })?;
                **v += by;
                Ok(())
            })?;

            gs.set("read", read)?;
            gs.set("inc", inc)?;

            let diff: i64 = lua
                .load(
                    r#"
                    local gs = ...
                    local a = gs.read()
                    gs.inc(5)
                    gs.inc(3)
                    local b = gs.read()
                    return b - a
                "#,
                )
                .call(gs)?;

            assert_eq!(
                diff, 8,
                "Lua should observe the mutation live (live-within-tick)"
            );
            Ok(())
        })?;
    }

    assert_eq!(counter, 8, "mutation must persist outside scope");
    Ok(())
}

/// Capture resistance: a closure stored in a Lua global survives the
/// scope in the Lua VM, but calling it after `scope` returns must fail
/// cleanly (not panic). This is the "Lua-side capture 耐性" promise in
/// plan-332 §2.
#[test]
fn spike_capture_resistance_closure_invalid_after_scope() -> LuaResult<()> {
    let lua = Lua::new();
    let mut counter: i64 = 0;

    {
        let counter_cell = RefCell::new(&mut counter);

        lua.scope(|s| {
            let read = s.create_function(|_, ()| -> LuaResult<i64> {
                let v = counter_cell
                    .try_borrow()
                    .map_err(|e| mlua::Error::RuntimeError(format!("try_borrow failed: {e}")))?;
                Ok(**v)
            })?;

            // stash the closure into the Lua VM outside the scope's reach
            lua.globals().set("saved_read", read)?;

            // In-scope call works
            let ok: i64 = lua.load(r#"return saved_read()"#).call(())?;
            assert_eq!(ok, 0);
            Ok(())
        })?;
    }

    // Post-scope call must fail (not panic)
    let result: LuaResult<i64> = lua.load(r#"return saved_read()"#).call(());
    assert!(
        result.is_err(),
        "closure stored outside scope must error when invoked post-scope"
    );

    // Optional: inspect error kind. mlua typically surfaces a CallbackError
    // wrapping the original error; for the spike we just confirm it's not a panic.
    if let Err(e) = result {
        eprintln!("expected post-scope error: {e:?}");
    }
    Ok(())
}

/// Sequential read-then-write-then-read inside a single Lua chunk does
/// not trip `try_borrow*` because each closure invocation borrows and
/// releases before returning. This is the canonical "borrow → mutate →
/// drop → Lua 戻り" invariant from plan-332 §4.
#[test]
fn spike_sequential_borrows_do_not_conflict() -> LuaResult<()> {
    let lua = Lua::new();
    let mut data: Vec<i64> = vec![1, 2, 3];

    let data_cell = RefCell::new(&mut data);
    lua.scope(|s| {
        let gs = lua.create_table()?;

        let len = s.create_function(|_, ()| -> LuaResult<i64> {
            let v = data_cell
                .try_borrow()
                .map_err(|e| mlua::Error::RuntimeError(format!("try_borrow failed: {e}")))?;
            Ok(v.len() as i64)
        })?;
        let push = s.create_function_mut(|_, x: i64| -> LuaResult<()> {
            let mut v = data_cell
                .try_borrow_mut()
                .map_err(|e| mlua::Error::RuntimeError(format!("try_borrow_mut failed: {e}")))?;
            v.push(x);
            Ok(())
        })?;
        let first = s.create_function(|_, ()| -> LuaResult<Option<i64>> {
            let v = data_cell
                .try_borrow()
                .map_err(|e| mlua::Error::RuntimeError(format!("try_borrow failed: {e}")))?;
            Ok(v.first().copied())
        })?;

        gs.set("len", len)?;
        gs.set("push", push)?;
        gs.set("first", first)?;

        let result: i64 = lua
            .load(
                r#"
                local gs = ...
                gs.push(42)
                gs.push(43)
                local f = gs.first()
                return gs.len() * 100 + f
            "#,
            )
            .call(gs)?;

        // len=5 after 2 pushes, first=1 → 5*100+1 = 501
        assert_eq!(result, 501);
        Ok(())
    })?;

    assert_eq!(data, vec![1, 2, 3, 42, 43]);
    Ok(())
}

/// If a write closure body itself tries to re-enter Lua (violating
/// plan-332 §4 "apply_* は Lua 不接触"), reentrancy is only a concern
/// when that Lua code calls back into the same `RefCell`. Our invariant
/// forbids this path entirely, but verify the safety net: a deliberate
/// attempt to borrow_mut twice (without Lua involvement) surfaces as a
/// clean `mlua::Error::RuntimeError`, not a panic.
#[test]
fn spike_deliberate_double_borrow_mut_is_runtime_error() -> LuaResult<()> {
    let lua = Lua::new();
    let mut counter: i64 = 0;

    let counter_cell = RefCell::new(&mut counter);
    let hit_panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lua.scope(|s| {
            let bad = s.create_function_mut(|_, ()| -> LuaResult<()> {
                // Grab the outer borrow
                let _outer = counter_cell
                    .try_borrow_mut()
                    .map_err(|e| mlua::Error::RuntimeError(format!("outer: {e}")))?;
                // Attempt a second borrow_mut while the first is held
                let _inner = counter_cell
                    .try_borrow_mut()
                    .map_err(|e| mlua::Error::RuntimeError(format!("inner: {e}")))?;
                Ok(())
            })?;

            let res: LuaResult<()> = bad.call(());
            assert!(res.is_err(), "second borrow_mut must error");
            if let Err(e) = res {
                let msg = format!("{e:?}");
                assert!(
                    msg.contains("already borrowed") || msg.contains("inner:"),
                    "error should surface RefCell conflict, got: {msg}"
                );
            }
            Ok(())
        })
    }));

    assert!(
        hit_panic.is_ok(),
        "must not panic, only surface runtime error"
    );
    Ok(())
}
