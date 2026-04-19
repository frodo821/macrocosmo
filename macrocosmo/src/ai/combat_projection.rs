//! Combat projection — Monte Carlo dry-run of [`simulate_combat`].
//!
//! This is a pure function (no ECS dependency) that wraps the combat
//! simulator to produce averaged win/loss statistics over multiple trials.
//! It lives in `macrocosmo/src/ai/` rather than `macrocosmo-ai/` because
//! it depends on [`ShipProfile`] and [`CombatConfig`] from the game crate.

use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::ship::combat_sim::{CombatConfig, CombatOutcome, ShipProfile, simulate_combat};

/// Result of a combat projection (Monte Carlo over [`simulate_combat`]).
#[derive(Debug, Clone)]
pub struct CombatProjection {
    /// Fraction of trials where the attacker won.
    pub win_probability: f64,
    /// Average fraction of attacker ships surviving (across all trials).
    pub avg_surviving_fraction: f64,
    /// Average fraction of defender ships surviving (across all trials).
    pub avg_enemy_surviving_fraction: f64,
    /// Confidence in the projection: `1.0 - stddev(win_rate_per_batch)`.
    /// Higher means more consistent results across trials.
    pub confidence: f64,
    /// Number of trials run.
    pub trials: u32,
}

/// Run [`simulate_combat`] `trials` times with different RNG seeds and
/// return averaged results.
///
/// Both `my_fleet` and `enemy_fleet` are cloned per trial so the originals
/// are not mutated. The base seed is `0`; each trial uses `seed = i`.
pub fn project_combat(
    my_fleet: &[ShipProfile],
    enemy_fleet: &[ShipProfile],
    config: &CombatConfig,
    trials: u32,
) -> CombatProjection {
    if trials == 0 {
        return CombatProjection {
            win_probability: 0.0,
            avg_surviving_fraction: 0.0,
            avg_enemy_surviving_fraction: 0.0,
            confidence: 0.0,
            trials: 0,
        };
    }

    let my_count = my_fleet.len() as f64;
    let enemy_count = enemy_fleet.len() as f64;

    let mut wins: u32 = 0;
    let mut total_my_surviving: f64 = 0.0;
    let mut total_enemy_surviving: f64 = 0.0;
    // For confidence: track per-trial win (1.0 or 0.0) to compute variance.
    let mut win_values: Vec<f64> = Vec::with_capacity(trials as usize);

    for seed in 0..trials {
        let mut attackers = my_fleet.to_vec();
        let mut defenders = enemy_fleet.to_vec();
        let mut rng = StdRng::seed_from_u64(seed as u64);

        let log = simulate_combat(&mut attackers, &mut defenders, config, &mut rng);

        let won = matches!(log.outcome, CombatOutcome::AttackerWon { .. });
        if won {
            wins += 1;
        }
        win_values.push(if won { 1.0 } else { 0.0 });

        // Count surviving ships.
        let my_alive = attackers.iter().filter(|s| s.is_alive()).count() as f64;
        let enemy_alive = defenders.iter().filter(|s| s.is_alive()).count() as f64;

        if my_count > 0.0 {
            total_my_surviving += my_alive / my_count;
        }
        if enemy_count > 0.0 {
            total_enemy_surviving += enemy_alive / enemy_count;
        }
    }

    let n = trials as f64;
    let win_probability = wins as f64 / n;
    let avg_surviving_fraction = total_my_surviving / n;
    let avg_enemy_surviving_fraction = total_enemy_surviving / n;

    // Confidence: 1 - stddev of win rate. For a Bernoulli variable,
    // stddev = sqrt(p * (1-p)). Max stddev is 0.5 (at p=0.5).
    let variance = win_probability * (1.0 - win_probability);
    let stddev = variance.sqrt();
    let confidence = (1.0 - stddev * 2.0).clamp(0.0, 1.0);

    CombatProjection {
        win_probability,
        avg_surviving_fraction,
        avg_enemy_surviving_fraction,
        confidence,
        trials,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ship_design::WeaponStats;

    fn laser() -> WeaponStats {
        WeaponStats {
            track: 5.0,
            precision: 0.85,
            cooldown: 1,
            range: 10.0,
            shield_damage: 4.0,
            shield_damage_div: 1.0,
            shield_piercing: 0.0,
            armor_damage: 2.0,
            armor_damage_div: 0.5,
            armor_piercing: 0.0,
            hull_damage: 3.0,
            hull_damage_div: 1.0,
        }
    }

    /// Config with enough turns to resolve combat decisively.
    fn projection_config() -> CombatConfig {
        CombatConfig {
            turns_per_tick: 200,
            ..CombatConfig::default()
        }
    }

    fn make_ship(weapons: Vec<WeaponStats>, speed: f64) -> ShipProfile {
        ShipProfile {
            weapons,
            hull: 50.0,
            hull_max: 50.0,
            armor: 20.0,
            armor_max: 20.0,
            shield: 10.0,
            shield_max: 10.0,
            shield_regen: 1.0,
            evasion: 2.0,
            speed,
            shield_regen_cooldown: 0,
        }
    }

    #[test]
    fn project_combat_equal_forces_is_roughly_even() {
        let my_fleet: Vec<_> = (0..3).map(|_| make_ship(vec![laser()], 2.0)).collect();
        let enemy_fleet: Vec<_> = (0..3).map(|_| make_ship(vec![laser()], 2.0)).collect();

        let result = project_combat(&my_fleet, &enemy_fleet, &projection_config(), 200);

        // With identical fleets, win rate should be roughly 50%.
        assert!(
            (0.2..=0.8).contains(&result.win_probability),
            "Expected roughly even win rate, got {}",
            result.win_probability
        );
        assert_eq!(result.trials, 200);
    }

    #[test]
    fn project_combat_stronger_fleet_wins_more() {
        let my_fleet: Vec<_> = (0..6).map(|_| make_ship(vec![laser()], 2.0)).collect();
        let enemy_fleet: Vec<_> = (0..3).map(|_| make_ship(vec![laser()], 2.0)).collect();

        let result = project_combat(&my_fleet, &enemy_fleet, &projection_config(), 200);

        // 2x fleet should win a clear majority.
        assert!(
            result.win_probability > 0.7,
            "Expected >70% win rate for 2x fleet, got {}",
            result.win_probability
        );
        // Stronger fleet should lose fewer ships on average.
        assert!(
            result.avg_surviving_fraction > result.avg_enemy_surviving_fraction,
            "Stronger fleet should have higher survival: my={}, enemy={}",
            result.avg_surviving_fraction,
            result.avg_enemy_surviving_fraction
        );
    }

    #[test]
    fn project_combat_zero_trials() {
        let my_fleet = vec![make_ship(vec![laser()], 2.0)];
        let enemy_fleet = vec![make_ship(vec![laser()], 2.0)];

        let result = project_combat(&my_fleet, &enemy_fleet, &CombatConfig::default(), 0);
        assert_eq!(result.trials, 0);
        assert!((result.win_probability - 0.0).abs() < 1e-9);
    }

    #[test]
    fn project_combat_confidence_higher_when_decisive() {
        // One-sided fight should have high confidence.
        let my_fleet: Vec<_> = (0..10).map(|_| make_ship(vec![laser()], 2.0)).collect();
        let enemy_fleet = vec![make_ship(vec![laser()], 2.0)];

        let result = project_combat(&my_fleet, &enemy_fleet, &projection_config(), 100);

        // Decisive victory should yield high confidence.
        assert!(
            result.confidence > 0.5,
            "Decisive fight should have high confidence, got {}",
            result.confidence
        );
    }
}
