//! Pure, ECS-independent combat simulation.
//!
//! This module provides a headless combat resolver that operates on plain
//! [`ShipProfile`] structs — no Bevy queries, no entities. The existing
//! [`super::combat::resolve_combat`] system extracts profiles from the ECS
//! world and delegates here, then writes the results back.

use rand::Rng;

use crate::ship_design::WeaponStats;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Default number of turns shield regen is suppressed after taking damage.
pub const DEFAULT_SHIELD_REGEN_DELAY: u32 = 3;

/// Ship profile extracted from ECS for simulation.
#[derive(Clone, Debug)]
pub struct ShipProfile {
    pub weapons: Vec<WeaponStats>,
    pub hull: f64,
    pub hull_max: f64,
    pub armor: f64,
    pub armor_max: f64,
    pub shield: f64,
    pub shield_max: f64,
    pub shield_regen: f64,
    pub evasion: f64,
    /// Sublight speed — used for distance control between fleets.
    pub speed: f64,
    /// Turns remaining before shield regen resumes (reset on hit).
    pub shield_regen_cooldown: u32,
}

impl ShipProfile {
    /// Total remaining hit points across all layers.
    pub fn total_hp(&self) -> f64 {
        self.hull + self.armor + self.shield
    }

    pub fn is_alive(&self) -> bool {
        self.hull > 0.0
    }
}

/// Tuning knobs for the simulation.
#[derive(Clone, Debug)]
pub struct CombatConfig {
    /// How many combat turns to simulate per game tick.
    pub turns_per_tick: u32,
    /// How much distance changes per unit of speed difference each turn.
    pub distance_step_factor: f64,
    /// Turns shield regen is suppressed after taking a hit.
    pub shield_regen_delay: u32,
}

impl Default for CombatConfig {
    fn default() -> Self {
        Self {
            turns_per_tick: 12,
            distance_step_factor: 1.0,
            shield_regen_delay: DEFAULT_SHIELD_REGEN_DELAY,
        }
    }
}

/// Per-turn snapshot for the combat log.
#[derive(Clone, Debug)]
pub struct TurnLog {
    pub turn: u32,
    pub distance: f64,
    pub attacker_ships_alive: u32,
    pub defender_ships_alive: u32,
    pub attacker_total_hp: f64,
    pub defender_total_hp: f64,
    pub weapons_fired: u32,
    pub damage_dealt_by_attacker: f64,
    pub damage_dealt_by_defender: f64,
}

/// Final outcome of the combat.
#[derive(Clone, Debug, PartialEq)]
pub enum CombatOutcome {
    AttackerWon {
        surviving_fraction: f64,
    },
    DefenderWon {
        surviving_fraction: f64,
    },
    /// Both sides ran out of weapons or the turn limit was reached.
    Stalemate,
}

/// Full combat log returned by [`simulate_combat`].
#[derive(Clone, Debug)]
pub struct CombatLog {
    pub turns: Vec<TurnLog>,
    pub outcome: CombatOutcome,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Hit chance: `precision * track / (track + evasion)`.
fn hit_chance(weapon: &WeaponStats, target_evasion: f64) -> f64 {
    weapon.precision * (weapon.track / (weapon.track + target_evasion))
}

/// Apply weapon damage through the 3-layer HP model.
/// Returns the total HP removed. Resets shield regen cooldown on any hit.
fn apply_damage_to_profile(
    target: &mut ShipProfile,
    weapon: &WeaponStats,
    rng: &mut impl Rng,
    shield_regen_delay: u32,
) -> f64 {
    let mut removed = 0.0;

    // Any damage suppresses shield regen.
    target.shield_regen_cooldown = shield_regen_delay;

    // Shield phase
    if target.shield > 0.0 && rng.random::<f64>() >= weapon.shield_piercing {
        let dmg = (weapon.shield_damage
            + weapon.shield_damage_div * (rng.random::<f64>() * 2.0 - 1.0))
            .max(0.0);
        let actual = dmg.min(target.shield);
        target.shield -= actual;
        removed += actual;
        return removed;
    }

    // Armor phase
    if target.armor > 0.0 && rng.random::<f64>() >= weapon.armor_piercing {
        let dmg = (weapon.armor_damage
            + weapon.armor_damage_div * (rng.random::<f64>() * 2.0 - 1.0))
            .max(0.0);
        let actual = dmg.min(target.armor);
        target.armor -= actual;
        removed += actual;
        return removed;
    }

    // Hull phase
    let dmg =
        (weapon.hull_damage + weapon.hull_damage_div * (rng.random::<f64>() * 2.0 - 1.0)).max(0.0);
    let actual = dmg.min(target.hull);
    target.hull -= actual;
    removed += actual;
    removed
}

/// Compute the *preferred range* for a fleet — the distance at which its
/// aggregate DPS is maximised. This is simply the shortest weapon range
/// across all alive ships (closer is always equal or better since all weapons
/// with range >= distance can fire).
fn preferred_range(fleet: &[ShipProfile]) -> f64 {
    fleet
        .iter()
        .filter(|s| s.is_alive())
        .flat_map(|s| s.weapons.iter().map(|w| w.range))
        .fold(f64::MAX, f64::min)
        .max(0.0)
}

/// Average speed of alive ships in the fleet.
fn avg_speed(fleet: &[ShipProfile]) -> f64 {
    let (sum, count) = fleet
        .iter()
        .filter(|s| s.is_alive())
        .fold((0.0, 0u32), |(s, c), ship| (s + ship.speed, c + 1));
    if count == 0 { 0.0 } else { sum / count as f64 }
}

/// Count alive ships.
fn alive_count(fleet: &[ShipProfile]) -> u32 {
    fleet.iter().filter(|s| s.is_alive()).count() as u32
}

/// Sum of total HP across all alive ships.
fn total_fleet_hp(fleet: &[ShipProfile]) -> f64 {
    fleet
        .iter()
        .filter(|s| s.is_alive())
        .map(|s| s.total_hp())
        .sum()
}

/// Check whether a fleet has any weapons at all (alive ships only).
fn fleet_has_weapons(fleet: &[ShipProfile]) -> bool {
    fleet.iter().any(|s| s.is_alive() && !s.weapons.is_empty())
}

// ---------------------------------------------------------------------------
// Core simulation
// ---------------------------------------------------------------------------

/// Run a pure combat simulation between two fleets.
///
/// Mutates the profiles in place (HP is reduced as damage is dealt) and
/// returns a [`CombatLog`] recording every turn.
pub fn simulate_combat(
    attackers: &mut [ShipProfile],
    defenders: &mut [ShipProfile],
    config: &CombatConfig,
    rng: &mut impl Rng,
) -> CombatLog {
    let mut turns = Vec::new();

    // Edge case: one or both sides empty.
    if attackers.is_empty() || defenders.is_empty() {
        let outcome = if attackers.is_empty() && defenders.is_empty() {
            CombatOutcome::Stalemate
        } else if attackers.is_empty() {
            CombatOutcome::DefenderWon {
                surviving_fraction: 1.0,
            }
        } else {
            CombatOutcome::AttackerWon {
                surviving_fraction: 1.0,
            }
        };
        return CombatLog { turns, outcome };
    }

    // Initial distance = max weapon range across all participating ships.
    let max_range = attackers
        .iter()
        .chain(defenders.iter())
        .flat_map(|s| s.weapons.iter().map(|w| w.range))
        .fold(0.0_f64, f64::max);
    let initial_distance = max_range.max(1.0); // at least 1.0 so distance is meaningful
    let mut distance = initial_distance;

    let initial_attacker_count = attackers.len() as f64;
    let initial_defender_count = defenders.len() as f64;

    for turn in 0..config.turns_per_tick {
        let att_alive = alive_count(attackers);
        let def_alive = alive_count(defenders);

        // Both sides annihilated simultaneously (or one side wiped last turn).
        if att_alive == 0 || def_alive == 0 {
            break;
        }

        // --- 1. Distance update ---
        let speed_a = avg_speed(attackers);
        let speed_b = avg_speed(defenders);
        let pref_a = preferred_range(attackers);
        let pref_b = preferred_range(defenders);

        // Each side wants to move toward its preferred range.
        // The faster side gets more influence over the distance.
        let speed_diff = speed_a - speed_b;
        if speed_diff.abs() > f64::EPSILON {
            let delta = speed_diff.abs() * config.distance_step_factor;
            if speed_diff > 0.0 {
                // Attackers are faster — move toward attacker's preferred range.
                if distance > pref_a {
                    distance = (distance - delta).max(pref_a);
                } else if distance < pref_a {
                    distance = (distance + delta).min(pref_a);
                }
            } else {
                // Defenders are faster — move toward defender's preferred range.
                if distance > pref_b {
                    distance = (distance - delta).max(pref_b);
                } else if distance < pref_b {
                    distance = (distance + delta).min(pref_b);
                }
            }
        }
        distance = distance.clamp(0.0, initial_distance);

        // --- 2. Weapon fire (simultaneous) ---
        let mut weapons_fired: u32 = 0;
        let mut dmg_by_attacker: f64 = 0.0;
        let mut dmg_by_defender: f64 = 0.0;

        // Fire weapons for both sides. Attacker fires first, then defender,
        // but destruction is checked only after both sides have shot — so
        // simultaneous fire is approximated.

        // Attacker fires
        {
            // Clone attacker weapon refs to avoid borrow conflict.
            let attacker_weapons: Vec<(Vec<WeaponStats>, f64)> = attackers
                .iter()
                .filter(|s| s.is_alive())
                .map(|s| (s.weapons.clone(), s.evasion))
                .collect();

            for (weapons, _) in &attacker_weapons {
                for weapon in weapons {
                    if weapon.range < distance {
                        continue;
                    }
                    if weapon.cooldown > 0 && (turn as i64) % weapon.cooldown != 0 {
                        continue;
                    }
                    // Find alive target.
                    let Some(target_idx) = find_weakest_alive_target(defenders) else {
                        break;
                    };
                    let target_evasion = defenders[target_idx].evasion;
                    let chance = hit_chance(weapon, target_evasion);
                    if rng.random::<f64>() < chance {
                        let dmg = apply_damage_to_profile(&mut defenders[target_idx], weapon, rng, config.shield_regen_delay);
                        dmg_by_attacker += dmg;
                    }
                    weapons_fired += 1;
                }
            }
        }

        // Defender fires
        {
            let defender_weapons: Vec<Vec<WeaponStats>> = defenders
                .iter()
                .filter(|s| s.is_alive())
                .map(|s| s.weapons.clone())
                .collect();

            for weapons in &defender_weapons {
                for weapon in weapons {
                    if weapon.range < distance {
                        continue;
                    }
                    if weapon.cooldown > 0 && (turn as i64) % weapon.cooldown != 0 {
                        continue;
                    }
                    let Some(target_idx) = find_weakest_alive_target(attackers) else {
                        break;
                    };
                    let target_evasion = attackers[target_idx].evasion;
                    let chance = hit_chance(weapon, target_evasion);
                    if rng.random::<f64>() < chance {
                        let dmg = apply_damage_to_profile(&mut attackers[target_idx], weapon, rng, config.shield_regen_delay);
                        dmg_by_defender += dmg;
                    }
                    weapons_fired += 1;
                }
            }
        }

        // --- 3. Shield regen (suppressed while cooldown > 0) ---
        for ship in attackers.iter_mut().chain(defenders.iter_mut()) {
            if !ship.is_alive() {
                continue;
            }
            if ship.shield_regen_cooldown > 0 {
                ship.shield_regen_cooldown -= 1;
            } else if ship.shield_regen > 0.0 {
                ship.shield = (ship.shield + ship.shield_regen).min(ship.shield_max);
            }
        }

        // --- 4. Log turn state (after damage, before destruction pruning) ---
        turns.push(TurnLog {
            turn,
            distance,
            attacker_ships_alive: alive_count(attackers),
            defender_ships_alive: alive_count(defenders),
            attacker_total_hp: total_fleet_hp(attackers),
            defender_total_hp: total_fleet_hp(defenders),
            weapons_fired,
            damage_dealt_by_attacker: dmg_by_attacker,
            damage_dealt_by_defender: dmg_by_defender,
        });

        // Early exit if one side is eliminated.
        if alive_count(attackers) == 0 || alive_count(defenders) == 0 {
            break;
        }

        // Stalemate detection: neither side can deal damage (no weapons in
        // range and speed won't change distance).
        if !fleet_has_weapons(attackers) && !fleet_has_weapons(defenders) {
            break;
        }
    }

    // --- Determine outcome ---
    let att_alive = alive_count(attackers);
    let def_alive = alive_count(defenders);
    let outcome = if att_alive > 0 && def_alive == 0 {
        CombatOutcome::AttackerWon {
            surviving_fraction: att_alive as f64 / initial_attacker_count,
        }
    } else if def_alive > 0 && att_alive == 0 {
        CombatOutcome::DefenderWon {
            surviving_fraction: def_alive as f64 / initial_defender_count,
        }
    } else {
        CombatOutcome::Stalemate
    };

    CombatLog { turns, outcome }
}

/// Find the weakest alive target (lowest total HP).
/// Ties broken by index (stable).
fn find_weakest_alive_target(fleet: &[ShipProfile]) -> Option<usize> {
    fleet
        .iter()
        .enumerate()
        .filter(|(_, s)| s.is_alive())
        .min_by(|(_, a), (_, b)| a.total_hp().partial_cmp(&b.total_hp()).unwrap())
        .map(|(i, _)| i)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    /// Helper: create a weapon with given stats.
    fn make_weapon(
        range: f64,
        track: f64,
        precision: f64,
        cooldown: i64,
        shield_damage: f64,
        armor_damage: f64,
        hull_damage: f64,
    ) -> WeaponStats {
        WeaponStats {
            track,
            precision,
            cooldown,
            range,
            shield_damage,
            shield_damage_div: 0.0,
            shield_piercing: 0.0,
            armor_damage,
            armor_damage_div: 0.0,
            armor_piercing: 0.0,
            hull_damage,
            hull_damage_div: 0.0,
        }
    }

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

    fn railgun() -> WeaponStats {
        WeaponStats {
            track: 2.0,
            precision: 0.90,
            cooldown: 3,
            range: 20.0,
            shield_damage: 1.0,
            shield_damage_div: 0.5,
            shield_piercing: 0.5,
            armor_damage: 8.0,
            armor_damage_div: 2.0,
            armor_piercing: 0.3,
            hull_damage: 10.0,
            hull_damage_div: 3.0,
        }
    }

    fn missile() -> WeaponStats {
        WeaponStats {
            track: 8.0,
            precision: 0.70,
            cooldown: 2,
            range: 15.0,
            shield_damage: 1.0,
            shield_damage_div: 0.5,
            shield_piercing: 0.8,
            armor_damage: 6.0,
            armor_damage_div: 2.0,
            armor_piercing: 0.1,
            hull_damage: 8.0,
            hull_damage_div: 2.0,
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

    fn config() -> CombatConfig {
        CombatConfig {
            turns_per_tick: 12,
            distance_step_factor: 1.0,
        }
    }

    // --- Test 1: Range advantage ---
    // Railgun fleet (range 20) vs laser fleet (range 10) with equal speed.
    // At initial distance 20, only railguns can fire. Railgun should win.
    #[test]
    fn range_advantage_railgun_vs_laser() {
        let mut rng = StdRng::seed_from_u64(42);
        let mut attackers: Vec<ShipProfile> =
            (0..4).map(|_| make_ship(vec![railgun()], 1.0)).collect();
        let mut defenders: Vec<ShipProfile> =
            (0..4).map(|_| make_ship(vec![laser()], 1.0)).collect();

        let log = simulate_combat(&mut attackers, &mut defenders, &config(), &mut rng);
        // Railgun fleet should win (or at least deal more damage).
        match &log.outcome {
            CombatOutcome::AttackerWon { .. } => {} // expected
            CombatOutcome::Stalemate => {
                // Stalemate is acceptable if the turn limit prevents full resolution,
                // but attackers should have dealt more damage.
                let att_hp: f64 = attackers.iter().map(|s| s.total_hp()).sum();
                let def_hp: f64 = defenders.iter().map(|s| s.total_hp()).sum();
                assert!(
                    att_hp > def_hp,
                    "Railgun fleet should have more HP remaining in stalemate"
                );
            }
            CombatOutcome::DefenderWon { .. } => {
                panic!("Laser fleet should not win against railgun fleet at equal speed");
            }
        }
    }

    // --- Test 2: Speed closes gap ---
    // Fast laser fleet vs slow railgun fleet. Lasers should close to range 10
    // and then fight effectively.
    #[test]
    fn speed_closes_gap_laser_vs_railgun() {
        let mut rng = StdRng::seed_from_u64(123);
        // Lasers with speed 5.0
        let mut attackers: Vec<ShipProfile> =
            (0..4).map(|_| make_ship(vec![laser()], 5.0)).collect();
        // Railguns with speed 1.0
        let mut defenders: Vec<ShipProfile> =
            (0..4).map(|_| make_ship(vec![railgun()], 1.0)).collect();

        // Use more turns so lasers can close distance.
        let cfg = CombatConfig {
            turns_per_tick: 30,
            distance_step_factor: 2.0,
        };
        let log = simulate_combat(&mut attackers, &mut defenders, &cfg, &mut rng);

        // Verify distance decreased over time (lasers closing).
        if log.turns.len() >= 2 {
            let first_dist = log.turns[0].distance;
            let last_dist = log.turns.last().unwrap().distance;
            assert!(
                last_dist < first_dist,
                "Distance should decrease as fast laser fleet closes gap: first={first_dist}, last={last_dist}"
            );
        }
        // With speed advantage, lasers should be competitive (not necessarily
        // winning since railguns fire during approach, but should deal damage).
        let att_dmg: f64 = log.turns.iter().map(|t| t.damage_dealt_by_attacker).sum();
        assert!(
            att_dmg > 0.0,
            "Laser fleet should deal some damage after closing gap"
        );
    }

    // --- Test 3: Shield piercing ---
    // Missile fleet vs shield-heavy fleet. Missiles have 0.8 shield_piercing.
    #[test]
    fn shield_piercing_missiles_vs_shields() {
        let mut rng = StdRng::seed_from_u64(456);
        let mut attackers: Vec<ShipProfile> =
            (0..4).map(|_| make_ship(vec![missile()], 2.0)).collect();

        // Defenders with heavy shields but missiles bypass them.
        let mut defenders: Vec<ShipProfile> = (0..4)
            .map(|_| ShipProfile {
                weapons: vec![laser()],
                hull: 30.0,
                hull_max: 30.0,
                armor: 10.0,
                armor_max: 10.0,
                shield: 80.0,
                shield_max: 80.0,
                shield_regen: 5.0,
                evasion: 2.0,
                speed: 2.0,
                shield_regen_cooldown: 0,
            })
            .collect();

        let cfg = CombatConfig {
            turns_per_tick: 24,
            distance_step_factor: 1.0,
        };
        let log = simulate_combat(&mut attackers, &mut defenders, &cfg, &mut rng);

        // Missiles should pierce shields and deal hull/armor damage.
        let att_dmg: f64 = log.turns.iter().map(|t| t.damage_dealt_by_attacker).sum();
        assert!(
            att_dmg > 10.0,
            "Missiles should deal significant damage despite heavy shields (dealt {att_dmg})"
        );
    }

    // --- Test 4: Armor advantage ---
    // Railgun fleet (armor_piercing 0.3) vs armor-heavy fleet.
    #[test]
    fn armor_piercing_railgun_vs_armor() {
        let mut rng = StdRng::seed_from_u64(789);
        let mut attackers: Vec<ShipProfile> =
            (0..4).map(|_| make_ship(vec![railgun()], 2.0)).collect();
        // Heavy armor defenders.
        let mut defenders: Vec<ShipProfile> = (0..4)
            .map(|_| ShipProfile {
                weapons: vec![laser()],
                hull: 30.0,
                hull_max: 30.0,
                armor: 80.0,
                armor_max: 80.0,
                shield: 5.0,
                shield_max: 5.0,
                shield_regen: 0.0,
                evasion: 2.0,
                speed: 2.0,
                shield_regen_cooldown: 0,
            })
            .collect();

        let cfg = CombatConfig {
            turns_per_tick: 24,
            distance_step_factor: 1.0,
        };
        let log = simulate_combat(&mut attackers, &mut defenders, &cfg, &mut rng);

        // Railguns should deal hull damage through armor_piercing.
        let att_dmg: f64 = log.turns.iter().map(|t| t.damage_dealt_by_attacker).sum();
        assert!(
            att_dmg > 15.0,
            "Railguns should deal meaningful damage to armored targets (dealt {att_dmg})"
        );
    }

    // --- Test 5: Mixed fleet ---
    #[test]
    fn mixed_fleet_vs_specialized() {
        let mut rng = StdRng::seed_from_u64(1010);
        // Mixed fleet: 2 laser + 1 railgun + 1 missile
        let mut attackers = vec![
            make_ship(vec![laser()], 3.0),
            make_ship(vec![laser()], 3.0),
            make_ship(vec![railgun()], 2.0),
            make_ship(vec![missile()], 2.5),
        ];
        // Specialized: 4 railguns
        let mut defenders: Vec<ShipProfile> =
            (0..4).map(|_| make_ship(vec![railgun()], 1.5)).collect();

        let cfg = CombatConfig {
            turns_per_tick: 24,
            distance_step_factor: 1.0,
        };
        let log = simulate_combat(&mut attackers, &mut defenders, &cfg, &mut rng);

        // Just verify it doesn't panic and produces valid output.
        assert!(!log.turns.is_empty(), "Should have at least one turn");
        assert!(
            matches!(
                log.outcome,
                CombatOutcome::AttackerWon { .. }
                    | CombatOutcome::DefenderWon { .. }
                    | CombatOutcome::Stalemate
            ),
            "Should produce a valid outcome"
        );
    }

    // --- Test 6: Equal forces (50/50) ---
    #[test]
    fn equal_forces_roughly_even() {
        let mut attacker_wins = 0;
        let mut defender_wins = 0;
        let trials = 100;

        for seed in 0..trials {
            let mut rng = StdRng::seed_from_u64(seed);
            let mut attackers: Vec<ShipProfile> =
                (0..3).map(|_| make_ship(vec![laser()], 2.0)).collect();
            let mut defenders: Vec<ShipProfile> =
                (0..3).map(|_| make_ship(vec![laser()], 2.0)).collect();

            let cfg = CombatConfig {
                turns_per_tick: 30,
                distance_step_factor: 1.0,
            };
            let log = simulate_combat(&mut attackers, &mut defenders, &cfg, &mut rng);
            match log.outcome {
                CombatOutcome::AttackerWon { .. } => attacker_wins += 1,
                CombatOutcome::DefenderWon { .. } => defender_wins += 1,
                CombatOutcome::Stalemate => {}
            }
        }
        // With identical fleets, win rates should be roughly balanced.
        // Allow generous margin — RNG variance with only 100 trials.
        let total_decisive = attacker_wins + defender_wins;
        if total_decisive > 10 {
            let att_ratio = attacker_wins as f64 / total_decisive as f64;
            assert!(
                (0.2..=0.8).contains(&att_ratio),
                "Attacker win ratio {att_ratio:.2} should be roughly 0.5 (att={attacker_wins}, def={defender_wins})"
            );
        }
    }

    // --- Test 7: Empty fleet edge case ---
    #[test]
    fn empty_fleet_no_panic() {
        let mut rng = StdRng::seed_from_u64(999);

        // Both empty.
        let log = simulate_combat(&mut [], &mut [], &config(), &mut rng);
        assert_eq!(log.outcome, CombatOutcome::Stalemate);
        assert!(log.turns.is_empty());

        // Only attackers.
        let mut attackers = vec![make_ship(vec![laser()], 2.0)];
        let log = simulate_combat(&mut attackers, &mut [], &config(), &mut rng);
        assert_eq!(
            log.outcome,
            CombatOutcome::AttackerWon {
                surviving_fraction: 1.0,
            }
        );

        // Only defenders.
        let mut defenders = vec![make_ship(vec![laser()], 2.0)];
        let log = simulate_combat(&mut [], &mut defenders, &config(), &mut rng);
        assert_eq!(
            log.outcome,
            CombatOutcome::DefenderWon {
                surviving_fraction: 1.0,
            }
        );
    }

    // --- Test 8: CombatLog records turns ---
    #[test]
    fn combat_log_records_turns() {
        let mut rng = StdRng::seed_from_u64(555);
        let mut attackers = vec![make_ship(vec![laser()], 2.0)];
        let mut defenders = vec![make_ship(vec![laser()], 2.0)];

        let cfg = CombatConfig {
            turns_per_tick: 8,
            distance_step_factor: 1.0,
        };
        let log = simulate_combat(&mut attackers, &mut defenders, &cfg, &mut rng);

        assert!(!log.turns.is_empty(), "Should have logged turns");

        // Verify turn numbers are sequential.
        for (i, turn) in log.turns.iter().enumerate() {
            assert_eq!(turn.turn, i as u32, "Turn numbers should be sequential");
            assert!(turn.distance >= 0.0, "Distance should be non-negative");
            assert!(turn.attacker_total_hp >= 0.0, "HP should be non-negative");
            assert!(turn.defender_total_hp >= 0.0, "HP should be non-negative");
        }

        // HP should decrease over time (at least one side takes damage).
        if log.turns.len() >= 2 {
            let first = &log.turns[0];
            let last = log.turns.last().unwrap();
            let total_first = first.attacker_total_hp + first.defender_total_hp;
            let total_last = last.attacker_total_hp + last.defender_total_hp;
            assert!(
                total_last < total_first,
                "Total HP should decrease over combat: first={total_first}, last={total_last}"
            );
        }
    }

    // --- Test 9: Weapons out of range don't fire ---
    #[test]
    fn weapons_out_of_range_dont_fire() {
        let mut rng = StdRng::seed_from_u64(777);
        // Short-range weapon vs long-range weapon, both slow (equal speed).
        let short_weapon = make_weapon(5.0, 5.0, 1.0, 1, 10.0, 10.0, 10.0);
        let long_weapon = make_weapon(20.0, 5.0, 1.0, 1, 10.0, 10.0, 10.0);

        let mut attackers = vec![ShipProfile {
            weapons: vec![short_weapon],
            hull: 100.0,
            hull_max: 100.0,
            armor: 0.0,
            armor_max: 0.0,
            shield: 0.0,
            shield_max: 0.0,
            shield_regen: 0.0,
            evasion: 0.0,
            speed: 1.0,
            shield_regen_cooldown: 0,
        }];
        let mut defenders = vec![ShipProfile {
            weapons: vec![long_weapon],
            hull: 100.0,
            hull_max: 100.0,
            armor: 0.0,
            armor_max: 0.0,
            shield: 0.0,
            shield_max: 0.0,
            shield_regen: 0.0,
            evasion: 0.0,
            speed: 1.0,
            shield_regen_cooldown: 0,
        }];

        let cfg = CombatConfig {
            turns_per_tick: 6,
            distance_step_factor: 1.0,
        };
        let log = simulate_combat(&mut attackers, &mut defenders, &cfg, &mut rng);

        // Distance starts at 20 (max weapon range). Short-range (5) can't fire.
        // Equal speed means distance won't change.
        // Only defender should deal damage.
        let att_total_dmg: f64 = log.turns.iter().map(|t| t.damage_dealt_by_attacker).sum();
        let def_total_dmg: f64 = log.turns.iter().map(|t| t.damage_dealt_by_defender).sum();
        assert!(
            att_total_dmg == 0.0,
            "Short-range weapon should not fire at distance 20 (dealt {att_total_dmg})"
        );
        assert!(
            def_total_dmg > 0.0,
            "Long-range weapon should fire at distance 20"
        );
    }

    // --- Test 10: Deterministic with same seed ---
    #[test]
    fn deterministic_with_same_seed() {
        let run = |seed: u64| -> CombatLog {
            let mut rng = StdRng::seed_from_u64(seed);
            let mut attackers: Vec<ShipProfile> = (0..3)
                .map(|_| make_ship(vec![laser(), missile()], 3.0))
                .collect();
            let mut defenders: Vec<ShipProfile> =
                (0..3).map(|_| make_ship(vec![railgun()], 2.0)).collect();
            simulate_combat(&mut attackers, &mut defenders, &config(), &mut rng)
        };

        let log1 = run(42);
        let log2 = run(42);

        assert_eq!(log1.turns.len(), log2.turns.len());
        for (t1, t2) in log1.turns.iter().zip(log2.turns.iter()) {
            assert_eq!(t1.distance, t2.distance);
            assert_eq!(t1.attacker_ships_alive, t2.attacker_ships_alive);
            assert_eq!(t1.defender_ships_alive, t2.defender_ships_alive);
        }
        assert_eq!(
            std::mem::discriminant(&log1.outcome),
            std::mem::discriminant(&log2.outcome)
        );
    }
}
