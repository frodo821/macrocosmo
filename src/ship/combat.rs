use bevy::prelude::*;
use rand::Rng;

use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{HostilePresence, StarSystem};
use crate::player::{Player, StationedAt};
use crate::ship_design::ModuleRegistry;
use crate::time_system::GameClock;

use super::{Ship, ShipHitpoints, ShipModifiers, ShipState, RulesOfEngagement};

/// Hit chance: precision * track / (track + evasion)
fn hit_chance(weapon: &crate::ship_design::WeaponStats, target_evasion: f64) -> f64 {
    weapon.precision * (weapon.track / (weapon.track + target_evasion))
}

/// Apply weapon damage to a hostile (single HP pool).
fn apply_damage_to_hostile(hostile_hp: &mut f64, weapon: &crate::ship_design::WeaponStats, rng: &mut impl Rng) {
    let dmg = (weapon.hull_damage + weapon.hull_damage_div * (rng.random::<f64>() * 2.0 - 1.0)).max(0.0);
    *hostile_hp -= dmg;
}

/// Apply damage through 3-layer HP: shield → armor → hull.
fn apply_damage_to_ship(hp: &mut ShipHitpoints, weapon: &crate::ship_design::WeaponStats, rng: &mut impl Rng) {
    // Shield phase
    if hp.shield > 0.0 && rng.random::<f64>() >= weapon.shield_piercing {
        let dmg = (weapon.shield_damage + weapon.shield_damage_div * (rng.random::<f64>() * 2.0 - 1.0)).max(0.0);
        hp.shield = (hp.shield - dmg).max(0.0);
        return; // damage absorbed by shield
    }

    // Armor phase
    if hp.armor > 0.0 && rng.random::<f64>() >= weapon.armor_piercing {
        let dmg = (weapon.armor_damage + weapon.armor_damage_div * (rng.random::<f64>() * 2.0 - 1.0)).max(0.0);
        hp.armor = (hp.armor - dmg).max(0.0);
        return; // damage absorbed by armor
    }

    // Hull phase
    let dmg = (weapon.hull_damage + weapon.hull_damage_div * (rng.random::<f64>() * 2.0 - 1.0)).max(0.0);
    hp.hull = (hp.hull - dmg).max(0.0);
}

/// Apply flat hostile damage through 3-layer HP (simplified for hostile attacks).
fn apply_flat_damage_to_ship(hp: &mut ShipHitpoints, damage: f64) {
    let mut remaining = damage;

    // Shield absorbs first
    if hp.shield > 0.0 {
        let absorbed = remaining.min(hp.shield);
        hp.shield -= absorbed;
        remaining -= absorbed;
    }

    // Armor absorbs next
    if remaining > 0.0 && hp.armor > 0.0 {
        let absorbed = remaining.min(hp.armor);
        hp.armor -= absorbed;
        remaining -= absorbed;
    }

    // Hull takes the rest
    if remaining > 0.0 {
        hp.hull = (hp.hull - remaining).max(0.0);
    }
}

/// Resolves combat between player ships and hostile presences at star systems.
/// Combat turns per hexadies: 12. Uses WeaponStats from equipped modules.
pub fn resolve_combat(
    mut commands: Commands,
    clock: Res<GameClock>,
    last_tick: Res<crate::colony::LastProductionTick>,
    mut ships: Query<(Entity, &Ship, &mut ShipHitpoints, &ShipModifiers, &ShipState, Option<&RulesOfEngagement>)>,
    mut hostiles: Query<(Entity, &mut HostilePresence)>,
    module_registry: Res<ModuleRegistry>,
    systems: Query<(Entity, &StarSystem)>,
    mut events: MessageWriter<GameEvent>,
    mut player_q: Query<(Entity, &mut StationedAt, Option<&crate::player::AboardShip>), With<Player>>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let combat_turns = (delta * 12) as u32;
    let mut rng = rand::rng();

    // Collect hostile systems first to avoid borrow issues
    let hostile_data: Vec<(Entity, Entity, f64, f64, f64, crate::galaxy::HostileType, f64)> = hostiles
        .iter()
        .map(|(e, h)| (e, h.system, h.strength, h.hp, h.max_hp, h.hostile_type, h.evasion))
        .collect();

    for (hostile_entity, system_entity, _hostile_strength, _hostile_hp, _hostile_max_hp, _hostile_type, hostile_evasion) in &hostile_data {
        let system_name = systems
            .get(*system_entity)
            .map(|(_, s)| s.name.clone())
            .unwrap_or_default();

        // Find all player ships docked at this system, excluding Retreat ROE
        let docked_ships: Vec<Entity> = ships
            .iter()
            .filter_map(|(entity, _ship, _hp, _mods, state, roe)| {
                let roe = roe.copied().unwrap_or_default();
                if roe == RulesOfEngagement::Retreat {
                    return None; // #57: Retreat ships skip combat
                }
                if let ShipState::Docked { system } = state {
                    if *system == *system_entity {
                        return Some(entity);
                    }
                }
                None
            })
            .collect();

        if docked_ships.is_empty() {
            continue;
        }

        // --- Player ships attack hostile ---
        // Collect weapon data for each ship
        struct ShipWeaponData {
            entity: Entity,
            weapons: Vec<crate::ship_design::WeaponStats>,
        }
        let mut ship_weapons: Vec<ShipWeaponData> = Vec::new();
        for &ship_entity in &docked_ships {
            if let Ok((_e, ship, _hp, _mods, _state, _roe)) = ships.get(ship_entity) {
                let mut weapons = Vec::new();
                for equipped in &ship.modules {
                    if let Some(module_def) = module_registry.modules.get(&equipped.module_id) {
                        if let Some(weapon) = &module_def.weapon {
                            weapons.push(weapon.clone());
                        }
                    }
                }
                ship_weapons.push(ShipWeaponData { entity: ship_entity, weapons });
            }
        }

        // Apply weapon damage to hostile
        let Ok((_he, mut hostile)) = hostiles.get_mut(*hostile_entity) else {
            continue;
        };

        for sw in &ship_weapons {
            for weapon in &sw.weapons {
                let shots = if weapon.cooldown > 0 { combat_turns / weapon.cooldown as u32 } else { combat_turns };
                for _ in 0..shots {
                    let chance = hit_chance(weapon, *hostile_evasion);
                    if rng.random::<f64>() < chance {
                        apply_damage_to_hostile(&mut hostile.hp, weapon, &mut rng);
                    }
                }
            }
        }

        // Check if hostile is destroyed
        if hostile.hp <= 0.0 {
            commands.entity(*hostile_entity).despawn();
            events.write(GameEvent {
                timestamp: clock.elapsed,
                kind: GameEventKind::CombatVictory,
                description: format!(
                    "Victory! Hostile {:?} at {} has been defeated",
                    hostile.hostile_type, system_name
                ),
                related_system: Some(*system_entity),
            });
            continue;
        }

        let hostile_str = hostile.strength;
        let hostile_tp = hostile.hostile_type;
        // Drop the mutable borrow on hostile before accessing ships mutably
        drop(hostile);

        // --- Hostile attacks player ships ---
        // Hostile deals strength damage per combat turn, distributed evenly
        if hostile_str > 0.0 && !docked_ships.is_empty() {
            let total_damage = hostile_str * combat_turns as f64;
            let damage_per_ship = total_damage / docked_ships.len() as f64;
            let mut destroyed_ships: Vec<(Entity, String)> = Vec::new();

            for &ship_entity in &docked_ships {
                if let Ok((_e, ship, mut hp, _mods, _state, _roe)) = ships.get_mut(ship_entity) {
                    apply_flat_damage_to_ship(&mut hp, damage_per_ship);
                    if hp.hull <= 0.0 {
                        destroyed_ships.push((ship_entity, ship.name.clone()));
                    }
                }
            }

            for (entity, name) in &destroyed_ships {
                // #59: Check if player is aboard this ship — respawn at capital
                if let Ok((player_entity, mut stationed, aboard)) = player_q.single_mut() {
                    if let Some(aboard_ship) = aboard {
                        if aboard_ship.ship == *entity {
                            // Find capital system entity
                            let capital_entity = systems.iter()
                                .find(|(_, s)| s.is_capital)
                                .map(|(e, _)| e);
                            if let Some(cap_entity) = capital_entity {
                                stationed.system = cap_entity;
                            }
                            commands.entity(player_entity).remove::<crate::player::AboardShip>();
                            events.write(GameEvent {
                                timestamp: clock.elapsed,
                                kind: GameEventKind::PlayerRespawn,
                                description: "Flagship destroyed! Respawned at capital.".to_string(),
                                related_system: capital_entity,
                            });
                        }
                    }
                }
                commands.entity(*entity).despawn();
                events.write(GameEvent {
                    timestamp: clock.elapsed,
                    kind: GameEventKind::CombatDefeat,
                    description: format!("{} destroyed in combat at {}", name, system_name),
                    related_system: Some(*system_entity),
                });
            }

            // Check if all player ships at this system are destroyed
            let surviving = docked_ships.len() - destroyed_ships.len();
            if surviving == 0 {
                events.write(GameEvent {
                    timestamp: clock.elapsed,
                    kind: GameEventKind::CombatDefeat,
                    description: format!(
                        "All ships destroyed by hostile {:?} at {}",
                        hostile_tp,
                        system_name
                    ),
                    related_system: Some(*system_entity),
                });
            }
        }
    }
}
