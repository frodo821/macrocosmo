//! Serde helpers for the [`GameRng`](crate::scripting::game_rng::GameRng) resource.
//!
//! ## Design note (Phase A compromise)
//!
//! `rand 0.9`'s `SmallRng` does **not** implement `Serialize`/`Deserialize`
//! directly — its inner xoshiro generator does, but the `SmallRng` wrapper
//! keeps it private. Rather than add a new RNG crate dependency or replace
//! the runtime type, Phase A captures a fresh *reseed* from the current
//! stream: we pull one `u64` from the live RNG and use it to seed a
//! deterministic successor.
//!
//! **Consequence**: the restored RNG continues a *derived* stream rather than
//! the exact stream that was running at save time. This is deterministic
//! across save→load→replay but does NOT give bit-for-bit continuation — a
//! limitation we accept for Phase A. Phase C may upgrade to a directly-
//! serializable RNG (rand_xoshiro, bevy_prng) without changing the on-disk
//! format (the seed-based wire layout survives the swap).

use crate::scripting::game_rng::GameRng;
use rand::Rng;
use serde::{Deserialize, Serialize};

/// Wire-format snapshot of a [`GameRng`] for inclusion in `SavedResources`.
/// Carries a single u64 successor seed; see the module-level design note.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedGameRng {
    /// Successor seed: pulled from the live RNG at capture time.
    pub successor_seed: u64,
}

impl SavedGameRng {
    /// Capture the current RNG by pulling a fresh u64 seed from its stream.
    /// The restored RNG will produce the same sequence as a fresh
    /// `SmallRng::seed_from_u64(successor_seed)`.
    pub fn capture(rng: &GameRng) -> Result<Self, postcard::Error> {
        let handle = rng.handle();
        let seed: u64 = {
            let mut g = handle.lock().expect("GameRng mutex poisoned");
            g.random()
        };
        Ok(Self { successor_seed: seed })
    }

    /// Restore the snapshot into a fresh [`GameRng`] seeded from
    /// `successor_seed`.
    pub fn restore(&self) -> Result<GameRng, postcard::Error> {
        Ok(GameRng::from_seed(self.successor_seed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_and_restore_is_deterministic() {
        // Two saves from two fresh RNGs with the same seed must capture the
        // same successor seed, and therefore produce the same restored stream.
        let a = GameRng::from_seed(42);
        let b = GameRng::from_seed(42);

        let snap_a = SavedGameRng::capture(&a).expect("capture a");
        let snap_b = SavedGameRng::capture(&b).expect("capture b");
        assert_eq!(snap_a.successor_seed, snap_b.successor_seed);

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
        assert_ne!(snap_a.successor_seed, snap_b.successor_seed);
    }
}
