use std::collections::HashMap;

use bevy::prelude::*;
use rand::Rng;

use crate::ai::convert::to_ai_faction;
use crate::ai::emit::AiBusWriter;
use crate::ai::schema::ids::evidence;
use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::faction::{FactionOwner, FactionRelations};
use crate::galaxy::{AtSystem, Hostile, HostileHitpoints, HostileStats, StarSystem};
use crate::knowledge::{
    CombatVictor, DelayedCombatEventQueue, DestroyedShipRecord, DestroyedShipRegistry,
    FactSysParam, KnowledgeFact, PlayerVantage,
};
use crate::player::{AboardShip, Player, StationedAt};
use crate::ship_design::ModuleRegistry;
use crate::time_system::GameClock;
use macrocosmo_ai::StandingEvidence;

use super::combat_sim::{CombatConfig, CombatOutcome, ShipProfile, simulate_combat};
use super::conquered::ConqueredCore;
use super::{
    CoreShip, DockedAt, Owner, RulesOfEngagement, Ship, ShipHitpoints, ShipModifiers, ShipState,
};

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

/// Extract a [`ShipProfile`] from ECS components for use in the pure combat
/// simulation.
/// Map [`RulesOfEngagement`] to a fleet-level retreat HP-fraction threshold.
///
/// - `Aggressive`: 0.25 — fights until badly damaged.
/// - `Defensive`:  0.50 — retreats at half HP.
/// - `Evasive`:    0.75 — retreats early.
/// - `Retreat`:    1.0  — immediate retreat (first turn).
/// - `Passive`:    0.75 — avoids combat; retreats early if forced in.
fn roe_to_retreat_threshold(roe: RulesOfEngagement) -> f64 {
    match roe {
        RulesOfEngagement::Aggressive => 0.25,
        RulesOfEngagement::Defensive => 0.50,
        RulesOfEngagement::Evasive => 0.75,
        RulesOfEngagement::Retreat => 1.0,
        RulesOfEngagement::Passive => 0.75,
    }
}

fn extract_ship_profile(
    index: usize,
    ship: &Ship,
    hp: &ShipHitpoints,
    mods: &ShipModifiers,
    module_registry: &ModuleRegistry,
    is_core: bool,
    is_conquered_core: bool,
    roe: RulesOfEngagement,
) -> ShipProfile {
    let mut weapons = Vec::new();
    for equipped in &ship.modules {
        if let Some(module_def) = module_registry.modules.get(&equipped.module_id) {
            if let Some(weapon) = &module_def.weapon {
                weapons.push(weapon.clone());
            }
        }
    }

    ShipProfile {
        weapons,
        hull: hp.hull,
        hull_max: hp.hull_max,
        armor: hp.armor,
        armor_max: hp.armor_max,
        shield: hp.shield,
        shield_max: hp.shield_max,
        shield_regen: hp.shield_regen,
        evasion: mods.evasion.final_value().to_f64(),
        speed: ship.sublight_speed,
        shield_regen_cooldown: 0,
        index,
        name: ship.name.clone(),
        is_core,
        is_conquered_core,
        retreat_threshold: roe_to_retreat_threshold(roe),
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
            Option<&DockedAt>,
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
    mut bus: AiBusWriter,
    mut destroyed_registry: ResMut<DestroyedShipRegistry>,
    mut delayed_combat_events: ResMut<DelayedCombatEventQueue>,
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
    let ruler_aboard = player_q
        .iter()
        .next()
        .and_then(|(_, _, a)| a.map(|_| ()))
        .is_some();
    let vantage = player_pos.map(|pos| PlayerVantage {
        player_pos: pos,
        ruler_aboard,
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
                |(entity, ship, _hp, _mods, state, roe, _core, _conquered, docked_at)| {
                    // #384: Ships with DockedAt are sheltered — skip combat entirely.
                    if docked_at.is_some() {
                        return None;
                    }
                    let roe = roe.copied().unwrap_or_default();
                    // #57: Retreat, Evasive, Passive ships skip combat
                    if matches!(
                        roe,
                        RulesOfEngagement::Retreat
                            | RulesOfEngagement::Evasive
                            | RulesOfEngagement::Passive
                    ) {
                        return None;
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
                        RulesOfEngagement::Retreat
                        | RulesOfEngagement::Evasive
                        | RulesOfEngagement::Passive => false,
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
            if let Ok((_e, ship, _hp, _mods, _state, _roe, _core, _conquered, _docked)) =
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
            let mut destroyed_ships: Vec<(Entity, String, String)> = Vec::new();

            for &ship_entity in &docked_ships {
                if let Ok((_e, ship, mut hp, _mods, _state, _roe, core, conquered, _docked)) =
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
                        destroyed_ships.push((ship_entity, ship.name.clone(), ship.design_id.clone()));
                    }
                }
            }

            for (entity, name, design_id) in &destroyed_ships {
                // #435: Build the player-facing description up front so the
                // pending record can fire the ShipDestroyed event when light
                // arrives (see `update_destroyed_ship_knowledge`).
                let desc = format!("{} destroyed in combat at {}", name, system_name);
                // #409 / #435: Record destruction for light-speed delayed
                // snapshot update AND event emission.
                if let Some(pos) = system_pos_arr {
                    destroyed_registry.records.push(DestroyedShipRecord {
                        entity: *entity,
                        destruction_pos: pos,
                        destruction_tick: clock.elapsed,
                        name: name.clone(),
                        design_id: design_id.clone(),
                        last_known_system: Some(*system_entity),
                        marked_missing: false,
                        destroyed_description: desc.clone(),
                        event_emitted: false,
                    });
                }
                // #59: Check if player is aboard this ship — respawn at capital.
                // PlayerRespawn is an engine-level event (not a remote
                // observation) and therefore stays immediate.
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
                // #435: The ShipDestroyed `GameEvent` itself is now fired by
                // `update_destroyed_ship_knowledge` once light reaches the
                // player empire's viewer. We still dual-write the
                // `CombatOutcome` fact here because the fact pipeline has its
                // own (relay-aware) light-delay machinery that governs banner
                // delivery.
                let event_id = fact_sys.allocate_event_id();
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
                // #435: Defer the CombatDefeat event until light from the
                // engagement reaches the player empire's viewer.
                if let Some(pos) = system_pos_arr {
                    delayed_combat_events.pending.push(
                        crate::knowledge::DelayedCombatEvent {
                            origin_pos: pos,
                            destruction_tick: clock.elapsed,
                            kind: GameEventKind::CombatDefeat,
                            description: desc.clone(),
                            related_system: Some(*system_entity),
                        },
                    );
                }
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

    // -----------------------------------------------------------------------
    // Ship-vs-ship combat (#399): Faction warfare via simulate_combat.
    // -----------------------------------------------------------------------

    // Group empire-owned, non-docked, non-retreating ships by (system, faction).
    let mut system_faction_ships: HashMap<(Entity, Entity), Vec<Entity>> = HashMap::new();
    for (entity, ship, _hp, _mods, state, roe, _core, _conquered, docked_at) in ships.iter() {
        if docked_at.is_some() {
            continue;
        }
        let roe = roe.copied().unwrap_or_default();
        if matches!(
            roe,
            RulesOfEngagement::Retreat | RulesOfEngagement::Evasive | RulesOfEngagement::Passive
        ) {
            continue;
        }
        let Owner::Empire(faction_entity) = ship.owner else {
            continue;
        };
        let ShipState::InSystem { system } = state else {
            continue;
        };
        system_faction_ships
            .entry((*system, faction_entity))
            .or_default()
            .push(entity);
    }

    // Collect unique systems that have ships from 2+ factions.
    let systems_with_ships: HashMap<Entity, Vec<Entity>> = {
        let mut map: HashMap<Entity, Vec<Entity>> = HashMap::new();
        for &(system, faction) in system_faction_ships.keys() {
            let factions_in_system = map.entry(system).or_default();
            if !factions_in_system.contains(&faction) {
                factions_in_system.push(faction);
            }
        }
        map
    };

    for (system_entity, factions_present) in &systems_with_ships {
        if factions_present.len() < 2 {
            continue;
        }

        let (system_name, system_pos_arr): (String, Option<[f64; 3]>) = systems
            .get(*system_entity)
            .map(|(_, s, p)| (s.name.clone(), Some(p.as_array())))
            .unwrap_or_default();

        // Check each pair of factions for war.
        for i in 0..factions_present.len() {
            for j in (i + 1)..factions_present.len() {
                let faction_a = factions_present[i];
                let faction_b = factions_present[j];

                let view = relations.get_or_default(faction_a, faction_b);
                if !view.is_at_war() {
                    continue;
                }

                let ships_a = match system_faction_ships.get(&(*system_entity, faction_a)) {
                    Some(s) => s.clone(),
                    None => continue,
                };
                let ships_b = match system_faction_ships.get(&(*system_entity, faction_b)) {
                    Some(s) => s.clone(),
                    None => continue,
                };

                if ships_a.is_empty() || ships_b.is_empty() {
                    continue;
                }

                // Extract profiles for both sides.
                let mut profiles_a: Vec<ShipProfile> = Vec::new();
                for (idx, &entity) in ships_a.iter().enumerate() {
                    if let Ok((_e, ship, hp, mods, _state, roe, core, conquered, _docked)) =
                        ships.get(entity)
                    {
                        profiles_a.push(extract_ship_profile(
                            idx,
                            ship,
                            hp,
                            mods,
                            &module_registry,
                            core.is_some(),
                            core.is_some() && conquered.is_some(),
                            roe.copied().unwrap_or_default(),
                        ));
                    }
                }

                let mut profiles_b: Vec<ShipProfile> = Vec::new();
                for (idx, &entity) in ships_b.iter().enumerate() {
                    if let Ok((_e, ship, hp, mods, _state, roe, core, conquered, _docked)) =
                        ships.get(entity)
                    {
                        profiles_b.push(extract_ship_profile(
                            idx,
                            ship,
                            hp,
                            mods,
                            &module_registry,
                            core.is_some(),
                            core.is_some() && conquered.is_some(),
                            roe.copied().unwrap_or_default(),
                        ));
                    }
                }

                if profiles_a.is_empty() || profiles_b.is_empty() {
                    continue;
                }

                // Run the simulation.
                let config = CombatConfig::default();
                let log = simulate_combat(&mut profiles_a, &mut profiles_b, &config, &mut rng);

                // --- Emit standing evidence to AI bus ---
                let ai_faction_a = to_ai_faction(faction_a);
                let ai_faction_b = to_ai_faction(faction_b);
                let now = bus.now();

                // Both sides observe a direct attack from the other.
                bus.emit_evidence(StandingEvidence::new(
                    evidence::direct_attack(),
                    ai_faction_a, // observer: faction A sees B attacking
                    ai_faction_b, // target: faction B is the attacker
                    1.0,
                    now,
                ));
                bus.emit_evidence(StandingEvidence::new(
                    evidence::direct_attack(),
                    ai_faction_b, // observer: faction B sees A attacking
                    ai_faction_a, // target: faction A is the attacker
                    1.0,
                    now,
                ));

                // Both sides observe hostile engagement.
                bus.emit_evidence(StandingEvidence::new(
                    evidence::hostile_engagement(),
                    ai_faction_a,
                    ai_faction_b,
                    1.0,
                    now,
                ));
                bus.emit_evidence(StandingEvidence::new(
                    evidence::hostile_engagement(),
                    ai_faction_b,
                    ai_faction_a,
                    1.0,
                    now,
                ));

                // --- Write results back to ECS ---

                // Apply HP changes from profiles_a (faction A ships).
                let mut destroyed_a: Vec<(Entity, String, String)> = Vec::new();
                for profile in &profiles_a {
                    let entity = ships_a[profile.index];
                    if let Ok((_e, ship, mut hp, _mods, _state, _roe, core, _conquered, _docked)) =
                        ships.get_mut(entity)
                    {
                        hp.hull = profile.hull;
                        hp.armor = profile.armor;
                        hp.shield = profile.shield;
                        if hp.hull <= 0.0 && core.is_none() {
                            destroyed_a.push((entity, ship.name.clone(), ship.design_id.clone()));
                        }
                    }
                }

                // Apply HP changes from profiles_b (faction B ships).
                let mut destroyed_b: Vec<(Entity, String, String)> = Vec::new();
                for profile in &profiles_b {
                    let entity = ships_b[profile.index];
                    if let Ok((_e, ship, mut hp, _mods, _state, _roe, core, _conquered, _docked)) =
                        ships.get_mut(entity)
                    {
                        hp.hull = profile.hull;
                        hp.armor = profile.armor;
                        hp.shield = profile.shield;
                        if hp.hull <= 0.0 && core.is_none() {
                            destroyed_b.push((entity, ship.name.clone(), ship.design_id.clone()));
                        }
                    }
                }

                // Emit fleet_loss evidence for each destroyed ship.
                // Magnitude scales with the number of ships lost.
                if !destroyed_a.is_empty() {
                    bus.emit_evidence(StandingEvidence::new(
                        evidence::fleet_loss(),
                        ai_faction_a, // observer: faction A lost ships
                        ai_faction_b, // target: faction B caused the loss
                        destroyed_a.len() as f64,
                        now,
                    ));
                }
                if !destroyed_b.is_empty() {
                    bus.emit_evidence(StandingEvidence::new(
                        evidence::fleet_loss(),
                        ai_faction_b, // observer: faction B lost ships
                        ai_faction_a, // target: faction A caused the loss
                        destroyed_b.len() as f64,
                        now,
                    ));
                }

                // Despawn destroyed ships + emit events.
                let faction_a_name = factions
                    .get(faction_a)
                    .map(|f| f.name.clone())
                    .unwrap_or_else(|_| "Unknown".to_string());
                let faction_b_name = factions
                    .get(faction_b)
                    .map(|f| f.name.clone())
                    .unwrap_or_else(|_| "Unknown".to_string());

                for (entity, name, design_id) in &destroyed_a {
                    // #435: Build description up front for the light-delayed event.
                    let desc = format!(
                        "{} ({}) destroyed by {} at {}",
                        name, faction_a_name, faction_b_name, system_name
                    );
                    // #409 / #435: Record destruction for light-speed delayed
                    // snapshot update AND ShipDestroyed event emission.
                    if let Some(pos) = system_pos_arr {
                        destroyed_registry.records.push(DestroyedShipRecord {
                            entity: *entity,
                            destruction_pos: pos,
                            destruction_tick: clock.elapsed,
                            name: name.clone(),
                            design_id: design_id.clone(),
                            last_known_system: Some(*system_entity),
                            marked_missing: false,
                            destroyed_description: desc.clone(),
                            event_emitted: false,
                        });
                    }
                    // Check if player is aboard. PlayerRespawn stays immediate
                    // (engine-level, not a remote observation).
                    if let Ok((player_entity, mut stationed, aboard)) = player_q.single_mut() {
                        if let Some(aboard_ship) = aboard {
                            if aboard_ship.ship == *entity {
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
                    // #435: ShipDestroyed event is deferred; the fact pipeline
                    // keeps its own light-delay path for the banner side.
                    let event_id = fact_sys.allocate_event_id();
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

                for (entity, name, design_id) in &destroyed_b {
                    // #435: Build description up front for the light-delayed event.
                    let desc = format!(
                        "{} ({}) destroyed by {} at {}",
                        name, faction_b_name, faction_a_name, system_name
                    );
                    // #409 / #435: Record destruction for light-speed delayed
                    // snapshot update AND ShipDestroyed event emission.
                    if let Some(pos) = system_pos_arr {
                        destroyed_registry.records.push(DestroyedShipRecord {
                            entity: *entity,
                            destruction_pos: pos,
                            destruction_tick: clock.elapsed,
                            name: name.clone(),
                            design_id: design_id.clone(),
                            last_known_system: Some(*system_entity),
                            marked_missing: false,
                            destroyed_description: desc.clone(),
                            event_emitted: false,
                        });
                    }
                    if let Ok((player_entity, mut stationed, aboard)) = player_q.single_mut() {
                        if let Some(aboard_ship) = aboard {
                            if aboard_ship.ship == *entity {
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
                    // #435: ShipDestroyed event is deferred; the fact pipeline
                    // keeps its own light-delay path for the banner side.
                    let event_id = fact_sys.allocate_event_id();
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

                // Emit victory event based on outcome.
                match &log.outcome {
                    CombatOutcome::AttackerWon { .. } => {
                        let event_id = fact_sys.allocate_event_id();
                        let desc = format!(
                            "{} victorious over {} at {}",
                            faction_a_name, faction_b_name, system_name
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
                    }
                    CombatOutcome::DefenderWon { .. } => {
                        let event_id = fact_sys.allocate_event_id();
                        let desc = format!(
                            "{} victorious over {} at {}",
                            faction_b_name, faction_a_name, system_name
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
                    }
                    CombatOutcome::AttackerRetreated { .. } => {
                        let event_id = fact_sys.allocate_event_id();
                        let desc = format!(
                            "{} retreated from {} at {}",
                            faction_a_name, faction_b_name, system_name
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
                    CombatOutcome::DefenderRetreated { .. } => {
                        let event_id = fact_sys.allocate_event_id();
                        let desc = format!(
                            "{} retreated from {} at {}",
                            faction_b_name, faction_a_name, system_name
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
                    }
                    CombatOutcome::MutualRetreat => {
                        // Both sides retreated — no victor.
                    }
                    CombatOutcome::Stalemate => {} // No event for stalemate.
                }
            }
        }
    }
}
