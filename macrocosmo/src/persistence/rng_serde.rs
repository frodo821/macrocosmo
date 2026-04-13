//! Serde helpers for the [`GameRng`](crate::scripting::game_rng::GameRng) resource.
//!
//! ## Design
//!
//! Uses `rand_xoshiro::Xoshiro256PlusPlus` directly (via the `serde` feature)
//! instead of `rand::rngs::SmallRng`, because `SmallRng` keeps its inner
//! generator private and does not implement `Serialize`/`Deserialize`.
//!
//! Because the full generator state is captured bit-for-bit, save→load
//! continues the **exact same stream** (not a derived one). A save produced
//! after N draws and then loaded yields the same (N+1)th draw as the live
//! RNG would have produced without ever saving.

use crate::scripting::game_rng::GameRng;
use rand_xoshiro::Xoshiro256PlusPlus;
use serde::{Deserialize, Serialize};

/// Wire-format snapshot of a [`GameRng`] for inclusion in `SavedResources`.
///
/// Carries the complete generator state so that save→load yields
/// bit-for-bit stream continuation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedGameRng {
    /// Full internal state of the Xoshiro256++ generator at capture time.
    pub state: Xoshiro256PlusPlus,
}

impl SavedGameRng {
    /// Capture the current RNG's full state. The restored RNG will produce
    /// the exact same subsequent sequence as the live RNG would have.
    pub fn capture(rng: &GameRng) -> Result<Self, postcard::Error> {
        let handle = rng.handle();
        let g = handle.lock().expect("GameRng mutex poisoned");
        Ok(Self { state: g.clone() })
    }

    /// Restore the snapshot into a fresh [`GameRng`] carrying the exact
    /// state captured at save time.
    pub fn restore(&self) -> Result<GameRng, postcard::Error> {
        Ok(GameRng::from_xoshiro(self.state.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    #[test]
    fn capture_and_restore_is_deterministic() {
        // Same seed → same state captured → identical restored streams.
        let a = GameRng::from_seed(42);
        let b = GameRng::from_seed(42);

        let snap_a = SavedGameRng::capture(&a).expect("capture a");
        let snap_b = SavedGameRng::capture(&b).expect("capture b");

        let restored_a = snap_a.restore().expect("restore a");
        let restored_b = snap_b.restore().expect("restore b");

        let mut xs: Vec<u64> = Vec::new();
        let mut ys: Vec<u64> = Vec::new();
        {
            let ha = restored_a.handle();
            let mut ga = ha.lock().unwrap();
            let hb = restored_b.handle();
            let mut gb = hb.lock().unwrap();
            for _ in 0..10 {
                xs.push(ga.random());
                ys.push(gb.random());
            }
        }
        assert_eq!(xs, ys, "restored RNGs must produce matching streams");
    }

    #[test]
    fn independent_seeds_diverge() {
        let a = GameRng::from_seed(1);
        let b = GameRng::from_seed(2);
        let snap_a = SavedGameRng::capture(&a).unwrap();
        let snap_b = SavedGameRng::capture(&b).unwrap();

        let restored_a = snap_a.restore().unwrap();
        let restored_b = snap_b.restore().unwrap();

        let ha = restored_a.handle();
        let hb = restored_b.handle();
        let mut ga = ha.lock().unwrap();
        let mut gb = hb.lock().unwrap();
        let xa: u64 = ga.random();
        let xb: u64 = gb.random();
        assert_ne!(xa, xb, "different seeds must diverge after restore");
    }

    #[test]
    fn save_load_continues_exact_stream() {
        // Bit-for-bit stream continuation: after draining N values from the
        // live RNG, a save captured at that point yields a restored RNG
        // whose next draw matches what the live RNG produces next.
        let live = GameRng::from_seed(12345);
        let n_draws = 7;
        let expected: Vec<u64>;
        let snap: SavedGameRng;

        {
            let h = live.handle();
            let mut g = h.lock().unwrap();
            for _ in 0..n_draws {
                let _: u64 = g.random();
            }
        }
        snap = SavedGameRng::capture(&live).expect("capture after N draws");
        {
            let h = live.handle();
            let mut g = h.lock().unwrap();
            expected = (0..10).map(|_| g.random::<u64>()).collect();
        }

        let restored = snap.restore().expect("restore");
        let actual: Vec<u64> = {
            let h = restored.handle();
            let mut g = h.lock().unwrap();
            (0..10).map(|_| g.random::<u64>()).collect()
        };
        assert_eq!(
            actual, expected,
            "restored RNG must continue the exact stream from save point"
        );
    }
}
