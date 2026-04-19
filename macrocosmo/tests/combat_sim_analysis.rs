//! Random combat scenario runner — collects statistics from many simulations
//! to verify balance across weapon/defense/speed matchups.
//!
//! Run with:
//!   cargo test -p macrocosmo --test combat_sim_analysis -- --nocapture

use macrocosmo::ship::combat_sim::*;
use macrocosmo::ship_design::WeaponStats;
use rand::SeedableRng;
use rand::rngs::StdRng;

// ---------------------------------------------------------------------------
// Weapon presets (matching Lua definitions)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Ship builders with defense variants
// ---------------------------------------------------------------------------

fn corvette(weapon: WeaponStats, speed: f64) -> ShipProfile {
    ShipProfile {
        weapons: vec![weapon],
        hull: 50.0,
        hull_max: 50.0,
        armor: 20.0,
        armor_max: 20.0,
        shield: 10.0,
        shield_max: 10.0,
        shield_regen: 1.0,
        evasion: 5.0,
        speed,
        shield_regen_cooldown: 0,
        index: 0,
        name: String::new(),
        is_core: false,
        is_conquered_core: false,
        retreat_threshold: 0.0,
    }
}

fn shield_tank(weapon: WeaponStats, speed: f64) -> ShipProfile {
    ShipProfile {
        weapons: vec![weapon],
        hull: 40.0,
        hull_max: 40.0,
        armor: 10.0,
        armor_max: 10.0,
        shield: 60.0,
        shield_max: 60.0,
        shield_regen: 4.0,
        evasion: 2.0,
        speed,
        shield_regen_cooldown: 0,
        index: 0,
        name: String::new(),
        is_core: false,
        is_conquered_core: false,
        retreat_threshold: 0.0,
    }
}

fn armor_tank(weapon: WeaponStats, speed: f64) -> ShipProfile {
    ShipProfile {
        weapons: vec![weapon],
        hull: 40.0,
        hull_max: 40.0,
        armor: 60.0,
        armor_max: 60.0,
        shield: 5.0,
        shield_max: 5.0,
        shield_regen: 0.0,
        evasion: 1.0,
        speed,
        shield_regen_cooldown: 0,
        index: 0,
        name: String::new(),
        is_core: false,
        is_conquered_core: false,
        retreat_threshold: 0.0,
    }
}

fn interceptor(weapon: WeaponStats, speed: f64) -> ShipProfile {
    ShipProfile {
        weapons: vec![weapon],
        hull: 30.0,
        hull_max: 30.0,
        armor: 5.0,
        armor_max: 5.0,
        shield: 5.0,
        shield_max: 5.0,
        shield_regen: 0.5,
        evasion: 10.0,
        speed,
        shield_regen_cooldown: 0,
        index: 0,
        name: String::new(),
        is_core: false,
        is_conquered_core: false,
        retreat_threshold: 0.0,
    }
}

// ---------------------------------------------------------------------------
// Scenario runner
// ---------------------------------------------------------------------------

struct ScenarioResult {
    name: String,
    attacker_wins: u32,
    defender_wins: u32,
    stalemates: u32,
    avg_turns: f64,
    avg_attacker_surviving: f64,
    avg_defender_surviving: f64,
    trials: u32,
}

fn run_scenario(
    name: &str,
    make_attackers: impl Fn() -> Vec<ShipProfile>,
    make_defenders: impl Fn() -> Vec<ShipProfile>,
    config: &CombatConfig,
    trials: u32,
) -> ScenarioResult {
    let mut attacker_wins = 0u32;
    let mut defender_wins = 0u32;
    let mut stalemates = 0u32;
    let mut total_turns = 0u64;
    let mut total_att_surviving = 0.0f64;
    let mut total_def_surviving = 0.0f64;

    for seed in 0..trials {
        let mut rng = StdRng::seed_from_u64(seed as u64);
        let mut att = make_attackers();
        let mut def = make_defenders();
        let log = simulate_combat(&mut att, &mut def, config, &mut rng);

        total_turns += log.turns.len() as u64;
        match &log.outcome {
            CombatOutcome::AttackerWon { surviving_fraction } => {
                attacker_wins += 1;
                total_att_surviving += surviving_fraction;
            }
            CombatOutcome::DefenderWon { surviving_fraction } => {
                defender_wins += 1;
                total_def_surviving += surviving_fraction;
            }
            CombatOutcome::AttackerRetreated { .. }
            | CombatOutcome::DefenderRetreated { .. }
            | CombatOutcome::MutualRetreat
            | CombatOutcome::Stalemate => {
                stalemates += 1;
            }
        }
    }

    ScenarioResult {
        name: name.to_string(),
        attacker_wins,
        defender_wins,
        stalemates,
        avg_turns: total_turns as f64 / trials as f64,
        avg_attacker_surviving: if attacker_wins > 0 {
            total_att_surviving / attacker_wins as f64
        } else {
            0.0
        },
        avg_defender_surviving: if defender_wins > 0 {
            total_def_surviving / defender_wins as f64
        } else {
            0.0
        },
        trials,
    }
}

fn print_results(results: &[ScenarioResult]) {
    println!();
    println!(
        "{:<50} {:>6} {:>6} {:>6} {:>6} {:>8} {:>8}",
        "Scenario", "A win%", "D win%", "Draw%", "Turns", "A surv%", "D surv%"
    );
    println!("{}", "-".repeat(100));
    for r in results {
        let aw = r.attacker_wins as f64 / r.trials as f64 * 100.0;
        let dw = r.defender_wins as f64 / r.trials as f64 * 100.0;
        let st = r.stalemates as f64 / r.trials as f64 * 100.0;
        println!(
            "{:<50} {:>5.1}% {:>5.1}% {:>5.1}% {:>6.1} {:>7.1}% {:>7.1}%",
            r.name,
            aw,
            dw,
            st,
            r.avg_turns,
            r.avg_attacker_surviving * 100.0,
            r.avg_defender_surviving * 100.0,
        );
    }
    println!();
}

// ---------------------------------------------------------------------------
// Main test
// ---------------------------------------------------------------------------

#[test]
fn combat_scenario_matrix() {
    let config = CombatConfig {
        turns_per_tick: 1200,
        distance_step_factor: 1.0,
        ..Default::default()
    };
    let trials = 200;
    let mut results = Vec::new();

    // === Mirror matchups (should be ~50/50) ===
    results.push(run_scenario(
        "Mirror: 4 laser corvettes",
        || (0..4).map(|_| corvette(laser(), 2.0)).collect(),
        || (0..4).map(|_| corvette(laser(), 2.0)).collect(),
        &config,
        trials,
    ));
    results.push(run_scenario(
        "Mirror: 4 railgun corvettes",
        || (0..4).map(|_| corvette(railgun(), 2.0)).collect(),
        || (0..4).map(|_| corvette(railgun(), 2.0)).collect(),
        &config,
        trials,
    ));
    results.push(run_scenario(
        "Mirror: 4 missile corvettes",
        || (0..4).map(|_| corvette(missile(), 2.0)).collect(),
        || (0..4).map(|_| corvette(missile(), 2.0)).collect(),
        &config,
        trials,
    ));

    // === Weapon type matchups (equal count, equal speed) ===
    results.push(run_scenario(
        "Laser vs Railgun (equal speed 2.0)",
        || (0..4).map(|_| corvette(laser(), 2.0)).collect(),
        || (0..4).map(|_| corvette(railgun(), 2.0)).collect(),
        &config,
        trials,
    ));
    results.push(run_scenario(
        "Laser vs Missile (equal speed 2.0)",
        || (0..4).map(|_| corvette(laser(), 2.0)).collect(),
        || (0..4).map(|_| corvette(missile(), 2.0)).collect(),
        &config,
        trials,
    ));
    results.push(run_scenario(
        "Railgun vs Missile (equal speed 2.0)",
        || (0..4).map(|_| corvette(railgun(), 2.0)).collect(),
        || (0..4).map(|_| corvette(missile(), 2.0)).collect(),
        &config,
        trials,
    ));

    // === Speed advantage scenarios ===
    results.push(run_scenario(
        "Fast laser (spd 5) vs Slow railgun (spd 1)",
        || (0..4).map(|_| corvette(laser(), 5.0)).collect(),
        || (0..4).map(|_| corvette(railgun(), 1.0)).collect(),
        &config,
        trials,
    ));
    results.push(run_scenario(
        "Slow laser (spd 1) vs Fast railgun (spd 5)",
        || (0..4).map(|_| corvette(laser(), 1.0)).collect(),
        || (0..4).map(|_| corvette(railgun(), 5.0)).collect(),
        &config,
        trials,
    ));

    // === Defense type matchups ===
    results.push(run_scenario(
        "Missile vs Shield-tank (laser)",
        || (0..4).map(|_| corvette(missile(), 2.0)).collect(),
        || (0..4).map(|_| shield_tank(laser(), 2.0)).collect(),
        &config,
        trials,
    ));
    results.push(run_scenario(
        "Laser vs Shield-tank (laser)",
        || (0..4).map(|_| corvette(laser(), 2.0)).collect(),
        || (0..4).map(|_| shield_tank(laser(), 2.0)).collect(),
        &config,
        trials,
    ));
    results.push(run_scenario(
        "Railgun vs Armor-tank (laser)",
        || (0..4).map(|_| corvette(railgun(), 2.0)).collect(),
        || (0..4).map(|_| armor_tank(laser(), 2.0)).collect(),
        &config,
        trials,
    ));
    results.push(run_scenario(
        "Laser vs Armor-tank (laser)",
        || (0..4).map(|_| corvette(laser(), 2.0)).collect(),
        || (0..4).map(|_| armor_tank(laser(), 2.0)).collect(),
        &config,
        trials,
    ));

    // === Evasion scenarios ===
    results.push(run_scenario(
        "Missile interceptor (spd 5) vs Laser corvette",
        || (0..4).map(|_| interceptor(missile(), 5.0)).collect(),
        || (0..4).map(|_| corvette(laser(), 2.0)).collect(),
        &config,
        trials,
    ));
    results.push(run_scenario(
        "Railgun corvette vs Interceptor (laser, spd 5)",
        || (0..4).map(|_| corvette(railgun(), 2.0)).collect(),
        || (0..4).map(|_| interceptor(laser(), 5.0)).collect(),
        &config,
        trials,
    ));

    // === Numerical advantage ===
    results.push(run_scenario(
        "3 laser corvettes vs 5 laser corvettes",
        || (0..3).map(|_| corvette(laser(), 2.0)).collect(),
        || (0..5).map(|_| corvette(laser(), 2.0)).collect(),
        &config,
        trials,
    ));
    results.push(run_scenario(
        "6 laser corvettes vs 4 railgun corvettes",
        || (0..6).map(|_| corvette(laser(), 2.0)).collect(),
        || (0..4).map(|_| corvette(railgun(), 2.0)).collect(),
        &config,
        trials,
    ));

    // === Mixed fleets ===
    results.push(run_scenario(
        "Mixed (2L+1R+1M) vs Pure railgun x4",
        || {
            vec![
                corvette(laser(), 3.0),
                corvette(laser(), 3.0),
                corvette(railgun(), 2.0),
                corvette(missile(), 2.5),
            ]
        },
        || (0..4).map(|_| corvette(railgun(), 1.5)).collect(),
        &config,
        trials,
    ));
    results.push(run_scenario(
        "Mixed (2L+1R+1M) vs Mixed (2M+2R)",
        || {
            vec![
                corvette(laser(), 3.0),
                corvette(laser(), 3.0),
                corvette(railgun(), 2.0),
                corvette(missile(), 2.5),
            ]
        },
        || {
            vec![
                corvette(missile(), 2.0),
                corvette(missile(), 2.0),
                corvette(railgun(), 2.0),
                corvette(railgun(), 2.0),
            ]
        },
        &config,
        trials,
    ));

    // === Counter-build scenarios ===
    results.push(run_scenario(
        "Counter: Missile vs Shield-heavy",
        || (0..4).map(|_| corvette(missile(), 2.0)).collect(),
        || (0..4).map(|_| shield_tank(laser(), 2.0)).collect(),
        &config,
        trials,
    ));
    results.push(run_scenario(
        "Counter: Railgun vs Armor-heavy",
        || (0..4).map(|_| corvette(railgun(), 2.0)).collect(),
        || (0..4).map(|_| armor_tank(laser(), 2.0)).collect(),
        &config,
        trials,
    ));
    results.push(run_scenario(
        "Counter: Fast laser (spd 6) vs Slow railgun (spd 1)",
        || (0..4).map(|_| corvette(laser(), 6.0)).collect(),
        || (0..4).map(|_| corvette(railgun(), 1.0)).collect(),
        &config,
        trials,
    ));

    // === Long engagement (60 turns) ===
    let long_config = CombatConfig {
        turns_per_tick: 240,
        distance_step_factor: 1.0,
        ..Default::default()
    };
    results.push(run_scenario(
        "Long: Shield-tank vs Shield-tank (laser)",
        || (0..4).map(|_| shield_tank(laser(), 2.0)).collect(),
        || (0..4).map(|_| shield_tank(laser(), 2.0)).collect(),
        &long_config,
        trials,
    ));
    results.push(run_scenario(
        "Long: Armor-tank (railgun) vs Shield-tank (missile)",
        || (0..4).map(|_| armor_tank(railgun(), 2.0)).collect(),
        || (0..4).map(|_| shield_tank(missile(), 2.0)).collect(),
        &long_config,
        trials,
    ));

    print_results(&results);

    // === Sanity observations (no hard asserts yet — observing balance) ===
    for r in results.iter().filter(|r| r.name.starts_with("Mirror")) {
        let stale = r.stalemates as f64 / r.trials as f64;
        if stale > 0.5 {
            println!(
                "WARNING: {} has {:.0}% stalemates — damage may be too low relative to HP",
                r.name,
                stale * 100.0
            );
        }
    }
}
