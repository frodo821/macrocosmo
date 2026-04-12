//! Game-managed RNG resource and Lua bindings (`game_rand` global).
//!
//! Lua scripts should prefer `game_rand` over `math.random` so that all
//! random draws funnel through a single, replayable Bevy resource. This is
//! a prerequisite for future deterministic replays / save-game seeding.
//!
//! The RNG handle is wrapped in `Arc<Mutex<SmallRng>>` so it can be cloned
//! into Lua callbacks (which can fire from any system at any time).

use bevy::prelude::*;
use mlua::prelude::*;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use std::sync::{Arc, Mutex};

/// Game-managed RNG for Lua scripts. Wrapped in `Arc<Mutex<_>>` so it can
/// be shared with Lua callbacks (which can fire at any time).
#[derive(Resource, Clone)]
pub struct GameRng {
    inner: Arc<Mutex<SmallRng>>,
}

impl Default for GameRng {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SmallRng::from_os_rng())),
        }
    }
}

impl GameRng {
    /// Construct a deterministic RNG from a u64 seed.
    pub fn from_seed(seed: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SmallRng::seed_from_u64(seed))),
        }
    }

    /// Get a clone of the shared RNG handle (for passing to Lua callbacks
    /// or other long-lived consumers).
    pub fn handle(&self) -> Arc<Mutex<SmallRng>> {
        Arc::clone(&self.inner)
    }
}

/// Register the `game_rand` global table on the given Lua state, sharing
/// access to the supplied RNG handle.
///
/// Functions exposed to Lua:
/// - `game_rand.range(min, max) -> number` in `[min, max)`
/// - `game_rand.range_int(min, max) -> integer` in `[min, max]` inclusive
/// - `game_rand.chance(p) -> bool` (true with probability `p`)
/// - `game_rand.choice(table) -> any` (uniform pick from sequence)
/// - `game_rand.weighted({ {weight=N, value=X}, ... }) -> any`
pub fn register_game_rand(lua: &Lua, rng: Arc<Mutex<SmallRng>>) -> LuaResult<()> {
    let table = lua.create_table()?;

    // game_rand.range(min, max) -> f64 in [min, max)
    let rng_clone = Arc::clone(&rng);
    table.set(
        "range",
        lua.create_function(move |_, (min, max): (f64, f64)| {
            if !(min < max) {
                return Err(LuaError::runtime(format!(
                    "game_rand.range: require min < max (got min={min}, max={max})"
                )));
            }
            let mut rng = rng_clone.lock().unwrap();
            Ok(rng.random_range(min..max))
        })?,
    )?;

    // game_rand.range_int(min, max) -> i64 in [min, max] inclusive
    let rng_clone = Arc::clone(&rng);
    table.set(
        "range_int",
        lua.create_function(move |_, (min, max): (i64, i64)| {
            if min > max {
                return Err(LuaError::runtime(format!(
                    "game_rand.range_int: require min <= max (got min={min}, max={max})"
                )));
            }
            let mut rng = rng_clone.lock().unwrap();
            Ok(rng.random_range(min..=max))
        })?,
    )?;

    // game_rand.chance(p) -> bool
    let rng_clone = Arc::clone(&rng);
    table.set(
        "chance",
        lua.create_function(move |_, p: f64| {
            // Clamp gracefully so callers don't have to worry about
            // floating-point drift above 1.0 or slightly below 0.0.
            if p <= 0.0 {
                return Ok(false);
            }
            if p >= 1.0 {
                return Ok(true);
            }
            let mut rng = rng_clone.lock().unwrap();
            Ok(rng.random::<f64>() < p)
        })?,
    )?;

    // game_rand.choice(table) -> any — uniform pick from a sequence
    let rng_clone = Arc::clone(&rng);
    table.set(
        "choice",
        lua.create_function(move |_, items: LuaTable| {
            let len = items.len()?;
            if len == 0 {
                return Err(LuaError::runtime("game_rand.choice: empty table"));
            }
            let mut rng = rng_clone.lock().unwrap();
            let idx = rng.random_range(0..len) + 1; // Lua sequences are 1-indexed
            items.get::<LuaValue>(idx)
        })?,
    )?;

    // game_rand.weighted({ {weight=N, value=X}, ... }) -> X
    let rng_clone = Arc::clone(&rng);
    table.set(
        "weighted",
        lua.create_function(move |_, items: LuaTable| {
            let mut total_weight = 0.0_f64;
            let mut entries: Vec<(f64, LuaValue)> = Vec::new();
            for pair in items.sequence_values::<LuaTable>() {
                let entry = pair?;
                let weight: f64 = entry.get("weight")?;
                if weight < 0.0 {
                    return Err(LuaError::runtime(format!(
                        "game_rand.weighted: negative weight {weight}"
                    )));
                }
                let value: LuaValue = entry.get("value")?;
                total_weight += weight;
                entries.push((weight, value));
            }
            if entries.is_empty() {
                return Err(LuaError::runtime("game_rand.weighted: empty table"));
            }
            if total_weight <= 0.0 {
                return Err(LuaError::runtime(
                    "game_rand.weighted: total weight must be > 0",
                ));
            }
            let mut rng = rng_clone.lock().unwrap();
            let mut roll: f64 = rng.random::<f64>() * total_weight;
            // Re-iterate by index because we moved entries above. Use a
            // for-loop that consumes the vec by value.
            let last_idx = entries.len() - 1;
            for (i, (weight, value)) in entries.into_iter().enumerate() {
                roll -= weight;
                if roll <= 0.0 || i == last_idx {
                    return Ok(value);
                }
            }
            // Unreachable: the i == last_idx branch above always returns.
            Err(LuaError::runtime("game_rand.weighted: roll fell through"))
        })?,
    )?;

    lua.globals().set("game_rand", table)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_lua_with_rng(seed: u64) -> Lua {
        let lua = Lua::new();
        let rng = GameRng::from_seed(seed);
        register_game_rand(&lua, rng.handle()).unwrap();
        lua
    }

    #[test]
    fn range_returns_value_in_bounds() {
        let lua = make_lua_with_rng(1);
        for _ in 0..100 {
            let v: f64 = lua
                .load("return game_rand.range(2.5, 7.5)")
                .eval()
                .unwrap();
            assert!(v >= 2.5 && v < 7.5, "got {v}");
        }
    }

    #[test]
    fn range_rejects_invalid_bounds() {
        let lua = make_lua_with_rng(1);
        let err = lua.load("return game_rand.range(5.0, 5.0)").eval::<f64>();
        assert!(err.is_err());
        let err = lua.load("return game_rand.range(5.0, 1.0)").eval::<f64>();
        assert!(err.is_err());
    }

    #[test]
    fn range_int_is_inclusive_and_bounded() {
        let lua = make_lua_with_rng(2);
        let mut saw_min = false;
        let mut saw_max = false;
        for _ in 0..2000 {
            let v: i64 = lua
                .load("return game_rand.range_int(1, 6)")
                .eval()
                .unwrap();
            assert!((1..=6).contains(&v), "got {v}");
            if v == 1 {
                saw_min = true;
            }
            if v == 6 {
                saw_max = true;
            }
        }
        assert!(saw_min, "never saw lower bound 1");
        assert!(saw_max, "never saw upper bound 6 (range should be inclusive)");
    }

    #[test]
    fn range_int_singleton_works() {
        let lua = make_lua_with_rng(3);
        let v: i64 = lua
            .load("return game_rand.range_int(7, 7)")
            .eval()
            .unwrap();
        assert_eq!(v, 7);
    }

    #[test]
    fn chance_zero_is_always_false() {
        let lua = make_lua_with_rng(4);
        for _ in 0..200 {
            let v: bool = lua.load("return game_rand.chance(0.0)").eval().unwrap();
            assert!(!v);
        }
    }

    #[test]
    fn chance_one_is_always_true() {
        let lua = make_lua_with_rng(5);
        for _ in 0..200 {
            let v: bool = lua.load("return game_rand.chance(1.0)").eval().unwrap();
            assert!(v);
        }
    }

    #[test]
    fn chance_half_produces_both_outcomes() {
        let lua = make_lua_with_rng(6);
        let mut t = 0;
        let mut f = 0;
        for _ in 0..500 {
            let v: bool = lua.load("return game_rand.chance(0.5)").eval().unwrap();
            if v {
                t += 1;
            } else {
                f += 1;
            }
        }
        assert!(t > 100 && f > 100, "biased: {t} true vs {f} false");
    }

    #[test]
    fn choice_returns_table_element() {
        let lua = make_lua_with_rng(7);
        for _ in 0..50 {
            let v: String = lua
                .load(r#"return game_rand.choice({"a", "b", "c"})"#)
                .eval()
                .unwrap();
            assert!(matches!(v.as_str(), "a" | "b" | "c"));
        }
    }

    #[test]
    fn choice_empty_table_errors() {
        let lua = make_lua_with_rng(8);
        let res = lua.load("return game_rand.choice({})").eval::<LuaValue>();
        assert!(res.is_err());
    }

    #[test]
    fn weighted_distribution_matches_weights() {
        let lua = make_lua_with_rng(9);
        // 90% "common", 10% "rare"
        let mut common = 0;
        let mut rare = 0;
        for _ in 0..2000 {
            let v: String = lua
                .load(
                    r#"return game_rand.weighted({
                        { weight = 90, value = "common" },
                        { weight = 10, value = "rare" },
                    })"#,
                )
                .eval()
                .unwrap();
            match v.as_str() {
                "common" => common += 1,
                "rare" => rare += 1,
                other => panic!("unexpected value {other}"),
            }
        }
        // Expect ~1800 common / ~200 rare. Allow generous slack.
        assert!(common > 1600 && common < 1950, "common = {common}");
        assert!(rare > 50 && rare < 400, "rare = {rare}");
    }

    #[test]
    fn weighted_empty_table_errors() {
        let lua = make_lua_with_rng(10);
        let res = lua
            .load("return game_rand.weighted({})")
            .eval::<LuaValue>();
        assert!(res.is_err());
    }

    #[test]
    fn weighted_zero_total_errors() {
        let lua = make_lua_with_rng(11);
        let res = lua
            .load(
                r#"return game_rand.weighted({
                    { weight = 0, value = "a" },
                    { weight = 0, value = "b" },
                })"#,
            )
            .eval::<LuaValue>();
        assert!(res.is_err());
    }

    #[test]
    fn weighted_negative_weight_errors() {
        let lua = make_lua_with_rng(12);
        let res = lua
            .load(
                r#"return game_rand.weighted({
                    { weight = -1, value = "a" },
                })"#,
            )
            .eval::<LuaValue>();
        assert!(res.is_err());
    }

    #[test]
    fn from_seed_is_deterministic() {
        let lua_a = make_lua_with_rng(42);
        let lua_b = make_lua_with_rng(42);
        let a: f64 = lua_a.load("return game_rand.range(0, 1)").eval().unwrap();
        let b: f64 = lua_b.load("return game_rand.range(0, 1)").eval().unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn handle_shares_state() {
        // Two clones of the same handle should advance the same RNG stream.
        let rng = GameRng::from_seed(123);
        let lua = Lua::new();
        register_game_rand(&lua, rng.handle()).unwrap();

        let first: f64 = lua.load("return game_rand.range(0, 1)").eval().unwrap();

        // Pull a value via the original handle directly.
        let direct: f64 = {
            let handle = rng.handle();
            let mut g = handle.lock().unwrap();
            g.random::<f64>()
        };

        let third: f64 = lua.load("return game_rand.range(0, 1)").eval().unwrap();

        // All three values should differ (sharing one stream means each call advances state).
        assert!(first != direct);
        assert!(direct != third);
        assert!(first != third);
    }
}
