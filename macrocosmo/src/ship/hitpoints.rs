use bevy::prelude::*;

use crate::time_system::GameClock;

use super::conquered::ConqueredCore;
use super::{ShipHitpoints, ShipModifiers, ShipState};

/// Sync ShipHitpoints max values from ShipModifiers.
/// Only updates when the modifier-computed values differ from current values.
pub fn sync_ship_hitpoints(mut ships: Query<(&ShipModifiers, &mut ShipHitpoints)>) {
    for (mods, mut hp) in &mut ships {
        let new_armor_max = mods.armor_max.final_value().to_f64();
        let new_shield_max = mods.shield_max.final_value().to_f64();
        let new_shield_regen = mods.shield_regen.final_value().to_f64();
        // Only update if values actually changed from modifiers
        if (hp.armor_max - new_armor_max).abs() > f64::EPSILON
            || (hp.shield_max - new_shield_max).abs() > f64::EPSILON
            || (hp.shield_regen - new_shield_regen).abs() > f64::EPSILON
        {
            hp.armor_max = new_armor_max;
            hp.shield_max = new_shield_max;
            hp.shield_regen = new_shield_regen;
            // Clamp current values to new max
            hp.armor = hp.armor.min(hp.armor_max);
            hp.shield = hp.shield.min(hp.shield_max);
        }
    }
}

/// Regenerate shields over time.
pub fn tick_shield_regen(
    clock: Res<GameClock>,
    last_tick: Res<crate::colony::LastProductionTick>,
    mut ships: Query<&mut ShipHitpoints>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as f64;
    for mut hp in &mut ships {
        if hp.shield < hp.shield_max && hp.shield_regen > 0.0 {
            hp.shield = (hp.shield + hp.shield_regen * d).min(hp.shield_max);
        }
    }
}

/// Default armor/hull repair rate at a Port, per hexady.
/// #160: canonical value lives in `GameBalance.repair_rate_per_hexadies`.
pub const REPAIR_RATE_PER_HEXADIES: f64 = 5.0;

pub fn tick_ship_repair(
    clock: Res<GameClock>,
    last_tick: Res<crate::colony::LastProductionTick>,
    // #298 (S-4): Exclude conquered Cores from normal Port repair — their
    // recovery is handled by `tick_conquered_recovery` in `conquered.rs`.
    mut ships: Query<(&ShipState, &mut ShipHitpoints), Without<ConqueredCore>>,
    system_buildings: Query<&crate::colony::SystemBuildings>,
    building_registry: Res<crate::colony::BuildingRegistry>,
    balance: Res<crate::technology::GameBalance>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let repair_amount = balance.repair_rate_per_hexadies() * delta as f64;

    for (state, mut hp) in &mut ships {
        let ShipState::InSystem { system } = state else {
            continue;
        };

        // Check if the system has a Port capability in system buildings
        let has_port = system_buildings
            .get(*system)
            .is_ok_and(|sb| sb.has_port(&building_registry));

        if has_port {
            // Repair armor first, then hull
            if hp.armor < hp.armor_max {
                hp.armor = (hp.armor + repair_amount).min(hp.armor_max);
            }
            if hp.hull < hp.hull_max {
                hp.hull = (hp.hull + repair_amount).min(hp.hull_max);
            }
        }
    }
}
