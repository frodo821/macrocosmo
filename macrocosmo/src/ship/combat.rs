use bevy::prelude::*;
use rand::Rng;

use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::faction::{FactionOwner, FactionRelations};
use crate::galaxy::{AtSystem, Hostile, HostileHitpoints, HostileStats, StarSystem};
use crate::knowledge::{CombatVictor, FactSysParam, KnowledgeFact, PlayerVantage};
use crate::player::{AboardShip, Player, StationedAt};
use crate::ship_design::ModuleRegistry;
use crate::time_system::GameClock;

use super::conquered::ConqueredCore;
use super::{CoreShip, Owner, RulesOfEngagement, Ship, ShipHitpoints, ShipModifiers, ShipState};

/// Hit chance: precision * track / (track + evasion)
fn hit_chance(weapon: &crate::ship_design::WeaponStats, target_evasion: f64) -> f64 {
    weapon.precision * (weapon.track / (weapon.track + target_evasion))
}

/// Apply weapon damage to a hostile (single HP pool).
fn apply_damage_to_hostile(
    hostile_hp: &mut f64,
    weapon: &crate::ship_design::WeaponStats,
    rng: &mut impl Rng,
) {
    let dmg =
        (weapon.hull_damage + weapon.hull_damage_div * (rng.random::<f64>() * 2.0 - 1.0)).max(0.0);
    *hostile_hp -= dmg;
}

/// Apply damage through 3-layer HP: shield → armor → hull.
fn apply_damage_to_ship(
    hp: &mut ShipHitpoints,
    weapon: &crate::ship_design::WeaponStats,
    rng: &mut impl Rng,
) {
    // Shield phase
    if hp.shield > 0.0 && rng.random::<f64>() >= weapon.shield_piercing {
        let dmg = (weapon.shield_damage
            + weapon.shield_damage_div * (rng.random::<f64>() * 2.0 - 1.0))
            .max(0.0);
        hp.shield = (hp.shield - dmg).max(0.0);
        return; // damage absorbed by shield
    }

    // Armor phase
    if hp.armor > 0.0 && rng.random::<f64>() >= weapon.armor_piercing {
        let dmg = (weapon.armor_damage
            + weapon.armor_damage_div * (rng.random::<f64>() * 2.0 - 1.0))
            .max(0.0);
        hp.armor = (hp.armor - dmg).max(0.0);
        return; // damage absorbed by armor
    }

    // Hull phase
    let dmg =
        (weapon.hull_damage + weapon.hull_damage_div * (rng.random::<f64>() * 2.0 - 1.0)).max(0.0);
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
///
/// **#168 — Faction-gated combat.** A ship engages a hostile only when the
/// `FactionRelations` view from the ship's faction (its `Owner::Empire(_)`
/// entity) toward the hostile's `FactionOwner` allows it. Hostile presences
/// without a `FactionOwner`, and ships whose owner is `Owner::Neutral`, are
/// skipped.
///
/// **#169 — ROE-aware engagement.** ROE now produces meaningfully distinct
/// behaviour rather than only gating `Retreat`:
/// - `Retreat`: never engages, regardless of relations or hostile presence.
/// - `Aggressive`: engages whenever
///   [`FactionView::can_attack_aggressive`] is true (`War`, or `Neutral` with
///   negative standing).
/// - `Defensive`: engages only when [`FactionView::should_engage_defensive`]
///   is true — open `War`, or when a hostile is present in the same system
///   (treated as "being attacked", since hostile presences are assumed to
///   initiate combat). Defensive therefore retaliates against `Peace` /
///   `Alliance` factions whose stale relation might still report
///   non-hostility.
///
/// The "being attacked" signal is currently inferred from the presence of any
/// hostile entity (`With<Hostile>`) co-located with the ship. A more granular
/// damage-event-driven counter-attack model is intentionally out of scope
/// here.
#[allow(clippy::too_many_arguments)]
pub fn resolve_combat(
    mut commands: Commands,
    clock: Res<GameClock>,
    last_tick: Res<crate::colony::LastProductionTick>,
    mut ships: Query<
        (
            Entity,
            &Ship,
            &mut ShipHitpoints,
            &ShipModifiers,
            &ShipState,
            Option<&RulesOfEngagement>,
            Option<&CoreShip>,
            Option<&ConqueredCore>,
        ),
        Without<Hostile>,
    >,
    mut hostiles: Query<
        (
            Entity,
            &AtSystem,
            &mut HostileHitpoints,
            &HostileStats,
            Option<&FactionOwner>,
        ),
        (With<Hostile>, Without<Ship>),
    >,
    factions: Query<&crate::player::Faction>,
    relations: Res<FactionRelations>,
    module_registry: Res<ModuleRegistry>,
    systems: Query<(Entity, &StarSystem, &Position)>,
    mut events: MessageWriter<GameEvent>,
    mut player_q: Query<(Entity, &mut StationedAt, Option<&AboardShip>), With<Player>>,
    mut fact_sys: FactSysParam,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let combat_turns = (delta * 12) as u32;
    let mut rng = rand::rng();

    // Collect hostile systems first to avoid borrow issues.
    // #293: Replaces hostile_type with faction entity for logging.
    let hostile_data: Vec<(Entity, Entity, f64, f64, Option<Entity>)> = hostiles
        .iter()
        .map(|(e, at_system, _hp, stats, owner)| {
            (
                e,
                at_system.0,
                stats.strength,
                stats.evasion,
                owner.map(|o| o.0),
            )
        })
        .collect();

    // #249: Snapshot player vantage once — used by CombatVictory / CombatDefeat fact dual-writes.
    let player_stationed_system = player_q.iter().next().map(|(_, s, _)| s.system);
    let player_pos: Option<[f64; 3]> = player_stationed_system
        .and_then(|s| systems.get(s).ok())
        .map(|(_, _, p)| p.as_array());
    let player_aboard = player_q
        .iter()
        .next()
        .and_then(|(_, _, a)| a.map(|_| ()))
        .is_some();
    let vantage = player_pos.map(|pos| PlayerVantage {
        player_pos: pos,
        player_aboard,
    });

    for (hostile_entity, system_entity, hostile_strength, hostile_evasion, hostile_faction) in
        &hostile_data
    {
        let (system_name, system_pos_arr): (String, Option<[f64; 3]>) = systems
            .get(*system_entity)
            .map(|(_, s, p)| (s.name.clone(), Some(p.as_array())))
            .unwrap_or_default();

        // #168: Skip hostiles without a FactionOwner — they have no diplomatic
        // identity in the new system, so combat cannot be evaluated. Legacy
        // (un-migrated) spawns therefore become passive instead of attacking.
        let Some(hostile_faction) = *hostile_faction else {
            continue;
        };

        // Find all player ships docked at this system, excluding Retreat ROE
        // and ships whose faction view + ROE combination forbids engagement.
        //
        // #169: Within this loop iteration we are processing a specific
        // hostile co-located with the ship, so for Defensive ROE the
        // `being_attacked` signal is `true` by construction (a hostile
        // presence is assumed to act on the ship). The ROE branches below
        // therefore differ only in the *trigger* used to engage:
        //   - Aggressive consults `can_attack_aggressive` (state-based).
        //   - Defensive consults `should_engage_defensive(true)` (war OR
        //     present-hostile retaliation).
        let docked_ships: Vec<Entity> = ships
            .iter()
            .filter_map(
                |(entity, ship, _hp, _mods, state, roe, _core, _conquered)| {
                    let roe = roe.copied().unwrap_or_default();
                    if roe == RulesOfEngagement::Retreat {
                        return None; // #57: Retreat ships skip combat
                    }
                    if let ShipState::InSystem { system } = state {
                        if *system != *system_entity {
                            return None;
                        }
                    } else {
                        return None;
                    }

                    // #168: Resolve ship's faction from Owner::Empire(_). Neutral
                    // ships have no diplomatic identity and cannot engage.
                    let Owner::Empire(faction_entity) = ship.owner else {
                        return None;
                    };

                    // Consult FactionRelations + ROE.
                    let view = relations.get_or_default(faction_entity, hostile_faction);
                    let engaged = match roe {
                        RulesOfEngagement::Aggressive => view.can_attack_aggressive(),
                        // A hostile co-located with the ship is treated as
                        // actively attacking — see #169 spec.
                        RulesOfEngagement::Defensive => view.should_engage_defensive(true),
                        // Already short-circuited above; `unreachable!` would
                        // also work but we prefer the safe fallthrough.
                        RulesOfEngagement::Retreat => false,
                    };
                    if !engaged {
                        return None;
                    }

                    Some(entity)
                },
            )
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
            if let Ok((_e, ship, _hp, _mods, _state, _roe, _core, _conquered)) =
                ships.get(ship_entity)
            {
                let mut weapons = Vec::new();
                for equipped in &ship.modules {
                    if let Some(module_def) = module_registry.modules.get(&equipped.module_id) {
                        if let Some(weapon) = &module_def.weapon {
                            weapons.push(weapon.clone());
                        }
                    }
                }
                ship_weapons.push(ShipWeaponData {
                    entity: ship_entity,
                    weapons,
                });
            }
        }

        // Apply weapon damage to hostile
        let Ok((_he, _at_system, mut hostile_hp, _stats, _owner)) =
            hostiles.get_mut(*hostile_entity)
        else {
            continue;
        };

        for sw in &ship_weapons {
            for weapon in &sw.weapons {
                let shots = if weapon.cooldown > 0 {
                    combat_turns / weapon.cooldown as u32
                } else {
                    combat_turns
                };
                for _ in 0..shots {
                    let chance = hit_chance(weapon, *hostile_evasion);
                    if rng.random::<f64>() < chance {
                        apply_damage_to_hostile(&mut hostile_hp.hp, weapon, &mut rng);
                    }
                }
            }
        }

        // Check if hostile is destroyed
        if hostile_hp.hp <= 0.0 {
            commands.entity(*hostile_entity).despawn();
            // #249: Dual-write CombatVictory.
            let event_id = fact_sys.allocate_event_id();
            // #293: hostile_type is gone — use the faction name for the
            // victory message. Falls back to "Hostile" if the faction
            // entity cannot be resolved (FactionOwner missing).
            let hostile_label = factions
                .get(hostile_faction)
                .map(|f| f.name.clone())
                .unwrap_or_else(|_| "Hostile".to_string());
            let desc = format!(
                "Victory! {} at {} has been defeated",
                hostile_label, system_name
            );
            events.write(GameEvent {
                id: event_id,
                timestamp: clock.elapsed,
                kind: GameEventKind::CombatVictory,
                description: desc.clone(),
                related_system: Some(*system_entity),
            });
            if let (Some(v), Some(op)) = (vantage, system_pos_arr) {
                let fact = KnowledgeFact::CombatOutcome {
                    event_id: Some(event_id),
                    system: *system_entity,
                    victor: CombatVictor::Player,
                    detail: desc,
                };
                fact_sys.record(fact, op, clock.elapsed, &v);
            }
            continue;
        }

        // #293: hostile strength is already captured above in `hostile_data`.
        let hostile_str = *hostile_strength;
        // Drop the mutable borrow on hostile hp before accessing ships mutably
        drop(hostile_hp);

        // --- Hostile attacks player ships ---
        // Hostile deals strength damage per combat turn, distributed evenly
        if hostile_str > 0.0 && !docked_ships.is_empty() {
            let total_damage = hostile_str * combat_turns as f64;
            let damage_per_ship = total_damage / docked_ships.len() as f64;
            let mut destroyed_ships: Vec<(Entity, String)> = Vec::new();

            for &ship_entity in &docked_ships {
                if let Ok((_e, ship, mut hp, _mods, _state, _roe, core, conquered)) =
                    ships.get_mut(ship_entity)
                {
                    // #298 (S-4): Conquered Cores are indestructible — skip damage entirely.
                    if core.is_some() && conquered.is_some() {
                        continue;
                    }
                    apply_flat_damage_to_ship(&mut hp, damage_per_ship);
                    // #298 (S-4): Core ships clamp at hull=1.0 instead of being destroyed.
                    if core.is_some() && hp.hull < 1.0 {
                        hp.hull = 1.0;
                        // Emit casus belli event for peacetime Core attack
                        if let Owner::Empire(owner_faction) = ship.owner {
                            let view = relations.get_or_default(owner_faction, hostile_faction);
                            if !view.is_at_war() {
                                let event_id = fact_sys.allocate_event_id();
                                events.write(GameEvent {
                                    id: event_id,
                                    timestamp: clock.elapsed,
                                    kind: GameEventKind::CasusBelli,
                                    description: format!(
                                        "Peacetime attack on Infrastructure Core '{}' at {}!",
                                        ship.name, system_name,
                                    ),
                                    related_system: Some(*system_entity),
                                });
                            }
                        }
                    } else if hp.hull <= 0.0 {
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
                            let capital_entity = systems
                                .iter()
                                .find(|(_, s, _)| s.is_capital)
                                .map(|(e, _, _)| e);
                            if let Some(cap_entity) = capital_entity {
                                stationed.system = cap_entity;
                            }
                            commands
                                .entity(player_entity)
                                .remove::<crate::player::AboardShip>();
                            events.write(GameEvent {
                                id: fact_sys.allocate_event_id(),
                                timestamp: clock.elapsed,
                                kind: GameEventKind::PlayerRespawn,
                                description: "Flagship destroyed! Respawned at capital."
                                    .to_string(),
                                related_system: capital_entity,
                            });
                        }
                    }
                }
                commands.entity(*entity).despawn();
                // #249: Per-ship CombatDefeat, dual-written.
                let event_id = fact_sys.allocate_event_id();
                let desc = format!("{} destroyed in combat at {}", name, system_name);
                events.write(GameEvent {
                    id: event_id,
                    timestamp: clock.elapsed,
                    kind: GameEventKind::CombatDefeat,
                    description: desc.clone(),
                    related_system: Some(*system_entity),
                });
                if let (Some(v), Some(op)) = (vantage, system_pos_arr) {
                    let fact = KnowledgeFact::CombatOutcome {
                        event_id: Some(event_id),
                        system: *system_entity,
                        victor: CombatVictor::Hostile,
                        detail: desc,
                    };
                    fact_sys.record(fact, op, clock.elapsed, &v);
                }
            }

            // Check if all player ships at this system are destroyed
            let surviving = docked_ships.len() - destroyed_ships.len();
            if surviving == 0 {
                // #249: Wipe CombatDefeat — same EventId dedupe suppresses the
                // extra banner when per-ship defeats already surfaced one.
                let event_id = fact_sys.allocate_event_id();
                // #293: hostile_type is gone — reuse the faction-derived label.
                let hostile_label = factions
                    .get(hostile_faction)
                    .map(|f| f.name.clone())
                    .unwrap_or_else(|_| "hostile".to_string());
                let desc = format!(
                    "All ships destroyed by {} at {}",
                    hostile_label, system_name
                );
                events.write(GameEvent {
                    id: event_id,
                    timestamp: clock.elapsed,
                    kind: GameEventKind::CombatDefeat,
                    description: desc.clone(),
                    related_system: Some(*system_entity),
                });
                if let (Some(v), Some(op)) = (vantage, system_pos_arr) {
                    let fact = KnowledgeFact::CombatOutcome {
                        event_id: Some(event_id),
                        system: *system_entity,
                        victor: CombatVictor::Hostile,
                        detail: desc,
                    };
                    fact_sys.record(fact, op, clock.elapsed, &v);
                }
            }
        }
    }
}
